//! Tracy profiler hooks (`cargo run --release --features tracy`).
//!
//! With the feature on, `main` adds a `tracing-tracy` layer, so the spans
//! below (sim tick stages, render frame phases) and every log event show up
//! in the Tracy GUI, with a frame mark per rendered frame and a secondary
//! "sim tick" mark per tick, and the process's resident memory plotted
//! ("memory MB") at each mark. Without the feature every hook compiles to
//! nothing: default builds pay no span cost. Profiling never reads into sim
//! state.

/// What `prof_span!` returns without the `tracy` feature. A unit struct, not
/// `()`, so `clippy::let_unit_value` (and its `--fix`) never strips the
/// `let _s =` that keeps the real span alive with the feature on.
pub struct NoSpan;

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
