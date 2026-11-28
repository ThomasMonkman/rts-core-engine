//! Interface ground decals (`UserDecal`: FAF's range rings under the
//! cursor): a texture, a centre and a size, drawn on the terrain. The
//! renderer takes them when they change (`UserVm::user_decals`).
//! Render-thread only.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::moho;
use crate::lua_api::shapes::{Loose, Vector, Vector2};
use crate::lua_api::{Obj, lua_fn, lua_method};

/// The field of a decal's script object holding its id.
const ID: &str = "_decal";

/// A decal: drawn once it has a texture.
#[derive(Clone, Debug, PartialEq)]
pub struct Decal {
    pub texture: Option<String>,
    /// Its centre.
    pub position: [f32; 3],
    pub scale: [f32; 3],
}

/// The interface's decals (app data of the user VM).
struct Decals {
    by_id: BTreeMap<u32, Decal>,
    next_id: u32,
    /// Changed since the renderer last took them.
    changed: bool,
}

impl Default for Decals {
    fn default() -> Self {
        Self { by_id: BTreeMap::new(), next_id: 0, changed: true }
    }
}

fn state(lua: &Lua) -> mlua::AppDataRefMut<'_, Decals> {
    if lua.app_data_ref::<Decals>().is_none() {
        lua.set_app_data(Decals::default());
    }
    lua.app_data_mut::<Decals>().expect("just set")
}

/// The textured decals in the order they were made, when any decal changed
/// since the last call (`None`: unchanged).
pub fn take_changed(lua: &Lua) -> Option<Vec<Decal>> {
    let mut s = state(lua);
    if !std::mem::take(&mut s.changed) {
        return None;
    }
    Some(s.by_id.values().filter(|d| d.texture.is_some()).cloned().collect())
}

/// Changes a decal object's decal, if it still has one.
fn edit(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Decal)) -> LuaResult<()> {
    let id = this.raw_get::<Option<u32>>(ID)?.unwrap_or(0);
    let mut s = state(lua);
    if let Some(d) = s.by_id.get_mut(&id) {
        f(d);
        s.changed = true;
    }
    Ok(())
}

/// `v[index] or v[name]`.
fn component(v: &LuaTable, index: i64, name: &str) -> LuaResult<LuaValue> {
    Ok(match v.get::<LuaValue>(index)? {
        LuaValue::Nil | LuaValue::Boolean(false) => v.get(name)?,
        value => value,
    })
}

fn number(v: LuaValue) -> f32 {
    match v {
        LuaValue::Integer(i) => i as f32,
        LuaValue::Number(n) => n as f32,
        _ => 0.0,
    }
}

/// A vector's three numbers (`{x, y, z}` or by name; 0 for one that isn't
/// a number). `None` for nil.
fn vector(v: LuaValue, what: &str) -> LuaResult<Option<[f32; 3]>> {
    match v {
        LuaValue::Nil | LuaValue::Boolean(false) => Ok(None),
        LuaValue::Table(v) => Ok(Some([number(component(&v, 1, "x")?), number(component(&v, 2, "y")?), number(component(&v, 3, "z")?)])),
        other => Err(LuaError::runtime(format!("{what}: expected a vector, got {}", other.type_name()))),
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Decals::default());
    lua_fn!(lua, global,
        /// Gives a new `UserDecal` its decal: at the origin, of size 1, not
        /// drawn until it has a texture.
        _c_CreateDecal = |lua, (decal, _spec): (Obj<moho::UserDecal>, Loose<Option<LuaTable>>)| {
            let id = {
                let mut s = state(lua);
                s.next_id += 1;
                let id = s.next_id;
                s.by_id.insert(id, Decal { texture: None, position: [0.0; 3], scale: [1.0; 3] });
                s.changed = true;
                id
            };
            decal.raw_set(ID, id)
        })?;
    lua_method!(lua, moho::UserDecal,
        /// The texture drawn (nil: none, the decal isn't drawn).
        SetTexture = |lua, (this, texture): (Obj<moho::UserDecal>, Loose<Option<String>>)| {
            let texture = match texture.into_inner() {
                LuaValue::String(s) => Some(s.to_string_lossy()),
                _ => None,
            };
            edit(lua, &this, |d| d.texture = texture)
        })?;
    lua_method!(lua, moho::UserDecal,
        /// Its centre in the world.
        SetPosition = |lua, (this, position): (Obj<moho::UserDecal>, Loose<Option<Vector>>)| {
            let Some(p) = vector(position.into_inner(), "SetPosition")? else { return Ok(()) };
            edit(lua, &this, |d| d.position = p)
        })?;
    lua_method!(lua, moho::UserDecal,
        /// Its centre as a point on the screen: where that is on the ground
        /// (`UnProject`); left where it is when that is nowhere.
        SetPositionByScreen = |lua, (this, position): (Obj<moho::UserDecal>, Loose<Option<Vector2>>)| {
            let screen = match position.into_inner() {
                LuaValue::Nil | LuaValue::Boolean(false) => return Ok(()),
                LuaValue::Table(v) => lua.create_sequence_from([component(&v, 1, "x")?, component(&v, 2, "y")?])?,
                other => return Err(LuaError::runtime(format!("SetPositionByScreen: expected a vector, got {}", other.type_name()))),
            };
            let world: LuaValue = lua.globals().get::<LuaFunction>("UnProject")?.call((LuaNil, screen))?;
            let Some(p) = vector(world, "SetPositionByScreen")? else { return Ok(()) };
            edit(lua, &this, |d| d.position = p)
        })?;
    lua_method!(lua, moho::UserDecal,
        /// Its size in the world (x and z; y is unused).
        SetScale = |lua, (this, scale): (Obj<moho::UserDecal>, Loose<Option<Vector>>)| {
            let Some(s) = vector(scale.into_inner(), "SetScale")? else { return Ok(()) };
            edit(lua, &this, |d| d.scale = s)
        })?;
    lua_method!(lua, moho::UserDecal,
        Destroy = |lua, this: Obj<moho::UserDecal>| {
            let id = this.raw_get::<Option<u32>>(ID)?.unwrap_or(0);
            let mut s = state(lua);
            if s.by_id.remove(&id).is_some() {
                s.changed = true;
            }
            Ok(())
        })?;
    Ok(())
}
