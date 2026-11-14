# Contributing

This page covers getting set up, the dev loop, the rules every change is held to, and how changes land.
For what the project is, see the [README](README.md). For how it's built, see [`docs/`](docs/README.md).

## Setup

### What you need

- **Rust**: nothing to choose. `rust-toolchain.toml` pins a nightly, and rustup installs it on the first build. Every lockstep peer must build with the same compiler, so it's bumped deliberately, never floating.
- **Supreme Commander: Forged Alliance**: a bought copy (Steam or GOG). The engine ships no game assets: models, textures, sounds and maps come from your install, found automatically or given with `--game-dir`.
- **FAF's Lua**: free. A checkout of [FAForever/fa](https://github.com/FAForever/fa) beside this repo, on tag **3841**, the version the engine is developed and tested against (the golden tests assume it): `git clone https://github.com/FAForever/fa ../fa && git -C ../fa checkout 3841`.
- **Linux**: `libasound2-dev`, `pkg-config` and Vulkan drivers. macOS and Windows need nothing extra.

### Reference checkouts

Check these out beside this repo, since the engine and the docs refer to them:

| Folder | What it is | Used for |
|---|---|---|
| `../fa/` | [FAForever/fa](https://github.com/FAForever/fa) | the Lua the engine runs; the reference for every moho API |
| `../FA-Binary-Patches/` | [FAForever/FA-Binary-Patches](https://github.com/FAForever/FA-Binary-Patches) | `moho.h` and the engine's real bindings: signatures, struct layouts |
| `../faf-wiki/` (optional) | [FAForever/wiki-pages](https://github.com/FAForever/wiki-pages) | what blueprint fields and APIs mean, as modders describe them |
| `../Supreme Commander Forged Alliance/` | your game install | game data, if it isn't found automatically |

## Finding something to work on

Open work is in the [issues](../../issues). The `tracking` issues are checklists by area:

- #1 sim APIs that are still stubs, and #2 interface ones: each is a self-contained piece of moho to implement.
- #3 callbacks the real engine makes into FAF's interface that we don't yet.
- #5 blueprint fields the real engine reads and we ignore.
- #6 weapons that fail to engage or hit in the weapon audit.
- #7–#12 behaviour to check against the real game, by area ([verifying-against-the-binary.md](docs/research/verifying-against-the-binary.md)).

Issues labelled `good first issue` are small and well defined. Comment on an issue before starting something big, so work isn't duplicated.

## The dev loop

Most work is making more of FAF's Lua run:

1. `cargo run -- --session` (straight into a skirmish; plain `cargo run` opens the main menu). The engine logs to `game.log`.
2. Find the first Lua error: `grep -n ERROR game.log`.
3. Implement the missing moho API against the FAF reference (`../fa`, `../FA-Binary-Patches`). Take signatures from the binding code, not from FAF's annotations.
4. Repeat.

Use debug builds while iterating, and `--release` for long runs, benchmarks and profiling.
[`docs/guides/running.md`](docs/guides/running.md) has every run mode: windowed, headless, screenshots, replays, stress maps and Tracy. [`docs/guides/testing.md`](docs/guides/testing.md) covers the tests.
[`docs/guides/lua-api-types.md`](docs/guides/lua-api-types.md) shows how to register a function, method or callback.

## Rules for every change

The engine runs in lockstep: every peer runs the whole sim, and a single differing bit desyncs the game. So every new API or system is checked against these three points before it's merged.

### 1. Determinism

- Anything the sim can observe must be bit-identical on x86 and ARM:
  - no `f32`/`f64` maths whose result feeds sim state. Use the sim's fixed-point types from `src/engine/sim/math.rs`: `Fx` (the [`fixed`](https://docs.rs/fixed) crate's `I32F32`: 32 integer bits, 32 fractional bits) and `FxVec2`, `FxVec3`, `FxQuat` built on it. Lua numbers (`f64`) are converted to `Fx` at the API boundary.
  - no `HashMap`/`HashSet` iteration order leaking into results (use `BTreeMap` or `IndexMap`, or sort before iterating)
  - no wall-clock time, no results that depend on thread ordering, no unseeded RNG
- Order matters to Lua. Anything that returns a list to Lua (file listings, entity queries, table keys built in Rust) returns it in a stable, sorted order.
- In the sim VM, Lua's `math.random`, `os.time`, `os.clock` and the like go through deterministic sources the sim owns.
- The render thread reads sim snapshots and never writes sim state.

Clippy checks part of this. In `src/engine/sim` and `src/engine/api` it rejects the libm float functions (`sin`, `atan2`, `powf`, ...), `Instant::now` and `HashMap`/`HashSet` (`clippy.toml`). Where a use is safe, for example a lookup-only map, add `#[allow(clippy::disallowed_types)]` with a comment saying why.

More in [`docs/architecture/determinism.md`](docs/architecture/determinism.md).

### 2. Performance

- Hot paths (anything run per tick, per unit or per blueprint) don't allocate, re-parse or repeatedly convert strings. Cache once and reuse.
- Don't hold a lock while Lua runs: a nested `doscript` would re-enter it. Copy what you need and drop the guard first.
- Measure before choosing Rust or Luau. A Rust callback costs about 60–100 ns of mlua overhead, so tiny leaf functions on Lua values (vector maths, small table helpers) are *faster* as Luau shims. For example, FAF's vector helpers (`VAdd`, `VDist3`, `VDot`, ...) are written in Luau in `src/engine/lua/shims/vector_math.lua`, because the `lua_call_overhead` bench in `benches/engine.rs` measured `VDist3` at 32 ns in Luau against 95 ns as a native callback (run it with `cargo bench -- lua_call`). The other files in `src/engine/lua/shims/` follow the same rule. Use Rust for anything that touches engine state, does heavy work or can run in parallel.
- Pre-size tables (`create_table_with_capacity`) when you know their shape.
- Benchmark hot paths: add or extend a criterion bench in `benches/engine.rs` for per-tick, per-unit or startup code you add or change, and compare it against a baseline from `develop` ([how](docs/guides/benchmarks.md#comparing-a-change)). Don't bench one-off setters.

#### Profiling with Tracy

Benches measure one function on its own. [Tracy](https://github.com/wolfpld/tracy) shows a whole running game, every sim stage, render phase and log line on a timeline, per thread.

```sh
brew install tracy                     # once the GUI is `tracy-profiler` (protocol 0.13.1)
tracy-profiler &                       # connect to "rts-core-engine" when it appears
cargo run --release --features tracy -- --session --speed max
```

- Add `--features lua-profile` to see each Lua thread resume as its own zone, named by the script function it was forked from. Tracy's Statistics view then ranks scripts by cost.
- For a headless capture to a file, use `tracy-capture -o run.tracy -f &`, then run the engine with `--headless --ticks N`.
- To time your own code, add a span: `let _s = prof_span!("my stage");` (`src/engine/tracy.rs`). It lasts until the end of the scope and compiles to nothing without the `tracy` feature.

[`docs/guides/running.md`](docs/guides/running.md#profiling-with-tracy) covers what each track shows and the GPU timing.

### 3. Parallelism

- Independent, side-effect-free work (file reads, decompression, blueprint parsing, translation) runs on `rayon`, and its results are merged in a deterministic order.
- The sim's Lua VM is single-threaded. Parallelise the Rust work around it, never inside a Lua callback.
- thread pooling work out has a fixed cost: waking rayon's workers takes about ~100 µs. Below a measured size, do the work serially on the calling thread (the sim thread, for sim stages), and only thread out above it. Many stages already do this with a `PARALLEL_MIN` constant (`weapon.rs`, `movement.rs`, `avoidance.rs`, `render/scene.rs`). Use `.with_min_len(n)` so each task gets enough work. Find the crossover with a bench, as in [`benchmarks.md`](docs/guides/benchmarks.md).

## Tests

```sh
cargo test                          # all suites; needs the game data
RTS_SKIP_DATA_TESTS=1 cargo test    # without the game data (what CI runs)
cargo check --benches
cargo clippy --workspace --all-targets -- -D warnings
```

Before opening a PR, run all three and check that each exits cleanly.
Golden files and scenario tests are described in [`docs/guides/testing.md`](docs/guides/testing.md).

Any new work should have tests and/or benches to back it up.

## Branches, commits and PRs

- `develop` is the integration branch. `main` holds releases.
- Branch off `develop`: `feature/<name>` for features, `fix/<slug>-<issue>` for issue fixes. Open the PR into `develop`.
- When a change adds an engine API, commit it on its own with the message `feat: added <ApiName>`. For other changes, use the conventional prefixes already in the history (`fix:`, `docs:`, `build:`, `chore:`).
- Group related PRs that touch the same files into a stack instead of opening them all against `develop`.
