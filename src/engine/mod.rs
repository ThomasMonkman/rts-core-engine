pub mod api;
pub mod assets;
pub mod keyframe;
pub mod replay;
pub mod session;
pub mod snapshot;
pub mod sync;
pub mod sim;
pub mod lua;
pub mod lua_translation;
pub mod game_code;
pub mod mods;
pub mod vfs;
pub mod watchdog;
pub mod tracy;

use crate::debug_ui::DebugUiState;
use crate::engine::lua::execute::exec_lua_string;
use crate::engine::lua::script_cache::ScriptCache;
use mlua::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use tracing::{info, warn};
use game_code::GameCode;
use vfs::{MountPoint, Vfs, tree::FsDir};

pub struct Engine {
    #[allow(dead_code)]
    sim_lua: Lua,
    /// The session's script cache, shared with a user VM (as the renderer
    /// does) so its scripts compile once.
    pub script_cache: Arc<ScriptCache>,
    #[allow(dead_code)]
    pub vfs: Arc<RwLock<Vfs>>,

    /// Game speed for the real-time loop (windowed runs).
    control: Option<Arc<snapshot::SimControl>>,
    /// A headless `--speed`: `run_ticks` keeps this speed (percent) in real time.
    pace: Option<u32>,
    /// Progress markers for the stall watchdog (`start_watchdog`).
    watchdog: Arc<watchdog::Watchdog>,
    /// Record the game into a new file in this folder when the session
    /// starts (`record_into`: the default recording).
    record_dir: Option<std::path::PathBuf>,
    /// Keyframes written as the game plays (`set_keyframes`).
    keyframes: Option<keyframe::recorder::Recorder>,
    /// Carry on from the latest keyframe at or before this tick once
    /// started (`set_keyframes`).
    resume_at: Option<u64>,
}

/// The recording notes how far the game got (`# end`) when its engine
/// goes (a run of `--ticks`, a test); a real-time session notes it when it
/// ends or closes.
impl Drop for Engine {
    fn drop(&mut self) {
        api::replay::end_recording(&self.sim_lua);
    }
}

impl Engine {
    // #[instrument(skip(debug_ui))]
    pub fn new(
        init_path: &str,
        fa_path: &str,
        debug_ui: Arc<Mutex<DebugUiState>>,
    ) -> LuaResult<Self> {
        Self::new_with_vault(init_path, fa_path, debug_ui, None)
    }

    /// Like [`Self::new`], also mounting a FAF vault folder's maps
    /// (`<vault>/maps/<name>` at `/maps/<name>`, as FAF's `MountMapContent`).
    pub fn new_with_vault(
        init_path: &str,
        fa_path: &str,
        debug_ui: Arc<Mutex<DebugUiState>>,
        vault: Option<&Path>,
    ) -> LuaResult<Self> {
        info!("starting engine");
        let _startup = crate::prof_span!("engine startup");

        let init_script_output = {
            let _s = crate::prof_span!("init.lua");
            init_output(init_path, fa_path)?
        };
        let mut mount_points = init_script_output.mount_points.clone();
        crate::net::map_download::set_maps_dir(vault.unwrap_or(&init_script_output.vault).join("maps"));
        if let Some(vault) = vault {
            mount_points.extend(vault_maps(vault).map_err(LuaError::external)?);
        }
        let mut hook_paths = init_script_output.hook_paths.clone();
        hook_paths.push(ENGINE_HOOK.to_string());

        // The game files are the same for every session in the process: the
        // index, the translated scripts and the compiled bytecode are built
        // once and shared.
        let files = game_files(mount_points, hook_paths)?;
        let vfs = files.vfs.clone();
        let script_cache = files.scripts.clone();

        debug_ui.lock().unwrap().vfs_tree = Some(files.tree.clone());

        let _s = crate::prof_span!("sim vm + engine API");
        let sim_lua = Lua::new();
        sim_lua.set_compiler(lua::script_cache::compiler());
        // Collections only at fixed points, the same on every machine.
        sim::gc::manual(&sim_lua);
        sim_lua.set_app_data(vfs.clone());
        sim_lua.set_app_data(script_cache.clone());
        api::register_safe(&sim_lua)?;
        keyframe::prepare(&sim_lua)?;
        lua::debugger::attach(&sim_lua, "Sim");
        let watchdog = Arc::new(watchdog::Watchdog::default());
        sim_lua.set_app_data(watchdog.clone());

        Ok(Self {
            sim_lua,
            script_cache: script_cache.clone(),
            vfs,
            control: None,
            record_dir: None,
            pace: None,
            watchdog,
            keyframes: None,
            resume_at: None,
        })
    }

    pub fn start(&mut self, session: &session::SessionConfig) -> LuaResult<()> {
        self.start_recording(session);
        {
            let _s = crate::prof_span!("game startup");
            {
                let _s = crate::prof_span!("mods");
                self.activate_mods(session)?;
            }
            {
                let _s = crate::prof_span!("blueprints (RuleInit.lua)");
                self.load_blueprints()?;
                sim::gc::collect(&self.sim_lua)?;
            }
            {
                let _s = crate::prof_span!("blueprint defaults");
                api::blueprints::apply_defaults(&self.sim_lua, false)?;
            }
            {
                let _s = crate::prof_span!("entity categories");
                api::categories::build(&self.sim_lua)?;
            }
            {
                let _s = crate::prof_span!("simInit.lua");
                self.init_sim()?;
                sim::gc::collect(&self.sim_lua)?;
            }
            let _s = crate::prof_span!("session setup");
            session::setup(&self.sim_lua, session)?;
            // A game launched over the network plays in lockstep.
            if let Some(launch) = &session.launch
                && crate::net::lockstep::start(&self.sim_lua, launch)? {
                // Waiting for players' turns ends when the player
                // leaves or the session closes (the front end loads).
                if let (Some(control), Some(mut l)) = (&self.control, self.sim_lua.app_data_mut::<crate::net::lockstep::Lockstep>()) {
                    l.set_stop(Arc::clone(control));
                }
                if session.net_bot {
                    let army = self.sim_lua.app_data_ref::<sim::army::Armies>().and_then(|a| {
                        let local = self.sim_lua.app_data_ref::<api::command_source::LocalCommandSource>().map_or(0, |s| s.0);
                        a.list.iter().filter(|a| a.human).nth(local as usize).map(|a| a.index)
                    });
                    if let Some(army) = army {
                        let peer = crate::net::core::current().and_then(|n| n.local_id()).unwrap_or(0);
                        self.sim_lua.set_app_data(crate::net::bot::Bot::new(army, peer));
                        info!(army, "net bot playing");
                    }
                }
            }
            sim::gc::collect(&self.sim_lua)?;
            // What a keyframe load starts from (game start-up is the same
            // every time).
            keyframe::mark_base(&self.sim_lua)?;
            if let (Some(k), Some(tick)) = (&self.keyframes, self.resume_at) {
                let _s = crate::prof_span!("resume from a keyframe");
                k.resume(&self.sim_lua, tick)?;
            }
        }
        self.watchdog.arm();
        match session.ticks {
            Some(n) => self.run_ticks(n),
            None => self.run_realtime(),
        }
    }

    /// Writes a keyframe every `every` ticks into `dir` as the game plays
    /// (a replay), and with `resume_at` carries on from the latest one at
    /// or before that tick instead of the start (a seek).
    pub fn set_keyframes(&mut self, dir: std::path::PathBuf, every: u64, resume_at: Option<u64>) {
        self.keyframes = Some(keyframe::recorder::Recorder::new(dir, every));
        self.resume_at = resume_at;
    }

    /// End of a tick: a keyframe if one is due.
    fn after_step(&self) -> LuaResult<()> {
        match &self.keyframes {
            Some(k) => k.after_tick(&self.sim_lua, api::sim_time::current_tick(&self.sim_lua)),
            None => Ok(()),
        }
    }

    /// Attaches a renderer: from now on each tick offers a snapshot. Never
    /// called in headless runs, which then build no snapshots at all.
    pub fn set_snapshot_sink(&mut self, mut sink: snapshot::SnapshotSink) {
        {
            // Renderer-only: no gcids (see `snapshot::publish`).
            let _frozen = crate::engine::lua::heap_snapshot::FrozenGcids::new(&self.sim_lua);
            sink.send_assets(&self.sim_lua);
        }
        self.sim_lua.set_app_data(sink);
    }

    /// Takes the renderer's channels back once the session is over, for the
    /// next session's engine: the snapshot sink (renewed: nothing sent) and
    /// the order link.
    pub fn release_renderer(&mut self) -> (Option<snapshot::SnapshotSink>, Option<sim::input::InputLink>) {
        let sink = self.sim_lua.remove_app_data::<snapshot::SnapshotSink>().map(snapshot::SnapshotSink::renew);
        (sink, self.sim_lua.remove_app_data::<sim::input::InputLink>())
    }

    /// Advances the sim by one tick (and writes a keyframe if one is due).
    pub fn tick(&mut self) -> LuaResult<()> {
        sim::tick::step(&self.sim_lua)?;
        self.after_step()
    }

    /// The sim VM (tests and tools inspect sim state through its app data).
    pub fn sim_lua(&self) -> &Lua {
        &self.sim_lua
    }

    /// Checksum of the current sim state (see `sim::checksum`).
    pub fn checksum(&self) -> u64 {
        sim::checksum::of_lua(&self.sim_lua)
    }

    /// Reports (and, per `config`, ends) runs whose sim stops making
    /// progress; see `watchdog`. Without this the markers are just unused.
    pub fn start_watchdog(&self, config: watchdog::Config) -> LuaResult<()> {
        watchdog::install_interrupt(&self.sim_lua);
        watchdog::spawn(self.watchdog.clone(), config).map_err(LuaError::external)
    }

    /// Times each tick stage, reported with the progress lines (every run:
    /// ~460 ns a tick). Wall time, for logs only.
    pub fn profile_stages(&self) {
        self.sim_lua.set_app_data(watchdog::StageTimes::default());
    }

    /// Runs `n` sim ticks back to back (headless runs, tests, benches).
    /// Logs progress every 1000 ticks (wall time is for the log only).
    pub fn run_ticks(&mut self, n: u64) -> LuaResult<()> {
        let mut lap = std::time::Instant::now();
        // Determinism hunting (dev): `RTS_TRACE_TICKS=a-b` logs the checksum
        // every tick in that range; `RTS_DUMP_TICK=t RTS_DUMP_FILE=path`
        // writes every live entity's state at tick t (diff two runs).
        let trace = std::env::var("RTS_TRACE_TICKS").ok().and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?))
        });
        // `RTS_GC_EVERY_TICK=1`: a full Lua collection after every tick (GC
        // timing must not change the sim).
        let gc_every_tick = std::env::var("RTS_GC_EVERY_TICK").is_ok();
        // `RTS_REFERENCE=path` (lines "tick checksum" from a trace): at the
        // first tick that differs, dump the state to `RTS_DUMP_FILE` and stop.
        let reference: Option<std::collections::BTreeMap<u64, String>> = std::env::var("RTS_REFERENCE").ok().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| {
            t.lines().filter_map(|l| {
                let (a, b) = l.split_once(' ')?;
                Some((a.parse().ok()?, b.trim().to_owned()))
            }).collect()
        });
        // `RTS_DUMP_TICK` is one tick or a range `a-b` (one file per tick,
        // `RTS_DUMP_FILE.<tick>`).
        let dump = std::env::var("RTS_DUMP_TICK").ok().and_then(|v| match v.split_once('-') {
            Some((a, b)) => Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?, true)),
            None => v.parse::<u64>().ok().map(|t| (t, t, false)),
        }).zip(std::env::var("RTS_DUMP_FILE").ok());
        let mut next = std::time::Instant::now();
        // Paced (headless network tests): speed changes apply as in a window.
        let pace = self.pace.map(snapshot::SimControl::new);
        // Dev: `RTS_NET_SPEED_AT=tick:step` asks for game speed `step` at
        // `tick`, as a player pressing `+` / `-` would.
        let speed_at = std::env::var("RTS_NET_SPEED_AT").ok().and_then(|v| {
            let (t, s) = v.split_once(':')?;
            Some((t.parse::<u64>().ok()?, s.parse::<i32>().ok()?))
        });
        for i in 1..=n {
            // The game ended (`EndGame`): nothing more to run (moho's
            // `/exitongameover`).
            if api::sim_time::has_ended(&self.sim_lua) {
                info!(tick = i - 1, of = n, "game over: the run ends");
                break;
            }
            if let (Some(control), Some((_, step))) = (&pace, speed_at.filter(|(t, _)| *t == i)) {
                control.set_speed(snapshot::SimControl::percent_of(step));
            }
            let step_started = std::time::Instant::now();
            sim::tick::step(&self.sim_lua)?;
            self.after_step()?;
            self.note_net_tick(step_started);
            if let Some(control) = &pace {
                self.sync_net_speed(control);
            }
            if let Some(speed) = pace.as_ref().map(|c| c.shown()).filter(|&s| s != snapshot::SimControl::MAX) {
                let speed = self.net_pace(speed);
                let factor = self.net_catch_up(speed);
                next += std::time::Duration::from_micros(((100_000 * 100 / speed.max(1) as u64) as f64 * factor) as u64);
                match next.checked_duration_since(std::time::Instant::now()) {
                    Some(wait) => std::thread::sleep(wait),
                    None => next = std::time::Instant::now(),
                }
            }
            if gc_every_tick {
                sim::gc::collect(&self.sim_lua)?;
            }
            if trace.is_some_and(|(a, b)| (a..=b).contains(&i)) {
                info!(tick = i, checksum = %format!("{:016x}", self.checksum()), "trace");
            }
            let mismatch = reference.as_ref().and_then(|r| r.get(&i)).is_some_and(|want| *want != format!("{:016x}", self.checksum()));
            if mismatch {
                tracing::error!(tick = i, "differs from the reference");
            }
            let dump_now = dump.as_ref().map(|((a, b, range), p)| ((*a..=*b).contains(&i), if *range { format!("{p}.{i}") } else { p.clone() })).filter(|d| d.0).or_else(|| mismatch.then(|| (true, std::env::var("RTS_DUMP_FILE").unwrap_or_else(|_| "dump.txt".into()))));
            if let Some((true, path)) = dump_now {
                if let Some(w) = self.sim_lua.app_data_ref::<sim::entity::EntityWorld>() {
                    let commands = self.sim_lua.app_data_ref::<sim::command::Commands>();
                    let mut out = String::new();
                    for e in w.iter().filter(|e| !e.destroyed) {
                        let cmd: Vec<String> = commands.as_ref().map(|c| c.queue(e.id).map(|c| format!("{:?}/{:?}/{:?}/n{}", c.kind, c.target, c.blueprint, c.id)).collect()).unwrap_or_default();
                        let motion = e.unit.as_deref().map(|u| format!("{:?} path {:?}", u.motion_state, u.path.as_ref().map(|p| (p.goal, p.next, p.waypoints.len()))));
                        out.push_str(&format!("{} {:?} {:?} {} {:?} {:?} {} {} {} cmd {:?} motion {:?}\n", e.id, e.kind, e.army, e.cold.blueprint_id, e.position(), e.orientation, e.health, e.max_health, e.fraction_complete, cmd, motion));
                    }
                    let _ = std::fs::write(path, out);
                }
                if mismatch {
                    return Ok(());
                }
            }
            if i % 1000 == 0 {
                let ms = lap.elapsed().as_secs_f64() * 1000.0 / 1000.0;
                let entities = self.sim_lua.app_data_ref::<sim::entity::EntityWorld>().map_or(0, |w| w.len());
                // The state checksum: two runs of one setup must agree at
                // every line (lockstep determinism).
                let checksum = format!("{:016x}", self.checksum());
                info!(tick = i, of = n, ms_per_tick = %format!("{ms:.2}"), entities, checksum = %checksum, lua_bytes = self.sim_lua.used_memory(), "progress");
                if let Some(mut t) = self.sim_lua.app_data_mut::<watchdog::StageTimes>() {
                    info!(tick = i, "stage ms/tick: {}", t.report(1000));
                }
                lap = std::time::Instant::now();
            }
        }
        if n % 1000 != 0
            && let Some(mut t) = self.sim_lua.app_data_mut::<watchdog::StageTimes>() {
            info!(tick = n, "stage ms/tick: {}", t.report(n % 1000));
        }
        info!(ticks = n, "sim ticks complete");
        if let Some(armies) = self.sim_lua.app_data_ref::<sim::army::Armies>() {
            for a in &armies.list {
                let (m, e) = (&a.economy.mass, &a.economy.energy);
                info!(
                    army = a.index,
                    mass = %format!("{}/{} +{}/tick -{}", m.stored, m.capacity, m.income, m.expense),
                    energy = %format!("{}/{} +{}/tick -{}", e.stored, e.capacity, e.income, e.expense),
                    "economy"
                );
            }
        }
        self.log_net_stats();
        Ok(())
    }

    /// End of a networked run: the lockstep's totals.
    fn log_net_stats(&self) {
        if let Some(l) = self.sim_lua.app_data_ref::<crate::net::lockstep::Lockstep>() {
            info!(stats = ?l.stats, delay = l.delay(), lost = ?l.lost(), "lockstep: totals");
        }
    }

    /// Runs the sim at moho's 10 ticks per second until an error. Wall-clock
    /// time only paces the loop; it never reaches sim state.
    pub fn run_realtime(&mut self) -> LuaResult<()> {
        let control = self.control.clone().unwrap_or_else(|| snapshot::SimControl::new(100));
        let mut next = std::time::Instant::now();
        // Ticks run since when, and the speed (percent) they came to: what
        // the game really runs at when the machine can't keep up with the
        // speed asked for (or runs flat out, "max").
        let mut measured = (std::time::Instant::now(), 0u32);
        let mut achieved: Option<u32> = None;
        // Ticks that took longer than their real-time slot (waiting for
        // players excluded: the lockstep logs that), for the log.
        let mut slow_ticks = crate::diagnostics::Hitches::new("sim tick");
        loop {
            self.sync_net_pause(&control);
            let tick = self.sim_lua.app_data_ref::<sim::scheduler::Scheduler>().map_or(0, |s| s.tick());
            // The game is over (`EndGame`, the same tick on every client)
            // or the player left it (`SessionEndGame`): no more ticks. The
            // network stays up (chat, peers' leaving) and the renderer
            // gets the final snapshot.
            // The renderer moved on (the main menu, another session).
            if control.closed() {
                info!(tick, "session closed");
                api::replay::end_recording(&self.sim_lua);
                return Ok(());
            }
            if api::sim_time::has_ended(&self.sim_lua) || control.left() {
                api::replay::end_recording(&self.sim_lua);
                self.watchdog.beat();
                if let Some(mut l) = self.sim_lua.remove_app_data::<crate::net::lockstep::Lockstep>() {
                    l.pump(&self.sim_lua);
                    self.sim_lua.set_app_data(l);
                }
                if !snapshot::game_over_delivered(&self.sim_lua) && !control.left() {
                    snapshot::republish(&self.sim_lua);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            if control.paused() || tick >= control.hold_at() {
                // Paused on purpose (or held for a screenshot): not a stall.
                self.watchdog.beat();
                if tick >= control.hold_at() {
                    snapshot::republish(&self.sim_lua);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
                next = std::time::Instant::now();
                measured = (next, 0);
                continue;
            }
            let step_started = std::time::Instant::now();
            sim::tick::step(&self.sim_lua)?;
            self.after_step()?;
            self.note_net_tick(step_started);
            self.sync_net_speed(&control);
            let speed = self.net_pace(control.shown());
            if speed != snapshot::SimControl::MAX {
                let waited = self.sim_lua.app_data_ref::<crate::net::lockstep::Lockstep>().map_or(0.0, |l| l.last_wait_ms());
                let cost = step_started.elapsed().as_secs_f64() * 1000.0 - waited;
                slow_ticks.note(cost, 100.0 * 100.0 / speed.max(1) as f64, tick);
            }
            // 10 ticks a second is 100%.
            measured.1 += 1;
            let secs = measured.0.elapsed().as_secs_f64();
            if secs >= 0.5 {
                achieved = Some((measured.1 as f64 * 10.0 / secs).round() as u32);
                measured = (std::time::Instant::now(), 0);
            }
            // The speed shown as actual: what the loop paces at, unless it
            // measurably falls short (5% slack for timer jitter).
            let actual = match achieved {
                Some(a) if speed == snapshot::SimControl::MAX || (a as f64) < speed as f64 * 0.95 => a,
                _ => speed,
            };
            control.set_effective(actual);
            if speed == snapshot::SimControl::MAX {
                next = std::time::Instant::now();
                continue;
            }
            // 100 ms per tick at 1x.
            let factor = self.net_catch_up(speed);
            next += std::time::Duration::from_micros(((100_000 * 100 / speed.max(1) as u64) as f64 * factor) as u64);
            match next.checked_duration_since(std::time::Instant::now()) {
                Some(wait) => std::thread::sleep(wait),
                // Running behind: don't try to catch up in a burst.
                None => next = std::time::Instant::now(),
            }
        }
    }

    /// Runs `--ticks` in real time at `percent` speed (headless network
    /// tests), not as fast as possible.
    pub fn set_pace(&mut self, percent: u32) {
        self.pace = Some(percent);
    }

    /// Game speed changes: one applied this tick (a networked game's, by
    /// any player, or a replay's) becomes the speed; in a networked game a
    /// speed set here is only a request, sent to everyone as a command
    /// (`net::lockstep::request_speed`), or undone if the lobby fixed it.
    fn sync_net_speed(&self, control: &snapshot::SimControl) {
        if let Some(c) = self.sim_lua.remove_app_data::<crate::net::lockstep::SpeedChanged>() {
            if c.networked {
                control.set_agreed(c.percent, c.by);
            } else {
                control.set_speed(c.percent);
            }
        }
        let Some(mut l) = self.sim_lua.app_data_mut::<crate::net::lockstep::Lockstep>() else {
            control.set_speed_fixed(false);
            return;
        };
        control.set_speed_fixed(!l.speed_adjustable());
        let (wanted, agreed) = (control.speed(), l.agreed_speed());
        if wanted != agreed && !l.request_speed(wanted) {
            info!("game speed is fixed by the lobby's Game Speed option");
            control.set_speed(agreed);
        }
    }

    /// The speed to pace at: in a networked game no faster than the slowest
    /// machine keeps up with (`net::lockstep`).
    fn net_pace(&self, requested: u32) -> u32 {
        match self.sim_lua.app_data_mut::<crate::net::lockstep::Lockstep>() {
            Some(mut l) if requested != snapshot::SimControl::MAX => l.effective_speed(requested),
            _ => requested,
        }
    }

    /// A networked game: pausing is everyone's. A pause or resume from
    /// another player is applied here; ours goes to them. While paused no
    /// tick runs, so the network is read here too.
    fn sync_net_pause(&self, control: &snapshot::SimControl) {
        let Some(mut l) = self.sim_lua.remove_app_data::<crate::net::lockstep::Lockstep>() else { return };
        if control.paused() || l.paused {
            l.pump(&self.sim_lua);
        }
        match l.pause_request.take() {
            Some(p) => control.set_paused(p),
            None => l.set_paused(control.paused()),
        }
        self.sim_lua.set_app_data(l);
    }

    /// A networked game: the share of a tick period to wait (below 1 while
    /// this client catches up with the others).
    fn net_catch_up(&self, speed: u32) -> f64 {
        match self.sim_lua.app_data_mut::<crate::net::lockstep::Lockstep>() {
            Some(mut l) => l.pace_factor(100.0 * 100.0 / speed.max(1) as f64),
            None => 1.0,
        }
    }

    /// A networked game: what this tick cost, for its speed capacity.
    fn note_net_tick(&self, started: std::time::Instant) {
        if let Some(mut l) = self.sim_lua.app_data_mut::<crate::net::lockstep::Lockstep>() {
            l.note_tick(started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// Connects the local player's command channel (windowed runs).
    pub fn set_input_link(&mut self, link: sim::input::InputLink) {
        api::player_input::set_link(&self.sim_lua, link);
    }

    /// Writes every applied player command to `path` (a replay), after a
    /// header naming this build and the session, written as it starts.
    pub fn record_commands(&mut self, path: &Path) -> std::io::Result<()> {
        let f = std::fs::File::create(path)?;
        self.sim_lua.set_app_data(sim::input::Recorder(std::io::BufWriter::new(f)));
        Ok(())
    }

    /// Records the game into a new file in `dir` once the session starts,
    /// named by the time and map (`replay::default_path`), keeping the
    /// newest `replay::KEEP_REPLAYS` there.
    pub fn record_into(&mut self, dir: &Path) {
        self.record_dir = Some(dir.to_path_buf());
    }

    /// Opens the default recording (if asked) and writes the replay header.
    fn start_recording(&mut self, session: &session::SessionConfig) {
        if let Some(dir) = self.record_dir.take() {
            let path = replay::default_path(&dir, std::time::SystemTime::now(), session);
            match std::fs::create_dir_all(&dir).and_then(|_| self.record_commands(&path)) {
                Ok(()) => {
                    info!(path = %path.display(), "recording the game");
                    replay::prune(&dir, replay::KEEP_REPLAYS);
                }
                Err(e) => tracing::warn!(path = %path.display(), "not recording the game: {e}"),
            }
        }
        if let Some(mut r) = self.sim_lua.app_data_mut::<sim::input::Recorder>()
            && let Err(e) = replay::write_header(&mut r.0, &crate::net::core::build_id(), session) {
            tracing::warn!("replay header: {e}");
        }
    }

    /// Queues the commands of a recorded replay (`record_commands`), and
    /// returns how many and the replay's header (its build and session).
    /// A recording from another build may play differently: warned.
    pub fn replay_commands(&mut self, path: &Path) -> Result<(usize, replay::Header), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let header = replay::read_header(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let ours = crate::net::core::build_id();
        match &header.build {
            Some(b) if *b != ours => tracing::warn!(recorded = %b, running = %ours, "this replay was recorded by another build: it may play differently"),
            None => tracing::warn!("this replay has no header (an older recording): the session must be given as when it was recorded"),
            _ => {}
        }
        let commands: Vec<sim::input::Scheduled> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|(i, l)| sim::input::decode(l).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1)))
            .collect::<Result<_, _>>()?;
        let n = commands.len();
        let timeline = replay::read_timeline(&text);
        if timeline.marked {
            self.sim_lua.set_app_data(api::replay::ReplayEnd(timeline.length));
        }
        self.sim_lua.set_app_data(api::army::Replay(true));
        api::player_input::schedule(&self.sim_lua, commands);
        Ok((n, header))
    }

    /// Playing a recording (`replay_commands`): nothing to record.
    pub fn is_replay(&self) -> bool {
        self.sim_lua.app_data_ref::<api::army::Replay>().is_some_and(|r| r.0)
    }

    /// Game speed control for the real-time loop (windowed runs).
    pub fn set_sim_control(&mut self, control: std::sync::Arc<snapshot::SimControl>) {
        self.control = Some(control);
    }

    /// Runs `/lua/simInit.lua`, which sets up the class system, globals and the
    /// session entry points (`SetupSession`, `BeginSession`, ...).
    ///
    /// TODO: moho loads blueprints in a separate rules VM and hands the sim VM a
    /// pre-filled `__blueprints`; for now both run in the same VM.
    fn init_sim(&mut self) -> LuaResult<()> {
        info!("running /lua/simInit.lua");
        let doscript: LuaFunction = self.sim_lua.globals().get("doscript")?;
        doscript.call::<()>("/lua/simInit.lua")?;
        info!("sim init complete");
        Ok(())
    }

    /// The session's sim mods (the launch's `GameMods` and `--mod`) become
    /// the sim's `__active_mods` and hook its scripts, before blueprints
    /// load. Published for the interface either way, so it never waits on
    /// a sim that found none.
    fn activate_mods(&mut self, session: &session::SessionConfig) -> LuaResult<()> {
        let vfs = self.vfs.clone();
        let vfs = vfs.read().unwrap();
        let all = Arc::new(mods::discover(&vfs));
        let mut uids = session.mods.clone();
        uids.extend(session.launch.as_ref().map(mods::uids_from_launch).unwrap_or_default());
        let (ui, sim): (Vec<_>, Vec<_>) = mods::resolve(&all, &uids).into_iter().partition(|m| m.ui_only);
        mods::publish(mods::SessionMods { sim: sim.clone(), ui: ui.iter().map(|m| m.uid.clone()).collect(), all: all.clone() });
        info!(installed = all.len(), sim = sim.len(), ui = ui.len(), "mods");
        mods::log_active("sim", &sim);
        mods::activate(&self.sim_lua, &vfs, &sim)?;
        if sim.is_empty() {
            self.sim_lua.remove_app_data::<mods::ModsHash>();
        } else {
            let hash = mods::hash(&vfs, &sim);
            info!(hash = %format!("{hash:016x}"), "sim mods hashed (part of the sim checksum)");
            self.sim_lua.set_app_data(mods::ModsHash(hash));
        }
        Ok(())
    }

    fn load_blueprints(&mut self) -> LuaResult<()> {
        let file_name = "lua/RuleInit.lua";
        let code = {
            let vfs = self.vfs.read().unwrap();
            mods::script_source(&self.sim_lua, &vfs, file_name).map_err(|e| LuaError::runtime(format!("{file_name}: {e}")))?
        };

        exec_lua_string(&self.sim_lua, &String::from_utf8_lossy(&code), file_name)?;
        Ok(())
    }
}

#[derive(Clone)]
struct InitScriptOutput {
    mount_points: Vec<MountPoint>,
    hook_paths: Vec<String>,
    /// The player's vault (FAF's `custom_vault_path`, else the game's
    /// folder in My Documents, as FAF's init file falls back to): its
    /// `maps` folder is where the lobby downloads maps to.
    vault: PathBuf,
}

/// FAF's init script finds the install and names the mounts: the same
/// paths give the same mounts, so it runs once per process and every
/// session shares its output.
fn init_output(init_path: &str, fa_path: &str) -> LuaResult<Arc<InitScriptOutput>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), Arc<InitScriptOutput>>>> = OnceLock::new();
    let key = (init_path.to_string(), fa_path.to_string());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    if let Some(out) = cache.get(&key) {
        return Ok(out.clone());
    }
    let out = Arc::new(run_init_script(init_path, fa_path)?);
    cache.insert(key, out.clone());
    Ok(out)
}

/// The game files of one set of mounts: the VFS index, the compiled scripts
/// and the debug UI's file tree. The same mounts give the same files, so
/// every session in the process shares one.
struct GameFiles {
    vfs: Arc<RwLock<Vfs>>,
    scripts: Arc<ScriptCache>,
    tree: Arc<FsDir>,
}

/// Identifies a set of mounts (the sources and where they are mounted).
#[derive(PartialEq, Eq, Hash)]
struct MountKey {
    mounts: Vec<(PathBuf, String)>,
    hooks: Vec<String>,
}

/// Where `--vfs-dump` writes the mounted paths; unset = no dump. Set once at
/// startup, before the game files are first built (they're built once per
/// process and mount set, so only the first build dumps).
static VFS_DUMP: OnceLock<std::path::PathBuf> = OnceLock::new();

/// `--vfs-dump FILE`: dump the VFS paths to `path` when the game files are built.
pub fn set_vfs_dump(path: std::path::PathBuf) {
    let _ = VFS_DUMP.set(path);
}

static GAME_FILES: OnceLock<Mutex<HashMap<MountKey, Arc<GameFiles>>>> = OnceLock::new();

/// The game files for these mounts, built once per process and shared by
/// every session that mounts the same files.
fn game_files(mount_points: Vec<MountPoint>, hook_paths: Vec<String>) -> LuaResult<Arc<GameFiles>> {
    let key = MountKey {
        mounts: mount_points.iter().map(|m| (m.source.clone(), m.mountpoint.clone())).collect(),
        hooks: hook_paths.clone(),
    };
    let cache = GAME_FILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    if let Some(files) = cache.get(&key) {
        return Ok(files.clone());
    }
    let files = build_game_files(mount_points, hook_paths)?;
    cache.insert(key, files.clone());
    Ok(files)
}

/// Reads the mounted files, translates the scripts and starts the background
/// work the sessions share.
fn build_game_files(mount_points: Vec<MountPoint>, hook_paths: Vec<String>) -> LuaResult<Arc<GameFiles>> {
    let mut vfs = {
        let _s = crate::prof_span!("vfs index");
        Vfs::new_with_embedded(mount_points, hook_paths, LUA_OVERRIDES)?
    };
    let code = GameCode::detect(&vfs);
    info!(game_code = code.name(), "game code");
    if !code.engine_hooks() {
        vfs.remove_hook_dir(ENGINE_HOOK);
    }
    // Lua files are translated from SupCom Lua to Luau once on first read and
    // cached by the VFS, rather than re-translated on every dofile/doscript.
    // Blueprints are Lua too: the base game's (and LOUD's, and older mods')
    // have `#` comments after fields (`StartColor = {...}, # R,G,B,A`).
    let translate: vfs::Transform = Arc::new(|b: &[u8]| lua_translation::translate(&String::from_utf8_lossy(b)).into_bytes());
    vfs.register_transform("lua", translate.clone());
    vfs.register_transform("bp", translate);
    let vfs = Arc::new(RwLock::new(vfs));

    let tree = {
        let _s = crate::prof_span!("vfs tree");
        let v = vfs.read().unwrap();
        let tree = Arc::new(FsDir::build_from_vfs(&v));
        // A debugging aid, only with `--vfs-dump`.
        if let Some(path) = VFS_DUMP.get() {
            let shown = path.display();
            match v.dump_paths(&path.to_string_lossy()) {
                Ok(()) => info!("vfs paths dumped to {shown}"),
                Err(e) => warn!("failed to dump vfs paths to {shown}: {e}"),
            }
        }
        tree
    };

    // For the network version check, off the startup path.
    crate::net::core::hash_code_in_background(vfs.clone());
    // And the installed sim mods, which the lobby compares before a launch.
    mods::hash_installed_in_background(vfs.clone());

    // Translate + compile every script on all cores in the background while
    // the (single-threaded) Lua VM starts executing; misses compile inline.
    let script_cache = Arc::new(ScriptCache::new(vfs.clone()));
    {
        let script_cache = script_cache.clone();
        std::thread::Builder::new()
            .name("script-prewarm".into())
            .spawn(move || {
                crate::engine::tracy::name_current();
                let _s = crate::prof_span!("script prewarm");
                script_cache.prewarm()
            })
            .map_err(LuaError::external)?;
    }

    Ok(Arc::new(GameFiles { vfs, scripts: script_cache, tree }))
}

fn run_init_script(init_path: &str, fa_path: &str) -> LuaResult<InitScriptOutput> {
    let lua = Lua::new();
    lua.set_compiler(lua::script_cache::compiler());

    // Empty VFS for the throwaway VM so io.dir/dofile work during init.
    // lua.set_app_data(Arc::new(RwLock::new(Vfs::new())));
    api::register_unsafe(&lua)?;

    let init_dir = Path::new(init_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .replace('\\', "/");

    lua.globals().set("InitFileDir", init_dir.as_str())?;
    lua.globals().set("fa_path", fa_path)?;
    // FAF's init files `dofile` the client's `fa_path.lua`, which names the
    // install the FAF client plays with; the one the engine found wins.
    lua.load(
        r#"
        local dofile, fa_path = dofile, fa_path
        _G.dofile = function(path)
            local r = table.pack(dofile(path))
            if string.find(string.lower(path), "fa_path.lua$") then _G.fa_path = fa_path end
            return unpack(r, 1, r.n)
        end
        "#,
    )
    .set_name("=[engine init dofile]")
    .exec()?;

    let raw = std::fs::read_to_string(init_path).map_err(LuaError::external)?;
    let raw = with_repository_location(&raw, init_path);

    let code = lua_translation::translate(&raw);
    exec_lua_string(&lua, &code, init_path)?;

    let path_val: LuaValue = lua.globals().get("path")?;
    info!(
        "init.lua path table:\n{}",
        lua::lua_pretty::PrettyLua(path_val)
    );

    let path_table: LuaTable = match lua.globals().get("path")? {
        LuaValue::Table(t) => t,
        _ => {
            warn!("no `path` table after init.lua — VFS will be empty");
            return Ok(InitScriptOutput {
                mount_points: vec![],
                hook_paths: vec![],
                vault: init_vault(&lua),
            });
        }
    };

    // Moho's `mount_dir` takes Windows paths (`InitFileDir .. '\\..\\gamedata'`)
    // and a wildcard in the last part (`*.scd`): each match is mounted, in
    // name order.
    let mut specs = Vec::new();
    for entry in path_table.sequence_values::<LuaTable>() {
        let entry = entry?;
        let (dir, mountpoint): (String, String) = (entry.get("dir")?, entry.get("mountpoint")?);
        specs.extend(api::io::expand_disk_glob(&dir).into_iter().map(|source| MountPoint { source, mountpoint: mountpoint.clone() }));
    }

    // The folders whose files hook the scripts of the same path (FAF: `/schook`).
    let hook_paths = match lua.globals().get("hook")? {
        LuaValue::Table(t) => t.sequence_values::<String>().collect::<LuaResult<_>>()?,
        _ => vec![],
    };

    // `lua` dropped here — throwaway VM discarded.
    Ok(InitScriptOutput {
        mount_points: specs,
        hook_paths,
        vault: init_vault(&lua),
    })
}

/// The vault the init file loaded maps and mods from: the FAF client's
/// `custom_vault_path` (set by its `fa_path.lua`), else My Documents'.
fn init_vault(lua: &Lua) -> PathBuf {
    match lua.globals().get::<Option<String>>("custom_vault_path") {
        Ok(Some(path)) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(api::sh::documents_dir() + "My Games/Gas Powered Games/Supreme Commander Forged Alliance"),
    }
}

/// The engine's replacements for FAF files, built into the binary and
/// mounted ahead of everything else.
/// Files under [`ENGINE_HOOK`] are the engine's own hooks, applied after
/// the init file's.
const LUA_OVERRIDES: &[(&str, &[u8])] = &[
    ("/engine_hook/lua/gamecolors.lua", include_bytes!("../lua_overrides/engine_hook/lua/gamecolors.lua")),
    ("/engine_hook/lua/ui/lobby/lobby.lua", include_bytes!("../lua_overrides/engine_hook/lua/ui/lobby/lobby.lua")),
    ("/engine_hook/lua/ui/lobby/lobbycomm.lua", include_bytes!("../lua_overrides/engine_hook/lua/ui/lobby/lobbycomm.lua")),
    // Picture-in-picture popups.
    ("/lua/sim/pip.lua", include_bytes!("../lua_overrides/lua/sim/pip.lua")),
    ("/lua/ui/game/pip.lua", include_bytes!("../lua_overrides/lua/ui/game/pip.lua")),
    // The replay timeline.
    ("/lua/ui/game/timeline.lua", include_bytes!("../lua_overrides/lua/ui/game/timeline.lua")),
    ("/engine_hook/lua/sim/Unit.lua", include_bytes!("../lua_overrides/engine_hook/lua/sim/Unit.lua")),
    ("/engine_hook/lua/sim/units/ACUUnit.lua", include_bytes!("../lua_overrides/engine_hook/lua/sim/units/ACUUnit.lua")),
    // The in-game tabs' exit names the platform it exits to.
    #[cfg(not(windows))]
    ("/engine_hook/lua/system/Localization.lua", include_bytes!("../lua_overrides/engine_hook/lua/system/Localization.lua")),
    // `debug.allocatedrsize` native (`src/engine/api/debug.rs`).
    ("/engine_hook/lua/system/debug.lua", include_bytes!("../lua_overrides/engine_hook/lua/system/debug.lua")),
    ("/engine_hook/lua/ui/game/gamemain.lua", include_bytes!("../lua_overrides/engine_hook/lua/ui/game/gamemain.lua")),
    ("/engine_hook/lua/options/options.lua", include_bytes!("../lua_overrides/engine_hook/lua/options/options.lua")),
];

/// The engine's hook folder, after the init file's `hook` folders: its
/// files are appended to FAF's scripts of the same path, so FAF's files
/// stay untouched and FAF's own hooks still apply.
const ENGINE_HOOK: &str = "/engine_hook";

/// FAF's development init file mounts the repository at
/// `local locationOfRepository = '<path>'`, a path each developer edits
/// in by hand; point it at the checkout the init file belongs to, so any
/// checkout works from anywhere. Other init files (`= false`) are left alone.
fn with_repository_location(code: &str, init_path: &str) -> String {
    let re = lazy_regex::regex!(r#"(?m)^local locationOfRepository\s*=\s*['"][^'"\n]*['"]"#);
    let Some(root) = repository_root(Path::new(init_path)) else { return code.to_string() };
    let dir = std::path::absolute(&root).unwrap_or(root).to_string_lossy().replace('\\', "/");
    re.replace(code, |_: &lazy_regex::Captures| format!("local locationOfRepository = '{dir}'")).into_owned()
}

/// The FAF checkout an init file belongs to: the nearest folder above it
/// holding `lua/version.lua` (the init file sits at the root before FAF
/// 3839, in `setup/bin/` since), else the init file's own folder.
pub fn repository_root(init_path: &Path) -> Option<PathBuf> {
    let dir = init_path.parent()?;
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    Some(dir.ancestors().find(|d| d.join("lua/version.lua").is_file()).unwrap_or(dir).to_path_buf())
}

/// Mount points for every map folder in `<vault>/maps`, then every mod
/// folder in `<vault>/mods` (FAF's `MountModContent`: `/mods/<name>`), in
/// name order. A vault without mods has none.
fn vault_maps(vault: &Path) -> std::io::Result<Vec<vfs::MountPoint>> {
    let mut mounts = vault_folders(&vault.join("maps"), "maps")?;
    if vault.join("mods").is_dir() {
        mounts.extend(vault_folders(&vault.join("mods"), "mods")?);
    }
    Ok(mounts)
}

fn vault_folders(dir: &Path, kind: &str) -> std::io::Result<Vec<vfs::MountPoint>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs
        .into_iter()
        .filter_map(|d| {
            let name = d.file_name()?.to_str()?.to_lowercase();
            Some(vfs::MountPoint { source: d, mountpoint: format!("/{kind}/{name}") })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FAF 3839 moved the development init file to `setup/bin/`: the
    /// repository is still the checkout's root.
    #[test]
    fn repository_root_is_the_checkout() {
        let dir = std::env::temp_dir().join(format!("repo_root_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("setup/bin")).unwrap();
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(dir.join("lua/version.lua"), "").unwrap();
        assert_eq!(repository_root(&dir.join("setup/bin/init_local_development.lua")), Some(dir.clone()));
        assert_eq!(repository_root(&dir.join("init_local_development.lua")), Some(dir.clone()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The exit key takes the locale's `_Exit`: on load and after a
    /// language change, and only when the locale has an `_Exit` to give it.
    #[cfg(not(windows))]
    #[test]
    fn localization_hook_remaps_exit_to_windows() {
        let lua = Lua::new();
        let scenario: LuaTable = lua
            .load(format!(
                r#"
                local exit_text = "Exit"
                local loc_table
                local function loadLanguage(_)
                    loc_table = {{
                        _Exit = exit_text,
                        _Exit_to_Windows = "Exit to Windows",
                        _Save_Game = "Save Game",
                    }}
                end
                function language(la)
                    loadLanguage(la)
                end
                loadLanguage("us")
                {hook}
                return {{
                    get = function(key) return loc_table[key] end,
                    change_language = function(la) language(la) end,
                    no_exit = function() exit_text = nil end,
                }}
                "#,
                hook = include_str!("../lua_overrides/engine_hook/lua/system/Localization.lua"),
            ))
            .eval()
            .unwrap();
        let get = |key: &str| -> Option<String> {
            scenario.get::<LuaFunction>("get").unwrap().call(key).unwrap()
        };
        let change_language = |la: &str| {
            scenario.get::<LuaFunction>("change_language").unwrap().call::<()>(la).unwrap()
        };
        assert_eq!(get("_Exit_to_Windows"), Some("Exit".into()));
        assert_eq!(get("_Save_Game"), Some("Save Game".into()), "other keys are untouched");
        change_language("de");
        assert_eq!(get("_Exit_to_Windows"), Some("Exit".into()), "a language change keeps the remap");
        scenario.get::<LuaFunction>("no_exit").unwrap().call::<()>(()).unwrap();
        change_language("de");
        assert_eq!(get("_Exit_to_Windows"), Some("Exit to Windows".into()), "no `_Exit` leaves FAF's label");
    }

    #[test]
    fn repository_location_points_at_the_init_files_folder() {
        let code = "-- local locationOfRepository = 'your-fa-repository-location'\nlocal locationOfRepository = 'your-fa-repository-location'\nMountDirectory(locationOfRepository, '/')\n";
        let out = with_repository_location(code, "some/where/fa/init_local_development.lua");
        let dir = std::path::absolute("some/where/fa").unwrap().to_string_lossy().replace('\\', "/");
        assert_eq!(out, format!("-- local locationOfRepository = 'your-fa-repository-location'\nlocal locationOfRepository = '{dir}'\nMountDirectory(locationOfRepository, '/')\n"));
        // The FAF client's init files mount nx2 archives instead.
        let client = "local locationOfRepository = false\n";
        assert_eq!(with_repository_location(client, "C:/ProgramData/FAForever/bin/init_faf.lua"), client);
    }
}
