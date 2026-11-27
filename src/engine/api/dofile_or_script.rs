use mlua::prelude::*;
use std::sync::{Arc, RwLock};
use tracing::{debug, warn};

use crate::engine::{
    lua::{execute::exec_lua_string, script_cache::ScriptCache},
    lua_translation,
    vfs::Vfs,
};
use crate::lua_api::{FnEntry, LuaArgs, Scope, Status, lua_fn};

pub fn register(lua: &Lua) -> LuaResult<()> {
    // The native part only compiles the chunk; running it happens in Lua so that
    // errors raised by the script stay plain Lua values (`pcall`/`safecall` can
    // concatenate them), nested doscripts don't stack Rust frames, and the VFS
    // lock is released before any script code runs.
    let load = lua.create_function(|lua, (path, env): (String, Option<LuaTable>)| {
        load_file_from_vfs(lua, &path, env)
    })?;
    lua.load(
        r#"
        local load = ...
        local function doscript(path, env)
            local chunk, err, level = load(path, env)
            if chunk == nil then
                if err ~= nil then error(err, level or 2) end
                return
            end
            -- The chunk's own return value is dropped: moho's doscript
            -- returns nothing (probed).
            chunk()
        end
        _G.doscript = doscript
        _G.dofile = doscript
        "#,
    )
    .set_name("=[engine doscript]")
    .call::<()>(load)?;
    // `doscript` and `dofile` are the Luau closures above (a native function
    // can't be re-entered by a nested script without stacking Rust frames);
    // recorded here so they appear in the API.
    for (name, doc) in [
        ("doscript", "Runs a script from the VFS, optionally in the environment `env`; returns nothing, whatever the chunk returns. A missing file raises `Unable to find file <path>`; one that doesn't compile is skipped with a warning."),
        ("dofile", "Same as `doscript`."),
    ] {
        crate::lua_api::registry::record(
            lua,
            FnEntry { scope: Scope::Global, name: name.into(), params: Some(&[Some("path"), Some("env")]), args: <(String, Option<std::collections::BTreeMap<String, LuaValue>>) as LuaArgs>::types, rets: <LuaMultiValue as LuaArgs>::types, doc, status: Status::Implemented },
        );
    }
    #[cfg(feature = "tracy")]
    traced_doscript(lua)?;
    Ok(())
}

/// Profiling builds: `doscript` runs each chunk inside a `doscript` span
/// named by its path (FAF's blueprint load, imports and simInit then show
/// file by file). A native stack holds the entered spans; the chunk runs
/// under `pcall` so an error still closes its span, then is re-raised as
/// the same value.
#[cfg(feature = "tracy")]
fn traced_doscript(lua: &Lua) -> LuaResult<()> {
    thread_local! {
        static SPANS: std::cell::RefCell<Vec<tracing::span::EnteredSpan>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let enter = lua.create_function(|_, path: String| {
        SPANS.with(|s| s.borrow_mut().push(tracing::trace_span!("doscript", path = %path).entered()));
        Ok(())
    })?;
    let leave = lua.create_function(|_, ()| {
        SPANS.with(|s| drop(s.borrow_mut().pop()));
        Ok(())
    })?;
    // Named Lua spans for engine-side profiling wrappers (BeginSession's
    // steps: `session::profile_begin_session`).
    lua_fn!(lua, global_raw,
        /// Profiling builds: opens a named span.
        #[lua(internal)]
        __prof_enter = |_, name: String| {
            SPANS.with(|s| s.borrow_mut().push(tracing::trace_span!("lua", name = %name).entered()));
            Ok(())
        })?;
    lua.globals().raw_set("__prof_leave", leave.clone())?;
    lua.load(
        r#"
        local enter, leave = ...
        local load = _G.doscript
        local function doscript(path, env)
            enter(path)
            local r = table.pack(pcall(load, path, env))
            leave()
            if not r[1] then error(r[2], 0) end
            return unpack(r, 2, r.n)
        end
        _G.doscript = doscript
        _G.dofile = doscript
        "#,
    )
    .set_name("=[engine traced doscript]")
    .call::<()>((enter, leave))
}

/// The globals translated scripts call that the source never names:
/// `for k, v in t` becomes `__forin(t)`, the bit operators `bit32.*`.
const TRANSLATION_HELPERS: [&str; 2] = ["__forin", "bit32"];

/// Lets a script run in `env` reach the [`TRANSLATION_HELPERS`]. An
/// environment that falls back to the globals already does. A bare table
/// (FAF's `NewSafeEnv` for a mod's `mod_icons.lua`, the table a prefs file
/// is read into) gets a metatable whose `__index` holds just the helpers, so
/// the table's own keys stay what the script set. One with a metatable of
/// its own that doesn't lead to them is left alone.
fn give_translation_helpers(lua: &Lua, env: &LuaTable) -> LuaResult<()> {
    if env.metatable().is_some() || !env.raw_get::<LuaValue>(TRANSLATION_HELPERS[0])?.is_nil() {
        return Ok(());
    }
    const KEY: &str = "doscript.translation_helpers";
    let meta = match lua.named_registry_value::<Option<LuaTable>>(KEY)? {
        Some(meta) => meta,
        None => {
            let helpers = lua.create_table()?;
            for name in TRANSLATION_HELPERS {
                helpers.raw_set(name, lua.globals().raw_get::<LuaValue>(name)?)?;
            }
            let meta = lua.create_table()?;
            meta.raw_set("__index", helpers)?;
            lua.set_named_registry_value(KEY, &meta)?;
            meta
        }
    };
    env.set_metatable(Some(meta));
    Ok(())
}

/// Loads `path` (compiled bytecode from the [`ScriptCache`]) into a function,
/// optionally bound to `env`. Returns `(nil, nil)` for one that doesn't
/// compile (warned), `(nil, message, level)` for an error `doscript` raises:
/// a missing file, or bytecode that won't load.
fn load_file_from_vfs(
    lua: &Lua,
    path: &str,
    env: Option<LuaTable>,
) -> LuaResult<(Option<LuaFunction>, Option<String>, Option<u8>)> {
    let cache = lua
        .app_data_ref::<Arc<ScriptCache>>()
        .expect("ScriptCache not in app_data")
        .clone();
    // A script an active mod hooks: compiled with the hooks of the mods
    // `__active_mods` lists now (moho reads it at every load).
    crate::engine::mods::sync_hooks(lua, || Some(cache.vfs().clone()));
    let hooked = lua.app_data_ref::<crate::engine::mods::ModHooks>().and_then(|h| h.get(&cache, path));
    let bytecode = match hooked.unwrap_or_else(|| cache.get(path)) {
        Ok(Ok(bc)) => bc,
        // A script that doesn't compile is skipped with a warning, as the
        // real engine's loader does (`SCR_LuaDoFileConcat`, FA exe 0x4d57f0,
        // warns and returns). FAF's map scan relies on it: one broken
        // `_scenario.lua` would otherwise end the lobby's whole map list.
        // Luau's error is `line: message`, so the file is named here.
        Ok(Err(message)) => {
            warn!(target: "lua", "SCR_LuaDoFileConcat: Loading \"{path}\" failed: [string \"{path}\"]:{message}");
            return Ok((None, None, None));
        }
        // A missing file raises, with no position, as the real `doscript`
        // does (`Unable to find file /lua/nosuch_file.lua`, probed). FAF's
        // `import` turns it into `Error importing '<path>'`, so a
        // `pcall(import, optional)` takes its fallback; FAF tests optional
        // files with `exists` / `DiskGetFileInfo` before `doscript`.
        Err(_) => return Ok((None, Some(format!("Unable to find file {path}")), Some(0))),
    };

    debug!(path = path, "doscript: exec");
    let mut chunk = lua.load(&*bytecode).set_name(path);
    if let Some(env) = env {
        give_translation_helpers(lua, &env)?;
        chunk = chunk.set_environment(env);
    }
    match chunk.into_function() {
        Ok(f) => {
            if crate::engine::lua::debugger::enabled() {
                crate::engine::lua::debugger::on_chunk(lua, path, &f);
            }
            Ok((Some(f), None, None))
        }
        Err(e) => Ok((None, Some(e.to_string()), None)),
    }
}

pub fn register_unsafe(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// Runs a Lua file from the real file system (the init VM only).
        dofile = |lua, path: String| exec_file_real_os_file(lua, &path))
}

pub fn exec_file_from_vfs(lua: &Lua, path: &str) -> LuaResult<()> {
    let vfs_arc = lua
        .app_data_ref::<Arc<RwLock<Vfs>>>()
        .expect("VFS not in app_data")
        .clone();
    let vfs = vfs_arc.read().expect("VFS poisoned");
    let Ok(code) = crate::engine::mods::script_source(lua, &vfs, path) else {
        debug!(path = path, "exec_file_from_vfs: skip missing");
        return Ok(());
    };

    debug!(path = path, "exec_file_from_vfs: exec");
    exec_lua_string(lua, &String::from_utf8_lossy(&code), path)?;

    Ok(())
}

pub fn exec_file_real_os_file(lua: &Lua, path: &str) -> LuaResult<()> {
    // let Some(bytes) = read_vfs_or_fs(lua, path) else {
    //     debug!(path = path, "dofile: skip missing");
    //     return Ok(());
    // };

    // The init VM runs Lua files only.
    if !super::io::is_lua_file(path) {
        debug!(path = path, "dofile: refused (not a .lua file)");
        return Ok(());
    }
    let Some(bytes) = std::fs::read(super::io::disk_path(path)).ok() else {
        debug!(path = path, "dofile: skip missing");
        return Ok(());
    };

    debug!(path = path, "dofile: exec");
    let code = lua_translation::translate(&String::from_utf8_lossy(&bytes));
    exec_lua_string(lua, &code, path)?;
    // lua.load(code).set_name(path).exec()?;

    Ok(())
}

// /// Try VFS first, then real filesystem.
// pub fn read_vfs_or_fs(lua: &Lua, path: &str) -> Option<Vec<u8>> {
//     // VFS lookup
//     {
//         let vfs_arc = lua
//             .app_data_ref::<Arc<RwLock<Vfs>>>()
//             .expect("VFS not in app_data")
//             .clone();
//         let vfs = vfs_arc.read().expect("VFS poisoned");
//         if let Some(bytes) = vfs.read_file(path) {
//             return Some(bytes);
//         }
//     }
//     // Filesystem fallback
//     std::fs::read(path).ok()
// }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::vfs::MountPoint;

    /// A script that doesn't compile is skipped, as in the real engine, and
    /// the script calling `doscript` carries on (one broken map's
    /// `_scenario.lua` emptied the lobby's map list).
    #[test]
    fn syntax_error_is_skipped() {
        let dir = std::env::temp_dir().join(format!("doscript_syntax_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("maps/broken")).unwrap();
        std::fs::write(dir.join("maps/broken/broken_scenario.lua"), "ScenarioInfo = {\n  a = 1 b = 2\n}\n").unwrap();
        let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        let lua = Lua::new();
        lua.set_app_data(Arc::new(ScriptCache::new(Arc::new(RwLock::new(vfs)))));
        register(&lua).unwrap();

        let after: i64 = lua
            .load("local data = {} doscript('/maps/broken/broken_scenario.lua', data) return data.ScenarioInfo == nil and 1 or 0")
            .eval()
            .unwrap();
        assert_eq!(after, 1);
        std::fs::remove_dir_all(dir).ok();
    }

    /// A script run in a bare environment can still loop over a table and
    /// call `bit32` (what `for k, v in t` and the bit operators become), and
    /// the environment's own keys are only what the script set.
    #[test]
    fn bare_environment_reaches_the_translation_helpers() {
        let dir = std::env::temp_dir().join(format!("doscript_bare_env_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(dir.join("lua/icons.lua"), "Sum = 0\nfor _, v in { 1, 2, 4 } do Sum = Sum + v end\nBits = bit32.band(6, 3)\nfunction Later(t) local n = 0 for _ in t do n = n + 1 end return n end\n").unwrap();
        let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        let lua = Lua::new();
        lua.set_app_data(Arc::new(ScriptCache::new(Arc::new(RwLock::new(vfs)))));
        register(&lua).unwrap();
        lua.load("function __forin(t) return next, t end").exec().unwrap();

        let out: String = lua
            .load(
                "local env = {} doscript('/lua/icons.lua', env)
                 local keys = {} for k in pairs(env) do table.insert(keys, k) end table.sort(keys)
                 return table.concat(keys, ',') .. ' ' .. env.Sum .. ' ' .. env.Bits .. ' ' .. env.Later({ 1, 2 })",
            )
            .eval()
            .unwrap();
        assert_eq!(out, "Bits,Later,Sum 7 2 2");
        std::fs::remove_dir_all(dir).ok();
    }

    /// A missing file raises `Unable to find file <path>` (no position), so
    /// FAF's `import` of a missing module raises too, as in the real game.
    #[test]
    fn missing_file_raises() {
        let dir = std::env::temp_dir().join(format!("doscript_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        let lua = Lua::new();
        lua.set_app_data(Arc::new(ScriptCache::new(Arc::new(RwLock::new(vfs)))));
        register(&lua).unwrap();

        let (ok, msg): (bool, String) = lua.load("return pcall(doscript, '/lua/nosuch_file.lua')").eval().unwrap();
        assert!(!ok);
        assert_eq!(msg, "Unable to find file /lua/nosuch_file.lua");
        std::fs::remove_dir_all(dir).ok();
    }

    /// `doscript` returns nothing whatever the chunk returns, as in the real
    /// game.
    #[test]
    fn returns_nothing() {
        let dir = std::env::temp_dir().join(format!("doscript_ret_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(dir.join("lua/ret.lua"), "x = 1 return 5").unwrap();
        let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        let lua = Lua::new();
        lua.set_app_data(Arc::new(ScriptCache::new(Arc::new(RwLock::new(vfs)))));
        register(&lua).unwrap();

        let (n, x): (i64, i64) = lua.load("return select('#', doscript('/lua/ret.lua')), x").eval().unwrap();
        assert_eq!((n, x), (0, 1));
        std::fs::remove_dir_all(dir).ok();
    }
}
