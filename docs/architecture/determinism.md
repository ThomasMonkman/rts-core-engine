# Determinism

## Goal
Bit-identical sim output across Windows x86 and Mac ARM (and any future platforms).

## Fixed-point math
Use `fixed` or `substrate-fixed` crate for all sim positions, velocities, HP, resources. No `f32`/`f64` in hot sim paths. This eliminates the main source of cross-platform divergence.

### Fixed-point square root (`sim::math::sqrt`)
`fixed`'s `sqrt` is a slow bit-by-bit loop. `math::sqrt` seeds the integer root of `bits << 32` with an f64 `sqrt` and then corrects it with integer comparisons to exactly `floor(sqrt(n))`, which is also what `fixed` returns: bit-identical results, several times faster (test `sqrt_matches_fixed_exactly`). The float is only a starting guess; even if a platform's `sqrt` were wrong, the integer correction would still land on the same root (just more slowly). IEEE 754 requires `sqrt` to be correctly rounded anyway. Use `math::sqrt`, not `Fx::sqrt`, in sim code.

`math::atan2` and `math::sin_cos` are integer-only: octant / quarter-turn reduction, then a fixed polynomial evaluated by Horner on I32F32 bits in `i128` (A&S 4.4.49 for atan, error < 5e-8; Taylor to r^12 for sin/cos, error < 2e-9). No floats, so bit-identical everywhere, and several times faster than the `cordic` crate they replaced. Changing either (coefficients, reduction, rounding) changes sim results: re-record the golden (`UPDATE_GOLDEN=1`).

## Checked by clippy
`clippy.toml` bans, in `src/engine/sim` and `src/engine/api` outside tests: the libm float functions (`sin`, `atan2`, `powf`, `powi`, `exp`, ...; plain `+ - * / sqrt` are exact IEEE on both platforms and stay allowed), `Instant::now` / `SystemTime::now`, and `HashMap` / `HashSet`. hashbrown probes in SIMD groups of 16 on SSE2 but 8 on NEON, so even with a fixed hasher the iteration order can differ between x86 and ARM. A use that's safe (lookup-only, sorted before walking, wall time for a log line) takes an `#[allow(clippy::disallowed_types)]` / `disallowed_methods` with a comment saying why. CI runs clippy with warnings denied.

## If floats are unavoidable
- Disable fast-math (`-fno-fast-math` / `/fp:precise`)
- Force SSE2, never x87 (`/arch:SSE2` or `-msse2 -mfpmath=sse`)
- Avoid `std::sin/cos/sqrt/atan` — use a cross-platform software math lib (SLEEF or custom)
- Avoid FMA unless used identically on both platforms
- Factorio's approach: replace stdlib trig with custom implementations, then test with per-tick CRC across thousands of scenarios

## SIMD / AVX2 / NEON
- **Not cross-platform deterministic** — AVX2 and NEON/NEON do not produce bit-identical FP results
- AVX2 is deterministic within the same ISA level; SVE varies by vector width between ARM chips
- For the sim: avoid SIMD FP entirely or restrict to non-determinism-sensitive paths (rendering, audio)

## Struct layout for hashing
```rust
#[repr(C)]          // field order guaranteed, C-compatible padding
#[derive(Pod, Zeroable)]  // bytemuck — safe to reinterpret bytes
struct UnitState {
    pos_x: i64,     // fixed-point
    pos_y: i64,
    pos_z: i64,
    hp: i32,
    _pad: [u8; 4],  // explicit padding — never leave implicit gaps
}
```
- Use explicit `_pad` fields so hash covers fully-defined bytes
- `Zeroable` guarantees padding starts zeroed — never uninitialized

## Per-tick checksumming
```rust
use xxhash_rust::xxh3::Xxh3;

fn hash_sim_state(world: &World) -> u64 {
    let mut h = Xxh3::new();
    for (id, unit) in world.units.iter_sorted_by_id() { // MUST be sorted
        h.update(bytemuck::bytes_of(&unit)); // safe via Pod
    }
    h.digest()
}
```
- Always sort by entity ID before hashing — HashMap order is non-deterministic
- Include RNG state if used
- Use `xxhash-rust` XXH3 — fastest option
- In production: hash once per tick, broadcast u64 to peers
- In debug builds (`#[cfg(feature="sync_debug")]`): hash after each sub-step for bisection

## Testing
Factorio's method: CRC every tick across thousands of automated test scenarios comparing x86 vs ARM builds. Any divergence surfaces immediately.

What exists now:
- `sim::checksum` (`Engine::checksum`): XXH3 over native sim state packed in id/index order, in parts (`PART_NAMES`): entities (position, facing, health, build progress), motion (units' speed, course, spin), orders (every busy unit's queue: ids, kinds, targets, counts), projectiles, weapons, economy (the whole record per resource: storage, income, requested, spent, totals), RNG, sim mods. The checksum is the hash of the parts; a networked desync logs which parts differ. Lua state isn't hashed; it follows from these through the deterministic VM.
- Golden scenario `tests/golden_fight.rs`: runs `dev/scenes/fight.lua` (2 Strikers vs 2 Mantises) for 250 ticks and compares the per-tick checksum plus the end-state unit table with `tests/golden/fight.txt`. A sim change shows as the first differing tick. When the change is intended: `UPDATE_GOLDEN=1 cargo test --test golden_fight` and review the diff. The same golden file must pass on x86 and ARM; that is the cross-platform check.
- Parallel passes (projectiles, weapons) have unit tests asserting parallel == serial.

## Rules for engine API implementations
- Any Rust API exposed to the sim Lua VM must return results in a deterministic order (sorted file lists, sorted entity ids). Never expose `HashMap` iteration order to Lua.
- No wall-clock time or OS-dependent values (paths case, line endings, locale) may influence sim-visible results; normalise them at the VFS boundary.
- Parallel work (rayon) is fine for pure computations, but must be collected and merged in a deterministic order before it touches sim state or Lua.

## Lua garbage collection must not change the sim

When a collection happens differs between runs and machines: allocation depends on memory addresses
and on the platform's allocation sizes, and scripts can see when collections happen (weak tables
such as FAF's TrashBags and reclaim grid; dead keys changing `pairs` order). So sim behaviour must
never depend on collection timing:

- **The sim VM's automatic collector is off** (`sim::gc`). It runs a full collection after each
  start-up stage and every 100 ticks at the end of the tick, the same on every machine.
- **Engine objects that sim logic reaches through weak tables need a strong reference.** For
  example, FAF holds manipulators only in weak TrashBags, so a collection could drop a recoil
  slider while its native side kept moving the muzzle. The engine holds every manipulator's Lua
  object until it is destroyed (`api/manipulator.rs`, as moho's native objects keep theirs).
- `tests/golden_fight.rs` guards this: the fight and a 400-tick AI game, each run normally and with
  a full collection forced after every tick, must give the same per-tick checksums.

## Luau patched for determinism

Upstream Luau isn't deterministic across runs. These patches make it so (all
in `vendor/luau0-src`, listed in its `MOHO_PATCHES.md`, grep `RTS-CORE-ENGINE
DETERMINISM`):

- **GC cycles run to completion** (`lgc.cpp` `luaC_step`). Upstream paces the
  collector on a wall-clock allocation rate and marks incrementally,
  interleaved with scripts; how far each step gets depends on table node
  order. So garbage made mid-cycle survives in one run and not in another.
  Scripts see that: weak tables (FAF's reclaim grid `cell.Reclaim`,
  TrashBags) and dead keys (a key removed and re-added after the collector
  marked it dead lands in another node, changing `pairs` order; FAF's
  `GridReclaim.UpdateList` then updated a different 8 cells, and an AI
  engineer chose another reclaim cell). Now a cycle runs whole, so it depends
  only on reachability.
- **One collection frees everything dead** (`lgc.cpp`
  `convergeephemerons`). A table weak on one side keeps an entry's other
  side only while the weak side is alive; upstream kept it one collection
  longer, which a keyframe load could not reproduce
  (`docs/architecture/replays.md`).
- **Object keys hash by creation order** (`gcid` on tables, userdata,
  closures, threads, buffers), not by address.
- **`tostring(obj)` prints the `gcid`**, not the address; FAF builds strings
  and keys from it.
- **`GetSystemTimeSecondsOnlyForProfileUse` returns sim seconds** in the sim
  VM (`api/profiling.rs`): FAF formats it into log strings, and different
  string contents shift allocation.

How to find the next one: see "Determinism checks" in `docs/guides/testing.md`. The tell for GC
trouble: with `collectgarbage` stopped, the heap size at each startup stage is the same in every
run, but with the collector on it isn't.

## The same game on Mac ARM and Windows x86

The golden fight and full-length AI games match on every tick between Mac M-series and Windows
x86-64 (same commit, same data). Garbage collection timing was the one real cause of a split (see
above). Two more rules keep compiled C and C++ from differing:

- **FMA contraction.** Apple clang fuses `a * b + c` into one FMA by
  default (`-ffp-contract=on`); MSVC x64 never does, and Lua's `%`
  (`a - floor(a / b) * b`) and `math.lerp` would round differently. Luau is
  built with `-ffp-contract=off` (and `/fp:precise` on MSVC); check with
  `llvm-objdump -d libluauvm.a | grep -c fmadd` (should be 0).
- **Platform libm.** `math.sin/cos/tan/asin/acos/atan/atan2/sinh/cosh/tanh/
  exp/log/log2/log10/pow`, the `^` operator and the compiler's constant
  folding of them would call the C library (Apple libm vs the MSVC CRT).
  They call `rts_det_*` (`Luau/DetMath.h`), implemented in Rust by
  `src/engine/lua/det_math.rs` on the `libm` crate (musl's algorithms, IEEE
  basic operations only). `sqrt` stays the hardware instruction (exactly
  rounded by IEEE).

How to find a split between machines: `RTS_TRACE_TICKS` on both and `diff` the traces for the
tick; `RTS_DUMP_TICK=a-b` (one file per tick, full order queues) to see what differs;
`RTS_LOG_ISSUE_UNIT` with `RTS_LOG_ISSUE_NOSTACK` to see where an order was issued. Without
`NOSTACK` the log runs Lua (`debug.traceback`), which creates objects and so changes the game from
that point. The progress line's `lua_bytes` shows whether the heap sizes differ.

Rust code in the sim never fuses or calls the platform libm on its own
(fixed-point throughout; `f64` only in tests and the renderer). Any new C or
C++ code that feeds the sim needs the same two rules, and nothing the sim
does may depend on memory sizes or addresses.
