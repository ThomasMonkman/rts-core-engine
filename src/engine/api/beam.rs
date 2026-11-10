//! Collision beams (`moho.CollisionBeamEntity`): the lasers of beam weapons.
//!
//! FAF's beam weapon creates one beam per muzzle (`self.BeamType(spec)`,
//! a `Class(moho.CollisionBeamEntity)`), so the engine's `__init(spec)` makes
//! a script entity owned by the weapon's unit, attached at the muzzle bone
//! (`spec.OtherBone`), then runs its `OnCreate`. While enabled (`Enable`), the
//! beam traces every `spec.CollisionCheckInterval` ticks from the muzzle
//! along the barrel as the turret points now
//! (`WeaponState::barrel_direction`, so a beam sweeps onto a new target and
//! trails a fast one) out to the weapon's
//! `MaximumBeamLength` (else `MaxRadius`) and calls `beam:OnImpact(type, entity)`: the nearest unit hit
//! (`Unit`, `UnitAir`, `UnitUnderwater`; allies of the launcher pass through)
//! or, for a weapon that shoots projectiles (the Cybran TMD's zapper), enemy
//! projectile,
//! else the ground (`Terrain`, or `Water` at the water surface), else `Air`
//! at full length. The beam's far end (where it stopped) is its bone 1:
//! FAF's `DoDamage` reads `self:GetPosition(1)`. Beams are processed in
//! entity id order after weapons.
//!
//! Inferred, not verified: 1-unit terrain sampling, and `Air` impacts when
//! nothing is hit.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::entity::{self, entity_id, object, world};
use crate::engine::api::callbacks::CollisionBeamEntityCallbacks;
use crate::engine::api::{moho, script_object};
use crate::lua_api::shapes::{Bone, Shaped};
use crate::lua_api::{Obj, lua_fn, lua_method};
use crate::engine::sim::entity::{Attachment, EntityId, EntityKind};
use crate::engine::sim::math::{Fx, FxQuat, FxVec3};

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct Beam {
    launcher: EntityId,
    weapon: u32,
    muzzle: Option<u32>,
    interval: u64,
    countdown: u64,
    enabled: bool,
    /// Where the last trace stopped (bone 1).
    end: FxVec3,
}

/// Live beams by entity id.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Beams(BTreeMap<EntityId, Beam>);

impl Beams {
    /// Beam entity `id`'s far end (see [`end_of`]).
    pub fn end(&self, id: EntityId) -> Option<FxVec3> {
        self.0.get(&id).map(|b| b.end)
    }
}

/// The beam's far end, for `GetPosition(1)` on a beam entity.
pub fn end_of(lua: &Lua, id: EntityId) -> Option<FxVec3> {
    lua.app_data_ref::<Beams>().and_then(|b| b.0.get(&id).map(|b| b.end))
}

/// World position of `bone` on entity `id`: a collision beam's bone 1 is its
/// far end (FAF's beams put their end-point effects there: `CreateAttachedEmitter(beam, 1, ...)`),
/// anything else the bone's rest position.
pub fn bone_position(lua: &Lua, world: &crate::engine::sim::entity::EntityWorld, id: EntityId, bone: Option<u32>) -> Option<FxVec3> {
    if bone == Some(1)
        && let Some(end) = end_of(lua, id) {
        return Some(end);
    }
    world.get(id).map(|e| e.bone_world_position(bone))
}

fn beams(lua: &Lua) -> mlua::AppDataRefMut<'_, Beams> {
    lua.app_data_mut::<Beams>().expect("Beams")
}

/// What FAF's beam weapon passes `BeamType(spec)` (the keys the engine reads).
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "CollisionBeamSpec", write_only)]
#[allow(dead_code)]
struct BeamSpec {
    /// The firing weapon: the beam belongs to its unit.
    #[lua(rename = "Weapon")]
    weapon: Obj<moho::Weapon>,
    /// The muzzle bone the beam attaches to.
    #[lua(rename = "OtherBone")]
    other_bone: Option<Bone>,
    /// Ticks between collision traces (at least 1, default 1).
    #[lua(rename = "CollisionCheckInterval")]
    collision_check_interval: Option<f64>,
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// The object itself when it is a live collision beam, else nil: moho
        /// returns the object, not a boolean, as its `IsProjectile` does (FA exe
        /// 0x75eba0, the entity's beam cast at vtable +0x20).
        IsCollisionBeam = |lua, v: LuaValue| {
            let LuaValue::Table(t) = v else { return Ok(None) };
            let Ok(id) = entity_id(&t) else { return Ok(None) };
            let live = beams(lua).0.contains_key(&id) && world(lua).get(id).is_some_and(|e| !e.destroyed);
            Ok(live.then(|| Obj::<moho::CollisionBeamEntity>::new(t)))
        })?;
    lua.set_app_data(Beams::default());
    lua_method!(lua, moho::CollisionBeamEntity, __init = |lua, (this, spec): (Obj<moho::CollisionBeamEntity>, Shaped<BeamSpec>)| {
        let weapon: LuaTable = spec.get("Weapon")?;
        let crate::engine::api::weapon::WeaponHandle { unit, index } = script_object::handle(&weapon)?;
        let (army, position, orientation) = world(lua).get(unit).map(|e| (e.army, e.position(), e.orientation)).unwrap_or((None, FxVec3::default(), FxQuat::IDENTITY));
        let id = entity::bind_script_entity(lua, &this, EntityKind::Script, army, position, orientation)?;
        let muzzle = match spec.get::<LuaValue>("OtherBone")? {
            LuaValue::Nil => None,
            b => crate::engine::api::manipulator::resolve_bone(lua, unit, &b)?,
        };
        entity::attach_posed(lua, id, Attachment { parent: unit, parent_bone: muzzle, self_bone: None });
        let interval = spec.get::<Option<f64>>("CollisionCheckInterval")?.unwrap_or(1.0).max(1.0) as u64;
        beams(lua).0.insert(id, Beam { launcher: unit, weapon: index, muzzle, interval, countdown: 0, enabled: false, end: position });
        CollisionBeamEntityCallbacks::on_create(lua, &this, "beam")?;
        Ok(())
    })?;
    fn set_enabled(lua: &Lua, this: &LuaTable, on: bool) -> LuaResult<()> {
        let id = entity_id(this)?;
        let changed = match beams(lua).0.get_mut(&id) {
            Some(b) if b.enabled != on => {
                b.enabled = on;
                b.countdown = 0;
                Some(*b)
            }
            _ => None,
        };
        if let Some(beam) = changed {
            // Switched on: the far end is where the beam stops now, not where
            // the last firing left it. FAF enables from the weapon's thread,
            // after this tick's traces, so otherwise this tick's snapshot (and
            // OnEnable's end-point emitters) would show the stale end, and
            // the new beam would jump to the target a tick later.
            if on {
                let (_, _, end) = collide(lua, &beam)?;
                if let Some(b) = beams(lua).0.get_mut(&id) {
                    b.end = end;
                }
            }
            let this = Obj::<moho::CollisionBeamEntity>::new(this.clone());
            if on {
                CollisionBeamEntityCallbacks::on_enable(lua, &this, "beam")?;
            } else {
                CollisionBeamEntityCallbacks::on_disable(lua, &this, "beam")?;
            }
        }
        Ok(())
    }
    lua_method!(lua, moho::CollisionBeamEntity, Enable = |lua, this: Obj<moho::CollisionBeamEntity>| set_enabled(lua, &this, true))?;
    lua_method!(lua, moho::CollisionBeamEntity, Disable = |lua, this: Obj<moho::CollisionBeamEntity>| set_enabled(lua, &this, false))?;
    lua_method!(lua, moho::CollisionBeamEntity, IsEnabled = |lua, this: Obj<moho::CollisionBeamEntity>| {
        let id = entity_id(&this)?;
        Ok(beams(lua).0.get(&id).is_some_and(|b| b.enabled))
    })?;
    lua_method!(lua, moho::CollisionBeamEntity, GetLauncher = |lua, this: Obj<moho::CollisionBeamEntity>| {
        let id = entity_id(&this)?;
        let launcher = beams(lua).0.get(&id).map(|b| b.launcher);
        match launcher.filter(|&u| world(lua).get(u).is_some_and(|e| !e.destroyed)) {
            Some(u) => crate::engine::api::entity::object_as::<moho::Unit>(lua, u),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::CollisionBeamEntity,
        /// The beam emitter's length follows the collision distance (render only).
        #[lua(stub)]
        SetBeamFx = |_, (_this, _fx, _check): (Obj<moho::CollisionBeamEntity>, LuaValue, LuaValue)| Ok(()))?;
    Ok(())
}

/// A trace: the units and shield bubbles on the beam (nearest first: impact
/// type, entity, where), then where it meets the ground or water (or its
/// end in the air). Each entity is asked in turn whether it takes the hit.
fn trace(lua: &Lua, beam: &Beam) -> (Vec<(&'static str, EntityId, FxVec3)>, (&'static str, FxVec3)) {
    use crate::engine::sim::layer::Layer;
    let w = world(lua);
    let Some(unit) = w.get(beam.launcher) else { return (Vec::new(), ("Air", beam.end)) };
    let (from, muzzle_rotation) = crate::engine::api::entity::posed_bone(lua, unit, beam.muzzle);
    // The beam runs where the weapon points, not straight at its target: a
    // turret's aim turns at its slew rate, so the beam sweeps onto a target
    // and trails a fast one (FAF widens fast units' collision boxes so beams
    // keep hitting them). A beam fixed to the hull (the CZAR's) runs along
    // its muzzle.
    let forward = FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE };
    let weapon = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>().and_then(|ws| {
        ws.get(beam.launcher, beam.weapon).map(|s| {
            let dir = if s.fixed_beam_muzzle.is_some() { muzzle_rotation.rotate(forward) } else { s.barrel_direction(s.frame(unit)) };
            (dir, s.max_beam_length.unwrap_or(s.max_radius), s.targets_projectiles)
        })
    });
    let (dir, range, anti_projectile) = weapon.unwrap_or((unit.orientation.rotate(forward), Fx::from_num(30), false));
    let dir = dir.normalized().unwrap_or(forward);
    let range = range.max(Fx::ONE);
    let to = from + dir * range;
    // Units on the segment (not the launcher, not its allies).
    let armies = lua.app_data_ref::<crate::engine::sim::army::Armies>();
    let allied = |other: Option<u32>| match (unit.army, other, armies.as_deref()) {
        (Some(a), Some(b), Some(armies)) => armies.alliance(a, b) == crate::engine::sim::army::Alliance::Ally,
        (Some(a), Some(b), None) => a == b,
        _ => false,
    };
    let reach = w.unit_reach();
    let mut near = Vec::new();
    w.unit_candidates(from.x.min(to.x) - reach, from.z.min(to.z) - reach, from.x.max(to.x) + reach, from.z.max(to.z) + reach, &mut near);
    near.sort_unstable();
    near.dedup();
    let mut hits: Vec<(Fx, EntityId, &'static str)> = Vec::new();
    for id in near {
        if id == beam.launcher {
            continue;
        }
        let Some(e) = w.get(id).filter(|e| !e.destroyed && e.unit.is_some()) else { continue };
        if allied(e.army) {
            continue;
        }
        let Some(t) = crate::engine::sim::collision::segment_hit(from, to, e.position(), e.orientation, &e.collision_shape()) else { continue };
        let kind = match e.unit.as_deref().map(|u| u.layer).unwrap_or_default() {
            Layer::Air => "UnitAir",
            Layer::Sub | Layer::Seabed => "UnitUnderwater",
            _ => "Unit",
        };
        hits.push((t, id, kind));
    }
    // Shield bubbles the beam enters from outside.
    for e in w.iter().filter(|e| !e.destroyed && e.kind == crate::engine::sim::entity::EntityKind::Shield) {
        let crate::engine::sim::entity::CollisionShape::Sphere { center, radius } = e.collision_shape() else { continue };
        if radius <= Fx::ZERO {
            continue;
        }
        let c = e.position() + e.orientation.rotate(center);
        // Only shields near the beam (keeps the fixed-point squares small).
        if c.x < from.x.min(to.x) - radius || c.x > from.x.max(to.x) + radius || c.z < from.z.min(to.z) - radius || c.z > from.z.max(to.z) + radius {
            continue;
        }
        if let Some(t) = crate::engine::sim::projectile::segment_enters_sphere(from, to, c, radius) {
            hits.push((t, e.id, "Shield"));
        }
    }
    // A projectile-shooting beam's (the Cybran TMD's zapper) hits enemy
    // projectiles on it: their collision shape (FAF gives missiles a sphere,
    // 0.11 x MaxSpeed for most), else `INTERCEPT_RADIUS` as a shot meeting
    // one. FAF probe probe_tmd: the real zapper kills a tactical missile as
    // it comes into range. Other beams pass through projectiles, as in the
    // real game (FAF probe probe_beamhit: a Monkeylord's laser never
    // reports a `Projectile` impact; the tank shells and missiles fired
    // back down it reach the Monkeylord).
    if anti_projectile {
        use crate::engine::sim::entity::CollisionShape;
        let default = CollisionShape::Sphere { center: FxVec3::default(), radius: crate::engine::sim::projectile::INTERCEPT_RADIUS };
        for e in w.iter().filter(|e| !e.destroyed && e.kind == crate::engine::sim::entity::EntityKind::Projectile) {
            let shape = match e.collision_shape() {
                CollisionShape::Sphere { radius, .. } if radius <= Fx::ZERO => default,
                CollisionShape::None => default,
                shape => shape,
            };
            let r = match shape {
                CollisionShape::Sphere { center, radius } => radius + center.x.abs() + center.y.abs() + center.z.abs(),
                CollisionShape::Box { center, half } => half.x + half.y + half.z + center.x.abs() + center.y.abs() + center.z.abs(),
                CollisionShape::None => Fx::ZERO,
            };
            let c = e.position();
            if c.x < from.x.min(to.x) - r || c.x > from.x.max(to.x) + r || c.z < from.z.min(to.z) - r || c.z > from.z.max(to.z) + r || allied(e.army) {
                continue;
            }
            if let Some(t) = crate::engine::sim::collision::segment_hit(from, to, c, e.orientation, &shape) {
                hits.push((t, e.id, "Projectile"));
            }
        }
    }
    hits.sort_by_key(|a| (a.0, a.1));
    // First ground / water contact, sampled every world unit.
    let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
    let ground = terrain.as_deref().and_then(|t| {
        let steps = range.ceil().to_num::<i64>().max(1);
        (1..=steps).find_map(|i| {
            let f = Fx::from_num(i) / Fx::from_num(steps);
            let p = from + (to - from) * f;
            let floor = t.height_at(p.x, p.z);
            let surface = t.surface_at(p.x, p.z);
            if p.y <= surface {
                Some((f, if surface > floor { "Water" } else { "Terrain" }))
            } else {
                None
            }
        })
    });
    let (stop, end) = match ground {
        Some((f, kind)) => (f, (kind, from + (to - from) * f)),
        None => (Fx::ONE, ("Air", to)),
    };
    let entities = hits.into_iter().filter(|(t, _, _)| *t <= stop).map(|(t, id, kind)| (kind, id, from + (to - from) * t)).collect();
    (entities, end)
}

/// Per tick, after weapons: enabled beams trace on their interval and call
/// `OnImpact`.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let due: Vec<(EntityId, Beam)> = {
        let Some(mut b) = lua.app_data_mut::<Beams>() else { return Ok(()) };
        let w = world(lua);
        b.0.retain(|id, _| w.get(*id).is_some_and(|e| !e.destroyed));
        let mut due = Vec::new();
        for (&id, beam) in b.0.iter_mut() {
            if !beam.enabled {
                continue;
            }
            if beam.countdown == 0 {
                beam.countdown = beam.interval;
                due.push((id, *beam));
            }
            beam.countdown -= 1;
        }
        due
    };
    for (id, beam) in due {
        let (kind, target, end) = collide(lua, &beam)?;
        if let Some(b) = beams(lua).0.get_mut(&id) {
            b.end = end;
        }
        let Some(obj) = object(lua, id)? else { continue };
        CollisionBeamEntityCallbacks::on_impact(lua, &Obj::<moho::CollisionBeamEntity>::new(obj), "beam", kind, crate::lua_api::shapes::Loose::new(target))?;
    }
    Ok(())
}

/// Where `beam` stops now: the impact type, the entity hit (else nil) and
/// the far end.
fn collide(lua: &Lua, beam: &Beam) -> LuaResult<(&'static str, LuaValue, FxVec3)> {
    let (entities, (ground_kind, ground_end)) = trace(lua, beam);
    let weapon = crate::engine::api::weapon::weapon_object(lua, beam.launcher, beam.weapon)?;
    // Moho asks each entity on the beam, nearest first, whether it takes
    // the hit (`OnCollisionCheckWeapon(weapon)`: dying units, allied
    // beams through shields); the first that does stops it.
    for (kind, e, at) in entities {
        let Some(t) = object(lua, e)? else { continue };
        let accepted = match (&weapon, t.get::<Option<LuaFunction>>("OnCollisionCheckWeapon")?) {
            (Some(w), Some(f)) => match f.call::<LuaValue>((t.clone(), w.clone())) {
                Ok(LuaValue::Boolean(b)) => b,
                Ok(LuaValue::Nil) => false,
                Ok(_) => true,
                Err(err) => {
                    tracing::warn!(target: "lua", "Error running OnCollisionCheckWeapon script: {err}");
                    true
                }
            },
            _ => true,
        };
        if accepted {
            return Ok((kind, LuaValue::Table(t), at));
        }
    }
    Ok((ground_kind, LuaNil, ground_end))
}
