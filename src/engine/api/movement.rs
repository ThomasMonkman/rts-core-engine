//! Runs `sim::movement` each tick and reports motion changes to scripts
//! (`OnMotionHorzEventChange(new, old)`).

use mlua::prelude::*;

use crate::engine::api::callbacks::{PropCallbacks, UnitCallbacks};
use crate::engine::api::script_object;
use crate::lua_api::Obj;
use crate::engine::sim::command::Commands;
use crate::engine::sim::entity::{EntityWorld, Tree};
use crate::engine::sim::math::{Fx, FxVec3};
use crate::engine::sim::movement::{self, MotionKind};

/// Reused per-tick buffers, and movement state carried between ticks. A
/// keyframe saves all of it but the two shown to be rebuilt the same from
/// scratch (`tests/keyframes.rs` late game): `avoidance` looks like scratch
/// and isn't (last tick's pushes and sliding units steer this tick's).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct MovementScratch {
    candidates: Vec<u32>,
    moved: Vec<movement::Moved>,
    movers: Vec<u32>,
    /// Units come to rest idle this tick (they settle off squad-mates).
    arrived: Vec<u32>,
    avoidance: crate::engine::sim::avoidance::Scratch,
    events: Vec<(u32, movement::HorzState, movement::HorzState)>,
    /// Search buffers for formation centres' paths (generation-stamped:
    /// results don't depend on what they held; not in keyframes).
    #[serde(skip)]
    formation_search: crate::engine::sim::nav::Search,
    /// Flow fields in use (derived from orders and the grid; rebuilt as
    /// needed, the same: not in keyframes).
    #[serde(skip)]
    flows: crate::engine::sim::flow::Flows,
    /// Not scratch: units waiting for a path, and their place (sim state).
    paths: movement::PathQueue,
    /// Not scratch: units with a leash (`UnitData::leash`), ascending.
    leashed: Vec<u32>,
    /// Not scratch: patrol / aggressive move tasks' steps, ascending
    /// (`movement::patrol_steps`).
    patrol_steps: Vec<(u32, movement::PatrolStep)>,
    /// Not scratch: the spots landing and landed aircraft hold.
    landing_spots: crate::engine::sim::landing::LandingSpots,
}

/// Aircraft killed in the air, falling (sim state): unit -> its fall
/// (`movement::crash_step`).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Falling(pub std::collections::BTreeMap<u32, movement::Fall>);

impl Falling {
    /// A falling unit's velocity, world units per tick.
    pub fn velocity(lua: &Lua, unit: u32) -> Option<FxVec3> {
        lua.app_data_ref::<Falling>().and_then(|f| f.0.get(&unit).map(|f| f.velocity))
    }
}

/// Submarines a task wants surfaced (sim state, ascending once drained):
/// moho's carrier retrieve and unload tasks call
/// `CUnitMotion::SetNewTargetLayer(Water)` on a `SurfacingSub` in the `Sub`
/// layer (MohoEngine.dll 0x101f1571, 0x102101bd), asked for before
/// movement (`transport::surface_tasks`) and started by this tick's
/// [`dive_orders`], or by an attack order (`AutoSurfaceToAttack`).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct SurfaceRequests(pub Vec<u32>);

/// Asks for `unit` to surface (see [`SurfaceRequests`]).
pub fn request_surface(lua: &Lua, unit: u32) {
    if let Some(mut r) = lua.app_data_mut::<SurfaceRequests>() {
        r.0.push(unit);
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(MovementScratch::default());
    lua.set_app_data(Falling::default());
    lua.set_app_data(SurfaceRequests::default());
    Ok(())
}

/// A unit was just killed: a finished aircraft in the air (moho: in the
/// `Air` layer, or a transport hovering with cargo, which ours always do
/// in the air) starts to fall with the velocity it was flying at and a
/// random tumble (`CUnitMotion::CalcMoveAir` 0x1029d9d9 puts a dead
/// aircraft in the `Ballistic` motion state). One killed on the ground
/// doesn't move and gets no `OnImpact` from the engine (FAF's
/// `AirUnit.OnKilled` gives it the normal death, or a transport the
/// `ShieldCollider` projectile's impact).
pub fn start_falling(lua: &Lua, unit: u32) {
    let (velocity, orientation, blueprint) = {
        let w = crate::engine::api::entity::world(lua);
        let Some(e) = w.get(unit).filter(|e| !e.destroyed && e.attachment.is_none() && e.fraction_complete >= Fx::ONE) else { return };
        if e.unit.as_deref().is_none_or(|u| u.layer != crate::engine::sim::layer::Layer::Air) {
            return;
        }
        let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
        (movement::flight_velocity(e, terrain.as_deref()), e.orientation, e.cold.blueprint_id.clone())
    };
    // The blueprint's size (its inertia); kills are rare, so read the table.
    let size = crate::engine::api::entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &blueprint).ok().map(|bp| {
        let get = |k: &str| crate::engine::sim::math::from_lua(bp.raw_get::<Option<f64>>(k).ok().flatten().unwrap_or(1.0));
        FxVec3 { x: get("SizeX"), y: get("SizeY"), z: get("SizeZ") }
    });
    let spin = match (size, lua.app_data_mut::<crate::engine::sim::rng::SimRng>()) {
        (Some(size), Some(mut rng)) => movement::crash_spin(orientation, movement::box_inertia(size), || Fx::from_bits((rng.next_u64() >> 32) as i64)),
        _ => crate::engine::sim::math::FxQuat::IDENTITY,
    };
    if let Some(mut f) = lua.app_data_mut::<Falling>() {
        f.0.insert(unit, movement::Fall { velocity, spin });
    }
}

/// Moves falling dead aircraft; those that hit the ground or water take
/// that layer (`OnLayerChange`), then get `OnImpact(with)` (moho's order,
/// 0x1029eee6 then 0x1029f0c9: FAF's wreck and sinking read the layer).
fn crash_tick(lua: &Lua) -> LuaResult<()> {
    let mut landed = Vec::new();
    {
        let (Some(mut world), Some(mut falling)) = (lua.app_data_mut::<EntityWorld>(), lua.app_data_mut::<Falling>()) else { return Ok(()) };
        if falling.0.is_empty() {
            return Ok(());
        }
        let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
        movement::crash_step(&mut world, terrain.as_deref(), &mut falling.0, &mut landed);
    }
    for (unit, with) in landed {
        let layer = match with {
            movement::CrashSurface::Terrain => crate::engine::sim::layer::Layer::Land,
            movement::CrashSurface::Water => crate::engine::sim::layer::Layer::Water,
        };
        set_layer(lua, unit, layer)?;
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            UnitCallbacks::on_impact(lua, &obj, "unit", with.as_str())?;
        }
    }
    Ok(())
}

/// Moves `unit` to `layer`, running `OnLayerChange(new, old)` if it changed
/// (transports: passengers ride in the air).
pub fn set_layer(lua: &Lua, unit: u32, layer: crate::engine::sim::layer::Layer) -> LuaResult<()> {
    let old = {
        let mut w = crate::engine::api::entity::world(lua);
        let Some(u) = w.get_mut(unit).and_then(|e| e.unit.as_deref_mut()) else { return Ok(()) };
        if u.layer == layer {
            return Ok(());
        }
        std::mem::replace(&mut u.layer, layer)
    };
    if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
        UnitCallbacks::on_layer_change(lua, &obj, "unit", layer.as_str(), old.as_str())?;
    }
    Ok(())
}

/// Idle aircraft landing (`sim::landing`), serially after a movement pass:
/// those due look for a spot (found: reserved and flown to; not:
/// `CannotFindPlaceToLand` and the wait starts over), those down on theirs
/// change to the Land (or Water) layer, those taking off to Air and give
/// their spot up.
fn land(
    world: &mut EntityWorld,
    nav: Option<&crate::engine::sim::nav::NavGrid>,
    terrain: Option<&crate::engine::sim::terrain::Terrain>,
    lua: &Lua,
    moved: &[movement::Moved],
    spots: &mut crate::engine::sim::landing::LandingSpots,
    layer_events: &mut Vec<(u32, crate::engine::sim::layer::Layer, crate::engine::sim::layer::Layer)>,
) {
    use crate::engine::sim::landing::{find_spot, Ground, ReservedCells};
    use crate::engine::sim::layer::Layer;
    use movement::LandEvent;
    if !spots.0.is_empty() {
        // Spots of aircraft no longer landing or landed (an order came, or
        // they died).
        spots.0.retain(|&id, _| world.get(id).filter(|e| !e.destroyed).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_state.landing.is_some() || u.layer != Layer::Air));
    }
    if !moved.iter().any(|m| m.land.is_some()) {
        return;
    }
    static STUCK: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| crate::engine::sim::unit_state::UnitStates::index("CannotFindPlaceToLand").expect("state"));
    let tick = crate::engine::api::sim_time::current_tick(lua);
    let bounds = lua
        .app_data_ref::<crate::engine::api::terrain::PlayableRect>()
        .map(|r| (r.min_x, r.min_z, r.max_x, r.max_z))
        .or_else(|| terrain.map(|t| (Fx::ZERO, Fx::ZERO, Fx::from_num(t.width), Fx::from_num(t.height))))
        .unwrap_or((Fx::ZERO, Fx::ZERO, Fx::MAX, Fx::MAX));
    let mut reserved = ReservedCells::new(spots);
    for m in moved {
        let Some(event) = m.land else { continue };
        match event {
            LandEvent::Search => {
                let Some((pos, goal, footprint, on_water)) = world.get(m.unit).and_then(|e| {
                    let u = e.unit.as_deref()?;
                    Some((e.position(), u.motion_state.hold?, u.motion_params.air.land_footprint, u.motion_params.air.lands_on_water))
                }) else {
                    continue;
                };
                let found = find_spot(&Ground { world, nav, terrain, bounds, reserved: &reserved }, m.unit, pos, goal, footprint, on_water);
                let Some(u) = world.get_mut(m.unit).and_then(|e| e.unit.as_deref_mut()) else { continue };
                u.states.set(*STUCK, found.is_none());
                match found {
                    Some((spot, rect)) => {
                        u.motion_state.landing = Some(spot);
                        spots.0.insert(m.unit, rect);
                        reserved.insert(m.unit, rect);
                    }
                    None => u.motion_state.idle_since = Some(tick),
                }
            }
            LandEvent::Touchdown => {
                let Some(e) = world.get_mut(m.unit) else { continue };
                let p = e.position();
                let wet = terrain.is_some_and(|t| t.surface_at(p.x, p.z) > t.height_at(p.x, p.z));
                let Some(u) = e.unit.as_deref_mut() else { continue };
                let layer = if wet { Layer::Water } else { Layer::Land };
                if u.layer != layer {
                    layer_events.push((m.unit, layer, std::mem::replace(&mut u.layer, layer)));
                }
            }
            LandEvent::TakeOff => {
                if let Some(rect) = spots.0.remove(&m.unit) {
                    reserved.remove(m.unit, rect);
                }
                if let Some(u) = world.get_mut(m.unit).and_then(|e| e.unit.as_deref_mut()) {
                    u.states.set(*STUCK, false);
                    if u.layer != Layer::Air {
                        layer_events.push((m.unit, Layer::Air, std::mem::replace(&mut u.layer, Layer::Air)));
                    }
                }
            }
        }
    }
}


/// The unit states a diving / surfacing submarine holds: `MovingDown` /
/// `MovingUp` (`SetNewTargetLayer` sets bit 10 / 11, MohoEngine.dll
/// 0x10297300 / 0x10297335; moho's state enum is 1-based, `MovingDown` 10,
/// `MovingUp` 11). Moho never sets `Diving` / `Surfacing`: the real game's
/// Atlantis reads `MovingDown` through a dive and `MovingUp` while
/// surfacing (`dev/probes/maps/probe_carrier`).
fn dive_states() -> (u32, u32) {
    static STATES: std::sync::LazyLock<(u32, u32)> = std::sync::LazyLock::new(|| {
        let i = |n| crate::engine::sim::unit_state::UnitStates::index(n).expect("state");
        (i("MovingDown"), i("MovingUp"))
    });
    *STATES
}

/// `Dive` orders at the head of a queue: a submerged submarine surfaces, a
/// surfaced one dives (moho's dive task, MohoEngine.dll 0x101f476c ->
/// `CUnitMotion::SetNewTargetLayer` 0x102972e0: unit state `MovingDown`
/// and vertical event `Down`, or `MovingUp` and `Up`), and the order is
/// done.
/// Units that can't dive (no `Elevation`, nothing under water in their
/// caps), or are on their way already, just finish the order.
fn dive_orders(world: &mut EntityWorld, commands: &mut Commands, surface: &mut Vec<u32>, vertical: &mut Vec<(u32, movement::VerticalMotion, movement::VerticalMotion)>) {
    use crate::engine::sim::command::CommandKind;
    use crate::engine::sim::layer::Layer;
    let diving: Vec<u32> = commands.units_doing(&[CommandKind::Dive]).into_iter().filter(|&u| commands.current(u).is_some_and(|c| c.kind == CommandKind::Dive) && !crate::engine::api::orders::held(world, u)).collect();
    for unit in diving {
        commands.pop_front(unit);
        let Some(u) = world.get_mut(unit).and_then(|e| e.unit.as_deref_mut()) else { continue };
        let p = &u.motion_params;
        if p.kind != MotionKind::Water || p.elevation == Fx::ZERO || u.dive.is_some() {
            continue;
        }
        let to = match u.layer {
            Layer::Sub => Layer::Water,
            Layer::Water => Layer::Sub,
            _ => continue,
        };
        start_dive(unit, u, to, vertical);
    }
    // Tasks surfacing a `SurfacingSub` (moho checks only the motion type and
    // the `Sub` layer).
    surface.sort_unstable();
    surface.dedup();
    for unit in surface.drain(..) {
        let Some(u) = world.get_mut(unit).filter(|e| !e.destroyed).and_then(|e| e.unit.as_deref_mut()) else { continue };
        if u.motion_params.surfacing_sub && u.layer == Layer::Sub && u.dive.is_none() {
            start_dive(unit, u, Layer::Water, vertical);
        }
    }
}

/// Starts `unit` diving (`to` = `Sub`) or surfacing (`Water`): the
/// `MovingDown` / `MovingUp` unit state and the `Down` / `Up` vertical
/// event.
fn start_dive(unit: u32, u: &mut crate::engine::sim::entity::UnitData, to: crate::engine::sim::layer::Layer, vertical: &mut Vec<(u32, movement::VerticalMotion, movement::VerticalMotion)>) {
    let (diving_state, surfacing_state) = dive_states();
    let (state, event) = match to {
        crate::engine::sim::layer::Layer::Water => (surfacing_state, movement::VerticalMotion::Up),
        _ => (diving_state, movement::VerticalMotion::Down),
    };
    u.dive = Some(to);
    u.states.set(state, true);
    vertical.push((unit, event, std::mem::replace(&mut u.motion_state.vertical, event)));
}

/// Submarines on their way down or up, one tick ([`movement::dive_step`]):
/// held their depth under the water; arrived, in their new layer, at
/// `Bottom` (submerged) or `Top` (surfaced). They pitch bow first on the
/// way and level off after ([`movement::dive_tilt`]; moho's `SnapToWater`,
/// after this tick's movement). `candidates`: this tick's movement
/// candidates, which include every unit diving, surfacing or levelling off.
fn dive_tick(world: &mut EntityWorld, candidates: &[u32], terrain: Option<&crate::engine::sim::terrain::Terrain>, layers: &mut Vec<(u32, crate::engine::sim::layer::Layer, crate::engine::sim::layer::Layer)>, vertical: &mut Vec<(u32, movement::VerticalMotion, movement::VerticalMotion)>) {
    use crate::engine::sim::layer::Layer;
    let Some(t) = terrain else { return };
    let (diving_state, surfacing_state) = dive_states();
    let mut moved = Vec::new();
    for &id in candidates {
        let Some(e) = world.get_mut(id).filter(|e| !e.destroyed && e.attachment.is_none()) else { continue };
        if e.unit.as_deref().is_none_or(|u| u.dive.is_none() && !movement::dive_tilting(u.dive_tilt)) {
            continue;
        }
        let p = e.position();
        let (nose, heading) = (e.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE }), movement::facing(e));
        let Some(u) = e.unit.as_deref_mut() else { continue };
        let Some(to) = u.dive else {
            // Levelling off after a dive (no depth change: the target is zero).
            let tilt = movement::dive_tilt(u.dive_tilt, nose, Fx::ZERO);
            (u.dive_tilt, u.motion_state.heading) = (tilt, Some(heading));
            moved.push((e.id, p, movement::dive_orientation(heading, tilt)));
            continue;
        };
        let (depth, done) = movement::dive_step(&u.motion_params, u.depth, to, t.height_at(p.x, p.z), t.surface_at(p.x, p.z));
        let tilt = movement::dive_tilt(u.dive_tilt, nose, if done { Fx::ZERO } else { depth - u.depth });
        // The facing is kept from now on: a tilted orientation's own
        // heading is skewed.
        (u.dive_tilt, u.motion_state.heading) = (tilt, Some(heading));
        u.depth = depth;
        moved.push((e.id, FxVec3 { y: t.surface_at(p.x, p.z) + depth, ..p }, movement::dive_orientation(heading, tilt)));
        if done {
            let u = e.unit.as_deref_mut().expect("unit");
            u.dive = None;
            u.states.set(if to == Layer::Sub { diving_state } else { surfacing_state }, false);
            let event = if to == Layer::Sub { movement::VerticalMotion::Bottom } else { movement::VerticalMotion::Top };
            vertical.push((e.id, event, std::mem::replace(&mut u.motion_state.vertical, event)));
            if u.layer != to {
                layers.push((e.id, to, std::mem::replace(&mut u.layer, to)));
            }
        }
    }
    for (id, at, q) in moved {
        world.set_transform(id, at, q);
    }
}

pub fn tick(lua: &Lua) -> LuaResult<()> {
    crash_tick(lua)?;
    // Submarines diving and surfacing: (unit, new, old) vertical events.
    let mut dive_events: Vec<(u32, movement::VerticalMotion, movement::VerticalMotion)> = Vec::new();
    let mut layer_events: Vec<(u32, crate::engine::sim::layer::Layer, crate::engine::sim::layer::Layer)> = Vec::new();
    // (unit, new terrain type code, old): movement effects per terrain type.
    let mut terrain_events: Vec<(u32, u8, u8)> = Vec::new();
    // (tree, unit, kind, unit -> tree direction, depth): trees knocked over.
    let mut tree_hits: Vec<(u32, u32, crate::engine::sim::entity::Tree, FxVec3, Fx)> = Vec::new();
    let mut events = {
        let (Some(mut world), Some(mut commands), Some(mut s)) =
            (lua.app_data_mut::<EntityWorld>(), lua.app_data_mut::<Commands>(), lua.app_data_mut::<MovementScratch>())
        else {
            return Ok(());
        };
        let MovementScratch { candidates, moved, movers, arrived, avoidance, events, formation_search, flows, paths, leashed, patrol_steps, landing_spots } = &mut *s;
        // Sector graphs up to date with the grid (only where it changed: a
        // structure placed or destroyed, terrain flattened, a footprint class
        // seen for the first time), every tick, so the cost lands when the
        // change happens rather than on the next tick with orders.
        if let Some(mut n) = lua.app_data_mut::<crate::engine::sim::nav::NavGrid>() {
            n.refresh();
        }
        movement::candidates(&world, &commands, candidates);
        let _section = crate::prof_span!("movement targets");
        // From one pass over the weapons (id order, by weapon index): each
        // unit's first target, as sorted lists (`UnitMap`):
        // - body-slaved weapons' targets and arcs, to turn to face them;
        // - entity targets, for winged attack runs (and `engaged`);
        // - ground targets (attack-ground orders).
        let mut face: movement::UnitMap<(FxVec3, Fx)> = movement::UnitMap::default();
        let mut air_targets: movement::UnitMap<u32> = movement::UnitMap::default();
        let mut ground_targets: movement::UnitMap<FxVec3> = movement::UnitMap::default();
        if let Some(w) = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>() {
            for s in w.iter() {
                match s.target {
                    Some(crate::engine::sim::weapon::WeaponTarget::Entity(t)) => air_targets.push_first(s.unit, t),
                    Some(crate::engine::sim::weapon::WeaponTarget::Ground(p)) => ground_targets.push_first(s.unit, p),
                    None => {}
                }
                // Units with an `AttackAngle` turn to an attack order's
                // target, to within a degree (inferred), whatever their
                // body-slaved weapons' arcs: moho's angle branch skips them.
                // Not to a target their weapons found themselves: moho
                // turns in its move code (`CalcMoveCommon`), and an idle
                // Fatboy firing at all four sides stays as it is (FAF
                // probe probe_turretreach).
                let attack_angle = s.target.is_some() && s.target_from_order && world.get(s.unit).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_params.attack_angle > Fx::ZERO);
                if (s.slaved_to_body || attack_angle) && face.0.last().is_none_or(|l| l.0 < s.unit) {
                    // The weapon's target point (the bone it picked): a
                    // body facing the target's centre leaves a rack fixed
                    // to it off a side bone by more than its firing
                    // tolerance up close (a Cybran MML at 25 never fired at
                    // a Monkeylord; the real one does, FAF probe
                    // probe_beamhit).
                    let at = match s.target {
                        Some(crate::engine::sim::weapon::WeaponTarget::Entity(t)) => world.get(t).filter(|e| !e.destroyed).map(|e| crate::engine::sim::weapon::target_point_of(s, e)),
                        Some(crate::engine::sim::weapon::WeaponTarget::Ground(p)) => Some(p),
                        None => None,
                    };
                    if let Some(at) = at {
                        face.push_first(s.unit, (at, if attack_angle { Fx::PI / 180 } else { s.slaved_arc }));
                    }
                }
            }
        }
        // Builders turning to face their work before starting it
        // (`NeedToFaceTargetToBuild`): face it fully.
        if let Some(f) = lua.app_data_ref::<crate::engine::api::construction::FaceToBuild>().filter(|f| !f.0.is_empty()) {
            let extra: Vec<_> = f.0.iter().filter(|(u, _)| face.get(u).is_none()).map(|&(u, p)| (u, (p, Fx::PI / 180))).collect();
            if !extra.is_empty() {
                face.0.extend(extra);
                face.0.sort_unstable_by_key(|f| f.0);
            }
        }
        // The range each unit engages its weapons' aim (a unit, else a ground
        // point) and its attack order's target with (`Weapons::range_for`):
        // what a hovering aircraft circles at and how close an attack closes.
        let mut aim_ranges: movement::UnitMap<Fx> = movement::UnitMap::default();
        let mut order_ranges: movement::UnitMap<Fx> = movement::UnitMap::default();
        // Submerged units that surface to attack (`AutoSurfaceToAttack`).
        let mut auto_surface: Vec<u32> = Vec::new();
        if let Some(w) = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>() {
            use crate::engine::sim::layer::LayerMask;
            use crate::engine::sim::weapon::RangeTarget;
            let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
            let unit_target = |t: u32| world.get(t).and_then(|e| Some(RangeTarget::Unit { layer: e.unit.as_deref()?.layer.mask(), category: e.category_index }));
            let ground = |p: FxVec3| RangeTarget::Ground {
                layer: if terrain.as_deref().is_some_and(|t| t.surface_at(p.x, p.z) > t.height_at(p.x, p.z)) { LayerMask::WATER } else { LayerMask::LAND },
            };
            // Only hovering aircraft circle (the rest never read it). A unit
            // target wins over a ground point, as in `movement::step`.
            let hovering = |u: &u32| world.get(*u).and_then(|e| e.unit.as_deref()).is_some_and(|d| d.layer == crate::engine::sim::layer::Layer::Air && !d.motion_params.winged);
            let mut aims: Vec<(u32, Option<RangeTarget>)> = air_targets
                .0
                .iter()
                .filter(|(u, _)| hovering(u))
                .map(|&(u, t)| (u, unit_target(t)))
                .chain(ground_targets.0.iter().filter(|(u, _)| air_targets.get(u).is_none() && hovering(u)).map(|&(u, p)| (u, Some(ground(p)))))
                .collect();
            aims.sort_unstable_by_key(|a| a.0);
            for (u, target) in aims {
                if let Some(r) = target.and_then(|t| w.range_for(u, t)) {
                    aim_ranges.push_first(u, r);
                }
            }
            use crate::engine::sim::command::{CommandKind as K, CommandTarget as T};
            for u in commands.units_doing(&[K::Attack, K::FormAttack, K::OverCharge]) {
                let target = match commands.current(u).map(|c| c.target) {
                    Some(T::Entity(t)) => unit_target(t),
                    Some(T::Ground(p)) => Some(ground(p)),
                    _ => None,
                };
                match target.map(|t| w.range_for(u, t)) {
                    Some(Some(r)) => order_ranges.push_first(u, r),
                    // No weapon can hit it from under the water: a
                    // `SurfacingSub` with `AI.AutoSurfaceToAttack` surfaces
                    // (`CUnitAttackTargetTask`, MohoEngine.dll 0x101df772..
                    // 0x101df8d4; moho checks as the attack starts).
                    Some(None) if world.get(u).and_then(|e| e.unit.as_deref()).is_some_and(|d| d.motion_params.auto_surface_to_attack && d.motion_params.surfacing_sub && d.layer == crate::engine::sim::layer::Layer::Sub) => {
                        auto_surface.push(u);
                    }
                    _ => {}
                }
            }
        }
        // Patrols and aggressive moves act on their weapons' targets a step
        // or two late (`movement::patrol_steps`).
        let held = movement::patrol_steps(&world, &commands, &air_targets, crate::engine::api::sim_time::current_tick(lua), patrol_steps);
        // Patrols, aggressive moves and guards chase their weapons' targets
        // into range, leashed (`movement::chases`); a chase closes to the
        // weapon's range of the target.
        let (chases, broke_off) = movement::chases(&mut world, &commands, &held, leashed);
        if !chases.is_empty()
            && let Some(w) = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>() {
            use crate::engine::sim::weapon::RangeTarget;
            for &(u, t) in &chases.0 {
                let target = world.get(t).and_then(|e| Some(RangeTarget::Unit { layer: e.unit.as_deref()?.layer.mask(), category: e.category_index }));
                if let Some(r) = target.and_then(|t| w.range_for(u, t)) {
                    order_ranges.push_first(u, r);
                }
            }
        }
        // (Facing units are movement candidates too.)
        if !face.is_empty() {
            candidates.extend(face.keys().copied());
            candidates.sort_unstable();
            candidates.dedup();
        }
        // Hovering aircraft circle what they shoot at, moving or not.
        let before = candidates.len();
        candidates.extend(air_targets.keys().chain(ground_targets.keys()).copied().filter(|&u| {
            world.get(u).and_then(|e| e.unit.as_deref()).is_some_and(|d| d.layer == crate::engine::sim::layer::Layer::Air && !d.motion_params.winged)
        }));
        if candidates.len() != before {
            candidates.sort_unstable();
            candidates.dedup();
        }
        // Transports short of their flight height keep a movement tick: one
        // on a load/unload order lowers to its `TransportHoverHeight`, one
        // that just finished climbs back to its `Elevation` (moho's
        // `ShouldHoverInsteadOfLand`, MohoEngine.dll 0x1029b210).
        let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
        let before = candidates.len();
        candidates.extend(world.units().filter(|e| {
            !e.destroyed
                && e.attachment.is_none()
                && e.unit.as_deref().is_some_and(|u| {
                    u.layer == crate::engine::sim::layer::Layer::Air
                        && u.motion_params.air.transport_hover_height > Fx::ZERO
                        && terrain.as_deref().is_some_and(|t| {
                            let want = if commands.current(e.id).is_some_and(|c| c.kind.is_transport_load_unload()) {
                                u.motion_params.air.transport_hover_height
                            } else {
                                u.motion_params.elevation
                            };
                            let p = e.position();
                            (p.y - t.surface_at(p.x, p.z) - want).abs() > Fx::from_num(0.05)
                        })
                })
        }).map(|e| e.id));
        if candidates.len() != before {
            candidates.sort_unstable();
            candidates.dedup();
        }
        // Stunned units hold still.
        let now = crate::engine::api::sim_time::current_tick(lua);
        candidates.retain(|&u| world.get(u).and_then(|e| e.unit.as_deref()).is_none_or(|d| d.stunned_until <= now));
        if candidates.is_empty() && avoidance.is_settled(world.id_end()) {
            return Ok(());
        }
        let nav = lua.app_data_ref::<crate::engine::sim::nav::NavGrid>();
        // Units with a weapon on a target, or reclaiming on the way, hold
        // during aggressive moves / patrols.
        // (Not those that broke off a chase: they carry on.)
        let mut engaged: Vec<u32> = held.keys().copied().filter(|u| broke_off.binary_search(u).is_err()).collect();
        if let Some(r) = lua.app_data_ref::<crate::engine::api::reclaim::Reclaims>() {
            engaged.extend(r.reclaiming());
        }
        let engaged = movement::UnitSet::new(engaged);
        let _ = _section;
        let _section = crate::prof_span!("movement formations");
        // Travelling formations move their centres first (members then
        // steer for the new slots).
        let (searches, work) = crate::engine::sim::formation::travel_tick(&mut commands, &world, nav.as_deref(), crate::engine::api::sim_time::current_tick(lua), formation_search);
        if searches > 0
            && let Some(mut t) = lua.app_data_mut::<crate::engine::watchdog::StageTimes>() {
            t.path_searches += searches;
            t.path_work += work;
        }
        let _ = _section;
        let _section = crate::prof_span!("movement flows");
        // Flow fields for this tick's followers of big plain orders.
        if let Some(nav) = nav.as_deref() {
            let wants: Vec<_> = candidates
                .iter()
                .filter_map(|&u| {
                    // (The unit first: aircraft and the like have no class,
                    // and that's cheaper to see than the order.)
                    let e = world.get(u)?;
                    let class = movement::nav_class(e.unit.as_deref()?, nav)?;
                    let goal = movement::flow_goal(commands.current(u)?)?;
                    Some((class, nav.cell_for(class, goal), nav.cell_for(class, e.position())))
                })
                .collect();
            let (built, work) = flows.prepare(nav, &wants);
            if (built > 0 || work > 0)
                && let Some(mut t) = lua.app_data_mut::<crate::engine::watchdog::StageTimes>() {
                t.path_searches += built;
                t.path_work += work;
            }
        }
        let _ = _section;
        // Builders walking to a site: where the planned structure stands
        // (snapped to the grid, as construction starts it) and its skirt
        // size, which their build range adds (`movement::goal`). Skirts read
        // once per blueprint.
        let mut build_sites: movement::UnitMap<(FxVec3, Fx)> = movement::UnitMap::default();
        {
            let mut by_bp: std::collections::BTreeMap<&str, Fx> = std::collections::BTreeMap::new();
            for u in commands.units_doing(&[crate::engine::sim::command::CommandKind::BuildMobile]) {
                let Some(cmd) = commands.current(u) else { continue };
                let (Some(bp), crate::engine::sim::command::CommandTarget::Ground(target)) = (cmd.blueprint.as_deref(), cmd.target) else { continue };
                let skirt = match by_bp.get(bp) {
                    Some(&s) => s,
                    None => {
                        let s = crate::engine::api::construction::skirt_max(lua, bp)?;
                        by_bp.insert(bp, s);
                        s
                    }
                };
                build_sites.push_first(u, (crate::engine::api::construction::snap_site(lua, bp, target)?, skirt));
            }
        }
        let _section = crate::prof_span!("movement step");
        let env = movement::Env { terrain: terrain.as_deref(), nav: nav.as_deref(), engaged: Some(&engaged), air_targets: Some(&air_targets), ground_targets: Some(&ground_targets), aim_ranges: Some(&aim_ranges), order_ranges: Some(&order_ranges), build_sites: Some(&build_sites), chases: Some(&chases), face: Some(&face), crowd: Some(avoidance.index()), flows: Some(flows), granted: None, tick: Some(crate::engine::api::sim_time::current_tick(lua)) };
        let (searches, work) = movement::step_queued(&world, &commands, env, candidates, movement::PARALLEL_MIN, paths, moved);
        if let Some(mut t) = lua.app_data_mut::<crate::engine::watchdog::StageTimes>() {
            t.path_searches += searches;
            t.path_work += work;
        }
        events.clear();
        let _ = _section;
        let _section = crate::prof_span!("movement apply");
        movement::apply(&mut world, &mut commands, moved, events);
        land(&mut world, nav.as_deref(), terrain.as_deref(), lua, moved, landing_spots, &mut layer_events);
        let _ = _section;
        let _section = crate::prof_span!("movement avoidance");
        // Then moving units push apart from their neighbours (`moved` is in
        // id order).
        movers.clear();
        movers.extend(moved.iter().filter(|m| m.state.speed != crate::engine::sim::math::Fx::ZERO).map(|m| m.unit));
        // Units coming to rest with nothing more to do, or parking to work
        // on a target (assisting engineers stopping on the same spot slide
        // off each other rather than stacking).
        arrived.clear();
        arrived.extend(moved.iter().filter(|m| m.state.speed == crate::engine::sim::math::Fx::ZERO && commands.current(m.unit).is_none_or(|c| movement::is_work(c.kind))).map(|m| m.unit));
        crate::engine::sim::avoidance::step(&world, &commands, movers, arrived, crate::engine::sim::avoidance::PARALLEL_MIN, avoidance);
        crate::engine::sim::avoidance::apply(&mut world, avoidance, terrain.as_deref(), nav.as_deref());
        let _ = _section;
        let _section = crate::prof_span!("movement bounds");
        // Units stay inside the playable area (`SetPlayableRect`, else the
        // map), as moho keeps them: aircraft turning wide would otherwise
        // leave it, and FAF's AI loops forever looking for a spot near a
        // target 30+ units off the map (`AIUtils.RandomLocation`).
        {
            let rect = lua.app_data_ref::<crate::engine::api::terrain::PlayableRect>().map(|r| *r).or_else(|| {
                terrain.as_deref().map(|t| crate::engine::api::terrain::PlayableRect { min_x: Fx::ZERO, min_z: Fx::ZERO, max_x: Fx::from_num(t.width), max_z: Fx::from_num(t.height) })
            });
            let ignored = lua.app_data_ref::<crate::engine::api::terrain::IgnorePlayableRect>();
            if let Some(r) = rect {
                for m in moved.iter() {
                    let Some(e) = world.get(m.unit).filter(|e| e.attachment.is_none()) else { continue };
                    if e.army.is_some_and(|a| ignored.as_ref().is_some_and(|i| i.0.contains(&a))) {
                        continue;
                    }
                    let p = e.position();
                    let clamped = FxVec3 { x: p.x.clamp(r.min_x, r.max_x), y: p.y, z: p.z.clamp(r.min_z, r.max_z) };
                    if clamped != p {
                        world.set_position(m.unit, clamped);
                    }
                }
            }
        }
        let _ = _section;
        let _section = crate::prof_span!("movement terrain");
        // Units that moved may have crossed a shore (hover, amphibious) or
        // onto another terrain type.
        if let Some(t) = terrain.as_deref() {
            for m in moved.iter() {
                let Some(e) = world.get_mut(m.unit) else { continue };
                if e.attachment.is_some() {
                    continue;
                }
                let p = e.position();
                let code = t.type_at(p.x, p.z);
                let Some(u) = e.unit.as_deref_mut() else { continue };
                let wet = t.water_surface.is_some_and(|w| crate::engine::sim::layer::wet(u.motion_params.kind, t.height_at(p.x, p.z), w, u.motion_params.layer_change_offset));
                match u.terrain_type.replace(code) {
                    Some(old) if old != code => terrain_events.push((m.unit, code, old)),
                    _ => {}
                }
                // (A submerged submarine stays under: it changes layer
                // diving and surfacing, `dive_tick`.)
                let layer = match crate::engine::sim::layer::Layer::for_motion(u.motion_params.kind, wet) {
                    crate::engine::sim::layer::Layer::Water if u.layer == crate::engine::sim::layer::Layer::Sub => crate::engine::sim::layer::Layer::Sub,
                    l => l,
                };
                // (Aircraft change layer landing and taking off: `land`.)
                if layer != u.layer && u.motion_params.kind != MotionKind::Air {
                    let old = std::mem::replace(&mut u.layer, layer);
                    layer_events.push((m.unit, layer, old));
                }
            }
        }
        let _ = _section;
        let _section = crate::prof_span!("movement trees");
        // Ground units moving into trees knock them over (each tree once).
        let mut near = Vec::new();
        for m in moved.iter() {
            let Some(e) = world.get(m.unit).filter(|e| e.attachment.is_none()) else { continue };
            let Some(u) = e.unit.as_deref() else { continue };
            if matches!(u.motion_params.kind, MotionKind::Air | MotionKind::None) {
                continue;
            }
            let (p, r) = (e.position(), u.radius);
            world.props_in_circle_into(p.x, p.z, r, &mut near);
            for &tree in &near {
                let Some(t) = world.get(tree) else { continue };
                let Some(kind) = t.cold.tree.filter(|k| *k != Tree::Hit) else { continue };
                let d = FxVec3 { x: t.position().x - p.x, y: Fx::ZERO, z: t.position().z - p.z };
                let dir = d.normalized().unwrap_or(FxVec3 { x: Fx::ONE, y: Fx::ZERO, z: Fx::ZERO });
                tree_hits.push((tree, m.unit, kind, dir, (r - d.length()).max(Fx::ZERO)));
            }
        }
        for &(tree, ..) in &tree_hits {
            if let Some(t) = world.get_mut(tree) {
                t.cold.tree = Some(Tree::Hit);
            }
        }
        let _ = _section;
        dive_tick(&mut world, candidates, terrain.as_deref(), &mut layer_events, &mut dive_events);
        // (After the dives: one ordered now first moves next tick, as the
        // real game's do, dev/probes probe_water.)
        if let Some(mut surface) = lua.app_data_mut::<SurfaceRequests>() {
            surface.0.append(&mut auto_surface);
            dive_orders(&mut world, &mut commands, &mut surface.0, &mut dive_events);
        }
        std::mem::take(events)
    };
    let _callbacks = crate::prof_span!("lua movement callbacks");
    for (tree, unit, kind, dir, depth) in std::mem::take(&mut tree_hits) {
        let (Some(t), Some(u)) = (crate::engine::api::entity::object(lua, tree)?, crate::engine::api::entity::object(lua, unit)?) else { continue };
        let (nx, ny, nz) = dir.to_f64();
        match kind {
            Tree::Single => PropCallbacks::on_collision(lua, &t, "prop", Obj::new(u), nx, ny, nz, depth.to_num::<f64>())?,
            // A tree group's `OnCollision` takes (unit, direction vector), not
            // the single tree's (unit, normal, depth): not a declared callback.
            _ => script_object::run_script(lua, &t, "OnCollision", "prop", (u, crate::engine::api::entity::vector(lua, dir)?))?,
        }
    }
    for (unit, new, old) in std::mem::take(&mut layer_events) {
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            UnitCallbacks::on_layer_change(lua, &obj, "unit", new.as_str(), old.as_str())?;
        }
    }
    for (unit, new, old) in terrain_events {
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            let entry = |code| crate::engine::api::terrain::terrain_type_entry(lua, code).map(crate::lua_api::shapes::Loose::new);
            UnitCallbacks::on_terrain_type_change(lua, &obj, "unit", entry(new)?, entry(old)?)?;
        }
    }
    for &(unit, new, old) in &events {
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            UnitCallbacks::on_motion_horz_event_change(lua, &obj, "unit", new.name(), old.name())?;
        }
    }
    // Aircraft taking off / reaching height / descending / landing (not
    // while still being built: FAF sets up the landing animations in
    // OnStopBeingBuilt).
    let vertical: Vec<(u32, movement::VerticalMotion, movement::VerticalMotion)> = {
        let w = lua.app_data_ref::<EntityWorld>();
        lua.app_data_ref::<MovementScratch>()
            .map(|s| {
                s.moved
                    .iter()
                    .filter(|m| w.as_deref().and_then(|w| w.get(m.unit)).is_some_and(|e| e.fraction_complete >= crate::engine::sim::math::Fx::ONE))
                    .filter_map(|m| m.vertical_event.map(|(n, o)| (m.unit, n, o)))
                    .collect()
            })
            .unwrap_or_default()
    };
    for (unit, new, old) in dive_events.into_iter().chain(vertical) {
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            UnitCallbacks::on_motion_vert_event_change(lua, &obj, "unit", new.name(), old.name())?;
        }
    }
    // Hand the buffer back for reuse.
    events.clear();
    if let Some(mut s) = lua.app_data_mut::<MovementScratch>() {
        s.events = events;
    }
    Ok(())
}

/// Fuel: airborne, unattached units with fuel (`Physics.FuelUseTime`) burn
/// `1 / FuelUseTime` of a tank per second; on running dry, FAF's
/// `OnRunOutOfFuel` (its speed penalty) runs. Landed aircraft refuel at a
/// tenth of their docked rate; docked ones refuel at air staging
/// (`api::transport`).
pub fn fuel_tick(lua: &Lua) -> LuaResult<()> {
    use crate::engine::sim::math::Fx;
    let mut refuelled = Vec::new();
    let empty: Vec<u32> = {
        let Some(mut world) = lua.app_data_mut::<EntityWorld>() else { return Ok(()) };
        let mut out = Vec::new();
        world.for_each_unit_mut(|e| {
            if e.destroyed || e.attachment.is_some() {
                return;
            }
            let Some(u) = e.unit.as_deref_mut() else { return };
            if u.fuel_use_time <= Fx::ZERO {
                return;
            }
            // Landed away from air staging: a tenth of the docked refuel rate
            // (moho's fuel tick with no staging parent, MohoEngine.dll
            // 0x102982f0).
            if u.layer != crate::engine::sim::layer::Layer::Air {
                if u.motion_params.kind == crate::engine::sim::movement::MotionKind::Air && u.fuel_ratio < Fx::ONE {
                    let old = u.fuel_ratio.max(Fx::ZERO);
                    u.fuel_ratio = (old + u.fuel_recharge / (u.fuel_use_time * Fx::from_num(100))).min(Fx::ONE);
                    if old == Fx::ZERO && u.fuel_ratio > Fx::ZERO {
                        refuelled.push(e.id);
                    }
                }
                return;
            }
            if u.fuel_ratio <= Fx::ZERO {
                return;
            }
            u.fuel_ratio = (u.fuel_ratio - Fx::ONE / (u.fuel_use_time * Fx::from_num(10))).max(Fx::ZERO);
            if u.fuel_ratio == Fx::ZERO {
                out.push(e.id);
            }
        });
        out
    };
    for unit in empty {
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            UnitCallbacks::on_run_out_of_fuel(lua, &obj, "unit")?;
        }
    }
    for unit in refuelled {
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            UnitCallbacks::on_got_fuel(lua, &obj, "unit")?;
        }
    }
    Ok(())
}
