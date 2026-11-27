//! Key maps (moho's `IN_*KeyMapTable`): tables of key name (`"Ctrl-S"`) to
//! action, a console command or `{ action = command }`. FAF adds and removes
//! whole tables (the player's bindings, the debug keys, a dialog's own) and
//! may change one after adding it, so the tables themselves are kept, the
//! most recently added looked in first. Render-thread only.

use mlua::Either;
use mlua::prelude::*;

use crate::lua_api::lua_fn;
use crate::ui::shapes::Keyed;

type KeyMap = Keyed<Either<String, LuaTable>>;

/// The key maps added, oldest first (a registry table).
fn maps(lua: &Lua) -> LuaResult<LuaTable> {
    const KEY: &str = "user.keymaps";
    if let Some(t) = lua.named_registry_value::<Option<LuaTable>>(KEY)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    lua.set_named_registry_value(KEY, &t)?;
    Ok(t)
}

/// The action bound to the first of `names` found, newest map first. A
/// binding table without an `action` ends the search with none.
pub fn action(lua: &Lua, names: &[String]) -> LuaResult<Option<String>> {
    let maps = maps(lua)?;
    for i in (1..=maps.raw_len()).rev() {
        let map: LuaTable = maps.raw_get(i)?;
        for name in names {
            match map.get::<LuaValue>(name.as_str())? {
                LuaValue::Nil | LuaValue::Boolean(false) => {}
                LuaValue::Table(entry) => return entry.get("action"),
                entry => return Option::<String>::from_lua(entry, lua),
            }
        }
    }
    Ok(None)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// Adds a key map: key name to a console command or `{ action = command }`.
        /// It is looked in before the ones added earlier.
        IN_AddKeyMapTable = |lua, key_map_table: Option<KeyMap>| {
            match key_map_table {
                Some(map) => maps(lua)?.raw_push(map.0),
                None => Ok(()),
            }
        })?;
    lua_fn!(lua, global,
        /// Removes the table given to `IN_AddKeyMapTable`.
        IN_RemoveKeyMapTable = |lua, key_map_table: Option<KeyMap>| {
            let Some(map) = key_map_table else { return Ok(()) };
            let maps = maps(lua)?;
            for i in (1..=maps.raw_len()).rev() {
                if maps.raw_get::<LuaTable>(i)? == map.0 {
                    maps.raw_remove(i)?;
                }
            }
            Ok(())
        })?;
    lua_fn!(lua, global, IN_ClearKeyMap = |lua, ()| maps(lua)?.clear())?;
    Ok(())
}
