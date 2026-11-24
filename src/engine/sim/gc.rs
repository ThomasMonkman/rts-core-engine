//! When the sim VM collects garbage: at fixed ticks, never by allocation.
//!
//! Collection timing is visible to scripts (FAF's weak tables: TrashBags,
//! the reclaim grid; finalizers), so every peer must collect at the same
//! points. Luau triggers a collection when its heap has grown by so many
//! bytes, and byte counts are not the same across platforms: the same game
//! held 50 KB more on Windows x86 than on Mac ARM at tick 1000 (allocation
//! sizes and padding differ between compilers), so the machines collected
//! at different moments and a 4v4 split at tick 22006 (an AI engineer's
//! order existed on one machine only). The sim VM's automatic collector is
//! therefore off for good; it collects fully after each start-up stage
//! ([`collect`]) and every [`EVERY`] ticks at the end of the tick ([`tick`]),
//! the same on every machine.
//!
//! A collection stops the sim for its whole length, which grows with the
//! heap, so its mark phase runs on rayon's pool ([`parallel`]).

use std::ffi::{c_int, c_void};

use mlua::prelude::*;

/// Ticks between collections (10 s of game time). A full collection of a
/// 4v4's 150-200 MB heap takes 40-60 ms; Luau's byte trigger had been
/// collecting about every 25 s.
pub const EVERY: u64 = 100;

/// Heap size from which a collection marks on rayon's threads
/// ([`parallel`]). Below it waking the pool costs more than it saves: the
/// two were level at 26 MB (4 ms) on 18 cores, `crossover` in
/// `tests/lua_parallel_gc.rs`. A 1v1's heap is over 100 MB.
pub const PARALLEL_MIN: usize = 32 << 20;

type Worker = unsafe extern "C" fn(ctx: *mut c_void, index: c_int);

unsafe extern "C" {
    fn lua_rts_gcparallel(run: Option<unsafe extern "C" fn(n: c_int, f: Worker, ctx: *mut c_void)>, threads: c_int, min_bytes: usize);
}

/// Runs the collector's `n` mark workers: the first on this thread, the rest
/// on rayon's pool. The mark doesn't wait for a worker whose pool thread is
/// busy: it starts late, or finds the mark done and returns at once. This
/// call does wait for each to have run, so with the whole pool busy it
/// returns when one pool thread comes free.
unsafe extern "C" fn rayon_run(n: c_int, f: Worker, ctx: *mut c_void) {
    let ctx = ctx as usize;
    rayon::in_place_scope(|s| {
        for i in 1..n {
            s.spawn(move |_| unsafe { f(ctx as *mut c_void, i) });
        }
        unsafe { f(ctx as *mut c_void, 0) };
    });
}

/// Full collections of heaps of `min_bytes` or more mark on rayon's pool,
/// one worker per pool thread (`usize::MAX`: never). For every VM of the
/// process. The mark finds what is reachable in whatever order, so a
/// collection frees the same objects as on one thread: the thread count
/// doesn't show in the sim (`tests/lua_parallel_gc.rs`).
pub fn parallel(min_bytes: usize) {
    let threads = if min_bytes == usize::MAX { 1 } else { rayon::current_num_threads() };
    unsafe { lua_rts_gcparallel(Some(rayon_run), threads as c_int, min_bytes) };
}

/// Turns the automatic collector off (from VM creation), and parallel
/// marking on. `RTS_GC_PARALLEL_MIN=bytes` replaces [`PARALLEL_MIN`] (0: every
/// collection on the pool, so the tests' small heaps take that path too).
pub fn manual(lua: &Lua) {
    static MIN: std::sync::LazyLock<usize> =
        std::sync::LazyLock::new(|| std::env::var("RTS_GC_PARALLEL_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(PARALLEL_MIN));
    lua.gc_stop();
    parallel(*MIN);
}

/// A full collection, leaving the automatic collector off (a collection
/// re-arms Luau's trigger). Through the keyframe raw call once the VM has it
/// (`heap_snapshot::prepare`): mlua's `gc_collect` makes two closures for
/// its protected call, garbage the collection itself can't take, so a
/// collection tick's heap wouldn't hold only live objects.
pub fn collect(lua: &Lua) -> LuaResult<()> {
    if crate::engine::lua::heap_snapshot::is_prepared(lua) {
        return crate::engine::lua::heap_snapshot::collect(lua);
    }
    lua.gc_collect()?;
    lua.gc_stop();
    Ok(())
}

/// Tick `now` ends with a collection.
pub fn due(now: u64) -> bool {
    now % EVERY == EVERY - 1
}

/// End of tick `now`.
pub fn tick(lua: &Lua, now: u64) -> LuaResult<()> {
    if due(now) {
        let _s = crate::prof_span!("lua gc");
        collect(lua)?;
    }
    Ok(())
}
