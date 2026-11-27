# The typed Lua API (`src/lua_api`)

Every native function, moho class method and script callback the engine
exposes is declared once, in Rust, with its Lua name, parameter names, types,
docs and status. From that the engine generates Luau and EmmyLua definitions,
explains argument errors, and compares itself with FAF's annotations. Rust is
the source of truth; FAF's `/engine` annotations are only compared against.

## What you write

| You want to | Write | Not |
|---|---|---|
| a global function | `lua_fn!(lua, global, /// doc \n Name = \|lua, (a, b): (A, B)\| ...)?;` | `g.set("Name", lua.create_function(...))` |
| a function set with `raw_set` | `lua_fn!(lua, global_raw, Name = ...)?;` | `g.raw_set("Name", ...)` |
| a function in a global table | `lua_fn!(lua, table(t, "TableName"), Name = ...)?;` | `t.set("Name", ...)` |
| a moho class method | `lua_method!(lua, moho::Unit, /// doc \n Name = \|lua, (this, x): (Obj<moho::Unit>, f64)\| ...)?;` | `moho::add_method(lua, "unit_methods", "Name", ...)` |
| names from a list (loops) | `lua_api::register_fn(lua, &t, "", name, None, "", Status::Implemented, false, f)` / `lua_api::add_class_fn(lua, moho::Unit::CLASS, name, None, "", status, f)` | |
| call a script callback | `UnitCallbacks::on_stop_being_built(lua, &obj, what, builder, layer)?` (declared in `src/engine/api/callbacks.rs`) | `run_script(lua, &obj, "OnStopBeingBuilt", what, (b, l))` |
| read a Lua table into Rust | `#[derive(LuaTable)] struct X { ... }` then `t.get::<X>("Key")` / `X::from_lua_table` | a `get("Key")` per field |
| write a Rust struct into a Lua table | `#[derive(LuaTable)]` (`#[lua(write_only)]` if never read) then `x.write_into(lua, &t)` / `t.raw_set(k, x)` | a `raw_set("key", ..)` per field |
| a string-valued enum | `#[derive(LuaEnum)] enum Layer { Land, ... }` | matching on string literals |

- `lua_fn!` / `lua_method!` / `callbacks!` evaluate to `LuaResult<()>` (end with `?;`).
- `/// docs` before the name end up in the generated definitions; write them
  for anything whose meaning isn't obvious from the name (units, ranges,
  what nil means, FAF quirks). Comments that explained the registration move
  into the `///` docs.
- Status: `#[lua(stub)]` (exists so scripts can capture it), `#[lua(partial = "what's missing")]`,
  `#[lua(internal)]` (our own `__engine_*` hooks; excluded from the FAF comparison).
- Parameter names come from the closure pattern: `|lua, (this, rate): (Obj<moho::Unit>, f64)|`
  names `rate` (`self` is dropped for methods). `|lua, args: (A, B)|` leaves
  them unnamed (`arg1`, `arg2`), so destructure. A leading `_` is stripped.

- Where each thing is declared is recorded for the API browser's source
  links: the macros store the line its name is written on;
  `register_fn` / `add_class_fn` / `record_stub` / `script::exec` are
  `#[track_caller]`. A hand-built `FnEntry` gives
  `Src { file: file!(), line: line!() }`.

### `Obj<C>`: a typed `self`

`Obj<moho::Unit>` is a script object of class `unit_methods`. It derefs to
`LuaTable`, so existing helpers taking `&LuaTable` (`with_unit(lua, &this, ..)`,
`entity_id(&this)`) work unchanged. Converting from Lua only checks that the
value is a table (the native handle is checked where it is read, as before):
using `Obj` never changes behaviour. Entity classes have `this.id()`. Wrap a
table you already know the class of with `Obj::new(table)`. Use the most
specific class that is always right (`Obj<moho::Entity>` for "unit or prop").

The class markers are in `src/lua_api/class.rs` and re-exported as
`crate::engine::api::moho::{Unit, Entity, Weapon, AIBrain, ...}`.

### Callbacks

`src/engine/api/callbacks.rs` lists every callback the sim calls, by class,
with typed arguments. Call sites use the generated `XCallbacks::on_name(lua,
&obj, what, args...)` (a `run_script`: missing callback = no-op, error
logged). Pass objects as `Obj::new(table)`, layers as `Layer`. If a call site
picks the name at run time (`if on { "OnA" } else { "OnB" }`), call the two
functions in an `if`. A callback whose argument shape differs by case (Prop
`OnCollision` for tree groups) may stay a `run_script` with a comment saying why.

UI callbacks into FAF modules (`gamemain.OnBeat`) and other direct
`get(name)` + `call` sites: keep them, but the name should be a `const` or come
from a `callbacks!` block where one fits.

### Stubs and untyped functions: `signatures!`

A stub accepts anything (`LuaMultiValue`) so it can raise "not implemented"
or warn and return nil whatever it's passed; its closure can't carry types.
Declare its Lua signature instead, in `src/engine/api/signatures.rs` (sim,
and globals both VMs share) or `src/ui/signatures.rs` (user VM):

```rust
signatures! {
    global {
        DrawCircle(center: Vector, diameter: f64, color: Color);
        // The same name twice in a row: overloads.
        GetUnitsInRect(rectangle: Rect) -> Option<Vec<Obj<moho::Unit>>>;
        GetUnitsInRect(x0: f64, z0: f64, x1: f64, z1: f64) -> Option<Vec<Obj<moho::Unit>>>;
    }
    moho::Projectile {
        SetDamage(amount: Option<f64>, radius: Option<f64>);
    }
}
```

- Only the generated definitions use it; nothing changes at run time. The
  stub keeps its status (`--- Not implemented yet (stub).`).
- It applies to a stub whose parameters are all `any`, or a function taking
  only `LuaMultiValue`. An implemented function with typed parameters keeps
  its own; typed returns are always the closure's.
- Shared shapes are in `lua_api::shapes`: `Vector`, `Vector2`,
  `Quaternion`, `Rect`, `Bone`, `Army`, `Color` (see below).
- The existing ones were drafted from FAF's `/engine` annotations (a stub's
  expected arguments) and reviewed against our code for implemented ones.
- When you implement a stub, `declared_signatures_are_used_once`
  (`tests/api_types.rs`) fails until you delete its signature.

### Functions implemented in Luau

Engine API written in Luau (the vector and string shims in
`src/engine/lua/shims/`, the interface's `src/ui/lobby.lua`, `units.lua`,
`camera.lua`) is recorded from the source the VM runs: load it with
`lua_api::script::exec(lua, include_str!(...), name)` (or `script::record`
after a `load(...).call(...)`; a function a loader chunk returns, such as
the sim's `WaitFor`, with `script::record_one`). At the source's least
indentation (column 0, or the indentation of a block inlined in Rust),
`function Name(` is a global and,
after `local var = moho.class_methods`, `var.Name = function(` /
`function var:Name(` is a method; the `--` lines right above are its doc.
Each is recorded as taking `...: any`, so give it a `signatures!` entry for
types (the shims have one). `__`-prefixed names are internal, a one-line
`function(...) end` a stub.

The comparison lists FAF's misfilings apart (`compare::MISFILED`: a function
FAF's annotations put in another class or among the globals, settled by the
binary's registration). The user VM's methods FAF declares and we don't have
are stubbed at run time from FAF's annotations in the game files
(`ui::user_api::register_method_stubs`), so the report, built without them,
still lists them. The sim's are stubbed by name in `engine::api::pending`: `UNUSED` (reviewed: no FAF script or AI mod calls them, left as stubs on purpose; the reasons note where the original game did) and `TO_REVIEW` (waiting on other work).

### Shapes: `Vector`, `Bone`, `Army`, ...

Use them for parameters and returns instead of `LuaTable` / `LuaValue`:

```rust
lua_method!(lua, moho::Entity,
    GetPosition = |lua, (this, bone): (Obj<moho::Entity>, Option<shapes::Bone>)| {
        let pos = bone_position(lua, &this, bone.map(shapes::Bone::into_inner))?;
        vector(lua, pos)                       // returns shapes::Vector
    })?;
lua_fn!(lua, global,
    Warp = |lua, (obj, pos, orient): (Obj<moho::Entity>, shapes::Vector, Option<shapes::Quaternion>)| { ... })?;
```
->
```lua
function GetPosition(self, bone: Bone?): Vector
declare function Warp(obj: Entity, pos: { number }, orient: { number }?): ()
```

- Each wraps what the parameter was before (`Vector`, `Vector2`,
  `Quaternion`, `Rect`: a `LuaTable`; `Bone`, `Army`, `Color`: a `LuaValue`)
  and converts exactly as it did, so retyping a parameter changes only its
  signature. They deref to the inner value (`read_vector(&pos)`,
  `resolve_army(lua, &army)` work unchanged); `into_inner()` gives it back.
- A vector parameter is `{ number }` in the definitions (a literal
  `{x, y, z}` or a `Vector`: the engine reads indices 1 to 3); a returned
  one is `Vector`, with `.x` / `.y` / `.z` ([`LuaType::param_ty`]).
- A script object parameter is `Obj<moho::X>` (`Obj<moho::Entity>` for
  "unit, prop or projectile"), never `LuaTable`: Luau doesn't treat a
  `Unit` as a plain table, so `{ [any]: any }` would reject one.

### Typed tables

```rust
/// A unit blueprint's `Air` table.
#[derive(LuaTable)]
#[lua(name = "UnitBlueprintAir", rename_all = "PascalCase")]
struct AirBlueprint {
    max_airspeed: Option<Fx>,           // missing -> None
    #[lua(rename = "CirclingRadiusChangeMinRatio")]
    circling_min_ratio: Option<Fx>,
    #[lua(default)] winged: bool,       // missing -> false
    #[lua(default = "1.0")] uniform_scale: f32,
    #[lua(skip)] cache: u32,            // never read or written
}
```

- A missing key is an error unless the field is an `Option` or has a default.
  To keep today's behaviour, mirror the old code exactly (`unwrap_or(x)` ->
  `#[lua(default = "x")]` or `Option` + `unwrap_or` at the use).
- `Fx` fields are read as `f64` and converted with `math::from_lua` (the
  same conversion the old code used). Keep `f64` where the old code did float
  math before converting.
- Reads use `get` (honours metatables) unless `#[lua(raw)]`; writes use
  `raw_set`. Match what the old code did.
- `write_into(lua, &t)` updates an existing table (no table allocation);
  `IntoLua` creates one.
- `X::unknown_keys(&t)` lists keys the struct doesn't know (typos in data,
  unimplemented fields).

## Rules for converting

1. **No behaviour change.** Same name, same table, `set` vs `raw_set` as
   before, same reads (`get` vs `raw_get`), same defaults, same conversions,
   same registration order (stubs in `pending.rs` first). The sim's golden
   checksum must not move.
2. **No new per-call cost.** The macros add nothing per call (a registry
   push at startup). Don't add allocations or extra Lua lookups to hot paths.
3. **Every Rust type crossing the boundary needs a Lua type** (`LuaType`):
   primitives, strings, `LuaTable`/`LuaValue`/`LuaFunction`, `Option`, `Vec`,
   maps, `Obj<C>`, derived structs/enums. For a userdata handle type, add an
   `impl crate::lua_api::LuaType` next to it (a `LuaTy::Class("moho_name")` if
   it is a moho class, else `LuaTy::Userdata`).

## Checks

| Check | Run | Catches |
|---|---|---|
| Lua-visible API unchanged | `cargo test --test api_surface lua_visible_api_is_unchanged` | a registration renamed, dropped or added |
| everything registered through `lua_api` | `cargo test --test api_surface every_native_function` | registrations still using `create_function` directly (lists them) |
| definitions generate, deterministically | `cargo test --test api_types` | generation failing, or output that changes between runs (unsorted maps, registration order) |
| macros, derives, error explanations | `cargo test --lib lua_api` | |
| declared signatures used, once each | `cargo test --test api_types` | a `signatures!` entry for a function since implemented with types |
| `.d.luau` valid Luau | `LUAU_LSP=/path/to/luau-lsp cargo test --test api_types -- --include-ignored` | syntax errors, unknown types (type-checks `tests/luau/*_usage.luau`) |
| sim unchanged | `cargo test --test golden_fight` | behaviour changes |

## Generated files

The generated files aren't checked in (`docs/api/` is ignored by git):
generate them locally, or take them from CI (the `rts-core-engine-lua-api` artifact of every `main`
build, and a zip on each `v*` release; see [packaging.md](packaging.md)). `cargo run -- --dump-api docs/api` writes, per VM
(`sim`, `user`), with no game files or FAF checkout needed:

- `<vm>.d.luau`: Luau definitions (`declare extern type Unit extends Entity with`,
  `declare function`, `export type UnitCallbacks = {...}`, enums as string
  unions, typed tables). For luau-lsp (`luau-lsp analyze --definitions=@moho=docs/api/sim.d.luau`);
  the stock `luau-analyze` CLI can't load definitions files. Luau's own
  libraries (`debug`) aren't redeclared, since that would replace them: our
  additions (`debug.getinfo`) are listed in a comment.
- `<vm>.lua`: EmmyLua annotations (`---@class moho.unit_methods : moho.entity_methods`),
  the format of FAF's `/engine` files, for lua-language-server.
- `index.html`: the API browser, one page for both VMs: a search palette (`/` or
  Ctrl-K: names, words from the docs, the other VM's names, recent pages), the
  classes as a tree, each function's signature with its types linked, docs,
  status (implemented / partial / stub), a link to the Rust line that
  registers it, what uses each type, and how much of each class works. The
  data is inlined, so it opens from disk (its libraries come from a CDN, so
  it needs a network connection). The page's source is
  `src/lua_api/browser/index.html`; opened from there it reads `api.json`
  beside it, so to work on it serve a folder holding both
  (`python3 -m http.server`). A link is `#sim/Unit/SetBuildRate`,
  `#sim/globals/IssueMove`, `#sim/types/Vector`, `#sim/Unit/on.OnCreate`.
- `api.json`: the same data for other tools (`src/lua_api/browser.rs`):
  per VM, the classes (bases, methods, callbacks), table functions, globals
  and named types; each function with its parameters, returns, overloads,
  doc, status and `src` (file and line). Types are spelled as in the
  `.d.luau`.
- `undocumented.md`: the functions and callbacks with no `///` doc, per
  VM and class, so filling them in can be tracked.
- `faf-compare.md`, only with `--faf-compare [FAF_DIR]` (default `FAF_REPO`,
  else `../fa`; fails if it isn't a FAF checkout): what FAF declares
  that we don't register, what we register that FAF doesn't declare, likely
  typos (an unregistered FAF name one or two edits away), and parameter count
  differences. A report to read, not a gate. Metamethods (`__add`) aren't
  compared; FAF's files without a `moho.` class tag (`UserUnit`, `Camera`)
  are read as the classes in `compare::UNTAGGED`.

Two scripts check the rest of FAF's `/engine` annotations against the
binaries (they need the game and a FAF checkout):

- `dev/audit/bp_audit.py`: blueprint fields, against moho's reflection tables
  (MohoEngine.dll and FA's exe) and FAF's `engine/Core/Blueprints` (see
  `docs/guides/audits.md`).
- `python3 dev/audit/enum_audit.py > docs/api/enums.md`: FAF's
  `engine/Enums.lua` against moho's enums (each member is registered as
  `push value; push "name"`, so names and values are read from the code) and
  the names our source uses; also checks that our `COMMAND_CAPS` /
  `TOGGLE_CAPS` bit order matches moho's values. FAF's annotations can be
  wrong: its `Layer` alias says `Orbital`, moho's string is `Orbit`.

Not compared: `engine/AnnotationCommon.lua` (helper classes), `Unsafe.lua`
(the init VM's `SHGetFolderPath`), `moho.lua` (an example table).

## Argument errors

Native functions are named in the VM (`Unit:SetBuildRate`, via the vendored
Luau patch `lua_setcdebugname`), so tracebacks name them, and errors logged
through `lua_api::errors::Explained` add the signature:

```
bad argument #2: error converting Lua nil to f64
  in Unit:SetBuildRate(rate: number): argument #2 `rate` should be number
```

Nothing is added to a call that succeeds: the explanation is built when the
error is logged.
