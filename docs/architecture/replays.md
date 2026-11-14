# Replays: keyframes, seeking and saving the sim

A keyframe is the whole sim at the end of a tick, saved so a replay can jump
back to it and, later, as a save game. Moho did the same with
its `gpg` archives: about 230 per-class serializers plus serializers for Lua
5.0's internals (`lua_State`, `LClosure`, `Proto`, `UpVal`, `Table`, `Udata`),
so a saved game's coroutines came back mid-function; FAF's `OnPostLoad`
hooks only re-send UI data.

Code: `src/engine/keyframe/` (engine side), `src/engine/lua/heap_snapshot.rs`
and `vendor/luau0-src/luau/VM/src/lsnapshot.cpp` (the Lua heap). Test:
`tests/keyframes.rs`.

## The base

A keyframe loads into an engine that started **the same session the same
way** and stopped at the *base*: the end of game start-up, before the first
tick (`keyframe::mark_base`, called by `Engine::start`). Start-up is
deterministic, so everything made up to there — blueprints, classes, the
map's props, the engine's registries — is the same object with the same
`gcid` (creation-order id, `MOHO_PATCHES.md`) in both VMs, and protos the
same `protoid`. A keyframe only carries what changed since:

- **Base objects** are matched by gcid and keep the loading VM's object,
  with the saved state written over what can change: a table's contents, a
  Lua closure's upvalues, a coroutine forked at start-up, a buffer's bytes,
  and the registry's string-keyed entries (the engine's named tables; mlua
  keeps its own under light userdata and integer keys). Native closures,
  userdata payloads and protos don't change. The VM's own threads (the main
  thread, mlua's ref thread: gcid <= `Base::internal`) are left alone.
- **Newer objects** are written in full and recreated with their gcid, so
  table iteration order (object keys hash by gcid) and every later gcid
  carry on exactly as in the saved game.

Start-up is about 0.6 s of a 16-army game (blueprints, `simInit`, session
setup; the game files are cached per process), which a load pays first.

## What a keyframe holds

| Part | What |
|------|------|
| `heap` | The Lua heap (`lsnapshot.cpp` format 2): header (base, gcid and proto counters, the RNG, the basic types' metatables), every live string by hash (a reference is its hash, so one new string doesn't renumber the rest), protos newer than the base, upvalues, then every object with a gcid in gcid order, each record length-prefixed. Saving only reads the heap and writes the objects in rayon parts |
| `userdata` | The engine userdata newer than the base (entity, weapon, command, platoon... handles: ids; categories; sounds), by gcid; remade with `create_userdata` on load |
| `state` | The engine's own sim state (`keyframe/state.rs`), one postcard blob per app-data type: entities, armies, commands, weapons, projectiles, manipulators, intel, threat, nav, terrain, jobs, the scheduler (threads and start arguments by gcid) |

Not saved: what start-up makes and never changes (blueprint registry,
armor, class tables, mod hooks...), file-backed caches (skeletons, which
are resolved again; clips, saved by path and reloaded, because posing reads
only clips already loaded), the renderer's and the session's own state.
Caches that fill from Lua *are* saved: a fill reads blueprints through Lua,
which can make objects, and a loaded game must miss them where the saved one
would have.

Keyframes are taken at the end of a collection tick (`sim::gc::EVERY`), when
the heap holds only live objects (the collection goes through the keyframe
raw call: mlua's `gc_collect` makes two closures per call), and a load ends
with a collection too.

"Only live objects" needs the collector to finish in one go: a second
collection right after must free nothing, or the load's collection drops
objects the saved game still had. Upstream Luau doesn't promise that for
weak tables, so the vendored collector is patched (`RTS-CORE-ENGINE
EPHEMERON` in `MOHO_PATCHES.md`; `tests/lua_ephemerons.rs` checks it on
random object graphs, `heap_snapshot::contents` before and after a second
collection is how to check a real game).

## Recordings

How to record and play one: [running.md](../guides/running.md#replays). The file is ours, not moho's
`.SCFAReplay` (which the engine can't play): a header, then every applied player command, one line
each (`sim::input::encode`):

```text
# rts-core-engine replay 1
# build 0.1.0+b0eaef28, FAF 3839, Lua 251ef597
# session <hex: the session config: map, armies, teams, seed, AI, the lobby's options>
```

The build line is the engine's commit, the FAF version and a hash of FAF's Lua (the same check as the
network handshake). Lines are flushed as they're written, so a game closed abruptly keeps what it
recorded. Files live at `replays/<profile>/<name>.rtsreplay`, moho's layout for its special files,
so the main menu's **Replay** dialog lists them through `GetSpecialFiles` / `GetSpecialFileInfo`.
Playing one from the menu (`LaunchReplaySession`) hands it to the engine thread in place of a lobby's
launch (`engine::session::Launch`). The viewer is an observer: their commands are dropped, except
game speed. Delete removes the file outright (moho moves it to the recycle bin).

A game whose sim reloaded files changed on disk (`--enable-disk-watch`) can't play back as it was
played: the edited code and blueprints aren't in the recording. The first reload warns and writes
`# reloaded <tick> <file>` (`replay::RELOADED`); loading such a recording (`--replay`, the Replay
dialog) warns and refuses it (`replay::unplayable`).

One process plays any number of sessions. The engine thread makes a fresh engine for each launch
(`main.rs`), and a session ends when the renderer closes it (`SimControl::close`): **Exit to menu**
(`ExitGame`) or **Restart** (`RestartSession`; a replay or a skirmish, not a networked game,
`SessionCanRestart`), which launches the last session again (`Launch::Restart`). The next engine's
first snapshot (`Snapshot::new_session`) clears the renderer's world and each head's scene; textures
stay loaded.

## The timeline

A replay shows a bar of its length with its major moments (FAF's `/lua/ui/game/timeline.lua`, our
`ui::replay`). The sim marks moments in the recording with `Replay.AddEvent(kind, unit)`; engine
hooks record `acu_death`, `experimental_built` and `nuke_launch`. They're written as `# event`
lines, with `# tick` / `# end` lines for the length (`engine::replay`). The renderer reads them when
it launches the replay and hands them to the interface (`Replay.GetEvents`, `Replay.GetLength`).

Clicking the bar seeks there; clicking a marker seeks to 10 s before it with the camera on it
(`Replay.Seek(tick, focus)`, below). The Interface option `replay_timeline` (Off / Main Screen /
Second Screen) picks where it shows.

## Replays: seeking and playing ahead

A replay's keyframes live in a folder per replay under
`install::keyframes_dir()` (`replays/.keyframes` in a checkout), named by
the replay file's bytes and the build (`recorder::dir_for_replay`): another
build's keyframes are never offered. On disk (`keyframe/store.rs`) each is a
file `<tick>-<base>.kf`: *full* (zstd on its own) every 6000 ticks, a *diff*
(zstd patch-from the previous keyframe, long-distance matching) otherwise.
Reading a diff reads its chain back to a full one (at most 10 links at one
a minute: ~140 ms for a 16-army game). Files are written under a temporary
name and renamed, so a reader never sees half of one.

- **Writing.** The engine playing a replay writes a keyframe every
  `every` ticks it reaches that isn't on disk yet
  (`keyframe::recorder::Recorder`): the save on the sim thread (~120 ms late
  in a 16-army game), compression and the write on a writer thread.
- **Playing ahead.** When a replay starts and the option is on, the engine
  thread starts the same binary headless on the replay with
  `--keyframes-out <dir> --background --parent-pid <viewer>`
  (`main.rs`, `RunAhead`): low priority, half the cores
  (`RAYON_NUM_THREADS`), carrying on from the folder's latest keyframe if a
  run got some way before. It ends with the replay, when the player leaves
  the replay, or when the viewer process does. It runs many times faster than
  real time, so keyframes are usually ready well ahead of the player.
- **Seeking.** `Replay.Seek(tick)` (the timeline) restarts the session with
  `Launch::Seek(tick)` when the target is earlier than now, or later with a
  keyframe more than one interval ahead of now; the new engine starts up to
  its base, loads the latest keyframe at or before the target
  (`Recorder::resume`; one from another build or start-up is skipped) and
  plays on flat out to the target. Otherwise it plays on from where it is,
  as before.
- **Interface.** `Replay.GetKeyframes()` (ticks), `Replay.GetBuffered()`
  (`{from, to}` ranges within an interval after the start or a keyframe:
  where a seek is quick) and `Replay.GetPrecompute()`; the timeline draws
  the buffered ranges as a lighter bar, like a video's. The renderer lists
  the folder once a second.
- **Options** (Interface tab): `replay_play_ahead` (on/off),
  `replay_keyframe_every` (30 s, 1 min, 2 min) and `replay_keyframe_budget`
  (512 MB, 2 GB, 8 GB): over the budget, the replays whose keyframes were
  written longest ago lose theirs (`recorder::evict`), never the one being
  watched.

A window must play the same game as a headless run of the replay, object
for object, or a keyframe written by the run-ahead process wouldn't be the
game the viewer reaches: renderer-only work runs with gcids frozen
(`heap_snapshot::FrozenGcids` around snapshot building, which reads
blueprints through mlua's protected calls), and engine set-up that differs
between them (the stall watchdog) makes no objects. `tests/keyframes.rs`
checks both start at the same base and play 300 ticks alike.

## Rules for new engine code

The loaded game must make exactly the objects the saved one would have:

- **No lazy Lua objects after the base.** mlua makes a userdata type's
  metatable on first use (`keyframe::prepare` makes one of each sim type
  up front: add new types to `UserdataKind`); anything else made on first
  use (a registry table, a cached Lua value) belongs in start-up.
- **New sim state goes in `keyframe/state.rs`** (derive
  `serde::Serialize, serde::Deserialize`). Skip a buffer (`#[serde(skip)]`)
  only once the late-game test shows the game plays the same without it:
  movement's avoidance "scratch" carries last tick's pushes into this tick,
  and skipping it parted the checksums on the first tick after a load. Only
  the flow fields, the formation search buffers and the effect expiry index
  (rebuilt on first use) are skipped.
  State holding Lua handles saves them by gcid (`SavedValue`,
  `heap_snapshot::gcid_of`) the way `Scheduler::save` does.
- **Renderer-only or mode-dependent code makes no Lua objects**: run it
  under `FrozenGcids`, or through `heap_snapshot::with_raw_state` instead of
  mlua calls that protect themselves (`exec_raw`, `gc_collect`, `get` on a
  table with a metatable).
- **Lua values held by Rust** (an `LuaTable` in app data, a `RegistryKey`)
  aren't in the heap's reach from Lua: save them by gcid, or the object is
  collected after a load.

## Checking it

`tests/keyframes.rs`: save at a collection tick, load into a fresh engine,
re-save (must be byte-identical), play both on asserting the gcid counter
and the checksum every tick, then compare the two whole keyframes at a later
collection tick. The late-game variant is opt-in:

```sh
cargo test --test keyframes                                                  # 2 AIs, tick 699
cargo test --release --test keyframes late_game -- --ignored --nocapture   # 16 AIs, tick 18000
```

When they part, the test names what differs: objects or strings only one
heap has (`heap_snapshot::contents`), protos (`protos`), and the record
holding the first differing byte (`locate`). The gcid counter usually parts
first; to find the code that allocated differently, trace allocations per
tick stage, then per object: patch `luaE_nextgcid` (`lstate.h`) to call a
hook for one `global_State` that prints the new gcid and the Lua frames
(walk `L->ci` back, `luaG_getline`; nothing allocated), run the first tick
after the load on both engines and diff. Differences found that way so far:
a clip cache read without loading, mlua's lazily made objects (userdata
metatables; its pool of error objects, which every native call draws on:
those now take no gcid, `heap_snapshot::prepare`), uninitialised bytes
(line-info padding, a new thread's first frame), per-VM layouts (the
registry) and hints (`cachedslot`), and "scratch" that wasn't (avoidance),
each fixed in the format or the engine.

## Cost

Saving stops the sim thread for a fraction of a second even in a big game; most of a keyframe is the
Lua heap, then entities, intel and the nav grid. Loading (after start-up to the base) takes a little
longer than saving.
