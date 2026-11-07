//! Stall watchdog: reports a sim that stops making progress instead of
//! leaving it spinning silently.
//!
//! The sim thread records the tick and the stage it is in (`enter`, plain
//! atomic stores) and bumps a beat at the end of every tick (`beat`). A
//! watchdog thread checks the beat once a second; when it hasn't moved for
//! `stall` it logs an `ERROR` naming the tick and stage, and asks the Lua
//! interrupt to break the running script. If Lua is what's spinning, the
//! interrupt logs the Lua traceback and raises an error in that script (the
//! sim carries on, like any other script error). If there is still no
//! progress after `exit_after` more (a Rust loop, or a script that keeps
//! re-entering), the process exits with code 3, so headless runs end
//! instead of hanging.
//!
//! The interrupt is a raw Luau callback that only reads an atomic (mlua's
//! `set_interrupt` trampoline made call-heavy Lua 2.4x slower; see the
//! `lua_interrupt` bench). Wall-clock time here never reaches sim state.

use std::ffi::{CStr, c_int};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mlua::{Lua, ffi};

/// Set by the watchdog thread when the sim stalls: the next Lua interrupt
/// logs a traceback and errors out of the running script. Process-wide,
/// since Luau's interrupt is a plain C function with no context; only
/// engines that spawned a watchdog ever set it.
static BREAK_LUA: AtomicBool = AtomicBool::new(false);

/// Tick stages, for the stall report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Idle,
    Economy,
    Threat,
    Orders,
    Movement,
    Construction,
    Reclaim,
    Weapons,
    Projectiles,
    LuaThreads,
    Sync,
    Compact,
    Snapshot,
    Intel,
    Input,
    /// The sim VM's full Lua collection (`sim::gc`), every `gc::EVERY`
    /// ticks at the end of the tick.
    LuaGc,
}

impl Phase {
    const ALL: [Phase; 16] = [
        Phase::Idle,
        Phase::Economy,
        Phase::Threat,
        Phase::Orders,
        Phase::Movement,
        Phase::Construction,
        Phase::Reclaim,
        Phase::Weapons,
        Phase::Projectiles,
        Phase::LuaThreads,
        Phase::Sync,
        Phase::Compact,
        Phase::Snapshot,
        Phase::Intel,
        Phase::Input,
        Phase::LuaGc,
    ];

    pub const COUNT: usize = Phase::ALL.len();

    fn from_u8(v: u8) -> Phase {
        Phase::ALL.get(v as usize).copied().unwrap_or(Phase::Idle)
    }
}

/// Wall time per tick stage (every engine run, `Engine::profile_stages`),
/// for finding where tick time goes. Present as app data only when enabled. Never sim state.
#[derive(Default)]
pub struct StageTimes {
    totals: [Duration; Phase::COUNT],
    current: Option<(Phase, Instant)>,
    /// The running tick's stages, and the slowest tick since the last
    /// report (tick, stages): averages hide one-tick spikes.
    this_tick: [Duration; Phase::COUNT],
    worst: Option<(u64, [Duration; Phase::COUNT])>,
    /// Path searches and their work (`nav::Search::work`) since the last
    /// report.
    pub path_searches: u64,
    pub path_work: u64,
}

impl StageTimes {
    /// Ends the running stage and starts `phase` of `tick` (`Idle` =
    /// between ticks).
    pub fn enter(&mut self, tick: u64, phase: Phase) {
        let now = Instant::now();
        if let Some((p, since)) = self.current.replace((phase, now))
            && p != Phase::Idle {
            self.totals[p as usize] += now - since;
            self.this_tick[p as usize] += now - since;
        }
        if phase == Phase::Idle {
            let total: Duration = self.this_tick.iter().sum();
            if self.worst.is_none_or(|(_, w)| total > w.iter().sum::<Duration>()) {
                self.worst = Some((tick, self.this_tick));
            }
            self.this_tick = Default::default();
        }
    }

    /// "Stage=ms/tick" for every stage that took time over `ticks`, slowest
    /// first, then the slowest tick and path searches; then resets.
    pub fn report(&mut self, ticks: u64) -> String {
        let rows = |t: &[Duration; Phase::COUNT], div: f64| {
            let mut rows: Vec<(Phase, Duration)> = Phase::ALL.iter().map(|&p| (p, t[p as usize])).filter(|r| !r.1.is_zero()).collect();
            rows.sort_by_key(|r| std::cmp::Reverse(r.1));
            rows.iter().map(|(p, d)| format!("{p:?}={:.3}", d.as_secs_f64() * 1000.0 / div)).collect::<Vec<_>>().join(" ")
        };
        let mut out = rows(&self.totals, ticks.max(1) as f64);
        if let Some((tick, w)) = self.worst.take() {
            let total: Duration = w.iter().sum();
            out += &format!(" | worst tick {tick}: {:.1} ms ({})", total.as_secs_f64() * 1000.0, rows(&w, 1.0));
        }
        if self.path_searches > 0 {
            out += &format!(" | paths: {} searches, {:.1}M work", self.path_searches, self.path_work as f64 / 1e6);
        }
        self.totals = Default::default();
        self.path_searches = 0;
        self.path_work = 0;
        out
    }
}

/// Progress markers shared between the sim thread and the watchdog thread.
#[derive(Default)]
pub struct Watchdog {
    tick: AtomicU64,
    phase: AtomicU8,
    beat: AtomicU64,
    /// Ticking has started; session setup before it (blueprints, FAF's nav
    /// generator: seconds in debug builds) isn't watched.
    armed: AtomicBool,
}

impl Watchdog {
    /// The sim is entering `phase` of `tick`.
    pub fn enter(&self, tick: u64, phase: Phase) {
        self.tick.store(tick, Ordering::Relaxed);
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    /// Starts watching (call when ticking begins).
    pub fn arm(&self) {
        self.beat();
        self.armed.store(true, Ordering::Relaxed);
    }

    /// Progress: a tick finished (or the sim is deliberately idle, e.g.
    /// paused).
    pub fn beat(&self) {
        self.beat.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// No progress for this long counts as a stall.
    pub stall: Duration,
    /// Exit with code 3 if still stalled this much later; `None` = never.
    pub exit_after: Option<Duration>,
}

/// Starts the watchdog thread; it ends when the engine drops its `Watchdog`.
pub fn spawn(wd: Arc<Watchdog>, config: Config) -> std::io::Result<()> {
    std::thread::Builder::new().name("sim-watchdog".into()).spawn(move || {
        let mut last = wd.beat.load(Ordering::Relaxed);
        let mut since = Instant::now();
        let mut reported = false;
        while Arc::strong_count(&wd) > 1 {
            std::thread::sleep(Duration::from_millis(500));
            let beat = wd.beat.load(Ordering::Relaxed);
            // A sim stopped in the Lua debugger isn't stalled.
            if beat != last || !wd.armed.load(Ordering::Relaxed) || crate::engine::lua::debugger::is_paused() {
                if reported {
                    tracing::warn!(tick = wd.tick.load(Ordering::Relaxed), "sim progressing again after a stall");
                }
                last = beat;
                since = Instant::now();
                reported = false;
                BREAK_LUA.store(false, Ordering::Relaxed);
                continue;
            }
            let stalled = since.elapsed();
            if !reported && stalled >= config.stall {
                reported = true;
                let (tick, phase) = (wd.tick.load(Ordering::Relaxed), Phase::from_u8(wd.phase.load(Ordering::Relaxed)));
                tracing::error!(tick, ?phase, secs = stalled.as_secs(), "sim stalled: no tick completed (breaking into Lua if it is running)");
                BREAK_LUA.store(true, Ordering::Relaxed);
            }
            if let Some(extra) = config.exit_after
                && stalled >= config.stall + extra {
                let (tick, phase) = (wd.tick.load(Ordering::Relaxed), Phase::from_u8(wd.phase.load(Ordering::Relaxed)));
                tracing::error!(tick, ?phase, secs = stalled.as_secs(), "sim still stalled; exiting with code 3");
                // Let the non-blocking log writer flush.
                std::thread::sleep(Duration::from_millis(500));
                std::process::exit(3);
            }
        }
    })?;
    Ok(())
}

/// The Lua interrupt of the sim VM (and of every VM the Lua debugger
/// attaches, which picks up its own work here: `engine::lua::debugger`).
pub(crate) unsafe extern "C-unwind" fn interrupt(state: *mut ffi::lua_State, gc: c_int) {
    // gc >= 0: a GC step, not a safe point to raise errors from.
    if gc >= 0 {
        return;
    }
    if crate::engine::lua::debugger::enabled() && unsafe { crate::engine::lua::debugger::on_interrupt(state) } {
        // Debug Console code past its time limit.
        unsafe {
            ffi::lua_checkstack(state, 2);
            ffi::luaL_error(state, c"console: stopped".as_ptr())
        }
    }
    if !BREAK_LUA.load(Ordering::Relaxed) {
        return;
    }
    BREAK_LUA.store(false, Ordering::Relaxed);
    let trace = unsafe {
        let p = ffi::lua_debugtrace(state);
        if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    tracing::error!(target: "lua", "sim stalled in Lua; interrupting the running script. Traceback:\n{trace}");
    drop(trace);
    // An interrupt safe point may leave no free stack slot for the message.
    unsafe {
        ffi::lua_checkstack(state, 2);
        ffi::luaL_error(state, c"sim stalled: script interrupted by the watchdog".as_ptr())
    }
}

/// Installs the watchdog's interrupt on `lua` (cheap: one atomic load per
/// Lua call / loop back-edge until a stall is reported).
pub fn install_interrupt(lua: &Lua) {
    // SAFETY: only sets the VM's interrupt callback pointer; the callback
    // itself raises errors only at Luau's interrupt safe points.
    let set = |state: *mut ffi::lua_State| unsafe { (*ffi::lua_callbacks(state)).interrupt = Some(interrupt) };
    // A sim VM: without making an object (the watchdog runs in some runs of a
    // game and not others; their keyframes must match).
    if crate::engine::lua::heap_snapshot::is_prepared(lua) {
        let _ = crate::engine::lua::heap_snapshot::with_raw_state(lua, set);
    } else {
        let _ = unsafe { lua.exec_raw::<()>((), set) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_breaks_a_runaway_lua_loop() {
        let lua = Lua::new();
        install_interrupt(&lua);
        // Normal code is untouched.
        assert_eq!(lua.load("local s = 0 for i = 1, 1000 do s = s + i end return s").eval::<i64>().unwrap(), 500500);
        BREAK_LUA.store(true, Ordering::Relaxed);
        let err = lua.load("while true do end").exec().unwrap_err();
        assert!(err.to_string().contains("interrupted by the watchdog"), "{err}");
        assert!(!BREAK_LUA.load(Ordering::Relaxed), "one break per report");
    }

    #[test]
    fn phase_round_trips() {
        for p in Phase::ALL {
            assert_eq!(Phase::from_u8(p as u8), p);
        }
    }
}
