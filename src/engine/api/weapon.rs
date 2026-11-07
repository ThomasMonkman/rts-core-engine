//! Unit weapons: script objects (instances of the unit class's `Weapons[label]`)
//! and `moho.weapon_methods`. The sim side (targeting, aim, fire clock) is
//! `sim/weapon.rs`; [`tick`] runs it and dispatches its events.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::callbacks::WeaponCallbacks;
use crate::engine::api::{entity, moho, script_object};
use crate::lua_api::{Obj, lua_method};
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::layer::LayerMask;
use crate::engine::sim::math::{Fx, FxQuat, FxVec3};
use crate::engine::sim::weapon::{Trajectory, WeaponEventKind, WeaponState, WeaponTarget, Weapons};

/// `_c_object` of a weapon: owning unit + 1-based weapon index.
#[derive(Clone, Copy, Debug)]
pub struct WeaponHandle {
    pub unit: EntityId,
    pub index: u32,
}
impl LuaUserData for WeaponHandle {}

const WEAPONS_KEY: &str = "engine.weapons";

/// Default seconds between target searches for weapons without
/// `TargetCheckInterval` (inferred; not verified against the binary).
const DEFAULT_TARGET_CHECK: f64 = 0.5;

/// Registry table: unit entity id -> array of weapon objects.
fn objects(lua: &Lua) -> LuaResult<LuaTable> {
    crate::engine::api::script_object::registry_table(lua, WEAPONS_KEY)
}

pub(crate) fn weapon_object(lua: &Lua, unit: EntityId, index: u32) -> LuaResult<Option<LuaTable>> {
    match objects(lua)?.raw_get::<Option<LuaTable>>(unit)? {
        Some(list) => list.raw_get(index),
        None => Ok(None),
    }
}

fn deg(v: f64) -> Fx {
    crate::engine::sim::math::from_lua(v) * Fx::PI / Fx::from_num(180)
}

/// Fills the blueprint-driven part of a weapon's state.
fn configure(lua: &Lua, st: &mut WeaponState, unit: EntityId, wbp: &LuaTable) -> LuaResult<()> {
    let num = |k: &str| -> LuaResult<Option<f64>> { wbp.get::<Option<f64>>(k) };
    let fx = |v: Option<f64>| crate::engine::sim::math::from_lua(v.unwrap_or(0.0));
    st.max_radius = fx(num("MaxRadius")?);
    st.min_radius = fx(num("MinRadius")?);
    st.tracking_radius = crate::engine::sim::math::from_lua(num("TrackingRadius")?.unwrap_or(1.0));
    st.rate_of_fire = crate::engine::sim::math::from_lua(num("RateOfFire")?.unwrap_or(1.0));
    st.firing_tolerance = deg(num("FiringTolerance")?.unwrap_or(0.0));
    st.slaved_to_body = wbp.get::<Option<bool>>("SlavedToBody")?.unwrap_or(false);
    // Default as `RUnitBlueprintWeapon`'s constructor sets it (MohoEngine.dll
    // 0x1010df8c: 1 degree).
    st.slaved_arc = deg(num("SlavedToBodyArcRange")?.unwrap_or(1.0));
    let check = num("TargetCheckInterval")?.unwrap_or(DEFAULT_TARGET_CHECK);
    st.target_check_ticks = crate::engine::sim::projectile::seconds_to_ticks(crate::engine::sim::math::from_lua(check)).max(1);
    // Default on, as `RUnitBlueprintWeapon`'s constructor sets it
    // (MohoEngine.dll 0x1010dfda); FAF's blueprint pass sets it per weapon.
    st.always_recheck = wbp.get::<Option<bool>>("AlwaysRecheckTarget")?.unwrap_or(true);
    st.muzzle_velocity = fx(num("MuzzleVelocity")?);
    st.muzzle_velocity_given = num("MuzzleVelocity")?.is_some();
    st.bomb_drop = match wbp.get::<Option<bool>>("NeedToComputeBombDrop")? {
        Some(true) => Some(fx(num("BombDropThreshold")?).max(Fx::ONE)),
        _ => None,
    };
    st.auto_initiate = wbp.get::<Option<bool>>("AutoInitiateAttackCommand")?.unwrap_or(false);
    st.collide_friendly = wbp.get::<Option<bool>>("CollideFriendly")?.unwrap_or(false);
    st.firing_randomness = fx(num("FiringRandomness")?);
    st.projectile_id = wbp.get::<Option<String>>("ProjectileId")?.unwrap_or_default().to_lowercase();
    st.projectile_lifetime = match (num("ProjectileLifetime")?, num("ProjectileLifetimeUsesMultiplier")?) {
        (Some(t), _) if t > 0.0 => Some(crate::engine::sim::math::from_lua(t)),
        // Long enough to cross the weapon's range, with margin.
        (_, Some(m)) if m > 0.0 && st.muzzle_velocity > Fx::ZERO => {
            Some(crate::engine::sim::math::from_lua(m) * st.max_radius / st.muzzle_velocity)
        }
        _ => None,
    };
    st.muzzle_velocity_reduce_distance = fx(num("MuzzleVelocityReduceDistance")?);
    st.muzzle_velocity_random = fx(num("MuzzleVelocityRandom")?);
    st.lead_target = wbp.get::<Option<bool>>("LeadTarget")?.unwrap_or(true);
    // As moho's firing solution (MohoEngine.dll 0x1021b5e4..): projectiles
    // under gravity (`UseGravity`, default on) are aimed along a ballistic
    // arc, unless they home (`TrackTarget`), which fly straight at the
    // target and are led at their `MaxSpeed`.
    let physics = if st.projectile_id.is_empty() {
        None
    } else {
        entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Projectile, &st.projectile_id)
            .ok()
            .map(|bp| bp.get::<Option<LuaTable>>("Physics").ok().flatten())
    };
    let flag = |k: &str, default: bool| physics.as_ref().and_then(|p| p.as_ref()?.get::<Option<bool>>(k).ok().flatten()).unwrap_or(default);
    let (gravity, homing) = match physics {
        Some(_) => (flag("UseGravity", true), flag("TrackTarget", false)),
        None => (false, false),
    };
    st.lead_speed = homing.then(|| {
        let max = physics.as_ref().and_then(|p| p.as_ref()?.get::<Option<f64>>("MaxSpeed").ok().flatten()).unwrap_or(0.0);
        crate::engine::sim::math::from_lua(max)
    });
    st.trajectory = match (gravity && !homing, wbp.get::<Option<String>>("BallisticArc")?.as_deref()) {
        (false, _) => Trajectory::Direct,
        (true, Some("RULEUBA_HighArc")) => Trajectory::HighArc,
        (true, _) => Trajectory::LowArc,
    };
    st.fire_arc = match wbp.get::<Option<String>>("BallisticArc")?.as_deref() {
        Some("RULEUBA_LowArc") => crate::engine::sim::terrain::FireArc::Low,
        Some("RULEUBA_HighArc") => crate::engine::sim::terrain::FireArc::High,
        _ => crate::engine::sim::terrain::FireArc::None,
    };
    st.manual_fire = wbp.get::<Option<bool>>("ManualFire")?.unwrap_or(false);
    st.overcharge = wbp.get::<Option<bool>>("OverChargeWeapon")?.unwrap_or(false);
    st.prefers_primary_target = wbp.get::<Option<bool>>("PrefersPrimaryWeaponTarget")?.unwrap_or(false);
    st.stop_on_primary_busy = wbp.get::<Option<bool>>("StopOnPrimaryWeaponBusy")?.unwrap_or(false);
    st.aims_straight_on_disable = wbp.get::<Option<bool>>("AimsStraightOnDisable")?.unwrap_or(false);
    st.yaw_only_on_target = wbp.get::<Option<bool>>("YawOnlyOnTarget")?.unwrap_or(false);
    st.ignore_if_disabled = wbp.get::<Option<bool>>("IgnoreIfDisabled")?.unwrap_or(false);
    st.use_firing_solution = wbp.get::<Option<bool>>("UseFiringSolutionInsteadOfAimBone")?.unwrap_or(false);
    st.retarget = wbp.get::<Option<bool>>("ReTargetOnMiss")?.unwrap_or(false).then(Default::default);
    st.fixed_muzzle = wbp.get::<Option<bool>>("Turreted")? == Some(false);
    st.turreted = wbp.get::<Option<bool>>("Turreted")? == Some(true);
    st.blueprint_max_height_diff = num("MaxHeightDiff")?.map(crate::engine::sim::math::from_lua);
    st.heading_arc = match num("HeadingArcRange")? {
        Some(range) if range < 180.0 => Some((deg(num("HeadingArcCenter")?.unwrap_or(0.0)), deg(range))),
        _ => None,
    };
    st.above_water_fire_only = wbp.get::<Option<bool>>("AboveWaterFireOnly")?.unwrap_or(false);
    st.below_water_fire_only = wbp.get::<Option<bool>>("BelowWaterFireOnly")?.unwrap_or(false);
    st.above_water_targets_only = wbp.get::<Option<bool>>("AboveWaterTargetsOnly")?.unwrap_or(false);
    st.below_water_targets_only = wbp.get::<Option<bool>>("BelowWaterTargetsOnly")?.unwrap_or(false);
    st.cannot_attack_ground = wbp.get::<Option<bool>>("CannotAttackGround")?.unwrap_or(false);
    st.max_beam_length = num("MaximumBeamLength")?.filter(|&l| l > 0.0).map(crate::engine::sim::math::from_lua);
    st.fixed_beam_muzzle = None;
    if wbp.get::<Option<bool>>("Turreted")? == Some(false) && num("BeamLifetime")?.is_some() {
        let first = wbp
            .get::<Option<LuaTable>>("RackBones")?
            .and_then(|r| r.get::<Option<LuaTable>>(1).ok().flatten())
            .and_then(|r| r.get::<Option<LuaTable>>("MuzzleBones").ok().flatten())
            .and_then(|m| m.get::<Option<LuaValue>>(1).ok().flatten());
        if let Some(bone) = first {
            st.fixed_beam_muzzle = crate::engine::api::manipulator::resolve_bone(lua, unit, &bone)?;
        }
    }
    st.targets_projectiles = wbp.get::<Option<String>>("TargetType")?.as_deref() == Some("RULEWTT_Projectile");
    let restriction = |k: &str| -> LuaResult<Option<crate::engine::sim::category::CategorySet>> {
        match wbp.get::<Option<String>>(k)? {
            Some(t) => crate::engine::api::categories::parse_restriction(lua, &t),
            None => Ok(None),
        }
    };
    st.only_allow = restriction("TargetRestrictOnlyAllow")?;
    st.disallow = restriction("TargetRestrictDisallow")?;
    st.max_storage = wbp.get::<Option<f64>>("MaxProjectileStorage")?.unwrap_or(0.0).max(0.0) as u32;
    st.silo = match wbp.get::<Option<bool>>("CountedProjectile")?.unwrap_or(false) {
        false => None,
        true if wbp.get::<Option<bool>>("NukeWeapon")?.unwrap_or(false) => Some(crate::engine::sim::weapon::SiloKind::Nuke),
        true => Some(crate::engine::sim::weapon::SiloKind::Tactical),
    };
    if let Some(bone) = wbp.get::<Option<LuaValue>>("TurretBoneYaw")? {
        st.aim_bone = crate::engine::api::manipulator::resolve_bone(lua, unit, &bone)?;
    }
    Ok(())
}

/// Creates the weapon objects for a unit from `bp.Weapon` (in blueprint order)
/// and runs each weapon's `OnCreate` (before the unit's, as in moho).
pub fn create_weapons(lua: &Lua, unit_obj: &LuaTable, unit: EntityId, bp: &LuaTable) -> LuaResult<()> {
    let list = lua.create_table()?;
    objects(lua)?.raw_set(unit, list.clone())?;
    let Some(weapon_bps) = bp.get::<Option<LuaTable>>("Weapon")? else {
        return Ok(());
    };
    // Each weapon's class comes from the unit (`GetWeaponClass(label)`:
    // FAF's falls back to the base `Weapon` for labels its script doesn't
    // define, e.g. the Cybran ACU's DummyWeapon), else its `Weapons` table.
    let classes: Option<LuaTable> = unit_obj.get("Weapons")?;
    let class_of: Option<LuaFunction> = unit_obj.get("GetWeaponClass")?;
    // `DummyWeapon` entries get no weapon ("the engine should not create an
    // actual weapon for this blueprint": special damage specs, such as death
    // and teleport weapons FAF's scripts read as data). Moho makes one weapon
    // per non-dummy entry and gives the k-th the k-th entry, so dummies must
    // come last (FAF's blueprint loading keeps them there and warns
    // otherwise): the first that many entries, as moho.
    let entries: Vec<LuaTable> = weapon_bps.sequence_values::<LuaTable>().collect::<LuaResult<_>>()?;
    let real = entries.iter().map(|w| w.get::<Option<bool>>("DummyWeapon").map(|d| d != Some(true))).collect::<LuaResult<Vec<bool>>>()?.into_iter().filter(|&r| r).count();
    let mut index = 0u32;
    for wbp in entries.into_iter().take(real) {
        index += 1;
        let label: Option<String> = wbp.get("Label")?;
        let class = match (&class_of, &classes, &label) {
            (Some(f), _, Some(l)) => f.call::<Option<LuaTable>>((unit_obj.clone(), l.as_str()))?,
            (None, Some(c), Some(l)) => c.get::<Option<LuaTable>>(l.as_str())?,
            _ => None,
        };
        let Some(class) = class else {
            tracing::warn!(unit, ?label, "unit has no weapon class for blueprint weapon; skipped");
            continue;
        };
        let obj: LuaTable = class.call(unit_obj)?;
        obj.raw_set(script_object::C_OBJECT, WeaponHandle { unit, index })?;
        let mut st = WeaponState::new(unit, index);
        configure(lua, &mut st, unit, &wbp)?;
        // The blockage test's line leaves 2 x `SizeZ` above the unit (moho
        // reads blueprint +0xb4, `SizeZ`, there, not `SizeY`).
        st.blockage_rise = crate::engine::sim::math::from_lua(bp.get::<Option<f64>>("SizeZ")?.unwrap_or(0.0) * 2.0);
        st.need_unpack = bp.get::<Option<LuaTable>>("AI")?.map(|ai| ai.get::<Option<bool>>("NeedUnpack")).transpose()?.flatten().unwrap_or(false);
        *lua.app_data_mut::<Weapons>().expect("Weapons not in app_data").get_mut(unit, index) = st;
        list.raw_set(index, obj.clone())?;
        WeaponCallbacks::on_create(lua, &Obj::<moho::Weapon>::new(obj.clone()), "weapon")?;
    }
    refresh_attack_range(lua, unit);
    Ok(())
}

/// A destroyed unit's weapons die with it: `OnDestroy` on each (FAF's
/// projectile weapons then enter `DeadState`, which stops a salvo mid-way).
pub fn destroy_weapons(lua: &Lua, unit: EntityId) -> LuaResult<()> {
    let Some(list) = objects(lua)?.raw_get::<Option<LuaTable>>(unit)? else { return Ok(()) };
    for w in list.sequence_values::<LuaTable>() {
        WeaponCallbacks::on_destroy(lua, &Obj::<moho::Weapon>::new(w?), "weapon")?;
    }
    Ok(())
}

/// Recomputes a unit's attack range (its longest weapon `MaxRadius`).
fn refresh_attack_range(lua: &Lua, unit: EntityId) {
    let range = lua
        .app_data_ref::<Weapons>()
        .map(|w| w.of(unit).iter().map(|s| s.max_radius).max().unwrap_or(Fx::ZERO))
        .unwrap_or(Fx::ZERO);
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.attack_range = range;
    }
}

/// Tick pass: targeting, aim and fire clock for every weapon, then the
/// resulting `OnLostTarget` / `OnGotTarget` / `OnFire` callbacks in
/// (unit, weapon) order.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let mut events = Vec::new();
    {
        let (Some(mut weapons), Some(world)) =
            (lua.app_data_mut::<Weapons>(), lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>())
        else {
            return Ok(());
        };
        if weapons.len() == 0 {
            return Ok(());
        }
        let armies = lua.app_data_ref::<crate::engine::sim::army::Armies>();
        let _section = crate::prof_span!("weapons orders");
        // Attack orders point the unit's weapons at their target; nuke /
        // tactical launch orders its counted weapon of that kind.
        weapons.ordered.clear();
        if let Some(commands) = lua.app_data_ref::<crate::engine::sim::command::Commands>() {
            use crate::engine::sim::command::{CommandKind as K, CommandTarget as T};
            for u in commands.units_doing(&[K::Attack, K::FormAttack, K::Nuke, K::Tactical]) {
                let Some(cmd) = commands.current(u) else { continue };
                let silo = match cmd.kind {
                    K::Attack | K::FormAttack => None,
                    K::Nuke => Some(crate::engine::sim::weapon::SiloKind::Nuke),
                    K::Tactical => Some(crate::engine::sim::weapon::SiloKind::Tactical),
                    _ => continue,
                };
                let target = match cmd.target {
                    T::Entity(id) => WeaponTarget::Entity(id),
                    T::Ground(p) => WeaponTarget::Ground(p),
                    T::None => continue,
                };
                weapons.ordered.insert(u, crate::engine::sim::weapon::OrderedTarget { target, silo, overcharge: None });
            }
            // Overcharge orders point their overcharge weapon (`api::overcharge`).
            if let Some(tasks) = lua.app_data_ref::<crate::engine::api::overcharge::OverchargeTasks>() {
                weapons.ordered.extend(tasks.ordered(&commands));
            }
        }
        let _ = _section;
        let _section = crate::prof_span!("weapons projectile views");
        // Projectiles for anti-missile / anti-torpedo weapons, if any.
        let mut views = std::mem::take(&mut weapons.projectiles);
        views.clear();
        if weapons.any_targets_projectiles()
            && let Some(ps) = lua.app_data_ref::<crate::engine::sim::projectile::Projectiles>() {
            views.extend(ps.iter().filter_map(|p| {
                let e = world.get(p.id).filter(|e| !e.destroyed)?;
                Some(crate::engine::sim::weapon::ProjectileView { id: p.id, position: e.position(), velocity: p.velocity(), army: e.army, category: e.category_index })
            }));
        }
        weapons.projectiles = views;
        let _ = _section;
        let _section = crate::prof_span!("weapons muzzles");
        {
            let manips = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>();
            let clips = lua.app_data_ref::<crate::engine::sim::pose::Clips>();
            let (w, m, c): (&crate::engine::sim::entity::EntityWorld, _, _) = (&world, manips.as_deref(), clips.as_deref());
            weapons.pose_muzzles(|s, all| {
                w.get(s.unit).map(|e| {
                    // One walk down the muzzle's chain gives both the aim's
                    // base and the muzzle as finally posed.
                    match m.and_then(|m| crate::engine::sim::pose::aim_base(e, s.index, s.aim_bone, s.pitch_bone, s.muzzle_bone, m, Some(all), c)) {
                        Some(base) => (base.muzzle_posed, Some(base)),
                        None => (crate::engine::sim::pose::bone_world(e, s.muzzle_bone, m, Some(all), c).0, None),
                    }
                })
            });
        }
        let _ = _section;
        let _section = crate::prof_span!("weapons step");
        weapons.tick = crate::engine::api::sim_time::current_tick(lua);
        weapons.water = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().and_then(|t| t.water_surface);
        let intel = lua.app_data_ref::<crate::engine::sim::vision::IntelGrids>();
        // Target searches skip what the terrain blocks (moho's
        // `WeaponTerrainBlockageTest`, on by default).
        let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
        crate::engine::sim::weapon::step_with(&mut weapons, &world, armies.as_deref(), intel.as_deref(), terrain.as_deref(), &mut events, crate::engine::sim::weapon::PARALLEL_MIN);
    }
    auto_initiate_attacks(lua);
    let _events = crate::prof_span!("lua weapon events");
    let now = crate::engine::api::sim_time::current_tick(lua);
    for ev in events {
        if ev.kind == WeaponEventKind::Fire
            && let Some(mut tasks) = lua.app_data_mut::<crate::engine::api::overcharge::OverchargeTasks>() {
            tasks.fired(ev.unit, ev.index, now);
        }
        let Some(obj) = weapon_object(lua, ev.unit, ev.index)? else { continue };
        // The turret starts / stops tracking with its target (FAF: barrel
        // spin sounds); the argument is the tracking aim controller's label,
        // so weapons without one (missile launchers) aren't told.
        // (Read by the event's unit and index: the weapon list, not the
        // script object's handle.)
        let label = if matches!(ev.kind, WeaponEventKind::LostTarget | WeaponEventKind::GotTarget) {
            lua.app_data_ref::<Weapons>().and_then(|w| w.get(ev.unit, ev.index).and_then(|w| w.fire_control.clone().or_else(|| w.first_aim.clone())))
        } else {
            None
        };
        let obj = Obj::<moho::Weapon>::new(obj);
        if let (WeaponEventKind::LostTarget, Some(l)) = (ev.kind, &label) {
            WeaponCallbacks::on_stop_tracking(lua, &obj, "weapon", l.clone())?;
        }
        match ev.kind {
            WeaponEventKind::HaltFire => WeaponCallbacks::on_halt_fire(lua, &obj, "weapon")?,
            WeaponEventKind::LostTarget => WeaponCallbacks::on_lost_target(lua, &obj, "weapon")?,
            WeaponEventKind::GotTarget => WeaponCallbacks::on_got_target(lua, &obj, "weapon")?,
            WeaponEventKind::Fire => {
                // Moho calls `OnGotTarget` from the target task
                // (`UnitWeapon::SetTarget`, MohoEngine.dll 0x102b1fc1), before
                // its fire task (`OnFire` 0x102b22d9) checks the weapon is
                // enabled; ours decides the shot first. A script that
                // disabled the weapon in the meantime (FAF's unpacking state,
                // `WeaponWantEnabled = false`) gets no shot and keeps its
                // reload: the Aeon TMD fires its flare once unpacked, not 4 s
                // later (FAF probe probe_tmd).
                let disabled = lua.app_data_mut::<Weapons>().is_some_and(|mut w| {
                    let s = w.get_mut(ev.unit, ev.index);
                    let disabled = !s.enabled;
                    if disabled {
                        s.cancel_shot();
                    }
                    disabled
                });
                // Nor does a unit an earlier event this tick killed (the pass
                // holds a killed unit's fire): the Fire Beetle's Suicide
                // `OnFire` kills it, and its ExplosionRadiusRingDummyWeapon,
                // holding the same target and as ready, never fires in FAF
                // (probe_firsttick).
                let killed = entity::world(lua).get(ev.unit).is_none_or(|e| e.destroyed || e.cold.killed);
                if !disabled && !killed {
                    crate::engine::api::orders::weapon_fired(lua, ev.unit);
                    WeaponCallbacks::on_fire(lua, &obj, "weapon")?;
                }
            }
        }
        if let (WeaponEventKind::GotTarget, Some(l)) = (ev.kind, label) {
            WeaponCallbacks::on_start_tracking(lua, &obj, "weapon", l)?;
        }
    }
    Ok(())
}

/// `AutoInitiateAttackCommand`: a unit with no orders whose flagged weapon
/// found a target is ordered to attack it (FAF's wiki, `Blueprints.md`; every
/// FAF aircraft has one, the strategic bombers on a target finder that never
/// fires), so idle aircraft leave their holding pattern for what's near.
/// The order ends with its target and the next one is found the same way.
fn auto_initiate_attacks(lua: &Lua) {
    let found = match lua.app_data_ref::<Weapons>() {
        Some(w) => w.auto_initiated(),
        None => return,
    };
    if found.is_empty() {
        return;
    }
    // Not under hold fire (1) or hold ground (2): those don't go after targets.
    let idle: Vec<(EntityId, EntityId)> = {
        let Some(commands) = lua.app_data_ref::<crate::engine::sim::command::Commands>() else { return };
        let world = entity::world(lua);
        found
            .into_iter()
            .filter(|&(u, _)| commands.current(u).is_none() && world.get(u).and_then(|e| e.unit.as_deref()).is_some_and(|d| d.fire_state == 0))
            .collect()
    };
    for (unit, target) in idle {
        crate::engine::api::command::issue(lua, &[unit], crate::engine::sim::command::CommandKind::Attack, crate::engine::sim::command::CommandTarget::Entity(target), None, 0);
    }
}

fn handle(obj: &LuaTable) -> LuaResult<WeaponHandle> {
    script_object::handle::<WeaponHandle>(obj)
}

fn with_state<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut WeaponState) -> R) -> LuaResult<R> {
    let h = handle(this)?;
    let mut w = lua.app_data_mut::<Weapons>().expect("Weapons not in app_data");
    Ok(f(w.get_mut(h.unit, h.index)))
}

/// World position the weapon is shooting at, if it has a target.
/// Where the weapon's target is, as moho's `GetCurrentTargetPos` (0x102b5090)
/// reads it: a unit's target point (`Unit::GetTargetPoint` through
/// 0x101cf6c0 with its position flag clear), the bone the weapon picked or
/// else the point it aims at, not its origin. FAF's flak bursts at that
/// height (`DetonatesAtTargetHeight`): from the origin, below an aircraft's
/// centre, a climbing shell burst 5-16 short.
fn target_position(lua: &Lua, w: &WeaponState) -> Option<FxVec3> {
    match w.target? {
        WeaponTarget::Ground(p) => Some(p),
        WeaponTarget::Entity(id) => entity::world(lua).get(id).map(|e| crate::engine::sim::weapon::target_point_of(w, e)),
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Weapons::default());
    lua_method!(lua, moho::Weapon,
        /// Sets the weapon's maximum range (world units) and refreshes the unit's attack range.
        ChangeMaxRadius = |lua, (this, r): (Obj<moho::Weapon>, f64)| {
        with_state(lua, &this, |w| w.max_radius = crate::engine::sim::math::from_lua(r))?;
        refresh_attack_range(lua, handle(&this)?.unit);
        Ok(())
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets how far above or below the unit a target may be; a negative
        /// value uses the blueprint's `MaxHeightDiff` (MohoEngine.dll
        /// 0x102b58e0, read by the range test 0x102b19a0).
        ChangeMaxHeightDiff = |lua, (this, v): (Obj<moho::Weapon>, f64)| {
        with_state(lua, &this, |w| {
            w.max_height_diff = if v < 0.0 { w.blueprint_max_height_diff } else { Some(crate::engine::sim::math::from_lua(v)) };
        })
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets the weapon's minimum range (world units).
        ChangeMinRadius = |lua, (this, r): (Obj<moho::Weapon>, f64)| {
        with_state(lua, &this, |w| w.min_radius = crate::engine::sim::math::from_lua(r))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets shots per second.
        ChangeRateOfFire = |lua, (this, r): (Obj<moho::Weapon>, f64)| {
        with_state(lua, &this, |w| w.rate_of_fire = crate::engine::sim::math::from_lua(r))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets how far (degrees) the turret may be off target and still fire.
        ChangeFiringTolerance = |lua, (this, t): (Obj<moho::Weapon>, f64)| {
        with_state(lua, &this, |w| w.firing_tolerance = deg(t))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets the aim controller (by label) that gates firing.
        SetFireControl = |lua, (this, label): (Obj<moho::Weapon>, String)| {
        let h = handle(&this)?;
        with_state(lua, &this, |w| w.fire_control = Some(label))?;
        crate::engine::api::manipulator::sync_fire_control(lua, h.unit, h.index);
        crate::engine::api::manipulator::sync_aim_activity(lua, h.unit, h.index);
        Ok(())
    })?;
    lua_method!(lua, moho::Weapon,
        /// True if `label` is the weapon's fire-control aim controller.
        IsFireControl = |lua, (this, label): (Obj<moho::Weapon>, String)| {
        with_state(lua, &this, |w| w.is_fire_control(&label))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Enables or disables the weapon.
        SetEnabled = |lua, (this, on): (Obj<moho::Weapon>, bool)| {
        with_state(lua, &this, |w| w.enabled = on)
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets the target priority list: entity categories, in order.
        SetTargetingPriorities = |lua, (this, list): (Obj<moho::Weapon>, shapes::List<crate::engine::api::categories::EntityCategory>)| {
        let mut priorities = Vec::new();
        for v in list.into_inner().sequence_values::<LuaUserDataRef<crate::engine::api::categories::EntityCategory>>() {
            priorities.push(v?.0.clone());
        }
        with_state(lua, &this, |w| w.priorities = priorities)
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets the layers the weapon may target (a layer-mask string).
        SetFireTargetLayerCaps = |lua, (this, mask): (Obj<moho::Weapon>, String)| {
        with_state(lua, &this, |w| w.fire_target_layers = LayerMask::parse(&mask))
    })?;
    lua_method!(lua, moho::Weapon,
        /// The layers the weapon may target (a layer-mask string), for the
        /// weapon audit (`dev/audit/`). Not moho's: its weapons have no getter.
        __engine_GetFireTargetLayerCaps = |lua, this: Obj<moho::Weapon>| {
        with_state(lua, &this, |w| w.fire_target_layers.as_mask_str())
    })?;
    lua_method!(lua, moho::Weapon,
        /// Fires now, whatever the fire clock and target say: runs `OnFire` (FAF's
        /// fire beetle detonates through it, the Seraphim energy ball fires its
        /// beam at the target it just set). True if it fired (inferred: false
        /// when the unit is gone).
        FireWeapon = |lua, this: Obj<moho::Weapon>| {
        let WeaponHandle { unit, .. } = handle(&this)?;
        if !entity::world(lua).get(unit).is_some_and(|e| !e.destroyed) {
            return Ok(false);
        }
        WeaponCallbacks::on_fire(lua, &this, "weapon")?;
        Ok(true)
    })?;
    lua_method!(lua, moho::Weapon,
        /// Overrides the weapon's damage.
        ChangeDamage = |lua, (this, damage): (Obj<moho::Weapon>, f64)| {
        with_state(lua, &this, |w| w.damage = Some(crate::engine::sim::math::from_lua(damage)))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Sets the firing spread (degrees, gaussian); nil is 0.
        SetFiringRandomness = |lua, (this, r): (Obj<moho::Weapon>, Option<f64>)| {
        with_state(lua, &this, |w| w.firing_randomness = crate::engine::sim::math::from_lua(r.unwrap_or(0.0)))
    })?;
    lua_method!(lua, moho::Weapon,
        /// The firing spread (degrees).
        GetFiringRandomness = |lua, this: Obj<moho::Weapon>| {
        with_state(lua, &this, |w| w.firing_randomness.to_num::<f64>())
    })?;
    lua_method!(lua, moho::Weapon,
        /// A weapon dies with its unit.
        BeenDestroyed = |lua, this: Obj<moho::Weapon>| {
        let h = handle(&this)?;
        Ok(entity::world(lua).get(h.unit).is_none_or(|u| u.destroyed))
    })?;
    lua_method!(lua, moho::Weapon,
        /// True if the weapon currently has a target.
        WeaponHasTarget = |lua, this: Obj<moho::Weapon>| {
        with_state(lua, &this, |w| w.target.is_some())
    })?;
    lua_method!(lua, moho::Weapon,
        /// The targeted entity, or nil (no target, or a ground target).
        GetCurrentTarget = |lua, this: Obj<moho::Weapon>| {
        match with_state(lua, &this, |w| w.target)? {
            Some(WeaponTarget::Entity(id)) => entity::object_as::<moho::Entity>(lua, id),
            _ => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Weapon,
        /// World position of the current target as a vector, or nil without a target.
        GetCurrentTargetPos = |lua, this: Obj<moho::Weapon>| {
        match with_state(lua, &this, |w| target_position(lua, w))? {
            Some(p) => entity::vector(lua, p).map(Some),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Weapon,
        /// Forget the target; the next tick searches again.
        ResetTarget = |lua, this: Obj<moho::Weapon>| {
        with_state(lua, &this, |w| {
            w.target = None;
            w.check_countdown = 0;
        })
    })?;
    lua_method!(lua, moho::Weapon,
        /// Targets `target` (an entity).
        SetTargetEntity = |lua, (this, target): (Obj<moho::Weapon>, Obj<moho::Entity>)| {
        let id = entity::entity_id(&target)?;
        with_state(lua, &this, |w| w.target = Some(WeaponTarget::Entity(id)))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Targets a ground position (an `{x, y, z}` table).
        SetTargetGround = |lua, (this, loc): (Obj<moho::Weapon>, shapes::Vector)| {
        let p = FxVec3::from_f64(loc.raw_get(1)?, loc.raw_get(2)?, loc.raw_get(3)?);
        with_state(lua, &this, |w| w.target = Some(WeaponTarget::Ground(p)))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Progress of the reload, 1 = ready.
        GetFireClockPct = |lua, this: Obj<moho::Weapon>| {
        with_state(lua, &this, |w| {
            let reload = w.reload_ticks();
            if reload == u32::MAX { 0.0 } else { 1.0 - w.fire_clock.min(reload) as f64 / reload as f64 }
        })
    })?;
    lua_method!(lua, moho::Weapon,
        /// True if the weapon is enabled and its fire clock has run out.
        CanFire = |lua, this: Obj<moho::Weapon>| {
        with_state(lua, &this, |w| w.enabled && w.fire_clock == 0)
    })?;
    lua_method!(lua, moho::Weapon,
        /// The projectile blueprint table, or nil when the weapon has none.
        GetProjectileBlueprint = |lua, this: Obj<moho::Weapon>| {
        let id = with_state(lua, &this, |w| w.projectile_id.clone())?;
        if id.is_empty() {
            return Ok(None);
        }
        entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Projectile, &id).map(|t| Some(shapes::ProjectileBlueprint(t)))
    })?;
    lua_method!(lua, moho::Weapon,
        /// Switches the projectile blueprint (by id).
        ChangeProjectileBlueprint = |lua, (this, bp): (Obj<moho::Weapon>, String)| {
        with_state(lua, &this, |w| w.projectile_id = bp.to_lowercase())
    })?;
    lua_method!(lua, moho::Weapon,
        /// Spawns the weapon's projectile at `muzzle`, along the weapon's aim at
        /// `MuzzleVelocity`, heading for the weapon's target.
        CreateProjectile = |lua, (this, muzzle): (Obj<moho::Weapon>, Option<shapes::Bone>)| {
        use crate::engine::sim::projectile::ProjectileTarget;
        let h = handle(&this)?;
        // A salvo can outlive its unit: nothing to fire from, no projectile.
        // Borrowed, not copied (a whole `WeaponState` per shot was a tenth
        // of a big battle's weapon threads); let go before any Lua runs.
        let weapons = lua.app_data_ref::<Weapons>();
        let Some(st) = weapons.as_deref().and_then(|w| w.get(h.unit, h.index)) else {
            return Ok(None);
        };
        if st.projectile_id.is_empty() {
            return Err(LuaError::runtime("CreateProjectile: weapon has no ProjectileId"));
        }
        let bone = match muzzle.as_deref() {
            None | Some(LuaValue::Nil) => None,
            Some(b) => crate::engine::api::manipulator::resolve_bone(lua, h.unit, b)?,
        };
        let (position, direction, distance) = {
            let w = entity::world(lua);
            let Some(u) = w.get(h.unit).filter(|u| !u.destroyed) else { return Ok(None) };
            // From the muzzle as posed now (recoil, walk cycle); flying
            // along the weapon's aim. Silo missiles never aim, nor do homing
            // missiles from fixed tubes: they leave along the muzzle bone
            // (straight up from a TML or a cruiser's hatches; FAF's flight
            // script then steers them up and over).
            let (at, rotation) = entity::posed_bone(lua, u, bone);
            let direction = if (st.silo.is_some() || (st.fixed_muzzle && st.lead_speed.is_some())) && bone.is_some() {
                rotation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE })
            } else {
                st.aim_direction(st.frame(u))
            };
            // Shots at projectiles are led again from the muzzle as they
            // leave: FAF's weapon thread fires a tick or two after the aim
            // was taken, and a missile covers more than the hit radius in
            // that time.
            let led = match st.target {
                Some(WeaponTarget::Entity(t)) if st.targets_projectiles && st.muzzle_velocity > Fx::ZERO => {
                    let velocity = lua.app_data_ref::<crate::engine::sim::projectile::Projectiles>().and_then(|ps| ps.get(t).map(|p| p.velocity()));
                    match (w.get(t).filter(|e| !e.destroyed).map(|e| e.position()), velocity) {
                        (Some(p), Some(v)) => {
                            let mut aim = p;
                            for _ in 0..3 {
                                aim = p + v * ((aim - at).length() / st.muzzle_velocity);
                            }
                            (aim - at).normalized()
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            // How far the target is from the muzzle, for the launch speed.
            let distance = match st.target {
                Some(WeaponTarget::Entity(t)) => w.get(t).filter(|e| !e.destroyed).map(|e| (crate::engine::sim::weapon::aim_point(e) - at).length()),
                Some(WeaponTarget::Ground(p)) => Some((p - at).length()),
                None => None,
            };
            (at, led.unwrap_or(direction), distance)
        };
        // Moho's `UnitWeapon` projectile launch (MohoEngine.dll 0x102b23c6):
        // with `FiringRandomness` the shot turns by a gaussian times it
        // (degrees) in heading and in pitch, in its own frame; then, if the
        // weapon has a `MuzzleVelocity`, it leaves at that speed (reduced up
        // close, plus a gaussian times `MuzzleVelocityRandom`; bombers' 0
        // drops bombs at the bomber's own speed), else at the projectile's
        // own `InitialSpeed`.
        let random = |lua: &Lua| -> Fx {
            match lua.app_data_mut::<crate::engine::sim::rng::SimRng>() {
                Some(mut r) => r.gaussian(),
                None => Fx::ZERO,
            }
        };
        let direction = if st.firing_randomness > Fx::ZERO && st.silo.is_none() {
            let scale = st.firing_randomness * Fx::PI / Fx::from_num(180);
            let (dh, dp) = (random(lua).saturating_mul(scale), random(lua).saturating_mul(scale));
            FxQuat::look_along(direction).mul(FxQuat::from_hpr(dh, dp, Fx::ZERO)).rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE })
        } else {
            direction
        };
        let speed = st.muzzle_velocity_given.then(|| {
            let mut v = distance.map_or(st.muzzle_velocity, |d| st.muzzle_velocity_at(d));
            if st.muzzle_velocity_random != Fx::ZERO {
                v += random(lua).saturating_mul(st.muzzle_velocity_random);
            }
            v
        });
        let target = match st.target.or(st.last_target) {
            Some(WeaponTarget::Entity(id)) => Some(ProjectileTarget::Entity(id)),
            Some(WeaponTarget::Ground(p)) => Some(ProjectileTarget::Ground(p)),
            None => None,
        };
        let launch = crate::engine::api::projectile::Launch {
            direction: Some(direction),
            speed,
            lifetime: st.projectile_lifetime,
            collide_friendly: st.collide_friendly,
            target,
            // Only shots at a unit are followed (MohoEngine.dll 0x102b27b9).
            reports_to: (st.retarget.is_some() && matches!(target, Some(ProjectileTarget::Entity(_)))).then_some(h.index),
            rescale_after_create: true,
        };
        let projectile_id = st.projectile_id.clone();
        drop(weapons);
        crate::engine::api::projectile::spawn_projectile(lua, h.unit, &projectile_id, position, launch).map(Some)
    })?;
    lua_method!(lua, moho::Weapon,
        /// Queues `sound` at the weapon's unit; nil does nothing.
        PlaySound = |lua, (this, sound): (Obj<moho::Weapon>, Option<LuaUserDataRef<crate::engine::api::sound::SoundHandle>>)| {
        let Some(sound) = sound else { return Ok(()) };
        let h = handle(&this)?;
        crate::engine::api::entity::queue_sound(lua, h.unit, &sound);
        Ok(())
    })?;
    lua_method!(lua, moho::Unit,
        /// Number of weapon objects the unit has.
        GetWeaponCount = |lua, this: Obj<moho::Unit>| {
        let id = entity::entity_id(&this)?;
        Ok(objects(lua)?.raw_get::<Option<LuaTable>>(id)?.map_or(0, |l| l.raw_len()))
    })?;
    lua_method!(lua, moho::Unit,
        /// The unit's weapon object at the 1-based `index`, or nil.
        GetWeapon = |lua, (this, index): (Obj<moho::Unit>, i64)| {
        let id = entity::entity_id(&this)?;
        match objects(lua)?.raw_get::<Option<LuaTable>>(id)? {
            Some(l) => Ok(l.raw_get::<Option<LuaTable>>(index)?.map(Obj::<moho::Weapon>::new)),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Weapon,
        /// The weapon's blueprint table (the unit blueprint's `Weapon[index]`).
        GetBlueprint = |lua, this: Obj<moho::Weapon>| {
        let h = handle(&this)?;
        let unit_obj = entity::object(lua, h.unit)?
            .ok_or_else(|| LuaError::runtime("weapon's unit no longer exists"))?;
        let bp: LuaTable = unit_obj.get::<LuaFunction>("GetBlueprint")?.call(&unit_obj)?;
        Ok(bp.get::<LuaTable>("Weapon")?.get::<Option<LuaTable>>(h.index)?.map(shapes::WeaponBlueprint))
    })?;
    Ok(())
}
