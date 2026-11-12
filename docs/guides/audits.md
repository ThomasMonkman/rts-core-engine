# Audits

Scripts that find what the engine is missing or does differently from moho, and how to re-run them. Their results live in GitHub issues labelled `tracking`:

| Audit | Results |
|---|---|
| [Engine API surface](#engine-api-surface) and [live stubs](#live-stubs) | #1 (sim), #2 (interface) |
| [Fake implementations](#fake-implementations) | #1, #2 |
| [Moho callbacks](#moho-callbacks) | #3, #4 |
| [Blueprint fields](#blueprint-fields) | #5 |
| [Weapon audit](#weapon-audit) | #6 |
| Verifying against the binary ([method](../research/verifying-against-the-binary.md)) | #7–#12 |

Re-run an audit after a batch of work, and update its issue.

## Live stubs

The quickest check. The typed Lua API marks every placeholder as a stub:

```sh
cargo run -- --dump-api /tmp/api
grep -B1 -A2 'stub' /tmp/api/sim.lua /tmp/api/user.lua
```

In the interface VM, some stubs are overwritten later by Lua (FAF's or the engine's), and the dump can't see that. To check one in a running session, look at the function's source: `[C]` means the Rust stub is still in place.

```sh
cargo run -- --session --no-audio --screenshot /tmp/x.png --screenshot-tick 30 \
  --ui-lua "LOG('SRC ' .. debug.info(rawget(_G, 'GetArmyScore'), 's'))"
```

## Engine API surface

1. **API surface.** `dev/audit/api_dump.lua` dumps every global and every
   `moho.*` method table in both VMs at runtime: the sim through `--script`,
   the interface through `--ui-lua` in a session. `dev/audit/api_audit.py`
   diffs the dumps against FAF's declarations (`../fa/engine/{Sim,User,Core}.lua`
   and the `Sim/`, `User/`, `Core/` class files). Each gap is ranked by its call
   sites in FAF's Lua, split by layer.
2. **Placeholders.** Being registered doesn't mean being implemented:
   - `api/pending.rs` registers 52 sim globals that **raise** when called.
   - `ui/user_api.rs` registers interface globals and methods that **warn
     once and return nil**.

   A temporary build recorded each placeholder closure, and the dump compared
   it with what was live after start-up, so it lists the stubs still in
   place in a running game.
3. **Callbacks.** Names moho calls into script = identifier strings in
   `SupremeCommander.exe` ∩ functions FAF's Lua defines − engine-registered
   APIs (317 names). Each was checked for a string literal in `src/`. The 92 with
   no reference were then read in FAF to see whether moho calls them or FAF
   calls them itself. Win32 names in the binary (`CreateBitmap`, `SetTimer`,
   `UpdateWindow`, ...) were dropped.
4. **Runtime.**
   - A 30-minute 4-AI release game (`dev/scenes/watch/census.lua`).
   - A forced defeat under assassination: `--session` defaults to
     `Victory = 'sandbox'`, which hides the end-of-game path.
   - The `game*.log` files from the recent LAN sessions.


### Re-running it

```sh
cargo run -- --headless --script dev/audit/api_dump.lua --ticks 20 && cp game.log /tmp/sim.log
cargo run -- --session --screenshot /tmp/x.png --screenshot-tick 200 \
  --ui-lua "$(sed "s/rawget(_G, 'APIDUMP_LAYER') or 'SIM'/'UI'/" dev/audit/api_dump.lua)" && cp game.log /tmp/ui.log
python3 dev/audit/api_audit.py /tmp/sim.log /tmp/ui.log
```

`api_audit.py` counts stubs as present. To list live stubs, temporarily make
`pending::register`, `user_api::register_stubs` and `register_method_stubs`
store each closure in a global `__STUBS[name]` (`moho.<class>.<method>` for
methods). The dump then emits a `<stubs>` line marking each entry `STUB`
(still the placeholder) or `real` (replaced). For callbacks, re-run the script
in the moho callbacks audit below, but check for string literals
(`grep -rhoE '"[A-Za-z_]+"' src`) rather than any mention.

## Fake implementations

Engine APIs that are registered as if implemented, but return a constant or ignore their arguments. The API surface audit can't see them, because a definition shadows the placeholder.


```sh
python3 dev/audit/fake_impls.py              # ranked table
python3 dev/audit/fake_impls.py --hide-ok    # without the names judged fine (KNOWN_OK)
python3 dev/audit/fake_impls.py --no-ignores # drop the ignores-args kind
python3 dev/audit/fake_impls.py --tsv        # every column, incl. comment, body, FAF files
```

It uses only the standard library and needs no build. It imports
`api_audit.py` to walk the FAF sources and count call sites.

**What it scans.**

- Lua inside the engine: the `include_str!` files under `src/` and every
  `r#"..."#` chunk in a `.rs` file.
- Rust closures registered through `lua.create_function`,
  `moho::add_method(lua, "<class>", "<Name>", …)`, `add(<class var>, "Name", …)`
  (`ui/controls.rs`) and `g.set` / `raw_set`. It also expands names that a
  `for name in [...]` loop or a `const` array feeds in.
- `#[cfg(test)]` modules are skipped. So are the placeholder loops over
  `USER_GLOBALS`, `CORE_GLOBALS` and `PENDING`.

**Kinds.**

| Kind | Lua | Rust |
|---|---|---|
| `empty` | no statements, or a bare `return` | `Ok(())` / `Ok(LuaNil)` |
| `const` | `return` + literals (nil, bool, number, string, a table of literals, `Vector(0,0,0)`) | `Ok(<literal>)`, `String::new()`, an empty `create_table()` |
| `arg` / `self` | returns an argument unchanged | `Ok(<arg>)`, or `Ok(this)` for chaining |
| `ignores-args` | — | every argument after `lua` is `_`-ignored, but the body does more |

**Columns.**

- `doc`: there is a comment directly above the definition, or above the run
  of one-liners it belongs to, or one inside the body.
- `xN`: the call count is an upper bound. For a method, N FAF classes
  (engine annotations or FAF's own Lua) define that name. For a global, FAF
  defines N-1 functions of its own with that name. For example, `lobby.lua`
  has a local `IsObserver(id)`.
- Calls are split by layer as in `api_audit.py`. Sim-side files outside the
  known sim directories, such as `lua/platoon.lua`, are still counted for
  the user layer.

**Limits.**

- It reads source text and doesn't parse it fully.
- It doesn't catch an accessor that reads a field the engine never fills,
  like `u and u.overchargePaused or false` before the snapshot carried it.
- It doesn't catch a setter that stores a value nothing reads.
- These still need a runtime check: compare the values a getter returns
  across a game.


## Moho callbacks

Moho calls script callbacks by name, so their names are in the binary. The list is every `On*` string in Forged Alliance's `SupremeCommander.exe` that FAF's Lua defines, minus the ones the engine already calls. Where moho's exact trigger is unknown, the guess goes on a "Verify against the binary" issue.

```sh
strings -n 4 "../Supreme Commander Forged Alliance/bin/SupremeCommander.exe" | grep -E '^On[A-Z][A-Za-z]+$' | sort -u > /tmp/exe_on.txt
for n in $(cat /tmp/exe_on.txt); do
  grep -rqE "$n *= *function|function [A-Za-z_.:]*$n\b" ../fa/lua ../fa/units && ! grep -rq "\"$n\"" src/ && echo "$n"
done
```

Check for string literals (`"OnPaused"`), not any mention: a name mentioned only in a comment once hid `OnPaused` from this check.

Callbacks the engine deliberately doesn't call:

- `OnEnableWeapon` / `OnDisableWeapon` from `SetWeaponEnabled`: FAF's
  overcharge calls `SetWeaponEnabled` inside them, so firing them from the
  setter would re-enter. Moho calls them only from overcharge orders
  (`api/overcharge.rs`, done).
- `OnDamageBy`, `OnMotionTurnEventChange`: empty in FAF (the latter removed as
  a "cycle eater").
- `CollisionBeam` `OnCollision`: no FAF class defines it (only trees do).


## Blueprint fields

Bugs here look like "a unit ignores a flag": a weapon fires outside its arc,
a torpedo targets a hovercraft. This audit lists the blueprint fields the real
engine reads that our source never names, from the blueprints as FAF loads
them.


1. `dev/audit/bp_dump.lua` walks every loaded blueprint (`__blueprints`, after
   FAF's blueprint loading has added and changed fields) and logs a `BPFIELD`
   line per field path: kind (unit, projectile, prop, mesh, emitter, ...),
   path (`Weapon[].RateOfFire`), how many blueprints set it, a sample value.
2. `dev/audit/bp_audit.py` sorts each field:
   - **engine field**: moho reflects it. The blueprint type tables hold each
     field's name followed by its description, after the struct's name
     (`RUnitBlueprintWeapon`, `RProjectileBlueprintPhysics`, ...): the report
     shows both. Both binaries have them: MohoEngine.dll is the original
     game's engine, and FA's `SupremeCommander.exe` adds the fields FA
     introduced (`HideLifebars`, `IgnoreIfDisabled`). A name that is only somewhere in the
     binary, not in those tables, is counted but not listed (mostly common
     words inside FAF's own tables: `Scale`, `Offset`, `Radius`).
   - **we read it**: the name is a string literal in `src/` (Rust or our Lua),
     or the Lua name of a `#[derive(LuaTable)]` field (`rename_all`), or a
     field under it is read (a table).
   - **FAF Lua**: FAF's scripts read `.Name` too. That doesn't make it
     handled: `FlattenSkirt` and `DummyWeapon` are engine behaviour FAF's
     scripts also look at.
   - **FAF's annotation**: FAF documents blueprint fields in
     `../fa/engine/Core/Blueprints/*.lua` (`---@field`, with a description):
     shown beside moho's. Fields FAF documents that moho doesn't reflect but
     whose names are in the binary are listed apart, and so are the fields
     moho reflects that FAF doesn't document.
3. A **gap** is an engine field we don't read. It's a name match, not a use:
   read how the engine uses a field in the binary
   ([`docs/research/verifying-against-the-binary.md`](../research/verifying-against-the-binary.md)) before implementing it, and add a
   Confirmed row.

```sh
cargo run -- --headless --ticks 1 --no-ai --no-audio --script dev/audit/bp_dump.lua
python3 dev/audit/bp_audit.py game.log > docs/api/blueprint-fields.md   # docs/api/ is git-ignored; --all also lists the fields we read
```


## Weapon audit

Which FAF weapons fail to engage or hit in our engine. Every weapon-bearing blueprint runs a real
engagement (spawn, attack order, ticks), and the audit reports how far the chain got: `closed`,
`fired`, `hit`, `damaged`. Code: `src/weapon_audit.rs` and `dev/audit/weapon_audit.lua`; golden:
`tests/golden/weapon_audit.txt`; open results: #6.

- **Damage is credited per weapon, never per unit.** A case passes only on damage from its own
  weapon: its projectiles, their fragments and child projectiles, its beams, and threads forked while
  its damage code runs (nuke rings, damage over time). So a unit's other weapons can't pass a weapon
  that never fired. The header of `dev/audit/weapon_audit.lua` has the mechanism.
- **Weapons that aren't attacks** (death weapons, the teleport module, transports' guidance systems,
  dummy target / painter / effect weapons) never engage and get `non-attack`. `unsupported` is for
  real weapons the audit can't set up yet.
- **Every case runs in a session of its own**, alone on the flat audit map `/maps/audit_duel`
  (`tests/vault/maps/audit_duel`; `python3 dev/audit/flat_map.py` regenerates it), with its
  engagement centred on the map. Sessions run on a thread per core (`WEAPON_AUDIT_THREADS` caps it),
  merged in case order, so a case's row is the same alone, in any subset or in the whole census. It
  used to be one shared session, and cases leaked into each other.
- **Bearings.** A turreted weapon is also tested with its target at other bearings round the
  attacker: see [What a bearing sub-case is](#what-a-bearing-sub-case-is) below.
- **Outcomes are data, not errors.** The run exits 0 whenever the census completes. Nonzero means the
  framework broke: a case unresolved by its budget, a Lua failure, or (2) the game data not found.

### Running it

```sh
cargo run --release -- --audit-table                                 # the whole census as one table (the log goes to stderr)
WEAPON_AUDIT_IDS=46,187 cargo run --release -- --audit-table         # a subset: the same rows as in the full table
WEAPON_AUDIT_IDS=531@90,531@-90 cargo run --release -- --audit-table # single bearing sub-cases
WEAPON_AUDIT_BEARINGS=all cargo run --release -- --audit-table > table.txt 2> audit.log   # every 45 degrees
python3 dev/audit/bearing_report.py audit.log --screen              # per-weapon bearing tables, offset and slew screens
```

The table fits the terminal's width, one row per line. Attacker and target use the blueprints' own
names with the bp id, and a short reason token for non-attack / unsupported rows:

```text
  id  attacker                         target                  outcome                 fired  damage
   1  Aeon T... (daa0206) w1 Kamikaze  Aeon T1 P... (uab1101)  pass                        1     350
  34  ... (uaa0104) w5 Weapon Guid...  -                       non-attack (guidance)       0       0
  81  ... (ual0001) w2 Overcharge ...  Aeon T1 P... (uab1101)  no-target                   0       0
 531  Ser (xsb2303) w1 Zthuthaam ...  Seraphim (xsb1101)      pass                        1    2400
 531@180  (xsb2303) w1 Zthuthaam ...  Seraphim (xsb1101)      pass                        1    2400
...
outcomes: fired-no-hit 17, held-no-fire 17, no-target 15, non-attack 183, pass 654, unsupported 4
reasons: non-attack: death (134), dummy (21), teleport (18), guidance (5), not-attack (5)
bearings (gate, 225 weapons): pass 0 213/225, 45 2/3, 90 8/8, ...
bearing misses (17 weapons, outcome per bearing):
  73 uab2303 w1: 0 fired-no-hit, 180 fired-no-hit
```

The golden test pins the census, row for row, including the fired and damage counts, so any change in
behaviour shows. Reproduce a changed row with `--headless --audit-test <id>`:

```sh
cargo test --test weapon_audit                 # the census against the golden
UPDATE_GOLDEN=1 cargo test --test weapon_audit # re-record after an intended change (review the diff)
```

### Watching a case

`--audit-test` runs one case's own session. The audit sets up its own session, so the session
options don't apply. The engagement's spot is printed to the console and the log; zoom there with
`--camera`.

```sh
cargo run --release -- --audit-test 46 --camera 512,512,330     # windowed, in real time until you close it
cargo run --release -- --audit-test 46 --ticks 300              # windowed for 300 ticks
cargo run --release -- --headless --audit-test 46               # the case line and its row, then exit (1: no such id)
cargo run --release -- --headless --audit-test 531@180          # a bearing sub-case, listed or not
```

`dev/audit/watch.sh` plays several cases one after another, each in its own window aimed at its
engagement (closing a window moves on, Ctrl+C stops):

```sh
cargo build --release
dev/audit/watch.sh 201 535 68     # these cases, in order (ZOOM=100 by default)
SPEED=4 dev/audit/watch.sh        # every engaged case in the golden, at 4x
```

### How cases are set up

- **Targets** come from the weapon's `TargetRestrictOnlyAllow` / `TargetRestrictDisallow`: a
  building, land tank, frigate, sub or fighter of the attacker's faction (the fighter first for AA),
  the first on a layer the weapon can reach (`dev/audit/scenario.lua`). Each target carries a vision
  marker, so no spotter is needed.
- **The attacker** stands on a layer the weapon fires from (torpedo launchers on water, a sub's deck
  gun surfaced). One that isn't on its planned layer after spawning is `unsupported (layer)`.
- **Energy and enhancements.** The attacker's army has 10M energy stored, so OverCharge and
  energy-charged weapons can fire. A weapon that comes from an enhancement gets that enhancement (and
  its prerequisites) after spawning.
- **Special weapons:**
  - OverCharge fires through `IssueOverCharge` (no sim effect yet, so those rows are
    `no-target`); AutoOverCharge through `SetAutoOvercharge(true)`.
  - Silo launchers get one missile and fire with `IssueNuke` / `IssueTactical`, the target at 0.9
    MaxRadius capped at 800.
  - Long-range guns (MaxRadius over 600) also get their target at 0.9 MaxRadius, clamped to 800 on
    the 1024 map (the case line says `map clamp`).
  - Tactical missile defence gets a *source* instead of a target: its faction's T2 launcher fires at
    it. Strategic missile defence faces a nuke launcher; anti-torpedo weapons face a torpedo launcher.
    The damage column counts missiles stopped: damaged, intercepted or redirected.
- A case that never damaged its target names its nearest impact in the case line.

### Tracing with the script route

To watch several engagements side by side or trace one, run the audit as a `--script`: a prelude that
sets globals, then `dev/audit/targets.lua`, `scenario.lua` and `weapon_audit.lua` joined.
`WEAPON_AUDIT_CASE` (`n` or `'n@b'`) runs one case on its census layout; `WEAPON_AUDIT_IDS` runs
several in one session on the larger `audit_flat` map, where they can interfere, so their rows aren't
the census's. `WEAPON_AUDIT_TRACE = true` logs `AUDITTRACE` lines every 20 ticks per open case: the
body's turn, the aim manipulator's `GetHeadingPitch`, the weapon's target, shots and credited impacts.

```sh
printf "rawset(_G, 'WEAPON_AUDIT_CASE', '531@180')\nrawset(_G, 'WEAPON_AUDIT_TRACE', true)\n" \
  | cat - dev/audit/targets.lua dev/audit/scenario.lua dev/audit/weapon_audit.lua > /tmp/case.lua
cargo run --release -- --headless --no-ai --fog none --ticks 2000 --script /tmp/case.lua --log case.log \
  --vault tests/vault --map /maps/audit_duel/audit_duel_scenario.lua
```

For `WEAPON_AUDIT_IDS`, use `/maps/audit_flat/audit_flat_scenario.lua` instead. The run needs `--fog none` (the engagement search must see the targets) and logs the tick count its
budgets need.

### What a bearing sub-case is

- **Id**: `id@bearing`, for example `531@180`. The canonical case keeps its plain id and its row is
  unchanged: re-recording the golden added 249 rows and changed none. A weapon's sub-cases follow its
  canonical row in clockwise order (45, 90, 135, 180, -135, -90, -45). The golden stays keyed by
  that first field. A separate golden section would have split a weapon's rows across the file, and
  `id.N` numbering would shift when the bearing set changes. `id@bearing` names the geometry itself:
  `--audit-test 238@-90` runs it alone, and so does any bearing of an eligible weapon, listed or not.
- **Geometry**: the attacker faces +z (heading 0, the way structures are built; FAF's
  `StructureUnit.RotateTowardsEnemy` turns a new weapon structure towards the map centre, which lies
  towards the target on this line, so the audit turns it back to heading 0). The target stands at
  the case's distance on the bearing, direction (sin b, cos b), so +90 is +x. That is the side
  positive `HeadingArcCenter` points: the Fatboy's right turrets have +75 / +120, and the engine's
  arc test adds the centre to the body heading. The line's midpoint is the point nearest the map
  centre, searched in rings 4 apart, where the line fits the case's media.
  - A ship keeps 8 units of water round it, so it isn't on a shore ramp. On a ramp's edge, frigates
    fired 15-17 short at the diagonals.
  - On a shore-crossing line, the start medium must reach to within 12 of the far end, measured
    along the map's axes, as with the canonical strip's first fit. The land is 2 above the water, so
    a flat shot from a ship's low muzzle at a target far inland hits the shore. Sub deck guns fell
    16-18 short at 180 before this rule.
  - Turning the attacker instead, with the target still on +x, was tried first and rejected. It keeps
    the world direction of fire fixed, and the canonical strip's media are fixed with it, but every
    structure passed at every bearing that way.
- **Who gets bearings**: a `Turreted` weapon on a ground or naval unit or structure, 225 weapons.
  Air units circle to attack, so they're left out. So are silo launches, anti-projectile intercepts,
  overcharge, and long range (MaxRadius over 600: a session costs up to 4000 ticks). The weapon's
  arc is `HeadingArcCenter` / `HeadingArcRange` (half-range, unit frame), else `TurretYaw` /
  `TurretYawRange`. A structure only takes bearings inside its arc, since outside it is no-target by
  design. A mobile unit takes them all; outside its arc it has to turn.
- **Sets** (`WEAPON_AUDIT_BEARINGS`):
  - `gate`, the default and the golden's: 180, plus the 45-degree bearing nearest a limited arc's
    centre. 890 cases, 708 sessions.
  - `all`: every 45 degrees. 2216 cases, 2034 sessions, about 6x the census time.
  - `none`, or a list such as `90,135`.

  The gate set came from timing. With 90 / 180 / -90 the census took 68.7 s wall instead of 18.5 s
  (1142 sessions, release, 18 threads), and with 180 alone it took 42 s. Misses that pass at 0 but
  fail at one bearing are spread thin: 2-7 per bearing in the full set (-135 7, -45 6, 135 5, 180 5,
  45 4, -90 3, 90 2). So the gate keeps one bearing for every weapon, the one with the turret's
  longest swing. The weapon-wide misses show at any bearing, and the arc-centre bearing covers the
  side turrets. What the gate set misses (the Fatboy and battleship side guns at ±90, the diagonals)
  is in the full-set results (#6).
- **Evidence**: each `AUDIT` line ends with `aim b= turn= arc= shot= slew= n= lat= long= max=`.
  - `turn`: the body's turn from the order to the verdict.
  - `arc`: whether the target was then inside the weapon's HeadingArc.
  - `shot`: ticks from the order to the first shot.
  - `slew` (structures only): the fewest ticks the turret needs to swing from straight ahead to the
    bearing at `TurretYawSpeed`.
  - `n` / `lat` / `long` / `max`: the case's own projectile impacts up to the verdict. `lat` is the
    mean miss across the fire line (+ towards positive bearings), `long` the mean miss along it
    (+ beyond), `max` the largest miss.

  A pass resolves on its first damage, so passing cases have 1-3 impacts. The old census kept going
  to 3 hits or 6 impacts. It also put its target at 0.7 MaxRadius, not 0.9.
