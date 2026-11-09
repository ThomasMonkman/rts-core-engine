# Agent notes

A from-scratch reimplementation of the Supreme Commander: Forged Alliance engine in Rust, running FAF's Lua unchanged via `mlua` + Luau.

**Read [CONTRIBUTING.md](CONTRIBUTING.md) first.** Its rules for every change (determinism, performance, parallelism) are non-negotiable: check every new engine API or system against them before committing. [docs/README.md](docs/README.md) indexes the rest of the docs.

## Design constraints in brief
- **Lockstep determinism**: the sim must be bit-identical on Windows x86 and Mac ARM. Use fixed-point maths in the sim; no `f32`/`f64` in sim paths.
- **Rust-native layout** with a Lua translation layer: don't bend Rust data structures to fit Lua; bridge them instead.
- **Sim and render are decoupled**: the render thread interpolates from sim snapshots and never mutates sim state.

## Agent-specific rules
- Commits: one commit per engine API added, with the message `feat: added <ApiName>` (match the existing history). No co-author trailers.
- Dev loop: `cargo run -- --session`, then `grep -n ERROR game.log` for the first Lua error, implement the missing moho API against `../fa` and `../FA-Binary-Patches`, commit, repeat.

## Working with sub-agents (token budget)

- **Pick the model per agent.** Use the top model for root-cause work (binary disassembly, engine design, tricky determinism). Use a smaller model (Sonnet/Haiku) for mechanical work: rebases, golden re-records, running probes, PR text, stack housekeeping.
- **Triage before fixing.** Run one cheap pass per issue first to answer "engine bug or test/audit artefact, and what evidence exists?" Two of six "engine bugs" in one batch were audit artefacts. Only start a fix agent for a confirmed engine bug.
- **Fewer, larger agents.** Group related issues into one agent instead of one agent per issue. Avoid parallel agents editing the same files (`weapon.rs`, goldens): it causes serial rebases and repeated re-records. Use parallel agents only for truly independent work.
- **Short reports, quiet logs.** Ask agents for reports under ~300 words. Keep logs and census output in files, and grep them instead of pasting them into context.
- **Verify once.** Run `cargo test` once at the end of a change, not after every step. Run the full weapon-audit bearing set (`WEAPON_AUDIT_BEARINGS=all`) only when bearings are what changed.
- **New session per batch.** End a batch with a short handoff note, and start the next batch in a fresh session instead of growing one very long conversation.

## Topic files (load as needed)
| File | When to read |
|------|-------------|
| `docs/architecture/overview.md` | Threads, data flow, where the code lives |
| `docs/guides/running.md` | Run commands: windowed, headless, screenshots, replays, flags, Tracy, stress maps |
| `docs/guides/testing.md` | The test suites, goldens and re-recording, scenario tests, determinism checks (trace / dump variables), CI |
| `docs/guides/debugging-lua.md` | The Lua debugger: `--lua-debug`, the VS Code extension in `dev/vscode-lua-debug/`, how breakpoints/stepping work, limits |
| `docs/guides/probing.md` | Differential probes in the real game (`dev/probes/`, Windows + FAF) |
| `docs/guides/lua-api-types.md` | The typed Lua API: registering functions/methods/callbacks (`lua_fn!`, `lua_method!`, `callbacks!`), typed tables/enums, `--dump-api` Luau/EmmyLua output, the checks |
| `docs/guides/benchmarks.md` | Running the criterion benches, comparing against a saved `develop` baseline, whole-game timing runs |
| `docs/guides/packaging.md` | Shipping builds to players: how the FAF / game folders are found (`src/install.rs`), what's built into the binary, the Windows / macOS package scripts |
| `docs/architecture/game-start-sequence.md` | Game start-up sequence |
| `docs/architecture/lua-bridge.md` | Lua/mlua integration, SupCom Lua → Luau translation, the single script-loading path (never `lua.load` game source directly), import, coroutines, callbacks |
| `docs/architecture/sim.md` | The tick stages, sim patterns (id order, parallel compute / serial apply), entity storage, orders, a table of systems to files. Exact rules are in each module's `//!` comments |
| `docs/architecture/determinism.md` | Cross-platform FP, SIMD, checksumming, repr(C) |
| `docs/architecture/vfs-assets.md` | VFS, .scd/.nx2 archives, .scmap/.scm decoders |
| `docs/architecture/rendering.md` | wgpu setup, egui logging UI, mesh pipeline |
| `docs/architecture/ui.md` | The interface: user-layer Lua VM, lifecycle, controls, input order, snapshots in / commands out, prefs, adding user-layer APIs |
| `docs/architecture/pip.md` | Picture-in-picture popups: sim `Request`, UI `Show`, the options, the example mod |
| `docs/architecture/networking.md` | LAN / direct-connect lockstep: UDP link, mesh, turns, input delay, slowdown, test harness |
| `docs/architecture/audio.md` | XACT banks, the audio thread and mixer, sound globals, world sounds via snapshots, `--audio-dump` |
| `docs/architecture/replays.md` | Saving / loading the whole sim (replay keyframes, save games): the base, the heap format, what's saved, rules for new sim state, how to debug a load that drifts |
| `docs/reference/moho-api.md` | Moho bindings surface: orders, economy, weapons |
| `docs/reference/crates.md` | Full crate list with rationale |
| `docs/research/verifying-against-the-binary.md` | How to check behaviour against the real game (static disassembly, probes in `dev/probes/`), and what's confirmed. Open items: issues #7–#12; add new guesses there |
| `docs/guides/audits.md` | Re-running the audits (API stubs, fakes, callbacks, blueprint fields, weapon audit). Results: `tracking` issues #1–#6 |
| `docs/research/weapon-aim-model.md` | How moho aims and fires turreted weapons (from the binary) |

## Reference folders

All are siblings of this repo (go up exactly one level).

| Folder | When to read |
|------|-------------|
| `../original supreme commander unpacked/` | The original Supreme Commander files |
| `../fa/` | FAF's code |
| `../FA-Binary-Patches/` | FAF's binary patches, with the `moho.h` bindings |
| `../faf-wiki/` (clone of `FAForever/wiki-pages`, the source of wiki.faforever.com) | Game mechanics as players and modders describe them: blueprint field meanings (`Development/Modding/Blueprints.md`), Lua API notes (`LUADOC.md`, `LUAGLOBALS.md`), per-unit stats (`generated/`) |
