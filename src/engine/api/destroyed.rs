//! `IsDestroyed(obj)` for any engine object.

use mlua::prelude::*;

use crate::engine::api::army::ArmyHandle;
use crate::engine::api::effect::EffectHandle;
use crate::engine::api::entity::EntityHandle;
use crate::engine::api::manipulator::ManipulatorHandle;
use crate::engine::api::platoon::PlatoonHandle;
use crate::engine::api::script_object::C_OBJECT;
use crate::engine::api::weapon::WeaponHandle;
use crate::engine::sim::entity::EntityWorld;
use crate::engine::sim::manipulator::Manipulators;
use crate::engine::sim::platoon::Platoons;
use crate::engine::sim::presentation::Presentation;
use crate::lua_api::{Obj, lua_fn, lua_method};
use crate::engine::api::moho;

/// Whether entity `id`'s object still exists. Moho's `IsDestroyed` tests
/// the script object's C pointer (MohoEngine.dll 0x100b9d50 -> 0x100ba4d0),
/// which stays set after `Destroy` until the object is freed after the beat
/// that ran its `OnDestroy` (`EntityWorld::free`); so a destroyed entity is
/// not `IsDestroyed` for the rest of that beat, while `BeenDestroyed` is.
fn entity_alive(lua: &Lua, id: u32) -> bool {
    lua.app_data_ref::<EntityWorld>()
        .and_then(|w| w.get(id).map(|e| !e.gone))
        .unwrap_or(false)
}

/// `BeenDestroyed`: `Destroy` has been called (moho reads the destroy-queued
/// flag, 0x102746d0 tests +0x1b9).
fn entity_destroy_queued(lua: &Lua, this: &LuaTable) -> LuaResult<bool> {
    let LuaValue::UserData(ud) = this.raw_get::<LuaValue>(C_OBJECT)? else { return Ok(true) };
    let Ok(h) = ud.borrow::<EntityHandle>() else { return is_destroyed(lua, &LuaValue::Table(this.clone())) };
    Ok(lua.app_data_ref::<EntityWorld>().and_then(|w| w.get(h.0).map(|e| e.destroyed)).unwrap_or(true))
}

/// True unless `v` is a live engine object (non-engine values count as destroyed).
pub fn is_destroyed(lua: &Lua, v: &LuaValue) -> LuaResult<bool> {
    let LuaValue::Table(t) = v else { return Ok(true) };
    let LuaValue::UserData(ud) = t.raw_get::<LuaValue>(C_OBJECT)? else { return Ok(true) };
    if let Ok(h) = ud.borrow::<EntityHandle>() {
        return Ok(!entity_alive(lua, h.0));
    }
    if let Ok(h) = ud.borrow::<WeaponHandle>() {
        return Ok(!entity_alive(lua, h.unit));
    }
    if let Ok(h) = ud.borrow::<ManipulatorHandle>() {
        return Ok(lua.app_data_ref::<Manipulators>().is_none_or(|m| m.get(h.0).is_none()));
    }
    if let Ok(h) = ud.borrow::<PlatoonHandle>() {
        // A platoon whose last unit left is gone for scripts already, though
        // it ends between thread resumes / at compaction.
        return Ok(lua.app_data_ref::<Platoons>().is_none_or(|p| p.get(h.0).is_none() || p.is_doomed(h.0)));
    }
    if let Ok(h) = ud.borrow::<EffectHandle>() {
        return Ok(lua.app_data_ref::<Presentation>().is_none_or(|p| !p.effects().contains_key(&h.0)));
    }
    Ok(!ud.borrow::<ArmyHandle>().is_ok())
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// False for a live engine object; anything else (nil, a plain table, a freed object) counts as destroyed.
        /// A destroyed entity stays live until the end of the beat that runs its `OnDestroy`.
        IsDestroyed = |lua, v: LuaValue| is_destroyed(lua, &v))?;
    lua_method!(lua, moho::Entity,
        /// True once `Destroy` has been called (its `OnDestroy` may still be
        /// to come, at the end of the beat).
        BeenDestroyed = |lua, this: Obj<moho::Entity>| entity_destroy_queued(lua, this.table()))?;
    lua_method!(lua, moho::Manipulator,
        /// True once the manipulator is destroyed. Moho has it on the slide
        /// manipulator only (SupremeCommander.exe); ours is on every one.
        BeenDestroyed = |lua, this: Obj<moho::Manipulator>| is_destroyed(lua, &LuaValue::Table(this.into_table())))?;
    lua_method!(lua, moho::IEffect,
        /// True once the effect is destroyed. Not moho's (it has it on entities,
        /// weapons and slide manipulators): ours, harmlessly wider.
        BeenDestroyed = |lua, this: Obj<moho::IEffect>| is_destroyed(lua, &LuaValue::Table(this.into_table())))?;
    Ok(())
}
