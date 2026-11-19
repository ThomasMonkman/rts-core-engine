//! Moho script objects: engine objects are exposed to Lua as plain tables (so
//! FAF can `setmetatable` them to its own classes) carrying the native handle
//! in a `_c_object` userdata field.

use mlua::prelude::*;

pub const C_OBJECT: &str = "_c_object";

/// Calls `obj:<name>(args...)` the way moho's `RunScript` does: a missing
/// method is a no-op, and an error is logged ("Error running <name> script in
/// <what>") instead of propagated, so a failing callback never aborts the
/// engine operation that fired it (e.g. `CreateProp` still returns the prop).
pub fn run_script(lua: &Lua, obj: &LuaTable, name: &str, what: &str, args: impl IntoLuaMulti) -> LuaResult<()> {
    let Some(f) = obj.get::<Option<LuaFunction>>(name)? else { return Ok(()) };
    // Tracy level 2: the Lua the sim runs serially, by callback method.
    #[cfg(feature = "tracy")]
    let _zone = crate::engine::tracy::callback_zone(name);
    let mut args = args.into_lua_multi(lua)?;
    args.push_front(LuaValue::Table(obj.clone()));
    if let Err(e) = f.call::<()>(args) {
        tracing::warn!(target: "lua", "Error running {name} script in {what}: {}", crate::lua_api::errors::Explained(lua, &e));
    }
    Ok(())
}

/// Creates a script-object table for `handle` with `metatable`.
pub fn create<T: LuaUserData + mlua::MaybeSend + 'static>(
    lua: &Lua,
    handle: T,
    metatable: LuaTable,
) -> LuaResult<LuaTable> {
    let t = lua.create_table_with_capacity(0, 4)?;
    t.raw_set(C_OBJECT, lua.create_userdata(handle)?)?;
    t.set_metatable(Some(metatable));
    Ok(t)
}

/// Reads the native handle of a script object (`self` in a native method).
pub fn handle<T: LuaUserData + Clone + 'static>(obj: &LuaTable) -> LuaResult<T> {
    match obj.raw_get::<LuaValue>(C_OBJECT)? {
        LuaValue::UserData(ud) => Ok(ud.borrow::<T>()?.clone()),
        _ => Err(LuaError::runtime("expected an engine object (no _c_object)")),
    }
}

/// Registry tables already fetched, by key (each VM its own).
#[derive(Default)]
struct RegistryTables(std::collections::BTreeMap<&'static str, LuaTable>);

/// Forgets the cached registry tables (a keyframe load may have replaced
/// them).
pub fn clear_registry_cache(lua: &Lua) {
    lua.remove_app_data::<RegistryTables>();
}

/// The registry table under `key`, created on first use, then served from a
/// cache: for tables set once and never replaced (entity, weapon, brain,
/// platoon, effect objects). They're looked up on every callback into
/// Lua, and a named registry lookup hashes the key each time.
pub fn registry_table(lua: &Lua, key: &'static str) -> LuaResult<LuaTable> {
    if let Some(t) = lua.app_data_ref::<RegistryTables>().and_then(|c| c.0.get(key).cloned()) {
        return Ok(t);
    }
    let t = match lua.named_registry_value::<Option<LuaTable>>(key)? {
        Some(t) => t,
        None => {
            let t = lua.create_table()?;
            lua.set_named_registry_value(key, t.clone())?;
            t
        }
    };
    if lua.app_data_ref::<RegistryTables>().is_none() {
        lua.set_app_data(RegistryTables::default());
    }
    if let Some(mut c) = lua.app_data_mut::<RegistryTables>() {
        c.0.insert(key, t.clone());
    }
    Ok(t)
}
