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
            local chunk, err = load(path, env)
            if chunk == nil then
                if err ~= nil then error(err, 2) end
                return
            end
            return chunk()
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
        ("doscript", "Runs a script from the VFS, optionally in the environment `env`; returns what it returns. A missing file does nothing; one that doesn't compile is skipped with a warning."),
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

/// Loads `path` (compiled bytecode from the [`ScriptCache`]) into a function,
/// optionally bound to `env`. Returns `(nil, nil)` for a missing file or one
/// that doesn't compile (warned), `(nil, message)` if the bytecode won't load.
fn load_file_from_vfs(
    lua: &Lua,
    path: &str,
    env: Option<LuaTable>,
) -> LuaResult<(Option<LuaFunction>, Option<String>)> {
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
            return Ok((None, None));
        }
        Err(_) => {
            debug!(path = path, "doscript: skip missing");
            return Ok((None, None));
        }
    };

    debug!(path = path, "doscript: exec");
    let mut chunk = lua.load(&*bytecode).set_name(path);
    if let Some(env) = env {
        chunk = chunk.set_environment(env);
    }
    match chunk.into_function() {
        Ok(f) => Ok((Some(f), None)),
        Err(e) => Ok((None, Some(e.to_string()))),
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
    let Some(bytes) = std::fs::read(path).ok() else {
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
}
