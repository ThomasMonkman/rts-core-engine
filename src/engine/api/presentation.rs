use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::{moho, script_object};
use crate::engine::sim::presentation::{Decal, Presentation, Splat};
use crate::lua_api::{Obj, lua_fn, lua_method};

/// `_c_object` of a `moho.CDecalHandle`.
#[derive(Clone, Copy, Debug)]
pub struct DecalHandle(pub u32);
impl LuaUserData for DecalHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Presentation::default());
    lua_fn!(lua, global,
        /// Paints a decal at `pos` (an `{x, y, z}` table). `duration` seconds
        /// (<= 0 is permanent: it lives until `Destroy`); `army` < 0 means no army. Returns the decal handle.
        CreateDecal = |lua, (pos, heading, tex1, tex2, kind, sx, sz, lod, duration, army, _fidelity): (shapes::Vector, f64, String, String, String, f64, f64, f64, f64, i64, Option<f64>)| {
                let decal = Decal {
                    position: [pos.raw_get(1)?, pos.raw_get(2)?, pos.raw_get(3)?],
                    heading,
                    textures: [tex1, tex2],
                    kind,
                    size: [sx, sz],
                    lod,
                    duration,
                    army: (army >= 0).then_some(army as u32),
                    created: crate::engine::api::sim_time::current_tick(lua),
                };
                let permanent = decal.duration <= 0.0;
                let id = lua.app_data_mut::<Presentation>().expect("Presentation").add_decal(decal);
                let obj = script_object::create(lua, DecalHandle(id), moho::class_table(lua, "CDecalHandle")?)?;
                // Permanent decals (tarmacs) live until Lua destroys them, often
                // from a weak-valued TrashBag: keep the handle alive meanwhile
                // (timed ones end by themselves).
                if permanent {
                    decal_objects(lua)?.raw_set(id, obj.clone())?;
                }
                Ok(Obj::<moho::CDecalHandle>::new(obj))
        })?;
    lua_method!(lua, moho::CDecalHandle, Destroy = |lua, this: Obj<moho::CDecalHandle>| {
        let id = script_object::handle::<DecalHandle>(&this)?.0;
        lua.app_data_mut::<Presentation>().expect("Presentation").remove_decal(id);
        decal_objects(lua)?.raw_set(id, LuaNil)?;
        Ok(())
    })?;
    lua_fn!(lua, global,
        /// Paints a splat (a ground texture) at `pos` (an `{x, y, z}` table);
        /// `army` < 0 means no army.
        CreateSplat = |lua, (pos, heading, texture, sx, sz, lod, duration, army, _fidelity): (shapes::Vector, f64, String, f64, f64, f64, f64, i64, Option<f64>)| {
                let splat = Splat {
                    position: [pos.raw_get(1)?, pos.raw_get(2)?, pos.raw_get(3)?],
                    heading,
                    texture,
                    size: [sx, sz],
                    lod,
                    duration,
                    army: (army >= 0).then_some(army as u32),
                    created: crate::engine::api::sim_time::current_tick(lua),
                };
                lua.app_data_mut::<Presentation>().expect("Presentation").add_splat(splat);
                Ok(())
        })?;
    lua_fn!(lua, global,
        /// Tread marks: a splat under `bone` plus `offset` (the entity's frame),
        /// facing the entity's heading.
        CreateSplatOnBone = |lua, (entity, offset, bone, texture, sx, sz, lod, duration, army): (Obj<moho::Entity>, Option<shapes::Vector>, shapes::Bone, String, f64, f64, f64, f64, i64)| {
                let at = crate::engine::api::entity::bone_position(lua, &entity, Some(bone.into_inner()))?;
                let id = crate::engine::api::entity::entity_id(&entity)?;
                let Some(orientation) = crate::engine::api::entity::world(lua).get(id).map(|e| e.orientation) else { return Ok(()) };
                let offset = match offset {
                    Some(o) => crate::engine::sim::math::FxVec3::from_f64(o.raw_get::<Option<f64>>(1)?.unwrap_or(0.0), o.raw_get::<Option<f64>>(2)?.unwrap_or(0.0), o.raw_get::<Option<f64>>(3)?.unwrap_or(0.0)),
                    None => crate::engine::sim::math::FxVec3::default(),
                };
                let (x, y, z) = (at + orientation.rotate(offset)).to_f64();
                let splat = Splat {
                    position: [x, y, z],
                    heading: orientation.heading().to_num::<f64>(),
                    texture,
                    size: [sx, sz],
                    lod,
                    duration,
                    army: (army >= 0).then_some(army as u32),
                    created: crate::engine::api::sim_time::current_tick(lua),
                };
                lua.app_data_mut::<Presentation>().expect("Presentation").add_splat(splat);
                Ok(())
        })
}

/// Strong references to permanent decals' handles, by id (moho keeps its
/// script objects alive; FAF's TrashBags only hold them weakly).
fn decal_objects(lua: &Lua) -> LuaResult<LuaTable> {
    if let Ok(t) = lua.named_registry_value::<LuaTable>("sim.decal_objects") {
        return Ok(t);
    }
    let t = lua.create_table()?;
    lua.set_named_registry_value("sim.decal_objects", t.clone())?;
    Ok(t)
}
