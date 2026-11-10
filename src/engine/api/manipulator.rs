//! Manipulator script objects (`moho.AimManipulator`, ...) and their methods.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::weapon::WeaponHandle;
use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::moho::MohoClass;
use crate::engine::api::{entity, moho, script_object};
use crate::lua_api::{Obj, Status, lua_fn, lua_method};
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::manipulator::{Aim, Animator, BuilderArm, CollisionDetector, FootPlant, FiringArc, ManipulatorId, ManipulatorKind, Manipulators, Slider};
use crate::engine::sim::math::{FxQuat, FxVec3};
use crate::engine::sim::math::Fx;

#[derive(Clone, Copy, Debug)]
pub struct ManipulatorHandle(pub ManipulatorId);
impl LuaUserData for ManipulatorHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

fn manips(lua: &Lua) -> mlua::AppDataRefMut<'_, Manipulators> {
    lua.app_data_mut::<Manipulators>().expect("Manipulators not in app_data")
}

fn id_of(obj: &LuaTable) -> LuaResult<ManipulatorId> {
    Ok(script_object::handle::<ManipulatorHandle>(obj)?.0)
}

fn fx(v: f64) -> Fx {
    crate::engine::sim::math::from_lua(v)
}

/// Resolves a bone argument (name or index) against an entity's skeleton.
pub fn resolve_bone(lua: &Lua, entity: EntityId, bone: &LuaValue) -> LuaResult<Option<u32>> {
    let w = entity::world(lua);
    let skel = w.get(entity).and_then(|e| e.cold.skeleton.clone());
    Ok(match (bone, skel) {
        (LuaValue::String(s), Some(sk)) => sk.find(&s.to_str()?),
        (LuaValue::Integer(i), Some(sk)) if *i >= 0 && (*i as usize) < sk.bones.len() => Some(*i as u32),
        (LuaValue::Number(n), Some(sk)) if *n >= 0.0 && (*n as usize) < sk.bones.len() => Some(*n as u32),
        // A collision beam has no skeleton but two points: 0 its start, 1
        // its far end (FAF puts end-point effects on bone 1).
        (LuaValue::Integer(i @ (0 | 1)), None) if crate::engine::api::beam::end_of(lua, entity).is_some() => Some(*i as u32),
        (LuaValue::Number(n), None) if (*n == 0.0 || *n == 1.0) && crate::engine::api::beam::end_of(lua, entity).is_some() => Some(*n as u32),
        _ => None,
    })
}

/// The engine's strong references to live manipulators' Lua objects, by id.
/// Moho's native manipulators keep their script objects alive; without
/// this, FAF's weak-valued TrashBags (recoil sliders) lose entries whenever
/// the garbage collector runs, which differs from run to run: a slider that
/// left its bag is never destroyed and keeps moving the muzzle.
fn objects(lua: &Lua) -> LuaResult<LuaTable> {
    if let Ok(t) = lua.named_registry_value::<LuaTable>("sim.manipulator_objects") {
        return Ok(t);
    }
    let t = lua.create_table()?;
    lua.set_named_registry_value("sim.manipulator_objects", t.clone())?;
    Ok(t)
}

fn create_object(lua: &Lua, id: ManipulatorId, class: &str) -> LuaResult<LuaTable> {
    let obj = script_object::create(lua, ManipulatorHandle(id), moho::class_table(lua, class)?)?;
    objects(lua)?.raw_set(id, obj.clone())?;
    Ok(obj)
}

/// Lets go of the Lua objects of manipulators the engine dropped.
pub fn release_objects(lua: &Lua, ids: &[ManipulatorId]) -> LuaResult<()> {
    let t = objects(lua)?;
    for &id in ids {
        t.raw_set(id, LuaNil)?;
    }
    Ok(())
}

/// Duration of clip `path`, loaded once per clip (keyframes kept in
/// `sim::pose::Clips` for posing bones); `None` if it won't load.
pub(crate) fn clip_duration(lua: &Lua, path: &str) -> Option<Fx> {
    use crate::engine::sim::pose::{Clips, SimClip};
    let key = path.to_lowercase();
    if let Some(c) = lua.app_data_ref::<Clips>().and_then(|c| c.0.get(&key).cloned()) {
        return c.map(|c| c.duration);
    }
    let bytes = {
        let vfs = lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>()?.clone();
        let vfs = vfs.read().ok()?;
        vfs.read(&key)
    };
    let clip = match bytes.and_then(|b| crate::engine::assets::sca::read_clip(&b)) {
        Ok(c) => Some(std::sync::Arc::new(SimClip::from_clip(&c))),
        Err(e) => {
            tracing::warn!(path, "failed to load animation: {e}");
            None
        }
    };
    if lua.app_data_ref::<Clips>().is_none() {
        lua.set_app_data(Clips::default());
    }
    let duration = clip.as_ref().map(|c| c.duration);
    lua.app_data_mut::<Clips>()?.0.insert(key, clip);
    duration
}

/// Whether `WaitFor` can stop waiting on `obj`: an animator, rotator or
/// slider that has finished, or an economy event fully paid (or removed);
/// other manipulators and other objects count as done. `None` for a destroyed manipulator: its "done" never comes, so
/// the waiting thread sleeps for good (inferred from moho waking `WaitFor`
/// on the manipulator's event; FAF loops like the UEF air factory's
/// `while true do SetGoal ... WaitFor(slider) end` would otherwise spin once
/// the unit's trash destroyed the slider).
fn is_done(lua: &Lua, obj: &LuaValue) -> LuaResult<Option<bool>> {
    let LuaValue::Table(t) = obj else { return Ok(Some(true)) };
    // An economy event (`CreateEconomyEvent`): FAF waits on its drain this
    // way (overcharge, weapon charge-ups, teleport) before removing it.
    if let Ok(ev) = script_object::handle::<crate::engine::api::economy_event::EconomyEventHandle>(t) {
        return Ok(Some(lua.app_data_ref::<crate::engine::sim::economy::EconomyEvents>().is_none_or(|e| e.is_done(ev.0))));
    }
    let Ok(id) = id_of(t) else { return Ok(Some(true)) };
    Ok(match manips(lua).get(id).map(|m| &m.kind) {
        None => None,
        Some(ManipulatorKind::Animator(a)) => Some(a.finished()),
        Some(ManipulatorKind::Rotator(r)) => Some(r.finished()),
        Some(ManipulatorKind::Slider(s)) => Some(s.finished()),
        _ => Some(true),
    })
}

/// Reads a value off the animator behind `this` (0 if it isn't one).
fn animator_value(lua: &Lua, this: &LuaTable, f: impl FnOnce(&Animator) -> Fx) -> LuaResult<f64> {
    let id = id_of(this)?;
    Ok(match manips(lua).get(id).map(|m| &m.kind) {
        Some(ManipulatorKind::Animator(a)) => f(a).to_num::<f64>(),
        _ => 0.0,
    })
}

/// Runs `f` on the animator behind `this` and returns `this` (moho's
/// animator methods chain).
fn with_animator(lua: &Lua, this: Obj<moho::AnimationManipulator>, f: impl FnOnce(&mut Animator)) -> LuaResult<Obj<moho::AnimationManipulator>> {
    let id = id_of(&this)?;
    if let Some(ManipulatorKind::Animator(a)) = manips(lua).get_mut(id).map(|m| &mut m.kind) {
        f(a);
    }
    Ok(this)
}

/// Runs `f` on the slider behind `this` and returns `this` (chains).
fn with_slider(lua: &Lua, this: Obj<moho::SlideManipulator>, f: impl FnOnce(&mut Slider)) -> LuaResult<Obj<moho::SlideManipulator>> {
    let id = id_of(&this)?;
    if let Some(ManipulatorKind::Slider(s)) = manips(lua).get_mut(id).map(|m| &mut m.kind) {
        f(s);
    }
    Ok(this)
}

/// Runs `f` on the rotator behind `this` and returns `this` (chains).
fn with_rotator(lua: &Lua, this: Obj<moho::RotateManipulator>, f: impl FnOnce(&mut crate::engine::sim::manipulator::Rotator)) -> LuaResult<Obj<moho::RotateManipulator>> {
    let id = id_of(&this)?;
    if let Some(ManipulatorKind::Rotator(r)) = manips(lua).get_mut(id).map(|m| &mut m.kind) {
        f(r);
    }
    Ok(this)
}

/// Copies the arc and yaw bone of weapon (`unit`, `index`)'s fire-control
/// aim controller into its weapon state (the weapon tick aims with these).
/// Setup-time only: scans the unit's manipulators.
/// Copies the state of weapon (`unit`, `index`)'s fire-control aim (enabled,
/// disabled, aiming straight) into its weapon state. As aims are enabled or
/// disabled, and with the fire control.
pub(crate) fn sync_aim_activity(lua: &Lua, unit: EntityId, index: u32) {
    use crate::engine::sim::weapon::{AimActivity, Weapons};
    let Some(mut weapons) = lua.app_data_mut::<Weapons>() else { return };
    let w = weapons.get_mut(unit, index);
    let m = manips(lua);
    let activity = m.of(unit).find_map(|m| match &m.kind {
        ManipulatorKind::Aim(a) if a.weapon_index == index && w.is_fire_control(&a.label) => Some(match (m.enabled, a.active) {
            (true, true) => AimActivity::Active,
            (true, false) if w.aims_straight_on_disable => AimActivity::Straightening,
            _ => AimActivity::Frozen,
        }),
        _ => None,
    });
    w.aim_activity = activity.unwrap_or(AimActivity::Active);
}

pub(crate) fn sync_fire_control(lua: &Lua, unit: EntityId, index: u32) {
    use crate::engine::sim::weapon::{AimArc, Weapons};
    let Some(mut weapons) = lua.app_data_mut::<Weapons>() else { return };
    let w = weapons.get_mut(unit, index);
    let mut m = manips(lua);
    let aim = m.of(unit).find_map(|m| match &m.kind {
        ManipulatorKind::Aim(a) if a.weapon_index == index && w.is_fire_control(&a.label) => Some(a.clone()),
        _ => None,
    });
    let Some(a) = aim else { return };
    // FAF's dual-arm weapons (`TurretDualManipulators`, weapon.lua
    // SetupTurret): a 'Torso' aim that only yaws, over the blueprint's yaw
    // range, and two arm aims that pitch, the right one the fire control,
    // their own yaw a twelfth of the torso's. Ours has one heading: it turns
    // the torso, within the torso's arc, and the arms only pitch.
    let yaw_aim = m.of(unit).find_map(|m| match &m.kind {
        ManipulatorKind::Aim(y) if y.weapon_index == index && y.label != a.label && y.yaw_bone.is_some() && y.pitch_bone.is_none() && y.arc != FiringArc::default() => {
            Some((y.label.clone(), y.yaw_bone, y.arc))
        }
        _ => None,
    });
    let rad = |deg: Fx| deg * Fx::PI / Fx::from_num(180);
    // An arc that was never set (all zero) leaves the weapon turret-less.
    // Moho's headings are positive toward the bone's +x, as the sim's, and
    // so are `SetFiringArc`'s limits: FAF's Fatboy turns its LeftTurret02
    // (`TurretYaw` -30, range 110) to -140 degrees (probe_turretreach).
    if a.arc != FiringArc::default() {
        let yaw = yaw_aim.as_ref().map_or(a.arc, |(_, _, arc)| *arc);
        w.arc = Some(AimArc {
            min_heading: rad(yaw.min_heading),
            max_heading: rad(yaw.max_heading),
            heading_slew: rad(yaw.heading_slew),
            min_pitch: rad(a.arc.min_pitch),
            max_pitch: rad(a.arc.max_pitch),
            pitch_slew: rad(a.arc.pitch_slew),
        });
    }
    match &yaw_aim {
        Some((_, bone, _)) => w.aim_bone = *bone,
        None if a.yaw_bone.is_some() => w.aim_bone = a.yaw_bone,
        None => {}
    }
    w.muzzle_bone = a.muzzle_bone;
    w.aim_muzzle = None;
    w.pitch_bone = a.pitch_bone;
    w.model_pitch_rest = entity::world(lua).get(unit).and_then(|e| e.cold.skeleton.clone()).map_or(FxQuat::IDENTITY, |sk| crate::engine::sim::pose::rest_pitch_frame(&sk, w.aim_bone, a.pitch_bone));
    w.pitch_rest = w.model_pitch_rest;
    w.rest_pitch = if w.pitch_rest == FxQuat::IDENTITY { Fx::ZERO } else { crate::engine::sim::pose::rest_elevation(w.pitch_rest) };
    w.aim_frame = None;
    drop(weapons);
    let ids: Vec<ManipulatorId> = m.of(unit).filter(|m| matches!(&m.kind, ManipulatorKind::Aim(x) if x.weapon_index == index)).map(|m| m.id).collect();
    for id in ids {
        if let Some(ManipulatorKind::Aim(x)) = m.get_mut(id).map(|m| &mut m.kind) {
            x.under_yaw_aim = yaw_aim.as_ref().is_some_and(|(label, ..)| *label != x.label);
        }
    }
}

fn with_aim<R: Default>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Aim) -> R) -> LuaResult<R> {
    let id = id_of(this)?;
    Ok(match manips(lua).get_mut(id).map(|m| &mut m.kind) {
        Some(ManipulatorKind::Aim(a)) => f(a),
        _ => R::default(),
    })
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Manipulators::default());

    lua_fn!(lua, global,
        /// An aim controller for `weapon`: turns the `yaw` / `pitch` bones
        /// (names or indices) to the weapon's target; `label` names the
        /// fire-control group and `muzzle` the bone shots leave from.
            CreateAimController = |lua,
             (weapon, label, yaw, pitch, muzzle): (
                Obj<moho::Weapon>,
                String,
                Option<shapes::Bone>,
                Option<shapes::Bone>,
                Option<shapes::Bone>,
            )| {
                let w = script_object::handle::<WeaponHandle>(&weapon)?;
                let bone = |b: &Option<shapes::Bone>| match b {
                    Some(b) => resolve_bone(lua, w.unit, b),
                    None => Ok(None),
                };
                let aim = Aim {
                    weapon_index: w.index,
                    label,
                    yaw_bone: bone(&yaw)?,
                    pitch_bone: bone(&pitch)?,
                    muzzle_bone: bone(&muzzle)?,
                    arc: FiringArc::default(),
                    reset_pose_time: Fx::ZERO,
                    heading: Fx::ZERO,
                    pitch: Fx::ZERO,
                    heading_offset: Fx::ZERO,
                    under_yaw_aim: false,
                    active: true,
                    straight_on_disable: lua.app_data_ref::<crate::engine::sim::weapon::Weapons>().and_then(|ws| ws.get(w.unit, w.index).map(|s| s.aims_straight_on_disable)).unwrap_or(false),
                };
                let first = aim.label.clone();
                let id = manips(lua).add(w.unit, ManipulatorKind::Aim(aim));
                if let Some(mut ws) = lua.app_data_mut::<crate::engine::sim::weapon::Weapons>() {
                    let st = ws.get_mut(w.unit, w.index);
                    st.first_aim.get_or_insert(first);
                }
                sync_fire_control(lua, w.unit, w.index);
                create_object(lua, id, "AimManipulator").map(Obj::<moho::AimManipulator>::new)
            },
    )?;

    lua_fn!(lua, global,
        /// A builder arm on `unit`: yaw and pitch bones, and the bone that points at the build site.
        CreateBuilderArmController = |lua, (unit, yaw, pitch, aim): (Obj<moho::Unit>, shapes::Bone, shapes::Bone, shapes::Bone)| {
            let owner = entity::entity_id(&unit)?;
            let arm = BuilderArm {
                yaw_bone: resolve_bone(lua, owner, &yaw)?,
                pitch_bone: resolve_bone(lua, owner, &pitch)?,
                aim_bone: resolve_bone(lua, owner, &aim)?,
                arc: FiringArc::default(),
                heading: Fx::ZERO,
                pitch: Fx::ZERO,
                active: false,
            };
            let id = manips(lua).add(owner, ManipulatorKind::BuilderArm(arm));
            create_object(lua, id, "BuilderArmManipulator").map(Obj::<moho::BuilderArmManipulator>::new)
        })?;

    lua_fn!(lua, global,
        /// A slider on `bone` of `obj`, starting towards the goal (`gx`, `gy`, `gz`) at `speed` units per second.
        /// `world`: the goal is in world units, not the bone's.
        CreateSlider = |lua,
             (obj, bone, gx, gy, gz, speed, world): (Obj<moho::Entity>, shapes::Bone, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<bool>)| {
                let owner = entity::entity_id(&obj)?;
                let slider = Slider {
                    bone: resolve_bone(lua, owner, &bone)?,
                    goal: FxVec3::from_f64(gx.unwrap_or(0.0), gy.unwrap_or(0.0), gz.unwrap_or(0.0)),
                    current: FxVec3::default(),
                    speed: fx(speed.unwrap_or(0.0)),
                    acceleration: Fx::ZERO,
                    deceleration: Fx::ZERO,
                    world_units: world.unwrap_or(false),
                };
                let id = manips(lua).add(owner, ManipulatorKind::Slider(slider));
                create_object(lua, id, "SlideManipulator").map(Obj::<moho::SlideManipulator>::new)
            },
    )?;

    lua_fn!(lua, global,
        /// A rotator turning `bone` of `obj` about `axis` (`"x"`, `"y"` or `"z"`); angles in degrees.
        /// `goal` nil spins freely at `goal_speed`.
        CreateRotator = |lua, (obj, bone, axis, goal, speed, accel, goal_speed): (Obj<moho::Entity>, shapes::Bone, String, Option<f64>, Option<f64>, Option<f64>, Option<f64>)| {
                let owner = entity::entity_id(&obj)?;
                let r = crate::engine::sim::manipulator::Rotator {
                    bone: resolve_bone(lua, owner, &bone)?,
                    axis,
                    angle: Fx::ZERO,
                    goal: goal.map(fx),
                    speed: fx(speed.unwrap_or(0.0)),
                    accel: fx(accel.unwrap_or(0.0)),
                    target_speed: fx(goal_speed.unwrap_or(0.0)),
                    spin: Fx::ZERO,
                };
                let id = manips(lua).add(owner, ManipulatorKind::Rotator(r));
                create_object(lua, id, "RotateManipulator").map(Obj::<moho::RotateManipulator>::new)
            },
    )?;

    lua_fn!(lua, global,
        /// A thrust controller on `bone` of `unit`.
        CreateThrustController = |lua, (unit, label, bone): (Obj<moho::Unit>, Option<String>, shapes::Bone)| {
            let owner = entity::entity_id(&unit)?;
            let thrust = crate::engine::sim::manipulator::Thrust { label: label.unwrap_or_default(), bone: resolve_bone(lua, owner, &bone)?, ..Default::default() };
            let id = manips(lua).add(owner, ManipulatorKind::Thrust(thrust));
            create_object(lua, id, "ThrustManipulator").map(Obj::<moho::ThrustManipulator>::new)
        })?;
    lua_fn!(lua, global,
        /// Makes bone `dest` of `obj` follow bone `src`.
        CreateSlaver = |lua, (obj, dest, src): (Obj<moho::Entity>, shapes::Bone, shapes::Bone)| {
            let owner = entity::entity_id(&obj)?;
            let kind = ManipulatorKind::Slave { dest: resolve_bone(lua, owner, &dest)?, src: resolve_bone(lua, owner, &src)? };
            let id = manips(lua).add(owner, kind);
            create_object(lua, id, "SlaveManipulator").map(Obj::<moho::SlaveManipulator>::new)
        })?;
    lua_fn!(lua, global,
        /// A storage manipulator on `bone` of `obj` for `resource`, moving it between the two points.
        CreateStorageManip = |lua, (obj, bone, resource, x0, y0, z0, x1, y1, z1): (Obj<moho::Entity>, shapes::Bone, Option<String>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>)| {
                let owner = entity::entity_id(&obj)?;
                let n = |v: Option<f64>| v.unwrap_or(0.0);
                let storage = crate::engine::sim::manipulator::Storage {
                    bone: resolve_bone(lua, owner, &bone)?,
                    resource: resource.unwrap_or_default().to_uppercase(),
                    min: FxVec3::from_f64(n(x0), n(y0), n(z0)),
                    max: FxVec3::from_f64(n(x1), n(y1), n(z1)),
                };
                let id = manips(lua).add(owner, ManipulatorKind::Storage(storage));
                create_object(lua, id, "StorageManipulator").map(Obj::<moho::StorageManipulator>::new)
            },
    )?;

    lua_fn!(lua, global,
        /// A collision detector on `unit`: watched bones reaching the ground call `OnAnimCollision`.
        CreateCollisionDetector = |lua, unit: Obj<moho::Unit>| {
            let owner = entity::entity_id(&unit)?;
            let id = manips(lua).add(owner, ManipulatorKind::CollisionDetector(CollisionDetector::default()));
            create_object(lua, id, "CollisionManipulator").map(Obj::<moho::CollisionManipulator>::new)
        })?;

    lua_fn!(lua, global,
        /// A foot-plant (leg IK) controller on `unit`'s foot, knee and hip bones.
        CreateFootPlantController = |lua,
             (unit, foot, knee, hip, straight, max_fall): (
                Obj<moho::Unit>,
                shapes::Bone,
                shapes::Bone,
                shapes::Bone,
                Option<bool>,
                Option<f64>,
            )| {
                let owner = entity::entity_id(&unit)?;
                let fp = FootPlant {
                    foot_bone: resolve_bone(lua, owner, &foot)?,
                    knee_bone: resolve_bone(lua, owner, &knee)?,
                    hip_bone: resolve_bone(lua, owner, &hip)?,
                    straight_legs: straight.unwrap_or(false),
                    max_foot_fall: fx(max_fall.unwrap_or(0.0)),
                };
                let id = manips(lua).add(owner, ManipulatorKind::FootPlant(fp));
                create_object(lua, id, "FootPlantManipulator").map(Obj::<moho::FootPlantManipulator>::new)
            },
    )?;

    lua_fn!(lua, global,
        /// An animation manipulator on `obj`; `align` matches the clip's rate to the entity's speed.
        CreateAnimator = |lua, (obj, align): (Obj<moho::Entity>, Option<bool>)| {
            let owner = entity::entity_id(&obj)?;
            let anim = Animator {
                clip: None,
                looping: false,
                rate: Fx::ONE,
                time: Fx::ZERO,
                duration: None,
                align_with_speed: align.unwrap_or(false),
                last_orientation: None,
                turned: false,
                disabled_bones: Default::default(),
            };
            let id = manips(lua).add(owner, ManipulatorKind::Animator(anim));
            create_object(lua, id, "AnimationManipulator").map(Obj::<moho::AnimationManipulator>::new)
        })?;

    // Starts `clip` from the beginning (rate is kept).
    lua_method!(lua, moho::AnimationManipulator,
        /// Starts `clip` from the beginning (rate is kept). Returns the manipulator.
        PlayAnim = |lua, (this, clip, looping): (Obj<moho::AnimationManipulator>, String, Option<bool>)| {
        let duration = clip_duration(lua, &clip);
        with_animator(lua, this, |a| {
            // Lowercased: the key `sim::pose::Clips` holds it under.
            a.clip = Some(clip.to_lowercase());
            a.looping = looping.unwrap_or(false);
            a.time = Fx::ZERO;
            a.duration = duration;
        })
    })?;

    lua_method!(lua, moho::AnimationManipulator,
        /// Playback rate multiplier (1 = the clip's own speed). Returns the manipulator.
        SetRate = |lua, (this, rate): (Obj<moho::AnimationManipulator>, f64)| {
        with_animator(lua, this, |a| a.rate = crate::engine::sim::math::from_lua(rate))
    })?;
    lua_method!(lua, moho::AnimationManipulator,
        /// Seeks to `t` seconds into the clip. Returns the manipulator.
        SetAnimationTime = |lua, (this, t): (Obj<moho::AnimationManipulator>, f64)| {
        with_animator(lua, this, |a| a.seek(crate::engine::sim::math::from_lua(t)))
    })?;
    lua_method!(lua, moho::AnimationManipulator,
        /// Seeks to a fraction (0 to 1) of the clip. Returns the manipulator.
        SetAnimationFraction = |lua, (this, f): (Obj<moho::AnimationManipulator>, f64)| {
        with_animator(lua, this, |a| a.seek(a.length().saturating_mul(crate::engine::sim::math::from_lua(f))))
    })?;
    lua_method!(lua, moho::AnimationManipulator,
        /// The clip's length in seconds (0 if none is playing).
        GetAnimationDuration = |lua, this: Obj<moho::AnimationManipulator>| {
        animator_value(lua, &this, |a| a.length())
    })?;
    lua_method!(lua, moho::AnimationManipulator,
        /// Seconds into the clip (0 if none is playing).
        GetAnimationTime = |lua, this: Obj<moho::AnimationManipulator>| {
        animator_value(lua, &this, |a| a.time)
    })?;
    lua_method!(lua, moho::AnimationManipulator,
        /// Fraction (0 to 1) of the clip played (0 if none is playing).
        GetAnimationFraction = |lua, this: Obj<moho::AnimationManipulator>| {
        animator_value(lua, &this, |a| if a.length() > Fx::ZERO { a.time.saturating_div(a.length()) } else { Fx::ZERO })
    })?;
    lua_method!(lua, moho::AnimationManipulator,
        /// The playback rate multiplier (0 if not an animator).
        GetRate = |lua, this: Obj<moho::AnimationManipulator>| {
        animator_value(lua, &this, |a| a.rate)
    })?;
    // `WaitFor(obj)` yields a tick at a time until `obj` is done. A native
    // function can't yield, so the loop is Luau around a native check.
    let is_done = lua.create_function(|lua, obj: LuaValue| is_done(lua, &obj))?;
    let wait_for: LuaFunction = lua
        .load(
            "local done, yield, suspend = ... return function(obj) \
             while true do local d = done(obj) if d == nil then suspend() return end if d then return end yield(1) end end",
        )
        .set_name("=[engine WaitFor]")
        .call((is_done, lua.globals().get::<LuaTable>("coroutine")?.get::<LuaFunction>("yield")?, lua.globals().get::<LuaFunction>("SuspendCurrentThread")?))?;
    lua.globals().set("WaitFor", wait_for)?;
    crate::lua_api::script::record_one(
        lua,
        crate::lua_api::registry::Scope::Global,
        "WaitFor",
        "Suspends the thread until `obj` (a manipulator, an economy event, a sound) is done.",
    );

    // moho.manipulator_methods (base of every manipulator)
    lua_method!(lua, moho::Manipulator,
        /// Evaluation order against the unit's other manipulators.
        SetPrecedence = |lua, (this, p): (Obj<moho::Manipulator>, i32)| {
        let id = id_of(&this)?;
        manips(lua).set_precedence(id, p);
        Ok(())
    })?;
    lua_method!(lua, moho::Manipulator,
        /// Removes the manipulator.
        Destroy = |lua, this: Obj<moho::Manipulator>| {
        if let Ok(id) = id_of(&this) {
            manips(lua).remove(id);
            release_objects(lua, &[id])?;
        }
        Ok(())
    })?;

    fn set_enabled(lua: &Lua, this: &LuaTable, on: bool) -> LuaResult<()> {
        let id = id_of(this)?;
        let aim = manips(lua).get_mut(id).and_then(|m| {
            m.enabled = on;
            match &m.kind {
                ManipulatorKind::Aim(a) => Some((m.owner, a.weapon_index)),
                _ => None,
            }
        });
        if let Some((owner, index)) = aim {
            sync_aim_activity(lua, owner, index);
        }
        Ok(())
    }
    lua_method!(lua, moho::Manipulator,
        /// Lets the manipulator act.
        Enable = |lua, this: Obj<moho::Manipulator>| set_enabled(lua, &this, true))?;
    lua_method!(lua, moho::Manipulator,
        /// Stops the manipulator acting, keeping it.
        Disable = |lua, this: Obj<moho::Manipulator>| set_enabled(lua, &this, false))?;

    // Directional walk clips (the Monkeylord): accepted, not used yet.
    lua_method!(lua, moho::AnimationManipulator,
        /// Directional walk clips (the Monkeylord): accepted, not used yet.
        SetDirectionalAnim = |_, (_this, _on): (Obj<moho::AnimationManipulator>, Option<bool>)| Ok(()))?;

    // moho.CollisionManipulator
    lua_method!(lua, moho::CollisionManipulator,
        /// Reports the bone (name or index) touching the ground via `OnAnimCollision`.
        WatchBone = |lua, (this, bone): (Obj<moho::CollisionManipulator>, shapes::Bone)| {
        let id = id_of(&this)?;
        let owner = manips(lua).get(id).map(|m| m.owner);
        let Some(owner) = owner else { return Ok(()) };
        let Some(b) = resolve_bone(lua, owner, &bone)? else { return Ok(()) };
        if let Some(ManipulatorKind::CollisionDetector(c)) = manips(lua).get_mut(id).map(|m| &mut m.kind)
            && !c.watched.contains(&b) {
            c.watched.push(b);
        }
        Ok(())
    })?;

    // Crashing aircraft: report the watched bones hitting the terrain
    // (`OnAnimTerrainCollision`) instead of footfalls.
    lua_method!(lua, moho::CollisionManipulator,
        /// Crashing aircraft: report the watched bones hitting the terrain (`OnAnimTerrainCollision`) instead of footfalls (default true).
        /// Moho requires the flag (MohoEngine.dll 0x10221860; FAF's annotation omits it, its scripts pass `true`).
        EnableTerrainCheck = |lua, (this, on): (Obj<moho::CollisionManipulator>, Option<bool>)| {
        let id = id_of(&this)?;
        if let Some(ManipulatorKind::CollisionDetector(c)) = manips(lua).get_mut(id).map(|m| &mut m.kind) {
            c.terrain_check = on.unwrap_or(true);
        }
        Ok(())
    })?;

    // moho.BuilderArmManipulator
    lua_method!(lua, moho::BuilderArmManipulator,
        /// Degrees and degrees per second (heading min/max/slew, then pitch); kept in radians.
        SetAimingArc = |lua, (this, min_h, max_h, h_slew, min_p, max_p, p_slew): (Obj<moho::BuilderArmManipulator>, f64, f64, f64, f64, f64, f64)| {
            let id = id_of(&this)?;
            // Degrees and degrees per second; kept in radians.
            let rad = |d: f64| fx(d) * Fx::PI / Fx::from_num(180);
            if let Some(ManipulatorKind::BuilderArm(a)) = manips(lua).get_mut(id).map(|m| &mut m.kind) {
                a.arc = FiringArc {
                    min_heading: rad(min_h),
                    max_heading: rad(max_h),
                    heading_slew: rad(h_slew),
                    min_pitch: rad(min_p),
                    max_pitch: rad(max_p),
                    pitch_slew: rad(p_slew),
                };
            }
            Ok(())
        },
    )?;

    // Presentation: which bones the animation drives (default with children).
    lua_method!(lua, moho::AnimationManipulator,
        /// Presentation: whether the animation drives `bone` (and its children, default true).
        SetBoneEnabled = |lua, (this, bone, on, children): (Obj<moho::AnimationManipulator>, shapes::Bone, bool, Option<bool>)| {
            let id = id_of(&this)?;
            let owner = manips(lua).get(id).map(|m| m.owner);
            let Some(owner) = owner else { return Ok(this) };
            let Some(b) = resolve_bone(lua, owner, &bone)? else { return Ok(this) };
            let bones = entity::world(lua).get(owner).map(|e| e.bone_set(b, children.unwrap_or(true))).unwrap_or_default();
            with_animator(lua, this, |a| {
                for b in bones {
                    if on {
                        a.disabled_bones.remove(&b);
                    } else {
                        a.disabled_bones.insert(b);
                    }
                }
            })
        },
    )?;
    // moho.SlideManipulator (motion: `Slider::step`).
    lua_method!(lua, moho::ThrustManipulator,
        /// The thrust controller's eight tuning parameters (the last two default 1 and 0.3).
        SetThrustingParam = |lua, (this, a, b, c, d, e, f, g, h): (Obj<moho::ThrustManipulator>, f64, f64, f64, f64, f64, f64, Option<f64>, Option<f64>)| {
            let id = id_of(&this)?;
            if let Some(ManipulatorKind::Thrust(t)) = manips(lua).get_mut(id).map(|m| &mut m.kind) {
                t.params = [a, b, c, d, e, f, g.unwrap_or(1.0), h.unwrap_or(0.3)].map(fx);
            }
            Ok(this)
        },
    )?;
    lua_method!(lua, moho::SlideManipulator,
        /// The slide goal (x, y, z). Returns the manipulator.
        SetGoal = |lua, (this, x, y, z): (Obj<moho::SlideManipulator>, f64, f64, f64)| {
        with_slider(lua, this, |s| s.goal = FxVec3::from_f64(x, y, z))
    })?;
    // Units per second; negative = jump to the goal immediately (FAF recoil).
    lua_method!(lua, moho::SlideManipulator,
        /// Units per second; negative = jump to the goal immediately (FAF recoil).
        SetSpeed = |lua, (this, speed): (Obj<moho::SlideManipulator>, f64)| {
        with_slider(lua, this, |s| s.speed = fx(speed))
    })?;
    lua_method!(lua, moho::SlideManipulator,
        /// Units per second squared.
        SetAcceleration = |lua, (this, a): (Obj<moho::SlideManipulator>, f64)| {
        with_slider(lua, this, |s| s.acceleration = fx(a))
    })?;
    lua_method!(lua, moho::SlideManipulator,
        /// Units per second squared.
        SetDeceleration = |lua, (this, d): (Obj<moho::SlideManipulator>, f64)| {
        with_slider(lua, this, |s| s.deceleration = fx(d))
    })?;
    lua_method!(lua, moho::SlideManipulator,
        /// Whether the goal is in world units rather than the bone's.
        SetWorldUnits = |lua, (this, on): (Obj<moho::SlideManipulator>, bool)| {
        with_slider(lua, this, |s| s.world_units = on)
    })?;

    lua_method!(lua, moho::BuilderArmManipulator,
        /// The arm's heading and pitch, radians, as an aim controller's (heading positive to the right).
        GetHeadingPitch = |lua, this: Obj<moho::BuilderArmManipulator>| {
        let id = id_of(&this)?;
        Ok(match manips(lua).get(id).map(|m| &m.kind) {
            // The aim controllers' convention (heading positive toward +x,
            // as moho's stores it: real-game probe `probe_aim`), so
            // CommandUnit's hand-over between the two keeps the arm still.
            Some(ManipulatorKind::BuilderArm(a)) => (a.heading.to_num::<f64>(), a.pitch.to_num::<f64>()),
            _ => (0.0, 0.0),
        })
    })?;
    // Radians, the aim controllers' convention (CommandUnit hands the arm
    // between its gun's aim and building with these).
    lua_method!(lua, moho::BuilderArmManipulator,
        /// Radians, the aim controllers' convention (CommandUnit hands the arm between its gun's aim and building with these).
        SetHeadingPitch = |lua, (this, h, p): (Obj<moho::BuilderArmManipulator>, f64, f64)| {
        let id = id_of(&this)?;
        if let Some(ManipulatorKind::BuilderArm(a)) = manips(lua).get_mut(id).map(|m| &mut m.kind) {
            a.heading = fx(h);
            a.pitch = fx(p);
        }
        Ok(())
    })?;
    // Moho's are the turret's live heading and pitch (CAimManipulator +0xb8
    // / +0xbc, MohoEngine.dll 0x1021c36f / 0x1021c504), which its weapon
    // drives, from the yaw bone's frame and the pitch bone's pose (a tank on
    // a slope reads its barrel's pitch off the tilted hull: real-game probe
    // `probe_aim`, -8.2 degrees with the muzzle 13.4 up on a 21.6 degree
    // roll), the heading positive to the right (toward +x). FAF hands an
    // ACU's arm between its gun and its build arm with them (CommandUnit.lua).
    lua_method!(lua, moho::AimManipulator,
        /// The aim's current heading and pitch, radians (heading positive to the right).
        GetHeadingPitch = |lua, this: Obj<moho::AimManipulator>| {
        let id = id_of(&this)?;
        let Some((owner, index, script)) = manips(lua).get(id).and_then(|m| match &m.kind {
            ManipulatorKind::Aim(a) => Some((m.owner, a.weapon_index, (a.heading.to_num::<f64>(), a.pitch.to_num::<f64>()))),
            _ => None,
        }) else {
            return Ok((0.0, 0.0));
        };
        let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
        Ok(match weapons.as_deref().and_then(|ws| ws.get(owner, index)) {
            Some(w) => (w.aim_heading.to_num::<f64>(), w.aim_pitch.to_num::<f64>()),
            None => script,
        })
    })?;
    lua_method!(lua, moho::AimManipulator,
        /// Sets the aim's heading and pitch, radians (heading positive to the right).
        SetHeadingPitch = |lua, (this, h, p): (Obj<moho::AimManipulator>, f64, f64)| {
        let id = id_of(&this)?;
        let aim = manips(lua).get_mut(id).and_then(|m| match &mut m.kind {
            ManipulatorKind::Aim(a) => {
                a.heading = fx(h);
                a.pitch = fx(p);
                Some((m.owner, a.weapon_index))
            }
            _ => None,
        });
        // A weapon's aim turns on from there.
        if let (Some((owner, index)), Some(mut weapons)) = (aim, lua.app_data_mut::<crate::engine::sim::weapon::Weapons>())
            && let Some(w) = weapons.find_mut(owner, index) {
            w.aim_heading = fx(h);
            w.aim_pitch = fx(p);
        }
        Ok(())
    })?;
    // moho.RotateManipulator (angles in degrees)
    lua_method!(lua, moho::RotateManipulator,
        /// The goal angle in degrees.
        SetGoal = |lua, (this, g): (Obj<moho::RotateManipulator>, f64)| with_rotator(lua, this, |r| r.goal = Some(fx(g))))?;
    lua_method!(lua, moho::RotateManipulator,
        /// Spins freely at the target speed.
        ClearGoal = |lua, this: Obj<moho::RotateManipulator>| with_rotator(lua, this, |r| r.goal = None))?;
    lua_method!(lua, moho::RotateManipulator,
        /// Degrees per second.
        SetSpeed = |lua, (this, v): (Obj<moho::RotateManipulator>, f64)| with_rotator(lua, this, |r| r.speed = fx(v)))?;
    lua_method!(lua, moho::RotateManipulator,
        /// Degrees per second squared.
        SetAccel = |lua, (this, v): (Obj<moho::RotateManipulator>, f64)| with_rotator(lua, this, |r| r.accel = fx(v)))?;
    lua_method!(lua, moho::RotateManipulator,
        /// The speed (degrees per second) the rotator accelerates to when it has no goal.
        SetTargetSpeed = |lua, (this, v): (Obj<moho::RotateManipulator>, f64)| with_rotator(lua, this, |r| r.target_speed = fx(v)))?;
    lua_method!(lua, moho::RotateManipulator,
        /// Jumps to the angle (degrees).
        SetCurrentAngle = |lua, (this, v): (Obj<moho::RotateManipulator>, f64)| with_rotator(lua, this, |r| r.angle = fx(v)))?;
    lua_method!(lua, moho::RotateManipulator,
        /// `true`: decelerate to a stop.
        SetSpinDown = |lua, (this, on): (Obj<moho::RotateManipulator>, bool)| {
        with_rotator(lua, this, |r| {
            if on {
                r.target_speed = Fx::ZERO;
            }
        })
    })?;
    lua_method!(lua, moho::RotateManipulator,
        /// The current angle in degrees.
        GetCurrentAngle = |lua, this: Obj<moho::RotateManipulator>| {
        let id = id_of(&this)?;
        Ok(match manips(lua).get(id).map(|m| &m.kind) {
            Some(ManipulatorKind::Rotator(r)) => r.angle.to_num::<f64>(),
            _ => 0.0,
        })
    })?;
    for name in ["SetFollowBone", "ClearFollowBone"] {
        crate::lua_api::add_class_fn(
            lua,
            moho::RotateManipulator::CLASS,
            name,
            None,
            "",
            Status::Stub,
            |_, (this, _b): (Obj<moho::RotateManipulator>, LuaValue)| Ok(this),
        )?;
    }
    // moho.AimManipulator
    lua_method!(lua, moho::AimManipulator,
        /// Enables or disables the aim controller: disabled, its bones show their rest pose and its weapon's aim and on-target state stay as they were (or, `AimsStraightOnDisable`, it turns back to straight ahead). Apart from the manipulator's `Enable` / `Disable`.
        SetEnabled = |lua, (this, on): (Obj<moho::AimManipulator>, bool)| {
        let weapon = with_aim(lua, &this, |a| {
            a.active = on;
            Some(a.weapon_index)
        })?;
        let owner = manips(lua).get(id_of(&this)?).map(|m| m.owner);
        if let (Some(index), Some(owner)) = (weapon, owner) {
            sync_aim_activity(lua, owner, index);
        }
        Ok(())
    })?;
    lua_method!(lua, moho::AimManipulator,
        /// Seconds the weapon takes to return to its rest pose; the weapon uses the latest set on any of its aims.
        SetResetPoseTime = |lua, (this, t): (Obj<moho::AimManipulator>, f64)| {
        with_aim(lua, &this, |a| a.reset_pose_time = fx(t))?;
        // The weapon's aims share one heading: its reset time is the
        // latest set on any of them (seconds x 10 ticks, as moho stores it).
        let id = id_of(&this)?;
        let weapon = manips(lua).get(id).and_then(|m| match &m.kind {
            ManipulatorKind::Aim(a) => Some((m.owner, a.weapon_index)),
            _ => None,
        });
        if let (Some((unit, index)), Some(mut weapons)) = (weapon, lua.app_data_mut::<crate::engine::sim::weapon::Weapons>()) {
            let ticks = fx(t).saturating_mul(Fx::from_num(10)).round().max(Fx::ZERO).to_num::<i64>().clamp(0, u32::MAX as i64) as u32;
            weapons.get_mut(unit, index).reset_pose_ticks = (ticks > 0).then_some(ticks);
        }
        Ok(())
    })?;
    lua_method!(lua, moho::AimManipulator,
        /// The aim's arc: heading min/max/slew and pitch min/max/slew (degrees, degrees per second).
        SetFiringArc = |lua, (this, min_h, max_h, h_slew, min_p, max_p, p_slew): (Obj<moho::AimManipulator>, f64, f64, f64, f64, f64, f64)| {
            let weapon = with_aim(lua, &this, |a| {
                a.arc = FiringArc {
                    min_heading: fx(min_h),
                    max_heading: fx(max_h),
                    heading_slew: fx(h_slew),
                    min_pitch: fx(min_p),
                    max_pitch: fx(max_p),
                    pitch_slew: fx(p_slew),
                };
                Some(a.weapon_index)
            })?;
            let owner = manips(lua).get(id_of(&this)?).map(|m| m.owner);
            if let (Some(index), Some(owner)) = (weapon, owner) {
                sync_fire_control(lua, owner, index);
            }
            Ok(())
        },
    )?;
    Ok(())
}

/// Builder arms turn toward what their unit builds, repairs, reclaims or
/// captures (its focus entity, at its collision centre), within their arcs;
/// back to rest without one. After construction (focus is current).
/// A watched bone touches the ground at most this high above it (world
/// units; a planted ACU foot bone reads 0.00-0.01), and must have lifted
/// past [`GROUND_LIFT`] since its last touch (the ACU's swings to ~0.33).
pub const GROUND_CONTACT: Fx = Fx::from_bits(214_748_365); // 0.05
pub const GROUND_LIFT: Fx = Fx::from_bits(644_245_094); // 0.15

/// Collision detectors, once a tick: a watched bone coming down to the
/// ground calls `OnAnimCollision(bone, x, y, z)` on its unit (footfalls:
/// dust, footprints, experimental stomp damage), or
/// `OnAnimTerrainCollision` when the detector checks terrain (aircraft
/// crashing after a shield bounce). `bone` is the bone's name (FAF matches
/// it against the blueprint's `FootBone`) and x, y, z the contact in the
/// unit's frame (FAF plays the left foot's sound for x > 0).
pub fn collision_detectors_tick(lua: &Lua) -> LuaResult<()> {
    let detectors: Vec<(ManipulatorId, EntityId, Vec<u32>, bool)> = match lua.app_data_ref::<Manipulators>() {
        Some(m) => m
            .iter()
            .filter(|m| m.enabled)
            .filter_map(|m| match &m.kind {
                ManipulatorKind::CollisionDetector(c) if !c.watched.is_empty() => Some((m.id, m.owner, c.watched.clone(), c.terrain_check)),
                _ => None,
            })
            .collect(),
        None => return Ok(()),
    };
    if detectors.is_empty() {
        return Ok(());
    }
    // (unit, bone name, contact point in the unit's frame, terrain check).
    let mut hits: Vec<(EntityId, String, FxVec3, bool)> = Vec::new();
    for (id, owner, watched, terrain_check) in detectors {
        // (bone name, position in the unit's frame, height above the
        // ground) per watched bone.
        let contacts: Vec<(String, FxVec3, Fx)> = {
            let w = entity::world(lua);
            let Some(e) = w.get(owner).filter(|e| !e.destroyed) else { continue };
            let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
            let to_local = e.orientation.conjugate();
            watched
                .iter()
                .map(|&b| {
                    let (p, _) = entity::posed_bone(lua, e, Some(b));
                    let ground = terrain.as_deref().map_or(Fx::ZERO, |t| t.height_at(p.x, p.z));
                    let name = e.cold.skeleton.as_ref().and_then(|s| s.bones.get(b as usize)).map(|b| b.name.clone()).unwrap_or_default();
                    (name, to_local.rotate(p - e.position()), p.y - ground)
                })
                .collect()
        };
        let mut m = manips(lua);
        let Some(ManipulatorKind::CollisionDetector(c)) = m.get_mut(id).map(|m| &mut m.kind) else { continue };
        c.grounded.resize(watched.len(), true);
        for (i, (name, local, height)) in contacts.iter().enumerate() {
            let height = *height;
            if height <= GROUND_CONTACT && !c.grounded[i] {
                hits.push((owner, name.clone(), *local, terrain_check));
                c.grounded[i] = true;
            } else if height > GROUND_LIFT {
                c.grounded[i] = false;
            }
        }
    }
    for (owner, bone, p, terrain_check) in hits {
        if let Some(obj) = entity::object(lua, owner)? {
            let (x, y, z) = p.to_f64();
            if terrain_check {
                UnitCallbacks::on_anim_terrain_collision(lua, &obj, "unit", bone, x, y, z)?;
            } else {
                UnitCallbacks::on_anim_collision(lua, &obj, "unit", bone, x, y, z)?;
            }
        }
    }
    Ok(())
}

pub fn builder_arms_tick(lua: &Lua) {
    let (Some(mut m), Some(world)) = (lua.app_data_mut::<Manipulators>(), lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>()) else { return };
    for man in m.iter_mut() {
        let ManipulatorKind::BuilderArm(arm) = &mut man.kind else { continue };
        let Some(e) = world.get(man.owner).filter(|e| !e.destroyed) else { continue };
        let target = e.unit.as_deref().and_then(|u| u.focus).and_then(|f| world.get(f)).filter(|t| !t.destroyed).map(crate::engine::sim::weapon::aim_point);
        arm.aim(e.bone_world_position(arm.yaw_bone), e.bone_world_rotation(arm.yaw_bone), target);
    }
}

