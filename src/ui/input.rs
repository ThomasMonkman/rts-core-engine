//! Input state the interface's scripts read and set: the controls
//! capturing all input (modal dialogs), the cursor object, the mouse, the
//! order waypoint under it, and console commands. Render-thread only.

use mlua::prelude::*;

use crate::engine::api::moho;
use crate::lua_api::shapes::{Loose, Shaped, Vector2};
use crate::lua_api::{Obj, lua_fn};
use crate::ui::signatures::HighlightCommand;

/// The controls capturing input, oldest first (a registry table).
fn captures(lua: &Lua) -> LuaResult<LuaTable> {
    const KEY: &str = "user.input_captures";
    if let Some(t) = lua.named_registry_value::<Option<LuaTable>>(KEY)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    lua.set_named_registry_value(KEY, &t)?;
    Ok(t)
}

/// Runs a console command: the interface's own here, the engine's go to
/// the renderer (`ui::console`). A failure is logged, not raised.
fn con_execute(lua: &Lua, command: &str) {
    if let Err(e) = crate::ui::console(lua, command) {
        tracing::warn!(target: "lua", line = command, "ConExecute failed: {e}");
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// `control` takes all input until removed (the last added first).
        AddInputCapture = |lua, control: Loose<Obj<moho::Control>>| {
            match control.into_inner() {
                LuaNil => Ok(()),
                control => captures(lua)?.raw_push(control),
            }
        })?;
    lua_fn!(lua, global,
        RemoveInputCapture = |lua, control: Loose<Obj<moho::Control>>| {
            let captures = captures(lua)?;
            for i in (1..=captures.raw_len()).rev() {
                if captures.raw_get::<LuaValue>(i)? == *control {
                    captures.raw_remove(i)?;
                }
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The control taking all input, nil when none does.
        GetInputCapture = |lua, ()| {
            let captures = captures(lua)?;
            captures.raw_get::<LuaValue>(captures.raw_len()).map(Loose::<Option<Obj<moho::Control>>>::new)
        })?;
    lua_fn!(lua, global, AnyInputCapture = |lua, ()| Ok(captures(lua)?.raw_len() > 0))?;
    lua_fn!(lua, global,
        /// The cursor object FAF made (its textures are kept by `ui::cursor`).
        SetCursor = |lua, cursor: Loose<Option<Obj<moho::Cursor>>>| lua.set_named_registry_value("user.cursor", cursor.into_inner()))?;
    lua_fn!(lua, global, GetCursor = |lua, ()| lua.named_registry_value::<LuaValue>("user.cursor").map(Loose::<Option<Obj<moho::Cursor>>>::new))?;
    lua_fn!(lua, global,
        /// Where the mouse is, in screen pixels.
        GetMouseScreenPos = |lua, ()| {
            let g = lua.globals();
            let at = |key: &str| -> LuaResult<LuaValue> {
                Ok(match g.raw_get::<LuaValue>(key)? {
                    LuaNil | LuaValue::Boolean(false) => LuaValue::Number(0.0),
                    v => v,
                })
            };
            lua.create_sequence_from([at("__engine_mouse_x")?, at("__engine_mouse_y")?]).map(Vector2)
        })?;
    lua_fn!(lua, global,
        /// The command-graph waypoint under the mouse (the engine sets it each
        /// frame), nil when none.
        GetHighlightCommand = |lua, ()| lua.globals().raw_get::<LuaValue>("__engine_highlight_command").map(Loose::<Option<Shaped<HighlightCommand>>>::new))?;
    lua_fn!(lua, global,
        ConExecute = |lua, command: String| {
            con_execute(lua, &command);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// As `ConExecute` (moho also keeps the command in the console's history).
        ConExecuteSave = |lua, command: String| {
            con_execute(lua, &command);
            Ok(())
        })?;
    Ok(())
}
