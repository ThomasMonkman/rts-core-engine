# API surface and Lua runtime: real game vs ours

Differential probe sweep, domain "API census + Lua runtime", run 2026-11-14 with the reruns of 2026-11-15 merged in. Nothing in the engine was changed.

## Probes and logs

| Probe (`dev/probes/maps/`) | What it does | Real-game log (`dev/probes/out/`) | State of the real log |
|---|---|---|---|
| `probe_api_census` | every global, `moho.*` class method, library member, both VMs | `probe_api_census.faf.log` | complete |
| `probe_api_lang` | 91 one-file syntax / semantics cases (`syn_*.lua`), entity ids, `loadstring` | `probe_api_lang.faf.log` | complete |
| `probe_api_spot` | ~840 sim and ~400 interface pcall'd cases: Lua runtime, core functions with odd arguments, live objects, blueprint dumps | `probe_api_spot.faf.log` | complete on the rerun: sim to `PROBE SIM end`, interface to `risky.Utf8Sub4`, where the real game dies (see "Calls that take the real game down"). The first run was cut off after `unit.weapons` (sim) and `ui.itemlist` (interface) |
| `probe_api_arity` | calls every C function with 12 nils so the engine's count check prints its arity | `probe_api_arity.faf.log` | complete on the rerun: sim 748 functions, interface 447, `PROBE done`. The first run stopped after `Rect` |

Our side was produced by running the same probes in ours; those logs and diffs are not kept in the repo.

How ours was run (matters, see H1): as a real map through the vault mount, so the probe goes through the same loader as FAF's Lua:
`target/release/rts-core-engine --session --player 1 --no-audio --vault dev/probes --map /maps/probe_api_spot/probe_api_spot_scenario.lua --ticks 400 --ui-lua "import('/maps/probe_api_spot/ui_probe.lua')"`.

### Harness findings (affect every probe)

| # | Finding | Evidence | State |
|---|---|---|---|
| <a id="h1"></a>H1 | `--script` loads the file with a raw `lua.load`, skipping `lua_translation` (`src/engine/session.rs:263-267`). A probe run that way is plain Luau: `for k, v in t` over a table whose metatable has `__call` (every moho class table) calls it forever, `for k in nil` raises, `!=`/`arg` don't work. A first run hung in `for k, v in getmetatable(navigator)` until the watchdog. `docs/guides/probing.md` tells people to use `--script`; the `--vault dev/probes --map` form above avoids it | ours: `tbl.forin_nil => ERR attempt to iterate over a nil value` under `--script`, `ok 0` through the map | open (`--script` still loads raw) |
| <a id="h2"></a>H2 | The real game dies (or the thread stops) on some bad-argument calls even under pcall. First spot run died in the interface at `STR_Utf8SubString` with out-of-range arguments (one of `('hello',0,2)`, `(4,10)`, `(2,0)`, `(10,1)`, `(-1,2)`); the second at `ItemList:GetItem(5)` / `DeleteItem(5)` / `SetSelection(5)` on a one-item list | log ends at `core.STR_Utf8Len` (run 1), `ui.itemlist` (run 2) | open (docs/guides/probing.md not updated) |
| <a id="h3"></a>H3 | Lua 5.0 can't yield across pcall: a probe body that waits must not be wrapped in pcall in the real game | `co.yield_pcall => ok true false false "cannot resume dead coroutine"` | open |
| <a id="h4"></a>H4 | `--ui-lua` code also skips `lua_translation` (raw `lua.load`, `src/render/frame.rs:1327`); an interface probe passed as a string there is plain Luau. Use `import(...)` of a vault file, as this sweep does | rerun | fixed `5d05759` |

## Present but behaves differently

Severity: **A** = raises or goes wrong in a path FAF's Lua takes; **B** = wrong value; **C** = cosmetic / only on error paths / ours is laxer. Confidence is for "the real game does this"; the source column is where ours decides it.

### Lua language and runtime

| # | Real game (evidence) | Ours | Probe | Sev | Conf | Our source | State |
|---|---|---|---|---|---|---|---|
| <a id="l1"></a>L1 | `^` is integer XOR, not power. `num.pow => ok 2 4294967296.000 4294967296.000 0` for `2^0.5, 2^-1, (-8)^(1/3), 0^0`; `2 ^ 3 ^ 2` = 3, `-2 ^ 2` = 0 (`syn.concat_prec`). Power is `math.pow` (`math.pow => ok 1024 1.414 nan`) and the global `__pow` (`base.pow_fn => ok 8`). FAF's own `DebugFunction.lua:367` says `OPCODE.POW ... also BXOR` | `^` is Luau power: `1.414 0.5 nan 1`. FAF uses `x ^ 0` as "truncate to integer": `fa/lua/sim/NavUtils.lua:443` and `:490`, `for k = 1, (0.5 * head) ^ 0 do` (reversing the path). Ours gives 1 for any `head`, so only the first and last entries are swapped: confirmed with a one-off run (`head = 9` loops once; real loops 4 times) | spot, lang | **A** | high | no rewrite of `^` in `src/engine/lua_translation/` (`bitwise.rs` handles `~ \| & << >>` only) | fixed `af857a4` |
| <a id="l2"></a>L2 | Closures share the loop variable. `syn.loopclosure => {3 3 3}` (numeric for), `syn.loopclosure_in => {}` (generic for: all nil after the loop) | fresh variable per iteration: `{1 2 3}`, `{10 20 30}`. FAF code that captures a loop variable in a closure or `ForkThread` body that runs after the loop behaves "correctly" in ours and differently in the real game | lang | B | high | Luau semantics; nothing in `lua_translation/` | open |
| <a id="l3"></a>L3 | `for k, v in nil do` raises: `tbl.forin_nil => ERR ... loop over expected but got nil`. (Issue #12 item "is an empty loop": **not so**.) Indexing nil is fine (`tbl.index_nil => ok`) | empty loop | spot | C (ours laxer; a real-game error path continues in ours) | high | `src/engine/lua/shims/lua50_aliases.lua:15-18` | open |
| <a id="l4"></a>L4 | `table.getn` honours an `n` field and `table.setn`: `tbl.getn1 => 2 0 0 5` for `{1,2,nil,4}`, `{nil,nil,3}`, `{}`, `{n=5}`; `tbl.setn => 3`; `table.insert` on `{n=0}` bumps `n` (`tbl.insert_n => 1 "a"`); `insert(t, 5, x)` on 2 items gives getn 5; `table.remove(t, 5)` on 3 items shrinks to 2 (`tbl.remove3 => nil 2`); `{1,nil,3}` has getn 1 (`syn.tablenil`) | `4 3 0 0`; `setn` is a no-op; `n` field ignored; sizes 2 / 3 / 3 | spot, lang | B | high | `lua50_aliases.lua:5-6` (setn no-op); `getn` is Luau's | open |
| <a id="l5"></a>L5 | `string.gsub` with a function that returns nil / false / a number deletes the match: `str.gsub5 => "ac" 1`, `gsub6 => "ac"`, `gsub12 => "ac"` | keeps the match / inserts the number: `"abc"`, `"abc"`, `"a7c"` | spot | B | high | Luau's gsub | open |
| <a id="l6"></a>L6 | `string.format` rounds halves away from zero: `fmt.f => "  2.3" "1" "2" "3"` for `%5.1f` 2.25, `%.0f` 0.5 / 1.5 / 2.5. Exponents have 3 digits (`1e+020`); `%x` of -1 is `ffffffff`; `%s` of nil raises | `"  2.2" "0" "2" "2"`; `1e+20`; 16 f's; `%s` nil prints "nil". Interface numbers at exact halves print one lower | spot | B (display) | high | Luau's format | open |
| <a id="l7"></a>L7 | Lua numbers are float32: `tostring(0.1)` = `0.10000000149012`, `16777217` prints `16777216`, `0.1+0.2 == 0.3` true, `for x = 0, 1, 0.1` runs 10 times and `for x = 0, 0.3, 0.1` 4 times (`syn.loopfloat => {10 4 10 10}`), `math.pi` = 3.1415927410 | doubles: 11 and 3 iterations, etc. Already in the Confirmed table ("Lua numbers are 32-bit floats ... Not changed yet"); the loop counts are new evidence of where it shows | spot, lang | B | high | known | open |
| <a id="l8"></a>L8 | Assigning the numeric-for variable changes the iteration: `syn.loopvar_assign => 3` | 5 | lang | C | high | Luau | open |
| <a id="l9"></a>L9 | No string-to-number coercion in arithmetic: `'10' + 5` raises `perform arithmetic on expected but got string`; engine functions also reject numeric strings (`Vector('1','2','3')`, `VDist2('0',..)`, `MATH_IRound('3.6')`, `GetTerrainHeight('10','10')`: `number expected but got string`) | coerces everywhere | spot | C (laxer) | high | Luau / mlua conversions | open |
| <a id="l10"></a>L10 | pcall turns every error into a string with a traceback appended: `err.tbl => ok false "string"` for `error({code=5})`; `err.num => "42"` (no position). `assert(1,2,3)` returns only `1` | error tables survive (`"table" 5`); messages have no traceback; assert returns all | spot | C | high | Luau | open |
| <a id="l11"></a>L11 | Proper tail calls (`syn.tailcall => {true 0}` at depth 1e6); plain recursion fails before depth 5000 | no tail calls: `stack overflow`; depth 5000 fine | lang | C | high | Luau | open |
| <a id="l12"></a>L12 | Nested long brackets work: `r = [[a [[b]] c]]` gives `"a [[b]] c"`, and `--[[ a [[ b ]] c ]]` is one comment | the file fails to compile, silently (`<unset>`). Any FAF or mod file with a nested long string / comment doesn't load | lang | B | high | not handled in `src/engine/lua_translation/` | open |
| <a id="l13"></a>L13 | Bit operators bind tighter than arithmetic: `1 \| 2 & 3 + 4` = 7, `1 + 2 \| 4 * 2` = 13; `1 << 32` = 1; `4294967295 \| 0` = 0; a string operand raises | 3, 11, 0, 4294967295, coerced | lang | B (rare in FAF: footprints use plain `A \| B`) | high | `src/engine/lua_translation/bitwise.rs` | open |
| <a id="l14"></a>L14 | `type(print)` is `"cfunction"`; `tostring(print)` starts `cfunction:`. FAF checks both names (`class.lua:453`, `config.lua:70`). `ForkThread(math.abs)` raises `first argument isn't a function`; `coroutine.create(cfunction)` raises | `"function"`; both accepted | census, spot | C | high | Luau | open |
| <a id="l15"></a>L15 | `tonumber('0x10')` nil, `tonumber('inf')` / `('nan')` nil, `tonumber(' 0x1F ')` nil | 16, inf, nan, 31 | spot | C | high | Luau | open |
| <a id="l16"></a>L16 | `table.concat` rejects numbers (`table contains non-strings`); `unpack(t, i, j)` ignores `i, j`; `unpack({1,nil,3})` stops at the hole; `table.foreachi` stops at a hole; `string.byte(s, 1, 3)` returns one value; `gsub` replacement `%0` / table / number raise, a trailing `%` is kept | all accepted / wider; `foreachi` on a holed list raises; trailing `%` raises | spot | C | high | Luau | open |
| <a id="l17"></a>L17 | `doscript` of a missing file raises `Unable to find file ...`, so `import` of a missing module raises (`core.import_bad => ERR ... Error importing '/lua/nosuch_file.lua'`). Relative, backslash and `..` paths also fail to import | `doscript` returns silently and `import` hands back an empty module table: `ok {__moduleinfo=... }`. `pcall(import, optionalFile)` succeeds where the real game takes the fallback. Relative paths import fine | spot, lang | **A** | high | `src/engine/api/dofile_or_script.rs:23-42` ("A missing file does nothing") | fixed `ae86c4a` |
| <a id="l18"></a>L18 | `doscript` returns nil whatever the chunk returns | returns the chunk's value | lang | C | high | same file | open |
| <a id="l19"></a>L19 | Arithmetic metamethods aren't tried from the right operand when the left is a number: `2 * Vector(1,2,3)` raises at the call site, `perform arithmetic on expected but got table` (`core.Vector_nummul`), and `meta.arith` (which includes `1 + t`) raises too. `Vector * 2`, `Vector + Vector`, `-Vector` work | `2 * Vector` reaches FAF's `__mul` (which then raises its own `invalid argument for vector multiplication`); `1 + t` calls `__add` | spot | C | medium (the `meta.arith` case mixes four operators; see Could not test) | Luau | open: needs a VM patch, low value |

### Core engine functions (both VMs)

| # | Real game (evidence) | Ours | Probe | Sev | Conf | Our source | State |
|---|---|---|---|---|---|---|---|
| <a id="c1"></a>C1 | `STR_GetTokens` returns a **0-based** table: `core.STR_GetTokens => ok {[0]="a" [1]="b" [2]="c" }`; an empty delimiter gives the whole string | 1-based `{[1]="a" [2]="b" [3]="c"}`; an empty delimiter raises `malformed pattern`. FAF iterates the result with `for i, node in` (`AI/aiutilities.lua:427`, `AI/aiattackutilities.lua:1191/1266/1370`, `ui/maputil.lua:429`), so contents agree but `getn` / `ipairs` / the order `next` visits differ | spot | B | high | `src/engine/lua/shims/string_utils.lua:7-18` | open |
| <a id="c2"></a>C2 | `OrientFromDir(Vector(1,2,3))` = `{-0.275 0.154 0.045 0.948}` | sim: `{-0.428 0.144 0.069 0.890}`. Axis directions match (`(1,0,0)`, `(0,0,1)`, `(0,1,0)`, zero). Interface: still a stub | spot | B | high | `src/engine/api/entity.rs:1051-1052`; UI stub `src/ui/user_api.rs:313` | open |
| <a id="c3"></a>C3 | `VPerpDot((1,2,3),(4,5,6))` = -6 (`a.x*b.z - a.z*b.x`) | +6 | spot | B (no FAF call site) | high | `src/engine/lua/shims/vector_math.lua:50-52` | open |
| <a id="c4"></a>C4 | `STR_itox(255)` = `"FF"`, `(-1)` = `"FFFFFFFF"` (upper case; issue #12 guessed lower); `STR_xtoi('ffffffff')` = -1, `('FFg')` = 4080, `(' 1f')` = 0 | lower case; 4294967295; 255; 31 | spot | C | high | `string_utils.lua` | open |
| <a id="c5"></a>C5 | `ParseEntityCategory`: space and `*` intersect, `,` unions (`'LAND,TECH1'` = 386), `+` and `-` are not operators (`'COMMAND + SUBCOMMANDER'` = 0, `'ALLUNITS - LAND'` = 291 = LAND, `'(COMMAND + SUBCOMMANDER) - UEF'` = 250 = UEF); ids are case-sensitive (`'UEL0201'` = 0) | `+` unions and `-` subtracts (33, 384, 25); `'UEL0201'` = 1 | spot | C (superset, but a different set for the same string) | high | `src/engine/api/categories.rs` | open |
| <a id="c6"></a>C6 | Categories cover projectile and prop blueprints too: `categories` has 1877 keys (218 upper-case names + 1635 ids: 675 units, 395 projectiles, 565 props); `EntityCategoryGetUnitList(categories.PROJECTILE)` = 288, `MISSILE` 38, `TORPEDO` 28, `UEF` 250, `RECLAIMABLE` 1093 | 917 keys (unit ids only); 0, 0, 0, 181, 574. Unit-only counts (ALLUNITS 675, LAND, AIR, TECH1/2, COMMAND...) match; TECH3 177 vs 173 and WALL 18 vs 15 differ by the non-unit entries | census, spot | B | high | `src/engine/api/categories.rs:63` | open |
| <a id="c7"></a>C7 | `__blueprints` has no emitter blueprints (5334 string keys: 675 unit, 395 proj, 565 prop, 3695 mesh, 4 other) and its numeric part is 0-based (`type(__blueprints[0])` = table, 1..5333); blueprint tables and their sub-tables have a metatable; `__blueprints.UEL0201` is nil | 8277 string keys (2943 emitters extra), numeric part 1-based, no metatables, upper-case ids resolve | spot | C | high | `src/engine/api/blueprints.rs` | open |
| <a id="c8"></a>C8 | `GetUnitBlueprintByName('uel0201') == __blueprints.uel0201` is **false**, and so is `unit:GetBlueprint() == __blueprints.uel0201` and `unit.Blueprint == ...` | all true (one shared table) | spot | C | high | same | open |
| <a id="c9"></a>C9 | `DiskGetFileInfo` fields: `IsFolder ReadOnly SizeBytes TimeStamp WriteTime`; a folder, a relative path, a backslash path and a trailing slash all give `false` | `IsFolder IsReadOnly SizeBytes` (wrong name, two fields missing); all four give a table | spot | B | high | `src/engine/api/disk_get_file_info.rs:21-37` | open |
| <a id="c10"></a>C10 | `Basename` needs exactly 2 arguments (`expected 2 args, but got 1`); `Dirname('/c.lua')` = `""`; `Dirname('')` and `FileCollapsePath('')` raise | 1 argument fine; `"/"`; empty strings | spot | C | high | `string_utils.lua:20-28` | open |
| <a id="c11"></a>C11 | `Vector`: 4 arguments raise, `v.foo` raises `'x', 'y', or 'z' expected`, `v.x = 9` leaves `v[1]` and `v.x` at 1; `Rect()` raises; `Rect` and `categories` have an (empty) metatable; `VDist3` reads `[1..3]` of plain tables, missing entries as 0 (`VDist3({0,0},{1,2})` = 2.236) and checks its count | 4 arguments accepted, `v.foo` nil, `v.x = 9` writes through, `Rect()` = `{}`, no metatables, `VDist3` raises on short tables and returns 0 with no arguments | spot | C | high | `src/engine/lua/shims/vector_math.lua` | open |
| <a id="c12"></a>C12 | `MATH_Lerp` with 4 arguments returns nothing | raises in the shim | spot | C | high | `[engine MATH_Lerp]` shim | open |
| <a id="c13"></a>C13 | `EntityCategoryFilterDown(cat, nil)` raises `Pass in invalid table of units`; `GetEntityById()` / `IsDestroyed()` / `KillThread()` / `IssueMove()` check their counts; `Sound({})` raises | `{}`, nil, `true`, nil, a command, a sound | spot | C (laxer) | high | typed API conversions | open |
| <a id="c14"></a>C14 | Entity ids encode the army: units `army_index << 20 \| n` (`a1:1 a1:2 a2:1048577 a3:2097153`), projectiles from `268435456` (0x10000000), props around `804257792` (0x2FF00000+) | one global counter: `5414, 5416, 5417...`, props `0..5413` | lang | C (nothing in FAF's Lua decodes them; mods might) | high | entity id allocation | open |
| <a id="c15"></a>C15 | `GetUnitById('nosuch')` returns a unit (id parsed as 0); `GetCurrentCommandSource()` in a sim thread is a number | nil; nil | spot | C | medium | `src/engine/api/command_source.rs:39` | open |

### Sim objects

| # | Real game (evidence) | Ours | Probe | Sev | Conf | Our source | State |
|---|---|---|---|---|---|---|---|
| <a id="s1"></a>S1 | `unit:GetStat('X')` with no default returns nil for a missing stat; with a default the table is `{Name, Type, Value}` (`unit.stats => ok nil {Name="NOSUCHSTAT" Type="Integer" Value=0 }`) | `{Value=0}` in both cases | spot | B | high | `src/engine/api/unit_stats.rs` | open |
| <a id="s2"></a>S2 | Issue* functions return a table with a metatable for a real order and **nothing** for an empty list; `IssueClearCommands` returns nothing | userdata always (also for `IssueMove({}, pos)` and `IssueClearCommands`) | spot | C | high | `src/engine/api/orders.rs` | open |
| <a id="s3"></a>S3 | Bad bone names / indexes raise (`GetPosition('NoSuchBone')`: `Invalid bone name`; `GetPosition(-1)`: `Invalid bone index`; `GetBoneDirection('Nope')`); `IsUnitState('NoSuchState')` raises listing the valid names | return the entity position / a direction / false | spot | C (laxer) | high | `src/engine/api/entity.rs`, `unit.rs` | open |
| <a id="s4"></a>S4 | `GetTerrainType` out of bounds is `Default` | `Water03` | spot | C | medium | `src/engine/api/terrain.rs` | open |
| <a id="s5"></a>S5 | A freshly spawned idle tank: `IsUnitState('Moving')` true, `IsIdleState()` false, 3 ticks after `CreateUnitHPR` (`unit.state => ok true false false`) | false / false / true | spot | B? (see units.md row 13) | medium | - | partly fixed `bf142e0` (`Guarding`, `Patrolling`, killed unit `NoCost`); the idle-tank states open |
| <a id="s6"></a>S6 | `Unit:CanPathTo` returns a **second value**, a position: `unit.canpath => ok true {[1]=492.500 [2]=18.764 [3]=512.500}` (goal 10 east), `unit.canpath_far => ok false {[1]=482 [2]=18.571 [3]=512}`. **Contradicts the Confirmed table** ("one boolean back ... FAF's AI reads a second value; there is none") | `true` / `false` only | spot | B | high | `src/engine/api/unit.rs:850`. FAF reads it: `AI/aiutilities.lua:1434/1438` (`result, bestGoal = unit:CanPathTo(...)`) | fixed `0161804` |
| <a id="s7"></a>S7 | Blueprints carry the engine's filled-in defaults: uel0201 428 fields vs ours 274 (uea0102 507/321, ueb0101 405/286, uel0105 374/265, ues0103 574/382, projectile tdfgauss01 139/36). Missing in ours, e.g. `AI.GuardReturnRadius=50`, `AI.GuardFormationName`, `AI.TargetBones={}`, all `Air.*` (`CanFly=false`, `Winged`, `KLift`...), `Economy.StorageEnergy/StorageMass/InitialRallyX/Z/BuildableCategory`, `General.UpgradesTo=""`, `SeedUnit`, `QuickSelectPriority`, `Weapon.N.MinRadius/HeadingArcCenter/HeadingArcRange/EffectiveRadius/TargetRestrictOnlyAllow/...`, `ScriptClass`, `ScriptModule`, `Selection*`, `InertiaTensor*`, `BlueprintOrdinal`; projectiles' `Physics.*`, `Footprint.*`, `Display.Mesh.*`. Values: `Weapon.N.ProjectileId` is lower-cased, `Display.Mesh.LODs.N.*Name` are full paths, `Physics.CatchUpAcc` 3.4 (derived) | the 62 defaults ours fills by design; ids keep their case; LOD names relative; `CatchUpAcc` 0 | spot (BP) | B | high | `src/engine/api/blueprints.rs:137` (`UNIT_DEFAULTS`; the doc comment above lists the deliberate omissions). `CatchUpAcc` is never read (Confirmed table) | fixed `18d8514` for `Air`, `AI`, `Economy`, `Weapon`; left out: `StagingPlatformScanRadius`, `MaxBrake`, footprint zeros, projectile fields |
| <a id="s8"></a>S8 | `GetHighestThreatPosition(1, true, 'Overall')` (no threat on the map): first value is a number (`brain.threat2 => ok "table" "number"`) | a table | spot | B? | low (only `type()` of the first value was logged) | `aibrain_methods`; FAF `ScenarioPlatoonAI.lua:103` reads `location, threat`. Needs a probe logging both values | closed: no change (the first value is a table) |
| <a id="s9"></a>S9 | `EntityCategoryFilterDown` drops a destroyed unit: `misc.after_destroy8 => {[1]=<u1>}` | keeps it (`[1]=eng Dead=true, [2]=u1`) | spot | C | high | `src/engine/api/categories.rs:229` | open |
| <a id="s10"></a>S10 | `brain:GetArmyStat('NOSUCH', 3)` = `{Name="NOSUCH" Type="Integer" Value=0}` (default ignored), same as `unit:GetStat` (S1) | `{Value=3}` | spot | C | high | aibrain stats | open |
| <a id="s11"></a>S11 | Error paths that raise in the real game: `GetCargo` on a non-transport, `TestCommandCaps('RULEUCC_NoSuch')` (enum list), `CanBuild('nosuch')`, `SetMaxHealth('500')`, `PlatoonExists(nil)`, `GetSquadUnits('NoSuchSquad')`, `CreateRotator` with a bad bone or axis `'q'`, `CreateAttachedEmitter` bad bone, `SetEmitterParam('NOSUCHPARAM')`, `DrawCircle(.., 'nosuchcolor')`, `CreateFootPlantController` count, any manipulator method after `Destroy`, `unit:GetShieldRatio()` and `platoon:IsCommandsActive()` without an argument (both take one) | all accepted | spot | C (laxer) | high | typed conversions in `src/engine/api/` | open |
| <a id="s12"></a>S12 | Ours **stricter** on error paths: `GetScriptBit(99)` is `false`; `CalculatePlatoonThreat('NoSuch', cat)` returns the `Overall` value (9.51) | raises `invalid script bit`; raises `unknown threat type` | spot | C | high | `unit.rs` script bits; platoon threat | open |
| <a id="s13"></a>S13 | Return shapes: empty squad `GetSquadUnits('Support')` returns nothing; `MoveToLocation` returns a table; `CreateTrail` with an emitter blueprint, `CreateEmitterAtEntity` with a missing blueprint and `CreateLightParticle` return nothing; `SetThrustingParam` returns nothing; `Rotator:ClearGoal` doesn't return self, `SetPrecedence` does; manipulator `Destroy` returns nothing; `CreatePrefetchSet` is userdata; `u:GetPosition() ~= u:GetPosition()` is false; `GetCurrentTargetPosition` with no target is `{0,0,0}`; `GetBlip(own army)` is nil; command-queue entries have `_c_object` | `{}`, userdata, objects, self, nil, `nil nil 1`, a table, true, the projectile's position, a blip, no `_c_object` | spot | C | high | `platoon.rs:235`, `orders.rs`, effects / manipulator bindings | open |

### Interface (ui.md goes deeper)

| # | Real game (evidence) | Ours | Probe | Sev | Conf | Our source | State |
|---|---|---|---|---|---|---|---|
| <a id="u1"></a>U1 | `GetTextureDimensions('/textures/ui/common/game/economic-overlay/econ_bmp_m.dds')` = `2 30`, and a bitmap with it has `BitmapWidth/Height` 2 x 30; a missing texture returns nothing; a bitmap with no texture is 2 x 2 | `4 32` (and bitmap 4 x 32); `0 0`; 0 x 0. Layouts sized from bitmaps are off | spot | B | high | texture size path behind `GetTextureDimensions` in `src/ui/` | fixed `53750b1` |
| <a id="u2"></a>U2 | Text metrics are integers and narrower: Arial 14, `GetStringAdvance('Hello World')` 72, ascent 13, descent 3, external leading 0; `'iiii'` 12, `'MMMM'` 44 | 80.67, 14.18, 3.32, 0.51; 13.92, 52.19 | spot | B | high | `src/ui/` text metrics | fixed `7a55a72` |
| <a id="u3"></a>U3 | Key code conversion works: `ui.keys => ok 0 65 13 342` for `KeycodeMSWToMaui(65)`, `KeycodeMauiToMSW(65)`, `KeycodeMSWToMaui(13)`, `KeycodeMSWToMaui(112)`; `ui.keys2 => ok 27 316 36 27` for `MSWToMaui(27)`, `MSWToMaui(37)`, `MauiToMSW(315)`, `MauiToMSW(27)` | stubs: return nothing | spot | B | high | `src/ui/user_api.rs` (stub list) | open |
| <a id="u4"></a>U4 | `SessionIsBeingRecorded()` true, `SessionIsObservingAllowed()` false, in a single-player session | false / true (constants) | spot | C | high | `src/ui/user_api.rs:1138-1139` | open |
| <a id="u5"></a>U5 | New control: `GetName()` = `"group"`, `GetRenderPass()` = 8 | `""`, 0 | spot | C | medium | `src/ui/controls.rs` | open |
| <a id="u6"></a>U6 | `SetSolidColor('nosuchcolor')` and `('ff00')` raise | accepted | spot | C | high | `src/ui/controls.rs` | open |
| <a id="u7"></a>U7 | `GetResourceSharing` takes 0 arguments; `IsAlly(1, 99)` raises `Invalid army`; `GetFrame()` raises, `GetFrame(5)` is nil; `SimCallback()` raises | all accepted; `GetFrame(5)` returns a frame | spot | C | high | `src/ui/user_api.rs` | open |
| <a id="u8"></a>U8 | `GetSessionClients()[1]` has `maxSP`, `ping` (and more, line truncated); `SessionGetScenarioInfo()` has `PlayableAreaWidth/Height`, `PlayableRect` and none of the sim-side keys | no `maxSP` / `ping` (has `uid`); carries the sim's `ArmySetup`, `BuilderTable`, `Env`, `MapData`...; army entries have an extra `armyIndex` | spot | C | medium (key lists truncated at 110 chars) | `src/ui/` session tables | open |
| <a id="u9"></a>U9 | `GetIdleEngineers()` returned one unit when only the commander existed | nil (`ENGINEER - COMMAND`) | spot | C | low (unit identity not logged) | `src/ui/user_api.rs:969` | open |
| <a id="u10"></a>U10 | `GetMouseScreenPos()` has a metatable | none | spot | C | high | - | open |

### Stubs: what the real ones return (ours raise "not implemented")

| API | Real result | Probe line |
|---|---|---|
| `PointVector(1,2,3,0,0,1)` | `{px=1 py=2 pz=3 vx=0 vy=0 vz=1}`, empty metatable | `core.PointVector` |
| `AITarget()` | table with `Type` (string) | `core.AITarget_noarg` |
| `EntityCategoryEmpty(cat)` | `false` for LAND, `true` for an empty product; works in both VMs | `core.catempty` |
| `GetEntitiesInRect(Rect)` | table (15 entities in a 4x4 rect round a tank); arity 1..4 | `sim.GetEntitiesInRect`, arity |
| `GetBlueprint(unit)` (global) | a blueprint table that is not `__blueprints[id]` | `sim.GetBlueprint` |
| `GenerateRandomOrientation()` | 4-number table | `sim.GenerateRandomOrientation` |
| `SelectedUnit()` | nothing | `sim.SelectedUnit` |
| `SecondsPerTick()` (interface) | `0.1` | `ui.SecondsPerTick` |
| `OrientFromDir` (interface) | as the sim's | `ui.OrientFromDir` |
| `KeycodeMSWToMaui` / `KeycodeMauiToMSW` | see U3 | `ui.keys` |
| `EntityCategoryCountAroundPosition` | takes 4 arguments (ours declares 2) | arity |
| `CoordinateAttacks` | takes 1 argument (ours 0) | arity |
| `PlayLoop` | takes 1 argument (ours 2) | arity |

Real results of methods ours leaves "not implemented" (all "no FAF script uses it"; mods may): `unit:IsValidTarget()` true, `GetCurrentMoveLocation()` a position, `HasValidTeleportDest()` false, `GetScale()` 1 1 1, `ReachedMaxShooters()` false, `HasMeleeSpaceAroundTarget(u)` true, `PrintCommandQueue()` returns the unit, `AlterArmor`/`RecoilImpulse`/`AddWorldImpulse` nothing, `SetIsValidTarget` nil; `navigator:AtGoal` etc. (`unit.nav_calls => "table" "table" true 0`); `weapon:ChangeDamageRadius/ChangeDamageType` nothing; `brain:FindUnit` false, `FindClosestArmyWithBase` nil, `SetGreaterOf` works; platoon `GetFactionIndex` 4, `GetAIPlan` `""`, `GetPersonality` a table, `GetPlatoonLifetimeStats` a number, `GetFerryBeacons` a table, `CanConsiderFormingPlatoon` false; `projectile:SetVelocityRandomUpVector` nothing; `AimManipulator:OnTarget` false; `SlaveManipulator:SetMaxRate` works. Method lists (`OBJ.*`) otherwise agree apart from M6 and the "ours has" extras.

Not differences: `IsMoving` / `GetVelocity` right after `IssueMove` (real hasn't started, ours has: the intended shorter latency); `unit.adjust` / `sethealth_big` 300 vs 500 (ours had accepted `SetMaxHealth('500')` the case before); `GetFactionIndex` 4 vs 1 and entity ids (launch, C14); projectile speed 1.176 vs 1.151 (weapons domain).

### Arity (complete)

Fixed in `95ed7aa`: `CanBuildPlatoon` takes its factories as optional, and the stub signatures of `CoordinateAttacks`, `EntityCategoryCountAroundPosition`, `GetEntitiesInRect`, `PlayLoop`, `StopLoop` and `SetAlliedVictory` take the real counts. `CAiAttacker:IsWithinAttackRange` and the rest are unchanged.

Real: sim 748 C functions (670 printed an arity), interface 447 (415). Against ours' `--dump-api` declarations: sim 16 stricter, 7 missing, 53 looser; interface 6 stricter, 20 missing, 66 looser. Each "stricter" one was then called in ours with the real minimum:

| Sev | Function | Real | Ours declares | Ours at runtime |
|---|---|---|---|---|
| raises | `aibrain:CanBuildPlatoon(template[, factories])` | 2..3 | 3..3 | `bad argument #3: error converting Lua nil to table` with one argument. FAF always passes two (`FactoryBuilderManager.lua:500`, `campaign-ai.lua:1211`) |
| stub | `EntityCategoryCountAroundPosition` (4), `GetEntitiesInRect` (1..4: rect or four numbers), `PlayLoop(sound)` / `StopLoop(handle)` (1), `CoordinateAttacks` (1), `SetAlliedVictory(army, bool)` (2), `CAiAttacker:IsWithinAttackRange` (2..3) | | 2 / 1 / 2 / 0 / 1 / 2 | "not implemented", whatever the count (no FAF use) |
| declared only | `CreateBuilderArmController` (2..4), `GetReclaimablesInRect` (1..4), `entity:Kill` (1..4), `platoon:MoveToLocation` / `MoveToTarget` (2..4), `aibrain:SetArmyStatsTrigger` (5..6), `aibrain:PickBestAttackVector` (6..8, not exercised); interface `GetPreference` (1..2), `IssueCommand` (1..3), `IssueUnitCommand` (1..4), `PlayVoice` (1..3), `SessionSendChatMessage` (1..2), `UIWorldView.__init` (4..6) | | more required / fewer allowed | accepted (the signature docs are wrong, the conversions take nil) |

Missing: sim `CDamage` `GetInstigator/GetTarget/SetInstigator/SetTarget`, `AddLocalImpulse`, `aibrain:AddArmyStat`, sim `GetMouseWorldPos` (all M2 / M6). The 20 interface ones (category `__add/__sub/__mul`, edit colour getters, `histogram` / `mesh` / `userDecal` / `world_mesh` methods, `UIWorldView.SetHighlightEnabled`) exist at runtime but are absent from `--dump-api`: documentation gaps. Looser (ours accepts what the real one rejects): mostly a required argument ours treats as optional, plus the interface `LockInput` / `UnlockInput` / `IsInputLocked` and `_c_CreateDecal` taking one extra. 110 functions printed no count (39 need a game object as self, 13 returned nil).

## Missing entirely

| # | Name | Where | FAF use | Sev | State |
|---|---|---|---|---|---|
| <a id="m1"></a>M1 | `_TRACEBACK` (global, both VMs) | census `SIM G` / `UI G` REAL-ONLY | called at `fa/lua/system/Localization.lua:177` (malformed loc tag) and `fa/lua/maui/layouthelpers.lua:2360`: both raise "nonexistent global" in ours instead of warning | **A** (rare paths) | fixed `21e595a` |
| <a id="m2"></a>M2 | `serialize` (global table `fromstring`, `tostring`), `__pow`, `_LOADED`, `loadfile`, `GetTimeForProfile` (returns a number), `__factory_objects`, `__serialize_*`, sim `GetMouseWorldPos` | census | `serialize` is read at load by `fa/lua/system/saveload.lua:33`; the rest unused | C | open |
| <a id="m3"></a>M3 | `loadstring` in the interface VM (works there: `ls.basic => ok 2`, takes FA syntax `ls.ne => ok true`, loads bytecode); `HashChecker` (interface) | census, lang | FAF nils it in the sim (`SimHooks.lua:5`); UI mods can use it | C | open |
| <a id="m4"></a>M4 | `debug.` `allobjects allocinfo debug gethook getlocal getupvalue listcode listk listlocals profiledata sethook setlocal setupvalue trackallocations` | census | `sim/Profiler.lua:25` (sethook), `shared/DebugFunction.lua`, `system/profile.lua:113`: developer tools only. `debug.getinfo` also lacks `func`, `namewhat`, `what` fields | C | open |
| <a id="m5"></a>M5 | `string.dump`, `string.lualex` | census | none | C | open |
| <a id="m6"></a>M6 | `AddLocalImpulse` on every entity class; `aibrain_methods.AddArmyStat`; `CDamage` `GetInstigator GetTarget SetInstigator SetTarget` | census | none | C | open |

## Ours has, real doesn't

- Luau extras: `bit32`, `buffer`, `utf8`, `vector`, `os` (`clock date difftime time`), `select`, `rawlen`, `typeof`, `xpcall`, `coroutine.close/isyieldable/running`, `math.fmod/huge/modf/sign/...`, `string.pack/unpack`, `table.create/freeze/move/pack/maxn`, string methods and a string metatable also exist in the real game (`str.method => ok 3`). A script that feature-tests `os` or `xpcall` sees them.
- FAF's blueprint-loader functions leak into both VMs' globals (the real game runs them in a separate state): `LoadBlueprints ModBlueprints PreModBlueprints PostModBlueprints UnitBlueprint MeshBlueprint ProjectileBlueprint PropBlueprint EmitterBlueprint BeamBlueprint TrailEmitterBlueprint RegisterAllBlueprints ReloadBlueprint ExtractMeshBlueprint ...` (about 40), plus footprint constants `LAND AIR WATER SEABED SUB ORBIT IgnoreStructures` and `Engine GameType GameVersion ClientVersion Replay LaunchDir`.
- Engine internals as globals: sim `__engine_IssueTransportReverseLoad`, `__forin`; interface ~70 `__engine_*`, `__net_*`, `__disc_*`, `__map_download`.
- Methods: `BeenDestroyed` on every manipulator class and `IEffect`; `HideBone ShowBone RevertCollisionShape` on entity / prop / projectile / blip / shield / beam; `unit_methods` `GetCreator IsAutoSurfaceMode SetAutoSurfaceMode`; `text_methods.SetVertical`; 25 control methods on `PathDebugger_methods`; `moho.CameraImpl_methods` and `moho.userunit_methods` exist at start (the real `moho` table lacks both then).
- Globals: sim `EntityCategoryFilterOut` (interface-only in the real game); interface `EntityCategoryCount`, `GetUnitBlueprintByName`.
- Language: `%`, `#t`, `//`, `+=`, hex / binary / `_` literals, `\x` `\z` escapes, `...` as an expression, `continue` as a name, `goto`: all rejected by the real game's compiler.

## Checked and matching

- Census: every `moho.*` class in both VMs has the real method names apart from the lists above (sim 34 classes, e.g. `unit_methods` 192 real names all present; interface 25 classes). All 218 upper-case category names. Library members of `math`, `string`, `table`, `coroutine` (real ones all present).
- `table.sort` order with ties in the comparator: identical for 5, 12, 30 and 100 elements, ascending and descending (`tbl.sort_ties*`). The invalid-comparator cases match too.
- Issue #12 items now checked: `MinLerp` and `MinSlerp` (4 inputs each, including the shorter-arc and near-parallel cases; results are normalised), `EnumColorNames()` (141, `AliceBlue` first, `transparent` last), `GetVersion()` (`1.5.` + version), `Vector * Vector` is the cross product in both (`core.Vector_mulvec => {-3 6 -3}`; the operators are FAF's `utils.lua`, the issue's "raises" note is out of date). Alliance-name case was not reached (see below).
- `EulerToQuaternion`, `VDist2/3(Sq)`, `VDiff`, `VAdd`, `VMult`, `VDot`, `MATH_IRound` (2.5 -> 2, 3.5 -> 4, -2.5 -> -2), `MATH_Lerp` 3 and 5 arguments, `Rect` fields, `STR_Utf8Len`, `Dirname` / `FileCollapsePath` normal cases, `DiskFindFiles` (case-insensitive, sorted the same), `import` case-insensitivity and module metatable, strict globals, nil indexing, `getfenv` / `setfenv`, protected metatables, `__index` / `__newindex` / `__call` / `__eq` / `__lt` / `__tostring`, coroutine basics and the yield-across-pcall / metamethod errors, `string.find` (all 15 cases incl. `%f`), `string.sub` / `rep` / `byte` / `char`, most `string.format` cases, `math.mod` signs, `floor` / `ceil`, `!=`, `continue` in all three loops, `#` comments, `arg` varargs, evaluation order, BOM / CRLF files.
- Category counts for unit-only categories (30 of 50 tested), `EntityCategoryGetUnitList` order (sorted by id), category operators and precedence on userdata, `EntityCategoryContains` with an id string.
- Sim: `GetMapSize`, terrain height and its bilinear sample, `GetTerrainType` keys and values, `GetUnitsInRect` (rect, four numbers, table; nil when empty), `GetReclaimablesInRect`, alliance functions, army functions, unit id / position / orientation / health / bone basics.
- Interface: `GetArmiesTable`, `GetEconomyTotals`, camera settings, cursor methods, `Project`, time and rate functions, session flags (bar U4), `GetCommandLineArg`, `GetPreference` defaults, edit and item-list basics.
- Second run: 97 of the 177 new sim cases and 30 of the 45 new interface cases, among them: the issue #12 alliance-name case (`IsAlly(armies[1], armies[2])` = false in both), command queue values, navigator basics, unit health setters (negative, nil, no argument), weapon labels and targets, brain economy and threat, platoon make / squads / disband / after-disband, projectile chains, props, emitters (resize, army, beams, decals, splats, destroy), rotator / animator / slider / collision / storage / builder-arm manipulators, economy events, blips, `Damage` with a nil instigator and a bad type, `Warp`, debug drawing, `FlushIntelInRect`, `PlaySound`, and `uu.*` user-unit cases (see ui.md).

## Calls that take the real game down

| Call | Real | Ours |
|---|---|---|
| `STR_Utf8SubString('hello', 10, 1)` (start past the end), interface | dies (after `risky.Utf8Sub4` = `('hello', 2, 0)` gave `""`) | `""` |
| `unit:GetWeapon(0)` / `GetWeapon(99)` (one or both) | dies | nothing back, no error |
| `unit:GetAttacker()` (and every call on its result) | dies | raises "not implemented" |
| `brain:CanBuildStructureAt('nosuch', pos)` | dies | raises `unknown Unit blueprint 'nosuch'` |
| `ItemList` `GetItem(5)` / `DeleteItem(5)` / `SetSelection(5)` on a one-item list (one of them) | dies | no-ops, count 1, selection -1 |
| `Register*` (e.g. `RegisterBeamBlueprint`) and `Spec*` (e.g. `SpecFootprints`) with 12 nils, sim | dies | not checked |

## After the `--ui-lua` fix

H4 is fixed (`--ui-lua` is translated, `5d05759`). The interface rows here (U1-U10, `uu.*`, the `UI` halves of `probe_api_lang` / `probe_api_spot`) were all run through `import()` of a vault file, which was always translated, so none is an artefact of the old path; see [ui.md](ui.md#after-the---ui-lua-fix). Also fixed since: L1 (`^` is XOR, `af857a4`), L17 (a missing `doscript` file raises `Unable to find file <path>`, so `import` raises `Error importing`, `ae86c4a`; the real game raises and ours had returned an empty module) and M1 (`_TRACEBACK`, `21e595a`). `syn.concat_prec` now matches the real log in both VMs (`2 ^ 3 ^ 2` = 3, `-2 ^ 2` = 0); `syn.powzero` gives 4 in both (real game rerun 2026-11-16), and `syn.rngpow` (RNGAI's `x ^ 2` expressions with fractional variables) gives `{2 2 3 28 2 7}` in the real game: XOR.

S8 was wrong: `GetHighestThreatPosition(1, true, 'Overall')` returns a table first in the real game too (the first value is the position), so there is no difference.

## Could not test

- Which operator fails in L19 (`meta.arith` is one case): split it in `probe_api_spot`.
- The order `next` visits `STR_GetTokens`' 0-based result (decides neighbour order at `aiattackutilities.lua:1370`).
- `EntityCategoryContains(categories.PROJECTILE, projectile)` and other category tests on live projectiles and props (no such case in the probe).
- Which of `GetItem(5)`, `DeleteItem(5)`, `SetSelection(5)` on a one-item list kills the real game.
