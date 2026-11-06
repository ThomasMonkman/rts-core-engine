# Running the engine

Every command runs from the repo root. The engine uses the sibling checkouts `../fa` (FAF's Lua) and
`../Supreme Commander Forged Alliance` (the game data) when they're there. Otherwise it looks for the
FAF client, Steam and GOG installs, or takes `--fa <DIR>` / `--game-dir <DIR>`
([packaging.md](packaging.md)). It logs to `game.log`, and the window's **Logs** panel shows the same
lines.

`cargo run -- --help` lists every option. A mistyped option is an error with a suggestion, never
silently ignored.

Tests have their own guide: [testing.md](testing.md).

## Quick start

```sh
cargo run                                   # the main menu
cargo run -- --session                      # straight into a skirmish: Seton's Clutch, AIs, 1x speed
cargo run -q -- --headless --ticks 400      # 40 game seconds with no window, as fast as possible
grep -n ERROR game.log                      # the first error
cargo test                                  # every test
```

## Build profiles

| Profile | Command | Use |
|---|---|---|
| `dev` | `cargo run` | Day-to-day work. Our crate at opt-level 1 with full debug info; dependencies optimised (opt-level 3, no debug info). Quick rebuilds |
| `test` | `cargo test` | The test suites, at opt-level 3 so the heavy ones run fast |
| `release` | `cargo run --release` | Long runs, censuses, benches and Tracy. No LTO and incremental, so still quick to rebuild |
| `dist` | `cargo build --profile dist` | Shipping builds: thin LTO, one codegen unit. Slow to build; binary in `target/dist/` |
| `dev-cranelift` | `cargo run --profile dev-cranelift` | Debug builds with the Cranelift backend; `cargo watch-cranelift` rebuilds and runs on save |

Timings shown in a `dev` build (ms/tick, frame times) are debug numbers: measure in `release`.
`Cargo.toml` has a commented `debug = "line-tables-only"` for our crate in `dev`: slightly faster
rebuilds, but no local variables in the debugger.

**Faster linking on Apple silicon.** Homebrew's `ld64.lld` links noticeably faster than the default
linker. `brew install lld`, then add this to your own `~/.cargo/config.toml` (not the repo's, which
would break machines without it):

```toml
[target.aarch64-apple-darwin]
linker = "/opt/homebrew/bin/ld64.lld"
rustflags = ["-C", "linker-flavor=ld64.lld"]
```

## Windowed

```sh
cargo run                                   # the main menu (FAF's front end; no session yet)
cargo run -- --session                      # skip the menu: default skirmish, Seton's Clutch, adaptive AIs, 1x speed
cargo run -- --session --speed max          # sim as fast as it can go
cargo run -- --session --speed 4            # 4x (any multiplier: 0.25, 0.5, 2, 16, ...)
cargo run -- --script dev/scenes/fight.lua         # plus a staged 2v2 tank fight in mid-map
cargo run -- --overlay paths,ranges         # start with debug overlays on
cargo run -- --player 1                     # play army 1 (human slot with its ACU); the other army is the AI
cargo run -- --player 1 --record my.rtsreplay  # ... recorded to my.rtsreplay (every windowed game is recorded anyway)
cargo run -- --replay replays/Player/2026-10-11_08-30-05_scmp_009.rtsreplay   # watch a recorded game
cargo run -- --vault ../faf-vault --map /maps/monument_valley_63/monument_valley_63_scenario.lua --auto-teams north-south --player 1
                                            # 63-army showcase, north against south (63: the engine's army limit;
                                            # FAF's lobby has 16 slots)
cargo run -- --auto-teams left-right        # teams by start position, as FAF's lobby AutoTeams option:
                                            # north-south (tvsb), left-right (lvsr), odd-even (pvsi); replaces --teams
```

A plain `cargo run` opens FAF's main menu, as the real game starts. From there, Skirmish opens FAF's
lobby, and LAN / Direct Connect play over the network
([networking.md](../architecture/networking.md)); FAF's online services aren't supported.
`--session` skips the menu and starts the game the options describe. Any session option (`--map`,
`--armies`, `--script`, `--player`, `--replay`, ...) and `--headless` imply `--session`. The window
opens at 1024×768, SupCom's minimum resolution.

Without `--player` you watch the AIs, but you can still select and order any army's units.

### Controls

| Input | Action |
|---|---|
| WASD / arrow keys, middle-drag | pan |
| mouse wheel | zoom toward the cursor; straight down when zoomed out, tilting to 40 degrees close in (moho's camera curve) |
| left-click / left-drag | select a unit / box-select (shift adds; click empty ground to clear) |
| double-click | every unit of that type (and army) on screen |
| right-click | order by what's under the cursor: attack an enemy, reclaim a prop (engineers), repair a damaged / guard an own unit, else move |
| right-drag | move in formation, facing the drag direction |
| shift + order | queue it after the current orders |
| alt + right-click, or `T` then click | attack-move |
| `P` then click | patrol |
| `H` | stop |
| `F` | cycle formation |
| ctrl/cmd + 0-9, 0-9 (twice: centre camera), shift + 0-9 | set / recall / add to a control group |
| Esc | cancel an armed order, else clear the selection |
| `+` / `-` | game speed up / down (0.25x … 16x, max) |
| Pause (FAF's key map), or the bottom bar's pause | pause / resume |

With FAF's interface up, its own key map and panels work as in the real game
([ui.md](../architecture/ui.md)).

### Debug windows and the status bar

**Ctrl+`** shows and hides the debug windows (plain ` opens moho's console):

- **Overlays**: unit paths, weapon ranges, the nav grid per layer (impassable cells red), and the threat map as one army sees it.
- **Selected**: the first selected unit's state.
- **Logs** and **VFS**: the log and the mounted game files.

The status bar along the bottom shows FPS; the CPU and GPU time per frame and which of CPU, GPU or
vsync limits the frame rate; process and GPU memory; sim ms/tick and ticks/s; the speed selector,
tick and entity counts. The ⏷ at its right end hides it; ⏶ in the bottom-right corner brings it back.

### Test scenes

Scripts in `dev/scenes/` that set up something to look at, run by hand: `visual/` (rendering and
effects checks), `play/` (demos to play), `ui/` (interface cases), `watch/` (added to a game to log
as it plays: the census, units left under the map) and `stress/` (big battles for profiling), with
the standard skirmish `fight.lua` at the top. Scripts written as tests live in `tests/scenarios/`
instead. Most need `--no-ai` and a `--camera` (the commands below include them):

| Command | Shows |
|---|---|
| `cargo run -- --no-ai --map /maps/SCMP_040/SCMP_040_scenario.lua --script dev/scenes/visual/all_units.lua --camera 250,250,660` | every unit in the game, in rows by blueprint prefix, all on hold fire. Set `ALL_UNITS_FILTER` (a pattern on the id, e.g. `'^xs'`) to place some |
| `cargo run -- --no-ai --player 1 --script dev/scenes/play/air_demo.lua --camera 744,192,337` | every aircraft, for you to fly, against an enemy range that respawns. `AUTO = true` in the file flies them for you |
| `cargo run -- --no-ai --script dev/scenes/visual/effect_meshes.lua --camera 300,620,264` | a strategic missile per faction landing, and artillery on shields |
| `cargo run -- --no-ai --script dev/scenes/visual/graphics_showcase.lua --camera 415,598,66` | each faction's power and shield generators |
| `cargo run -- --no-ai --script dev/scenes/visual/particle_cases.lua --camera 370,545,35` | units firing, one effect case per row |
| `cargo run -- --no-ai --script dev/scenes/visual/all_emitters.lua --camera 383,560,132` | every emitter blueprint in a grid (index logged as `EMITTER` lines) |
| `cargo run -- --no-ai --script dev/scenes/visual/icon_cases.lua --camera 400,600,330` | strategic icons over four armies' units (ids logged as `ICONCASE` lines, for `--select`) |
| `cargo run -- --no-ai --script dev/scenes/visual/icon_air.lua --camera 405,608,308` | aircraft between the icon zoom and the mesh's last LOD |
| `cargo run -- --no-ai --script dev/scenes/visual/decal_cases.lua --camera 380,580,66` | decal LOD cutoffs: rows of scorches at three LOD settings; zoom out |

## Headless

Headless runs never open a window and never build render snapshots. They run as fast as they can
unless given a `--speed`.

```sh
cargo run -q -- --headless                  # session set-up only (0 ticks)
cargo run -q -- --headless --ticks 400      # 400 ticks (40 game seconds)
cargo run -q -- --headless --ticks 3000 --script dev/scenes/fight.lua
```

Every 1000 ticks, and at the end of a `--ticks` run, the log gets:

- a `progress` line: ms/tick, entity count, the sim checksum;
- a `stage ms` line: each sim stage's ms/tick, slowest first, plus the slowest tick's own breakdown
  and the path searches. The Lua garbage collection (every 100 ticks) is its own stage, `LuaGc`.

**Stall watchdog.** If no tick completes for `--stall-timeout` seconds (default 30; `0` turns it
off), the engine logs `ERROR ... sim stalled` with the tick and the stage. If Lua is spinning, it
also logs the Lua traceback and errors out of that script, and the sim carries on. A headless run
still stuck 10 s later exits with code 3, so a hung background run ends. `grep -E "sim stalled|progress" game.log`
shows both.

## Screenshots

The renderer saves its own frame, so no screen-recording permission is needed. It opens the window,
waits for the tick, writes a PNG and exits. The sim holds at that tick until the frame is captured,
so the shot is exactly that tick, even at `--speed max`. A hidden window (behind others, another
desktop, display asleep) renders offscreen, so this works anyway.

```sh
# whole map at tick 300
cargo run -- --session --screenshot shot.png --screenshot-tick 300

# the UEF base at 2 minutes, with paths and weapon ranges, ACU (entity 0) selected
cargo run -- --screenshot base.png --screenshot-tick 1200 --camera 668,350,154 --overlay paths,ranges --select 0

# land nav grid / threat map as seen by army 1
cargo run -- --screenshot nav.png --screenshot-tick 20 --overlay nav=land
cargo run -- --screenshot threat.png --screenshot-tick 600 --overlay threat=1

# a dialog in the interface
cargo run -- --screenshot menu.png --ui-lua "<Lua that opens it>"
```

In the main menu (no session), `--screenshot-tick` counts interface time (10 = 1 s) and is at least
6 s, so the menu's opening animations have finished.

## Replays

Every windowed game is recorded unless you pass `--no-record`. Recordings go to
`replays/<profile>/` (in the working directory for a source checkout, else in the settings folder:
[packaging.md](packaging.md)). A lobby game is recorded under the current profile, a `--session`
game under `Player`. Each file is named by its start time (UTC) and map, e.g.
`2026-10-11_08-30-05_scmp_009.rtsreplay`. The newest 100 default-named recordings per profile are
kept; renamed ones are never deleted.

```sh
cargo run -- --replay replays/Player/2026-10-11_08-30-05_scmp_009.rtsreplay   # watch it
cargo run -- --headless --replay <file> --ticks 3000                          # replay headless
```

The main menu's **Replay** dialog lists and plays them too. The sim is deterministic, so the same
build plays the same game. A replay from another build is warned about and may play differently,
and a map from a vault needs the same `--vault`. File format and internals:
[replays.md](../architecture/replays.md#recordings).

## Options

`cargo run -- --help` has the full list. The ones you'll use most:

### Session

| Flag | Meaning | Default |
|---|---|---|
| `--map <scenario path>` | VFS path of a `_scenario.lua` | `/maps/scmp_009/scmp_009_scenario.lua` |
| `--armies <n>` | AI armies filled from the start spots (0 = every spot) | 0 |
| `--teams <n>` | teams by start spot, alternating as FAF's lobby does (2 = odd against even); 0 or 1 = free for all | 2 |
| `--auto-teams <layout>` | teams by start position: `north-south`, `left-right`, `odd-even`; replaces `--teams` | off |
| `--seed <n>` | sim random seed | 0 |
| `--ticks <n>` | ticks to run, then stop. A window runs them at `--speed` and then closes | windowed: forever |
| `--speed <x\|max>` | game speed. A window defaults to 1; a headless run goes flat out unless given this | 1 / max |
| `--script <file.lua>` | a Lua file from disk, run in the sim after `BeginSession` | none |
| `--player <army>` | the army you play: a human slot with its commander | none (observer) |
| `--no-ai` | no AI and no starting commanders (a `--script` drives the units) | off |
| `--prebuilt` | FAF's PrebuiltUnits option: each army starts with a small base | off |
| `--fog <none\|explored\|unexplored>` | the FogOfWar option | explored |
| `--victory <condition>` | the Victory option: `sandbox` (never ends), `demoralization`, `domination`, `eradication`, `decapitation` | sandbox |
| `--mod <uid>` | run a mod as if the lobby had it on (repeatable; recorded in the replay) | none |
| `--vault <dir>` | mount a FAF vault folder's `maps/` and `mods/`, e.g. `--vault ../faf-vault` | none |
| `--log <file>` | log file; give runs in parallel separate files | `game.log` |
| `--record <file>` / `--no-record` | record to this file / don't record | windowed: `replays/` |
| `--replay <file>` | play a recording (headless too; `--ticks` still applies) | off |
| `--stall-timeout <secs>` | seconds without a tick before the watchdog reports a stall; 0 = off | 30 |

The run-ahead process for replay seeking uses `--keyframes-out`, `--keyframe-every`, `--background`
and `--parent-pid` ([replays.md](../architecture/replays.md)). Networked games use `--net-host`,
`--net-join` and friends ([networking.md](../architecture/networking.md#running)).

### Window

| Flag | Meaning |
|---|---|
| `--camera x,z,zoom` | initial camera: look-at point and zoom (the ground across the window's longer side, as moho's `GetZoom`) |
| `--overlay <list>` | comma list of `paths`, `ranges`, `nav=land\|hover\|naval\|amph`, `threat=<army>`, `fog=<army>` |
| `--select <ids>` | entities selected at start (their orders show as the command graph) |
| `--screenshot <png>`, `--screenshot-tick <n>` | save one frame at that tick (default 50), then exit |
| `--ui-lua <code>` | Lua run in the interface once it's up (open a dialog for a screenshot) |
| `--cursor X,Y` | where the cursor starts, in physical pixels; screenshot what hovering shows |
| `--hold-shift` | Shift counts as held (screenshot queued orders, reclaim labels) |
| `--prefs <file>` | the `Game.prefs` to read and save ([ui.md](../architecture/ui.md#preferences)). Screenshot runs use none unless given this |
| `--window-size WxH` | the window's size in pixels, whatever the display (profile at another machine's resolution) |
| `--second-head <windowed\|offscreen\|w,h,hz>` | FAF's second head: a second window, drawn offscreen, or borderless on another monitor |
| `--no-aa` | start with anti-aliasing off |
| `--gpu-passes` | log each render pass's GPU time every 120 frames ([rendering.md](../architecture/rendering.md#gpu-time-per-pass---gpu-passes)) |
| `--no-audio` | no sound output (use it for test runs); headless runs never open a device |
| `--audio-dump <wav>` | write the first 20 s of the mix to a WAV ([audio.md](../architecture/audio.md)) |
| `--vfs-dump [file]` | write every mounted game file path to `file` (default `vfs_dump.txt`) |

### Exit codes

`0` success; `2` the engine failed to start (bad `--map`, a Lua error during set-up, game data not
found); `3` the stall watchdog ended a headless run.

## Performance

How to run the criterion benches and compare a change: [benchmarks.md](benchmarks.md). For a whole
game, the `stage ms` lines in a headless run's log (above) show where tick time goes.

### Profiling with Tracy

Build with the `tracy` feature and open the [Tracy](https://github.com/wolfpld/tracy) GUI. The crates
are pinned to protocol 0.13.1, which Homebrew's `tracy` ships.

```sh
brew install tracy          # once; the GUI is `tracy-profiler`
tracy-profiler &            # connect to "rts-core-engine" when it appears
cargo run --release --features tracy -- --session --speed max
```

It shows:

- a frame mark per rendered frame, and a "sim tick" mark per tick ("head 2 frame" for the second head);
- spans for every sim stage (`Economy`, `Movement`, `LuaThreads`, ...) inside `sim tick`, and for the
  render frame (`receive snapshots`, `prepare scene`, `ui`, `submit`, `present`);
- every log event as a message;
- on GPUs with timestamp queries, a "wgpu" track timing the `scene pass` and `ui pass`;
- model loading (`load model`, `upload models`) and a "memory MB" plot of resident memory.

Without the feature the hooks compile to nothing.

To profile the Lua, build with `lua-profile` (it turns on `tracy` too). Every Lua thread resume
becomes a zone inside `LuaThreads`, named by the function the thread was forked with
(`/lua/ai/gridrecon.lua:204`). Tracy's Statistics view then ranks scripts by time. It's a separate
feature because it adds a lookup to every thread resume.

```sh
cargo build --release --features lua-profile
tracy-capture -o lua.tracy -f &             # save a trace from a headless run
./target/release/rts-core-engine --headless --ticks 3000
```

### Stress maps (manual)

`dev/scenes/stress/gen_stress_map.py <n>` writes an `n`-army scenario on SCMP_030 into
`dev/vault/maps/stress_<n>/` (16 and 63 are checked in; 63 armies is the engine's limit).
`dev/scenes/stress/stress.lua` spawns 1000 land units per army (`StressPerArmy` overrides it) and sends
everyone to the centre:

```sh
cargo build --release --features tracy   # or without the feature
tracy-capture -o stress_16.tracy -f &    # optional: saves a trace when the run ends
./target/release/rts-core-engine --headless --vault dev/vault --no-ai --armies 0 \
    --map /maps/stress_16/stress_16_scenario.lua --script dev/scenes/stress/stress.lua --ticks 1200
```

The other scripts in `dev/scenes/stress/` (each one's header has its command line):

| Script | What |
|---|---|
| `land.lua` | free-for-all on `stress_63`: 1000 tanks and 10 experimentals per army, all attack-moving on the centre |
| `air.lua` | the same with fighters: one huge dogfight |
| `headon.lua` | two armies of 2000 moving straight at each other |
| `city.lua` | two armies of 1000 crossing a city of ~14k structures |
| `crossing.lua` | on any map with start markers: every army crosses to the farthest start |

```sh
cargo run --release -- --no-audio --vault dev/vault --no-ai --armies 0 --player 1 \
    --map /maps/stress_63/stress_63_scenario.lua --script dev/scenes/stress/land.lua
```

### Long AI games (manual)

`dev/scenes/watch/census.lua` logs each AI's units, economy and commander enhancements every 5 game minutes. A
30-minute game runs in well under a minute in release:

```sh
cargo build --release
./target/release/rts-core-engine --headless --ticks 18000 --script dev/scenes/watch/census.lua --log census.log
grep -E "CENSUS|ACUENH" census.log
grep -E "WARN lua|ERROR" census.log | grep -v "Overriding\|Preferences\|AI DEBUG" | sort | uniq -c | sort -rn
```

### Small machines

The engine sizes its thread pools from the core count (`rts_core_engine::threads::cores`). Rayon's
global pool (one worker per core) runs the sim's stages and background loading; the render pool (a
third of the cores, 2-6) prepares frames at a higher priority; the sim, audio, network and watchdog
each have their own thread. `RTS_CORES=n` sizes the pools for `n` cores. To see a small machine's
start-up on a big one, size for it and keep the other cores busy:

```sh
for i in $(seq 1 14); do (while :; do :; done) & done   # 18 hardware threads -> about 4 left
RTS_CORES=4 /usr/bin/time -p ./target/release/rts-core-engine --headless
kill $(jobs -p)
```

## LOUD

LOUD (github.com/LOUD-Project/Git-LOUD) isn't an FAF mod. It replaces the base game's Lua and data,
and it is launched with its own init file, which `--fa` takes like FAF's:

```sh
# Git-LOUD checked out as <FA>/Git-LOUD (its README's layout): its dev init file
cargo run -- --fa "<FA>/Git-LOUD/bin/SupComDataPath.lua" --game-dir "<FA>" --vault "<FA>"
# a headless game: 8 LOUD AIs on Seton's Clutch
cargo run -- --fa "<FA>/Git-LOUD/bin/SupComDataPath.lua" --game-dir "<FA>" --vault "<FA>" \
    --headless --map /maps/scmp_009/scmp_009_scenario.lua --ticks 3000
```

- `<FA>` is the Forged Alliance folder. LOUD's init files mount the base game's data from two folders
  above their own (`InitFileDir .. '\..\..\gamedata\...'`). A LOUD release uses `bin/LoudDataPath.lua`
  the same way.
- Init files are written for Windows. The engine takes their backslash paths and wildcards anywhere
  in a file name (`BrewLAN-StrategicIcons*.scd`) on every platform, mounting matches in
  case-insensitive name order (`api::io::expand_disk_glob`).
- **Maps:** the dev init file mounts LOUD's maps from the release's `LOUD/maps`, which a Git-LOUD
  checkout doesn't have. `--vault <FA>` mounts the base game's `maps/<name>` at `/maps/<name>`.
  LOUD's skirmish lobby opens on `LastScenario` (else LOUD's own default map, Blasted_20); with
  only the base game's maps, pick one in the lobby.
- **Packs** (BlackOps, Total Mayhem, BrewLAN, 4D, Wyvern, LOUD Unit Additions, ...) are mods under
  `/mods`. LOUD's lobby runs the ones the profile selected in its mod manager
  (`/lua/mods.lua` `GetSelectedMods`: the `active_mods` preference, empty on a new profile), so
  `--session` runs none, as a new profile does. Add a pack with `--mod <uid>`; the uid is in the
  pack's `mod_info.lua`, e.g. LOUD Unit Additions `fffffffe-6e98-4864-9599-4133236eea7a`, BlackOps
  Unleashed `9e8ea941-c306-4751-b367-a11000000502`, Total Mayhem `62e2j64a-53a2-y6sg-32h5-146as555a18u3`.

What runs differently on game code that isn't FAF's (`engine::game_code::GameCode`: FAF has
`/lua/version.lua`, LOUD has `/lua/loudutilities.lua`):

| | FAF | LOUD | other (base game) |
|---|---|---|---|
| The engine's hooks (`/engine_hook`: PiP, replay timeline, 63-slot lobby) | on | off | off |
| Option setters run when an interface starts | sound and video | all (moho's `OPTIONS_Apply`) | all |
| `--session`: options the code's lobby defaults (`lobbyoptions.lua`, `/lua/CustomOptions`) | no | yes | yes |
| `--session`: the AI, when the code's lobby list has no 'adaptive' | - | its first ('loud') | its first |
| `--session`: LOUD's slot fields (`PlayerCount`, `WheelColor`, `Mult`, `ACT`) | no | yes | no |
| A mod named `Hotstats` is installed (FAF's `mods.lua` skips it) | no | yes | yes |

LOUD's AI logs a lot when asked: set `ScenarioInfo.EngineerDialog`, `PriorityDialog`,
`DisplayFactoryBuilds`, `PlatoonDialog` or `InstanceDialog` to `true` in a `--script`.

## Comparing against the real game

`dev/probes/` holds a kit for running the same script in the real game and diffing the logs:
[verifying-against-the-binary.md](../research/verifying-against-the-binary.md).

The weapon audit and the other audits: [audits.md](audits.md).
