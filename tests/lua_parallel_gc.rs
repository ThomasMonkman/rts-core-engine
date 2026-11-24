//! A full collection marks on several threads (`RTS-CORE-ENGINE PARMARK` in
//! `vendor/luau0-src`, `lgc.cpp`; `sim::gc::parallel`). It must free exactly
//! what the collection on one thread frees, or peers with different core
//! counts would desync.
//!
//! Plain Luau VMs, no game data. One test only: the setting is the
//! process's.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mlua::prelude::*;
use rts_core_engine::engine::lua::heap_snapshot;
use rts_core_engine::engine::sim::gc;

/// A heap of `N` objects of every collectable kind, referring to earlier
/// ones at random (same every run): about a third are held from `ROOT`, the
/// rest only through each other and through weak tables (weak keys, weak
/// values, both; chains of weak-keyed entries whose values are other keys).
const BUILD: &str = r#"
local N = ...
local seed = 12345
local function rnd(n)
    seed = (seed * 1103515245 + 12345) % 2147483648
    return seed % n + 1
end
local K = setmetatable({}, { __mode = "k" })
local V = setmetatable({}, { __mode = "v" })
local KV = setmetatable({}, { __mode = "kv" })
local live = {}
-- one metatable for many objects, as a class's instances share theirs
local CLASS = { __index = {} }
ROOT = { live = live, K = K, V = V, KV = KV }
local all = {}
for i = 1, N do
    local earlier = all[rnd(#all + 1)]
    local kind = rnd(8)
    local o
    if kind <= 3 then
        o = { i, "s" .. i, earlier }
    elseif kind == 4 then
        local own = {}
        o = function() return earlier, own, i end
    elseif kind == 5 then
        local held = { i }
        o = coroutine.create(function(x)
            local up = { x }
            coroutine.yield(function() return up, held end)
            return up
        end)
        local _, f = coroutine.resume(o, earlier)
        if rnd(2) == 1 then V[#V + 1] = f end
    elseif kind == 6 then
        o = buffer.create(16)
    elseif kind == 7 then
        o = newproxy(true)
        getmetatable(o).ref = earlier
    elseif i % 2 == 0 then
        o = setmetatable({}, { __index = earlier })
    else
        o = setmetatable({ earlier }, CLASS)
    end
    all[#all + 1] = o
    if rnd(3) == 1 then live[#live + 1] = o end
    local other = all[rnd(#all)]
    local how = rnd(6)
    if how == 1 then
        K[o] = other
    elseif how == 2 then
        V[#V + 1] = o
    elseif how == 3 then
        KV[o] = other
    elseif how == 4 then
        K[o] = { o }
    elseif how == 5 then
        V[{ i }] = o
    end
end
"#;

fn new_lua() -> Lua {
    // The patched VM's maths functions are the engine's (`det_math`); naming
    // one links them into this test.
    std::hint::black_box(rts_core_engine::engine::lua::det_math::rts_det_pow as extern "C" fn(f64, f64) -> f64);
    let lua = Lua::new();
    heap_snapshot::prepare(&lua).expect("prepare");
    lua
}

fn build(objects: u32) -> Lua {
    let lua = new_lua();
    lua.load(BUILD).call::<()>(objects).expect("build");
    lua
}

/// What is left of a heap after one collection: every object (gcid, type)
/// and every string.
fn collect(lua: &Lua) -> (Vec<(u32, u8)>, Vec<Vec<u8>>) {
    heap_snapshot::collect(lua).expect("collect");
    heap_snapshot::validate(lua).expect("heap valid");
    let (mut objects, mut strings) = heap_snapshot::contents(lua).expect("contents");
    objects.sort_unstable();
    strings.sort_unstable();
    (objects, strings)
}

fn survivors(objects: u32) -> (Vec<(u32, u8)>, Vec<Vec<u8>>) {
    collect(&build(objects))
}

#[test]
fn parallel_mark_frees_what_the_serial_one_does() {
    const OBJECTS: u32 = 200_000;
    gc::parallel(usize::MAX);
    let serial = survivors(OBJECTS);
    assert!(serial.0.len() > OBJECTS as usize / 3, "the heap should keep a good part of its objects, kept {}", serial.0.len());
    gc::parallel(0);
    // More than once: which worker marks what differs from run to run.
    for run in 0..5 {
        let parallel = survivors(OBJECTS);
        assert_eq!(serial.0.len(), parallel.0.len(), "run {run}: objects kept");
        assert!(serial == parallel, "run {run}: the parallel mark kept different objects or strings");
    }

    // With pool threads busy (a window's renderer shares the pool): all but
    // one, so one thread runs the workers one after another, the later ones
    // after the mark is done; and all, so the calling thread marks alone.
    let threads = rayon::current_num_threads();
    for busy in [threads - 1, threads] {
        let lua = build(OBJECTS);
        let started = Arc::new(AtomicUsize::new(0));
        for _ in 0..busy {
            let started = started.clone();
            rayon::spawn(move || {
                started.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(200));
            });
        }
        while started.load(Ordering::SeqCst) < busy {
            std::thread::yield_now();
        }
        assert!(serial == collect(&lua), "{busy} of {threads} pool threads busy: the parallel mark kept different objects or strings");
    }
}

/// Times collections of heaps of several sizes with and without threads, to
/// place `gc::PARALLEL_MIN`: `cargo test --release --test lua_parallel_gc --
/// --ignored --nocapture`.
#[test]
#[ignore]
fn crossover() {
    for objects in [2_000u32, 5_000, 10_000, 20_000, 50_000, 100_000, 400_000] {
        let lua = new_lua();
        lua.load(BUILD).call::<()>(objects).expect("build");
        heap_snapshot::collect(&lua).expect("collect");
        let bytes = lua.used_memory();
        let mut best = [f64::MAX; 2];
        for _ in 0..20 {
            for (i, min) in [usize::MAX, 0].into_iter().enumerate() {
                gc::parallel(min);
                let start = std::time::Instant::now();
                heap_snapshot::collect(&lua).expect("collect");
                best[i] = best[i].min(start.elapsed().as_secs_f64() * 1e3);
            }
        }
        println!("{:>7.2} MB: one thread {:.3} ms, pool {:.3} ms", bytes as f64 / (1 << 20) as f64, best[0], best[1]);
    }
}
