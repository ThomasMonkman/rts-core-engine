//! A bad connection on demand, for testing: latency, jitter, spikes and
//! packet loss on this client's packets, each way, set from the stats bar's
//! network panel (or `RTS_NET_LAG` at start).
//!
//! The settings are process-wide atomics: the transport asks [`fate`] for
//! every packet it sends or receives (UDP and iroh alike) and drops it,
//! passes it on, or holds it until a moment later. Held packets overtake
//! each other, as on a real link. Only the link layer above sees the
//! difference: its resends, RTT and the lockstep's input delay react as
//! they would to a real line.
//!
//! Network threads and the stats bar only: none of this touches sim state,
//! and its random numbers are test noise, never sim input.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// Which way a packet goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Out,
    In,
}

/// One direction's conditions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Conditions {
    /// Every packet is held this long (ms)...
    pub latency_ms: u32,
    /// ...plus up to this much more, at random (ms).
    pub jitter_ms: u32,
    /// Packets lost, per thousand.
    pub loss_permille: u32,
    /// One packet in this many (0: none)...
    pub spike_every: u32,
    /// ...is held this much more (ms).
    pub spike_ms: u32,
}

impl Conditions {
    pub const OFF: Conditions = Conditions { latency_ms: 0, jitter_ms: 0, loss_permille: 0, spike_every: 0, spike_ms: 0 };

    pub fn is_off(&self) -> bool {
        self.latency_ms == 0 && self.jitter_ms == 0 && self.loss_permille == 0 && (self.spike_every == 0 || self.spike_ms == 0)
    }
}

/// Ready-made lines (each way): name and conditions.
pub const PRESETS: [(&str, Conditions); 5] = [
    ("off", Conditions::OFF),
    ("LAN", Conditions { latency_ms: 1, jitter_ms: 1, loss_permille: 0, spike_every: 0, spike_ms: 0 }),
    ("Wi-Fi", Conditions { latency_ms: 5, jitter_ms: 15, loss_permille: 5, spike_every: 200, spike_ms: 80 }),
    ("intercontinental", Conditions { latency_ms: 75, jitter_ms: 10, loss_permille: 2, spike_every: 0, spike_ms: 0 }),
    // ~350 ms round trip on average: (150 + 50/2) ms each way.
    ("awful", Conditions { latency_ms: 150, jitter_ms: 50, loss_permille: 50, spike_every: 50, spike_ms: 400 }),
];

/// What becomes of a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    Pass,
    Drop,
    /// Hold it until then.
    Hold(Instant),
}

struct Side {
    latency_ms: AtomicU32,
    jitter_ms: AtomicU32,
    loss_permille: AtomicU32,
    spike_every: AtomicU32,
    spike_ms: AtomicU32,
    /// Counters since start: packets seen, dropped, held.
    seen: AtomicU64,
    dropped: AtomicU64,
    held: AtomicU64,
}

impl Side {
    const fn new() -> Side {
        Side {
            latency_ms: AtomicU32::new(0),
            jitter_ms: AtomicU32::new(0),
            loss_permille: AtomicU32::new(0),
            spike_every: AtomicU32::new(0),
            spike_ms: AtomicU32::new(0),
            seen: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            held: AtomicU64::new(0),
        }
    }

    fn get(&self) -> Conditions {
        Conditions {
            latency_ms: self.latency_ms.load(Relaxed),
            jitter_ms: self.jitter_ms.load(Relaxed),
            loss_permille: self.loss_permille.load(Relaxed),
            spike_every: self.spike_every.load(Relaxed),
            spike_ms: self.spike_ms.load(Relaxed),
        }
    }

    fn set(&self, c: Conditions) {
        self.latency_ms.store(c.latency_ms, Relaxed);
        self.jitter_ms.store(c.jitter_ms, Relaxed);
        self.loss_permille.store(c.loss_permille.min(1000), Relaxed);
        self.spike_every.store(c.spike_every, Relaxed);
        self.spike_ms.store(c.spike_ms, Relaxed);
    }
}

static OUT: Side = Side::new();
static IN: Side = Side::new();
static RNG: AtomicU64 = AtomicU64::new(0x2545_f491_4f6c_dd1d);
static FROM_ENV: std::sync::Once = std::sync::Once::new();

fn side(dir: Dir) -> &'static Side {
    match dir {
        Dir::Out => &OUT,
        Dir::In => &IN,
    }
}

/// The conditions one way now.
pub fn get(dir: Dir) -> Conditions {
    side(dir).get()
}

/// Sets the conditions one way (logged: a test run's log says what line it had).
pub fn set(dir: Dir, c: Conditions) {
    if side(dir).get() == c {
        return;
    }
    side(dir).set(c);
    tracing::info!(?dir, latency_ms = c.latency_ms, jitter_ms = c.jitter_ms, loss_permille = c.loss_permille, spike_every = c.spike_every, spike_ms = c.spike_ms, "network: conditions");
}

/// Packets one way since start: (seen, dropped, held).
pub fn counts(dir: Dir) -> (u64, u64, u64) {
    let s = side(dir);
    (s.seen.load(Relaxed), s.dropped.load(Relaxed), s.held.load(Relaxed))
}

/// Any conditions either way.
pub fn active() -> bool {
    !get(Dir::Out).is_off() || !get(Dir::In).is_off()
}

/// Dev: `RTS_NET_LAG=<base>:<jitter>[:<n>:<spike>[:<loss per mille>]]`
/// (milliseconds) sets the outgoing conditions at start: a bad connection
/// (wifi, another country) on one machine. Read once.
pub fn init_from_env() {
    FROM_ENV.call_once(|| {
        let Ok(spec) = std::env::var("RTS_NET_LAG") else { return };
        let Some(c) = parse_spec(&spec) else {
            tracing::warn!(spec, "network: RTS_NET_LAG not understood (base:jitter[:n:spike[:loss per mille]])");
            return;
        };
        tracing::warn!(?c, "network: RTS_NET_LAG holds every packet we send");
        set(Dir::Out, c);
    });
}

fn parse_spec(spec: &str) -> Option<Conditions> {
    let n: Vec<u32> = spec.split(':').map(|v| v.trim().parse().ok()).collect::<Option<_>>()?;
    let at = |i: usize| n.get(i).copied().unwrap_or(0);
    Some(Conditions { latency_ms: *n.first()?, jitter_ms: at(1), spike_every: at(2), spike_ms: at(3), loss_permille: at(4) })
}

/// What becomes of the next packet `dir`.
pub fn fate(dir: Dir) -> Fate {
    let s = side(dir);
    let c = s.get();
    if c.is_off() {
        return Fate::Pass;
    }
    s.seen.fetch_add(1, Relaxed);
    let x = next_random();
    match decide(&c, x) {
        None => {
            s.dropped.fetch_add(1, Relaxed);
            Fate::Drop
        }
        Some(0) => Fate::Pass,
        Some(ms) => {
            s.held.fetch_add(1, Relaxed);
            Fate::Hold(Instant::now() + Duration::from_millis(ms))
        }
    }
}

/// `c` applied with random bits `x`: `None` lost, else the hold (ms).
fn decide(c: &Conditions, x: u64) -> Option<u64> {
    // Separate bits for each draw.
    let (loss, jitter, spike) = (x & 0xffff, (x >> 16) & 0xffff, x >> 32);
    if c.loss_permille > 0 && loss % 1000 < c.loss_permille as u64 {
        return None;
    }
    let mut ms = c.latency_ms as u64 + if c.jitter_ms > 0 { jitter % (c.jitter_ms as u64 + 1) } else { 0 };
    if c.spike_every > 0 && spike.is_multiple_of(c.spike_every as u64) {
        ms += c.spike_ms as u64;
    }
    Some(ms)
}

/// xorshift64: test noise, never sim state (a lost race between threads
/// only repeats a number).
fn next_random() -> u64 {
    let mut x = RNG.load(Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    RNG.store(x, Relaxed);
    x
}

/// Packets held for later, in due order (ties: arrival order).
pub struct HoldQueue<T> {
    heap: std::collections::BinaryHeap<std::cmp::Reverse<(Instant, u64, Held<T>)>>,
    seq: u64,
}

/// A held packet (ordered by its due time and sequence only).
pub struct Held<T>(pub T);

impl<T> PartialEq for Held<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl<T> Eq for Held<T> {}
impl<T> PartialOrd for Held<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Held<T> {
    fn cmp(&self, _: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}

impl<T> Default for HoldQueue<T> {
    fn default() -> Self {
        HoldQueue { heap: Default::default(), seq: 0 }
    }
}

impl<T> HoldQueue<T> {
    pub fn push(&mut self, due: Instant, item: T) {
        self.seq += 1;
        self.heap.push(std::cmp::Reverse((due, self.seq, Held(item))));
    }

    /// When the next one is due.
    pub fn next_due(&self) -> Option<Instant> {
        self.heap.peek().map(|std::cmp::Reverse(p)| p.0)
    }

    /// The next one due by `now`.
    pub fn pop_due(&mut self, now: Instant) -> Option<T> {
        if self.next_due()? > now {
            return None;
        }
        self.heap.pop().map(|std::cmp::Reverse((_, _, Held(t)))| t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_spec_parses() {
        assert_eq!(parse_spec("80:20"), Some(Conditions { latency_ms: 80, jitter_ms: 20, ..Conditions::OFF }));
        assert_eq!(parse_spec("10:0:50:300:25"), Some(Conditions { latency_ms: 10, jitter_ms: 0, spike_every: 50, spike_ms: 300, loss_permille: 25 }));
        assert_eq!(parse_spec("ten"), None);
    }

    #[test]
    fn decide_drops_holds_and_spikes() {
        let c = Conditions { latency_ms: 50, jitter_ms: 10, loss_permille: 0, spike_every: 0, spike_ms: 0 };
        for x in [0u64, 1, 0xffff_0000, u64::MAX] {
            let ms = decide(&c, x).unwrap();
            assert!((50..=60).contains(&ms), "{ms}");
        }
        let lossy = Conditions { loss_permille: 1000, ..c };
        assert_eq!(decide(&lossy, 12345), None);
        let spiky = Conditions { spike_every: 1, spike_ms: 500, jitter_ms: 0, ..c };
        assert_eq!(decide(&spiky, 7), Some(550));
    }

    #[test]
    fn loss_rate_is_about_right() {
        let c = Conditions { loss_permille: 100, ..Conditions::OFF };
        let mut x = 0x1234_5678_9abc_def0u64;
        let mut lost = 0;
        for _ in 0..10_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            lost += decide(&c, x).is_none() as u32;
        }
        assert!((800..1200).contains(&lost), "{lost}");
    }

    #[test]
    fn hold_queue_releases_in_due_order() {
        let t = Instant::now();
        let mut q = HoldQueue::default();
        q.push(t + Duration::from_millis(20), "b");
        q.push(t + Duration::from_millis(10), "a");
        q.push(t + Duration::from_millis(20), "c");
        assert_eq!(q.pop_due(t), None);
        assert_eq!(q.pop_due(t + Duration::from_millis(30)), Some("a"));
        assert_eq!(q.pop_due(t + Duration::from_millis(30)), Some("b"));
        assert_eq!(q.pop_due(t + Duration::from_millis(30)), Some("c"));
        assert_eq!(q.pop_due(t + Duration::from_millis(30)), None);
    }
}
