//! The connection mesh: one UDP socket per client that both listens and
//! connects, with a reliable ordered [`Link`] per peer.
//!
//! Lobby shape (moho's CLobby): one client hosts; a joiner connects to the
//! host, which gives it an id and the other members' addresses, and the
//! joiner then connects to each of them, so every pair of clients has its
//! own link (a full mesh, as SupCom's peer-to-peer lockstep needs: a turn
//! goes straight to everyone, never through a relay).
//!
//! Threads: a network thread receives packets, resends what wasn't acked,
//! sends keepalives and notices peers that went quiet. Sending (from the
//! lobby interface or the sim thread) queues bytes on the peer's link and
//! transmits at once from the caller's thread, so a turn doesn't wait for
//! the network thread's next pass. Everything that happens is queued as an
//! [`Event`]: the lobby interface drains the queue each frame, and once a
//! game is launched the lockstep does (whatever arrives in between waits).
//!
//! Packets go over UDP or, for peers joined by internet join code, over
//! iroh ([`Transport`]); the mesh doesn't distinguish them beyond member
//! addresses. UDP has no encryption or authentication (LAN and direct
//! connect between people who trust each other, like SupCom's LAN mode);
//! iroh's connections are encrypted and authenticated by their keys.

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::link::{Link, MTU, Packet};
use super::transport::{self, Transport};
use super::wire::{MAX_FRAME, Msg, PROTOCOL, PathKind, PeerAddr, PeerPing};

/// FAF's `gamecreate.lua` default when the port is left to the engine.
pub const DEFAULT_PORT: u16 = 16010;
/// How many ports up from the default to try when it's taken (several
/// clients on one machine).
const PORT_TRIES: u16 = 64;
/// A peer not heard from for this long is gone.
const LOST_AFTER: Duration = Duration::from_secs(20);
/// A connection that hasn't finished its handshake by now is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// The same over iroh: finding the host by its join code (n0's DNS
/// lookup), then the relay and hole punching, can take well over 10 s on a
/// slow or busy machine; a join dropped mid-way leaves FAF's joiner on
/// "Connecting..." (its ConnectionFailed dialog needs the lobby panel).
const IROH_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the network thread sleeps in `recv` when idle (its resend and
/// keepalive resolution).
const POLL: Duration = Duration::from_millis(2);
/// How often each client tells the others its pings (the ping grid).
const PINGS_EVERY: Duration = Duration::from_secs(1);

/// What happened, in arrival order.
#[derive(Clone, Debug)]
pub enum Event {
    /// `HostGame` took effect.
    Hosting,
    /// A join was accepted by the host.
    Welcomed { local_id: u32, host_id: u32 },
    /// A join failed before being accepted.
    JoinFailed { reason: String },
    PeerConnected { id: u32, name: String, uid: String },
    PeerDisconnected { id: u32, name: String, reason: String },
    Message { from: u32, msg: Msg },
}

/// A client of the game being played (`GetSessionClients`), in command
/// source order.
#[derive(Clone, Debug)]
pub struct SessionClient {
    /// Command source (0-based).
    pub source: u32,
    pub peer: u32,
    pub name: String,
    pub local: bool,
    pub connected: bool,
    /// When its last turn arrived (`None`: none yet; it may still be
    /// loading).
    pub last_turn: Option<Instant>,
    /// Peers that voted to eject it.
    pub ejected_by: Vec<u32>,
}

/// A member as `GetPeers` shows it.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub id: u32,
    pub name: String,
    pub uid: String,
    pub addr: SocketAddr,
    pub ping_ms: f64,
    /// 99th percentile of the recent round trips (the spikes).
    pub ping99_ms: f64,
    pub quiet_ms: f64,
    /// Who this member says it is connected to.
    pub established: Vec<u32>,
}

enum Role {
    /// Someone connected to us; waiting for their `Hello`.
    Incoming,
    /// We asked the host to join; waiting for `Welcome`, then its `Hello`.
    Joining,
    Welcomed { your_id: u32, host_id: u32, others: Vec<PeerAddr> },
    /// We connected to another member to complete the mesh.
    Mesh { id: u32, name: String },
    /// A lobby member.
    Member(u32),
}

#[derive(Clone, Copy)]
enum Kind {
    Incoming,
    Joining,
    Welcomed,
    Mesh(u32),
    Member(u32),
}

struct Conn {
    link: Link,
    /// The far end's per-process nonce: a new one at the same address is a
    /// restarted client, not this connection.
    their_nonce: u32,
    role: Role,
    opened: Instant,
    /// The last send to this address failed (logged once per run of failures).
    send_failing: bool,
}

struct Member {
    addr: SocketAddr,
    name: String,
    uid: String,
    /// Where it accepts connections (as seen from here).
    listen: SocketAddr,
    /// Its internet join code (empty: iroh off there).
    iroh: String,
    established: Vec<u32>,
    /// Its pings to the others, as it last reported them.
    pings: Vec<PeerPing>,
    /// Its installed sim mods (uid, hash) once it has said (`Msg::Mods`).
    mods: Option<Vec<(String, u64)>>,
    /// We've sent it ours (they go once our hashing is done).
    sent_mods: bool,
}

/// Everyone's ping to everyone (the ping grid): `rows[a]` holds `a`'s round
/// trip to each other member, ours measured now, the rest as reported.
#[derive(Clone, Debug, Default)]
pub struct PingGrid {
    /// (id, name) by id, us included.
    pub players: Vec<(u32, String)>,
    pub local: u32,
    pub rows: BTreeMap<u32, Vec<PeerPing>>,
    /// The fastest game speed (percent) each player's machine keeps up
    /// with, once a game runs (`lockstep::capacity`); none in the lobby.
    pub max_speed: BTreeMap<u32, u32>,
    /// Each player's input delay (ticks: how far ahead its orders are
    /// scheduled), once a game runs.
    pub input_delay: BTreeMap<u32, u32>,
}

#[derive(Default)]
struct State {
    hosting: bool,
    local_id: Option<u32>,
    host_id: Option<u32>,
    next_id: u32,
    conns: BTreeMap<SocketAddr, Conn>,
    members: BTreeMap<u32, Member>,
    /// Addresses we closed: their packets are ignored for a while (until
    /// they notice).
    closed_addrs: BTreeMap<SocketAddr, (u32, Instant)>,
    launched: bool,
    closed: bool,
    pings_sent: Option<Instant>,
}

pub struct NetCore {
    name: String,
    uid: String,
    port: u16,
    nonce: u32,
    transport: Transport,
    state: Mutex<State>,
    events: Mutex<VecDeque<Event>>,
    /// Signalled on every new event (the lockstep waits on it).
    arrived: Condvar,
    stop: AtomicBool,
    /// The game's clients, once one is launched (set by the lockstep).
    session: Mutex<Vec<SessionClient>>,
    /// Each player's machine's fastest game speed (percent), by peer id,
    /// from the lockstep's turns (the ping grid shows them).
    max_speeds: Mutex<BTreeMap<u32, u32>>,
    /// Each player's input delay (ticks), by peer id, from their turns.
    input_delays: Mutex<BTreeMap<u32, u32>>,
    /// Chat received in game (sender name, message), for the interface.
    chat: Mutex<VecDeque<(String, crate::engine::sim::lua_data::LuaData)>>,
    /// The networked game is paused (nobody counts as quiet).
    session_paused: AtomicBool,
    /// Pauses and resumes for the interface (FAF's `gamemain.OnPause` /
    /// `OnResume`): paused, and by which peer.
    pause_events: Mutex<VecDeque<(bool, u32)>>,
    /// Desyncs for the interface (FAF's `uimain.ShowDesyncDialog`): the
    /// tick and a line per differing checksum.
    desyncs: Mutex<VecDeque<(u64, Vec<String>)>>,
    /// Desyncs this game so far (the stats bar's badge).
    desync_count: std::sync::atomic::AtomicU64,
    /// When the game started (a peer that hasn't sent a turn yet counts as
    /// loading for a while, not quiet).
    session_started: Mutex<Option<Instant>>,
    /// Our installed sim mods as sent to members (tests set their own;
    /// else `mods::installed_now`).
    local_mods: Mutex<Option<Vec<(String, u64)>>>,
    /// Tests: our mods are still being hashed.
    local_mods_pending: AtomicBool,
}

/// The current lobby or game connection, shared by the interface (which
/// creates it) and the sim thread (which plays the game over it).
static CURRENT: Mutex<Option<Arc<NetCore>>> = Mutex::new(None);

pub fn current() -> Option<Arc<NetCore>> {
    CURRENT.lock().ok().and_then(|c| c.clone())
}

/// Ends the current connection (telling every peer), if any.
pub fn close_current() {
    let net = CURRENT.lock().ok().and_then(|mut c| c.take());
    if let Some(net) = net {
        net.close();
    }
}

/// The game code the engine loaded ([`Vfs::code_hash`](crate::engine::vfs::Vfs::code_hash)),
/// hashed on a background thread once the file system is up.
static CODE_HASH: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
static HASHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The FAF release the Lua says it is (`/lua/version.lua`), for people.
static FAF_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// "FAF 3839" from FAF's `/lua/version.lua` (`local Version = "3839"`),
/// "(FAF client)" when its deployment stamped it (`GameType = "FAF"`).
pub fn faf_version(version_lua: &str) -> String {
    let field = |name| crate::engine::api::core::version_lua_field(version_lua, name);
    match (field("Version"), field("GameType")) {
        (Some(v), Some(t)) if t == "FAF" => format!("FAF {v} (FAF client)"),
        (Some(v), _) => format!("FAF {v}"),
        (None, _) => "FAF ?".to_string(),
    }
}

/// Hashes the game code on its own thread (seconds from a cold disk, tens
/// of milliseconds warm); [`build_id`] waits for it if asked first. The
/// hash is of the whole game code, so it is taken once per process: a
/// second session would only repeat the work.
pub fn hash_code_in_background(vfs: Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>) {
    static SPAWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SPAWNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    HASHING.store(true, std::sync::atomic::Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("code-hash".into()).spawn(move || {
        let start = Instant::now();
        if let Some(v) = vfs.read().ok().and_then(|v| v.read("/lua/version.lua").ok()) {
            let _ = FAF_VERSION.set(faf_version(&String::from_utf8_lossy(&v)));
        }
        let hash = vfs.read().map_err(|e| e.to_string()).and_then(|v| v.code_hash().map_err(|e| e.to_string()));
        let hash = hash.unwrap_or_else(|e| {
            tracing::error!("hashing the game code failed: {e}");
            0
        });
        let _ = CODE_HASH.set(hash);
        tracing::info!(build = %build_id(), ms = start.elapsed().as_millis() as u64, "game code hashed");
    });
    if spawned.is_err() {
        let _ = CODE_HASH.set(0);
    }
}

/// The refusal shown to a joiner whose build differs (FAF's "Connection
/// failed" dialog): both builds, and in words what differs.
pub fn mismatch(ours: &str, theirs: &str) -> String {
    // `<engine>, FAF <release>[ (FAF client)], Lua <hash>`
    let parts = |b: &str| -> (String, String) {
        let mut p = b.split(", ");
        (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string())
    };
    let ((our_engine, our_faf), (their_engine, their_faf)) = (parts(ours), parts(theirs));
    let why = if our_engine != their_engine {
        "The engine builds differ: everyone needs the same download of the engine."
    } else if our_faf != their_faf {
        "The FAF versions differ: everyone needs the same FAF version (and the same kind: the FAF client's, or the same download of github.com/FAForever/fa)."
    } else {
        "Same FAF version but different FAF files: one copy is changed or incomplete. A fresh copy of the same FAF version fixes it."
    };
    format!("Different versions. This game runs {ours}; you have {theirs}. {why}")
}

/// Identifies the build in the handshake: the engine's version and commit,
/// and the hash of the FAF Lua it runs. Peers that differ in either would
/// desync, so the handshake refuses them.
pub fn build_id() -> String {
    let hash = if HASHING.load(std::sync::atomic::Ordering::SeqCst) { Some(*CODE_HASH.wait()) } else { CODE_HASH.get().copied() };
    let code = hash.map_or_else(|| "unknown".to_string(), |h| format!("{:08x}", h >> 32));
    let faf = FAF_VERSION.get().map_or("FAF ?", String::as_str);
    format!("{}+{}, {faf}, Lua {code}", env!("CARGO_PKG_VERSION"), env!("RTS_BUILD_ID"))
}

impl NetCore {
    /// Opens the socket on `port` (0: the default port, or the next free
    /// one above it), starts the network thread and makes it the current
    /// connection.
    pub fn create(port: u16, name: &str, uid: &str) -> Result<Arc<NetCore>, String> {
        close_current();
        let net = Self::open_with(port, name, uid, transport::iroh_enabled())?;
        *CURRENT.lock().map_err(|_| "network state poisoned")? = Some(net.clone());
        Ok(net)
    }

    /// Opens a LAN-only connection without making it the current one
    /// (tests run several clients in one process).
    pub fn open(port: u16, name: &str, uid: &str) -> Result<Arc<NetCore>, String> {
        Self::open_with(port, name, uid, false)
    }

    /// As [`open`](Self::open), with an iroh endpoint beside the socket
    /// when `iroh` (internet join codes).
    pub fn open_with(port: u16, name: &str, uid: &str, iroh: bool) -> Result<Arc<NetCore>, String> {
        let socket = bind(port)?;
        socket.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
        let transport = Transport::new(socket, iroh);
        let port = transport.local_port().map_err(|e| e.to_string())?;
        let nonce = {
            // Any value unlikely to repeat across restarts (not sim state).
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            (t.as_nanos() as u32) ^ std::process::id().rotate_left(16) | 1
        };
        let net = Arc::new(NetCore {
            name: name.to_owned(),
            uid: uid.to_owned(),
            port,
            nonce,
            transport,
            state: Mutex::new(State { next_id: 2, ..State::default() }),
            events: Mutex::new(VecDeque::new()),
            arrived: Condvar::new(),
            stop: AtomicBool::new(false),
            session: Mutex::new(Vec::new()),
            max_speeds: Mutex::new(BTreeMap::new()),
            input_delays: Mutex::new(BTreeMap::new()),
            chat: Mutex::new(VecDeque::new()),
            session_paused: AtomicBool::new(false),
            pause_events: Mutex::new(VecDeque::new()),
            desyncs: Mutex::new(VecDeque::new()),
            desync_count: std::sync::atomic::AtomicU64::new(0),
            session_started: Mutex::new(None),
            local_mods: Mutex::new(None),
            local_mods_pending: AtomicBool::new(false),
        });
        let worker = net.clone();
        std::thread::Builder::new().name("net".into()).spawn(move || worker.run()).map_err(|e| e.to_string())?;
        tracing::info!(port, name, "network: listening");
        Ok(net)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The code others paste into Direct Connect to join over the internet
    /// (`None`: iroh is off).
    pub fn join_code(&self) -> Option<String> {
        self.transport.join_code()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn local_id(&self) -> Option<u32> {
        self.state.lock().ok().and_then(|s| s.local_id)
    }

    pub fn host_id(&self) -> Option<u32> {
        self.state.lock().ok().and_then(|s| s.host_id)
    }

    pub fn is_host(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.hosting)
    }

    /// Hosts: this client is member 1 and accepts joins.
    pub fn host(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.hosting = true;
            s.local_id = Some(1);
            s.host_id = Some(1);
        }
        self.push(Event::Hosting);
    }

    /// Joins the lobby hosted at `addr` (`host:port`; a name or an address).
    pub fn join(&self, addr: &str) {
        let target = match self.transport.resolve(addr) {
            Ok(a) => a,
            Err(reason) => return self.push(Event::JoinFailed { reason }),
        };
        let hello = self.hello(None).encode();
        let Ok(mut s) = self.state.lock() else { return };
        let mut conn = Conn { link: Link::new(Instant::now()), their_nonce: 0, role: Role::Joining, opened: Instant::now(), send_failing: false };
        conn.link.queue(&hello);
        s.conns.insert(target, conn);
        self.flush(&mut s, target);
        tracing::info!(%target, "network: joining");
    }

    /// Queues `msg` for one member (dropped if it's gone) and sends it.
    pub fn send(&self, to: u32, msg: &Msg) {
        let bytes = msg.encode();
        let Ok(mut s) = self.state.lock() else { return };
        let Some(addr) = s.members.get(&to).map(|m| m.addr) else { return };
        if let Some(c) = s.conns.get_mut(&addr) {
            c.link.queue(&bytes);
        }
        self.flush(&mut s, addr);
    }

    /// Queues `msg` for every member and sends it.
    pub fn broadcast(&self, msg: &Msg) {
        let bytes = msg.encode();
        let Ok(mut s) = self.state.lock() else { return };
        let addrs: Vec<SocketAddr> = s.members.values().map(|m| m.addr).collect();
        for addr in addrs {
            if let Some(c) = s.conns.get_mut(&addr) {
                c.link.queue(&bytes);
            }
            self.flush(&mut s, addr);
        }
    }

    /// Drops one member (`DisconnectFromPeer`); with a reason it is told
    /// first (`EjectPeer`).
    pub fn disconnect(&self, id: u32, eject: Option<&str>) {
        let Ok(mut s) = self.state.lock() else { return };
        let Some(addr) = s.members.get(&id).map(|m| m.addr) else { return };
        let bye = match eject {
            Some(reason) => Msg::Eject { reason: reason.to_owned() },
            None => Msg::Bye,
        };
        if let Some(c) = s.conns.get_mut(&addr) {
            c.link.queue(&bye.encode());
        }
        self.flush(&mut s, addr);
        self.drop_conn(&mut s, addr, eject.unwrap_or("Disconnected"));
    }

    /// No more joins: the game is starting.
    pub fn mark_launched(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.launched = true;
        }
    }

    pub fn launched(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.launched)
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        let Ok(s) = self.state.lock() else { return Vec::new() };
        s.members
            .iter()
            .filter_map(|(&id, m)| {
                let c = s.conns.get(&m.addr)?;
                Some(PeerInfo {
                    id,
                    name: m.name.clone(),
                    uid: m.uid.clone(),
                    addr: m.listen,
                    ping_ms: c.link.rtt_ms(),
                    ping99_ms: c.link.rtt_p99_ms().unwrap_or_else(|| c.link.rtt_ms()),
                    quiet_ms: c.link.last_heard.elapsed().as_secs_f64() * 1000.0,
                    established: m.established.clone(),
                })
            })
            .collect()
    }

    /// Everyone's ping to everyone, for the ping grid.
    pub fn ping_grid(&self) -> PingGrid {
        let Ok(s) = self.state.lock() else { return PingGrid::default() };
        let local = s.local_id.unwrap_or(0);
        let mut players: Vec<(u32, String)> = s.members.iter().map(|(&id, m)| (id, m.name.clone())).collect();
        players.push((local, self.name.clone()));
        players.sort_by_key(|p| p.0);
        let mut rows: BTreeMap<u32, Vec<PeerPing>> = s.members.iter().map(|(&id, m)| (id, m.pings.clone())).collect();
        rows.insert(local, self.own_pings(&s));
        let max_speed = self.max_speeds.lock().map(|m| m.clone()).unwrap_or_default();
        let input_delay = self.input_delays.lock().map(|m| m.clone()).unwrap_or_default();
        PingGrid { players, local, rows, max_speed, input_delay }
    }

    /// `peer` schedules its orders `ticks` ahead.
    pub fn set_input_delay(&self, peer: u32, ticks: u32) {
        if let Ok(mut m) = self.input_delays.lock() {
            m.insert(peer, ticks);
        }
    }

    /// `peer`'s machine keeps up with game speeds to `percent`.
    pub fn set_max_speed(&self, peer: u32, percent: u32) {
        if let Ok(mut m) = self.max_speeds.lock() {
            m.insert(peer, percent);
        }
    }

    fn own_pings(&self, s: &State) -> Vec<PeerPing> {
        s.members
            .iter()
            .filter_map(|(&peer, m)| {
                let c = s.conns.get(&m.addr)?;
                let micros = |ms: f64| (ms * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32;
                let rtt_us = micros(c.link.rtt_ms());
                let rtt99_us = c.link.rtt_p99_ms().map_or(rtt_us, micros);
                Some(PeerPing { peer, rtt_us, rtt99_us, path: self.transport.path(m.addr).unwrap_or(PathKind::Udp) })
            })
            .collect()
    }

    /// Our installed sim mods, (uid, hash) by uid; `None` while they're
    /// still being hashed (never waits: callers hold the state lock).
    fn local_mods(&self) -> Option<Vec<(String, u64)>> {
        if self.local_mods_pending.load(Ordering::SeqCst) {
            return None;
        }
        if let Some(m) = self.local_mods.lock().ok().and_then(|m| m.clone()) {
            return Some(m);
        }
        crate::engine::mods::installed_now().map(|i| i.iter().map(|(uid, m)| (uid.clone(), m.hash)).collect())
    }

    /// Tests: the installed mods this client tells members it has.
    pub fn set_local_mods(&self, mods: Vec<(String, u64)>) {
        if let Ok(mut m) = self.local_mods.lock() {
            *m = Some(mods);
        }
        self.local_mods_pending.store(false, Ordering::SeqCst);
    }

    /// Tests: this client's mods are still being hashed.
    pub fn set_local_mods_pending(&self) {
        self.local_mods_pending.store(true, Ordering::SeqCst);
    }

    /// Sends our mods to each member that hasn't had them, once they're
    /// hashed.
    fn send_mods(&self, s: &mut State) {
        if s.members.values().all(|m| m.sent_mods) {
            return;
        }
        let Some(mods) = self.local_mods() else { return };
        let bytes = Msg::Mods { mods }.encode();
        let addrs: Vec<SocketAddr> = s.members.values_mut().filter(|m| !m.sent_mods).map(|m| {
            m.sent_mods = true;
            m.addr
        }).collect();
        for addr in addrs {
            if let Some(c) = s.conns.get_mut(&addr) {
                c.link.queue(&bytes);
            }
            self.flush(s, addr);
        }
    }

    /// Why the game can't launch with the sim mods `uids`: each member
    /// that lacks one, has a different copy (another version, or changed
    /// files: they'd desync), or hasn't said which mods it has yet. Empty
    /// when everyone has the same. FAF's lobby compares uids only. Ours
    /// must be hashed and include every one of `uids`, or there's nothing
    /// to compare theirs with.
    pub fn mod_problems(&self, uids: &[String]) -> Vec<String> {
        let Some(ours) = self.local_mods() else { return vec!["Your mods are still being checked.".to_owned()] };
        let ours: BTreeMap<String, u64> = ours.into_iter().collect();
        let installed = crate::engine::mods::installed_now();
        let name = |uid: &str| installed.and_then(|i| i.get(uid)).map_or_else(|| uid.to_owned(), |m| m.name.clone());
        let mut uids: Vec<&String> = uids.iter().collect();
        uids.sort();
        uids.dedup();
        let mut problems: Vec<String> = uids.iter().filter(|uid| !ours.contains_key(uid.as_str())).map(|uid| format!("You don't have the mod {}.", name(uid))).collect();
        let Ok(s) = self.state.lock() else { return Vec::new() };
        for m in s.members.values() {
            let Some(theirs) = &m.mods else {
                problems.push(format!("{} hasn't said which mods they have yet.", m.name));
                continue;
            };
            for uid in &uids {
                let mine = ours.get(uid.as_str());
                match (mine, theirs.iter().find(|(u, _)| u == *uid).map(|(_, h)| h)) {
                    (_, None) => problems.push(format!("{} doesn't have the mod {}.", m.name, name(uid))),
                    (Some(a), Some(b)) if a != b => problems.push(format!("{} has a different version of the mod {}.", m.name, name(uid))),
                    _ => {}
                }
            }
        }
        problems
    }

    /// The game's clients (empty before a launch).
    pub fn session_clients(&self) -> Vec<SessionClient> {
        self.session.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn set_session_clients(&self, clients: Vec<SessionClient>) {
        if let Ok(mut s) = self.session.lock() {
            *s = clients;
        }
        if let Ok(mut t) = self.session_started.lock() {
            *t = Some(Instant::now());
        }
    }

    /// A turn arrived from `peer`.
    pub fn note_turn(&self, peer: u32) {
        if let Ok(mut s) = self.session.lock()
            && let Some(c) = s.iter_mut().find(|c| c.peer == peer) {
            c.last_turn = Some(Instant::now());
        }
    }

    /// Who has voted to eject `peer` (peer ids).
    pub fn set_ejected_by(&self, peer: u32, voters: Vec<u32>) {
        if let Ok(mut s) = self.session.lock()
            && let Some(c) = s.iter_mut().find(|c| c.peer == peer) {
            c.ejected_by = voters;
        }
    }

    pub fn set_session_paused(&self, paused: bool) {
        self.session_paused.store(paused, Ordering::Relaxed);
    }

    /// The game was paused or resumed by `peer` (us included): the
    /// interface is told.
    pub fn push_pause_event(&self, paused: bool, peer: u32) {
        if let Ok(mut q) = self.pause_events.lock() {
            q.push_back((paused, peer));
        }
    }

    pub fn take_pause_events(&self) -> Vec<(bool, u32)> {
        self.pause_events.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
    }

    /// The checksums after `tick` differ: the interface is told.
    pub fn push_desync(&self, tick: u64, lines: Vec<String>) {
        self.desync_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut q) = self.desyncs.lock() {
            q.push_back((tick, lines));
        }
    }

    /// Desyncs this game so far.
    pub fn desync_count(&self) -> u64 {
        self.desync_count.load(Ordering::Relaxed)
    }

    pub fn take_desyncs(&self) -> Vec<(u64, Vec<String>)> {
        self.desyncs.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
    }

    /// How long a client has been quiet (ms), as FAF's disconnect dialog
    /// reads it: time since its last turn. Zero for us, for a client no
    /// longer connected, while paused, and for a peer still loading (no
    /// turn yet, for its first minute).
    pub fn quiet_ms(&self, c: &SessionClient) -> f64 {
        // A client that's gone isn't quiet, it's "Not Connected": the dialog
        // then stays up only until everyone has ejected it.
        if c.local || !c.connected || self.session_paused.load(Ordering::Relaxed) {
            return 0.0;
        }
        match c.last_turn {
            Some(t) => t.elapsed().as_secs_f64() * 1000.0,
            None => {
                let started = self.session_started.lock().ok().and_then(|t| *t).map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
                if started < 60_000.0 { 0.0 } else { started }
            }
        }
    }

    /// Our vote to eject `target` (`EjectSessionClient`): to everyone, and
    /// to our own lockstep.
    pub fn vote_eject(&self, target: u32) {
        self.broadcast(&Msg::EjectVote { target });
        if let Some(me) = self.local_id() {
            self.push(Event::Message { from: me, msg: Msg::EjectVote { target } });
        }
    }

    /// The game was paused or resumed here: tell everyone.
    pub fn send_pause(&self, paused: bool) {
        self.set_session_paused(paused);
        self.broadcast(&Msg::Pause { paused });
    }

    /// A client of the game is gone (`connected = false` for the interface).
    pub fn session_client_lost(&self, peer: u32) {
        if let Ok(mut s) = self.session.lock() {
            for c in s.iter_mut().filter(|c| c.peer == peer) {
                c.connected = false;
            }
        }
    }

    /// Queues a chat message for the interface.
    pub fn push_chat(&self, from: String, data: crate::engine::sim::lua_data::LuaData) {
        if let Ok(mut q) = self.chat.lock() {
            q.push_back((from, data));
        }
    }

    /// Chat received since the last call.
    pub fn take_chat(&self) -> Vec<(String, crate::engine::sim::lua_data::LuaData)> {
        self.chat.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
    }

    /// Takes everything that has happened since the last call.
    pub fn poll(&self) -> Vec<Event> {
        self.events.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
    }

    /// Puts events back at the front of the queue, in order (what arrived
    /// after a launch belongs to the game, not the lobby).
    pub fn unpoll(&self, events: Vec<Event>) {
        if let Ok(mut q) = self.events.lock() {
            for e in events.into_iter().rev() {
                q.push_front(e);
            }
        }
    }

    /// Takes the queued events, waiting up to `timeout` for one if none are
    /// queued.
    pub fn wait(&self, timeout: Duration) -> Vec<Event> {
        let Ok(mut q) = self.events.lock() else { return Vec::new() };
        if q.is_empty() {
            q = match self.arrived.wait_timeout(q, timeout) {
                Ok((q, _)) => q,
                Err(_) => return Vec::new(),
            };
        }
        q.drain(..).collect()
    }

    /// Tells every peer we're leaving and stops the network thread.
    pub fn close(&self) {
        if let Ok(mut s) = self.state.lock() {
            if s.closed {
                return;
            }
            s.closed = true;
            let bye = Msg::Bye.encode();
            let addrs: Vec<SocketAddr> = s.conns.keys().copied().collect();
            for addr in addrs {
                if let Some(c) = s.conns.get_mut(&addr) {
                    c.link.queue(&bye);
                }
                self.flush(&mut s, addr);
            }
        }
        self.stop.store(true, Ordering::Relaxed);
        tracing::info!("network: closed");
    }

    fn push(&self, e: Event) {
        if let Ok(mut q) = self.events.lock() {
            q.push_back(e);
        }
        self.arrived.notify_all();
    }

    fn hello(&self, peer_id: Option<u32>) -> Msg {
        Msg::Hello { protocol: PROTOCOL, build: build_id(), name: self.name.clone(), uid: self.uid.clone(), listen_port: self.port, peer_id, iroh: self.join_code().unwrap_or_default() }
    }

    /// Sends whatever the link to `addr` has ready.
    fn flush(&self, s: &mut State, addr: SocketAddr) {
        let Some(c) = s.conns.get_mut(&addr) else { return };
        for p in c.link.transmit(self.nonce, Instant::now()) {
            // A failing send is otherwise silent: macOS without Local Network
            // access says "No route to host" here and the far end hears nothing.
            match self.transport.send_to(&p, addr) {
                Ok(()) => c.send_failing = false,
                Err(e) => {
                    if !c.send_failing {
                        tracing::warn!(%addr, error = %e, "network: send failed");
                    }
                    c.send_failing = true;
                }
            }
        }
    }

    fn drop_conn(&self, s: &mut State, addr: SocketAddr, reason: &str) {
        let Some(c) = s.conns.remove(&addr) else { return };
        s.closed_addrs.insert(addr, (c.their_nonce, Instant::now()));
        match c.role {
            Role::Member(id) => {
                if let Some(m) = s.members.remove(&id) {
                    tracing::warn!(id, name = m.name, reason, "network: peer gone");
                    self.push(Event::PeerDisconnected { id, name: m.name, reason: reason.to_owned() });
                    self.announce(s);
                }
            }
            Role::Joining | Role::Welcomed { .. } => self.push(Event::JoinFailed { reason: reason.to_owned() }),
            Role::Mesh { id, name } => self.push(Event::PeerDisconnected { id, name, reason: format!("unreachable: {reason}") }),
            Role::Incoming => {}
        }
    }

    /// The network thread.
    fn run(self: Arc<Self>) {
        let mut buf = vec![0u8; 2048];
        while !self.stop.load(Ordering::Relaxed) {
            // Everything waiting, then housekeeping.
            for _ in 0..512 {
                match self.transport.recv_from(&mut buf) {
                    Ok((n, from)) => self.on_packet(from, &buf[..n]),
                    // Windows reports an ICMP "port unreachable" from an
                    // earlier send as an error here; it's not ours to act on.
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionRefused) => continue,
                    Err(_) => break,
                }
            }
            self.service();
        }
        // Give the farewells a moment to leave, then end iroh's connections.
        std::thread::sleep(Duration::from_millis(50));
        self.transport.close();
    }

    fn on_packet(&self, from: SocketAddr, bytes: &[u8]) {
        let Some(p) = Packet::decode(bytes) else { return };
        let Ok(mut s) = self.state.lock() else { return };
        if s.closed {
            return;
        }
        if let Some(&(nonce, _)) = s.closed_addrs.get(&from) {
            if nonce == p.nonce || nonce == 0 {
                return;
            }
            s.closed_addrs.remove(&from);
        }
        // A restarted client at a known address: the old connection is over.
        if s.conns.get(&from).is_some_and(|c| c.their_nonce != 0 && c.their_nonce != p.nonce) {
            self.drop_conn(&mut s, from, "restarted");
            s.closed_addrs.remove(&from);
        }
        if !s.conns.contains_key(&from) {
            if s.conns.len() >= 4 * super::MAX_PEERS {
                return;
            }
            s.conns.insert(from, Conn { link: Link::new(Instant::now()), their_nonce: p.nonce, role: Role::Incoming, opened: Instant::now(), send_failing: false });
        }
        let frames = {
            let c = s.conns.get_mut(&from).expect("inserted");
            c.their_nonce = p.nonce;
            c.link.receive(&p, Instant::now());
            c.link.frames(MAX_FRAME)
        };
        match frames {
            Ok(frames) => {
                for f in frames {
                    match Msg::decode(&f) {
                        Ok(msg) => self.on_frame(&mut s, from, msg),
                        Err(e) => tracing::warn!(%from, e, "network: bad message"),
                    }
                    if !s.conns.contains_key(&from) {
                        break;
                    }
                }
            }
            Err(e) => self.drop_conn(&mut s, from, &e),
        }
        // Ack promptly (and send anything the handling queued).
        self.flush(&mut s, from);
    }

    fn on_frame(&self, s: &mut State, from: SocketAddr, msg: Msg) {
        let Some(c) = s.conns.get(&from) else { return };
        let kind = match &c.role {
            Role::Incoming => Kind::Incoming,
            Role::Joining => Kind::Joining,
            Role::Welcomed { .. } => Kind::Welcomed,
            Role::Mesh { id, .. } => Kind::Mesh(*id),
            Role::Member(id) => Kind::Member(*id),
        };
        match (kind, msg) {
            (Kind::Member(_), Msg::Bye) => self.drop_conn(s, from, "Left"),
            (Kind::Member(id), Msg::Established { peers }) => {
                if let Some(m) = s.members.get_mut(&id) {
                    m.established = peers;
                }
            }
            (Kind::Member(id), Msg::Pings { pings }) => {
                if let Some(m) = s.members.get_mut(&id) {
                    m.pings = pings;
                }
            }
            (Kind::Member(id), Msg::Mods { mods }) => {
                if let Some(m) = s.members.get_mut(&id) {
                    m.mods = Some(mods);
                }
            }
            (Kind::Member(id), msg) => self.push(Event::Message { from: id, msg }),
            (Kind::Incoming, Msg::Hello { protocol, build, name, uid, listen_port, peer_id, iroh }) => {
                // An iroh peer is reached where it called from.
                let listen = if transport::is_iroh(from) { from } else { SocketAddr::new(from.ip(), listen_port) };
                if protocol != PROTOCOL {
                    return self.refuse(s, from, "IncompatibleVersion");
                }
                if build != build_id() {
                    // FAF's lobby shows this text in its "Connection failed" dialog.
                    let ours = build_id();
                    tracing::warn!(name, theirs = build, ours, "network: peer runs another build");
                    return self.refuse(s, from, &mismatch(&ours, &build));
                }
                let id = match peer_id {
                    // A member completing the mesh.
                    Some(id) => {
                        let hello = self.hello(s.local_id).encode();
                        s.conns.get_mut(&from).expect("conn").link.queue(&hello);
                        id
                    }
                    // A join request: only the host takes them, before the launch.
                    None => {
                        let refusal = if !s.hosting {
                            Some("NotHosting")
                        } else if s.launched {
                            Some("GameLaunched")
                        } else if s.members.len() >= super::MAX_PEERS {
                            Some("LobbyFull")
                        } else {
                            None
                        };
                        if let Some(reason) = refusal {
                            return self.refuse(s, from, reason);
                        }
                        let id = s.next_id;
                        s.next_id += 1;
                        let others: Vec<PeerAddr> = s
                            .members
                            .iter()
                            .map(|(&pid, m)| PeerAddr {
                                id: pid,
                                name: m.name.clone(),
                                uid: m.uid.clone(),
                                // A joiner that came by join code reaches
                                // the others by theirs (their LAN addresses
                                // mean nothing to it); members on this
                                // machine at the address the joiner used for
                                // the host.
                                addr: if transport::is_iroh(from) && !m.iroh.is_empty() {
                                    format!("{}{}", transport::IROH_PREFIX, m.iroh)
                                } else if m.listen.ip().is_loopback() {
                                    format!(":{}", m.listen.port())
                                } else {
                                    self.transport.describe(m.listen)
                                },
                            })
                            .collect();
                        let host_id = s.host_id.unwrap_or(1);
                        let mut reply = Msg::Welcome { your_id: id, host_id, peers: others }.encode();
                        reply.extend(self.hello(Some(host_id)).encode());
                        s.conns.get_mut(&from).expect("conn").link.queue(&reply);
                        id
                    }
                };
                self.admit(s, from, id, name, uid, listen, iroh);
            }
            (Kind::Joining, Msg::Welcome { your_id, host_id, peers }) => {
                if let Some(c) = s.conns.get_mut(&from) {
                    c.role = Role::Welcomed { your_id, host_id, others: peers };
                }
            }
            (Kind::Joining, Msg::Reject { reason }) => self.drop_conn(s, from, &reason),
            (Kind::Welcomed, Msg::Hello { name, uid, iroh, .. }) => {
                let c = s.conns.get_mut(&from).expect("conn");
                let Role::Welcomed { your_id, host_id, others } = std::mem::replace(&mut c.role, Role::Incoming) else { unreachable!() };
                s.local_id = Some(your_id);
                s.host_id = Some(host_id);
                self.admit(s, from, host_id, name, uid, from, iroh);
                self.push(Event::Welcomed { local_id: your_id, host_id });
                // Complete the mesh.
                let hello = self.hello(Some(your_id)).encode();
                for p in others {
                    let target = match p.addr.strip_prefix(':') {
                        Some(port) => port.parse().map(|port| SocketAddr::new(from.ip(), port)).ok(),
                        None => self.transport.resolve(&p.addr).ok(),
                    };
                    let Some(target) = target else {
                        tracing::warn!(addr = p.addr, "network: bad member address");
                        continue;
                    };
                    let mut conn = Conn { link: Link::new(Instant::now()), their_nonce: 0, role: Role::Mesh { id: p.id, name: p.name.clone() }, opened: Instant::now(), send_failing: false };
                    conn.link.queue(&hello);
                    s.conns.insert(target, conn);
                    self.flush(s, target);
                }
            }
            (Kind::Mesh(id), Msg::Hello { name, uid, iroh, .. }) => self.admit(s, from, id, name, uid, from, iroh),
            (_, Msg::Bye) => self.drop_conn(s, from, "Left"),
            (_, other) => tracing::warn!(%from, ?other, "network: unexpected message during handshake"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn admit(&self, s: &mut State, addr: SocketAddr, id: u32, name: String, uid: String, listen: SocketAddr, iroh: String) {
        if let Some(old) = s.members.get(&id).map(|m| m.addr)
            && old != addr {
            tracing::warn!(id, "network: member reconnected from a new address");
            s.conns.remove(&old);
        }
        if let Some(c) = s.conns.get_mut(&addr) {
            c.role = Role::Member(id);
        }
        s.members.insert(id, Member { addr, name: name.clone(), uid: uid.clone(), listen, iroh, established: Vec::new(), pings: Vec::new(), mods: None, sent_mods: false });
        // Our mods, for the host's check before a launch (now, or once
        // they're hashed: `service`).
        self.send_mods(s);
        match self.transport.path(addr) {
            Some(path @ (PathKind::IrohDirect | PathKind::IrohRelay)) => tracing::info!(id, name, ?path, "network: peer connected over iroh"),
            _ => tracing::info!(id, name, %listen, "network: peer connected"),
        }
        self.push(Event::PeerConnected { id, name, uid });
        self.announce(s);
    }

    /// Tells every member who we're connected to.
    fn announce(&self, s: &mut State) {
        let bytes = Msg::Established { peers: s.members.keys().copied().collect() }.encode();
        let addrs: Vec<SocketAddr> = s.members.values().map(|m| m.addr).collect();
        for addr in addrs {
            if let Some(c) = s.conns.get_mut(&addr) {
                c.link.queue(&bytes);
            }
            self.flush(s, addr);
        }
    }

    fn refuse(&self, s: &mut State, from: SocketAddr, reason: &str) {
        tracing::warn!(%from, reason, "network: refused a connection");
        if let Some(c) = s.conns.get_mut(&from) {
            c.link.queue(&Msg::Reject { reason: reason.to_owned() }.encode());
        }
        self.flush(s, from);
        if let Some(c) = s.conns.remove(&from) {
            s.closed_addrs.insert(from, (c.their_nonce, Instant::now()));
        }
    }

    /// Resends, keepalives, timeouts.
    fn service(&self) {
        let Ok(mut s) = self.state.lock() else { return };
        let now = Instant::now();
        let addrs: Vec<SocketAddr> = s.conns.keys().copied().collect();
        for addr in addrs {
            let c = &s.conns[&addr];
            let silent = now.duration_since(c.link.last_heard);
            let handshaking = !matches!(c.role, Role::Member(_));
            let handshake_timeout = if transport::is_iroh(addr) { IROH_HANDSHAKE_TIMEOUT } else { HANDSHAKE_TIMEOUT };
            if silent > LOST_AFTER || (handshaking && now.duration_since(c.opened) > handshake_timeout) {
                let reason = if handshaking { "TimedOut" } else { "TimedOutToHost" };
                self.drop_conn(&mut s, addr, reason);
                continue;
            }
            self.flush(&mut s, addr);
        }
        s.closed_addrs.retain(|_, (_, t)| now.duration_since(*t) < LOST_AFTER);
        self.send_mods(&mut s);
        // Our pings, for everyone's ping grid.
        if s.local_id.is_some() && !s.members.is_empty() && s.pings_sent.is_none_or(|t| now.duration_since(t) >= PINGS_EVERY) {
            s.pings_sent = Some(now);
            let bytes = Msg::Pings { pings: self.own_pings(&s) }.encode();
            let addrs: Vec<SocketAddr> = s.members.values().map(|m| m.addr).collect();
            for addr in addrs {
                if let Some(c) = s.conns.get_mut(&addr) {
                    c.link.queue(&bytes);
                }
                self.flush(&mut s, addr);
            }
        }
    }
}

/// Binds the UDP socket: `port`, or for 0 the first free port from
/// [`DEFAULT_PORT`] up.
fn bind(port: u16) -> Result<UdpSocket, String> {
    let any = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    if port != 0 {
        return UdpSocket::bind(SocketAddr::new(any, port)).map_err(|e| format!("port {port}: {e}"));
    }
    for p in DEFAULT_PORT..DEFAULT_PORT + PORT_TRIES {
        if let Ok(s) = UdpSocket::bind(SocketAddr::new(any, p)) {
            return Ok(s);
        }
    }
    UdpSocket::bind(SocketAddr::new(any, 0)).map_err(|e| e.to_string())
}

/// Datagram size check at compile time (the link's packets fit an MTU).
const _: () = assert!(MTU <= 1400);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faf_version_from_version_lua() {
        let checkout = "local GameType = 'unknown'  -- x\nlocal Commit = 'unknown'\nlocal Version = \"3839\"\n";
        assert_eq!(faf_version(checkout), "FAF 3839");
        let client = "local GameType = \"FAF\"  -- x\nlocal Commit = \"36701ed9\"\nlocal Version = \"3839\"\n";
        assert_eq!(faf_version(client), "FAF 3839 (FAF client)");
        assert_eq!(faf_version(""), "FAF ?");
    }

    #[test]
    fn mismatch_says_what_differs() {
        let a = "0.1.0+4d0b72e5, FAF 3839, Lua 3943eb84";
        assert!(mismatch(a, "0.1.0+aaaaaaaa, FAF 3839, Lua 3943eb84").contains("engine builds differ"));
        assert!(mismatch(a, "0.1.0+4d0b72e5, FAF 3840, Lua dbfe2813").contains("FAF versions differ"));
        assert!(mismatch(a, "0.1.0+4d0b72e5, FAF 3839, Lua dbfe2813").contains("different FAF files"));
    }

    fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn messages(net: &NetCore, into: &mut Vec<(u32, Msg)>) {
        for e in net.poll() {
            if let Event::Message { from, msg } = e {
                into.push((from, msg));
            }
        }
    }

    /// A host and two joiners over loopback: everyone ends up connected to
    /// everyone (the second joiner reaches the first through the address the
    /// host gave it), knows who everyone else is connected to, gets lobby
    /// data, and hears when someone leaves.
    #[test]
    fn lobby_mesh_forms_and_carries_data() {
        let host = NetCore::open(0, "host", "u1").unwrap();
        host.host();
        let addr = format!("127.0.0.1:{}", host.port());
        let a = NetCore::open(0, "a", "u2").unwrap();
        a.join(&addr);
        wait_until("a to join", || a.local_id().is_some() && host.peers().len() == 1);
        let b = NetCore::open(0, "b", "u3").unwrap();
        b.join(&addr);
        let all = [&host, &a, &b];
        wait_until("the mesh", || all.iter().all(|n| n.peers().len() == 2));
        // Ids: host 1, joiners in order.
        assert_eq!((host.local_id(), a.local_id(), b.local_id()), (Some(1), Some(2), Some(3)));
        // Everyone reports a full mesh (FAF's lobby checks this to launch).
        wait_until("establishedPeers", || all.iter().all(|n| n.peers().iter().all(|p| p.established.len() == 2)));

        // Lobby data from b reaches both others, in order.
        let data = |i: f64| Msg::Data(crate::engine::sim::lua_data::LuaData::number(i));
        for i in 0..50 {
            b.broadcast(&data(i as f64));
        }
        let (mut got_host, mut got_a) = (Vec::new(), Vec::new());
        wait_until("data", || {
            messages(&host, &mut got_host);
            messages(&a, &mut got_a);
            got_host.iter().filter(|(_, m)| matches!(m, Msg::Data(_))).count() == 50 && got_a.iter().filter(|(_, m)| matches!(m, Msg::Data(_))).count() == 50
        });
        let from_b: Vec<&Msg> = got_a.iter().filter(|(f, m)| *f == 3 && matches!(m, Msg::Data(_))).map(|(_, m)| m).collect();
        assert_eq!(from_b, (0..50).map(|i| data(i as f64)).collect::<Vec<_>>().iter().collect::<Vec<_>>(), "in order");

        // b leaves: the others hear it at once (not after a timeout).
        b.close();
        let mut gone = (false, false);
        wait_until("b to leave", || {
            for (n, flag) in [(&host, &mut gone.0), (&a, &mut gone.1)] {
                for e in n.poll() {
                    if matches!(e, Event::PeerDisconnected { id: 3, .. }) {
                        *flag = true;
                    }
                }
            }
            gone == (true, true)
        });
        host.close();
        a.close();
    }
    /// Members tell each other their installed sim mods on connecting; the
    /// host's launch check names whoever lacks a game mod or has another
    /// copy of it, and passes once everyone has the same.
    #[test]
    fn launch_check_compares_everyones_mods() {
        let host = NetCore::open(0, "host", "u1").unwrap();
        host.set_local_mods(vec![("rich".into(), 1), ("chess".into(), 2)]);
        host.host();
        let addr = format!("127.0.0.1:{}", host.port());
        let a = NetCore::open(0, "Ann", "u2").unwrap();
        a.set_local_mods(vec![("rich".into(), 1)]);
        a.join(&addr);
        let b = NetCore::open(0, "Bob", "u3").unwrap();
        b.set_local_mods(vec![("rich".into(), 9), ("chess".into(), 2)]);
        wait_until("a to join", || host.peers().len() == 1);
        b.join(&addr);
        wait_until("everyone's mods", || host.mod_problems(&[]).is_empty() && host.peers().len() == 2);
        assert_eq!(host.mod_problems(&["rich".into(), "chess".into()]), ["Ann doesn't have the mod chess.", "Bob has a different version of the mod rich."]);
        assert_eq!(host.mod_problems(&["chess".into()]), ["Ann doesn't have the mod chess."]);
        assert!(host.mod_problems(&[]).is_empty(), "no mods: nothing to compare");
        for n in [&host, &a, &b] {
            n.close();
        }
    }

    /// Mods still being hashed when players connect: nobody waits on them
    /// (the connection carries on), they're sent once hashed, and the host
    /// can't launch until both sides' are in.
    #[test]
    fn launch_check_waits_for_mods_still_hashing() {
        let host = NetCore::open(0, "host", "u1").unwrap();
        host.set_local_mods_pending();
        host.host();
        let a = NetCore::open(0, "Ann", "u2").unwrap();
        a.set_local_mods_pending();
        a.join(&format!("127.0.0.1:{}", host.port()));
        wait_until("a to join", || host.peers().len() == 1 && a.peers().len() == 1);
        assert_eq!(host.mod_problems(&[]), ["Your mods are still being checked."]);
        host.set_local_mods(vec![("rich".into(), 1)]);
        assert_eq!(host.mod_problems(&["rich".into()]), ["Ann hasn't said which mods they have yet."]);
        assert_eq!(host.mod_problems(&["chess".into()]), ["You don't have the mod chess.", "Ann hasn't said which mods they have yet."]);
        a.set_local_mods(vec![("rich".into(), 1)]);
        wait_until("Ann's mods", || host.mod_problems(&["rich".into()]).is_empty());
        wait_until("the host's mods", || a.mod_problems(&["rich".into()]).is_empty());
        for n in [&host, &a] {
            n.close();
        }
    }

    /// The same over iroh: two joiners by the host's join code (the second
    /// reaches the first by the code the host passes on), and lobby data
    /// bigger than a packet arrives whole. Needs the internet (n0's relays
    /// and DNS address lookup).
    #[test]
    #[ignore = "needs the internet (n0 relays and DNS)"]
    fn lobby_mesh_forms_over_iroh_join_codes() {
        let host = NetCore::open_with(0, "host", "u1", true).unwrap();
        host.host();
        let code = host.join_code().expect("iroh up");
        let a = NetCore::open_with(0, "a", "u2", true).unwrap();
        a.join(&code);
        wait_until_long("a to join", || a.local_id().is_some() && host.peers().len() == 1);
        let b = NetCore::open_with(0, "b", "u3", true).unwrap();
        b.join(&format!("iroh:{code}"));
        let all = [&host, &a, &b];
        wait_until_long("the mesh", || all.iter().all(|n| n.peers().len() == 2));
        // b reached a over iroh, not a LAN address.
        assert!(b.peers().iter().all(|p| transport::is_iroh(p.addr)), "{:?}", b.peers().iter().map(|p| p.addr).collect::<Vec<_>>());
        let big = crate::engine::sim::lua_data::LuaData::string(&"x".repeat(20_000));
        a.broadcast(&Msg::Data(big.clone()));
        let mut got = Vec::new();
        wait_until_long("a big message", || {
            messages(&b, &mut got);
            got.iter().any(|(f, m)| *f == 2 && *m == Msg::Data(big.clone()))
        });
        for n in all {
            n.close();
        }
    }

    fn wait_until_long(what: &str, mut f: impl FnMut() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
