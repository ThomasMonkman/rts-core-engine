# The interface (user layer)

Moho runs two Lua states: the sim, and the *user layer*, which runs FAF's interface scripts
(`/lua/userInit.lua`, `/lua/maui/*`, `/lua/ui/**`). We do the same, and FAF's interface Lua runs
unchanged. The engine side is `src/ui/` (the VM, controls, the user-layer API) plus the parts of
`src/render/` that draw and feed it.

```text
            snapshots (units, selection details, Sync, economy, fog)
  sim  ─────────────────────────────────────────────────────▶  UserVm (render thread)
 thread ◀─────────────────────────────────────────────────────   FAF's interface Lua
            player commands (orders, toggles, SimCallback)          │ draw list
                                                                    ▼
                                                                renderer
```

`ui::UserVm` lives on the render thread. It reads the sim only through snapshots and acts on it
only through player commands, so the interface can never touch sim state or break determinism. The
same rule as the renderer: [overview.md](overview.md).

## Lifecycle

**Front end.** A plain `cargo run` has no session. The engine mounts the files and runs
`src/ui/front_end.lua`: `uimain.SetupUI()`, then FAF's main menu (`uimain.StartFrontEndUI()`).
`GetCurrentUIState()` is `'frontend'`.

**Lobby.** Skirmish opens FAF's `lobby.lua` on the engine's `CLobby` (`src/ui/lobby.lua`): a lobby
with no network. LAN and Direct Connect lobbies are networked ([networking.md](networking.md)).
`LaunchGame(config)` hands the configuration to the renderer (`UserVm::take_launch_request`), which
sends it to the engine thread, drops the front end's VM and boots a fresh one for the game, carrying
the preferences over. Moho switches Lua states the same way.

**Game.** `UserVm::new` registers the engine globals, then a stub for every global FAF declares in
`/engine/User.lua` and `/engine/Core.lua`, then the implemented ones (`ui::user_api`, `ui::prefs`,
`ui::controls`). `init()` loads blueprints and runs `/lua/userInit.lua`. Once the session's armies
and units are in a frame, `start_hud()` (`src/ui/hud.lua`) starts the game interface as moho does:
`UserSync.lua`, `uimain.SetupUI()`, FAF's world UI provider, `CreateGameInterface(false)` (FAF's
whole `gamemain.CreateUI`) and `StopLoadingDialog()`.

Restarting or leaving a session: [replays.md](replays.md#recordings).

## Controls, layout and drawing

`InternalCreateFrame/Group/Bitmap/Text/...` (`ui::controls`) make a native control for the Lua object
FAF's class just built: a `_c_object` handle, the layout LazyVars (`Left Top Right Bottom Width
Height Depth`), then `OnInit`.

- **FAF's Lua does all the layout** (`LayoutHelpers`, LazyVar relations). The engine keeps each
  control's rectangle and depth and re-reads them only when they go dirty: a watcher LazyVar per
  control queues it, and the stale ones are read in one Luau call. A control FAF never laid out isn't
  drawn.
- **Drawing** (`render/ui.rs`): each frame the visible controls become a draw list of quads and text,
  back to front by `Depth` then tree order. As in SupCom, UI textures and glyphs share one atlas, so
  the interface is a few draw calls; images over 512 px get their own texture. Movies are a quad on
  their own pipeline ([rendering.md](rendering.md#movies-in-the-interface-renderuirs-shadersuiwesl-fs_movie)).
- **Display density:** the interface works in logical pixels, as the base game at the window's size.
  The window's scale factor divides the screen size and mouse positions it sees and multiplies its
  draw list, snapped to whole pixels. FAF's `ui_scale` option works on top.
- **Heads:** one root frame per head (`GetFrame(n)`). Each head draws its own frame's controls and
  the mouse only reaches the head it's on ([rendering.md](rendering.md#the-second-head-statesecondhead-framesecond_frame)).

## Input

**Mouse** (`ui::controls::mouse`). The topmost hit-testable control under the cursor gets
`HandleEvent`, bubbling to its parents until one returns true. Over a world view (or no control) the
input belongs to the world and goes on to the renderer's selection and orders (`render/player.rs`);
over any other control the world never sees it. Every press first goes to FAF's
`uimain.OnMouseButtonPress` (popups close on a click outside them).

**Keys** go to the first of these that takes them:

1. an `Edit` with the focus (typed text, editing keys, Enter / Esc callbacks);
2. the newest input capture (`AddInputCapture`: FAF's modal dialogs);
3. controls holding the keyboard focus (`AcquireKeyboardFocus`), newest first;
4. the key map: the player's bindings (FAF's preset, or their own from `Game.prefs`), looked up by
   FAF's key names and run as console commands (`UserVm::console`). Most run in the interface's Lua;
   a few (`UI_SelectByCategory`, `UI_TrackUnit`, ...) run in the engine (`render/console.rs`).

While an edit has the focus, the renderer's own keys (game speed, camera) stand down.

## Seeing the sim

**Units.** The interface sees units as `UserUnit`s (`src/ui/units.lua`), one Lua object per entity
id. Their methods read the latest snapshot (blueprint, army, health, position, work progress) and the
blueprints. Selected units also get their details (`snapshot::UnitDetail`: queue, economy, fire state,
script bits, build restrictions, stats), so FAF's panels answer from the sim. A detail's tables are
only rebuilt when that part changed, so a big selection doesn't make thousands of tables a beat.

**Selection.** When the renderer's selection changes, `UserVm::set_selection` runs FAF's
`gamemain.OnSelectionChanged`, so the unit view, orders and construction panels follow it. Details
arrive a beat after the selection, so the interface re-runs it when they do.

**Sync.** At the end of each beat the sim copies its `Sync` table (`engine::sync`; engine objects
become entity ids) into the next snapshot. The renderer gives each copy to the interface in order and
FAF's `OnSync` (`/lua/UserSync.lua`) runs, so unit data, printed text, scores and alerts flow as FAF
expects.

**Beats and frames.** Each new tick, `UserVm::beat` moves `GameTick`, updates the economy and runs
FAF's `gamemain.OnBeat`. Each frame, `UserVm::frame` runs user threads and `OnFrame`; building the draw list then calls `OnRenderWorld(delta)` of each world view with `SetCustomRender(true)`, whose `UI_Draw*` shapes go with that view to the renderer (`ui/world_draw.rs`).

**Fog of war.** The interface only sees what the renderer's frame holds, which is the view army's
fog-of-war snapshot ([rendering.md](rendering.md#fog-of-war)). Undetected enemies aren't in it at
all, and radar blips are drawn but left out of the interface's unit data, so nothing in FAF's Lua can
identify them.

## Acting on the sim

Everything the interface does to the game becomes a player command, applied by the sim a tick later
like any other input, and recorded in replays ([sim.md](sim.md)).

- **Orders.** FAF's `Issue*` globals queue orders, which the renderer turns into player commands
  (`UserVm::take_orders`, `ui::command_kind`). Orders that need a target arm FAF's command mode, and
  the next left click on the world gives them (`PlayerControl::issue_mode`).
- **Unit settings** (`SetFireState`, `SetPaused`, `ToggleScriptBit`) are commands carrying a
  `Toggle`, applied through each unit's own Lua methods, so FAF's callbacks run.
- **`SimCallback({Func, Args})`** (pings, diplomacy, give units, the cheat menu, ...) converts `Args`
  to plain data when called and is sent in order with the orders.
- **Questions the renderer can't answer** (can a structure go here?) go to the sim as a
  `DebugRequest` and come back in a later snapshot, e.g. the build ghost's green / red
  ([rendering.md](rendering.md)).

The engine also calls FAF's user-layer hooks, as moho does: `commandmode.OnCommandIssued` for every
order, `gamemain.OnPause` / `OnResume`, `gamemain.OnQueueChanged`, `commandgraph.OnCommandGraphShow`,
`uimain.OnApplicationResize`. Callbacks moho makes that we don't yet: #3.

## Cameras and world views

Each shown world view is a draw item with its rectangle and camera name. The scene draws each view's
camera into its rectangle, so FAF's split view works; with no interface the main camera fills the
window. Picks, box select, orders and rollover go through the view under the cursor
(`World::view_at`).

FAF's camera API (`GetCamera(name)`, `src/ui/camera.lua`) reads the renderer's cameras each frame and
queues moves (`MoveTo`, `SnapTo`, `SetZoom`, `TrackEntities`, `UIZoomTo`, ...), which the renderer
applies (`UserVm::take_camera_requests`). Timed moves ease from the current view; panning by hand
ends a move or a track. The camera's own maths follows the retail exe
([verifying-against-the-binary.md](../research/verifying-against-the-binary.md#confirmed)).

Picture-in-picture popups: [pip.md](pip.md). The replay timeline: [replays.md](replays.md#the-timeline).

## Preferences

`GetPreference` / `SetPreference` keep the player's settings (options, key bindings, window
positions, profiles) in one store (`ui::prefs`). Tables go in and come out as **copies**, as moho's
do: FAF's options code relies on it to spot what changed.

At start it loads the first file that exists (`install::choose_prefs`):

1. `--prefs <file>`;
2. `dev/Game.prefs` in a source checkout, if you make one (it isn't committed);
3. the engine's own `Game.prefs` beside `settings.ini` (`install::config_dir`);
4. on Windows, Forged Alliance's own (`%LOCALAPPDATA%\Gas Powered Games\Supreme Commander Forged
   Alliance\Game.prefs`), so a FAF player keeps their options and hotkeys. It's only ever read.

`SavePreferences` and closing the game write the store back in moho's format to the file it came
from (or the engine's own). `--screenshot` runs read and write nothing unless given `--prefs`, so
screenshots look the same on every machine.

## Adding to the interface

Run FAF's interface and look for what's missing:

- Unimplemented user-layer globals and methods (from FAF's `/engine/User*.lua`) **warn once and return
  nil**, so the interface builds around them. `grep WARN game.log` lists the ones hit. The open list:
  #2.
- `UI_DOSCRIPT=/lua/ui/<file>.lua cargo test --test ui debug_doscript -- --ignored --nocapture`
  imports one interface file and prints why it fails.
- `--ui-lua "<code>"` runs Lua in the interface once it's up. With `--screenshot`, it captures a
  dialog: `--ui-lua "import('/lua/ui/dialogs/options.lua').CreateDialog(GetFrame(0))"`.

Implement the API in `ui::user_api` (globals) or on the control in `ui::controls`, with the typed
macros ([lua-api-types.md](../guides/lua-api-types.md)). Take behaviour from the binary over FAF's
annotations ([verifying-against-the-binary.md](../research/verifying-against-the-binary.md)).

`tests/ui.rs` checks that userInit boots, FAF's layout places controls where it says, and the
economy bar shows the economy after a beat.
