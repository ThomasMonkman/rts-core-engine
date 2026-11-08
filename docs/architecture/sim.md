# The sim

The sim is the game itself: units, orders, weapons, economy, intel. It runs at a fixed 10 ticks a
second on its own thread, in fixed-point maths, and must give bit-identical results on every machine
([determinism.md](determinism.md)). FAF's sim Lua runs on top of it unchanged: the engine provides
moho's native objects and calls FAF's scripts back at the same moments the real engine does.

This page is the map. Each system's exact rules (formulas, edge cases, what was checked against the
real game and what is inferred) are in the module comments of its file, listed below.

## The tick

`sim::tick::step` runs one tick. The tick number goes up first, so every stage of beat `n` reads
`GetGameTick() == n`, as in the real game. Then, in order:

| Stage | What happens | Code |
|---|---|---|
| Input | player commands due this tick are applied as orders | `api/player_input.rs`, `sim/input.rs` |
| Economy | production, consumption and stalls; economy events; regen | `sim/economy.rs`, `api/economy_event.rs` |
| Threat | the AI threat map; animators (manipulators) step | `sim/threat.rs`, `sim/manipulator.rs` |
| Orders | orders whose target is gone end | `api/orders.rs` |
| Construction | building, upgrades, assisting; silos, teleport, script tasks, transports | `api/construction.rs`, `api/transport.rs`, ... |
| Reclaim | reclaim, capture, sacrifice | `api/reclaim.rs`, `api/capture.rs` |
| Weapons | targeting, aim and the fire clock; beams | `sim/weapon.rs`, `api/weapon.rs`, `api/beam.rs` |
| LuaThreads | FAF's Lua threads due this tick | `sim/scheduler.rs`, `api/threads.rs` |
| Movement | units move, air units fly, submarines dive; attached entities follow their parents | `sim/movement.rs`, `api/movement.rs` |
| Projectiles | flight, collisions, `OnImpact` | `sim/projectile.rs`, `sim/collision.rs` |
| Intel | vision, radar, sonar and the rest are re-stamped; who sees what | `sim/vision.rs` |
| (end of Intel) | the destroy queue: the `OnDestroy` of everything destroyed this tick, first destroyed first (moho's end-of-beat drain) | `api/entity.rs` (`run_destroy_queue`) |
| Sync | the `Sync` table for the interface | `engine/sync.rs` |
| Compact | destroyed entities removed; their effects end | `sim/entity.rs` |
| LuaGc | the Lua garbage collection, at fixed ticks only | `sim/gc.rs` |
| Snapshot | the state handed to the renderer (windowed runs only) | `engine/snapshot/` |

The stage names are the ones in the log's `stage ms` lines and in Tracy.

Movement, projectiles and intel come after the Lua threads, as in moho: `Sim::AdvanceBeat` runs its command dispatch stage, then the script stage, then the motion stage where every entity's motion task lives (units' motion, projectile flight and `OnImpact`, attachments), then the recon refresh. So a thread forked from `OnImpact` or from a death first runs the next tick.

## Patterns every system follows

- **Id order.** Entities, commands and per-system state are kept sorted by entity id (or (unit,
  weapon) and similar keys), and every pass walks them in that order. Nothing depends on hash order
  or thread timing.
- **Parallel compute, serial apply.** A pass that does real work per unit computes in parallel with
  rayon, each item reading only the world and its own state, into a scratch list. Results are then
  applied, and Lua callbacks fired, serially in id order. Each such pass has a test that its parallel
  and serial results match. It only goes parallel above a measured size (`PARALLEL_MIN`), because
  waking the thread pool has a fixed cost ([CONTRIBUTING.md](../../CONTRIBUTING.md)).
- **Native pass first, Lua after.** Systems collect events (`OnFire`, `OnImpact`, `OnStartBuild`,
  ...) during the native pass and dispatch them to FAF's scripts afterwards, in a fixed order.
- **Lua sees ids, not memory.** Script objects carry `_c_object = EntityHandle(id)`, and every native
  method looks the id up.
- **Verified vs inferred.** Module comments say which rules were checked against the real game
  (with the binary address or probe) and which are inferred. The evidence is in
  [verifying-against-the-binary.md](../research/verifying-against-the-binary.md); open questions are
  issues #7–#12.

## Entity storage (`sim/entity.rs`)

Not an ECS: an id-ordered **sparse set** with a **hot/cold split**.

- `dense: Vec<Entity>` holds entities sorted by id; `slot[id]` is the dense index (O(1) lookup).
- Ids are monotonic and **never reused**, so a stale id finds an empty slot instead of a newer
  entity. FAF keys Lua tables by entity id (`UnitData[self.EntityId]`), so reuse would leak script
  state between units.
- `Entity` holds only what per-tick systems and spatial queries scan (id, kind, army, transform,
  health, ...). Cold data (blueprint, bones, mesh, intel, reclaim) is behind `Box<EntityCold>`, and
  unit-only state behind `Option<Box<UnitData>>`. A test caps the hot struct's size.
- Destroying is moho's: `Destroy` only flags the entity (`destroyed`: `BeenDestroyed`, and systems
  skip it) and queues it. At the end of the tick the queue drains in order: each `OnDestroy` runs with
  the entity still attached, then it's detached; a destroy made meanwhile joins the same drain, so
  `OnDestroy` never nests. Then the entities are freed (`gone`: `IsDestroyed`), and `compact`
  removes freed ones with an order-preserving `retain` at the end of a tick (once at least 1/8 are
  freed). So iteration is always in id order with no sorting.

**Why not an ECS:** an ECS makes a system pass faster, but every Lua method call looks an entity up
by id, and that lookup is several times faster in a dense array. Archetype iteration order also
shifts as components change, so every order-sensitive system would need a sort. If a system pass
becomes hot, move its fields into parallel arrays (SoA) rather than adopting an ECS. Iterating the
dense array includes destroyed entities until compaction, which callers must remember (#14).

Units are also indexed in a uniform grid of 32-unit cells (`sim/spatial.rs`). Every move goes
through `EntityWorld::set_position` / `set_transform`, which re-cell the entity, so the index can't
drift. Map props live in their own grid, so unit queries never see them.

## Orders

1. **Issuing.** An `Issue*` call (or a player command) makes one shared command (`sim/command.rs`,
   moho's `SimCommand`), and each unit queues its id. Queues are cleared first unless the order is
   queued (Shift). A command is done once no unit has it queued.
2. **Resolving.** Each tick, `api/orders.rs` ends orders whose target is gone or no longer hostile.
3. **Executing.** The system for the front order's kind acts on it: movement for moves, patrols and
   approaching a target; construction for builds, upgrades, guard and repair; reclaim and capture;
   weapons for attack targets; transports; script tasks (`IssueScript`, e.g. ACU enhancements).
4. **Finishing.** The executing system pops the order when it's done, and the next one starts.

A factory has a second queue for the units it makes (`Commands::factory_queue`): moves and patrols
given to it go there, and each finished unit joins those commands. Formations, path searches spread
over ticks, flow fields for big groups, and pushing between units are all part of movement:
`sim/formation.rs`, `sim/nav.rs` + `sim/hpa.rs`, `sim/flow.rs` and `sim/avoidance.rs`.

## Systems

| System | Files | In short |
|---|---|---|
| Movement | `sim/movement.rs`, `api/movement.rs` | moho's motion model: turn rate, acceleration, braking, reversing; air units' flight and combat states, fuel; submarines diving; dead aircraft falling |
| Pathfinding | `sim/nav.rs`, `sim/hpa.rs` | passability per footprint class (moho's `OCCUPY_MobileCheck`); windowed A* for short trips, a sector graph (HPA*) for long ones |
| Groups | `sim/formation.rs`, `sim/flow.rs` | formation slots from FAF's `formations.lua`, groups travelling as a block; shared flow fields for big plain moves |
| Avoidance | `sim/avoidance.rs` | steering round moving units, pushes between overlapping ones, crowds stopping round a shared goal |
| Landing | `sim/landing.rs` | where idle aircraft set down |
| Weapons | `sim/weapon.rs`, `api/weapon.rs` | target search, turret aim and leading, the fire clock, `OnFire`, muzzle position from the posed bones |
| Projectiles | `sim/projectile.rs`, `sim/collision.rs` | ballistic and homing flight, zig-zag, detonation heights, unit and shield collisions |
| Beams | `api/beam.rs` | collision beams traced along the barrel |
| Construction | `api/construction.rs` | builds, upgrades, assisting, factories guarding factories |
| Reclaim, capture | `api/reclaim.rs`, `api/capture.rs` | reclaim and capture orders, auto-reclaim on patrol |
| Transports | `api/transport.rs`, `api/ferry.rs` | loading onto attach bones, carriers, air staging, ferry routes |
| Economy | `sim/economy.rs`, `api/economy_event.rs` | production, consumption, stalls; `CreateEconomyEvent` |
| Intel | `sim/vision.rs`, `sim/intel.rs`, `engine/snapshot/fog.rs` | coverage grids per sensor, detection, cloak and stealth, jammers; the player's fog of war |
| AI support | `sim/threat.rs`, `api/personality.rs`, `api/platoon.rs`, `api/build.rs` | the threat map, personalities (placeholders), platoons, `CanBuildStructureAt` |
| Armor | `api/armor.rs` | damage multipliers from FAF's `armordefinition.lua` |
| Props | `api/terrain.rs` | the map's props, spawned after `SetupSession` so they reach FAF's reclaim grid |
| Lua threads | `sim/scheduler.rs`, `sim/gc.rs` | FAF's coroutines, and garbage collection at fixed ticks |

## Player commands

Input reaches the sim only as `PlayerCommand`s (`sim/input.rs`): army, order kind, unit ids, target,
Shift flag, formation and facing, all as sim values (entity ids, fixed-point positions), so every
peer decodes the same bits. The renderer (`render/player.rs`), a network peer or a replay sends
them; none of them touches sim state.

- **Timing.** Each command is stamped for `now + INPUT_DELAY` (the lockstep input lag) and applied
  at the start of that tick, in `(tick, client, seq)` order. In multiplayer a tick runs only once
  every client's commands for it have arrived ([networking.md](networking.md)).
- **Checks.** Applying is defensive, as for any network input: only live units of the command's
  army take it, and entity targets must exist.
- **Command sources** (`api/command_source.rs`): each human army's player in army order, then
  observers. `OkayToMessWithArmy` follows moho's rules (the army's own players, else cheats).
- **Sim callbacks.** A command can carry a `SimCallback` (`Func`, `Args`) instead of an order. `Args`
  travel as a canonical data tree (`sim::lua_data::LuaData`) so every peer rebuilds the same table;
  then FAF's `SimCallbacks.lua` `DoCallback` runs.
- **Retargeting.** Dragging a queued order's waypoint sends the command id with a new target; the
  sim checks the sender's army and moves the shared command.
- **Replays.** Commands are also the replay format: one line each (`sim::input::encode`), so a
  recorded game replays bit-identically ([replays.md](replays.md)).

## Testing the sim

- `tests/golden_fight.rs`: a fixed fight's per-tick checksum, against a golden file.
- `tests/scenarios/*.lua`: situations that check the engine and FAF's Lua agree (upgrades, transports,
  formations, ...).
- `tests/player_orders.rs`: player commands, and a recorded run replaying to the same checksum.
- The weapon audit: every weapon in the game against a target ([audits.md](../guides/audits.md#weapon-audit)).

How to run them: [testing.md](../guides/testing.md).
