//! Tracy profiler hooks (`cargo run --release --features tracy`).
//!
//! With the feature on, `main` adds a `tracing-tracy` layer, so the spans
//! below (sim tick stages, render frame phases) and every log event show up
//! in the Tracy GUI, with a frame mark per rendered frame and a secondary
//! "sim tick" mark per tick, and the process's resident memory plotted
//! ("memory MB") at each mark. Without the feature every hook compiles to
//! nothing: default builds pay no span cost. Profiling never reads into sim
//! state.
//!
//! `--tracy-level N` picks how much goes to Tracy, so one build serves all:
//! 1 (default) the spans above; 2 adds a zone per engine function Lua calls
//! (named as Lua knows it, `Unit:GetPosition`) and per script callback (by
//! method, `OnDamage`); 3 adds a zone per Lua thread resume, named by where
//! the thread's function was defined (`/lua/ai/gridrecon.lua:204`), and the
//! `prof_plot!` counters. Level 2's function zones are chosen when the
//! function is registered, so below level 2 Lua calls the bare function:
//! no cost at all. Callbacks, thread resumes and plots test the level per
//! call. Every dynamic name is interned once to a leaked source location,
//! so a zone costs the same as one with a literal name.

use std::sync::atomic::{AtomicU8, Ordering};

static LEVEL: AtomicU8 = AtomicU8::new(1);

/// Sets the detail level (`--tracy-level`); before any Lua VM is built.
pub fn set_level(level: u8) {
    LEVEL.store(level, Ordering::Relaxed);
}

/// Whether detail level `n` is on (never without the feature).
#[inline(always)]
pub fn on(n: u8) -> bool {
    cfg!(feature = "tracy") && LEVEL.load(Ordering::Relaxed) >= n
}

/// A zone's name and place, made once per name and never freed (Tracy
/// keeps the pointer).
#[cfg(feature = "tracy")]
pub struct Location(tracy_client_sys::___tracy_source_location_data);

// SAFETY: the pointers inside are to leaked, never-written strings.
#[cfg(feature = "tracy")]
unsafe impl Sync for Location {}
#[cfg(feature = "tracy")]
unsafe impl Send for Location {}

/// The location named `name` in group `function` (Tracy's "function"
/// column: `engine fn`, `lua callback`, `lua thread`), interned: the same
/// name always gives the same location. `file:line` names (Lua threads)
/// also set Tracy's source file and line.
#[cfg(feature = "tracy")]
// Lookups only, never iterated.
#[allow(clippy::disallowed_types)]
pub fn location(function: &'static str, name: &str) -> &'static Location {
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::sync::{LazyLock, Mutex};
    static INTERNED: LazyLock<Mutex<HashMap<(&'static str, String), &'static Location>>> = LazyLock::new(Default::default);
    let mut m = INTERNED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(l) = m.get(&(function, name.to_string())) {
        return l;
    }
    let leak = |s: &str| CString::new(s.replace('\0', "?")).unwrap_or_default().into_raw() as *const std::ffi::c_char;
    let (file, line) = match name.rsplit_once(':') {
        Some((f, l)) if l.parse::<u32>().is_ok() => (f, l.parse().unwrap_or(0)),
        _ => ("", 0),
    };
    let loc: &'static Location = Box::leak(Box::new(Location(tracy_client_sys::___tracy_source_location_data {
        name: leak(name),
        function: leak(function),
        file: leak(file),
        line,
        color: 0,
    })));
    m.insert((function, name.to_string()), loc);
    loc
}

/// An open Tracy zone, closed when dropped.
#[cfg(feature = "tracy")]
pub struct Zone(tracy_client_sys::___tracy_c_zone_context);

#[cfg(feature = "tracy")]
impl Zone {
    /// Opens a zone at `loc` on this thread.
    #[inline]
    pub fn begin(loc: &'static Location) -> Zone {
        // SAFETY: `loc` lives for ever; the client starts with the process
        // (no `manual-lifetime`).
        Zone(unsafe { tracy_client_sys::___tracy_emit_zone_begin(&loc.0, 1) })
    }
}

#[cfg(feature = "tracy")]
impl Drop for Zone {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: a zone `begin` opened, closed once.
        unsafe { tracy_client_sys::___tracy_emit_zone_end(self.0) }
    }
}

/// A zone for script callback `method` at level 2 or more, else nothing
/// (one thread-local lookup by name when on).
#[cfg(feature = "tracy")]
// Lookups only, never iterated.
#[allow(clippy::disallowed_types)]
pub fn callback_zone(method: &str) -> Option<Zone> {
    if !on(2) {
        return None;
    }
    thread_local! {
        static LOCS: std::cell::RefCell<std::collections::HashMap<String, &'static Location>> = Default::default();
    }
    let loc = LOCS.with(|m| {
        let mut m = m.borrow_mut();
        match m.get(method) {
            Some(l) => *l,
            None => {
                let l = location("lua callback", method);
                m.insert(method.to_string(), l);
                l
            }
        }
    });
    Some(Zone::begin(loc))
}

/// What `prof_span!` returns without the `tracy` feature. A unit struct, not
/// `()`, so `clippy::let_unit_value` (and its `--fix`) never strips the
/// `let _s =` that keeps the real span alive with the feature on.
pub struct NoSpan;

/// Empty, so `drop(span)` can close a span early in both builds without
/// `clippy::drop_non_drop` firing when the feature is off.
impl Drop for NoSpan {
    fn drop(&mut self) {}
}

/// A profiling span entered for the rest of the enclosing scope:
/// `let _s = prof_span!("receive");`. A no-op without the `tracy` feature.
#[macro_export]
macro_rules! prof_span {
    ($name:literal) => {{
        #[cfg(feature = "tracy")]
        let s = tracing::trace_span!($name).entered();
        #[cfg(not(feature = "tracy"))]
        let s = $crate::engine::tracy::NoSpan;
        s
    }};
    // With fields: `prof_span!("doscript", path = %path)`.
    ($name:literal, $($fields:tt)+) => {{
        #[cfg(feature = "tracy")]
        let s = tracing::trace_span!($name, $($fields)+).entered();
        #[cfg(not(feature = "tracy"))]
        let s = $crate::engine::tracy::NoSpan;
        s
    }};
}

/// A value plotted in Tracy per call at level 3: `prof_plot!("intel
/// changes", n)`. A no-op without the `tracy` feature.
#[macro_export]
macro_rules! prof_plot {
    ($name:literal, $value:expr) => {{
        #[cfg(feature = "tracy")]
        if $crate::engine::tracy::on(3)
            && let Some(c) = tracy_client::Client::running()
        {
            c.plot(tracy_client::plot_name!($name), $value as f64);
        }
        #[cfg(not(feature = "tracy"))]
        let _ = &$value;
    }};
}

/// Names the calling thread in Tracy's timeline (nothing without the
/// feature).
pub fn name_thread(name: &str) {
    #[cfg(feature = "tracy")]
    if let Some(c) = tracy_client::Client::running() {
        c.set_thread_name(name);
    }
    #[cfg(not(feature = "tracy"))]
    let _ = name;
}

/// Names the calling thread in Tracy after its OS name (helper threads
/// call this first thing; unnamed threads stay as they are).
pub fn name_current() {
    if let Some(n) = std::thread::current().name() {
        name_thread(n);
    }
}

/// One rendered frame ends.
pub fn frame_mark() {
    #[cfg(feature = "tracy")]
    if let Some(c) = tracy_client::Client::running() {
        c.frame_mark();
        plot_memory(&c);
    }
}

/// One of the second head's frames ends (its own frame track: it runs at
/// its monitor's rate, apart from the main window's).
pub fn head2_frame_mark() {
    #[cfg(feature = "tracy")]
    if let Some(c) = tracy_client::Client::running() {
        c.secondary_frame_mark(tracy_client::frame_name!("head 2 frame"));
    }
}

/// Plots resident memory (one OS query, ~1 µs; no allocator hooks).
#[cfg(feature = "tracy")]
fn plot_memory(c: &tracy_client::Client) {
    if let Some(m) = memory_stats::memory_stats() {
        c.plot(tracy_client::plot_name!("memory MB"), m.physical_mem as f64 / (1024.0 * 1024.0));
    }
}

/// One sim tick ends (a second frame track, since sim and render run on
/// different threads and rates).
pub fn sim_tick_mark() {
    #[cfg(feature = "tracy")]
    if let Some(c) = tracy_client::Client::running() {
        c.secondary_frame_mark(tracy_client::frame_name!("sim tick"));
        plot_memory(&c);
    }
}

/// The span for a sim stage (static names, so Tracy groups them).
#[cfg(feature = "tracy")]
pub fn stage_span(phase: crate::engine::watchdog::Phase) -> tracing::Span {
    use crate::engine::watchdog::Phase as P;
    match phase {
        P::Idle => tracing::Span::none(),
        P::Economy => tracing::trace_span!("Economy"),
        P::Threat => tracing::trace_span!("Threat"),
        P::Orders => tracing::trace_span!("Orders"),
        P::Movement => tracing::trace_span!("Movement"),
        P::Construction => tracing::trace_span!("Construction"),
        P::Reclaim => tracing::trace_span!("Reclaim"),
        P::Weapons => tracing::trace_span!("Weapons"),
        P::Projectiles => tracing::trace_span!("Projectiles"),
        P::LuaThreads => tracing::trace_span!("LuaThreads"),
        P::Sync => tracing::trace_span!("Sync"),
        P::Compact => tracing::trace_span!("Compact"),
        P::Snapshot => tracing::trace_span!("Snapshot"),
        P::Intel => tracing::trace_span!("Intel"),
        P::Input => tracing::trace_span!("Input"),
        P::LuaGc => tracing::trace_span!("LuaGc"),
        P::Destroy => tracing::trace_span!("Destroy"),
    }
}
