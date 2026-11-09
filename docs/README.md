# Docs

New here? Read [CONTRIBUTING.md](../CONTRIBUTING.md) first, then [the architecture overview](architecture/overview.md).

## Guides: how to do things

| Doc | What's in it |
|---|---|
| [running.md](guides/running.md) | Running the engine: quick start, build profiles, windowed, headless, screenshots, replays, the options, Tracy and stress maps |
| [testing.md](guides/testing.md) | The test suites, golden files, scenario tests, determinism checks, CI |
| [lua-api-types.md](guides/lua-api-types.md) | Adding engine APIs: `lua_fn!`, `lua_method!`, `callbacks!`, typed tables and enums, the `--dump-api` Luau/EmmyLua output |
| [benchmarks.md](guides/benchmarks.md) | Running the benches and comparing a change against a baseline; whole-game timing runs |
| [packaging.md](guides/packaging.md) | Shipping builds: how the game and FAF folders are found, what's built into the binary, the package scripts |
| [audits.md](guides/audits.md) | The scripts that find missing APIs, callbacks, blueprint fields and weapon failures, and how to re-run them |
| [debugging-lua.md](guides/debugging-lua.md) | Debugging the game's Lua in VS Code: breakpoints, stepping, the stack and variables (`--lua-debug`) |
| [probing.md](guides/probing.md) | Running the same script in the real game and ours to compare behaviour (needs Windows and FAF) |

## Architecture: how it works

| Doc | What's in it |
|---|---|
| [overview.md](architecture/overview.md) | Start here: the threads, the sim/render split, where the code lives |
| [game-start-sequence.md](architecture/game-start-sequence.md) | What happens from launch to the first tick |
| [sim.md](architecture/sim.md) | The sim's map: the tick stages, the patterns every system follows, entity storage, how orders run, which file holds each system |
| [determinism.md](architecture/determinism.md) | Fixed-point maths, SIMD, checksums, cross-platform rules |
| [lua-bridge.md](architecture/lua-bridge.md) | mlua/Luau, translating SupCom Lua to Luau, the one script-loading path, `import`, coroutines, callbacks |
| [blueprints.md](architecture/blueprints.md) | How blueprints are loaded and post-processed |
| [vfs-assets.md](architecture/vfs-assets.md) | The virtual file system, `.scd`/`.nx2` archives, the `.scmap`/`.scm` decoders |
| [replays.md](architecture/replays.md) | Saving and loading the whole sim (replay keyframes, save games) |
| [networking.md](architecture/networking.md) | Peer-to-peer lockstep: links, turns, input delay, slowdown, the test harness |
| [rendering.md](architecture/rendering.md) | wgpu, the main and present threads, the mesh pipeline |
| [ui.md](architecture/ui.md) | The interface: its Lua VM, lifecycle, controls and drawing, input, how it sees and acts on the sim, adding user-layer APIs |
| [pip.md](architecture/pip.md) | Picture-in-picture popups: small world views that follow a unit, and the API for mods |
| [audio.md](architecture/audio.md) | XACT banks, the audio thread and mixer, world sounds |

## Reference

| Doc | What's in it |
|---|---|
| [moho-api.md](reference/moho-api.md) | The moho binding surface: orders, economy, weapons |
| [crates.md](reference/crates.md) | Every dependency and why it's there |
| `api/` | Generated output, not checked in (git-ignored): the Luau and EmmyLua types for the whole Lua API (`cargo run -- --dump-api docs/api`) and the audit scripts' reports ([audits.md](guides/audits.md)) |

## What's missing or unconfirmed

Open work is tracked in GitHub issues labelled `tracking`, not in the docs:

| Issue | What's in it |
|---|---|
| #1 | Sim API stubs that still raise or do nothing |
| #2 | Interface API stubs and placeholders |
| #3 | Interface callbacks moho makes that the engine never calls |
| #4 | Items held for a design decision (player abandonment, formation choice, save / load) |
| #5 | Blueprint fields the real engine reads that we ignore |
| #6 | Weapon audit: weapons that fail to engage or hit |
| #7–#12 | Behaviour still to verify against the real game, by area |

[audits.md](guides/audits.md) explains how each list is produced and how to re-run it.

## Research

| Doc | What's in it |
|---|---|
| [verifying-against-the-binary.md](research/verifying-against-the-binary.md) | How to check behaviour against the real game (static disassembly, probes), and everything confirmed so far |
| [weapon-aim-model.md](research/weapon-aim-model.md) | How moho aims and fires turreted weapons, read from the retail binary |

## Archive

[benchmark-results.md](archive/benchmark-results.md): benchmark and stress results recorded during development (some source comments cite it).
