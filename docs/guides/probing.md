# Probing the real game

A probe is a Lua script that runs unchanged in the real game and in ours, logging what it sees as
`PROBE ...` lines. Diff the two logs and you have the real engine's behaviour next to ours: unit
order, timings, positions, what the interface shows. Use one when the binary or FAF's code leaves a
question open. Other ways to check (strings, disassembly) and everything confirmed so far:
[verifying-against-the-binary.md](../research/verifying-against-the-binary.md).

## What you need

- **Windows with the FAF client installed.** Probes run in FAF's `ForgedAlliance.exe`: it's DRM-free,
  has FAF's engine patches and loads FAF's Lua, so it's the same game our engine targets. The stock
  Steam `SupremeCommander.exe` is DRM-wrapped and runs the unpatched 2007 engine.
- **A desktop session.** The game needs a display, so a probe can't run over ssh.
- macOS has no way to run it: CrossOver fails on the Steam DRM. `dev/probes/run_fa.sh` is the old
  CrossOver runner, kept in case that changes.

## The kit (`dev/probes/`)

| Path | What |
|---|---|
| `maps/probe_<name>/` | one probe per map folder: `probe.lua` (the body), plus a scenario and a map script that runs it in the real game |
| `maps/probe_<name>/ui_probe.lua` | optional: an interface half that runs in the user layer once the interface is up |
| `run_faf.ps1` | runs a probe in FAF's `ForgedAlliance.exe` and writes `out\<probe>.faf.log` |
| `init_probe.lua` | init file for the stock game: the game data plus `dev/probes/maps` at `/maps` |
| `ui_hooks/` | mounted at `/schook` by `run_faf.ps1`: hooks `gamemain.CreateUI` to start `ui_probe.lua` |
| `out/` | logs and screenshots (not committed) |

## Writing a probe

Copy a folder (`maps/probe_fight` is the simplest) and rename it and its files. In `probe.lua`:

- Use engine APIs only (`CreateUnitHPR`, `IssueMove`, `GetPosition`, ...), not FAF's helpers that
  might differ, so the same file runs in both engines.
- Log with `LOG('PROBE ', ...)`, formatting numbers to a fixed precision (`string.format('%.2f', n)`).
  The real game uses floats and we use fixed point, so compare order and structure exactly and
  numbers within a tolerance.
- Log `PROBE done` at the end: the runner stops the game when it sees it.

Gotchas:

- In the real game the probe body runs at tick 0, while the armies' storage is still 0: a
  `GiveResource` at the top of the file without a `GiveStorage` in the same tick is lost. FAF's own
  `GiveStorage('Energy', 100)` (`CreateBrainShared`) shows from tick 2.
- `GiveStorage` keeps one script storage pair: `GiveStorage('MASS', ...)` then
  `GiveStorage('ENERGY', ...)` leaves no mass storage (in both engines). For mass storage, build a
  storage structure (`ueb1106`).
- A `ForkThread` with `WaitTicks(1)` started by the probe body resumes at tick 2 in the real game and
  tick 1 in ours: compare ticks counted from a `t0` the probe takes itself.
- In ours, a windowed run with `--ticks N` runs the sim back to back at 1x and ignores pause and game
  speed from the interface. Probe those without `--ticks` (stop the run by hand).

## Running it

```powershell
# the real game (Windows desktop): writes dev\probes\out\probe_fight.faf.log
powershell -ExecutionPolicy Bypass -File dev\probes\run_faf.ps1 probe_fight
```

```sh
# ours: the same body as a --script
cargo run -- --headless --ticks 160 --script dev/probes/maps/probe_fight/probe.lua --log probe_fight.log
grep PROBE probe_fight.log
```

Then diff the `PROBE` lines from the two logs.

A `--script` file is translated exactly as game Lua is, so the one body means the same in both: `!=`, the implicit `arg`, `for k, v in t`, `#` as a comment and `^` as XOR all behave as in the real game. Use `table.getn` and `math.pow`.

### All-AI games and mods

A probe map's optional `launch_args.txt` adds to the game's command line. With
`/probeai <key>[,<key>...]` the `ui_hooks` `SinglePlayerLaunch.lua` hook starts every army as that
AI (a list is cycled by army) instead of the stock `/map` launch (army 1 human, the rest 'rush'),
set up as ours sets up `--session --ai`: the same options with the lobby defaults (the mods' AI
options included), factions UEF, Aeon, Cybran, Seraphim by army. `/probemod <uid>[,<uid>...]` runs
those sim mods (they must be in the game's mods folder) instead of the game prefs' active mods.
Give the probe scenario the real map's `name`: game code picks plans by it (FAF's AI has a plan
only for `"Seton's Clutch"`).

### Interface probes

A map folder's `ui_probe.lua` runs in the real game's user layer once its interface is up. In ours,
pass the same file's code with `--ui-lua`. Example: `maps/probe_order`.

### Visual probes

A `PROBE shot <name>` log line makes `run_faf.ps1` capture the screen to `out\<probe>_<name>.png`.
The probe holds the view for a few seconds after logging it. In ours, run the same `ui_probe.lua`
through `--ui-lua` with `PROBE_NO_PAUSE` set, and `--screenshot` at the same tick. Example:
`maps/probe_buildbeams`, whose `ui_probe.lua` pauses, points the camera with `SnapTo` and logs a shot
per view.

At the same `SnapTo` zoom, moho's camera shows about 0.81x the ground ours does (unexplained).
Comparisons scale ours with `PROBE_ZOOM_SCALE`.

## After a probe

Record what it showed: tick the item on its "Verify against the binary" issue (#7–#12) and add a
row to [Confirmed](../research/verifying-against-the-binary.md#confirmed), naming the probe.
