//! `Damage`, `DamageArea`, `DamageRing`.
//!
//! The engine only routes damage: it calls the target's `OnDamage(instigator,
//! amount, direction, damageType)` and FAF's Lua adjusts health, knocks trees
//! over, etc. Semantics follow FAF's own Lua re-implementation in
//! `/lua/sim/DamageArea.lua`: units and props whose collision shape the sphere
//! (or ring) touches are hit, in id order, units first; the instigator only if
//! `damageSelf`; allies of the instigator's army only if `damageFriendly`.
//! Shield bubbles the blast touches from outside absorb part of it, as in
//! moho (`shields_absorbing`): units under them take the rest, then each
//! shield takes what it absorbed. `NOSPLASHDAMAGE` entities are never hit.

use crate::engine::api::moho;
use crate::lua_api::{Arity, Obj, shapes};
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
/// `direction` makes what `OnDamage` gets from the target's position. Moho then divides by 1 + the target army's handicap, set only
/// by the `DebugSetArmyHandicap` console command (0x103076b6): ours has
/// none, so the divisor is 1, as in every normal game.
fn deal(lua: &Lua, instigator: &LuaValue, direction: impl FnOnce(FxVec3) -> FxVec3, target: EntityId, amount: f64, kind: &LuaValue) -> LuaResult<()> {
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
    let dir = vector(lua, direction(pos))?;
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
    let (units, shield_targets, props, source_army) = {
        let w = world(lua);
        // Both hit what their spheres touch, as moho's do: a ring skips
        // anything its inner sphere touches.
        let (min_r, max_r) = (fx(min_r), fx(max_r));
        let units = w.touching_shell(Indexed::Units, center, min_r, max_r);
        let props = w.touching_shell(Indexed::Props, center, min_r, max_r);
        // Shield bubbles are gathered like any entity: a blast from under a
        // bubble damages it as well as what's under it (FAF probe
        // probe_splashwater: 100 to the tank and 100 to the shield). From
        // outside, the bubble's own share covers its position, so this hit
        // nets to nothing and it takes only what it absorbed. Only bubbles
        // with a shape: FAF clears it while one is down.
        let shields = shield_entities_touched(&w, center, min_r, max_r);
        (units, shields, props, source.and_then(|id| w.get(id)).and_then(|e| e.army))
    };
    // `NOSPLASHDAMAGE` entities take no area or ring damage (MohoEngine.dll
    // 0x10308396, 0x10308848; no stock FAF unit has it, mods may). No
    // categories yet (no blueprints loaded): nothing can carry it.
    let no_splash = lua.app_data_ref::<std::sync::Arc<crate::engine::sim::category::CategoryDomain>>().and_then(|d| d.named.get("NOSPLASHDAMAGE").cloned());
    // Snapshot who may be hit before any callback runs (a callback may move,
    // kill or change the alliance of what follows; moho gathers up front too).
    let targets: Vec<EntityId> = {
        let w = world(lua);
        let armies = lua.app_data_ref::<Armies>();
        let splashable = |id: EntityId| {
            let Some(e) = w.get(id) else { return false };
            !no_splash.as_ref().is_some_and(|set| e.category_index.is_some_and(|c| set.contains(c)))
        };
        units
            .into_iter()
            .chain(shield_targets)
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
            .filter(|&id| splashable(id))
            .collect()
    };
    let shields = shields_absorbing(lua, &instigator, center, fx(max_r), amount, &kind, friendly, source_army)?;
    for id in targets {
        // A target under a shield that absorbs part of the blast takes the
        // rest (0x103071b0: every absorbing shield whose shape holds the
        // target's position takes its share off), nothing when that is
        // 0 or less (0x10308447).
        let mut left = amount;
        if !shields.is_empty() {
            let w = world(lua);
            if let Some(pos) = w.get(id).map(|e| e.position()) {
                for s in &shields {
                    if w.get(s.id).is_some_and(|e| crate::engine::sim::collision::sphere_touches(pos, Fx::ZERO, e.position(), e.orientation, &e.collision_shape())) {
                        left -= s.absorbed;
                    }
                }
            }
        }
        if left <= 0.0 {
            continue;
        }
        // Moho's area damage passes the target's position minus the centre,
        // not normalised (MohoEngine.dll 0x103088da..0x10308919 stores it in
        // the damage's direction, +0x80, which `DealDamage` hands to
        // `OnDamage` at 0x103081c7; the same at 0x10308470 and 0x10308590).
        deal(lua, &instigator, |pos| pos - center, id, left, &kind)?;
    }
    // Then each absorbing shield takes what it absorbed, after everything
    // else (0x10308547..0x1030869f).
    for s in shields {
        deal(lua, &instigator, |pos| pos - center, s.id, s.absorbed, &kind)?;
    }
    Ok(())
}

/// Shield entities with a collision shape that the shell between `min_r`
/// (exclusive; 0 = a full sphere) and `r` around `center` touches, in id
/// order.
fn shield_entities_touched(w: &crate::engine::sim::entity::EntityWorld, center: FxVec3, min_r: Fx, r: Fx) -> Vec<EntityId> {
    use crate::engine::sim::collision::sphere_touches;
    use crate::engine::sim::entity::{CollisionShape, EntityKind};
    let mut ids: Vec<EntityId> = w
        .iter()
        .filter(|e| !e.destroyed && e.kind == EntityKind::Shield)
        .filter(|e| {
            let shape = e.collision_shape();
            !matches!(shape, CollisionShape::None)
                && !(min_r > Fx::ZERO && sphere_touches(center, min_r, e.position(), e.orientation, &shape))
                && sphere_touches(center, r, e.position(), e.orientation, &shape)
        })
        .map(|e| e.id)
        .collect();
    ids.sort_unstable();
    ids
}

/// A shield bubble taking part of an area blast.
struct Absorbing {
    id: EntityId,
    absorbed: f64,
}

/// Moho's shield pass before area damage (MohoEngine.dll 0x10307220, from
/// the area and ring paths 0x10308240 / 0x10308720): each shield entity with
/// a collision shape (a bubble that is down, or a personal shield, has none
/// in FAF) that isn't the instigator's ally (unless `friendly`), whose
/// sphere doesn't hold the blast's centre (0.1 inside its surface,
/// 0x1030730b: a blast from under the bubble isn't stopped by it), and
/// that the blast's sphere touches, is asked
/// `OnGetDamageAbsorption(instigator, amount, type)` (0x1030aa50; 0 without
/// one); those answering more than 0 absorb that much. Only for positive
/// damage (0x103073e8; what moho does with negative area damage and
/// shields isn't traced, ours ignores them). Shields in id order.
#[allow(clippy::too_many_arguments)]
fn shields_absorbing(
    lua: &Lua,
    instigator: &LuaValue,
    center: FxVec3,
    radius: Fx,
    amount: f64,
    kind: &LuaValue,
    friendly: bool,
    source_army: Option<u32>,
) -> LuaResult<Vec<Absorbing>> {
    use crate::engine::sim::collision::sphere_touches;
    use crate::engine::sim::entity::{CollisionShape, EntityKind};
    if amount <= 0.0 {
        return Ok(Vec::new());
    }
    let candidates: Vec<EntityId> = {
        let w = world(lua);
        let armies = lua.app_data_ref::<Armies>();
        let mut ids: Vec<EntityId> = w
            .iter()
            .filter(|e| !e.destroyed && e.kind == EntityKind::Shield)
            .filter(|e| match (friendly, source_army, e.army, &armies) {
                (false, Some(from), Some(to), Some(a)) => a.alliance(from, to) != Alliance::Ally,
                _ => true,
            })
            .filter(|e| {
                let shape = e.collision_shape();
                let outside = match shape {
                    CollisionShape::None => return false,
                    CollisionShape::Sphere { center: c, radius: r } => {
                        let inner = CollisionShape::Sphere { center: c, radius: r - Fx::from_num(0.1) };
                        !sphere_touches(center, Fx::ZERO, e.position(), e.orientation, &inner)
                    }
                    CollisionShape::Box { .. } => true,
                };
                outside && sphere_touches(center, radius, e.position(), e.orientation, &shape)
            })
            .map(|e| e.id)
            .collect();
        ids.sort_unstable();
        ids
    };
    let mut out = Vec::new();
    for id in candidates {
        let Some(obj) = object(lua, id)? else { continue };
        let Some(f) = obj.get::<Option<LuaFunction>>("OnGetDamageAbsorption")? else { continue };
        let absorbed = match f.call::<LuaValue>((obj.clone(), instigator.clone(), amount, kind.clone())) {
            Ok(LuaValue::Number(n)) => n,
            Ok(LuaValue::Integer(n)) => n as f64,
            Ok(_) => 0.0,
            Err(e) => {
                tracing::warn!(target: "lua", "Error running OnGetDamageAbsorption script in shield: {}", crate::lua_api::errors::Explained(lua, &e));
                0.0
            }
        };
        if absorbed > 0.0 {
            out.push(Absorbing { id, absorbed });
        }
    }
    Ok(out)
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
            // The target's position minus `location`, not normalised
            // (MohoEngine.dll 0x103099e0..0x10309a39): FAF's shields offset
            // their impact flash by its length, onto the bubble's surface.
            let location = read_location(&location)?;
            deal(lua, &instigator, |pos| pos - location, target_id, amount, &kind)
        })?;
    lua_fn!(lua, global,
        /// Damages every unit and prop within `radius` of `location`, units
        /// first, in id order. The instigator only if `damageSelf`; its allies
        /// only if `damageFriendly`. Takes 6 or 7 arguments, an explicit `nil`
        /// counting as one (MohoEngine.dll usage string
        /// "DamageArea(instigator,location,radius,amount,damageType,damageFriendly,[damageSelf])";
        /// the real game raises on 5, FAF probe `probe_splashwater`).
        DamageArea = |lua, Arity((instigator, location, radius, amount, kind, damage_friendly, damage_self)): Arity<6, 7, (shapes::Instigator, shapes::Vector, f64, f64, shapes::DamageType, bool, Option<bool>)>| {
            check_amount(amount)?;
            damage_shell(lua, instigator.into_inner(), location.into_inner(), 0.0, radius, amount, kind.into_inner(), damage_friendly, damage_self.unwrap_or(false))
        })?;
    lua_fn!(lua, global,
        /// Like `DamageArea`, but hits only `min_radius` < distance <= `max_radius`.
        /// Takes 7 or 8 arguments (usage string
        /// "DamageRing(instigator,location,minRadius,maxRadius,amount,damageType,damageFriendly,[damageSelf])").
        DamageRing = |lua, Arity((instigator, location, min_radius, max_radius, amount, kind, damage_friendly, damage_self)): Arity<7, 8, (shapes::Instigator, shapes::Vector, f64, f64, f64, shapes::DamageType, bool, Option<bool>)>| {
            check_amount(amount)?;
            damage_shell(lua, instigator.into_inner(), location.into_inner(), min_radius, max_radius, amount, kind.into_inner(), damage_friendly, damage_self.unwrap_or(false))
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
        assert_eq!(run("DamageRing(I, {0, 0, 0}, 0.1, 10, 7, 'Normal', false, false)"), "E:7:-5 P:7:3");
        assert_eq!(
            run("DamageArea(I, {0, 0, 0}, 10, 2, 'Normal', true, true)"),
            "I:2:0 E:2:-5 A:2:4 N:2:0.05 P:2:3",
            "self and friendly included; units (id order) then props"
        );
        assert_eq!(run("DamageArea(nil, {0, 0, 0}, 4.5, 1, 'Force', false)"), "I:1:0 A:1:4 N:1:0.05 P:1:3", "no instigator: no filter");
        assert_eq!(run("Damage(I, {0, 0, 0}, I, 5, 'Normal') Damage(I, {0, 0, 0}, F, 5, 'Normal')"), "F:5:20", "not normalised");
        // Moho counts the arguments, an explicit nil included.
        let err = |code: &str| lua.load(code).exec().unwrap_err().to_string();
        assert!(err("DamageArea(nil, {0, 0, 0}, 1, 1, 'Force')").contains("expected between 6 and 7 args, but got 5"));
        assert!(err("DamageArea(nil, {0, 0, 0}, 1, 1, 'Force', false, false, 1)").contains("but got 8"));
        assert!(err("DamageRing(nil, {0, 0, 0}, 0.1, 1, 1, 'Force')").contains("expected between 7 and 8 args, but got 6"));
        assert_eq!(run("DamageArea(nil, {0, 0, 0}, 4.5, 1, 'Force', nil)"), "I:1:0 A:1:4 N:1:0.05 P:1:3", "a nil damageFriendly counts");
    }

    #[test]
    fn shield_bubbles_absorb_area_damage() {
        let lua = Lua::new();
        moho::register(&lua).unwrap();
        entity::register(&lua).unwrap();
        register(&lua).unwrap();
        lua.set_named_registry_value("vector_metatable", lua.create_table().unwrap()).unwrap();
        lua.set_app_data(Armies::default());
        lua.globals().set("hits", lua.create_table().unwrap()).unwrap();
        spawn(&lua, "I", EntityKind::Unit, Some(1), -20.0);
        // A bubble of radius 5 at the origin absorbing 3/4 of what it's
        // offered; one unit under it, one outside it.
        spawn(&lua, "S", EntityKind::Shield, Some(3), 0.0);
        spawn(&lua, "U", EntityKind::Unit, Some(3), 4.0);
        spawn(&lua, "O", EntityKind::Unit, Some(3), 8.0);
        lua.load("moho.entity_methods.SetCollisionShape(S, 'Sphere', 0, 0, 0, 5)
            S.OnGetDamageAbsorption = function(self, inst, amount, kind) return amount * 0.75 end")
            .exec()
            .unwrap();
        let run = |code: &str| -> String {
            lua.load(format!("hits = {{}} {code} return table.concat(hits, ' ')")).eval().unwrap()
        };
        assert_eq!(
            run("DamageArea(I, {6, 0, 0}, 3, 8, 'Normal', false)"),
            "U:2:-2 O:8:2 S:2:-6 S:6:-6",
            "from outside: the unit under the bubble takes the rest, the one outside all of it; the bubble, gathered too, the rest at its own position, then its share"
        );
        assert_eq!(run("DamageRing(I, {6, 0, 0}, 1, 3, 8, 'Normal', false)"), "U:2:-2 O:8:2 S:6:-6", "a ring absorbs the same; its inner sphere touches the bubble, so it isn't a target");
        assert_eq!(run("DamageArea(I, {3, 0, 0}, 3, 8, 'Normal', false)"), "U:8:1 S:8:-3", "from under the bubble: not absorbed; the bubble takes it as well");
        assert_eq!(run("DamageArea(I, {10.5, 0, 0}, 3, 8, 'Normal', false)"), "O:8:-2.5", "a blast that doesn't reach the bubble");
        lua.load("S.OnGetDamageAbsorption = function(self, inst, amount, kind) return amount end").exec().unwrap();
        assert_eq!(run("DamageArea(I, {6, 0, 0}, 3, 8, 'Normal', false)"), "O:8:2 S:8:-6", "fully absorbed: nothing under the bubble");
        lua.load("moho.entity_methods.SetCollisionShape(S, 'None')").exec().unwrap();
        assert_eq!(run("DamageArea(I, {6, 0, 0}, 3, 8, 'Normal', false)"), "U:8:-2 O:8:2", "a bubble that's down absorbs nothing");
    }
}
