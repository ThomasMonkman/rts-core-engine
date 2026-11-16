//! Structure adjacency callbacks (moho): when a structure is finished, each
//! same-army structure whose build skirt touches its own gets
//! `OnAdjacentTo(other, trigger)` in both directions (FAF's adjacency
//! bonuses: mass storage by extractors, power by factories, ...); when one
//! is destroyed, `OnNotAdjacentTo(other)` in both directions. Only finished
//! structures count, and an upgrade and the structure it upgrades never
//! hear of each other (dev/probes probe_unit_build G1, G4).

use mlua::prelude::*;

use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::entity;
use crate::lua_api::Obj;
use crate::engine::sim::entity::EntityId;

/// `id` (a structure) is finished: it and each structure touching it learn
/// of each other.
pub fn on_finished(lua: &Lua, id: EntityId) -> LuaResult<()> {
    let mut others = crate::engine::api::build::adjacent_structures(lua, id, false)?;
    others.retain(|&o| !upgrade_pair(lua, id, o));
    let Some(me) = entity::object(lua, id)? else { return Ok(()) };
    for o in others {
        let Some(other) = entity::object(lua, o)? else { continue };
        UnitCallbacks::on_adjacent_to(lua, &me, "unit", Obj::new(other.clone()), Obj::new(me.clone()))?;
        UnitCallbacks::on_adjacent_to(lua, &other, "unit", Obj::new(me.clone()), Obj::new(me.clone()))?;
    }
    Ok(())
}

/// `id` (a structure) is about to be destroyed: it and each structure
/// touching it part ways.
pub fn on_destroying(lua: &Lua, id: EntityId) -> LuaResult<()> {
    let mut others = crate::engine::api::build::adjacent_structures(lua, id, true)?;
    others.retain(|&o| !upgrade_pair(lua, id, o));
    if others.is_empty() {
        return Ok(());
    }
    let Some(me) = entity::object(lua, id)? else { return Ok(()) };
    for o in others {
        let Some(other) = entity::object(lua, o)? else { continue };
        UnitCallbacks::on_not_adjacent_to(lua, &me, "unit", Obj::new(other.clone()))?;
        UnitCallbacks::on_not_adjacent_to(lua, &other, "unit", Obj::new(me.clone()))?;
    }
    Ok(())
}

/// Whether `a` and `b` are an upgrade and the structure it upgrades: they
/// stand at the same spot and one's blueprint upgrades to the other's
/// (`General.UpgradesTo` / `UpgradesFrom`). Read from the blueprints, as
/// the old structure's order is gone by the time it's destroyed.
fn upgrade_pair(lua: &Lua, a: EntityId, b: EntityId) -> bool {
    let (bp_a, bp_b) = {
        let w = entity::world(lua);
        let (Some(ea), Some(eb)) = (w.get(a), w.get(b)) else { return false };
        if ea.position().x != eb.position().x || ea.position().z != eb.position().z {
            return false;
        }
        (ea.cold.blueprint_id.clone(), eb.cold.blueprint_id.clone())
    };
    let general = |bp: &str, key: &str| -> Option<String> {
        let t = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp).ok()?;
        let g = t.get::<Option<LuaTable>>("General").ok()??;
        g.get::<Option<String>>(key).ok()?.map(|s| s.to_lowercase())
    };
    let upgrades = |from: &str, to: &str| general(from, "UpgradesTo").as_deref() == Some(to) || general(to, "UpgradesFrom").as_deref() == Some(from);
    upgrades(&bp_a, &bp_b) || upgrades(&bp_b, &bp_a)
}
