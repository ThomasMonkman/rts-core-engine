//! The active build template (`UIBuildTemplate`): `{ width, height,
//! { blueprint, id, x, z }, ... }`, x / z offsets from the cursor. It is
//! placed in build mode, every structure at once (render/frame.rs). FAF
//! changes the table it gets (rotating it) and sets it again, so the engine
//! keeps its own copy and hands out a new table each time. Render-thread only.

use mlua::prelude::*;

use crate::lua_api::lua_fn;
use crate::lua_api::shapes::{List, Loose};

/// One structure of a template.
#[derive(Clone, Debug, Default)]
struct Entry {
    blueprint: Option<String>,
    /// FAF's order of the structures.
    id: Option<f64>,
    x: Option<f64>,
    z: Option<f64>,
}

#[derive(Clone, Debug, Default)]
struct Template {
    width: Option<f64>,
    height: Option<f64>,
    /// By position from the template's third entry; `None` where the
    /// table had something other than a structure.
    entries: Vec<Option<Entry>>,
}

/// The active template (app data of the user VM).
#[derive(Default)]
struct Active(Option<Template>);

fn active(lua: &Lua) -> mlua::AppDataRefMut<'_, Active> {
    if lua.app_data_ref::<Active>().is_none() {
        lua.set_app_data(Active::default());
    }
    lua.app_data_mut::<Active>().expect("just set")
}

fn read(lua: &Lua, t: &LuaTable) -> LuaResult<Template> {
    let num = |t: &LuaTable, i: i64| -> LuaResult<Option<f64>> { lua.coerce_number(t.get(i)?) };
    let mut entries = Vec::new();
    for i in 3..=t.raw_len() as i64 {
        entries.push(match t.get::<LuaValue>(i)? {
            LuaValue::Table(e) => Some(Entry {
                blueprint: match e.get::<LuaValue>(1)? {
                    LuaValue::String(s) => Some(s.to_string_lossy()),
                    _ => None,
                },
                id: num(&e, 2)?,
                x: num(&e, 3)?,
                z: num(&e, 4)?,
            }),
            _ => None,
        });
    }
    Ok(Template { width: num(t, 1)?, height: num(t, 2)?, entries })
}

fn write(lua: &Lua, template: &Template) -> LuaResult<LuaTable> {
    let t = lua.create_table_with_capacity(template.entries.len() + 2, 0)?;
    t.raw_set(1, template.width)?;
    t.raw_set(2, template.height)?;
    for (i, e) in template.entries.iter().enumerate() {
        let Some(e) = e else { continue };
        let entry = lua.create_table_with_capacity(4, 0)?;
        entry.raw_set(1, e.blueprint.as_deref())?;
        entry.raw_set(2, e.id)?;
        entry.raw_set(3, e.x)?;
        entry.raw_set(4, e.z)?;
        t.raw_set(i + 3, entry)?;
    }
    Ok(t)
}

/// The active template's structures (lower-case blueprint, x / z offset
/// from the cursor), if one with any is set.
pub fn structures(lua: &Lua) -> Option<Vec<(String, f32, f32)>> {
    let active = active(lua);
    let out: Vec<_> = active.0.as_ref()?.entries.iter().flatten().filter_map(|e| Some((e.blueprint.as_ref()?.to_lowercase(), e.x? as f32, e.z? as f32))).collect();
    (!out.is_empty()).then_some(out)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Active::default());
    lua_fn!(lua, global,
        /// Makes a copy of `template` the one placed in build mode (anything
        /// but a table: an empty one).
        SetActiveBuildTemplate = |lua, template: Loose<List<LuaValue>>| {
            let template = match template.into_inner() {
                LuaValue::Table(t) => read(lua, &t)?,
                _ => Template::default(),
            };
            active(lua).0 = Some(template);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// A copy of the active template; an empty table when there is none.
        GetActiveBuildTemplate = |lua, ()| {
            let template = active(lua).0.clone().unwrap_or_default();
            write(lua, &template).map(List::<LuaValue>::new)
        })?;
    lua_fn!(lua, global,
        ClearBuildTemplates = |lua, ()| {
            active(lua).0 = None;
            Ok(())
        })?;
    Ok(())
}
