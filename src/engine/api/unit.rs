//! Unit spawning and `moho.unit_methods`.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::army::resolve_army;
use crate::engine::api::blueprints::BlueprintKind;
use crate::engine::api::entity::{blueprint_table, create_object, entity_id, world};
use crate::engine::api::callbacks::{AIBrainCallbacks, UnitCallbacks};
use crate::engine::api::moho;
use crate::lua_api::{lua_fn, lua_method, MohoClass, Obj, Status};
use crate::engine::sim::army::Armies;
use crate::engine::sim::entity::EntityKind;
use crate::engine::sim::math::{Fx, FxQuat, FxVec3};
use crate::engine::sim::terrain::Terrain;

/// Runs `f` on the unit data behind script object `this` (no-op if gone).
pub(crate) fn with_unit<R: Default>(
    lua: &Lua,
    this: &LuaTable,
    f: impl FnOnce(&mut crate::engine::sim::entity::UnitData) -> R,
) -> LuaResult<R> {
    let id = entity_id(this)?;
    Ok(world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()).map(f).unwrap_or_default())
}

/// Lua module + class for a unit blueprint: explicit `ScriptModule`/
/// `ScriptClass`, else `<Source minus _unit.bp>_script.lua` and its `TypeClass`.
fn script_of(bp: &LuaTable, id: &str) -> LuaResult<(String, String)> {
    if let (Some(m), Some(c)) = (
        bp.get::<Option<String>>("ScriptModule")?,
        bp.get::<Option<String>>("ScriptClass")?,
    ) {
        return Ok((m, c));
    }
    let source: String = bp
        .get::<Option<String>>("Source")?
        .unwrap_or_else(|| format!("/units/{id}/{id}_unit.bp"));
    let module = match source.to_lowercase().strip_suffix("_unit.bp") {
        Some(stem) => format!("{stem}_script.lua"),
        None => return Err(LuaError::runtime(format!("unit blueprint '{id}' has unexpected Source '{source}'"))),
    };
    Ok((module, "TypeClass".into()))
}

/// The build skirt around a unit blueprint's footprint, relative to its
/// position (x0, z0, x1, z1): `Physics.SkirtSize/Offset` about
/// `Footprint.SizeX/Z` (else `SizeX/Z`), halved. The pad a builder's build
/// range reaches (FAF measures to the target's skirt, so the range need
/// only touch it).
pub(crate) fn skirt_of_bp(bp: &LuaTable) -> LuaResult<(Fx, Fx, Fx, Fx)> {
    let fp = bp.get::<Option<LuaTable>>("Footprint")?;
    let half = |k: &str| -> LuaResult<Fx> {
        let v = match &fp {
            Some(t) => t.get::<Option<f64>>(k)?,
            None => None,
        };
        Ok(crate::engine::sim::math::from_lua(v.or(bp.get::<Option<f64>>(k)?).unwrap_or(1.0) / 2.0))
    };
    let (hx, hz) = (half("SizeX")?, half("SizeZ")?);
    let physics = bp.get::<Option<LuaTable>>("Physics")?;
    let get = |k: &str, d: Fx| -> LuaResult<Fx> {
        Ok(match &physics {
            Some(p) => p.get::<Option<f64>>(k)?.map(crate::engine::sim::math::from_lua).unwrap_or(d),
            None => d,
        })
    };
    let (x0, z0) = (get("SkirtOffsetX", Fx::ZERO)? - hx, get("SkirtOffsetZ", Fx::ZERO)? - hz);
    Ok((x0, z0, x0 + get("SkirtSizeX", hx * 2)?, z0 + get("SkirtSizeZ", hz * 2)?))
}

/// A unit blueprint's `Air` table: the fields the motion model reads. Unset
/// fields keep `AirParams`' defaults (moho's `RUnitBlueprintAir`).
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "UnitBlueprintAir", rename_all = "PascalCase")]
struct AirBlueprint {
    max_airspeed: Option<Fx>,
    /// Radians per second (the engine works in degrees).
    turn_speed: Option<f64>,
    /// 0: three times the unit's size.
    start_turn_distance: Option<Fx>,
    circling_turn_mult: Option<Fx>,
    #[lua(rename = "CirclingRadiusChangeMinRatio")]
    circling_min_ratio: Option<Fx>,
    #[lua(rename = "CirclingRadiusChangeMaxRatio")]
    circling_max_ratio: Option<Fx>,
    #[lua(rename = "CirclingRadiusVsAirMult")]
    circling_vs_air_mult: Option<Fx>,
    #[lua(rename = "CirclingElevationChangeRatio")]
    circling_elevation_ratio: Option<Fx>,
    circling_dir_change: Option<bool>,
    #[lua(rename = "CirclingFlightChangeFrequency")]
    circling_frequency: Option<Fx>,
    hover_over_attack: Option<bool>,
    bank_factor: Option<Fx>,
    auto_land_time: Option<Fx>,
    transport_hover_height: Option<Fx>,
    #[lua(rename = "SustainedTurnThreshold")]
    sustained_turn: Option<Fx>,
    #[lua(rename = "RandomBreakOffDistanceMult")]
    random_break_mult: Option<Fx>,
    k_move: Option<Fx>,
    k_move_damping: Option<Fx>,
    winged: Option<bool>,
    min_airspeed: Option<Fx>,
    lift_factor: Option<Fx>,
    break_off_trigger: Option<Fx>,
    break_off_distance: Option<Fx>,
    predict_ahead_for_bomb_drop: Option<Fx>,
    combat_turn_speed: Option<Fx>,
    break_off_if_near_new_target: Option<bool>,
    random_min_change_combat_state_time: Option<Fx>,
    random_max_change_combat_state_time: Option<Fx>,
    engage_distance: Option<Fx>,
    k_lift: Option<Fx>,
    k_lift_damping: Option<Fx>,
    k_turn: Option<Fx>,
    k_turn_damping: Option<Fx>,
    k_roll: Option<Fx>,
    k_roll_damping: Option<Fx>,
    tight_turn_multiplier: Option<Fx>,
    bank_forward: Option<bool>,
}

/// Motion limits from blueprint `Physics` (`Air.MaxAirspeed` for air units),
/// and the build range (`Economy.MaxBuildDistance`, default 5).
fn motion_of(bp: &LuaTable, motion_type: &str) -> LuaResult<(crate::engine::sim::movement::MotionParams, Fx)> {
    use crate::engine::sim::movement::{AirParams, MotionKind, MotionParams};
    let fx = |v: Option<f64>| crate::engine::sim::math::from_lua(v.unwrap_or(0.0));
    let kind = MotionKind::parse(motion_type);
    let physics = bp.get::<Option<LuaTable>>("Physics")?;
    let get = |k: &str| -> LuaResult<Option<f64>> {
        match &physics {
            Some(p) => p.get::<Option<f64>>(k),
            None => Ok(None),
        }
    };
    let mut max_speed = fx(get("MaxSpeed")?);
    let mut turn = get("TurnRate")?.unwrap_or(90.0);
    let (mut winged, mut min_speed, mut lift, mut break_trigger, mut break_distance) = (false, Fx::ZERO, Fx::ZERO, Fx::ZERO, Fx::ZERO);
    let mut air_params = AirParams::default();
    if kind == MotionKind::Air {
        // moho's default TurnSpeed is 1 rad/s (`RUnitBlueprintAir` ctor).
        turn = 57.29577951308232;
        if let Some(air) = bp.get::<Option<AirBlueprint>>("Air")? {
            max_speed = air.max_airspeed.unwrap_or_else(|| fx(Some(max_speed.to_num())));
            turn = air.turn_speed.map_or(turn, |t| t * 57.29577951308232);
            let d = AirParams::default();
            // StartTurnDistance 0: three times the unit's `SizeZ`
            // (`RUnitBlueprintAir::ComputeDerivedQuantities` 0x1010da90).
            let size = fx(bp.get::<Option<f64>>("SizeZ")?);
            let start_turn = air.start_turn_distance.unwrap_or(Fx::ZERO);
            air_params = AirParams {
                circling_turn_mult: air.circling_turn_mult.unwrap_or(d.circling_turn_mult),
                circling_min_ratio: air.circling_min_ratio.unwrap_or(d.circling_min_ratio),
                circling_max_ratio: air.circling_max_ratio.unwrap_or(d.circling_max_ratio),
                circling_vs_air_mult: air.circling_vs_air_mult.unwrap_or(d.circling_vs_air_mult),
                circling_elevation_ratio: air.circling_elevation_ratio.unwrap_or(d.circling_elevation_ratio),
                circling_dir_change: air.circling_dir_change.unwrap_or(d.circling_dir_change),
                circling_frequency: air.circling_frequency.unwrap_or(d.circling_frequency),
                hover_over_attack: air.hover_over_attack.unwrap_or(d.hover_over_attack),
                bank_factor: air.bank_factor.unwrap_or(d.bank_factor),
                start_turn_distance: if start_turn > Fx::ZERO { start_turn } else { size * 3 },
                auto_land_time: air.auto_land_time.unwrap_or(d.auto_land_time),
                transport_hover_height: air.transport_hover_height.unwrap_or(d.transport_hover_height),
                sustained_turn: air.sustained_turn.unwrap_or(d.sustained_turn),
                random_break_mult: air.random_break_mult.unwrap_or(d.random_break_mult),
                k_move: air.k_move.unwrap_or(d.k_move),
                k_move_damping: air.k_move_damping.unwrap_or(d.k_move_damping),
                predict_ahead_for_bomb_drop: air.predict_ahead_for_bomb_drop.unwrap_or(d.predict_ahead_for_bomb_drop),
                combat_turn_speed: air.combat_turn_speed.unwrap_or(d.combat_turn_speed),
                break_off_if_near: air.break_off_if_near_new_target.unwrap_or(d.break_off_if_near),
                combat_state_time_min: air.random_min_change_combat_state_time.unwrap_or(d.combat_state_time_min),
                combat_state_time_max: air.random_max_change_combat_state_time.unwrap_or(d.combat_state_time_max),
                engage_distance: air.engage_distance.unwrap_or(d.engage_distance),
                k_lift: air.k_lift.unwrap_or(d.k_lift),
                k_lift_damping: air.k_lift_damping.unwrap_or(d.k_lift_damping),
                k_turn: air.k_turn.unwrap_or(d.k_turn),
                k_turn_damping: air.k_turn_damping.unwrap_or(d.k_turn_damping),
                k_roll: air.k_roll.unwrap_or(d.k_roll),
                k_roll_damping: air.k_roll_damping.unwrap_or(d.k_roll_damping),
                tight_turn_multiplier: air.tight_turn_multiplier.unwrap_or(d.tight_turn_multiplier),
                bank_forward: air.bank_forward.unwrap_or(d.bank_forward),
                ..d
            };
            // Inertia per unit mass: `InertiaTensorX/Y/Z`, or a solid box
            // of the unit's size when any is 0 (`SPhysBody` 0x10279090).
            let top = |k: &str| -> LuaResult<Fx> { Ok(fx(bp.get::<Option<f64>>(k)?)) };
            let given = crate::engine::sim::math::FxVec3 { x: top("InertiaTensorX")?, y: top("InertiaTensorY")?, z: top("InertiaTensorZ")? };
            let inertia = if given.x * given.y * given.z != Fx::ZERO { given } else { crate::engine::sim::movement::box_inertia(crate::engine::sim::math::FxVec3 { x: top("SizeX")?, y: top("SizeY")?, z: top("SizeZ")? }) };
            // (A degenerate axis gets a tiny inertia rather than a division by 0.)
            let floor = Fx::from_num(0.001);
            let inertia = crate::engine::sim::math::FxVec3 { x: inertia.x.max(floor), y: inertia.y.max(floor), z: inertia.z.max(floor) };
            air_params.inertia = inertia;
            air_params.inverse_inertia = crate::engine::sim::math::FxVec3 { x: Fx::ONE / inertia.x, y: Fx::ONE / inertia.y, z: Fx::ONE / inertia.z };
            winged = air.winged.unwrap_or(false);
            min_speed = air.min_airspeed.unwrap_or(Fx::ZERO).min(max_speed);
            lift = air.lift_factor.unwrap_or(Fx::ZERO);
            break_trigger = air.break_off_trigger.unwrap_or(Fx::ZERO);
            break_distance = air.break_off_distance.unwrap_or(Fx::ZERO);
        }
        // Ground collision (`HandleGroundCollision` 0x1029ae30): skipped
        // higher than 2 x max(SizeX, SizeZ).
        air_params.ground_collision_offset = fx(get("GroundCollisionOffset")?);
        let size = |k: &str| -> LuaResult<Fx> { Ok(fx(bp.get::<Option<f64>>(k)?)) };
        air_params.ground_collision_reach = size("SizeX")?.max(size("SizeZ")?) * 2;
        // Its landing spot: the footprint's cells; transports may land on
        // water (MohoEngine.dll 0x10215586: `TRANSPORTATION`).
        if let Some(fp) = bp.get::<Option<LuaTable>>("Footprint")? {
            let cells = |k: &str| -> LuaResult<u8> { Ok(fp.get::<Option<f64>>(k)?.unwrap_or(1.0).clamp(1.0, 255.0) as u8) };
            air_params.land_footprint = (cells("SizeX")?, cells("SizeZ")?);
        }
        air_params.lands_on_water = match bp.get::<Option<LuaTable>>("CategoriesHash")? {
            Some(c) => c.get::<Option<bool>>("TRANSPORTATION")?.unwrap_or(false),
            None => bp.get::<Option<Vec<String>>>("Categories")?.is_some_and(|c| c.iter().any(|c| c == "TRANSPORTATION")),
        };
    }
    let mut accel = fx(get("MaxAcceleration")?);
    // Aircraft blueprints give no MaxAcceleration (their engine model is
    // thrust-based): reach top speed in about a second (inferred).
    if kind == MotionKind::Air && accel <= Fx::ZERO {
        accel = max_speed;
    }
    let brake = get("MaxBrake")?.map_or(accel, crate::engine::sim::math::from_lua);
    let surface = !matches!(kind, MotionKind::Air | MotionKind::None);
    let surface_fx = |k: &str| -> LuaResult<Fx> { Ok(if surface { fx(get(k)?) } else { Fx::ZERO }) };
    let rotate_on_spot = surface
        && match bp.get::<Option<LuaTable>>("Physics")? {
            Some(p) => p.get::<Option<bool>>("RotateOnSpot")?.unwrap_or(false),
            None => false,
        };
    let params = MotionParams {
        kind,
        // `RUnitBlueprintPhysics` constructor (MohoEngine.dll 0x1010db3f):
        // 1 unless set.
        dive_surface_speed: crate::engine::sim::math::from_lua(get("DiveSurfaceSpeed")?.unwrap_or(1.0)),
        surfacing_sub: motion_type == "RULEUMT_SurfacingSub",
        // AI +0x6c (unit blueprint +0x4c0); no FAF unit sets it.
        auto_surface_to_attack: bp.get::<Option<LuaTable>>("AI")?.map(|ai| ai.get::<LuaValue>("AutoSurfaceToAttack")).transpose()?.is_some_and(|v| matches!(v, LuaValue::Boolean(true))),
        max_speed,
        acceleration: accel,
        brake,
        turn_rate: crate::engine::sim::math::from_lua(turn) * Fx::PI / Fx::from_num(180),
        elevation: fx(get("Elevation")?),
        attack_elevation: fx(get("AttackElevation")?),
        winged,
        min_speed,
        lift,
        break_trigger,
        break_distance,
        reverse_speed: surface_fx("MaxSpeedReverse")?,
        backup_distance: surface_fx("BackUpDistance")?,
        steer_force: surface_fx("MaxSteerForce")?,
        rotate_on_spot,
        // Unset: 0.1 rad, the value most blueprints that set it use
        // (inferred). A zero threshold would pivot forever.
        rotate_threshold: if rotate_on_spot { fx(Some(get("RotateOnSpotThreshold")?.unwrap_or(0.1))).max(Fx::from_num(0.01)) } else { Fx::ZERO },
        facing_rate: surface_fx("TurnFacingRate")? * Fx::PI / Fx::from_num(180),
        layer_change_offset: surface_fx("LayerChangeOffsetHeight")?,
        attack_angle: if surface {
            fx(bp.get::<Option<LuaTable>>("AI")?.map(|ai| ai.get::<Option<f64>>("AttackAngle")).transpose()?.flatten()).max(Fx::ZERO) * Fx::PI / Fx::from_num(180)
        } else {
            Fx::ZERO
        },
        // 0 (or unset): no limit (moho's default is +inf).
        turn_radius: surface_fx("TurnRadius")?.max(Fx::ZERO),
        air: air_params,
        ground: crate::engine::sim::movement::GroundParams {
            size_x: fx(bp.get::<Option<f64>>("SizeX")?.or(Some(1.0))),
            size_z: fx(bp.get::<Option<f64>>("SizeZ")?.or(Some(1.0))),
            stand_upright: physics.as_ref().map_or(Ok(None), |p| p.get::<Option<bool>>("StandUpright"))?.unwrap_or(false),
            sink_lower: physics.as_ref().map_or(Ok(None), |p| p.get::<Option<bool>>("SinkLower"))?.unwrap_or(false),
            wobble_factor: fx(get("WobbleFactor")?),
            wobble_speed: fx(get("WobbleSpeed")?),
            banking_slope: fx(get("BankingSlope")?),
        },
    };
    let build = match bp.get::<Option<LuaTable>>("Economy")? {
        Some(e) => e.get::<Option<f64>>("MaxBuildDistance")?.unwrap_or(5.0),
        None => 5.0,
    };
    Ok((params, crate::engine::sim::math::from_lua(build)))
}

/// A unit's AI threat from blueprint `Defense.*ThreatLevel` and its
/// categories (see `sim::threat`).
fn unit_threat(lua: &Lua, bp: &LuaTable, category_index: Option<u32>) -> LuaResult<crate::engine::sim::threat::ThreatLevels> {
    use crate::engine::sim::threat::{unit_threat, ThreatKinds};
    let level = |k: &str| -> LuaResult<Fx> {
        Ok(match bp.get::<Option<LuaTable>>("Defense")? {
            Some(d) => crate::engine::sim::math::from_lua(d.get::<Option<f64>>(k)?.unwrap_or(0.0)),
            None => Fx::ZERO,
        })
    };
    let domain = crate::engine::api::categories::domain(lua)?;
    let is = |name: &str| match (category_index, domain.named.get(name)) {
        (Some(i), Some(set)) => set.contains(i),
        _ => false,
    };
    let kinds = ThreatKinds {
        structure: is("STRUCTURE"),
        mass_extraction: is("MASSEXTRACTION"),
        naval: is("NAVAL"),
        air: is("AIR"),
        land: is("LAND"),
        experimental: is("EXPERIMENTAL"),
        commander: is("COMMAND"),
        artillery: is("ARTILLERY"),
    };
    Ok(unit_threat(level("AirThreatLevel")?, level("SurfaceThreatLevel")?, level("SubThreatLevel")?, level("EconomyThreatLevel")?, kinds))
}

/// A unit's collision box from its blueprint: half-extents `Size*/2`,
/// sitting on `CollisionOffset*` (bottom face at the offset's y). Units with
/// `SizeSphere` get their sphere from FAF's `OnStopBeingBuilt`.
fn blueprint_collision(bp: &LuaTable) -> LuaResult<crate::engine::sim::entity::CollisionShape> {
    let num = |k: &str| -> LuaResult<Fx> { Ok(crate::engine::sim::math::from_lua(bp.get::<Option<f64>>(k)?.unwrap_or(0.0))) };
    let half = FxVec3 { x: num("SizeX")? / 2, y: num("SizeY")? / 2, z: num("SizeZ")? / 2 };
    if half == FxVec3::default() {
        return Ok(crate::engine::sim::entity::CollisionShape::None);
    }
    let center = FxVec3 { x: num("CollisionOffsetX")?, y: num("CollisionOffsetY")? + half.y, z: num("CollisionOffsetZ")? };
    Ok(crate::engine::sim::entity::CollisionShape::Box { center, half })
}

/// Spawns a completed unit and runs moho's creation callbacks:
/// `OnPreCreate`, weapons (+ their `OnCreate`), `OnLayerChange(layer,
/// 'None')`, `OnCreate`, then `OnStopBeingBuilt(nil, layer)`.
pub fn spawn_unit(
    lua: &Lua,
    bp_id: &str,
    army: u32,
    position: FxVec3,
    orientation: FxQuat,
) -> LuaResult<Obj<moho::Unit>> {
    spawn_unit_built_by(lua, bp_id, army, position, orientation, None)
}

/// [`spawn_unit`], or with `builder`, as a fresh construction: fraction 0,
/// health 1, `BeingBuilt` state, and `OnStartBeingBuilt(builder, layer)`
/// instead of `OnStopBeingBuilt` (the build system finishes it).
pub fn spawn_unit_built_by(
    lua: &Lua,
    bp_id: &str,
    army: u32,
    position: FxVec3,
    orientation: FxQuat,
    builder: Option<&LuaTable>,
) -> LuaResult<Obj<moho::Unit>> {
    let _s = crate::prof_span!("create unit", bp = bp_id);
    let bp_id = bp_id.to_lowercase();
    let bp = blueprint_table(lua, BlueprintKind::Unit, &bp_id)?;
    let (module, class) = script_of(&bp, &bp_id)?;
    let skeleton = crate::engine::api::mesh::skeleton_for(lua, &bp)?;
    let render_mesh = crate::engine::api::mesh::render_index_for(lua, &bp)?;
    let target_bones: Vec<Option<u32>> = match bp.get::<Option<LuaTable>>("AI")?.map(|ai| ai.get::<Option<LuaTable>>("TargetBones")).transpose()?.flatten() {
        Some(list) => list.sequence_values::<String>().filter_map(Result::ok).map(|name| skeleton.as_ref().and_then(|s| s.find(&name))).collect(),
        None => Vec::new(),
    };
    let id = world(lua).spawn(EntityKind::Unit, BlueprintKind::Unit, bp_id.clone(), Some(army), position, orientation);
    let intel = crate::engine::api::entity::intel_from_blueprint(&bp)?;
    // Health from the blueprint (moho does this on creation).
    let (max_health, health) = match bp.get::<Option<LuaTable>>("Defense")? {
        Some(d) => {
            let max = d.get::<Option<f64>>("MaxHealth")?.unwrap_or(1.0);
            (max, d.get::<Option<f64>>("Health")?.unwrap_or(max).min(max))
        }
        None => (1.0, 1.0),
    };
    let (storage_mass, storage_energy) = match bp.get::<Option<LuaTable>>("Economy")? {
        Some(econ) => (
            econ.get::<Option<f64>>("StorageMass")?.unwrap_or(0.0),
            econ.get::<Option<f64>>("StorageEnergy")?.unwrap_or(0.0),
        ),
        None => (0.0, 0.0),
    };
    // Capturable unless the blueprint lists the "Not Capturable" ability
    // (the rule FAF's UI uses; inferred for the engine).
    let capturable = match bp.get::<Option<LuaTable>>("Display")?.map(|d| d.get::<Option<LuaTable>>("Abilities")).transpose()?.flatten() {
        Some(abilities) => !abilities.sequence_values::<String>().any(|a| a.is_ok_and(|a| a.contains("ability_notcap"))),
        None => true,
    };
    let category_index = crate::engine::api::categories::domain(lua)?.index_of(&bp_id);
    let motion_type: String = bp
        .get::<Option<LuaTable>>("Physics")?
        .map(|p| p.get::<Option<String>>("MotionType"))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    let (motion_params, build_range) = motion_of(&bp, &motion_type)?;
    let wet = lua
        .app_data_ref::<Terrain>()
        .and_then(|t| t.water_surface.map(|w| crate::engine::sim::layer::wet(motion_params.kind, t.height_at(position.x, position.z), w, motion_params.layer_change_offset)))
        .unwrap_or(false);
    let mut layer = crate::engine::sim::layer::Layer::initial(&motion_type, wet, crate::engine::sim::layer::LayerMask::build_caps(&bp)?);
    // Over water, a unit that can be under it starts there unless it's an
    // experimental (moho's spawn layer, MohoEngine.dll 0x10214c36: `Sub` in
    // its footprint's caps): submarines start submerged.
    let sub_caps = crate::engine::sim::nav::caps_of_motion_type(&motion_type).is_some_and(|c| c & crate::engine::sim::nav::caps::SUB != 0);
    let experimental = match (category_index, crate::engine::api::categories::domain(lua)?.named.get("EXPERIMENTAL")) {
        (Some(i), Some(set)) => set.contains(i),
        _ => false,
    };
    if layer == crate::engine::sim::layer::Layer::Water && sub_caps && !experimental {
        layer = crate::engine::sim::layer::Layer::Sub;
    }
    // An aircraft a script makes starts at its flying height (the surface,
    // water included, + `Elevation`) whatever height it's given: the real
    // game's do (dev/probes probe_air, probe_turns). Factory-built ones
    // start on the pad.
    if builder.is_none() && motion_params.kind == crate::engine::sim::movement::MotionKind::Air && layer == crate::engine::sim::layer::Layer::Air
        && let Some(surface) = lua.app_data_ref::<Terrain>().map(|t| t.surface_at(position.x, position.z)) {
        world(lua).set_position(id, FxVec3 { y: surface + motion_params.elevation, ..position });
    }
    // Likewise a naval unit starts on the water, a submarine its
    // `Elevation` under it, whatever height it's given (moho's
    // `IUnit::CalcSpawnElevation` 0x1012eef0: water, or water + `Elevation`
    // in the Sub layer; the real game's frigate made on the seabed is on
    // the surface at once and its sub 1.5 under, dev/probes probe_water).
    let depth = if layer == crate::engine::sim::layer::Layer::Sub { motion_params.elevation } else { Fx::ZERO };
    if builder.is_none() && motion_params.kind == crate::engine::sim::movement::MotionKind::Water && matches!(layer, crate::engine::sim::layer::Layer::Water | crate::engine::sim::layer::Layer::Sub)
        && let Some(surface) = lua.app_data_ref::<Terrain>().map(|t| t.surface_at(position.x, position.z)) {
        world(lua).set_position(id, FxVec3 { y: surface + depth, ..position });
    }
    let threat = unit_threat(lua, &bp, category_index)?;
    let skirt = skirt_of_bp(&bp)?;
    let caps = |key: &str, names: &[&str]| -> LuaResult<u32> {
        match bp.get::<Option<LuaTable>>("General")? {
            Some(general) => crate::engine::sim::unit_state::bitfield_bits(names, "", &general.get::<LuaValue>(key)?),
            None => Ok(0),
        }
    };
    let command_caps = caps("CommandCaps", &crate::engine::sim::unit_state::COMMAND_CAPS)?;
    let toggle_caps = caps("ToggleCaps", &crate::engine::sim::unit_state::TOGGLE_CAPS)?;
    let build_rate = match bp.get::<Option<LuaTable>>("Economy")? {
        Some(e) => crate::engine::sim::math::from_lua(e.get::<Option<f64>>("BuildRate")?.unwrap_or(0.0)),
        None => Fx::ZERO,
    };
    let jammer = match bp.get::<Option<LuaTable>>("Intel")? {
        Some(intel) => {
            let blips = intel.get::<Option<f64>>("JammerBlips")?.unwrap_or(0.0);
            let (min, max) = match intel.get::<Option<LuaTable>>("JamRadius")? {
                Some(r) => (r.get::<Option<f64>>("Min")?.unwrap_or(0.0), r.get::<Option<f64>>("Max")?.unwrap_or(0.0)),
                None => (0.0, 0.0),
            };
            (blips > 0.0).then(|| crate::engine::sim::vision::JammerSpec {
                blips: blips as u32,
                min: crate::engine::sim::math::from_lua(min),
                max: crate::engine::sim::math::from_lua(max.max(min)),
            })
        }
        None => None,
    };
    // Moho seeds the unit's regen from `Defense.RegenRate` when it makes the
    // unit (MohoEngine.dll 0x102845ed); FAF's buffs only change it later.
    let (armor_type, regen): (Option<String>, f64) = match bp.get::<Option<LuaTable>>("Defense")? {
        Some(d) => (d.get::<Option<String>>("ArmorType")?, d.get::<Option<f64>>("RegenRate")?.unwrap_or(0.0)),
        None => (None, 0.0),
    };
    // Units with `Physics.FuelUseTime` start with a full tank; the others
    // report -1 (FAF checks `fuel > -1`).
    let (fuel_use_time, fuel_recharge) = match bp.get::<Option<LuaTable>>("Physics")? {
        Some(p) => (p.get::<Option<f64>>("FuelUseTime")?.unwrap_or(0.0), p.get::<Option<f64>>("FuelRechargeRate")?.unwrap_or(0.0)),
        None => (0.0, 0.0),
    };
    let transport_class = match bp.get::<Option<LuaTable>>("Transport")? {
        Some(t) => t.get::<Option<f64>>("TransportClass")?.unwrap_or(1.0),
        None => 1.0,
    };
    // Avoidance radius: half the larger footprint side (default 1x1).
    let radius = match bp.get::<Option<LuaTable>>("Footprint")? {
        Some(fp) => fp.get::<Option<f64>>("SizeX")?.unwrap_or(1.0).max(fp.get::<Option<f64>>("SizeZ")?.unwrap_or(1.0)),
        None => 1.0,
    };
    // Push mass (moho: blueprint SizeX x SizeY x SizeZ x AverageDensity).
    let size = |k: &str| -> LuaResult<f64> { Ok(bp.get::<Option<f64>>(k)?.unwrap_or(1.0)) };
    let mass = size("SizeX")? * size("SizeY")? * size("SizeZ")? * bp.get::<Option<f64>>("AverageDensity")?.unwrap_or(1.0);
    // Auto mode and engineering stations' operational radius (`AI`).
    let (initial_auto, assist_radius) = match bp.get::<Option<LuaTable>>("AI")? {
        Some(ai) => {
            let guard = ai.get::<Option<f64>>("GuardScanRadius")?.unwrap_or(0.0);
            let staging = ai.get::<Option<f64>>("StagingPlatformScanRadius")?.unwrap_or(0.0);
            (ai.get::<Option<bool>>("InitialAutoMode")?.unwrap_or(false), if guard > 0.0 { guard } else { staging })
        }
        None => (false, 0.0),
    };
    // Air staging platforms' service to docked aircraft (`AI.Refueling*`,
    // `AI.RepairConsume*`; moho's defaults).
    let staging = if bp.get::<Option<LuaTable>>("CategoriesHash")?.map(|c| c.get::<Option<bool>>("AIRSTAGINGPLATFORM")).transpose()?.flatten().unwrap_or(false) {
        let ai = bp.get::<Option<LuaTable>>("AI")?;
        let field = |k: &str, default: f64| -> LuaResult<Fx> {
            let v = match &ai {
                Some(ai) => ai.get::<Option<f64>>(k)?,
                None => None,
            };
            Ok(crate::engine::sim::math::from_lua(v.unwrap_or(default)))
        };
        Some(Box::new(crate::engine::sim::entity::StagingService {
            refuel_multiplier: field("RefuelingMultiplier", 1.0)?,
            repair_amount: field("RefuelingRepairAmount", 20.0)?,
            repair_energy: field("RepairConsumeEnergy", 2.0)?,
            repair_mass: field("RepairConsumeMass", 0.5)?,
            carrier: bp.get::<Option<LuaTable>>("CategoriesHash")?.map(|c| c.get::<Option<bool>>("CARRIER")).transpose()?.flatten().unwrap_or(false),
        }))
    } else {
        None
    };
    let face_to_build = match bp.get::<Option<LuaTable>>("Economy")? {
        Some(e) => e.get::<Option<bool>>("NeedToFaceTargetToBuild")?.unwrap_or(false),
        None => false,
    };
    // Chase and leash radii (moho's `RUnitBlueprintAI` defaults, 0x1010e210).
    let (guard_scan, guard_return) = match bp.get::<Option<LuaTable>>("AI")? {
        Some(ai) => (ai.get::<Option<f64>>("GuardScanRadius")?.unwrap_or(25.0), ai.get::<Option<f64>>("GuardReturnRadius")?.unwrap_or(50.0)),
        None => (25.0, 50.0),
    };
    let engineering_station = bp.get::<Option<LuaTable>>("CategoriesHash")?.map(|c| c.get::<Option<bool>>("ENGINEERSTATION")).transpose()?.flatten().unwrap_or(false);
    let cap_cost = match bp.get::<Option<LuaTable>>("General")? {
        Some(g) => g.get::<Option<f64>>("CapCost")?.unwrap_or(1.0),
        None => 1.0,
    };
    if let Some(e) = world(lua).get_mut(id) {
        e.category_index = category_index;
        if let Some(u) = e.unit.as_mut() {
            u.storage_mass = crate::engine::sim::math::from_lua(storage_mass);
            u.storage_energy = crate::engine::sim::math::from_lua(storage_energy);
            u.resource_consumed = Fx::ONE;
            u.layer = layer;
            u.threat = threat;
            u.cap_cost = crate::engine::sim::math::from_lua(cap_cost);
            u.motion_params = motion_params;
            u.depth = depth;
            u.build_range = build_range;
            u.build_skirt = skirt;
            u.build_rate = build_rate;
            u.command_caps = command_caps;
            u.capturable = capturable;
            u.radius = crate::engine::sim::math::from_lua(radius) / 2;
            u.mass = crate::engine::sim::math::from_lua(mass.min(1.0e9));
            u.fuel_use_time = crate::engine::sim::math::from_lua(fuel_use_time);
            // Lowercase: armour names match without regard to case (`api::armor`).
            u.armor_type = armor_type.map(|a| std::sync::Arc::from(a.to_ascii_lowercase()));
            u.fuel_recharge = crate::engine::sim::math::from_lua(fuel_recharge);
            u.transport_class = transport_class.clamp(1.0, 3.0) as u8;
            u.jammer = jammer;
            u.fuel_ratio = if fuel_use_time > 0.0 { Fx::ONE } else { -Fx::ONE };
            u.blueprint_command_caps = command_caps;
            u.toggle_caps = toggle_caps;
            u.blueprint_toggle_caps = toggle_caps;
            u.regen = crate::engine::sim::math::from_lua(regen);
            u.blueprint_regen = u.regen;
            u.auto_mode = initial_auto;
            u.engineering_station = engineering_station;
            u.assist_radius = crate::engine::sim::math::from_lua(assist_radius);
            u.staging = staging;
            u.guard_scan_radius = crate::engine::sim::math::from_lua(guard_scan);
            u.face_to_build = face_to_build;
            u.guard_return_radius = crate::engine::sim::math::from_lua(guard_return);
            u.target_bones = target_bones;
        }
        e.cold.skeleton = skeleton;
        e.cold.render_mesh = render_mesh;
        e.cold.intel = intel;
        e.cold.scale = crate::engine::api::entity::blueprint_scale(&bp)?;
        e.cold.draw_scale = FxVec3 { x: e.cold.scale, y: e.cold.scale, z: e.cold.scale };
        e.max_health = crate::engine::sim::math::from_lua(max_health);
        e.health = crate::engine::sim::math::from_lua(health);
        if builder.is_some() {
            // FAF detects a fresh construction by `GetHealth() == 1`.
            e.fraction_complete = Fx::ZERO;
            e.health = Fx::ONE.min(e.max_health);
            if let Some(u) = e.unit.as_mut() {
                let being_built = crate::engine::sim::unit_state::UnitStates::index("BeingBuilt").expect("state");
                u.states.set(being_built, true);
            }
        }
    }
    if builder.is_some() {
        crate::engine::api::construction::mark_unfinished(lua, id);
    }
    world(lua).set_base_collision(id, blueprint_collision(&bp)?);
    // Structures (finished or not) block their footprint for pathing.
    let is_structure = match (crate::engine::api::categories::domain(lua)?.named.get("STRUCTURE"), category_index) {
        (Some(set), Some(i)) => set.contains(i),
        _ => false,
    };
    if is_structure {
        let fp = bp.get::<Option<LuaTable>>("Footprint")?;
        let size = |k: &str| -> LuaResult<Fx> {
            let v = match &fp {
                Some(t) => t.get::<Option<f64>>(k)?,
                None => None,
            };
            Ok(crate::engine::sim::math::from_lua(v.unwrap_or(1.0)) / 2)
        };
        let (hx, hz) = (size("SizeX")?, size("SizeZ")?);
        let rect = (position.x - hx, position.z - hz, position.x + hx, position.z + hz);
        // `Physics.OccupyRects`: (centre x, centre z, half x, half z) per
        // rect, unrotated, each edge rounded to the nearest cell line; they
        // replace the footprint (MohoEngine.dll `Unit::ExecuteOccupyGround`
        // 0x10286f50).
        let occupy: Vec<(Fx, Fx, Fx, Fx)> = match bp.get::<Option<LuaTable>>("Physics")?.map(|p| p.get::<Option<Vec<f64>>>("OccupyRects")).transpose()?.flatten() {
            Some(v) => {
                let cell = |v: Fx| (v + Fx::from_num(0.5)).floor();
                let v: Vec<Fx> = v.into_iter().map(crate::engine::sim::math::from_lua).collect();
                let (px, pz) = (position.x, position.z);
                v.chunks_exact(4).map(|r| (cell(px + r[0] - r[2]), cell(pz + r[1] - r[3]), cell(px + r[0] + r[2]), cell(pz + r[1] + r[3]))).collect()
            }
            None => Vec::new(),
        };
        if let Some(mut grid) = lua.app_data_mut::<crate::engine::sim::nav::NavGrid>() {
            if occupy.is_empty() {
                grid.block_rect(rect.0, rect.1, rect.2, rect.3, true);
            }
            for r in &occupy {
                grid.block_rect(r.0, r.1, r.2, r.3, true);
            }
        }
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.nav_block = Some(rect);
            u.nav_occupy = occupy;
        }
    }
    // Its pathing footprint class: by motion type and blueprint footprint.
    if let Some(caps) = crate::engine::sim::nav::caps_of_motion_type(&motion_type) {
        let class = footprint_class(lua, &bp, caps, false)?;
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.nav_class = class;
        }
    }
    // Every new unit starts in its army's pool platoon (moho).
    crate::engine::api::platoon::add_to_pool(lua, army, id);
    // Built / being-built stats, as moho's constructor counts them.
    crate::engine::api::unit_stats::unit_created(lua, id)?;
    // A factory's starting rally, as moho's constructor gives it (before
    // the scripts run).
    crate::engine::api::construction::initial_rally(lua, id)?;
    let obj = create_object(lua, id, &module, &class)?;
    UnitCallbacks::on_pre_create(lua, &obj, &bp_id)?;
    crate::engine::api::weapon::create_weapons(lua, &obj, id, &bp)?;
    // Before `OnCreate` (FAF's `Unit.OnLayerChange` checks for old == 'None'
    // and sets up weapon target layers from it).
    UnitCallbacks::on_layer_change(lua, &obj, &bp_id, layer.as_str(), "None")?;
    UnitCallbacks::on_create(lua, &obj, &bp_id)?;
    // The terrain type it starts on: FAF only learns it from this callback
    // (`Unit.TerrainType`, which its tread marks need), from its default
    // type (`GetTerrainType(-1, -1)`).
    let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map(|t| {
        let p = position;
        (t.type_at(p.x, p.z), t.type_at(-Fx::ONE, -Fx::ONE))
    });
    if let Some((code, default)) = terrain {
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.terrain_type = Some(code);
        }
        let entry = |code| crate::engine::api::terrain::terrain_type_entry(lua, code).map(crate::lua_api::shapes::Loose::new);
        UnitCallbacks::on_terrain_type_change(lua, &obj, &bp_id, entry(code)?, entry(default)?)?;
    }
    match builder {
        Some(b) => UnitCallbacks::on_start_being_built(lua, &obj, &bp_id, Obj::new(b.clone()), layer)?,
        None => {
            UnitCallbacks::on_stop_being_built(lua, &obj, &bp_id, None, layer)?;
            crate::engine::api::adjacency::on_finished(lua, id)?;
        }
    }
    Ok(Obj::new(obj))
}

/// Engine toggle caps in bit order (`/engine/Enums.lua` `EngineToggleCap`).
const TOGGLE_CAPS: [&str; 9] = [
    "RULEUTC_ShieldToggle",
    "RULEUTC_WeaponToggle",
    "RULEUTC_JammingToggle",
    "RULEUTC_IntelToggle",
    "RULEUTC_ProductionToggle",
    "RULEUTC_StealthToggle",
    "RULEUTC_GenericToggle",
    "RULEUTC_SpecialToggle",
    "RULEUTC_CloakToggle",
];

/// Sets or clears a script bit, calling `OnScriptBitSet`/`OnScriptBitClear(bit)`
/// only when it flips; the world borrow is released before the callback.
fn set_script_bit(lua: &Lua, this: &LuaTable, bit: u16, on: bool) -> LuaResult<()> {
    let changed = with_unit(lua, this, |u| {
        let old = u.script_bits;
        if on { u.script_bits |= 1 << bit } else { u.script_bits &= !(1 << bit) }
        old != u.script_bits
    })?;
    if changed {
        if on { UnitCallbacks::on_script_bit_set(lua, this, "unit", bit)? } else { UnitCallbacks::on_script_bit_clear(lua, this, "unit", bit)? }
    }
    Ok(())
}

/// Script bit index from a toggle cap name or a bit number.
fn script_bit(v: &LuaValue) -> LuaResult<u16> {
    let bit = match v {
        LuaValue::String(s) => {
            let s = s.to_str()?;
            TOGGLE_CAPS.iter().position(|c| *c == &*s).map(|i| i as i64)
        }
        LuaValue::Integer(i) => Some(*i as i64),
        LuaValue::Number(n) => Some(*n as i64),
        _ => None,
    };
    match bit {
        Some(b) if (0..TOGGLE_CAPS.len() as i64).contains(&b) => Ok(b as u16),
        _ => Err(LuaError::runtime(format!("invalid script bit {v:?}"))),
    }
}

/// Registers `unit:<name>(on)` toggling unit state `state`.
fn state_setter(lua: &Lua, name: &'static str, state: &'static str) -> LuaResult<()> {
    let index = crate::engine::sim::unit_state::UnitStates::index(state).expect("known unit state");
    crate::lua_api::add_class_fn(lua, moho::Unit::CLASS, name, Some(&[Some("this"), Some("on")]), "", Status::Implemented, move |lua, (this, on): (Obj<moho::Unit>, bool)| {
        with_unit(lua, &this, |u| u.states.set(index, on))
    })
}

/// The footprint class a unit of blueprint `bp` paths with on layers
/// `caps`: the nearest to its `Footprint` (`AltFootprint` with `alt`),
/// which registration already set to a class's size.
fn footprint_class(lua: &Lua, bp: &LuaTable, caps: u8, alt: bool) -> LuaResult<Option<crate::engine::sim::nav::NavClass>> {
    let footprint = bp.get::<Option<LuaTable>>(if alt { "AltFootprint" } else { "Footprint" })?;
    let side = |k: &str| -> LuaResult<u32> { Ok(footprint.as_ref().map(|f| f.get::<Option<f64>>(k)).transpose()?.flatten().unwrap_or(1.0).max(0.0) as u32) };
    let (size_x, size_z) = (side("SizeX")?, side("SizeZ")?);
    Ok(lua.app_data_mut::<crate::engine::sim::nav::NavGrid>().and_then(|mut grid| {
        let class = grid.class_for(caps, size_x, size_z)?;
        grid.ensure(class);
        Some(class)
    }))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// The unit object for an entity id (string or number); nil if there's no
        /// live unit with it.
        GetUnitById = |lua, id: shapes::EntityId| {
            let id = match &*id {
                LuaValue::Integer(i) => *i as i64,
                LuaValue::Number(n) => *n as i64,
                LuaValue::String(s) => s.to_str()?.trim().parse::<i64>().unwrap_or(-1),
                _ => -1,
            };
            let Ok(id) = u32::try_from(id) else { return Ok(None) };
            let live = crate::engine::api::entity::world(lua).get(id).is_some_and(|e| !e.destroyed && e.unit.is_some());
            if live { crate::engine::api::entity::object_as::<moho::Unit>(lua, id) } else { Ok(None) }
        })?;
    lua_fn!(lua, global,
        CreateInitialArmyUnit = |lua, (army, bp): (shapes::Army, String)| {
            let index = resolve_army(lua, &army)?;
            let start = lua
                .app_data_ref::<Armies>()
                .and_then(|a| a.get(index).and_then(|a| a.start))
                .ok_or_else(|| LuaError::runtime(format!("army {index} has no start position")))?;
            let y = lua
                .app_data_ref::<Terrain>()
                .map(|t| t.surface_at(start.x, start.z))
                .unwrap_or_default();
            let pos = FxVec3 { x: start.x, y, z: start.z };
            spawn_unit(lua, &bp, index, pos, FxQuat::IDENTITY)
        })?;

    /// Registers `unit:<name>(value)` writing a fixed-point unit field.
    fn fx_setter(
        lua: &Lua,
        name: &'static str,
        field: fn(&mut crate::engine::sim::entity::UnitData) -> &mut Fx,
    ) -> LuaResult<()> {
        crate::lua_api::add_class_fn(lua, moho::Unit::CLASS, name, Some(&[Some("this"), Some("value")]), "", Status::Implemented, move |lua, (this, v): (Obj<moho::Unit>, f64)| {
            with_unit(lua, &this, |u| *field(u) = crate::engine::sim::math::from_lua(v))
        })
    }
    /// Registers `unit:<name>()` reading a fixed-point unit field.
    fn fx_getter(
        lua: &Lua,
        name: &'static str,
        field: fn(&mut crate::engine::sim::entity::UnitData) -> &mut Fx,
    ) -> LuaResult<()> {
        crate::lua_api::add_class_fn(lua, moho::Unit::CLASS, name, Some(&[Some("this")]), "", Status::Implemented, move |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| field(u).to_num::<f64>()))
    }
    fx_getter(lua, "GetConsumptionPerSecondEnergy", |u| &mut u.consumption_energy)?;
    fx_getter(lua, "GetConsumptionPerSecondMass", |u| &mut u.consumption_mass)?;
    fx_getter(lua, "GetProductionPerSecondEnergy", |u| &mut u.production_energy)?;
    fx_getter(lua, "GetProductionPerSecondMass", |u| &mut u.production_mass)?;
    fx_setter(lua, "SetConsumptionPerSecondEnergy", |u| &mut u.consumption_energy)?;
    fx_setter(lua, "SetConsumptionPerSecondMass", |u| &mut u.consumption_mass)?;
    fx_setter(lua, "SetProductionPerSecondEnergy", |u| &mut u.production_energy)?;
    fx_setter(lua, "SetProductionPerSecondMass", |u| &mut u.production_mass)?;
    fx_setter(lua, "SetSpeedMult", |u| &mut u.motion.speed)?;
    // Height above the surface it moves at (air units, hover).
    fx_setter(lua, "SetElevation", |u| &mut u.motion_params.elevation)?;
    lua_method!(lua, moho::Unit,
        /// Back to the blueprint's `Physics.Elevation`.
        RevertElevation = |lua, this: Obj<moho::Unit>| {
        let id = crate::engine::api::entity::entity_id(&this)?;
        let Some(bp_id) = world(lua).get(id).map(|e| e.cold.blueprint_id.clone()) else { return Ok(()) };
        let bp = crate::engine::api::entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)?;
        let elevation = bp.get::<Option<LuaTable>>("Physics")?.map(|p| p.get::<Option<f64>>("Elevation")).transpose()?.flatten().unwrap_or(0.0);
        with_unit(lua, &this, |u| u.motion_params.elevation = crate::engine::sim::math::from_lua(elevation))
    })?;
    fx_setter(lua, "SetAccMult", |u| &mut u.motion.accel)?;
    fx_setter(lua, "SetTurnMult", |u| &mut u.motion.turn)?;
    lua_method!(lua, moho::Unit,
        /// FAF's shields set their owner's ratio from creation on: the unit then
        /// shows a shield bar under its health bar.
        SetShieldRatio = |lua, (this, ratio): (Obj<moho::Unit>, f64)| {
        with_unit(lua, &this, |u| {
            u.shield_ratio = crate::engine::sim::math::from_lua(ratio.clamp(0.0, 1.0));
            u.has_shield = true;
        })
    })?;
    lua_method!(lua, moho::Unit,
        /// The script hears when production / consumption turns on or off
        /// (`OnProductionActive` / `OnProductionInActive`, brain events).
        SetProductionActive = |lua, (this, on): (Obj<moho::Unit>, bool)| {
        if with_unit(lua, &this, |u| std::mem::replace(&mut u.production_active, on) != on)? {
            if on { UnitCallbacks::on_production_active(lua, &this, "unit")? } else { UnitCallbacks::on_production_in_active(lua, &this, "unit")? }
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Unit,
        /// Calls `OnScriptBitSet`/`OnScriptBitClear(bit)` only when the bit flips;
        /// the world borrow is released before the callback runs.
        SetScriptBit = |lua, (this, bit, on): (Obj<moho::Unit>, crate::lua_api::shapes::ScriptBit, bool)| {
        set_script_bit(lua, &this, script_bit(&bit)?, on)
    })?;
    lua_method!(lua, moho::Unit,
        /// Whether the unit can path to `goal` (a position or an entity's):
        /// the goal is in the unit's connected region for its footprint
        /// class. One result, as moho's binding (MohoEngine.dll 0x102a9840;
        /// FAF's AI also reads a second, always nil). Fliers: true
        /// (inferred; FAF's AI doesn't ask them).
        CanPathTo = |lua, (this, goal): (Obj<moho::Unit>, shapes::OrderTarget)| {
        let id = entity_id(&this)?;
        let to = match crate::engine::api::command::target_of(&goal)? {
            crate::engine::sim::command::CommandTarget::Entity(e) => match world(lua).get(e) {
                Some(e) => e.position(),
                None => return Ok(false),
            },
            crate::engine::sim::command::CommandTarget::Ground(p) => p,
            _ => return Ok(false),
        };
        let (from, class) = {
            let w = world(lua);
            let Some(e) = w.get(id) else { return Ok(false) };
            let Some(u) = e.unit.as_deref() else { return Ok(false) };
            let Some(mut nav) = lua.app_data_mut::<crate::engine::sim::nav::NavGrid>() else { return Ok(false) };
            (e.position(), crate::engine::sim::movement::nav_class(u, &nav).inspect(|&c| nav.ensure(c)))
        };
        let Some(class) = class else { return Ok(true) };
        Ok(lua.app_data_mut::<crate::engine::sim::nav::NavGrid>().is_some_and(|mut nav| nav.reachable(class, from, to)))
    })?;
    lua_method!(lua, moho::Unit,
        /// How many units the unit's queued factory and engineer build orders
        /// make of blueprints in `category` (each order's count; MohoEngine.dll
        /// 0x102a5810 walks the queue for command types 7 and 8).
        GetNumBuildOrders = |lua, (this, category): (Obj<moho::Unit>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
        use crate::engine::sim::command::CommandKind;
        let id = entity_id(&this)?;
        let domain = crate::engine::api::categories::domain(lua)?;
        let Some(commands) = lua.app_data_ref::<crate::engine::sim::command::Commands>() else { return Ok(0) };
        Ok(commands
            .queue(id)
            .filter(|c| matches!(c.kind, CommandKind::BuildFactory | CommandKind::BuildMobile))
            .filter(|c| c.blueprint.as_deref().and_then(|bp| domain.index_of(bp)).is_some_and(|i| category.0.contains(i)))
            .map(|c| c.count)
            .sum::<u32>())
    })?;
    lua_method!(lua, moho::Unit,
        /// Whether the toggle's bit is set (MohoEngine.dll 0x102a5c70).
        GetScriptBit = |lua, (this, bit): (Obj<moho::Unit>, crate::lua_api::shapes::ScriptBit)| {
        let bit = script_bit(&bit)?;
        with_unit(lua, &this, |u| u.script_bits & (1 << bit) != 0)
    })?;
    lua_method!(lua, moho::Unit,
        /// Flips the toggle's bit, as the player's toggle button does: only
        /// when the unit has that toggle cap, and not while it rides in a
        /// transport (`Unit::ToggleScriptBit`, MohoEngine.dll 0x10286cc0).
        ToggleScriptBit = |lua, (this, bit): (Obj<moho::Unit>, crate::lua_api::shapes::ScriptBit)| {
        let bit = script_bit(&bit)?;
        let id = entity_id(&this)?;
        let (on, parent) = {
            let w = world(lua);
            let Some(e) = w.get(id) else { return Ok(()) };
            let Some(u) = e.unit.as_deref() else { return Ok(()) };
            let attached = crate::engine::sim::unit_state::UnitStates::index("Attached").is_some_and(|i| u.states.contains(i));
            if u.toggle_caps & (1 << bit) == 0 {
                return Ok(());
            }
            (u.script_bits & (1 << bit) == 0, if attached { e.attachment.map(|a| a.parent) } else { None })
        };
        if let Some(parent) = parent && crate::engine::api::construction::has_category(lua, parent, "TRANSPORTATION")? {
            return Ok(());
        }
        set_script_bit(lua, &this, bit, on)
    })?;
    lua_method!(lua, moho::Unit, SetFocusEntity = |lua, (this, focus): (Obj<moho::Unit>, Obj<moho::Entity>)| {
        set_focus(lua, entity_id(&this)?, Some(entity_id(&focus)?))
    })?;
    lua_method!(lua, moho::Unit,
        /// FAF clears a shield unit's focus before destroying its shield.
        ClearFocusEntity = |lua, this: Obj<moho::Unit>| set_focus(lua, entity_id(&this)?, None))?;
    lua_method!(lua, moho::Unit,
        /// Stunned for `seconds`: holds still and holds fire (a longer stun
        /// replaces a shorter one).
        SetStunned = |lua, (this, seconds): (Obj<moho::Unit>, f64)| {
        let until = crate::engine::api::sim_time::current_tick(lua) + crate::engine::sim::projectile::seconds_to_ticks(crate::engine::sim::math::from_lua(seconds.max(0.0))) as u64;
        with_unit(lua, &this, |u| u.stunned_until = u.stunned_until.max(until))
    })?;
    lua_method!(lua, moho::Unit, IsStunned = |lua, this: Obj<moho::Unit>| {
        let now = crate::engine::api::sim_time::current_tick(lua);
        with_unit(lua, &this, |u| u.stunned_until > now)
    })?;
    lua_method!(lua, moho::Unit, GetResourceConsumed = |lua, this: Obj<moho::Unit>| {
        with_unit(lua, &this, |u| u.resource_consumed.to_num::<f64>())
    })?;
    state_setter(lua, "SetBusy", "Busy")?;
    state_setter(lua, "SetUnSelectable", "UnSelectable")?;
    state_setter(lua, "SetBlockCommandQueue", "BlockCommandQueue")?;
    state_setter(lua, "SetImmobile", "Immobile")?;
    /// Registers `Test/Add/Remove<kind>Cap(name)` over a cap bit set.
    fn caps(lua: &Lua, names: &'static [&'static str], test: &'static str, add: &'static str, remove: &'static str, field: fn(&mut crate::engine::sim::entity::UnitData) -> &mut u32) -> LuaResult<()> {
        crate::lua_api::add_class_fn(lua, moho::Unit::CLASS, test, Some(&[Some("this"), Some("name")]), "", Status::Implemented, move |lua, (this, name): (Obj<moho::Unit>, String)| {
            let bit = crate::engine::sim::unit_state::cap_bit(names, &name).unwrap_or(0);
            with_unit(lua, &this, |u| bit != 0 && *field(u) & bit != 0)
        })?;
        crate::lua_api::add_class_fn(lua, moho::Unit::CLASS, add, Some(&[Some("this"), Some("name")]), "", Status::Implemented, move |lua, (this, name): (Obj<moho::Unit>, String)| {
            let bit = crate::engine::sim::unit_state::cap_bit(names, &name).unwrap_or(0);
            with_unit(lua, &this, |u| *field(u) |= bit)
        })?;
        crate::lua_api::add_class_fn(lua, moho::Unit::CLASS, remove, Some(&[Some("this"), Some("name")]), "", Status::Implemented, move |lua, (this, name): (Obj<moho::Unit>, String)| {
            let bit = crate::engine::sim::unit_state::cap_bit(names, &name).unwrap_or(0);
            with_unit(lua, &this, |u| *field(u) &= !bit)
        })
    }
    caps(lua, &crate::engine::sim::unit_state::COMMAND_CAPS, "TestCommandCaps", "AddCommandCap", "RemoveCommandCap", |u| &mut u.command_caps)?;
    caps(lua, &crate::engine::sim::unit_state::TOGGLE_CAPS, "TestToggleCaps", "AddToggleCap", "RemoveToggleCap", |u| &mut u.toggle_caps)?;
    lua_method!(lua, moho::Unit, RestoreCommandCaps = |lua, this: Obj<moho::Unit>| {
        with_unit(lua, &this, |u| u.command_caps = u.blueprint_command_caps)
    })?;
    lua_method!(lua, moho::Unit,
        /// Back to the blueprint's toggle caps (MohoEngine.dll 0x102a7290).
        RestoreToggleCaps = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.toggle_caps = u.blueprint_toggle_caps))?;
    lua_method!(lua, moho::Unit, SetUnitState = |lua, (this, state, on): (Obj<moho::Unit>, String, bool)| {
        let index = crate::engine::sim::unit_state::UnitStates::index(&state)
            .ok_or_else(|| LuaError::runtime(format!("SetUnitState: unknown unit state '{state}'")))?;
        with_unit(lua, &this, |u| u.states.set(index, on))
    })?;
    lua_method!(lua, moho::Unit,
        /// Unknown state names read as false (moho doesn't validate either).
        /// `Attacking` is also on while the unit's current order is an
        /// attack (FAF's Fire Beetle enables its kamikaze weapon only then:
        /// the real game's fires on `IssueAttack` and never on a target it
        /// merely sees, dev/probes/maps/probe_auditgaps g5).
        IsUnitState = |lua, (this, state): (Obj<moho::Unit>, String)| {
        let Some(index) = crate::engine::sim::unit_state::UnitStates::index(&state) else { return Ok(false) };
        if with_unit(lua, &this, |u| u.states.contains(index))? {
            return Ok(true);
        }
        static ATTACKING: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| crate::engine::sim::unit_state::UnitStates::index("Attacking").expect("unit state"));
        if index != *ATTACKING {
            return Ok(false);
        }
        let id = crate::engine::api::entity::entity_id(&this)?;
        Ok(crate::engine::api::command::is_attacking(lua, id))
    })?;
    lua_method!(lua, moho::Unit, SetConsumptionActive = |lua, (this, on): (Obj<moho::Unit>, bool)| {
        if with_unit(lua, &this, |u| std::mem::replace(&mut u.consumption_active, on) != on)? {
            if on { UnitCallbacks::on_consumption_active(lua, &this, "unit")? } else { UnitCallbacks::on_consumption_in_active(lua, &this, "unit")? }
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Unit, SetFireState = |lua, (this, state): (Obj<moho::Unit>, i32)| {
        with_unit(lua, &this, |u| u.fire_state = state)
    })?;
    lua_method!(lua, moho::Unit, GetFireState = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.fire_state))?;
    lua_method!(lua, moho::Unit,
        /// Steps return fire -> hold fire -> hold ground -> return fire
        /// (MohoEngine.dll 0x102a6150: `(state + 1) % 3`).
        ToggleFireState = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.fire_state = (u.fire_state + 1).rem_euclid(3)))?;
    lua_method!(lua, moho::Unit,
        /// The fill `SetShieldRatio` last set (MohoEngine.dll 0x102ad950).
        GetShieldRatio = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.shield_ratio.to_num::<f64>()))?;
    lua_method!(lua, moho::Unit,
        /// The strategic icon drawn under the unit's own (the campaign's
        /// objective markers; "" clears it). Moho keeps it on the entity
        /// (`Entity::SetStrategicUnderlay`, MohoEngine.dll 0x102a3c5b).
        #[lua(partial = "stored; not drawn yet")]
        SetStrategicUnderlay = |lua, (this, icon): (Obj<moho::Unit>, String)| with_unit(lua, &this, |u| u.strategic_underlay = icon))?;
    lua_method!(lua, moho::Unit,
        /// FAF binary patch semantics: returns true (and sets nothing) when the stat
        /// does not exist yet, so Lua can create it via GetStat first.
        SetStat = |lua, (this, name, value): (Obj<moho::Unit>, String, Option<f64>)| {
        // nil is accepted (FAF passes optional blueprint values) and stored as 0.
        let value = value.unwrap_or(0.0);
        with_unit(lua, &this, |u| match u.stats.get_mut(&name) {
            Some(v) => {
                *v = value;
                false
            }
            None => true,
        })
    })?;
    /// `GetStat`'s result.
    #[derive(crate::lua_api::LuaTable)]
    #[lua(name = "UnitStat", rename_all = "PascalCase", write_only)]
    #[allow(dead_code)]
    struct UnitStat {
        value: f64,
    }
    lua_method!(lua, moho::Unit,
        /// Returns `{ Value = v }`, creating the stat with `default` if missing.
        GetStat = |lua, (this, name, default): (Obj<moho::Unit>, String, Option<f64>)| {
        let value = with_unit(lua, &this, |u| *u.stats.entry(name).or_insert(default.unwrap_or(0.0)))?;
        let t = lua.create_table_with_capacity(0, 1)?;
        t.raw_set("Value", value)?;
        Ok(shapes::Shaped::<UnitStat>::new(t))
    })?;
    /// Registers `unit:<name>(bool)` writing a unit flag.
    fn bool_setter(
        lua: &Lua,
        name: &'static str,
        field: fn(&mut crate::engine::sim::entity::UnitData) -> &mut bool,
    ) -> LuaResult<()> {
        crate::lua_api::add_class_fn(lua, moho::Unit::CLASS, name, Some(&[Some("this"), Some("value")]), "", Status::Implemented, move |lua, (this, v): (Obj<moho::Unit>, bool)| {
            with_unit(lua, &this, |u| *field(u) = v)
        })
    }
    bool_setter(lua, "SetCapturable", |u| &mut u.capturable)?;
    lua_method!(
        lua,
        moho::Unit,
        AddBuildRestriction = |lua, (this, cat): (Obj<moho::Unit>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            with_unit(lua, &this, |u| {
                u.build_restrictions = Some(match &u.build_restrictions {
                    Some(r) => r.union(&cat.0),
                    None => cat.0.clone(),
                })
            })
        },
    )?;
    lua_method!(
        lua,
        moho::Unit,
        RemoveBuildRestriction = |lua, (this, cat): (Obj<moho::Unit>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            with_unit(lua, &this, |u| {
                if let Some(r) = &u.build_restrictions {
                    u.build_restrictions = Some(r.difference(&cat.0));
                }
            })
        },
    )?;
    lua_method!(lua, moho::Unit,
        /// Back to the blueprint's buildable categories: drops the unit's own
        /// restrictions (army-wide ones stay).
        RestoreBuildRestrictions = |lua, this: Obj<moho::Unit>| {
        with_unit(lua, &this, |u| u.build_restrictions = None)
    })?;
    lua_method!(lua, moho::Unit, SetCustomName = |lua, (this, name): (Obj<moho::Unit>, String)| {
        with_unit(lua, &this, |u| u.custom_name = Some(name))
    })?;
    lua_method!(lua, moho::Unit,
        /// `vector` (x, y, z) from the unit's centre in its own frame, in world
        /// space: the Tempest puts what it built behind itself with it.
        CalculateWorldPositionFromRelative = |lua, (this, v): (Obj<moho::Unit>, shapes::Vector)| {
        let id = crate::engine::api::entity::entity_id(&this)?;
        let get = |i: i64| -> LuaResult<Fx> { Ok(crate::engine::sim::math::from_lua(v.raw_get::<Option<f64>>(i)?.unwrap_or(0.0))) };
        let local = crate::engine::sim::math::FxVec3 { x: get(1)?, y: get(2)?, z: get(3)? };
        let p = {
            let w = world(lua);
            let Some(e) = w.get(id) else { return Ok(None) };
            e.position() + e.orientation.rotate(local)
        };
        crate::engine::api::entity::vector(lua, p).map(Some)
    })?;
    lua_method!(lua, moho::Unit,
        /// Paths with the blueprint's alternative footprint (`Physics.AltMotionType`,
        /// else `AltFootprint.OccupancyCaps`) or, with false, its own again: the
        /// Cybran destroyer's land toggle keeps it on water. Its route is
        /// planned again.
        ForceAltFootprint = |lua, (this, alt): (Obj<moho::Unit>, bool)| {
        let id = crate::engine::api::entity::entity_id(&this)?;
        let bp_id = world(lua).get(id).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
        let bp = crate::engine::api::entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)?;
        let physics = bp.get::<Option<LuaTable>>("Physics")?;
        let motion = |key: &str| -> LuaResult<Option<u8>> {
            Ok(match &physics {
                Some(p) => p.get::<Option<String>>(key)?.and_then(|m| crate::engine::sim::nav::caps_of_motion_type(&m)),
                None => None,
            })
        };
        let caps = if alt {
            match motion("AltMotionType")? {
                Some(c) => Some(c),
                None => bp.get::<Option<LuaTable>>("AltFootprint")?.and_then(|f| f.get::<Option<u8>>("OccupancyCaps").ok().flatten()),
            }
        } else {
            motion("MotionType")?
        };
        let Some(caps) = caps else { return Ok(()) };
        let class = footprint_class(lua, &bp, caps, alt)?;
        with_unit(lua, &this, |u| {
            u.nav_class = class;
            u.path = None;
        })
    })?;
    lua_method!(lua, moho::Unit, SetOverchargePaused = |lua, (this, paused): (Obj<moho::Unit>, bool)| {
        with_unit(lua, &this, |u| u.overcharge_paused = paused)
    })?;
    lua_method!(lua, moho::Unit, IsOverchargePaused = |lua, this: Obj<moho::Unit>| {
        with_unit(lua, &this, |u| u.overcharge_paused)
    })?;
    lua_fn!(lua, global,
        /// `CreateUnitHPR(bp, army, x, y, z, pitch, yaw, roll)`: pitch first,
        /// heading second (MohoEngine.dll's own help text and the binding
        /// 0x102ac8f0, which builds the rotation as yaw, pitch, roll; map
        /// saves keep a unit's facing in `Orientation[2]`).
        CreateUnitHPR = |lua, (bp, army, x, y, z, p, h, r): (String, shapes::Army, f64, f64, f64, f64, f64, f64)| {
            let index = resolve_army(lua, &army)?;
            let fx = crate::engine::sim::math::from_lua;
            let q = FxQuat::from_hpr(fx(h), fx(p), fx(r));
            spawn_unit(lua, &bp, index, FxVec3::from_f64(x, y, z), q)
        })?;
    lua_fn!(lua, global,
        /// `CreateUnit(bp, army, x, y, z, qx, qy, qz, qw, layer)`: the quaternion
        /// version (Cybran build drones). The layer follows from the spot as for
        /// every spawn; the argument is ignored.
        CreateUnit = |lua, (bp, army, x, y, z, qx, qy, qz, qw, _layer): (String, shapes::Army, f64, f64, f64, f64, f64, f64, f64, Option<String>)| {
            let index = resolve_army(lua, &army)?;
            let fx = crate::engine::sim::math::from_lua;
            let q = FxQuat { x: fx(qx), y: fx(qy), z: fx(qz), w: fx(qw) };
            spawn_unit(lua, &bp, index, FxVec3::from_f64(x, y, z), q)
        })?;
    lua_fn!(lua, global,
        /// `ChangeUnitArmy(unit, army, allowCommanders)`: a copy of the unit in
        /// `army` (same blueprint, transform, health, build progress), then the
        /// old one is destroyed. nil for dead units, a move to the same army, or
        /// COMMAND units without `allowCommanders` (FAF's engine patch). Nor
        /// past the receiving army's unit cap: the giving army's brain hears
        /// `OnFailedUnitTransfer` and the unit stays (inferred: which brain, and
        /// that `allowCommanders` doesn't lift the cap; FAF's army merge raises
        /// the cap first).
        ChangeUnitArmy = |lua, (unit, army, allow_commanders): (Obj<moho::Unit>, shapes::Army, Option<bool>)| {
            let id = crate::engine::api::entity::entity_id(&unit)?;
            let to = resolve_army(lua, &army)?;
            let Some((bp, pos, rot, health, max_health, fraction, from)) = world(lua)
                .get(id)
                .filter(|e| !e.destroyed && e.unit.is_some())
                .map(|e| (e.cold.blueprint_id.clone(), e.position(), e.orientation, e.health, e.max_health, e.fraction_complete, e.army))
            else {
                return Ok(None);
            };
            if from == Some(to) {
                return Ok(None);
            }
            if !allow_commanders.unwrap_or(false) {
                let domain = crate::engine::api::categories::domain(lua)?;
                let commander = domain.named.get("COMMAND").zip(domain.index_of(&bp)).is_some_and(|(set, i)| set.contains(i));
                if commander {
                    return Ok(None);
                }
            }
            if over_cap(lua, id, to) {
                if let Some(LuaValue::Table(brain)) = from.map(|a| crate::engine::api::army::brain(lua, a)).transpose()? {
                    AIBrainCallbacks::on_failed_unit_transfer(lua, &brain, "brain")?;
                }
                return Ok(None);
            }
            let new = spawn_unit(lua, &bp, to, pos, rot)?;
            let new_id = crate::engine::api::entity::entity_id(&new)?;
            if let Some(e) = world(lua).get_mut(new_id) {
                e.max_health = max_health;
                e.health = health.min(max_health);
                e.fraction_complete = fraction;
            }
            if fraction < Fx::ONE {
                crate::engine::api::construction::mark_unfinished(lua, new_id);
            }
            if let Some(destroy) = unit.get::<Option<LuaFunction>>("Destroy")? {
                destroy.call::<()>(&*unit)?;
            }
            Ok(Some(new))
        })?;
    lua_fn!(lua, global,
        /// Sum of the army's live units' cap costs.
        GetArmyUnitCostTotal = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            let w = world(lua);
            let total = w
                .iter()
                .filter(|e| e.kind == EntityKind::Unit && !e.destroyed && e.army == Some(index))
                .filter_map(|e| e.unit.as_deref().map(|u| u.cap_cost))
                .fold(Fx::ZERO, |a, b| a.saturating_add(b));
            Ok(total.to_num::<f64>())
        })?;
    lua_method!(lua, moho::Unit, IsBeingBuilt = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        Ok(world(lua).get(id).is_some_and(|e| e.fraction_complete < Fx::ONE))
    })?;
    lua_method!(lua, moho::Unit,
        /// Missile silo ammo (nukes: SML / SMD; tactical: TML / ACU upgrades).
        GetNukeSiloAmmoCount = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.nuke_ammo))?;
    lua_method!(lua, moho::Unit, GetTacticalSiloAmmoCount = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.tactical_ammo))?;
    lua_method!(lua, moho::Unit,
        /// Adds `n` missiles. With `in_blocks` (FA-Binary-Patches#15), `n` sets the progress of the
        /// missile in progress, in blocks of one economy tick; with none in
        /// progress, the next one starts with it (`api::silo`).
        GiveNukeSiloAmmo = |lua, (this, n, in_blocks): (Obj<moho::Unit>, u32, Option<bool>)| {
        if in_blocks.unwrap_or(false) {
            let unit = crate::engine::api::entity::entity_id(&this)?;
            if !crate::engine::api::silo::set_blocks(lua, unit, n) {
                with_unit(lua, &this, |u| u.silo_blocks = n)?;
            }
            return Ok(());
        }
        with_unit(lua, &this, |u| u.nuke_ammo = u.nuke_ammo.saturating_add(n))
    })?;
    lua_method!(lua, moho::Unit, GiveTacticalSiloAmmo = |lua, (this, n): (Obj<moho::Unit>, u32)| {
        with_unit(lua, &this, |u| u.tactical_ammo = u.tactical_ammo.saturating_add(n))
    })?;
    lua_method!(lua, moho::Unit, RemoveNukeSiloAmmo = |lua, (this, n): (Obj<moho::Unit>, u32)| {
        with_unit(lua, &this, |u| u.nuke_ammo = u.nuke_ammo.saturating_sub(n))
    })?;
    lua_method!(lua, moho::Unit, RemoveTacticalSiloAmmo = |lua, (this, n): (Obj<moho::Unit>, u32)| {
        with_unit(lua, &this, |u| u.tactical_ammo = u.tactical_ammo.saturating_sub(n))
    })?;
    lua_method!(lua, moho::Unit, IsCapturable = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.capturable))?;
    lua_method!(lua, moho::Unit, SetRegenRate = |lua, (this, rate): (Obj<moho::Unit>, f64)| {
        with_unit(lua, &this, |u| u.regen = crate::engine::sim::math::from_lua(rate))
    })?;
    lua_method!(lua, moho::Unit,
        /// Back to the blueprint's `Defense.RegenRate` (MohoEngine.dll 0x102a76c0;
        /// the original game's enhancement scripts call it on removal).
        RevertRegenRate = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.regen = u.blueprint_regen))?;
    lua_method!(lua, moho::Unit,
        /// Per-tick displacement (moho reports velocity per tick: x10 = per second).
        /// A dead aircraft reports its fall (FAF's ShieldCollider follows it).
        GetVelocity = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        if let Some(v) = crate::engine::api::movement::Falling::velocity(lua, id) {
            return Ok(v.to_f64());
        }
        let w = world(lua);
        let Some(e) = w.get(id) else { return Ok((0.0, 0.0, 0.0)) };
        let f = crate::engine::sim::movement::velocity(e) * (Fx::ONE / Fx::from_num(10));
        Ok(f.to_f64())
    })?;
    lua_method!(lua, moho::Unit,
        /// The unit's target: what its attack order is on, else the first of its
        /// weapons (by index) holding a unit target (inferred).
        GetTargetEntity = |lua, this: Obj<moho::Unit>| {
        use crate::engine::sim::command::{CommandKind as K, CommandTarget};
        let id = entity_id(&this)?;
        let ordered = lua.app_data_ref::<crate::engine::sim::command::Commands>().and_then(|c| {
            c.current(id).filter(|c| matches!(c.kind, K::Attack | K::FormAttack)).and_then(|c| match c.target {
                CommandTarget::Entity(t) => Some(t),
                _ => None,
            })
        });
        let target = ordered.or_else(|| {
            lua.app_data_ref::<crate::engine::sim::weapon::Weapons>().and_then(|w| {
                w.iter().filter(|s| s.unit == id).find_map(|s| match s.target {
                    Some(crate::engine::sim::weapon::WeaponTarget::Entity(t)) => Some(t),
                    _ => None,
                })
            })
        });
        match target.filter(|&t| world(lua).get(t).is_some_and(|e| !e.destroyed)) {
            Some(t) => crate::engine::api::entity::object_as::<moho::Entity>(lua, t),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Unit,
        /// Units attached to this one (transport cargo), in id order.
        GetCargo = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        let cargo: Vec<_> = world(lua)
            .iter()
            .filter(|e| !e.destroyed && e.kind == EntityKind::Unit && e.attachment.is_some_and(|a| a.parent == id))
            .map(|e| e.id)
            .collect();
        let out = lua.create_table_with_capacity(cargo.len(), 0)?;
        for (i, c) in cargo.into_iter().enumerate() {
            if let Some(obj) = crate::engine::api::entity::object(lua, c)? {
                out.raw_set(i + 1, obj)?;
            }
        }
        Ok(shapes::List::<Obj<moho::Unit>>::new(out))
    })?;
    lua_method!(lua, moho::Unit, SetBreakOffTriggerMult = |lua, (this, m): (Obj<moho::Unit>, f64)| {
        with_unit(lua, &this, |u| u.break_off_trigger = Some(crate::engine::sim::math::from_lua(m)))
    })?;
    lua_method!(lua, moho::Unit, SetBreakOffDistanceMult = |lua, (this, m): (Obj<moho::Unit>, f64)| {
        with_unit(lua, &this, |u| u.break_off_distance = Some(crate::engine::sim::math::from_lua(m)))
    })?;
    lua_method!(lua, moho::Unit, SetDoNotTarget = |lua, (this, on): (Obj<moho::Unit>, bool)| with_unit(lua, &this, |u| u.do_not_target = on))?;
    lua_method!(lua, moho::Unit, SetCreator = |lua, (this, creator): (Obj<moho::Unit>, Option<Obj<moho::Entity>>)| {
        let creator = creator.map(|c| entity_id(&c)).transpose()?;
        with_unit(lua, &this, |u| u.creator = creator)
    })?;
    lua_method!(lua, moho::Unit,
        /// The unit that made this one, if alive. Moho has `GetCreator` on the
        /// user side only (`UserUnit`, SupremeCommander.exe); ours, harmlessly
        /// wider.
        GetCreator = |lua, this: Obj<moho::Unit>| {
        let creator = with_unit(lua, &this, |u| u.creator)?;
        match creator.filter(|&c| world(lua).get(c).is_some_and(|e| !e.destroyed)) {
            Some(c) => crate::engine::api::entity::object_as::<moho::Unit>(lua, c),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Unit,
        /// Carries the interface's auto-surface toggle to the sim
        /// (`player_input`). Moho has `SetAutoSurfaceMode` as a user global
        /// and `IsAutoSurfaceMode` on `UserUnit` only (MohoEngine.dll).
        SetAutoSurfaceMode = |lua, (this, on): (Obj<moho::Unit>, bool)| with_unit(lua, &this, |u| u.auto_surface = on))?;
    lua_method!(lua, moho::Unit,
        /// Whether the unit surfaces on its own (see `SetAutoSurfaceMode`).
        IsAutoSurfaceMode = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.auto_surface))?;
    lua_method!(lua, moho::Unit,
        /// Auto construction (silos build missiles without orders): the script
        /// hears when it turns on or off.
        SetAutoMode = |lua, (this, on): (Obj<moho::Unit>, bool)| {
        let changed = with_unit(lua, &this, |u| std::mem::replace(&mut u.auto_mode, on) != on)?;
        if changed {
            if on { UnitCallbacks::on_auto_mode_on(lua, &this, "unit")? } else { UnitCallbacks::on_auto_mode_off(lua, &this, "unit")? }
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Unit,
        /// Whether auto mode is on, for scenario tests. Not moho's: it has
        /// `SetAutoMode` only.
        __engine_GetAutoMode = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.auto_mode))?;
    lua_method!(lua, moho::Unit, TransportHasSpaceFor = |lua, (this, target): (Obj<moho::Unit>, Obj<moho::Entity>)| {
        Ok(crate::engine::api::transport::has_space_for(lua, entity_id(&this)?, entity_id(&target)?))
    })?;
    lua_method!(lua, moho::Unit,
        /// Fuel: tracked, not yet drained by flight (no air staging either).
        GetFuelRatio = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        Ok(world(lua).get(id).and_then(|e| e.unit.as_deref()).map_or(-1.0, |u| u.fuel_ratio.to_num::<f64>()))
    })?;
    lua_method!(lua, moho::Unit, SetFuelRatio = |lua, (this, r): (Obj<moho::Unit>, f64)| {
        let id = entity_id(&this)?;
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.fuel_ratio = crate::engine::sim::math::from_lua(r.clamp(-1.0, 1.0));
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Unit, GetFuelUseTime = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        Ok(world(lua).get(id).and_then(|e| e.unit.as_deref()).map_or(0.0, |u| u.fuel_use_time.to_num::<f64>()))
    })?;
    lua_method!(lua, moho::Unit, SetFuelUseTime = |lua, (this, t): (Obj<moho::Unit>, f64)| {
        let id = entity_id(&this)?;
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.fuel_use_time = crate::engine::sim::math::from_lua(t.max(0.0));
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Unit, SetReclaimable = |lua, (this, on): (Obj<moho::Unit>, bool)| {
        with_unit(lua, &this, |u| u.reclaimable = Some(on))
    })?;
    lua_method!(lua, moho::Unit, GetCurrentLayer = |lua, this: Obj<moho::Unit>| {
        with_unit(lua, &this, |u| u.layer.as_str())
    })?;
    lua_method!(lua, moho::Unit, GetUnitId = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        Ok(world(lua).get(id).map(|e| e.cold.blueprint_id.clone()))
    })?;
    Ok(())
}

/// `health` / `max` floored to quarters (0-4): moho's `OnHealthChanged`
/// arguments are these steps (FAF compares `oldHealth == 0.75`).
fn health_band(health: Fx, max: Fx) -> u8 {
    if max <= Fx::ZERO {
        return 4;
    }
    // floor(4 health / max), clamped to 0..=4, without the 128-bit
    // division (a third of this pass in a 63-army game): the steps k with
    // k max <= 4 health.
    let four_h = health.max(Fx::ZERO).saturating_mul(Fx::from_num(4));
    let mut band = 0u8;
    let mut step = max;
    while band < 4 && step <= four_h {
        band += 1;
        step = step.saturating_add(max);
    }
    band
}

/// Once a tick (after weapons and projectiles): living units whose health
/// crossed a quarter step since the last check get `OnHealthChanged(new,
/// old)` with the steps as fractions (FAF's damage smoke and fire, brain
/// events). Dead units are left to `OnKilled`.
/// Factory repeat build on / off; FAF hears `OnStartRepeatQueue` /
/// `OnStopRepeatQueue` when it changes.
pub(crate) fn set_repeat_queue(lua: &Lua, unit: u32, on: bool) -> LuaResult<()> {
    let changed = match world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        Some(u) => std::mem::replace(&mut u.repeat_queue, on) != on,
        None => return Ok(()),
    };
    if changed
        && let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
        if on { UnitCallbacks::on_start_repeat_queue(lua, &obj, "unit")? } else { UnitCallbacks::on_stop_repeat_queue(lua, &obj, "unit")? }
    }
    Ok(())
}

/// Sets a unit's focus (what it builds / repairs / reclaims / captures);
/// a new focus is announced with `OnAssignedFocusEntity`.
pub(crate) fn set_focus(lua: &Lua, unit: u32, focus: Option<u32>) -> LuaResult<()> {
    let changed = match world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        Some(u) => std::mem::replace(&mut u.focus, focus) != focus,
        None => return Ok(()),
    };
    if changed && focus.is_some()
        && let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
        UnitCallbacks::on_assigned_focus_entity(lua, &obj, "unit")?;
    }
    Ok(())
}

pub fn health_changes_tick(lua: &Lua) -> LuaResult<()> {
    let changed: Vec<(u32, u8, u8)> = {
        let Some(mut w) = lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>() else { return Ok(()) };
        let mut out = Vec::new();
        w.for_each_unit_mut(|e| {
            if e.destroyed || e.health <= Fx::ZERO {
                return;
            }
            let band = health_band(e.health, e.max_health);
            let Some(u) = e.unit.as_deref_mut() else { return };
            match u.health_band.replace(band) {
                Some(old) if old != band => out.push((e.id, band, old)),
                _ => {}
            }
        });
        out
    };
    let _lua = crate::prof_span!("lua health callbacks");
    for (id, new, old) in changed {
        if let Some(obj) = crate::engine::api::entity::object(lua, id)? {
            UnitCallbacks::on_health_changed(lua, &obj, "unit", new as f64 / 4.0, old as f64 / 4.0)?;
        }
    }
    Ok(())
}

/// Would unit `id` put army `to` over its unit cap (none, 0, or ignored:
/// never)?
fn over_cap(lua: &Lua, id: crate::engine::sim::entity::EntityId, to: u32) -> bool {
    let Some((cap, ignore)) = lua.app_data_ref::<crate::engine::sim::army::Armies>().and_then(|a| a.get(to).map(|a| (a.unit_cap, a.ignore_unit_cap))) else { return false };
    if ignore || cap == 0 {
        return false;
    }
    let w = world(lua);
    let cost = w.get(id).and_then(|e| e.unit.as_deref().map(|u| u.cap_cost)).unwrap_or(Fx::ONE);
    let used = w
        .units()
        .filter(|e| !e.destroyed && e.army == Some(to))
        .filter_map(|e| e.unit.as_deref().map(|u| u.cap_cost))
        .fold(Fx::ZERO, |a, b| a.saturating_add(b));
    used.saturating_add(cost) > Fx::from_num(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The multiply-compare band is the old `floor(4 health / max)`.
    #[test]
    fn health_band_matches_the_division() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..100_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let max = Fx::from_bits(((x >> 8) % (100_000u64 << 32)) as i64 + 1);
            let health = Fx::from_bits(((x >> 3) % ((max.to_bits() as u64) * 5 / 4 + 1)) as i64);
            let old = ((health.max(Fx::ZERO) * Fx::from_num(4) / max).to_num::<i64>()).clamp(0, 4) as u8;
            assert_eq!(health_band(health, max), old, "{health} / {max}");
        }
        assert_eq!(health_band(Fx::from_num(75), Fx::from_num(100)), 3);
        assert_eq!(health_band(Fx::from_num(100), Fx::from_num(100)), 4);
    }
}
