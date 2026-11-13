//! Projectile spawning and `moho.projectile_methods`.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::blueprints::BlueprintKind;
use crate::engine::api::entity::{blueprint_table, create_object, entity_id, world};
use crate::engine::api::callbacks::ProjectileCallbacks;
use crate::engine::api::moho;
use crate::lua_api::{MohoClass, Obj, Status, lua_method};
use crate::engine::sim::entity::{EntityId, EntityKind};
use crate::engine::sim::math::{Fx, FxQuat, FxVec3};
use crate::engine::sim::projectile::{ProjectileState, ProjectileTemplate, Projectiles};

fn projectiles(lua: &Lua) -> mlua::AppDataRefMut<'_, Projectiles> {
    lua.app_data_mut::<Projectiles>().expect("Projectiles not in app_data")
}

/// Runs `f` on the projectile state behind script object `this` (no-op if
/// it is gone).
fn with_projectile<R: Default>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut ProjectileState) -> R) -> LuaResult<R> {
    let id = entity_id(this)?;
    Ok(projectiles(lua).get_mut(id).map(f).unwrap_or_default())
}

/// Reads a projectile blueprint's `Physics` section.
fn parse_template(lua: &Lua, bp: &LuaTable) -> LuaResult<ProjectileTemplate> {
    let render_mesh = crate::engine::api::mesh::render_index_for(lua, bp)?;
    let draw_scale = crate::engine::api::entity::blueprint_scale(bp)?;
    let strategic_icon = strategic_icon(lua, bp)?;
    let camera_follow = camera_follow(bp)?;
    // No `Physics` section: all defaults.
    let p = match bp.get::<Option<LuaTable>>("Physics")? {
        Some(p) => p,
        None => lua.create_table()?,
    };
    let num = |k: &str| -> LuaResult<Fx> { Ok(crate::engine::sim::math::from_lua(p.get::<Option<f64>>(k)?.unwrap_or(0.0))) };
    let flag = |k: &str| -> LuaResult<bool> { Ok(p.get::<Option<bool>>(k)?.unwrap_or(false)) };
    let positive = |v: Fx| (v > Fx::ZERO).then_some(v);
    // Defaults as moho's `RProjectileBlueprintPhysics` constructor sets them
    // (MohoEngine.dll 0x10109450): CollideSurface, CollideEntity,
    // VelocityAlign, LeadTarget and UseGravity on; InitialSpeed 1. (137 FA
    // projectiles, the Striker's and Lobo's shells among them, leave
    // UseGravity out and fly ballistic.)
    let default_on = |k: &str| -> LuaResult<bool> { Ok(p.get::<Option<bool>>(k)?.unwrap_or(true)) };
    Ok(ProjectileTemplate {
        initial_speed: match p.get::<Option<f64>>("InitialSpeed")? {
            Some(v) => crate::engine::sim::math::from_lua(v),
            None => Fx::ONE,
        },
        acceleration: num("Acceleration")?,
        initial_speed_range: num("InitialSpeedRange")?,
        acceleration_range: num("AccelerationRange")?,
        turn_rate_range: num("TurnRateRange")?,
        max_speed: positive(num("MaxSpeed")?),
        direction: FxVec3 { x: num("DirectionX")?, y: num("DirectionY")?, z: num("DirectionZ")? },
        direction_range: FxVec3 { x: num("DirectionXRange")?, y: num("DirectionYRange")?, z: num("DirectionZRange")? },
        min_bounce_count: p.get::<Option<u32>>("MinBounceCount")?.unwrap_or(0),
        max_bounce_count: p.get::<Option<u32>>("MaxBounceCount")?.unwrap_or(0),
        // Moho's `RProjectileBlueprintPhysics` constructor: 0.5.
        bounce_velocity_damp: match p.get::<Option<f64>>("BounceVelDamp")? {
            Some(v) => crate::engine::sim::math::from_lua(v),
            None => Fx::from_num(0.5),
        },
        rotational_velocity: num("RotationalVelocity")?,
        rotational_velocity_range: num("RotationalVelocityRange")?,
        position: FxVec3 { x: num("PositionX")?, y: num("PositionY")?, z: num("PositionZ")? },
        position_range: FxVec3 { x: num("PositionXRange")?, y: num("PositionYRange")?, z: num("PositionZRange")? },
        lifetime: positive(num("Lifetime")?),
        collide_entity: default_on("CollideEntity")?,
        collide_surface: default_on("CollideSurface")?,
        destroy_on_water: flag("DestroyOnWater")?,
        use_gravity: default_on("UseGravity")?,
        realistic_ordinance: flag("RealisticOrdinance")?,
        stay_upright: flag("StayUpright")?,
        velocity_align: default_on("VelocityAlign")?,
        track_target: flag("TrackTarget")?,
        lead_target: default_on("LeadTarget")?,
        turn_rate: num("TurnRate")?,
        max_zigzag: num("MaxZigZag")?,
        zigzag_frequency: num("ZigZagFrequency")?,
        detonate_above: num("DetonateAboveHeight")?,
        detonate_below: num("DetonateBelowHeight")?,
        stay_underwater: flag("StayUnderwater")?,
        render_mesh,
        draw_scale,
        strategic_icon,
        camera_follow,
    })
}

/// The blueprint's strategic-view icon: `Display.StrategicIconSize` (moho
/// reads it at RProjectileBlueprint+0x1CC) and `StrategicIconName`. None
/// when it has neither (no size: nothing to draw).
fn strategic_icon(lua: &Lua, bp: &LuaTable) -> LuaResult<Option<u32>> {
    let size = bp.get::<Option<LuaTable>>("Display")?.map(|d| d.get::<Option<f64>>("StrategicIconSize")).transpose()?.flatten().unwrap_or(0.0) as f32;
    let texture = bp.get::<Option<String>>("StrategicIconName")?.filter(|t| !t.is_empty()).map(|t| t.to_lowercase());
    if size <= 0.0 && texture.is_none() {
        return Ok(None);
    }
    if lua.app_data_ref::<crate::engine::sim::projectile::ProjectileIcons>().is_none() {
        lua.set_app_data(crate::engine::sim::projectile::ProjectileIcons::default());
    }
    let mut icons = lua.app_data_mut::<crate::engine::sim::projectile::ProjectileIcons>().expect("ProjectileIcons");
    Ok(Some(icons.index_of(crate::engine::sim::projectile::ProjectileIcon { size, texture })))
}

/// `Display.CameraFollowsProjectile`: the seconds a following camera holds
/// on its end (`CameraFollowTimeout`, default 1: RProjectileBlueprint
/// +0x1c4 / +0x1c8, MohoEngine.dll 0x10109540).
fn camera_follow(bp: &LuaTable) -> LuaResult<Option<f32>> {
    let Some(d) = bp.get::<Option<LuaTable>>("Display")? else { return Ok(None) };
    if !d.get::<Option<bool>>("CameraFollowsProjectile")?.unwrap_or(false) {
        return Ok(None);
    }
    Ok(Some(d.get::<Option<f64>>("CameraFollowTimeout")?.unwrap_or(1.0) as f32))
}

/// Lua module + class for a projectile blueprint: explicit `ScriptModule`/
/// `ScriptClass`, else `<id minus _proj.bp>_script.lua` and its `TypeClass`.
fn script_of(bp: &LuaTable, id: &str) -> LuaResult<(String, String)> {
    if let (Some(m), Some(c)) = (
        bp.get::<Option<String>>("ScriptModule")?,
        bp.get::<Option<String>>("ScriptClass")?,
    ) {
        return Ok((m, c));
    }
    match id.strip_suffix("_proj.bp") {
        Some(stem) => Ok((format!("{stem}_script.lua"), "TypeClass".into())),
        None => Err(LuaError::runtime(format!("projectile blueprint '{id}' is not a _proj.bp"))),
    }
}

/// How a projectile is launched beyond its blueprint.
#[derive(Clone, Copy, Debug, Default)]
pub struct Launch {
    /// Any length; `None` = the blueprint's `Direction*` (spread by
    /// `Direction*Range`), else +z (moho: `GetDirection`, `COORDS_Orient`).
    pub direction: Option<FxVec3>,
    /// Overrides `InitialSpeed` (weapons launch at `MuzzleVelocity`).
    pub speed: Option<Fx>,
    /// A weapon's shot: after `OnCreate`, the velocity the script left is
    /// rescaled to `speed` (unless 0), keeping its direction (moho's
    /// `UnitWeapon` launch, MohoEngine.dll 0x102b26ba..0x102b2774: normalize
    /// +0x270, times `GetMuzzleVelocity`, skipped when the weapon's
    /// `MuzzleVelocity` is 0). The Corsair's darts aim their own velocity in
    /// `OnCreate` and still leave at 55.
    pub rescale_after_create: bool,
    /// Overrides the blueprint `Lifetime` (seconds).
    pub lifetime: Option<Fx>,
    pub collide_friendly: bool,
    pub target: Option<crate::engine::sim::projectile::ProjectileTarget>,
    /// The launcher's weapon (index) told whether the shot hit its target.
    pub reports_to: Option<u32>,
}

/// Tells a followed shot's weapon ([`ProjectileState::reports_to`]) how it
/// ended (moho's `Projectile::Impact`, MohoEngine.dll 0x1027f30f): a hit on
/// its target unit, or on something riding it (`IsHit` 0x10214680); a
/// projectile or a shield is neither; anything else, its lifetime running
/// out included, a miss.
fn report_shot(lua: &Lua, impact: &crate::engine::sim::projectile::Impact) {
    use crate::engine::sim::projectile::{self, ImpactKind};
    use crate::engine::sim::weapon::{ShotResult, Weapons};
    let Some((launcher, index, target)) = projectiles(lua).get(impact.id).and_then(|p| Some((p.launcher?, p.reports_to?, p.target))) else { return };
    let w = world(lua);
    let result = match (impact.kind, impact.target, target) {
        (ImpactKind::Projectile | ImpactKind::Shield, ..) => ShotResult::Neutral,
        (_, Some(hit), Some(projectile::ProjectileTarget::Entity(t))) if hit == t || w.get(hit).and_then(|e| e.attachment).is_some_and(|a| a.parent == t) => ShotResult::Hit,
        _ => ShotResult::Miss,
    };
    if let Some(mut weapons) = lua.app_data_mut::<Weapons>() {
        weapons.shot_result(launcher, index, result, &w);
    }
}

/// `base` plus a uniform draw in `[-range, range]` from the sim's random
/// numbers, drawn only when there is a range. Moho's blueprint getters
/// (`RProjectileBlueprint::GetDirection`, `GetRandomInitialSpeed`, ...)
/// all vary a field this way.
fn spread(lua: &Lua, base: Fx, range: Fx) -> Fx {
    if range == Fx::ZERO {
        return base;
    }
    let u = lua.app_data_mut::<crate::engine::sim::rng::SimRng>().map_or(Fx::ZERO, |mut r| r.uniform_fx());
    base + range * (u * Fx::from_num(2) - Fx::ONE)
}

/// A blueprint's launch direction: `Direction*`, each axis spread by its
/// `Direction*Range`.
fn blueprint_direction(lua: &Lua, template: &ProjectileTemplate) -> FxVec3 {
    let spread = |base: Fx, range: Fx| spread(lua, base, range);
    let (d, r) = (template.direction, template.direction_range);
    FxVec3 { x: spread(d.x, r.x), y: spread(d.y, r.y), z: spread(d.z, r.z) }
}

/// A `RealisticOrdinance` projectile's launch velocity (bombs, air-dropped
/// torpedoes; moho's projectile constructor, MohoEngine.dll 0x1027c923):
/// its launcher's velocity, turned horizontally toward its target when it
/// has one, at the same speed. The target point is where a unit will be
/// after the launcher's `PredictAheadForBombDrop` seconds when that is set
/// (0x1027c9ff; moho steps the unit's motion, turning with it, 0x10216a90;
/// ours goes straight on), else where it is. A weapon with a muzzle
/// velocity then rescales it to that speed (0x102b2745), through moho's
/// normalize, which leaves a zero vector zero (0x10051db2): a torpedo from
/// a sub that isn't moving starts at rest and accelerates away.
fn realistic_launch(lua: &Lua, launcher: EntityId, launch: Launch, dir: FxVec3) -> FxVec3 {
    use crate::engine::sim::projectile::ProjectileTarget;
    let w = world(lua);
    let Some(from) = w.get(launcher) else { return dir * launch.speed.unwrap_or(Fx::ZERO) };
    let mut velocity = crate::engine::sim::movement::velocity(from);
    let ahead = from.unit.as_deref().map_or(Fx::ZERO, |u| u.motion_params.air.predict_ahead_for_bomb_drop);
    let aim = match launch.target {
        Some(ProjectileTarget::Entity(t)) => w.get(t).filter(|e| !e.destroyed).map(|e| {
            let at = e.position();
            if ahead > Fx::ZERO && e.unit.is_some() {
                let v = crate::engine::sim::movement::velocity(e);
                FxVec3 { x: at.x + v.x * ahead, y: at.y, z: at.z + v.z * ahead }
            } else {
                at
            }
        }),
        Some(ProjectileTarget::Ground(p)) => Some(p),
        None => None,
    };
    if let Some(aim) = aim {
        let p = from.position();
        if let Some(toward) = (FxVec3 { x: aim.x - p.x, y: Fx::ZERO, z: aim.z - p.z }).normalized() {
            velocity = toward * velocity.length();
        }
    }
    match launch.speed {
        Some(s) if s != Fx::ZERO => velocity.normalized().map_or(FxVec3::default(), |d| d * s),
        _ => velocity,
    }
}

// The parsed templates sit beside the live projectiles: only they go.
inventory::submit! {
    crate::engine::api::blueprints::DerivedCache {
        name: "projectile templates",
        forget: |lua| {
            if let Some(mut p) = lua.app_data_mut::<Projectiles>() {
                p.forget_templates();
            }
        },
    }
}

/// The `Physics` template of blueprint `bp_id` (parsed once, then cached).
pub(crate) fn template_of(lua: &Lua, bp_id: &str, bp: &LuaTable) -> LuaResult<std::sync::Arc<ProjectileTemplate>> {
    let cached = projectiles(lua).template(bp_id);
    match cached {
        Some(t) => Ok(t),
        None => {
            let t = parse_template(lua, bp)?;
            Ok(projectiles(lua).cache_template(bp_id.to_owned(), t))
        }
    }
}

/// A blueprint's offset from its creator: `Position*`, each axis spread by
/// its `Position*Range` (`RProjectileBlueprint::GetPositionOffset`,
/// MohoEngine.dll 0x10109f60).
fn blueprint_offset(lua: &Lua, bp_path: &str) -> LuaResult<FxVec3> {
    let bp_id = bp_path.to_lowercase();
    let bp = blueprint_table(lua, BlueprintKind::Projectile, &bp_id)?;
    let t = template_of(lua, &bp_id, &bp)?;
    let (p, r) = (t.position, t.position_range);
    Ok(FxVec3 { x: spread(lua, p.x, r.x), y: spread(lua, p.y, r.y), z: spread(lua, p.z, r.z) })
}

/// A blueprint's spin (`RProjectileBlueprint::GetAngularVelocity`,
/// MohoEngine.dll 0x1010a4d0): `RotationalVelocity` +- `RotationalVelocityRange`
/// degrees a second about a random axis (moho normalises three Gaussian
/// draws). No spin, and no draws, without either.
fn blueprint_spin(lua: &Lua, template: &ProjectileTemplate) -> FxVec3 {
    if template.rotational_velocity == Fx::ZERO && template.rotational_velocity_range == Fx::ZERO {
        return FxVec3::default();
    }
    let axis = {
        let Some(mut rng) = lua.app_data_mut::<crate::engine::sim::rng::SimRng>() else { return FxVec3::default() };
        loop {
            let v = FxVec3 { x: rng.gaussian(), y: rng.gaussian(), z: rng.gaussian() };
            if let Some(axis) = v.normalized() {
                break axis;
            }
        }
    };
    let degrees = spread(lua, template.rotational_velocity, template.rotational_velocity_range);
    axis * (degrees * Fx::PI / 180)
}

/// Spawns a projectile from blueprint `bp_path` at `position`, launched by
/// `launcher` (army, `GetLauncher`), and runs its `OnCreate(inWater)`.
pub fn spawn_projectile(lua: &Lua, launcher: EntityId, bp_path: &str, position: FxVec3, launch: Launch) -> LuaResult<Obj<moho::Projectile>> {
    let direction = launch.direction;
    let bp_id = bp_path.to_lowercase();
    let bp = blueprint_table(lua, BlueprintKind::Projectile, &bp_id)?;
    let template = template_of(lua, &bp_id, &bp)?;
    let army = {
        let w = world(lua);
        w.get(launcher).ok_or_else(|| LuaError::runtime("CreateProjectile: launcher no longer exists"))?.army
    };
    // No direction given: the blueprint's, each axis spread uniformly by its
    // `Direction*Range` (`RProjectileBlueprint::GetDirection`, MohoEngine.dll
    // 0x1010a090); a zero direction faces +z (`COORDS_Orient` 0x100f8b40
    // returns the identity). Never the launcher's facing.
    let dir = direction
        .or_else(|| Some(blueprint_direction(lua, &template)))
        .and_then(FxVec3::normalized)
        .unwrap_or(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
    let orientation = FxQuat::look_along(dir);
    let (module, class) = script_of(&bp, &bp_id)?;
    let category_index = crate::engine::api::categories::domain(lua)?.index_of(&bp_id);
    let id = world(lua).spawn(EntityKind::Projectile, BlueprintKind::Projectile, bp_id.clone(), army, position, orientation);
    if let Some(e) = world(lua).get_mut(id) {
        e.category_index = category_index;
        e.cold.render_mesh = template.render_mesh;
        e.cold.strategic_icon = template.strategic_icon;
        e.cold.scale = template.draw_scale;
        e.cold.draw_scale = FxVec3 { x: template.draw_scale, y: template.draw_scale, z: template.draw_scale };
    }
    // Per-projectile variation, drawn in the order moho's constructor draws
    // it (`Projectile::Projectile`, MohoEngine.dll 0x1027c5d4: spin, turn rate,
    // acceleration, bounces, then initial speed at 0x1027cc8b). Weapons launch at
    // their muzzle velocity instead (0x102b26b5 overwrites the velocity), so
    // `InitialSpeedRange` only varies script-made projectiles.
    let angular_velocity = blueprint_spin(lua, &template);
    let turn_rate = spread(lua, template.turn_rate, template.turn_rate_range);
    let acceleration = spread(lua, template.acceleration, template.acceleration_range);
    // Bounces: min + a 32-bit draw x (max - min) >> 32, so in [min, max)
    // (0x1027c8e9); drawn only with a range.
    let bounces = match template.max_bounce_count.checked_sub(template.min_bounce_count) {
        Some(span) if span > 0 => {
            let r = lua.app_data_mut::<crate::engine::sim::rng::SimRng>().map_or(0, |mut r| r.next_u64() >> 32);
            template.min_bounce_count + ((r * span as u64) >> 32) as u32
        }
        _ => template.min_bounce_count,
    };
    let speed = launch.speed.unwrap_or_else(|| spread(lua, template.initial_speed, template.initial_speed_range));
    let mut velocity = dir * speed;
    if template.realistic_ordinance {
        velocity = realistic_launch(lua, launcher, launch, dir);
        // Bombs scatter: moho adds a uniform draw in +- the blueprint's
        // `RotationalVelocityRange` to the horizontal velocity
        // (0x1027cb89; FAF's napalm carpet bomb, 0.25).
        let r = template.rotational_velocity_range;
        if r > Fx::ZERO {
            velocity.x = spread(lua, velocity.x, r);
            velocity.z = spread(lua, velocity.z, r);
        }
    }
    // A camera following the launcher follows it (`Projectile` ctor,
    // MohoEngine.dll 0x1027cf73: `Sim::QueueCameraFollowEvent`, sent to the
    // interface with the tick's sync).
    if let Some(timeout) = template.camera_follow
        && let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
        p.camera_follows.push(crate::engine::sim::presentation::CameraFollow { launcher, projectile: id, timeout });
    }
    if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
        p.launches.push((id, [position.x.to_num(), position.y.to_num(), position.z.to_num()]));
    }
    let mut state = ProjectileState::new(id, Some(launcher), &template, velocity);
    state.turn_rate = turn_rate;
    state.angular_velocity = angular_velocity;
    state.bounces_left = bounces;
    state.acceleration = acceleration;
    if launch.lifetime.is_some() {
        state.set_lifetime(launch.lifetime);
    }
    state.collide_friendly = launch.collide_friendly;
    state.target = launch.target;
    state.reports_to = launch.reports_to;
    projectiles(lua).insert(state);
    let in_water = lua
        .app_data_ref::<crate::engine::sim::terrain::Terrain>()
        .and_then(|t| t.water_surface)
        .is_some_and(|w| position.y < w);
    let obj = create_object(lua, id, &module, &class)?;
    ProjectileCallbacks::on_create(lua, &Obj::<moho::Projectile>::new(obj.clone()), &bp_id, in_water)?;
    // (Only a velocity the script changed: the launch one already has that
    // speed, and renormalizing it would only add rounding.)
    if let (true, Some(speed)) = (launch.rescale_after_create, launch.speed)
        && speed != Fx::ZERO
        && let Some(p) = projectiles(lua).get_mut(id).filter(|p| p.velocity() != velocity)
        && let Some(dir) = p.velocity().normalized() {
        p.set_velocity(dir * speed);
    }
    Ok(Obj::new(obj))
}

/// Tick pass: moves projectiles, then runs `OnImpact(targetType, target)`
/// for each impact in id order. A projectile whose lifetime ran out is destroyed
/// if its script didn't do so; one that hit the surface and survived stops
/// colliding with it, so the impact fires once.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    use crate::engine::sim::projectile::{self, ImpactKind};
    let mut impacts = Vec::new();
    {
        let (Some(mut ps), Some(mut w)) = (lua.app_data_mut::<Projectiles>(), lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>()) else {
            return Ok(());
        };
        if ps.len() == 0 {
            ps.script_sphere_centers.clear();
            return Ok(());
        }
        let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
        let armies = lua.app_data_ref::<crate::engine::sim::army::Armies>();
        // Shield bubbles that are up (FAF gives them a sphere collision
        // while on, `None` while down), and script entities with a sphere
        // (flares, missile redirectors, depth charges).
        use crate::engine::sim::entity::{CollisionShape, EntityKind};
        let (mut shields, mut script_spheres) = (Vec::new(), Vec::new());
        let last = std::mem::take(&mut ps.script_sphere_centers);
        for e in w.iter().filter(|e| !e.destroyed && matches!(e.kind, EntityKind::Shield | EntityKind::Script)) {
            if let CollisionShape::Sphere { center, radius } = e.collision_shape()
                && radius > crate::engine::sim::math::Fx::ZERO {
                let center = e.position() + e.orientation.rotate(center);
                if e.kind == EntityKind::Shield {
                    shields.push(projectile::ShieldSphere { id: e.id, center, radius, moved: Some(Default::default()) });
                } else {
                    let moved = last.binary_search_by_key(&e.id, |c| c.0).ok().map(|i| center - last[i].1);
                    script_spheres.push(projectile::ShieldSphere { id: e.id, center, radius, moved });
                }
            }
        }
        // (Ids ascending, for next step's lookups.)
        let mut centers: Vec<_> = script_spheres.iter().map(|s| (s.id, s.center)).collect();
        centers.sort_unstable_by_key(|c| c.0);
        ps.script_sphere_centers = centers;
        // Projectile velocities, when some projectile shoots at another.
        let chasing = ps.iter().any(|p| matches!(p.target, Some(projectile::ProjectileTarget::Entity(t)) if w.get(t).is_some_and(|e| e.kind == EntityKind::Projectile)));
        let velocities: Vec<(u32, crate::engine::sim::math::FxVec3)> = if chasing { ps.iter().map(|p| (p.id, p.velocity())).collect() } else { Vec::new() };
        let env = projectile::Env { terrain: terrain.as_deref(), shields: &shields, armies: armies.as_deref(), script_spheres: &script_spheres, projectile_velocities: &velocities };
        if let Some(mut rng) = lua.app_data_mut::<crate::engine::sim::rng::SimRng>() {
            projectile::roll_zigzags(&mut ps, &mut rng);
        }
        projectile::step(&mut ps, &mut w, env, &mut impacts);
    }
    // The rest is the projectiles' scripts: water crossings, collision
    // checks, impacts (and the damage they deal: `OnDamage`, `OnKilled`).
    let _lua = crate::prof_span!("lua projectile impacts");
    // Splashes and torpedo behaviour, before impacts (a torpedo's script
    // retunes it on entering the water).
    // Moho runs these from `Projectile::CheckCollision` (MohoEngine.dll
    // 0x1027e430), called after `SetPendingTransform` (0x1027d6e6) but
    // before the move is committed: the script sees the projectile where
    // it was before the step, and children it makes there
    // (`CreateChildProjectile` takes the committed transform, 0x10283d1f)
    // start on the old side of the surface. The Solace's torpedo cluster
    // splits in `OnEnterWater`: its torpedoes start above the water, enter
    // it next tick and home from their own `OnEnterWater`.
    let crossings = std::mem::take(&mut projectiles(lua).water_crossings);
    let from = std::mem::take(&mut projectiles(lua).water_crossing_from);
    for (i, &(id, entering)) in crossings.iter().enumerate() {
        if let Some(obj) = crate::engine::api::entity::object(lua, id)? {
            let obj = Obj::<moho::Projectile>::new(obj);
            let now = world(lua).get(id).map(|e| e.position());
            let before = from.get(i).copied();
            if let (Some(_), Some(before)) = (now, before) {
                world(lua).set_position(id, before);
            }
            if entering {
                ProjectileCallbacks::on_enter_water(lua, &obj, "projectile")?;
            } else {
                ProjectileCallbacks::on_exit_water(lua, &obj, "projectile")?;
            }
            // Committed after the script, unless it moved the projectile.
            if let (Some(now), Some(before)) = (now, before) {
                let mut w = world(lua);
                if w.get(id).is_some_and(|e| !e.destroyed && e.position() == before) {
                    w.set_position(id, now);
                }
            }
        }
    }
    projectiles(lua).water_crossings = crossings;
    projectiles(lua).water_crossing_from = from;
    // Projectiles entering a script entity's sphere: the entity is asked
    // (`OnCollisionCheck(projectile)`); FAF's flares, missile redirectors
    // and depth charges retarget the projectile there and answer false.
    let entered = std::mem::take(&mut projectiles(lua).script_crossings);
    for &(id, entity) in &entered {
        if let (Some(e), Some(p)) = (crate::engine::api::entity::object(lua, entity)?, crate::engine::api::entity::object(lua, id)?) {
            ask_collision(&e, (e.clone(), p))?;
        }
    }
    projectiles(lua).script_crossings = entered;
    for impact in impacts {
        let Some(obj) = crate::engine::api::entity::object(lua, impact.id)? else { continue };
        let target = match impact.target {
            Some(t) => crate::engine::api::entity::object(lua, t)?.map_or(LuaNil, LuaValue::Table),
            None => LuaNil,
        };
        // Moho asks the unit or shield hit whether the collision counts
        // (`OnCollisionCheck(projectile, firingWeapon)`): dying or upgrading
        // units, allied shots through shields. Refused: fly on past it.
        // Projectile against projectile: both are asked
        // (`OnCollisionCheck(other)`; FAF's missiles only accept the
        // anti-missile shot aimed at them).
        if matches!(impact.kind, ImpactKind::Unit | ImpactKind::UnitAir | ImpactKind::UnitUnderwater | ImpactKind::Shield | ImpactKind::Projectile)
            && let LuaValue::Table(t) = &target {
            let accepted = if impact.kind == ImpactKind::Projectile {
                ask_collision(t, (t.clone(), obj.clone()))? && ask_collision(&obj, (obj.clone(), t.clone()))?
            } else {
                ask_collision(t, (t.clone(), obj.clone(), LuaNil))?
            };
            if !accepted {
                if let Some(p) = projectiles(lua).get_mut(impact.id) {
                    p.last_hit = impact.target;
                }
                continue;
            }
        }
        report_shot(lua, &impact);
        ProjectileCallbacks::on_impact(lua, &Obj::<moho::Projectile>::new(obj.clone()), "projectile", impact.kind.as_str(), crate::lua_api::shapes::Loose::new(target))?;
        let alive = world(lua).get(impact.id).is_some_and(|e| !e.destroyed);
        if !alive {
            continue;
        }
        match impact.kind {
            ImpactKind::Air => crate::engine::api::entity::destroy(lua, &Obj::new(obj.clone()))?,
            // Still flying after hitting the ground: no more surface hits.
            ImpactKind::Terrain => {
                if let Some(p) = projectiles(lua).get_mut(impact.id) {
                    p.collide_surface = false;
                }
            }
            // Through the water surface (`DestroyOnWater = false`): it can
            // still hit the seabed (FAF's sinker, which drags a hover unit
            // dead over water down, destroys it there; the surface itself
            // is only hit coming down through it, so not again).
            ImpactKind::Water => {}
            // Passed through (DestroyOnImpact off): don't hit that unit again.
            ImpactKind::Unit | ImpactKind::UnitAir | ImpactKind::UnitUnderwater | ImpactKind::Shield | ImpactKind::Projectile => {
                if let Some(p) = projectiles(lua).get_mut(impact.id) {
                    p.last_hit = impact.target;
                }
            }
        }
    }
    Ok(())
}

/// `OnCollisionCheck(args)` on `obj`: true unless it answers false or nil
/// (no function, or an error: the collision counts).
fn ask_collision(obj: &LuaTable, args: impl IntoLuaMulti) -> LuaResult<bool> {
    Ok(match obj.get::<Option<LuaFunction>>("OnCollisionCheck")? {
        Some(f) => match f.call::<LuaValue>(args) {
            Ok(LuaValue::Boolean(b)) => b,
            Ok(LuaValue::Nil) => false,
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(target: "lua", "Error running OnCollisionCheck script: {e}");
                true
            }
        },
        None => true,
    })
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Projectiles::default());
    lua_method!(lua, moho::Entity,
        /// At the entity's position plus the offset, both world space (not
        /// rotated by the entity: MohoEngine.dll 0x1026dc8c adds the offset to
        /// the transform's translation); facing the direction, or the
        /// blueprint's without one (`spawn_projectile`). No offset (moho
        /// checks the x only, 0x1026db04): the blueprint's `Position*`.
        CreateProjectile = |lua, (this, bp, ox, oy, oz, dx, dy, dz): (Obj<moho::Entity>, String, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>)| {
            let launcher = entity_id(&this)?;
            let offset = match ox {
                Some(x) => FxVec3::from_f64(x, oy.unwrap_or(0.0), oz.unwrap_or(0.0)),
                None => blueprint_offset(lua, &bp)?,
            };
            let origin = {
                let w = world(lua);
                w.get(launcher).ok_or_else(|| LuaError::runtime("CreateProjectile: entity no longer exists"))?.position()
            };
            let direction = match (dx, dy, dz) {
                (Some(x), Some(y), Some(z)) => Some(FxVec3::from_f64(x, y, z)),
                _ => None,
            };
            spawn_projectile(lua, launcher, &bp, origin + offset, Launch { direction, ..Default::default() })
        })?;
    lua_method!(lua, moho::Entity,
        /// At the bone's posed world position, flying along its posed facing
        /// (inferred).
        CreateProjectileAtBone = |lua, (this, bp, bone): (Obj<moho::Entity>, String, shapes::Bone)| {
        let launcher = entity_id(&this)?;
        let index = crate::engine::api::manipulator::resolve_bone(lua, launcher, &bone)?;
        let (origin, direction) = {
            let w = world(lua);
            let e = w.get(launcher).ok_or_else(|| LuaError::runtime("CreateProjectileAtBone: entity no longer exists"))?;
            let (pos, rot) = crate::engine::api::entity::posed_bone(lua, e, index);
            (pos, rot.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE }))
        };
        spawn_projectile(lua, launcher, &bp, origin, Launch { direction: Some(direction), ..Default::default() })
    })?;
    lua_method!(lua, moho::Projectile, GetLauncher = |lua, this: Obj<moho::Projectile>| {
        let launcher = with_projectile(lua, &this, |p| p.launcher)?;
        match launcher {
            Some(id) => crate::engine::api::entity::object_as::<moho::Unit>(lua, id),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Projectile,
        /// Speeds are world units per second here (blueprint units); moho's
        /// `GetVelocity` / `GetCurrentSpeed` report per tick (x 0.1), per the
        /// annotations. `SetVelocity(speed)` keeps the direction.
        SetVelocity = |lua, (this, x, y, z): (Obj<moho::Projectile>, f64, Option<f64>, Option<f64>)| {
            let id = entity_id(&this)?;
            let facing = world(lua).get(id).map(|e| e.orientation).unwrap_or(FxQuat::IDENTITY);
            with_projectile(lua, &this, |p| {
                let v = match (y, z) {
                    (Some(y), Some(z)) => FxVec3::from_f64(x, y, z),
                    _ => {
                        let dir = p.velocity().normalized().unwrap_or_else(|| facing.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE }));
                        dir * crate::engine::sim::math::from_lua(x)
                    }
                };
                p.set_velocity(v);
            })?;
            Ok(this)
        })?;
    lua_method!(lua, moho::Projectile, GetVelocity = |lua, this: Obj<moho::Projectile>| {
        let v = with_projectile(lua, &this, |p| p.velocity())? * (Fx::ONE / Fx::from_num(10));
        Ok(v.to_f64())
    })?;
    lua_method!(lua, moho::Projectile, GetCurrentSpeed = |lua, this: Obj<moho::Projectile>| {
        let v = with_projectile(lua, &this, |p| p.velocity())?;
        Ok((v.length() / Fx::from_num(10)).to_num::<f64>())
    })?;
    /// Registers a chainable setter `name(value)` on projectiles.
    fn setter(lua: &Lua, name: &'static str, f: fn(&mut ProjectileState, f64)) -> LuaResult<()> {
        crate::lua_api::add_class_fn(lua, moho::Projectile::CLASS, name, None, "", Status::Implemented, move |lua, (this, v): (Obj<moho::Projectile>, f64)| {
            with_projectile(lua, &this, |p| f(p, v))?;
            Ok(this)
        })
    }
    setter(lua, "SetAcceleration", |p, v| p.acceleration = crate::engine::sim::math::from_lua(v))?;
    setter(lua, "SetBallisticAcceleration", |p, v| p.ballistic_acceleration = crate::engine::sim::math::from_lua(v))?;
    setter(lua, "SetMaxSpeed", |p, v| p.max_speed = (v > 0.0).then(|| crate::engine::sim::math::from_lua(v)))?;
    setter(lua, "SetLifetime", |p, v| p.set_lifetime(Some(crate::engine::sim::math::from_lua(v))))?;
    setter(lua, "SetTurnRate", |p, v| p.turn_rate = crate::engine::sim::math::from_lua(v))?;
    /// Registers a flag setter `name(bool)`; `chain` returns self.
    fn flag(lua: &Lua, name: &'static str, chain: bool, f: fn(&mut ProjectileState, bool)) -> LuaResult<()> {
        // Chaining setters return `self`; the others return nothing.
        if chain {
            crate::lua_api::add_class_fn(lua, moho::Projectile::CLASS, name, None, "", Status::Implemented, move |lua, (this, on): (Obj<moho::Projectile>, bool)| {
                with_projectile(lua, &this, |p| f(p, on))?;
                Ok(this)
            })
        } else {
            crate::lua_api::add_class_fn(lua, moho::Projectile::CLASS, name, None, "", Status::Implemented, move |lua, (this, on): (Obj<moho::Projectile>, bool)| {
                with_projectile(lua, &this, |p| f(p, on))
            })
        }
    }
    flag(lua, "SetCollideEntity", true, |p, on| p.collide_entity = on)?;
    flag(lua, "SetCollideSurface", true, |p, on| p.collide_surface = on)?;
    flag(lua, "TrackTarget", true, |p, on| p.track_target = on)?;
    flag(lua, "StayUnderwater", true, |p, on| p.stay_underwater = on)?;
    flag(lua, "SetDestroyOnWater", false, |p, on| p.destroy_on_water = on)?;
    flag(lua, "SetVelocityAlign", false, |p, on| p.velocity_align = on)?;
    flag(lua, "SetStayUpright", false, |p, on| p.stay_upright = on)?;
    lua_method!(lua, moho::Projectile,
        /// `SetScaleVelocity(x, y, z)` (or one value for all): the mesh grows by
        /// that much per second (render-only).
        SetScaleVelocity = |lua, (this, x, y, z): (Obj<moho::Projectile>, f64, Option<f64>, Option<f64>)| {
        let v = FxVec3::from_f64(x, y.unwrap_or(x), z.unwrap_or(x));
        with_projectile(lua, &this, |p| p.scale_velocity = (v != FxVec3::default()).then_some(v))?;
        Ok(this)
    })?;
    lua_method!(lua, moho::Projectile,
        /// Spin, radians a second about the projectile's own axes (moho
        /// stores it as given, in the slot the blueprint's spin fills).
        SetLocalAngularVelocity = |lua, (this, x, y, z): (Obj<moho::Projectile>, f64, f64, f64)| {
        with_projectile(lua, &this, |p| p.angular_velocity = FxVec3::from_f64(x, y, z))?;
        Ok(this)
    })?;
    // Moho sets the projectile's own damage amount / radius (nil leaves
    // one unchanged); FAF's projectiles deal damage from Lua (`DoDamage`)
    // and its scripts never call this.
    lua_method!(lua, moho::Projectile,
        #[lua(stub)]
        SetDamage = |_, (_this, _v): (Obj<moho::Projectile>, LuaValue)| Ok(()))?;
    // Zig-zag and detonation heights: moho keeps each override (-1 at
    // creation) beside the blueprint value and reads the blueprint's while
    // the override is negative (MohoEngine.dll 0x1027de73, 0x1027ef97); we
    // resolve it when changed. None chain (annotations).
    fn change(lua: &Lua, name: &'static str, blueprint: fn(&ProjectileTemplate) -> Fx, set: fn(&mut ProjectileState, Fx)) -> LuaResult<()> {
        crate::lua_api::add_class_fn(lua, moho::Projectile::CLASS, name, None, "", Status::Implemented, move |lua, (this, v): (Obj<moho::Projectile>, f64)| {
            let value = if v < 0.0 {
                let id = entity_id(&this)?;
                let bp = world(lua).get(id).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
                projectiles(lua).template(&bp).map_or(Fx::ZERO, |t| blueprint(&t))
            } else {
                crate::engine::sim::math::from_lua(v)
            };
            with_projectile(lua, &this, |p| set(p, value))
        })
    }
    change(lua, "ChangeMaxZigZag", |t| t.max_zigzag, |p, v| p.max_zigzag = v)?;
    change(lua, "ChangeZigZagFrequency", |t| t.zigzag_frequency, |p, v| p.zigzag_frequency = v)?;
    change(lua, "ChangeDetonateAboveHeight", |t| t.detonate_above, |p, v| p.detonate_above = v)?;
    change(lua, "ChangeDetonateBelowHeight", |t| t.detonate_below, |p, v| p.detonate_below = v)?;
    lua_method!(lua, moho::Projectile,
        /// FAF's engine patch: the override, -1 while there is none (FAF's Lua
        /// then reads the blueprint). Ours: the value in force, which FAF's
        /// wrappers return either way.
        GetMaxZigZag = |lua, this: Obj<moho::Projectile>| with_projectile(lua, &this, |p| p.max_zigzag.to_num::<f64>()))?;
    lua_method!(lua, moho::Projectile, GetZigZagFrequency = |lua, this: Obj<moho::Projectile>| with_projectile(lua, &this, |p| p.zigzag_frequency.to_num::<f64>()))?;
    lua_method!(lua, moho::Projectile, SetNewTarget = |lua, (this, target): (Obj<moho::Projectile>, Obj<moho::Entity>)| {
        let t = entity_id(&target)?;
        with_projectile(lua, &this, |p| p.target = Some(crate::engine::sim::projectile::ProjectileTarget::Entity(t)))
    })?;
    lua_method!(lua, moho::Projectile, SetNewTargetGround = |lua, (this, pos): (Obj<moho::Projectile>, shapes::Vector)| {
        let p3 = FxVec3::from_f64(pos.raw_get(1)?, pos.raw_get(2)?, pos.raw_get(3)?);
        with_projectile(lua, &this, |p| p.target = Some(crate::engine::sim::projectile::ProjectileTarget::Ground(p3)))
    })?;
    lua_method!(lua, moho::Projectile, SetNewTargetGroundXYZ = |lua, (this, x, y, z): (Obj<moho::Projectile>, f64, f64, f64)| {
        let p3 = FxVec3::from_f64(x, y, z);
        with_projectile(lua, &this, |p| p.target = Some(crate::engine::sim::projectile::ProjectileTarget::Ground(p3)))
    })?;
    fn target_position(lua: &Lua, this: &LuaTable) -> LuaResult<Option<FxVec3>> {
        use crate::engine::sim::projectile::ProjectileTarget;
        Ok(match with_projectile(lua, this, |p| p.target)? {
            Some(ProjectileTarget::Ground(p)) => Some(p),
            Some(ProjectileTarget::Entity(id)) => world(lua).get(id).map(|e| e.position()),
            None => None,
        })
    }
    lua_method!(lua, moho::Projectile,
        /// Without a target: where the projectile is (FAF's scripts assume a
        /// position).
        GetCurrentTargetPosition = |lua, this: Obj<moho::Projectile>| {
        let p = match target_position(lua, &this)? {
            Some(p) => p,
            None => world(lua).get(entity_id(&this)?).map(|e| e.position()).unwrap_or_default(),
        };
        crate::engine::api::entity::vector(lua, p)
    })?;
    lua_method!(lua, moho::Projectile, GetCurrentTargetPositionXYZ = |lua, this: Obj<moho::Projectile>| {
        Ok(target_position(lua, &this)?.unwrap_or_default().to_f64())
    })?;
    lua_method!(lua, moho::Projectile, GetTrackingTarget = |lua, this: Obj<moho::Projectile>| {
        match with_projectile(lua, &this, |p| p.target)? {
            Some(crate::engine::sim::projectile::ProjectileTarget::Entity(id)) => crate::engine::api::entity::object_as::<moho::Entity>(lua, id),
            _ => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Projectile,
        /// A child at the parent's position, heading the same way at the same
        /// speed, aimed at the parent's target (fragmentation shells, splitting
        /// missiles). Its launcher is the parent projectile, as moho's
        /// `PROJ_Create(sim, bp, army, parent, parent transform, ...)` call
        /// (MohoEngine.dll 0x10283d31); FAF's wrapper copies the parent's
        /// `Launcher` field for damage credit.
        CreateChildProjectile = |lua, (this, bp): (Obj<moho::Projectile>, String)| {
        let id = entity_id(&this)?;
        let (position, facing) = {
            let w = world(lua);
            let e = w.get(id).ok_or_else(|| LuaError::runtime("CreateChildProjectile: projectile no longer exists"))?;
            (e.position(), e.orientation)
        };
        let (velocity, target) = projectiles(lua).get(id).map(|p| (p.velocity(), p.target)).unwrap_or((FxVec3::default(), None));
        let direction = velocity.normalized().or_else(|| Some(facing.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE })));
        let launch = Launch { direction, speed: Some(velocity.length()), target, ..Default::default() };
        spawn_projectile(lua, id, &bp, position, launch)
    })?;
    lua_method!(lua, moho::Projectile,
        /// Toggles both entity and surface collision; returns self for chaining.
        SetCollision = |lua, (this, collide): (Obj<moho::Projectile>, bool)| {
        with_projectile(lua, &this, |p| {
            p.collide_entity = collide;
            p.collide_surface = collide;
        })?;
        Ok(this)
    })?;
    Ok(())
}
