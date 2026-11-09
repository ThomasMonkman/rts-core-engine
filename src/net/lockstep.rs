//! Lockstep over the mesh: every client runs the whole sim and only player
//! input crosses the network.
//!
//! **Turns.** At the start of each tick `t` every client sends one turn to
//! every other: the commands its player gave since its last turn, stamped
//! to run on tick `sched = t + delay`, and by being sent at all it says
//! "nothing more from me up to `sched`". A client may run tick `T` once it
//! holds a turn with `sched >= T` from everyone (its *watermark* for that
//! peer): then it has every command for `T`, from every client, and applies
//! them in `(tick, client, seq)` order, the same on every machine. Commands
//! sent in tick `t` run `delay` ticks later everywhere, the sender included.
//!
//! **The delay** is what hides the network: a turn has `delay` ticks of
//! real time to reach everyone before it is needed. Too short and clients
//! stall waiting for turns (stutter); too long and orders feel laggy. Each
//! client picks its own from what it measures, as a whole number of ticks:
//! `ceil((max RTT / 2 + JITTER_MS) / tick length)`, at least
//! [`MIN_DELAY`]. The tick length is real time at the current game speed
//! (100 ms at normal speed, 50 ms at +1), so a faster game takes more ticks
//! of delay for the same network. `JITTER_MS` (default 100) covers what an
//! RTT average hides: a client's slow tick, a Lua collection, the OS
//! scheduler. On a LAN that gives 2 ticks (200 ms) at normal speed; SupCom
//! used a fixed 500 ms (`net_Lag`). Clients needn't agree on it: each
//! turn carries its own `sched`. The delay rises at once when needed and
//! falls a tick at a time (a client skips sending for a tick to fall by
//! one), which keeps every client's `sched`s strictly increasing.
//!
//! **Slow machines slow the game, smoothly.** Lockstep already makes
//! everyone wait for the slowest client, but as a stutter (the fast ones
//! run a tick, stall, run, stall). Instead, as SupCom does, the game slows
//! to a speed every machine can sustain: each client measures what its
//! ticks cost (smoothed, waiting for peers excluded) and puts the fastest
//! speed it could keep up with, with [`HEADROOM`], in every turn
//! (`capacity`). Every client paces at the lowest of the speed the players
//! set, its own capacity and every peer's: the game slows together when a
//! machine struggles and speeds up again when it recovers. Pacing is
//! real time only, never sim state, so it can't desync.
//!
//! **Falling behind.** A client that started late (slow loading) or had a
//! hitch runs behind the others, and they then wait for its turns every
//! tick: smooth, but their orders take that much longer. Turns carry the
//! sender's current tick, so each client can tell how far behind the
//! others it is (allowing for the turn's travel time); one that is behind
//! everyone runs its ticks faster ([`Lockstep::pace_factor`]) until it has
//! caught up.
//!
//! **Desync detection.** Every tick's sim checksum goes out in the next turn
//! (as moho: `sim_ChecksumPeriod` 1), so a mismatch names the first tick
//! that differs, for `RTS_DUMP_TICK` hunting. It is logged, shown (FAF's
//! desync dialog), and the first one with each peer swaps the checksum's
//! parts (`ChecksumParts`): the log says which parts differ (entities,
//! motion, orders, economy...).
//!
//! **A peer leaving.** A peer whose connection is gone (it left, timed out
//! or was ejected) is dropped by the engine, as moho does: FAF's disconnect
//! dialog only offers Eject for clients still connected, so nobody could
//! vote it out. A peer still connected but silent (lagging, frozen) stays
//! until everyone ejects it. Its last turns may have reached some clients
//! and not others, and they must all apply the same ones. Each client announces the
//! watermark it holds for the lost peer (`Dropped`); the highest is the cut.
//! A client holding the turns up to the cut relays them (`Forward`) to the
//! others; once everyone has the turns up to the cut, they stop waiting for
//! the peer after it. Its army stays in the game, taking no more orders.
//!
//! The sim thread runs this inside the tick's input stage. Waiting for
//! turns only ever delays a tick; it never changes what the tick does.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mlua::prelude::*;

use super::core::{Event, NetCore};
use super::wire::Msg;
use crate::engine::sim::input::{CommandStream, PlayerCommand, Scheduled};

/// Fewest ticks of input delay.
pub const MIN_DELAY: u64 = 2;
/// Most ticks of input delay (5 s at normal speed).
pub const MAX_DELAY: u64 = 50;
/// Ticks of our checksums kept to compare with peers' (moho keeps 128).
const CHECKSUMS_KEPT: usize = 128;
/// Turns kept per peer to relay if another peer drops.
const HISTORY: usize = 600;
/// Ticks between delay re-evaluations.
const ADAPT_EVERY: u64 = 10;
/// A machine counts as keeping up at a speed if its ticks take at most
/// 1 / HEADROOM of the tick's real time (spare for spikes: collections,
/// big fights).
pub const HEADROOM: f64 = 1.25;
/// Smoothing of the measured tick cost (per tick): ~ the last 50 ticks.
const COST_SMOOTHING: f64 = 0.02;
/// Faster smoothing for the first ticks measured: the game's first ticks
/// are slow one-offs (commanders created, unit scripts loaded for the first
/// time), and at [`COST_SMOOTHING`] one such tick held a game at a quarter
/// speed for ~30 s. This lets them fade within a dozen ticks; a machine
/// that is slow throughout still measures slow.
const WARMUP_COST_SMOOTHING: f64 = 0.25;
/// How many ticks [`WARMUP_COST_SMOOTHING`] applies to (5 s of game time).
const WARMUP_TICKS: u32 = 50;
/// Capacities are rounded down to this many percent, so tiny cost changes
/// don't keep moving the game speed.
const CAPACITY_STEP: u32 = 5;

/// The tick-cost average after a tick costing `cost` ms, `ticks_measured`
/// ticks into the game ([`WARMUP_COST_SMOOTHING`] for the first
/// [`WARMUP_TICKS`], then [`COST_SMOOTHING`]).
pub fn smoothed_cost(previous: Option<f64>, cost: f64, ticks_measured: u32) -> f64 {
    let smoothing = if ticks_measured < WARMUP_TICKS { WARMUP_COST_SMOOTHING } else { COST_SMOOTHING };
    previous.map_or(cost, |c| c + (cost - c) * smoothing)
}

/// The fastest game speed (percent) a machine whose ticks cost `cost_ms`
/// keeps up with.
pub fn capacity_for(cost_ms: f64) -> u32 {
    let tick_budget_ms = cost_ms.max(0.01) * HEADROOM;
    let percent = (100.0 * 100.0 / tick_budget_ms).min(100_000.0) as u32;
    (percent / CAPACITY_STEP * CAPACITY_STEP).max(CAPACITY_STEP)
}

/// Most ticks a delay rises by at once (a player still loading or a single
/// freak spike mustn't put seconds of lag on everyone; repeated waits keep
/// raising it).
const MAX_RISE: u64 = 3;
/// After a rise, the delay holds this long before it may fall again.
const HOLD: Duration = Duration::from_secs(15);
/// Spare time (ms) on top of the slowest one-way trip.
const MARGIN_MS: f64 = 30.0;
/// A tick that waited this long for players' turns is logged as a hitch.
const HITCH_MS: f64 = 20.0;

/// The input delay (ticks) for a one-way trip (ms, spikes included), game
/// speed (percent) and margin (ms).
pub fn delay_for(one_way_ms: f64, speed_percent: u32, margin_ms: f64) -> u64 {
    let tick_ms = 100.0 * 100.0 / speed_percent.clamp(1, 10_000) as f64;
    (((one_way_ms + margin_ms) / tick_ms).ceil() as u64).clamp(MIN_DELAY, MAX_DELAY)
}

/// The slowest one-way trip to the peers (ms), spikes included: half the
/// round trip, or the 99th percentile round trip less the usual return
/// leg (a spike can be all one way).
pub fn one_way_ms(peers: &[super::core::PeerInfo]) -> f64 {
    peers.iter().map(|p| (p.ping99_ms - p.ping_ms / 2.0).max(p.ping_ms / 2.0)).fold(0.0, f64::max)
}

/// The margin over the one-way trip (`RTS_NET_JITTER_MS`, else
/// [`MARGIN_MS`]).
pub fn margin_ms() -> f64 {
    std::env::var("RTS_NET_JITTER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(MARGIN_MS)
}

struct Remote {
    name: String,
    /// The command source its commands run as.
    source: u32,
    /// Every turn up to here has arrived.
    watermark: u64,
    connected: bool,
    /// Last turns, for relaying if another peer drops.
    history: VecDeque<(u64, Vec<String>)>,
    /// Lost mid-game: the agreed last turn once settled.
    cut: Option<u64>,
    /// Being dropped (our claim sent).
    dropping: bool,
    /// Lost: the watermark each remaining client reported holding.
    drop_claims: BTreeMap<u32, u64>,
}

/// Running totals for logs and the handover.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub turns_sent: u64,
    pub turns_received: u64,
    pub commands_sent: u64,
    pub ticks_waited: u64,
    pub wait_ms_total: f64,
    pub wait_ms_max: f64,
    pub desyncs: u64,
    pub checksums_matched: u64,
}

/// Lockstep state (sim VM app data in a networked game).
pub struct Lockstep {
    net: Arc<NetCore>,
    local_peer: u32,
    local_source: u32,
    remotes: BTreeMap<u32, Remote>,
    /// `sched` of our last turn.
    last_sched: u64,
    delay: u64,
    margin_ms: f64,
    /// This window (`ADAPT_EVERY` ticks): per peer, the longest we waited
    /// for its turn (ms) and its turns' fewest ticks to spare.
    waited_on: BTreeMap<u32, f64>,
    spare: BTreeMap<u32, i64>,
    /// Our report of the last window, for our next turn.
    slack_out: Vec<(u32, i32)>,
    /// Peers' reports on our turns since we last adapted (worst each).
    slack_in: BTreeMap<u32, i32>,
    /// The delay doesn't fall before this (just raised).
    hold_until: Instant,
    /// Each peer's input delay (ticks), from its last turn.
    peer_delays: BTreeMap<u32, u64>,
    /// Reports before this tick describe turns sent before our last rise:
    /// ignored, or one late spell would raise the delay again and again.
    settle_until: u64,
    /// The game speed every client applied (percent): changes are commands
    /// (`Toggle::GameSpeed`), applied at their tick by everyone.
    speed_percent: u32,
    /// The lobby's Game Speed option allows changes (`adjustable`); `normal`
    /// and `fast` fix the speed.
    speed_adjustable: bool,
    /// The step we asked for and haven't seen applied yet.
    speed_requested: Option<i32>,
    next_seq: u32,
    /// Commands taken while holding back a turn (the delay falling).
    held: Vec<PlayerCommand>,
    /// What our ticks cost (ms, smoothed, waiting for peers excluded).
    cost_ms: Option<f64>,
    /// Ticks measured into `cost_ms` so far (up to [`WARMUP_TICKS`]).
    ticks_measured: u32,
    /// Time this tick spent waiting for peers (ms).
    waited_ms: f64,
    /// Each peer's tick when it sent its last turn, and when that arrived.
    peer_at: BTreeMap<u32, (u64, Instant)>,
    /// Ticks behind the furthest-behind peer, as last estimated.
    behind: f64,
    /// The tick we last started, and when.
    started: (u64, Instant),
    /// Each peer's last reported capacity (percent).
    capacities: BTreeMap<u32, u32>,
    /// The speed the game runs at now (percent): the players' setting or
    /// the slowest machine's capacity, whichever is lower.
    effective: u32,
    /// Our checksum after each recent tick, and its parts.
    ours: BTreeMap<u64, u64>,
    our_parts: BTreeMap<u64, crate::engine::sim::checksum::Parts>,
    /// Our checksums not sent yet (held while the input delay falls).
    unsent: Vec<(u64, u64)>,
    /// Peers we've sent our parts of a desynced tick to (once each).
    parts_sent: BTreeSet<u32>,
    /// Peers' checksums we can't check yet (we haven't run that tick).
    theirs: BTreeMap<(u64, u32), u64>,
    pub stats: Stats,
    /// Pause set by another player, for the real-time loop to apply.
    pub pause_request: Option<bool>,
    /// The game is paused (by anyone).
    pub paused: bool,
    /// Eject votes: target peer -> the peers that voted.
    votes: BTreeMap<u32, BTreeSet<u32>>,
    /// The session's control: waiting for players' turns ends when the
    /// player leaves or the session closes (`SimControl::left`).
    stop: Option<Arc<crate::engine::snapshot::SimControl>>,
    log_every: Instant,
}

impl Lockstep {
    /// `peers`: every other client's peer id, command source and name.
    /// `start_tick` is the first tick that will run and `delay` the input
    /// delay everyone starts with (from the launch).
    pub fn new(net: Arc<NetCore>, local_peer: u32, local_source: u32, peers: Vec<(u32, u32, String)>, start_tick: u64, delay: u64) -> Self {
        let remotes = peers
            .into_iter()
            .map(|(id, source, name)| {
                // Nothing can be due before the first turns' `sched`.
                let r = Remote { name, source, watermark: start_tick + delay - 1, connected: true, history: VecDeque::new(), cut: None, dropping: false, drop_claims: BTreeMap::new() };
                (id, r)
            })
            .collect();
        let margin_ms = margin_ms();
        tracing::info!(local_peer, local_source, delay, start_tick, margin_ms, "lockstep: started");
        Lockstep {
            net,
            local_peer,
            local_source,
            remotes,
            last_sched: start_tick + delay - 1,
            delay,
            margin_ms,
            waited_on: BTreeMap::new(),
            spare: BTreeMap::new(),
            slack_out: Vec::new(),
            slack_in: BTreeMap::new(),
            hold_until: Instant::now(),
            peer_delays: BTreeMap::new(),
            settle_until: 0,
            speed_percent: 100,
            speed_adjustable: true,
            speed_requested: None,
            next_seq: 0,
            held: Vec::new(),
            cost_ms: None,
            ticks_measured: 0,
            waited_ms: 0.0,
            capacities: BTreeMap::new(),
            peer_at: BTreeMap::new(),
            behind: 0.0,
            started: (start_tick, Instant::now()),
            effective: 100,
            ours: BTreeMap::new(),
            our_parts: BTreeMap::new(),
            unsent: Vec::new(),
            parts_sent: BTreeSet::new(),
            theirs: BTreeMap::new(),
            stats: Stats::default(),
            pause_request: None,
            paused: false,
            votes: BTreeMap::new(),
            stop: None,
            log_every: Instant::now(),
        }
    }

    pub fn delay(&self) -> u64 {
        self.delay
    }

    /// Waiting for players' turns gives up once `control` says the player
    /// left or the session closed: otherwise a game stalled on a peer would
    /// never let the engine thread see it, and the front end never loads.
    pub fn set_stop(&mut self, control: Arc<crate::engine::snapshot::SimControl>) {
        self.stop = Some(control);
    }

    fn stopped(&self) -> bool {
        self.stop.as_ref().is_some_and(|c| c.left())
    }

    /// The game was paused or resumed here: everyone follows.
    pub fn set_paused(&mut self, paused: bool) {
        if paused != self.paused {
            tracing::info!(paused, "lockstep: game paused here, telling everyone");
            self.paused = paused;
            self.net.send_pause(paused);
            self.net.push_pause_event(paused, self.local_peer);
        }
    }

    /// Takes in network events while no tick runs (paused).
    pub fn pump(&mut self, lua: &Lua) {
        let now = crate::engine::api::sim_time::current_tick(lua);
        let events = self.net.poll();
        self.take(lua, now, events);
    }

    /// A tick took `step_ms` of wall time (waiting for peers included; the
    /// lockstep subtracts it).
    pub fn note_tick(&mut self, step_ms: f64) {
        let cost = (step_ms - self.waited_ms).max(0.0);
        self.cost_ms = Some(smoothed_cost(self.cost_ms, cost, self.ticks_measured));
        self.ticks_measured = (self.ticks_measured + 1).min(WARMUP_TICKS);
    }

    /// How much of a normal tick period to wait before the next tick: below
    /// 1 when this client is behind every other one (it catches up), else 1.
    /// `tick_ms` is the real-time tick length. Positions compare like with
    /// like: the tick a client last started plus the time since, in ticks.
    pub fn pace_factor(&mut self, tick_ms: f64) -> f64 {
        let tick_ms = tick_ms.max(1.0);
        let ours = self.started.0 as f64 + self.started.1.elapsed().as_secs_f64() * 1000.0 / tick_ms;
        let half_rtt: BTreeMap<u32, f64> = self.net.peers().into_iter().map(|p| (p.id, p.ping_ms / 2.0)).collect();
        // Where each peer is by now: its last turn's tick, plus the ticks
        // it has run since (the turn's travel time and the time since).
        let ahead = self
            .peer_at
            .iter()
            .filter(|(id, _)| self.remotes.get(id).is_some_and(|r| r.connected))
            .map(|(id, (at, when))| {
                let since = when.elapsed().as_secs_f64() * 1000.0 + half_rtt.get(id).copied().unwrap_or(0.0);
                *at as f64 + since / tick_ms - ours
            })
            .fold(f64::INFINITY, f64::min);
        self.behind = if ahead.is_finite() { ahead } else { 0.0 };
        // Behind everyone by more than half a tick: up to twice as fast.
        if self.behind > 0.5 { (1.0 / (1.0 + (self.behind - 0.5) * 0.25)).max(0.5) } else { 1.0 }
    }

    /// How long the last tick waited for players' turns (ms).
    pub fn last_wait_ms(&self) -> f64 {
        self.waited_ms
    }

    /// The fastest speed this machine keeps up with (percent).
    pub fn capacity(&self) -> u32 {
        self.cost_ms.map_or(100_000, capacity_for)
    }

    /// The speed to run at (percent) when the players asked for
    /// `requested`: lower if any machine can't keep up.
    pub fn effective_speed(&mut self, requested: u32) -> u32 {
        let slowest = self
            .capacities
            .iter()
            .filter(|(id, _)| self.remotes.get(id).is_some_and(|r| r.connected))
            .map(|(_, c)| *c)
            .chain([self.capacity()])
            .min()
            .unwrap_or(u32::MAX);
        let speed = requested.min(slowest);
        if speed != self.effective {
            let limited_by = if speed < requested {
                if self.capacity() == slowest { "this machine".to_owned() } else {
                    let id = self.capacities.iter().filter(|(_, c)| **c == slowest).map(|(id, _)| *id).next();
                    id.and_then(|id| self.remotes.get(&id)).map_or("?".to_owned(), |r| r.name.clone())
                }
            } else {
                "nobody".to_owned()
            };
            tracing::info!(from = self.effective, to = speed, requested, limited_by, "lockstep: game speed");
            self.effective = speed;
        }
        speed
    }

    /// The lobby's Game Speed option lets players change it.
    pub fn speed_adjustable(&self) -> bool {
        self.speed_adjustable
    }

    /// The game speed every client applied (percent).
    pub fn agreed_speed(&self) -> u32 {
        self.speed_percent
    }

    /// This player asked for speed `percent` (`+` / `-`, `SetGameSpeed`):
    /// it goes to everyone as a command in our next turn, and applies at
    /// that tick everywhere. False if the lobby fixed the game speed.
    pub fn request_speed(&mut self, percent: u32) -> bool {
        if !self.speed_adjustable {
            return false;
        }
        let step = crate::engine::snapshot::SimControl::step_of(percent);
        if self.speed_requested != Some(step) {
            tracing::info!(step, "lockstep: asking everyone for a game speed");
            self.speed_requested = Some(step);
            self.held.push(speed_command(step));
        }
        true
    }

    /// Start of tick `now` (the input stage): sends our turn with `local`
    /// (the player's new commands), waits until every peer's commands for
    /// `now` are in, and puts every command received into `stream`.
    pub fn tick(&mut self, lua: &Lua, now: u64, local: Vec<PlayerCommand>) {
        self.started = (now, Instant::now());
        // Dev: `RTS_NET_SLOW_MS=n` makes this machine's ticks n ms slower
        // (testing how the game slows for a slow client).
        static SLOW: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let slow = *SLOW.get_or_init(|| std::env::var("RTS_NET_SLOW_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
        if slow > 0 {
            std::thread::sleep(Duration::from_millis(slow));
        }
        // Our checksum after the previous tick, to compare and to send.
        if now > 0 {
            let parts = crate::engine::sim::checksum::parts_of_lua(lua);
            let c = crate::engine::sim::checksum::combine(&parts);
            self.ours.insert(now - 1, c);
            self.our_parts.insert(now - 1, parts);
            while self.ours.len() > CHECKSUMS_KEPT {
                self.ours.pop_first();
            }
            while self.our_parts.len() > CHECKSUMS_KEPT {
                self.our_parts.pop_first();
            }
            self.unsent.push((now - 1, c));
        }
        if now.is_multiple_of(ADAPT_EVERY) {
            self.report_slack();
            self.adapt(now);
        }
        self.held.extend(local);
        let want = now + self.delay;
        if want > self.last_sched {
            let sched = want;
            let commands = std::mem::take(&mut self.held);
            let mut lines = Vec::with_capacity(commands.len());
            if let Some(mut stream) = lua.app_data_mut::<CommandStream>() {
                for command in commands {
                    let s = Scheduled { tick: sched, client: self.local_source, seq: self.next_seq, command };
                    self.next_seq = self.next_seq.wrapping_add(1);
                    let line = crate::engine::sim::input::encode(&s);
                    // What the peers read from the line, not what we meant:
                    // a field the line can't carry then differs nowhere.
                    match crate::engine::sim::input::decode(&line) {
                        Ok(read) => {
                            stream.schedule(read);
                            lines.push(line);
                        }
                        Err(e) => tracing::warn!(e, "lockstep: dropped a command its line can't carry"),
                    }
                }
            }
            self.stats.commands_sent += lines.len() as u64;
            self.stats.turns_sent += 1;
            self.last_sched = sched;
            let capacity = self.capacity();
            let slack = std::mem::take(&mut self.slack_out);
            self.net.set_max_speed(self.local_peer, capacity);
            self.net.set_input_delay(self.local_peer, self.delay as u32);
            let checksums = std::mem::take(&mut self.unsent);
            self.net.broadcast(&Msg::Turn { sched, at: now, commands: lines, checksums, capacity, slack });
        }
        // Else holding this tick's turn (the delay is falling): the
        // checksums wait in `unsent` for the next one.
        self.check_checksums();
        // Wait for everyone's turns for `now`.
        let started = Instant::now();
        let mut said = Instant::now();
        let mut last = Instant::now();
        let watchdog = lua.app_data_ref::<Arc<crate::engine::watchdog::Watchdog>>().map(|w| Arc::clone(&w));
        let mut waited_on: BTreeMap<u32, f64> = BTreeMap::new();
        let mut missing: Vec<u32> = Vec::new();
        loop {
            let events = self.net.poll();
            self.take(lua, now, events);
            // The time since the last look goes to whoever we were missing
            // then (their report, and the hitch log).
            let dt = last.elapsed().as_secs_f64() * 1000.0;
            last = Instant::now();
            for id in &missing {
                *waited_on.entry(*id).or_default() += dt;
            }
            missing = self.remotes.iter().filter(|(_, r)| r.connected && !self.ready(r, now)).map(|(id, _)| *id).collect();
            let waiting: Vec<&str> = self.remotes.values().filter(|r| !self.ready(r, now)).map(|r| r.name.as_str()).collect();
            if waiting.is_empty() {
                break;
            }
            // The player left (score screen, quit) or the session closed:
            // this client runs no more ticks, so stop waiting. The tick
            // finishes without the missing turns, on this client only,
            // which nobody sees (no more turns or checksums go out).
            if self.stopped() {
                tracing::warn!(tick = now, ?waiting, "lockstep: stopped waiting for players (the player left)");
                break;
            }
            if said.elapsed() > Duration::from_secs(5) {
                tracing::warn!(tick = now, ?waiting, "lockstep: waiting for players");
                said = Instant::now();
            }
            if let Some(w) = &watchdog {
                w.beat();
            }
            let events = self.net.wait(Duration::from_millis(20));
            self.take(lua, now, events);
        }
        let waited = started.elapsed().as_secs_f64() * 1000.0;
        self.waited_ms = waited;
        // A hitch: the game stood still waiting for someone's turn.
        if waited >= HITCH_MS {
            let who: Vec<String> = waited_on
                .iter()
                .filter(|(_, ms)| **ms >= 1.0)
                .map(|(id, ms)| format!("{} {:.0} ms", self.remotes.get(id).map_or("?", |r| r.name.as_str()), ms))
                .collect();
            tracing::warn!(tick = now, waited_ms = format!("{waited:.0}"), our_delay = self.delay, waiting_for = %who.join(", "), "hitch: the game waited for players' turns");
        }
        for (id, ms) in waited_on {
            let w = self.waited_on.entry(id).or_default();
            *w = w.max(ms);
        }
        if waited > 1.0 {
            self.stats.ticks_waited += 1;
            self.stats.wait_ms_total += waited;
            self.stats.wait_ms_max = self.stats.wait_ms_max.max(waited);
        }
        if self.log_every.elapsed() > Duration::from_secs(10) {
            self.log_every = Instant::now();
            self.log_players(now);
            let pings: Vec<String> = self.net.peers().iter().map(|p| format!("{}={:.1}ms", p.name, p.ping_ms)).collect();
            tracing::info!(tick = now, delay = self.delay, speed = self.effective, behind = format!("{:.1}", self.behind), capacity = self.capacity(), peer_capacities = ?self.capacities, cost_ms = self.cost_ms.unwrap_or(0.0), stats = ?self.stats, ?pings, "lockstep");
        }
    }

    /// Every 10 s, for bug reports: each player's ping (and its spikes),
    /// the top game speed their machine keeps up with, and their input
    /// delay; then ours.
    fn log_players(&self, now: u64) {
        let tick_ms = self.tick_ms();
        let x = |percent: u32| format!("{:.2}x", percent as f64 / 100.0);
        let peers = self.net.peers();
        let mut lines: Vec<String> = self
            .remotes
            .iter()
            .map(|(id, r)| {
                let ping = peers.iter().find(|p| p.id == *id).map_or("not connected".to_string(), |p| format!("ping {:.0} ms (spikes {:.0})", p.ping_ms, p.ping99_ms));
                let top = self.capacities.get(id).map_or("?".to_string(), |&c| x(c));
                let delay = self.peer_delays.get(id).map_or("?".to_string(), |&d| format!("{:.0} ms", d as f64 * tick_ms));
                format!("{}: {ping}, top speed {top}, input delay {delay}", r.name)
            })
            .collect();
        lines.push(format!("us: top speed {}, input delay {:.0} ms", x(self.capacity()), self.delay as f64 * tick_ms));
        tracing::info!(tick = now, game_speed = %x(self.effective), "players: {}", lines.join(" | "));
    }

    fn ready(&self, r: &Remote, now: u64) -> bool {
        r.watermark >= now || r.cut.is_some_and(|cut| cut < now && r.watermark >= cut)
    }

    /// Real time per tick now (ms).
    fn tick_ms(&self) -> f64 {
        100.0 * 100.0 / self.speed_percent.min(self.effective).clamp(1, 10_000) as f64
    }

    /// Our report on the last window's turns from each peer, sent with our
    /// next turn: the ticks we waited for them (negative), else the whole
    /// ticks they had to spare (a turn is read at the start of a tick, so
    /// one less than it seemed).
    fn report_slack(&mut self) {
        let tick_ms = self.tick_ms();
        let mut out = Vec::new();
        for (&id, r) in &self.remotes {
            if !r.connected {
                continue;
            }
            let waited = self.waited_on.get(&id).copied().unwrap_or(0.0);
            // Waits under a millisecond are the poll's own timing.
            let slack = if waited >= 1.0 {
                -((waited / tick_ms).ceil() as i32).max(1)
            } else if let Some(&spare) = self.spare.get(&id) {
                (spare - 1).clamp(0, 1000) as i32
            } else {
                continue;
            };
            out.push((id, slack));
        }
        self.waited_on.clear();
        self.spare.clear();
        self.slack_out = out;
    }

    /// Picks our input delay: up at once when anyone waited for our turns
    /// (by the ticks they waited, at most [`MAX_RISE`]) or the links' trip
    /// times need it; down a tick at a time only after [`HOLD`] with two
    /// ticks to spare everywhere. A report of 0 (just in time) holds it.
    fn adapt(&mut self, tick: u64) {
        let peers = self.net.peers();
        let one_way = one_way_ms(&peers);
        let target = delay_for(one_way, self.speed_percent.min(self.effective), self.margin_ms);
        let worst = std::mem::take(&mut self.slack_in).into_values().min().filter(|_| tick >= self.settle_until);
        let now = Instant::now();
        let (next, why) = match worst {
            Some(w) if w < 0 => {
                self.hold_until = now + HOLD;
                ((self.delay + (-w as u64)).min(self.delay + MAX_RISE), "a player waited for our turns")
            }
            _ if target > self.delay => {
                self.hold_until = now + HOLD;
                (target, "the trip to a player got longer")
            }
            Some(w) if w >= 2 && target < self.delay && now >= self.hold_until => (self.delay - 1, "turns arrive with time to spare"),
            None if target < self.delay && now >= self.hold_until => (self.delay - 1, "the trips got shorter"),
            _ => (self.delay, ""),
        };
        let next = next.clamp(MIN_DELAY, MAX_DELAY);
        if next > self.delay {
            // Turns sent from now on carry the new delay; their reports
            // arrive about a window after they do.
            self.settle_until = tick + next + 2 * ADAPT_EVERY;
        }
        if next != self.delay {
            tracing::info!(from = self.delay, to = next, why, worst_slack = ?worst, one_way_ms = format!("{one_way:.0}"), speed = self.speed_percent, "lockstep: input delay");
            self.delay = next;
        }
    }

    /// Takes in network events.
    fn take(&mut self, lua: &Lua, now: u64, events: Vec<Event>) {
        for e in events {
            match e {
                Event::Message { from, msg } => self.message(lua, now, from, msg),
                Event::PeerDisconnected { id, name, reason } => {
                    if let Some(r) = self.remotes.get_mut(&id)
                        && r.connected {
                        r.connected = false;
                        self.net.session_client_lost(id);
                        tracing::warn!(peer = id, name, reason, watermark = r.watermark, "lockstep: player lost");
                        // Gone, not lagging: the engine drops it, as
                        // moho does (FAF's dialog has no Eject for a
                        // client no longer connected).
                        self.begin_drop(id);
                    }
                    self.mark_dropped_ejected();
                    // Votes others needed this peer for may now be complete.
                    let targets: Vec<u32> = self.votes.keys().copied().collect();
                    for t in targets {
                        self.check_eject(t);
                    }
                }
                _ => {}
            }
        }
    }

    /// Starts dropping a lost or ejected peer: tell everyone what we hold
    /// of its turns and settle the cut.
    fn begin_drop(&mut self, id: u32) {
        let Some(r) = self.remotes.get_mut(&id) else { return };
        if r.dropping {
            return;
        }
        r.dropping = true;
        r.drop_claims.insert(self.local_peer, r.watermark);
        self.net.broadcast(&Msg::Dropped { peer: id, watermark: r.watermark });
        self.settle(id);
    }

    /// FAF's disconnect dialog stays open for a client no longer connected
    /// until its `ejectedBy` is every client still in (moho fills it for a
    /// client it dropped). So each dropped client counts as ejected by all
    /// still connected, us included; redone whenever someone else goes.
    fn mark_dropped_ejected(&self) {
        let stillin: BTreeSet<u32> = self.remotes.iter().filter(|(_, r)| r.connected).map(|(id, _)| *id).chain([self.local_peer]).collect();
        for (id, r) in &self.remotes {
            if !r.connected && r.dropping {
                self.net.set_ejected_by(*id, stillin.iter().copied().collect());
            }
        }
    }

    /// Everyone still in the game has voted to eject `target`: drop it.
    fn check_eject(&mut self, target: u32) {
        let voters = self.votes.get(&target).cloned().unwrap_or_default();
        if target == self.local_peer {
            let others: Vec<u32> = self.remotes.iter().filter(|(_, r)| r.connected).map(|(id, _)| *id).collect();
            if !others.is_empty() && others.iter().all(|o| voters.contains(o)) {
                tracing::error!("lockstep: the other players ejected us");
            }
            return;
        }
        let required: Vec<u32> = self.remotes.iter().filter(|(id, r)| **id != target && r.connected).map(|(id, _)| *id).chain([self.local_peer]).collect();
        if voters.is_empty() || !required.iter().all(|v| voters.contains(v)) {
            return;
        }
        let Some(r) = self.remotes.get(&target) else { return };
        if r.connected {
            // Its link goes too; the disconnect comes back as an event.
            tracing::warn!(peer = target, name = r.name, "lockstep: ejected by everyone");
            self.net.disconnect(target, Some("Ejected"));
            if let Some(r) = self.remotes.get_mut(&target) {
                r.connected = false;
            }
            self.net.session_client_lost(target);
        }
        self.begin_drop(target);
        self.mark_dropped_ejected();
    }

    fn message(&mut self, lua: &Lua, now: u64, from: u32, msg: Msg) {
        match msg {
            Msg::EjectVote { target } => {
                let voters = self.votes.entry(target).or_default();
                voters.insert(from);
                let list: Vec<u32> = voters.iter().copied().collect();
                tracing::warn!(target, voter = from, votes = ?list, "lockstep: eject vote");
                self.net.set_ejected_by(target, list);
                self.check_eject(target);
            }
            Msg::Pause { paused } => {
                tracing::info!(paused, from, "lockstep: pause from another player");
                self.net.set_session_paused(paused);
                self.net.push_pause_event(paused, from);
                self.pause_request = Some(paused);
                self.paused = paused;
            }
            Msg::Turn { sched, at, commands, checksums, capacity, slack } => {
                // Ticks to spare on this turn, and their report on ours.
                let spare = sched as i64 - now as i64;
                let s = self.spare.entry(from).or_insert(spare);
                *s = (*s).min(spare);
                if let Some(&(_, mine)) = slack.iter().find(|(p, _)| *p == self.local_peer) {
                    let w = self.slack_in.entry(from).or_insert(mine);
                    *w = (*w).min(mine);
                }
                self.stats.turns_received += 1;
                self.net.note_turn(from);
                self.peer_at.insert(from, (at, Instant::now()));
                self.capacities.insert(from, capacity);
                self.net.set_max_speed(from, capacity);
                // Their input delay: the tick it's for, less the tick they sent it on.
                self.net.set_input_delay(from, sched.saturating_sub(at) as u32);
                self.peer_delays.insert(from, sched.saturating_sub(at));
                for (t, c) in checksums {
                    self.theirs.insert((t, from), c);
                }
                // Once we've told everyone how many of a dropped player's
                // turns we hold, more from it would be ours alone: what
                // we lack comes relayed (`Forward`), as everyone agreed.
                if !self.remotes.get(&from).is_some_and(|r| r.dropping) {
                    self.accept_turn(lua, now, from, sched, commands);
                }
                self.check_checksums();
            }
            Msg::Forward { origin, sched, commands } => self.accept_turn(lua, now, origin, sched, commands),
            Msg::Chat(data) => {
                let name = self.remotes.get(&from).map_or_else(|| from.to_string(), |r| r.name.clone());
                self.net.push_chat(name, data);
            }
            Msg::ChecksumParts { tick, parts } => self.compare_parts(from, tick, &parts),
            Msg::Dropped { peer, watermark } => {
                if let Some(r) = self.remotes.get_mut(&peer) {
                    r.drop_claims.insert(from, watermark);
                }
                self.settle(peer);
            }
            _ => {}
        }
    }

    fn accept_turn(&mut self, lua: &Lua, now: u64, peer: u32, sched: u64, commands: Vec<String>) {
        let Some(r) = self.remotes.get_mut(&peer) else { return };
        if sched <= r.watermark {
            return; // already have it (a relayed copy)
        }
        // Past the agreed last turn: nobody else runs it.
        if r.cut.is_some_and(|cut| sched > cut) {
            return;
        }
        if sched < now {
            tracing::error!(peer, sched, now, "lockstep: a turn arrived after its tick ran (desync)");
        }
        if let Some(mut stream) = lua.app_data_mut::<CommandStream>() {
            for line in &commands {
                match crate::engine::sim::input::decode(line) {
                    // The sender's source is ours to know, not its to say.
                    Ok(s) => stream.schedule(Scheduled { tick: sched, client: r.source, ..s }),
                    Err(e) => tracing::warn!(peer, e, "lockstep: bad command"),
                }
            }
        }
        r.watermark = sched;
        r.history.push_back((sched, commands));
        while r.history.len() > HISTORY {
            r.history.pop_front();
        }
        let lost = !r.connected;
        if lost {
            self.settle(peer);
        }
    }

    /// A lost peer: once every remaining client has said what it holds, the
    /// highest watermark is the cut; relay turns others lack.
    fn settle(&mut self, peer: u32) {
        let others: BTreeSet<u32> = self.remotes.iter().filter(|(id, r)| **id != peer && r.connected).map(|(id, _)| *id).collect();
        let Some(r) = self.remotes.get_mut(&peer) else { return };
        if r.connected || r.cut.is_some() {
            return;
        }
        // The claim we sent stands (relayed turns since don't change it).
        r.drop_claims.entry(self.local_peer).or_insert(r.watermark);
        if !others.iter().all(|o| r.drop_claims.contains_key(o)) {
            return;
        }
        let cut = r.drop_claims.values().copied().max().unwrap_or(r.watermark);
        let lowest = r.drop_claims.values().copied().min().unwrap_or(cut);
        // The member with the most (lowest id among equals) relays.
        let relay = r.drop_claims.iter().filter(|(_, w)| **w == cut).map(|(id, _)| *id).min();
        if relay == Some(self.local_peer) && lowest < cut {
            for (sched, commands) in r.history.iter().filter(|(s, _)| *s > lowest) {
                self.net.broadcast(&Msg::Forward { origin: peer, sched: *sched, commands: commands.clone() });
            }
        }
        r.cut = Some(cut);
        tracing::warn!(peer, name = r.name, cut, "lockstep: player dropped after their last turn");
    }

    fn check_checksums(&mut self) {
        let mut done = Vec::new();
        // Per tick, the players whose checksum differs from ours.
        let mut differing: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for (&(tick, peer), &theirs) in &self.theirs {
            let Some(&ours) = self.ours.get(&tick) else { continue };
            done.push((tick, peer));
            if ours == theirs {
                self.stats.checksums_matched += 1;
            } else {
                self.stats.desyncs += 1;
                let name = self.remotes.get(&peer).map_or("?", |r| r.name.as_str());
                tracing::error!(tick, peer, name, ours = format!("{ours:016x}"), theirs = format!("{theirs:016x}"), "lockstep: DESYNC");
                differing.entry(tick).or_insert_with(|| vec![format!("{}: {ours:016x}", self.net.name())]).push(format!("{name}: {theirs:016x}"));
                // Our parts, once: they log which parts differ (and we do,
                // from theirs).
                if let Some(parts) = self.our_parts.get(&tick).filter(|_| !self.parts_sent.contains(&peer)) {
                    self.net.send(peer, &Msg::ChecksumParts { tick, parts: parts.to_vec() });
                    self.parts_sent.insert(peer);
                }
            }
        }
        for (tick, lines) in differing {
            self.net.push_desync(tick, lines);
        }
        for k in done {
            self.theirs.remove(&k);
        }
        // Forget claims too old to ever check.
        if let Some(&oldest) = self.ours.keys().next() {
            self.theirs.retain(|(t, _), _| *t >= oldest || self.ours.is_empty());
        }
    }

    /// A peer's checksum parts after `tick`: logs the parts that differ
    /// from ours.
    fn compare_parts(&self, peer: u32, tick: u64, theirs: &[u64]) {
        use crate::engine::sim::checksum::PART_NAMES;
        let name = self.remotes.get(&peer).map_or("?", |r| r.name.as_str());
        let Some(ours) = self.our_parts.get(&tick) else {
            tracing::warn!(tick, peer, name, "lockstep: a peer's checksum parts for a tick we no longer hold");
            return;
        };
        let differing: Vec<&str> = PART_NAMES.iter().zip(ours.iter().zip(theirs)).filter(|(_, (a, b))| a != b).map(|(n, _)| *n).collect();
        tracing::error!(tick, peer, name, ?differing, "lockstep: DESYNC parts (RTS_DUMP_TICK={tick} on both to see the entities)");
    }

    /// The local player's command source.
    pub fn local_source(&self) -> u32 {
        self.local_source
    }

    /// Lost players (for the interface).
    pub fn lost(&self) -> Vec<String> {
        self.remotes.values().filter(|r| !r.connected).map(|r| r.name.clone()).collect()
    }
}

/// A game speed applied this tick, for the real-time loop to take
/// (`Engine::sync_net_speed`). `by` is the session client (1-based) that
/// asked, 0 for none (the starting speed).
pub struct SpeedChanged {
    pub percent: u32,
    pub by: u32,
    pub networked: bool,
}

/// The command asking everyone for game speed `step`.
fn speed_command(step: i32) -> crate::engine::sim::input::PlayerCommand {
    use crate::engine::sim::command::{CommandKind, CommandTarget};
    crate::engine::sim::input::PlayerCommand {
        army: 0,
        kind: CommandKind::Stop,
        units: Vec::new(),
        target: CommandTarget::None,
        queue: false,
        formation: None,
        facing: None,
        blueprint: None,
        count: 0,
        toggle: Some(crate::engine::sim::input::Toggle::GameSpeed(step)),
        script: Vec::new(),
        callback: None,
        category: None,
        retarget: None,
    }
}

/// A `Toggle::GameSpeed` from command source `source` came due: every
/// client applies it at this tick, in command order, so all agree. In a
/// networked game the lobby's Game Speed option may forbid it (then every
/// client ignores it). Outside one (a replay) it simply applies.
pub fn apply_game_speed(lua: &Lua, source: u32, step: i32) {
    let percent = crate::engine::snapshot::SimControl::percent_of(step);
    let Some(mut l) = lua.app_data_mut::<Lockstep>() else {
        lua.set_app_data(SpeedChanged { percent, by: 1, networked: false });
        return;
    };
    if !l.speed_adjustable {
        tracing::warn!(source, step, "lockstep: game speed is fixed (lobby option); ignored a change");
        return;
    }
    if source == l.local_source && l.speed_requested == Some(step) {
        l.speed_requested = None;
    }
    l.speed_percent = percent;
    let by = l.net.session_clients().iter().position(|c| c.source == source).map_or(0, |i| i as u32 + 1);
    tracing::info!(tick = crate::engine::api::sim_time::current_tick(lua), source, by, step, percent, "lockstep: game speed changed");
    drop(l);
    lua.set_app_data(SpeedChanged { percent, by, networked: true });
}

/// Every client's command source, by peer id: `owners` (human armies'
/// owners in army order, then observers in slot order) by position, anyone
/// else after them by peer id. Every client works it out the same from the
/// same launch, its own source included.
fn command_sources(owners: &[u32], ids: &[u32]) -> BTreeMap<u32, u32> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    let mut extra = owners.len() as u32;
    ids.into_iter()
        .map(|id| match owners.iter().position(|&o| o == id) {
            Some(i) => (id, i as u32),
            None => {
                extra += 1;
                (id, extra - 1)
            }
        })
        .collect()
}

/// At session start of a networked game (the launch came over the network
/// and the connection is still up): the lockstep for this client, as sim
/// app data. Peers map to command sources the way moho numbers them: each
/// human army's owner in army order, then observers in slot order; anyone
/// else connected (none, normally) after them by peer id.
pub fn start(lua: &Lua, launch: &crate::engine::sim::lua_data::LuaData) -> LuaResult<bool> {
    use crate::engine::sim::lua_data::LuaData;
    let Some(net) = super::core::current().filter(|n| n.launched()) else { return Ok(false) };
    let LuaData::Table(fields) = launch else { return Ok(false) };
    let field = |k: &str| fields.iter().find(|(key, _)| *key == LuaData::string(k)).map(|(_, v)| v);
    let Some(local_peer) = net.local_id() else { return Ok(false) };
    let delay = match field("Delay") {
        Some(LuaData::Number(bits)) => (f64::from_bits(*bits) as u64).clamp(MIN_DELAY, MAX_DELAY),
        _ => MIN_DELAY,
    };
    // Owners of the human armies, in army order.
    let setup: Option<LuaTable> = lua.globals().get::<Option<LuaTable>>("ScenarioInfo")?.and_then(|s| s.get("ArmySetup").ok());
    let mut owners: Vec<u32> = Vec::new();
    if let (Some(setup), Some(armies)) = (setup, lua.app_data_ref::<crate::engine::sim::army::Armies>()) {
        for army in armies.list.iter().filter(|a| a.human) {
            let owner = setup.get::<Option<LuaTable>>(army.name.as_str())?.and_then(|o| o.get::<Option<String>>("OwnerID").ok().flatten());
            owners.push(owner.and_then(|o| o.parse().ok()).unwrap_or(u32::MAX));
        }
    }
    // Observers, in slot order.
    if let Some(LuaData::Table(config)) = field("Config")
        && let Some((_, LuaData::Table(observers))) = config.iter().find(|(k, _)| *k == LuaData::string("Observers")) {
        for (_, o) in observers {
            if let LuaData::Table(o) = o {
                let owner = o.iter().find(|(k, _)| *k == LuaData::string("OwnerID")).and_then(|(_, v)| match v {
                    LuaData::String(s) => std::str::from_utf8(s).ok().and_then(|s| s.parse().ok()),
                    LuaData::Number(b) => Some(f64::from_bits(*b) as u32),
                    _ => None,
                });
                owners.push(owner.unwrap_or(u32::MAX));
            }
        }
    }
    let mut peers: Vec<(u32, String)> = net.peers().into_iter().map(|p| (p.id, p.name)).collect();
    peers.sort_by_key(|p| p.0);
    let ids: Vec<u32> = peers.iter().map(|p| p.0).chain([local_peer]).collect();
    let sources = command_sources(&owners, &ids);
    // Ours as every peer numbers us: the session's guess (an observer after
    // every player) is the first observer's for every observer.
    let local_source = sources[&local_peer];
    let guessed = crate::engine::api::command_source::local_source(lua);
    if local_source != guessed {
        tracing::info!(guessed, local_source, "lockstep: our command source, as the peers number it");
        lua.set_app_data(crate::engine::api::command_source::LocalCommandSource(local_source));
    }
    let peers: Vec<(u32, u32, String)> = peers.into_iter().map(|(id, name)| (id, sources[&id], name)).collect();
    let start_tick = crate::engine::api::sim_time::current_tick(lua);
    tracing::info!(?peers, local_peer, local_source, "lockstep: players");
    let mut clients: Vec<super::core::SessionClient> = peers
        .iter()
        .map(|(id, source, name)| super::core::SessionClient { source: *source, peer: *id, name: name.clone(), local: false, connected: true, last_turn: None, ejected_by: Vec::new() })
        .collect();
    clients.push(super::core::SessionClient { source: local_source, peer: local_peer, name: net.name().to_owned(), local: true, connected: true, last_turn: None, ejected_by: Vec::new() });
    clients.sort_by_key(|c| (c.source, c.peer));
    net.set_session_clients(clients);
    let mut lockstep = Lockstep::new(net, local_peer, local_source, peers, start_tick, delay);
    // The lobby's Game Speed: `normal` (+0) and `fast` (+4) are fixed,
    // `adjustable` lets players change it.
    let option: Option<String> = lua.globals().get::<Option<LuaTable>>("ScenarioInfo")?.and_then(|s| s.get::<Option<LuaTable>>("Options").ok().flatten()).and_then(|o| o.get("GameSpeed").ok().flatten());
    lockstep.speed_adjustable = option.as_deref() == Some("adjustable");
    let step = if option.as_deref() == Some("fast") { 4 } else { 0 };
    lockstep.speed_percent = crate::engine::snapshot::SimControl::percent_of(step);
    lua.set_app_data(SpeedChanged { percent: lockstep.speed_percent, by: 0, networked: true });
    tracing::info!(?option, adjustable = lockstep.speed_adjustable, percent = lockstep.speed_percent, "lockstep: game speed rule");
    lua.set_app_data(lockstep);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_covers_the_one_way_trip_plus_margin_in_whole_ticks() {
        // LAN, normal speed: 2 ticks.
        assert_eq!(delay_for(0.5, 100, 30.0), 2);
        // 300 ms one way (spikes included) + 30 = 330 ms = 4 ticks.
        assert_eq!(delay_for(300.0, 100, 30.0), 4);
        // Twice the speed: ticks are 50 ms, so twice the ticks.
        assert_eq!(delay_for(300.0, 200, 30.0), 7);
        // Never below the minimum, never above the maximum.
        assert_eq!(delay_for(0.0, 25, 0.0), MIN_DELAY);
        assert_eq!(delay_for(100_000.0, 100, 30.0), MAX_DELAY);
    }

    /// Spikes count one way: a 40 ms link that spikes to 440 ms has to
    /// cover 420 ms, not half the average.
    #[test]
    fn one_way_trip_counts_spikes_one_way() {
        let peer = |ping_ms: f64, ping99_ms: f64| crate::net::core::PeerInfo {
            id: 1,
            name: String::new(),
            uid: String::new(),
            addr: "127.0.0.1:1".parse().unwrap(),
            ping_ms,
            ping99_ms,
            quiet_ms: 0.0,
            established: Vec::new(),
        };
        assert_eq!(one_way_ms(&[peer(40.0, 40.0)]), 20.0);
        assert_eq!(one_way_ms(&[peer(40.0, 440.0), peer(100.0, 100.0)]), 420.0);
        assert_eq!(one_way_ms(&[]), 0.0);
    }

    /// A slow first tick (3 s: start-up one-offs) no longer holds the game
    /// back: full speed within 15 ticks of 1 ms. A machine slow throughout
    /// (120 ms ticks) still measures below full speed after the warm-up.
    #[test]
    fn a_slow_first_tick_fades_in_the_warm_up() {
        let mut cost = None;
        for tick in 0..16 {
            cost = Some(smoothed_cost(cost, if tick == 0 { 3000.0 } else { 1.0 }, tick));
        }
        assert!(capacity_for(cost.unwrap()) >= 100, "cost {cost:?}");
        let mut cost = None;
        for tick in 0..200 {
            cost = Some(smoothed_cost(cost, 120.0, tick.min(WARMUP_TICKS)));
        }
        assert!(capacity_for(cost.unwrap()) < 100);
        // After the warm-up a one-off spike moves the average a little only.
        let settled = smoothed_cost(Some(1.0), 1000.0, WARMUP_TICKS);
        assert!(settled < 25.0, "{settled}");
    }

    /// A checksum that differs is for the interface too (FAF's desync
    /// dialog), not just the log: once per tick, ours first.
    #[test]
    fn a_differing_checksum_reaches_the_interface() {
        let net = NetCore::open(0, "us", "u1").unwrap();
        let mut l = Lockstep::new(net.clone(), 1, 0, vec![(2, 1, "them".into()), (3, 2, "other".into())], 1, 2);
        l.ours.insert(9, 0xaa);
        l.theirs.insert((9, 2), 0xbb);
        l.theirs.insert((9, 3), 0xaa);
        l.ours.insert(19, 0xcc);
        l.theirs.insert((19, 2), 0xcc);
        l.check_checksums();
        assert_eq!(net.take_desyncs(), vec![(9, vec!["us: 00000000000000aa".to_string(), "them: 00000000000000bb".to_string()])]);
        assert_eq!((l.stats.desyncs, l.stats.checksums_matched), (1, 2));
        assert!(net.take_desyncs().is_empty());
    }

    /// An ejected player's turn still on its way when everyone has voted
    /// runs nowhere: we hold to the claim we sent, the cut is the one all
    /// agree on, and nothing past it is taken, relayed or not.
    #[test]
    fn an_ejected_players_late_turns_run_nowhere() {
        let lua = Lua::new();
        crate::engine::api::player_input::register(&lua).unwrap();
        let net = NetCore::open(0, "us", "u1").unwrap();
        let mut l = Lockstep::new(net, 1, 0, vec![(2, 1, "laggy".into()), (3, 2, "other".into())], 1, 2);
        let line = |sched| crate::engine::sim::input::encode(&Scheduled { tick: sched, client: 1, seq: 0, command: speed_command(1) });
        let turn = |sched| Msg::Turn { sched, at: sched - 2, commands: vec![line(sched)], checksums: Vec::new(), capacity: 100, slack: Vec::new() };
        l.message(&lua, 3, 2, turn(4));
        // Everyone votes: we claim turn 4.
        l.votes.insert(2, [1, 3].into());
        l.check_eject(2);
        assert_eq!(l.remotes[&2].drop_claims[&1], 4);
        // Its next turn was already on its way.
        l.message(&lua, 3, 2, turn(5));
        assert_eq!(l.remotes[&2].watermark, 4);
        // The other player holds turn 6 and relays 5 and 6: the cut is 6.
        l.message(&lua, 3, 3, Msg::Dropped { peer: 2, watermark: 6 });
        assert_eq!(l.remotes[&2].cut, Some(6));
        for sched in [5, 6, 7] {
            l.message(&lua, 3, 3, Msg::Forward { origin: 2, sched, commands: vec![line(sched)] });
        }
        assert_eq!(l.remotes[&2].drop_claims[&1], 4, "our claim is the one we sent");
        let due: Vec<u64> = lua.app_data_mut::<CommandStream>().unwrap().take_due(1_000).iter().map(|s| s.tick).collect();
        assert_eq!(due, vec![4, 5, 6]);
    }

    /// A host and `n - 1` joiners meshed over loopback (peer ids 1..=n).
    fn mesh(n: usize) -> Vec<Arc<NetCore>> {
        let wait_until = |what: &str, f: &dyn Fn() -> bool| {
            let start = Instant::now();
            while !f() {
                assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let host = NetCore::open(0, "p1", "u1").unwrap();
        host.host();
        let addr = format!("127.0.0.1:{}", host.port());
        let mut all = vec![host];
        for i in 2..=n {
            let j = NetCore::open(0, &format!("p{i}"), &format!("u{i}")).unwrap();
            j.join(&addr);
            wait_until("a join", &|| j.local_id() == Some(i as u32));
            all.push(j);
        }
        wait_until("the mesh", &|| all.iter().all(|c| c.peers().len() == n - 1));
        all
    }

    /// Peer `id`'s lockstep in an `n`-peer game: source = id - 1, delay 2,
    /// and its session clients (for `ejectedBy`).
    fn lockstep_for(net: &Arc<NetCore>, id: u32, n: u32) -> Lockstep {
        let peers: Vec<(u32, u32, String)> = (1..=n).filter(|&p| p != id).map(|p| (p, p - 1, format!("p{p}"))).collect();
        let clients = (1..=n)
            .map(|p| crate::net::core::SessionClient { source: p - 1, peer: p, name: format!("p{p}"), local: p == id, connected: true, last_turn: None, ejected_by: Vec::new() })
            .collect();
        net.set_session_clients(clients);
        Lockstep::new(Arc::clone(net), id, id - 1, peers, 1, 2)
    }

    /// A 3-player game; one player leaves mid-game (the score screen:
    /// its link says goodbye). The others don't wait for an eject vote that
    /// FAF's dialog can't offer (no Eject button for a client no longer
    /// connected): they drop it and play on, agree on its last turn, run
    /// exactly the same commands, and FAF's dialog sees it ejected by all
    /// still in (so it closes). A 2-player LAN game stalled forever here.
    #[test]
    fn a_player_who_leaves_is_dropped_and_the_others_play_on_alike() {
        const TICKS: u64 = 60;
        const LEAVES_AFTER: u64 = 15;
        let nets = mesh(3);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        for (i, net) in nets.iter().enumerate() {
            let id = i as u32 + 1;
            let net = Arc::clone(net);
            let done_tx = done_tx.clone();
            std::thread::spawn(move || {
                let lua = Lua::new();
                crate::engine::api::player_input::register(&lua).unwrap();
                let mut l = lockstep_for(&net, id, 3);
                let mut ran = Vec::new();
                for tick in 1..=TICKS {
                    // Some input every tick from everyone.
                    l.tick(&lua, tick, vec![speed_command((tick % 3) as i32)]);
                    let due = lua.app_data_mut::<CommandStream>().unwrap().take_due(tick);
                    ran.extend(due.iter().map(|s| (s.tick, s.client, s.seq)));
                    if id == 3 && tick == LEAVES_AFTER {
                        net.close();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                let cut = l.remotes[&3].cut;
                let ejected = net.session_clients().iter().find(|c| c.peer == 3).map(|c| c.ejected_by.clone());
                let _ = done_tx.send((id, cut, ran, ejected));
            });
        }
        let mut results = BTreeMap::new();
        for _ in 0..2 {
            let (id, cut, ran, ejected) = done_rx.recv_timeout(Duration::from_secs(20)).expect("the players still in stalled after one left");
            results.insert(id, (cut, ran, ejected));
        }
        let (cut1, ran1, ejected1) = &results[&1];
        let (cut2, ran2, ejected2) = &results[&2];
        assert!(cut1.is_some_and(|c| c >= LEAVES_AFTER), "cut {cut1:?}");
        assert_eq!(cut1, cut2, "both agree on the leaver's last turn");
        assert_eq!(ran1, ran2, "both ran the same commands");
        assert!(ran1.iter().any(|&(t, c, _)| c == 2 && t == cut1.unwrap()), "the leaver's turns up to the cut ran");
        assert!(!ran1.iter().any(|&(t, c, _)| c == 2 && t > cut1.unwrap()), "nothing of the leaver's after the cut");
        assert_eq!(ejected1.as_deref(), Some(&[1, 2][..]));
        assert_eq!(ejected2.as_deref(), Some(&[1, 2][..]));
        for n in nets {
            n.close();
        }
    }

    /// A game stalled on a player who is still connected but sends no turns
    /// (frozen): the score screen's leave (`SessionEndGame`) or the session
    /// closing (back to the menu) ends the wait at once, so the engine
    /// thread sees it and the front end loads (it went black).
    #[test]
    fn leaving_ends_a_wait_for_players() {
        let nets = mesh(2);
        let control = crate::engine::snapshot::SimControl::new(100);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let net = Arc::clone(&nets[0]);
        let stop = Arc::clone(&control);
        std::thread::spawn(move || {
            let lua = Lua::new();
            crate::engine::api::player_input::register(&lua).unwrap();
            let mut l = lockstep_for(&net, 1, 2);
            l.set_stop(stop);
            // Peer 2 never sends a turn: ticks 1 and 2 are covered by the
            // launch's delay, tick 3 waits for it.
            for tick in 1..=4 {
                l.tick(&lua, tick, Vec::new());
                let _ = done_tx.send(tick);
            }
        });
        for tick in 1..=2 {
            assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)), Ok(tick));
        }
        assert!(done_rx.recv_timeout(Duration::from_millis(300)).is_err(), "tick 3 waits for the frozen player");
        control.leave();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(2)), Ok(3), "the wait ended when the player left");
        for n in nets {
            n.close();
        }
    }

    /// Two players (peers 1, 2), two observers (4, then 3 by slot) and a
    /// peer the launch doesn't name (5): each client, us included, numbers
    /// them all the same, so no two share a source.
    #[test]
    fn every_client_numbers_command_sources_alike() {
        let owners = [1, 2, 4, 3];
        let everyone = [1, 2, 3, 4, 5];
        let expected: BTreeMap<u32, u32> = [(1, 0), (2, 1), (4, 2), (3, 3), (5, 4)].into();
        for local in everyone {
            // As `start` sees it: the peers, then us.
            let ids: Vec<u32> = everyone.iter().copied().filter(|&p| p != local).chain([local]).collect();
            assert_eq!(command_sources(&owners, &ids), expected, "client {local}");
        }
    }

    #[test]
    fn capacity_is_the_speed_whose_tick_fits_the_cost_with_headroom() {
        // 20 ms ticks: 25 ms with headroom, so 4x (400%).
        assert_eq!(capacity_for(20.0), 400);
        // 100 ms ticks can't keep normal speed: 80%.
        assert_eq!(capacity_for(100.0), 80);
        // Rounded down to whole steps, never zero.
        assert_eq!(capacity_for(33.0), 240);
        assert_eq!(capacity_for(1e9), CAPACITY_STEP);
    }
}
