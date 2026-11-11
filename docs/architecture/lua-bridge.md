# Lua Bridge

## Runtime
`mlua` with Luau feature.

SupCom ran its own Lua 5.0, with syntax of its own. We run Luau, so every script is translated from SupCom Lua to Luau before it's compiled ([Translation](#translation-supcom-lua-to-luau)), and every script reaches the VM through one loading path ([Loading scripts](#loading-scripts)).

## Loading scripts

All game Lua (FAF's, the base game's, mods', maps') is loaded the same way, through `doscript` / `dofile` (`src/engine/api/dofile_or_script.rs`):

```
VFS file ──▶ hooks appended ──▶ translated ──▶ compiled ──▶ lua.load(bytecode)
             (moho's script      (VFS transform  (ScriptCache:   (optionally in
              hooks, then the     for `.lua`,     Luau bytecode   an `env` table)
              session's mods)     cached)         per path)
```

1. **Read and hook** (`Vfs::read_processed`): the file at the top mount, with moho's script hooks appended (each hook folder's file of the same path runs after it, in the same chunk), plus a session's mod hooks (`engine::mods::ModHooks`).
2. **Translate**: a VFS transform registered for `.lua` (`build_game_files` in `src/engine/mod.rs`) runs [`lua_translation::translate`](#translation-supcom-lua-to-luau) once per file. The VFS caches the result, so it's shared by every session in the process.
3. **Compile** (`ScriptCache`, `src/engine/lua/script_cache.rs`): Luau bytecode per path. At startup a background thread translates and compiles every `.lua` / `.bp` in parallel on rayon, in the order Lua will ask for them. On a miss, the VM compiles inline. Compiling is deterministic, so if both race they produce the same bytes.
4. **Load**: `lua.load(bytecode)`, bound to `env` if one is given. The chunk then runs from a tiny Luau wrapper, not from Rust, so script errors stay plain Lua values, and nested `doscript`s don't stack Rust frames or hold the VFS lock.

A file that's missing does nothing. One that doesn't compile is skipped with a warning, as moho's `SCR_LuaDoFileConcat` does: FAF's map scan relies on that.

**The rule: never `lua.load` game source yourself.** Go through `doscript` (or `ScriptCache`), or the script misses the translation, the hooks, the mod hooks and the cache. Lua that isn't in the VFS translates itself before loading: the init file and the init VM's `dofile` (real files), `Game.prefs` (`ui/prefs.rs`) and the console's `ui_lua`. Engine-written Lua (the chunks in `src/`, `--script` files) is plain Luau and isn't translated.

`.bp` files go through the same path but have no translation transform: FAF's blueprints use no SupCom-only syntax. A mod's blueprint that did would fail to compile.

### `import`
`import(path)` is FAF's own Lua (`/lua/system/import.lua`), built on our `doscript(path, env)`. It makes a module table whose metatable falls back to `_G`, runs the file in it, and caches it. Each module gets its own environment, so its globals don't leak into `_G`. Every caller gets the **same** table, which class inheritance depends on.

### Reloading changed files
`--enable-disk-watch` (the game's `/EnableDiskWatch`) watches the VFS's folder mounts (`src/engine/disk_watch.rs`). When a file in the VFS changes, its cached source and bytecode are dropped, and each VM calls every function in its `__diskwatch` table with the lowercase VFS path. The interface does this every frame, and the sim before each tick (but not in a networked game, where it would desync). FAF's `import.lua` registers `dirty_module` there: it runs the module's `__moduleinfo.OnDirty`, then removes it and every module that imported it from `__modules`, so the next `import` runs the new file (and `OnReload` gets the new module). A changed hook file reloads the script it hooks. Only Lua modules reload: blueprints and textures don't, and files added after startup aren't picked up.

## Translation: SupCom Lua to Luau

`src/engine/lua_translation/`, one module per difference, run in this order by `translate()`:

| SupCom Lua | Luau | Pass |
|---|---|---|
| A UTF-8 BOM at the start | rejected | stripped (`strip_bom`) |
| `# comment` (line start or after code) | `#` is the length operator | `#` → `--` (`hash_comment.rs`) |
| `a != b` | only `~=` | `!=` → `~=` (`neq.rs`; a plain text replace, so it would also change a `!=` inside a string. FAF has none) |
| `{&N &M}` (table pre-size hints) | not valid | stripped to `{}` (`prealloc.rs`) |
| `&`, `\|`, `~`, `<<`, `>>` | no bitwise operators | `bit32.band`, `bor`, `bxor`, `lshift`, `rshift` (`bitwise.rs`, parsed with `full_moon` as Lua 5.3 + Luau) |
| implicit `arg` table in `function(...)` | no such local | `local arg = table.pack(...)`, only where `arg` is used (`vararg.rs`) |
| `for k, v in t do` over a table (5.0) | calls the table if it has `__call` (FAF classes do) | `for k, v in __forin(t)`, which iterates tables with `next` (`forin.rs`) |
| `continue` | native | unchanged |

`lexscan.rs` holds the byte-level scanners the passes share, so none of them rewrites text inside strings or comments.

To add one: write a new pass module, add it to `translate()`, and test it against real FAF files (`actual_fa_files_tests.rs`). The other Lua 5.0 differences are runtime ones, not syntax: see [Moho VM semantics we replicate](#moho-vm-semantics-we-replicate).

## Coroutine scheduler
FAF uses `ForkThread` / `WaitSeconds` extensively. Key design:
- `ForkThread(fn, ...)` pushes to a `pending_fork` queue; picked up at end of tick
- `WaitSeconds(n)` yields with a resume tick target
- Sim waits are moho's (`Scheduler::wait`, measured in the real game): `coroutine.yield(n)` (FAF's sim `WaitTicks`) wakes `floor(n) - 1` ticks later, at least 1; below 1 it runs again at once, before any other thread; a bare yield, or one of a non-number or a negative count, ends the thread (`CLuaTask::Execute`). `ResumeThread` also cuts a `WaitTicks` short: the thread runs this tick if the pass hasn't reached it yet, else next tick (`Scheduler::resume`). FAF's `WaitSeconds(n)` is `WaitTicks(n * 10 + 1)`, so `n` seconds. The interface VM's yields still wait `max(n, 1)` frames (unmeasured)
- Scheduler: `BinaryHeap` (sleeping by tick) + `VecDeque` (ready to run)
- Run `drain_ready_queue()` each tick, then `wake_sleeping_tasks(current_tick)`
- Collect pending forks **after each resume** so children are visible same tick

## Native → Lua callbacks
- Rust systems produce `SimEvent` variants (OnDamaged, OnKilled, OnCreate, OnImpact...)
- Events are drained **after all Rust systems finish** — end of tick phase
- Callbacks are **synchronous** — Lua runs inline, blocks until done
- Per-entity callback tables stored in Lua registry: `__entity_callbacks[entity_id][method_name]`

```rust
// Drain events after systems, before next tick
for event in world.drain_events() {
    callbacks.dispatch(event)?;
}
coroutine_scheduler.tick(current_tick);
```

## Table pre-allocation
For known-shape game objects, use `create_table_with_capacity(0, n)` on Rust side.
On Lua side, always use constructor literals with all keys defined — compiler pre-sizes the hash part.

## Type checking
Luau types are static analysis only — VM does not enforce at runtime. Use `luau-analyze` as an optional pre-load step (dev/CI only). Gate behind env var `FAF_TYPECHECK=1`.

## Moho VM semantics we replicate
| Behaviour | Where |
|-----------|-------|
| Indexing `nil` yields `nil` (writes/calls still raise) | `api/nil_metatable.rs` — per-type metatable on nil |
| Indexing a boolean, number, function or userdata yields `nil` too (writes still raise); ours logs a warning once per call site for numbers, functions and userdata, as that is nearly always a script bug | `api/nil_metatable.rs` (`register_number_metatable`, `register_functions`, `add_nil_index` on every handle type) |
| `VDist3` reads a `nil` vector as `{0, 0, 0}` (`VDist3Sq` and `VDist2` raise) | `lua/shims/vector_math.lua` |
| Ordering comparisons with `nil` or mismatched types are `false` | patched Luau, `vendor/luau0-src/MOHO_PATCHES.md` |
| Implicit vararg `arg` table | `lua_translation/vararg.rs` |
| `math.mod`, `math.pow`, `string.gfind`, `table.setn` | `lua/shims/lua50_aliases.lua` |
