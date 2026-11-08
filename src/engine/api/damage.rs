//! `Damage`, `DamageArea`, `DamageRing`.
//!
//! The engine only routes damage: it calls the target's `OnDamage(instigator,
//! amount, direction, damageType)` and FAF's Lua adjusts health, knocks trees
//! over, etc. Semantics follow FAF's own Lua re-implementation in
//! `/lua/sim/DamageArea.lua`: units and props whose collision shape the sphere
//! (or ring) touches are hit, in id order, units first; the instigator only if
//! `damageSelf`; allies of the instigator's army only if `damageFriendly`.

use crate::engine::api::moho;
use crate::lua_api::{Obj, shapes};
use mlua::prelude::*;

use crate::engine::api::entity::{EntityHandle, object, vector, world};
use crate::engine::api::callbacks::EntityCallbacks;
use crate::engine::api::script_object;
use crate::lua_api::lua_fn;
use crate::engine::sim::army::{Alliance, Armies};
use crate::engine::sim::entity::{EntityId, Indexed};
use crate::engine::sim::math::{Fx, FxVec3};

/// Entity id of a script object, `None` for nil / non-entities.
fn entity_of(v: &LuaValue) -> Option<EntityId> {
    match v {
        LuaValue::Table(t) => script_object::handle::<EntityHandle>(t).ok().map(|h| h.0),
        _ => None,
    }
}

fn read_location(t: &LuaTable) -> LuaResult<FxVec3> {
    Ok(FxVec3::from_f64(t.raw_get(1)?, t.raw_get(2)?, t.raw_get(3)?))
}

fn fx(v: f64) -> Fx {
    crate::engine::sim::math::from_lua(v)
}

/// Moho's `DealDamage` (MohoEngine.dll 0x103074b0): nothing for an amount of
/// 0 (0x103075f4); a unit's armour (`api::armor`), then for a unit damaged
/// by an instigator with an army `OnDamageBy(army + 1)` (0x10307708) and
/// `OnExtraDamageDealt(type)` when the armour multiplied it by 2 or more
/// (0x10307761); the damage stats; and `target:OnDamage(instigator, amount,
/// direction, type)` only when the amount is more than 0 (0x10308181), so
/// negative damage doesn't heal and armour that takes it to 0 (the
/// `Experimental` armour's `ExperimentalFootfall`) runs no `OnDamage`.
/// Direction = unit vector from `location` to the target (zero if they
/// coincide). Moho then divides by 1 + the target army's handicap, set only
/// by the `DebugSetArmyHandicap` console command (0x103076b6): ours has
/// none, so the divisor is 1, as in every normal game.
fn deal(lua: &Lua, instigator: &LuaValue, location: FxVec3, target: EntityId, amount: f64, kind: &LuaValue) -> LuaResult<()> {
    if amount == 0.0 {
        return Ok(());
    }
    let Some((pos, is_unit)) = world(lua).get(target).filter(|e| !e.destroyed).map(|e| (e.position(), e.unit.is_some())) else { return Ok(()) };
    let source = entity_of(instigator);
    let raw = amount;
    // Units take damage through their armor (`api::armor`).
    let amount = if is_unit {
        let kind = match kind {
            LuaValue::String(s) => s.to_str()?.to_string(),
            _ => "Normal".to_string(),
        };
        amount * crate::engine::api::armor::mult(lua, target, &kind)?
    } else {
        amount
    };
    let obj = object(lua, target)?;
    if is_unit && let Some(obj) = &obj {
        let army = source.and_then(|id| world(lua).get(id).and_then(|e| e.army));
        if amount > 0.0 && let Some(army) = army {
            crate::engine::api::callbacks::UnitCallbacks::on_damage_by(lua, obj, "unit", army + 1)?;
        }
        if amount / raw >= 2.0 {
            crate::engine::api::callbacks::UnitCallbacks::on_extra_damage_dealt(lua, obj, "unit", shapes::DamageType(kind.clone()))?;
        }
    }
    if amount != 0.0 {
        crate::engine::api::unit_stats::damage(lua, source, target, amount);
    }
    if amount <= 0.0 {
        return Ok(());
    }
    let Some(obj) = obj else { return Ok(()) };
    let dir = (pos - location).normalized().unwrap_or_default();
    let dir = vector(lua, dir)?;
    EntityCallbacks::on_damage(lua, &obj, "entity", shapes::Instigator(instigator.clone()), amount, dir, shapes::DamageType(kind.clone()))
}

#[allow(clippy::too_many_arguments)]
fn damage_shell(
    lua: &Lua,
    instigator: LuaValue,
    location: LuaTable,
    min_r: f64,
    max_r: f64,
    amount: f64,
    kind: LuaValue,
    friendly: bool,
    damage_self: bool,
) -> LuaResult<()> {
    let center = read_location(&location)?;
    let source = entity_of(&instigator);
    let (units, props, source_army) = {
        let w = world(lua);
        // Both hit what their spheres touch, as moho's do: a ring skips
        // anything its inner sphere touches.
        let (min_r, max_r) = (fx(min_r), fx(max_r));
        let units = w.touching_shell(Indexed::Units, center, min_r, max_r);
        let props = w.touching_shell(Indexed::Props, center, min_r, max_r);
        (units, props, source.and_then(|id| w.get(id)).and_then(|e| e.army))
    };
    // Snapshot who may be hit before any callback runs (a callback may move,
    // kill or change the alliance of what follows; moho gathers up front too).
    let targets: Vec<EntityId> = {
        let w = world(lua);
        let armies = lua.app_data_ref::<Armies>();
        units
            .into_iter()
            .filter(|&id| {
                if Some(id) == source {
                    return damage_self;
                }
                let army = w.get(id).and_then(|e| e.army);
                match (friendly, source_army, army, &armies) {
                    (false, Some(from), Some(to), Some(a)) => a.alliance(from, to) != Alliance::Ally,
                    _ => true,
                }
            })
            .chain(props)
            .collect()
    };
    for id in targets {
        deal(lua, &instigator, center, id, amount, &kind)?;
    }
    Ok(())
}

/// `Damage`, `DamageArea` and `DamageRing` raise on an amount of 0
/// (MohoEngine.dll 0x10309a44, 0x10309d35, 0x1030a078; FAF's callers skip
/// zero damage themselves).
fn check_amount(amount: f64) -> LuaResult<()> {
    if amount == 0.0 { Err(LuaError::runtime("0 damage specified.")) } else { Ok(()) }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// Deprecated in FAF and used only by a projectile no unit fires
        /// (`ShellTankTerran01`); what moho did with it isn't known, so it does
        /// nothing rather than raise.
        MetaImpact = |_, _args: LuaMultiValue| Ok(()))?;
    lua_fn!(lua, global,
        /// Deals `amount` damage of type `kind` to `target` (`OnDamage`).
        /// An entity never damages itself through `Damage` (FAF's DamageArea.lua
        /// works around exactly this for `damageSelf`).
        Damage = |lua, (instigator, location, target, amount, kind): (shapes::Instigator, shapes::Vector, Obj<moho::Entity>, f64, shapes::DamageType)| {
            check_amount(amount)?;
            let Some(target_id) = entity_of(&LuaValue::Table(target.into_table())) else { return Ok(()) };
            if entity_of(&instigator) == Some(target_id) {
                return Ok(());
            }
            deal(lua, &instigator, read_location(&location)?, target_id, amount, &kind)
        })?;
    lua_fn!(lua, global,
        /// Damages every unit and prop within `radius` of `location`, units
        /// first, in id order. The instigator only if `damageSelf`; its allies
        /// only if `damageFriendly` (both default false).
        DamageArea = |lua, (instigator, location, radius, amount, kind, damage_friendly, damage_self): (shapes::Instigator, shapes::Vector, f64, f64, shapes::DamageType, Option<bool>, Option<bool>)| {
            check_amount(amount)?;
            damage_shell(lua, instigator.into_inner(), location.into_inner(), 0.0, radius, amount, kind.into_inner(), damage_friendly.unwrap_or(false), damage_self.unwrap_or(false))
        })?;
    lua_fn!(lua, global,
        /// Like `DamageArea`, but hits only `min_radius` < distance <= `max_radius`.
        DamageRing = |lua, (instigator, location, min_radius, max_radius, amount, kind, damage_friendly, damage_self): (shapes::Instigator, shapes::Vector, f64, f64, f64, shapes::DamageType, Option<bool>, Option<bool>)| {
            check_amount(amount)?;
            damage_shell(lua, instigator.into_inner(), location.into_inner(), min_radius, max_radius, amount, kind.into_inner(), damage_friendly.unwrap_or(false), damage_self.unwrap_or(false))
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::{entity, moho};
    use crate::engine::sim::entity::EntityKind;
    use crate::engine::sim::math::FxQuat;

    /// Spawns a script entity named `name` that logs `OnDamage` into `hits`.
    fn spawn(lua: &Lua, name: &str, kind: EntityKind, army: Option<u32>, x: f64) -> LuaTable {
        let obj: LuaTable = lua
            .load(format!(
                "return {{ name = '{name}', OnDamage = function(self, inst, amount, dir, kind)
                    table.insert(hits, self.name .. ':' .. amount .. ':' .. math.floor(dir[1] * 1000 + 0.5) / 1000) end }}"
            ))
            .eval()
            .unwrap();
        entity::bind_script_entity(lua, &obj, kind, army, FxVec3::from_f64(x, 0.0, 0.0), FxQuat::IDENTITY).unwrap();
        lua.globals().set(name, obj.clone()).unwrap();
        obj
    }

    #[test]
    fn area_and_ring_filters_and_order() {
        let lua = Lua::new();
        moho::register(&lua).unwrap();
        entity::register(&lua).unwrap();
        register(&lua).unwrap();
        lua.set_named_registry_value("vector_metatable", lua.create_table().unwrap()).unwrap();
        let mut armies = Armies::default();
        armies.set_alliance_one_way(1, 2, Alliance::Ally);
        lua.set_app_data(armies);
        lua.globals().set("hits", lua.create_table().unwrap()).unwrap();
        spawn(&lua, "P", EntityKind::Prop, None, 3.0); // prop: lowest id, still after units
        spawn(&lua, "I", EntityKind::Unit, Some(1), 0.0);
        spawn(&lua, "E", EntityKind::Unit, Some(3), -5.0);
        spawn(&lua, "A", EntityKind::Unit, Some(2), 4.0);
        spawn(&lua, "N", EntityKind::Unit, Some(3), 0.05);
        spawn(&lua, "F", EntityKind::Unit, Some(3), 20.0);
        let run = |code: &str| -> String {
            lua.load(format!("hits = {{}} {code} return table.concat(hits, ' ')")).eval().unwrap()
        };
        assert_eq!(run("DamageRing(I, {0, 0, 0}, 0.1, 10, 7, 'Normal', false, false)"), "E:7:-1 P:7:1");
        assert_eq!(
            run("DamageArea(I, {0, 0, 0}, 10, 2, 'Normal', true, true)"),
            "I:2:0 E:2:-1 A:2:1 N:2:1 P:2:1",
            "self and friendly included; units (id order) then props"
        );
        assert_eq!(run("DamageArea(nil, {0, 0, 0}, 4.5, 1, 'Force')"), "I:1:0 A:1:1 N:1:1 P:1:1", "no instigator: no filter");
        assert_eq!(run("Damage(I, {0, 0, 0}, I, 5, 'Normal') Damage(I, {0, 0, 0}, F, 5, 'Normal')"), "F:5:1");
    }
}
