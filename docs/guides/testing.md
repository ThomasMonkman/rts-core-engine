# Testing

Plain `cargo test` runs everything: the unit tests, every suite in `tests/` and the doctests, in
about a minute. Before merging into `develop`, run it and `cargo check --workspace --benches`, and
go by their exit codes, not a grep of the output.

```sh
cargo test                                    # everything
cargo test --lib                              # unit tests only
cargo test --test scenarios                   # one suite: tests/scenarios/*.lua + a 2000-tick AI game with no Lua errors
cargo test --test scenarios transport         # one test in it
cargo test --test player_orders               # player commands: stream, army check, queueing, replay == record
cargo test --bin rts-core-engine              # render-state tests and golden images (needs a GPU)
cargo check --workspace --benches             # the benches still compile
```

Benchmarks have their own guide: [benchmarks.md](benchmarks.md).

## How the suites run

Each suite process keeps session caches (the game-file index, translated and compiled scripts,
initialised blueprint state), so every test that boots a session after the first starts warm. A
process-per-test runner such as nextest loses those caches, so it isn't used.

Test binaries build under `[profile.test]`: `dev` with `opt-level = 3` and line-table debug info, so
the compute-heavy suites (the weapon audit census, scenario ticks) run optimised while `cargo run`
keeps its quick-to-build profile. The sim is fixed-point, so the optimisation level can't change its
results. A golden failure after a profile-only change is an engine bug, never a reason to re-record.

`dev/bench/test_suite_times.sh <outfile>` times each suite on its own, to compare a change against.

## Game data

Tests and benches that need data outside the repo (`../fa`, `../Supreme Commander Forged Alliance`,
`../original supreme commander unpacked`, `../faf-vault`) **fail** when it's missing, naming the path
(`rts_core_engine::test_data::require`). They don't pass silently. `RTS_SKIP_DATA_TESTS=1` skips
them instead, for a machine without the data (CI does this).

The paths are relative to the crate root, so in a git worktree under `.claude/worktrees/<name>/`
they resolve inside `.claude/worktrees/`: symlink the four folders there.

## Golden tests

A golden test compares a run with a file checked in under `tests/golden/`. Any change in behaviour
shows as a diff. After an intended change, re-record with `UPDATE_GOLDEN=1` and review the diff
before committing it.

| Golden | What it pins | Re-record |
|---|---|---|
| `fight.txt` | `dev/scenes/fight.lua`'s per-tick sim checksum | `UPDATE_GOLDEN=1 cargo test --test golden_fight` |
| `weapon_audit.txt` | every weapon's audit row ([audits.md](audits.md#weapon-audit)) | `UPDATE_GOLDEN=1 cargo test --test weapon_audit` |
| `images/*.png` | two rendered scenes: the UEF start area close up, the whole map zoomed out | `UPDATE_GOLDEN=1 cargo test --bin rts-core-engine golden_images`, then look at the new images |

The image comparison tolerates GPU rounding (mean channel difference under 2, under 1% of pixels off
by more than 32), and a failure saves `<name>.actual.png` beside the golden. The images were recorded
on Apple silicon (Metal); another GPU may need its own.

## Render-state tests

`src/render/render_tests.rs` drives a real engine into a real `Scene` on a windowless GPU device
(fixed interpolation, bones recorded for every model) and asserts on what the renderer worked out,
not on pixels, so results don't depend on the GPU. They check, for example:

- the Fatboy's drawn muzzles point at their targets (within 10 degrees)
- attached effects sit on their posed bones
- a 400-tick AI game leaves no entity drawn as a box and no asset failing to load (`Scene::asset_failures`)
- the vault maps' textures load, and decals cover their footprint

They skip without a GPU adapter and fail without the game data.

## Scenario tests

Each `tests/scenarios/<name>.lua` sets up a situation with FAF's real unit scripts and records checks in
a global `ScenarioResult` (`check(ok, msg)`, `note(msg)`, `done()`). `tests/scenarios.rs` runs it in a
session whose two armies have no AI and no commanders, steps until `done`, and fails with the failed
checks. They cover what unit tests can't: the engine and FAF's Lua agreeing (upgrades, enhancements,
capture, reclaim, formations, bombing runs, flight, fuel, armor, transports).

Add one per new gameplay system: copy a file, keep the helper block at its top, and add a `#[test]`
naming it. Use the land around (380-430, 560-670) on the default map (east of x 440 is water from z 600 down, where land units drown); much of the rest is water.
Iterate with the binary first:

```sh
cargo run --release -- --headless --ticks 600 --script tests/scenarios/x.lua --log x.log
```

To watch them, `dev/show_scenarios.sh [names...]` plays each in a window in turn, with its name and
result on screen and the camera following its units. Options: `SPEED`, `ZOOM`, `HOLD` (ticks shown
after it's done) and `KEEP=1` (keep the logs). On Windows:
`powershell -ExecutionPolicy Bypass -File dev\show_scenarios.ps1 [names...]` with `-Speed`, `-Zoom`,
`-Hold` and `-Keep`.

## Determinism checks

Headless progress lines (every 1000 ticks) carry the sim `checksum`; two runs of the same setup must
print the same ones. They also carry `lua_bytes`, the sim VM's heap size, which differs between
platforms; that's why Lua collections run at fixed ticks ([determinism.md](../architecture/determinism.md)).

A soak test: two full-length 4v4s side by side, in release.

```sh
for r in 1 2; do ./target/release/rts-core-engine --headless --ticks 36000 --log soak_$r.log & done; wait
for r in 1 2; do grep progress soak_$r.log | grep -o 'checksum=[0-9a-f]*' | tr '\n' ' '; echo; done
```

A divergence can be rare, so run 6-8 at once. These switches narrow one down (`engine/mod.rs`
`run_ticks`, `api/command.rs`):

| Variable | Effect |
|---|---|
| `RTS_TRACE_TICKS=a-b` | log the checksum every tick in that range, to find the first tick that differs |
| `RTS_DUMP_TICK=t RTS_DUMP_FILE=path` | write every live entity's state (position, health, orders, motion) at tick t, to `diff` two runs. A range `a-b` writes `path.<tick>` for each tick |
| `RTS_REFERENCE=path` | `path` holds `tick checksum` lines from a trace; at the first tick that differs, dump to `RTS_DUMP_FILE` and stop |
| `RTS_GC_EVERY_TICK=1` | a full Lua collection after every tick (the sim must not change) |
| `RTS_GC_PARALLEL_MIN=bytes` | heap size from which a collection marks on rayon's pool (`sim/gc.rs`; default 32 MB). `0` puts every collection there, the tests' small heaps too; a huge value keeps all on one thread (the sim must not change either way) |
| `RTS_LOG_ISSUE_UNIT=id` | log every order given to that unit, with the Lua stack that gave it, and how each ends. Running the traceback changes the game; add `RTS_LOG_ISSUE_NOSTACK=1` to leave it unchanged |

Then instrument with a `--script` (wrap the FAF function that gave the order and log its inputs every
tick) and diff the two logs.

## Continuous integration

`.github/workflows/ci.yml` builds and tests on Linux for every push to `main` / `develop` and every
pull request. CI has no game data, so it sets `RTS_SKIP_DATA_TESTS=1`: the data tests skip, the GPU
tests skip without an adapter, and what's left is the build, the unit tests, the data-free suites,
the doctests, `cargo check --benches` and [clippy](#clippy). `--no-fail-fast` reports every failing suite.

The player downloads are built by a separate workflow: [packaging.md](packaging.md#builds-in-ci-and-releases).

To keep CI quick:

- The compiled dependencies are cached by [rust-cache](https://github.com/Swatinem/rust-cache), keyed
  by `Cargo.lock` and the toolchain. Only `develop` saves the cache; a pull request restores
  `develop`'s.
- Our own crates always rebuild, at opt-level 0 and without debug info or incremental compilation
  (environment variables in the workflow). Without game data the tests run in seconds anyway, and
  debug info would fill the runner's disk.
- `build.rs` only watches git files that exist. Cargo treats a missing `rerun-if-changed` path as
  changed, and CI's shallow checkout has no `packed-refs`, so otherwise every cargo command rebuilt
  the engine crate.

## Clippy

`cargo clippy --workspace --all-targets -- -D warnings` must pass (CI runs it). The pinned toolchain
installs clippy. Two places configure it:

- `[workspace.lints.clippy]` in `Cargo.toml`: the default lints, minus a few allowed ones that fight
  the code's shape (long argument lists, nested generic types, indexed loops). Each workspace crate
  opts in with `[lints] workspace = true`.
- `clippy.toml`: the determinism bans (libm float functions, wall clock, `HashMap` / `HashSet`),
  turned on only in `src/engine/sim` and `src/engine/api` outside tests. See
  [determinism.md](../architecture/determinism.md#checked-by-clippy).

`cargo clippy --fix` applies most suggestions, with two catches. The repo isn't rustfmt-formatted,
so a collapsed `if` keeps its old body indentation, which you have to fix by hand. And check the
fixes: a guard or `inspect` rewrite can fail to compile, and `--fix` backs out the whole crate when
that happens.

## If the GPU tests crawl

On macOS, the first Metal call asks for the process's main bundle, and CoreFoundation lists the
directory the executable is in, `target/debug/deps`. Stale build output piles up there, and with
enough of it the listing takes many seconds per test process. `cargo clean` fixes it.
