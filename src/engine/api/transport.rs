//! Transports and air staging: `IssueTransportLoad` /
//! `__engine_IssueTransportReverseLoad` (tests) / `IssueTransportUnload` /
//! `IssueTransportUnloadSpecific`, the `DetachFromTransport` order, and ferry
//! routes (`IssueFerry`).
//!
//! A transport's slots are its attach bones: names containing `Attachpoint`
//! (any case), sized by the name: `Lrg` large, `Med` medium, anything else
//! small (`Attachpoint_sml_01`, `Attachpoint_Small_01`, `Left_Attachpoint03`,
//! ...). A unit of blueprint `Transport.TransportClass` 1 / 2 / 3 (1 when
//! unset) takes a free small / medium / large slot.
//!
//! Per tick, in unit id order:
//! - Loading: a unit whose current order is `TransportLoadUnits` on a live
//!   transport walks to it (movement); once within reach (XZ) the beam picks
//!   it up: `OnStartTransportBeamUp(transport, bone)` runs, the unit holds
//!   still and rises to its slot over ~9 ticks (~0.9 s, cosine-eased, each
//!   boarder on its own rise so boarding overlaps), then it is attached at
//!   the reserved slot, `transport:OnTransportAttach(bone, unit)` runs
//!   (FAF's handler calls `unit:OnAttachedToTransport`), and the order
//!   ends. No fitting slot: the order ends unloaded.
//! - Reverse loading: a transport ordered onto a unit
//!   (`TransportReverseLoadUnits`: right-clicking a unit with a transport
//!   selected) flies out to it (movement) and the unit boards the same way
//!   once the transport is in reach (the passenger holds no order of its
//!   own, so the beam-up refusal read off its load order doesn't apply).
//! - Unloading: a transport whose current order is `TransportUnloadUnits`
//!   flies to the point; there, each passenger (id order) is set down on the
//!   ground under its slot and `transport:OnTransportDetach(bone, unit)`
//!   runs; the order ends. `TransportUnloadSpecificUnits` unloads only cargo
//!   in the command's category (`IssueTransportUnloadSpecific`). With
//!   nothing aboard (of that category) the order ends at once; a submerged
//!   `SurfacingSub` surfaces before it sets off (moho's `CUnitUnloadUnits`).
//! - Detaching: a passenger ordered out (`DetachFromTransport`) leaves the
//!   transport at once, set down below, its order done.
//! - Ferrying: a transport on a ferry route reads `Ferrying` and loops it
//!   for as long as the order stands: to the beacon, picking up the units
//!   waiting there, out along the route, unloading at its end, and back
//!   ([`crate::engine::api::ferry`]). As a transport starts a ferry order, moho's ferry task
//!   (`CUnitFerryTask`, MohoEngine.dll 0x101f8b20) puts the route's beacon
//!   there: the order's own if it has one, else (`AI.BeaconName`) a new one
//!   for its army at the order's point, and `OnFerryPointSet` runs; the
//!   beacon goes with the order ([`ferry_tasks`]).
//! - Air staging: an aircraft attached to an `AIRSTAGINGPLATFORM` refuels at
//!   `FuelRechargeRate` seconds of flight per second x the platform's
//!   `RefuelingMultiplier`, and while damaged is repaired
//!   `RefuelingRepairAmount` a second for `RepairConsumeEnergy` /
//!   `RepairConsumeMass` a tick (`OnStartRefueling` when that begins,
//!   `OnGotFuel` once it has fuel again); it leaves as soon as it has another
//!   order (FAF's AI docks with `IssueTransportLoad({plane}, platform)`).
//!
//! Carriers (`Transport.StorageSlots` > 0: Atlantis, Keefer, Czar, ...)
//! take units inside instead of onto attach points, one slot each: the unit
//! rides attached, undrawn, untargetable and without collision, and hears
//! `OnAddToStorage(carrier)`; an unload order (or the carrier's death)
//! releases everything with `OnRemoveFromStorage(carrier)` (a dying
//! carrier's cargo is killed by FAF's `KillCargo`), each through the
//! carrier's next launch point: aircraft take the bone's transform and fly
//! off along it at `MaxAirspeed`. A carrier that is also air staging
//! (Atlantis, CZAR, carriers) refuels what it holds, keeps it aboard when
//! it's given another order (the order waits in its queue), and unloads
//! without moving: its aircraft are sent to guard the point and leave one
//! every second tick (moho's `CUnitCarrierLaunch`, timings measured in the
//! real game: `dev/probes/maps/probe_launch`). A submerged `SurfacingSub` carrier (Atlantis)
//! surfaces once every unit of a load order onto it is on that order, and
//! takes them in only once it's up (moho's `CUnitCarrierRetrieve`).
//!
//! Load sequence callbacks: a transport that units are ordered aboard is
//! `TransportLoading` and gets `OnTransportOrdered` + `OnStartTransportLoading`;
//! when no load orders for it remain, `OnTransportAborted` (if nobody boarded)
//! and `OnStopTransportLoading`. Each boarding unit gets
//! `OnStartTransportBeamUp(transport, bone)`, then the attach, then
//! `OnStopTransportBeamUp`. No room: the transport's brain hears
//! `OnTransportFull`. Air staging platforms (docking) skip all of these.
//!
//! Inferred, not verified: the reach rule, slot sizes from bone names without
//! adjacent-slot blocking, the transport waiting rather than flying to its
//! cargo, and unloading all passengers at once (the engine detaches
//! everything in one call: `TransportDetachAllUnits` 0x101d3d00 loops the
//! attached set with no pacing counter).

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::entity::{self, object};
use crate::engine::api::callbacks::{AIBrainCallbacks, UnitCallbacks};
use crate::engine::api::moho;
use crate::lua_api::{Obj, lua_method};
use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::{Attachment, EntityId};
use crate::engine::sim::math::{Fx, FxVec3};
use crate::engine::sim::unit_state::UnitStates;

/// A carrier's `Transport.StorageSlots` (0: not a carrier).
pub(crate) fn storage_slots(lua: &Lua, transport: EntityId) -> u32 {
    let bp_id = entity::world(lua).get(transport).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
    entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)
        .ok()
        .and_then(|bp| bp.get::<Option<LuaTable>>("Transport").ok().flatten())
        .and_then(|t| t.get::<Option<f64>>("StorageSlots").ok().flatten())
        .map_or(0, |n| n.max(0.0) as u32)
}

/// Units stored in `carrier`, id order.
pub(crate) fn stored_units(lua: &Lua, carrier: EntityId) -> Vec<EntityId> {
    entity::world(lua)
        .iter()
        .filter(|e| !e.destroyed && e.attachment.is_some_and(|a| a.parent == carrier) && e.unit.as_deref().is_some_and(|u| u.stored_in == Some(carrier)))
        .map(|e| e.id)
        .collect()
}

/// Puts `unit` inside `carrier`.
fn store(lua: &Lua, carrier: EntityId, unit: EntityId) -> LuaResult<()> {
    {
        let mut w = entity::world(lua);
        w.attach(unit, Attachment { parent: carrier, parent_bone: None, self_bone: None });
        w.set_collision_override(unit, Some(crate::engine::sim::entity::CollisionShape::None));
        if let Some(u) = w.get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
            u.stored_in = Some(carrier);
            u.do_not_target = true;
        }
    }
    set_state(lua, unit, "Attached", true);
    if let Some(mut loading) = lua.app_data_mut::<Loading>()
        && let Some(n) = loading.0.get_mut(&carrier) {
        *n += 1;
    }
    if let (Some(c), Some(u)) = (object(lua, carrier)?, object(lua, unit)?) {
        UnitCallbacks::on_add_to_storage(lua, &u, "unit", Obj::new(c))?;
    }
    Ok(())
}

/// Lets `unit` out of `carrier` through the carrier's next launch point
/// (moho's `CAiTransportImpl::TransportRemoveFromStorage`: the callback,
/// the detach, then the launch bone's transform, [`launch_pose`]). An
/// aircraft takes the bone's position and orientation and is flying along
/// its nose at `Air.MaxAirspeed` (`CUnitCarrierLaunch` 298-323; the real
/// game's interceptors leave the Atlantis at the bone, facing along it, and
/// cover 1.44 the first tick: probe_launch), so it leaves by the deck, not
/// the hull centre;
/// anything else is put on the surface under the point. No launch bones:
/// where the carrier is.
fn release(lua: &Lua, carrier: EntityId, unit: EntityId) -> LuaResult<()> {
    end_service(lua, unit);
    let air = {
        let mut w = entity::world(lua);
        let Some(e) = w.get_mut(unit) else { return Ok(()) };
        e.attachment = None;
        let air = e.unit.as_deref().is_some_and(|u| u.motion_params.kind == crate::engine::sim::movement::MotionKind::Air);
        if let Some(u) = e.unit.as_deref_mut() {
            u.stored_in = None;
            u.do_not_target = false;
        }
        w.set_collision_override(unit, None);
        air
    };
    set_state(lua, unit, "Attached", false);
    if let (Some(c), Some(u)) = (object(lua, carrier)?, object(lua, unit)?) {
        UnitCallbacks::on_remove_from_storage(lua, &u, "unit", Obj::new(c))?;
    }
    if entity::world(lua).get(unit).is_none_or(|e| e.destroyed) {
        return Ok(());
    }
    let pose = launch_pose(lua, carrier);
    if air {
        if let Some((p, q)) = pose {
            let mut w = entity::world(lua);
            w.set_transform(unit, p, q);
            if let Some(u) = w.get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
                let m = &mut u.motion_state;
                m.speed = u.motion_params.max_speed;
                m.course = None;
                m.heading = None;
                m.climb = Fx::ZERO;
                m.horz = crate::engine::sim::movement::HorzState::TopSpeed;
            }
        }
        crate::engine::api::movement::set_layer(lua, unit, crate::engine::sim::layer::Layer::Air)?;
    } else {
        let p = match pose {
            Some((p, _)) => p,
            None => match entity::world(lua).get(unit) {
                Some(e) => e.position(),
                None => return Ok(()),
            },
        };
        let y = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map_or(p.y, |t| t.surface_at(p.x, p.z));
        entity::world(lua).set_position(unit, FxVec3 { y, ..p });
    }
    Ok(())
}

/// The carrier's next launch point, world position and orientation (moho's
/// `TransportRemoveFromStorage`, `SetUpAttachPoints` for the lists): its
/// bones named `Launchpoint` (any case), else its attach points, in
/// skeleton order; the index (`mLaunchAttachIndex`) advances before use and
/// wraps, so the first launch takes the second point. `None`: no such
/// bones. As posed (the Atlantis' doors open on its deck).
///
/// Moho's generic list holds the attach points of class up to
/// `Transport.ClassGenericUpTo`; every stock carrier's are plain
/// (`AttachpointNN`, class 1, and `ClassGenericUpTo` >= 2), so all of them.
fn launch_pose(lua: &Lua, carrier: EntityId) -> Option<(FxVec3, crate::engine::sim::math::FxQuat)> {
    let mut w = entity::world(lua);
    let e = w.get(carrier)?;
    let bones = &e.cold.skeleton.as_ref()?.bones;
    let mut points: Vec<u32> = bones.iter().enumerate().filter(|(_, b)| b.name.to_ascii_lowercase().contains("launchpoint")).map(|(i, _)| i as u32).collect();
    if points.is_empty() {
        points = bones.iter().enumerate().filter(|(_, b)| slot_class(&b.name).is_some()).map(|(i, _)| i as u32).collect();
    }
    if points.is_empty() {
        return None;
    }
    let index = (e.unit.as_deref()?.launch_index + 1) % points.len() as u32;
    let bone = Some(points[index as usize]);
    let pose = {
        let manips = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>();
        let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
        let clips = lua.app_data_ref::<crate::engine::sim::pose::Clips>();
        crate::engine::sim::pose::bone_world(e, bone, manips.as_deref(), weapons.as_deref(), clips.as_deref())
    };
    w.get_mut(carrier)?.unit.as_deref_mut()?.launch_index = index;
    Some(pose)
}

/// Whether `unit` is a carrier that launches what it holds from where it is
/// (`AIRSTAGINGPLATFORM` and `CARRIER`: moho's `CUnitCarrierLaunch`).
fn launches(lua: &Lua, unit: EntityId) -> bool {
    entity::world(lua).get(unit).and_then(|e| e.unit.as_deref()).and_then(|u| u.staging.as_deref()).is_some_and(|s| s.carrier)
}

/// Carriers launching for an unload order (their aircraft already sent to
/// guard the point), carrier to its launch.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct CarrierLaunches(BTreeMap<EntityId, Launch>);

/// One carrier's launch ([`carrier_launch`]).
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Launch {
    /// The unload order it is for.
    command: crate::engine::sim::command::CommandId,
    /// The sim tick of its next step.
    ready_at: u64,
    /// Seen under the water: it waits a tick once up.
    submerged: bool,
    /// Empty: the order ends at `ready_at`.
    finishing: bool,
}

/// Transports being loaded, with how many units have boarded since.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Loading(BTreeMap<EntityId, u32>);

/// Units rising into a transport on the beam: passenger to its beam state.
/// Moho lifts each boarder over ~9 ticks (~0.9 s, cosine-eased) between
/// `OnStartTransportBeamUp` and the attach (`CUnitCallTransport` 0x101eb661:
/// countdown 10.0 to <= 1.0 at 1.0 a tick); each unit runs its own rise
/// concurrently, so boarding overlaps with no inter-unit interval. The bone
/// is reserved from beam start (moho's `ReserveBone`), so a second boarder
/// picks another slot instead of racing for the same one.
#[derive(serde::Serialize, serde::Deserialize)]
#[derive(Default)]
pub struct Beaming(BTreeMap<EntityId, Beam>);

/// One unit's rise into a transport.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Beam {
    /// The transport rising to.
    transport: EntityId,
    /// The reserved slot.
    bone: u32,
    /// Ticks of rise left.
    left: u32,
}


/// Ticks a unit spends on the beam before attaching (moho 0x101eb661).
const BEAM_TICKS: u32 = 9;

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Loading::default());
    lua.set_app_data(Beaming::default());
    lua.set_app_data(FerryTasks::default());
    crate::engine::api::ferry::register(lua);
    lua.set_app_data(CarrierLaunches::default());
    lua_method!(lua, moho::Unit,
        /// Lets everything off: passengers and docked aircraft are detached
        /// (`OnTransportDetach`), stored units released (`OnRemoveFromStorage`).
        /// FAF's air staging calls it as it dies so docked aircraft aren't stuck.
        /// `destroySomeUnits` is ignored (FAF only passes false).
        TransportDetachAllUnits = |lua, (this, _destroy_some): (Obj<moho::Unit>, Option<bool>)| {
        let transport = entity::entity_id(&this)?;
        for unit in stored_units(lua, transport) {
            release(lua, transport, unit)?;
        }
        for unit in passengers(lua, transport) {
            detach(lua, transport, unit)?;
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Unit,
        /// The beacon of the ferry route this unit has queued first (its first
        /// order's focus unit, MohoEngine.dll 0x10287f40), or nil.
        GetTransportFerryBeacon = |lua, this: Obj<moho::Unit>| {
        let unit = entity::entity_id(&this)?;
        // (Its route's beacon, with a leg queued ahead of the ferry order.)
        let beacon = lua.app_data_ref::<Commands>().and_then(|c| c.queue(unit).find(|cmd| cmd.kind == CommandKind::Ferry)?.focus);
        match beacon {
            Some(b) => Ok(object(lua, b)?.map(Obj::<moho::Unit>::new)),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::Unit,
        /// Whether this carrier has a free storage slot.
        TransportHasAvailableStorage = |lua, this: Obj<moho::Unit>| {
        let carrier = entity::entity_id(&this)?;
        Ok((stored_units(lua, carrier).len() as u32) < storage_slots(lua, carrier))
    })?;
    lua_method!(lua, moho::Unit,
        /// Puts `unit` inside this carrier (FAF's carriers store what they build);
        /// nothing happens when it is full or already holds it.
        AddUnitToStorage = |lua, (this, unit): (Obj<moho::Unit>, Obj<moho::Unit>)| {
        let (carrier, unit) = (entity::entity_id(&this)?, entity::entity_id(&unit)?);
        let stored = stored_units(lua, carrier);
        if stored.contains(&unit) || stored.len() as u32 >= storage_slots(lua, carrier) {
            return Ok(());
        }
        store(lua, carrier, unit)
    })?;
    Ok(())
}

/// Slot size of an attach bone name, if it is one (1 small, 2 medium, 3
/// large, 4 super). Moho's `SetUpAttachPoints` (0x101d11d0) tries
/// `Attachpoint_Spr`, `Attachpoint_Lrg`, `Attachpoint_Med`, then plain
/// `Attachpoint`, in that order.
pub fn slot_class(bone: &str) -> Option<u8> {
    let b = bone.to_ascii_lowercase();
    if !b.contains("attachpoint") {
        return None;
    }
    Some(if b.contains("spr") { 4 } else if b.contains("lrg") { 3 } else if b.contains("med") { 2 } else { 1 })
}

/// The transport's `Transport.ClassGenericUpTo` (0 when unset).
pub(crate) fn generic_up_to(lua: &Lua, transport: EntityId) -> u8 {
    let bp_id = entity::world(lua).get(transport).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
    entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)
        .ok()
        .and_then(|bp| bp.get::<Option<LuaTable>>("Transport").ok().flatten())
        .and_then(|t| t.get::<Option<f64>>("ClassGenericUpTo").ok().flatten())
        .map_or(0, |n| n.clamp(0.0, 255.0) as u8)
}

/// The transport's attach bones of `class` not holding a passenger, in bone
/// order. Bones with a unit still rising to them (`Beaming`) are reserved,
/// as moho's `ReserveBone`. A class up to the transport's
/// `ClassGenericUpTo` takes any of its generic points, the bones of class
/// up to it (`TransportFindAttachList` 0x101d343f).
pub(crate) fn free_slots(lua: &Lua, transport: EntityId, class: u8) -> Vec<u32> {
    let up_to = generic_up_to(lua, transport);
    let fits = |c: u8| if class <= up_to { c <= up_to } else { c == class };
    let w = entity::world(lua);
    let bones = match w.get(transport).and_then(|t| t.cold.skeleton.as_ref()) {
        Some(sk) => &sk.bones,
        None => return Vec::new(),
    };
    let taken: Vec<u32> = w.iter().filter(|e| !e.destroyed).filter_map(|e| e.attachment.filter(|a| a.parent == transport).and_then(|a| a.parent_bone)).collect();
    let reserved: Vec<u32> = match lua.app_data_ref::<Beaming>() {
        Some(b) => b.0.values().filter(|beam| beam.transport == transport).map(|beam| beam.bone).collect(),
        None => Vec::new(),
    };
    bones
        .iter()
        .enumerate()
        .filter(|(i, b)| slot_class(&b.name).is_some_and(fits) && !taken.contains(&(*i as u32)) && !reserved.contains(&(*i as u32)))
        .map(|(i, _)| i as u32)
        .collect()
}

/// A unit's transport class (blueprint `Transport.TransportClass`, 1 when
/// unset; not clamped: classes past 4 fit nowhere).
pub(crate) fn class_of(lua: &Lua, unit: EntityId) -> u8 {
    entity::world(lua).get(unit).and_then(|e| e.unit.as_deref()).map_or(1, |u| u.transport_class)
}

/// `unit` and `transport` as the shared carry rule sees them
/// ([`crate::engine::sim::carry`]).
pub(crate) fn carry_sides(lua: &Lua, unit: EntityId, transport: EntityId) -> (crate::engine::sim::carry::Passenger, crate::engine::sim::carry::Carrier) {
    let w = entity::world(lua);
    carry_sides_in(lua, &w, unit, transport)
}

/// [`carry_sides`] with the world already borrowed.
pub(crate) fn carry_sides_in(lua: &Lua, w: &crate::engine::sim::entity::EntityWorld, unit: EntityId, transport: EntityId) -> (crate::engine::sim::carry::Passenger, crate::engine::sim::carry::Carrier) {
    use crate::engine::sim::carry::{Carrier, Passenger};
    use crate::engine::sim::movement::MotionKind;
    let bp_id = w.get(transport).map(|e| e.cold.blueprint_id.as_str()).unwrap_or_default();
    let transport_bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp_id)
        .ok()
        .and_then(|bp| bp.get::<Option<LuaTable>>("Transport").ok().flatten());
    let field = |name: &str| transport_bp.as_ref().and_then(|t| t.get::<Option<f64>>(name).ok().flatten());
    let storage = field("StorageSlots").is_some_and(|n| n > 0.0);
    let generic_up_to = field("ClassGenericUpTo").map_or(0, |n| n.clamp(0.0, 255.0) as u8);
    let domain = crate::engine::api::categories::domain(lua).ok();
    let is = |id: EntityId, name: &str| {
        let index = w.get(id).and_then(|e| e.category_index);
        domain.as_ref().and_then(|d| d.named.get(name)).zip(index).is_some_and(|(s, i)| s.contains(i))
    };
    let motion = w.get(unit).and_then(|e| e.unit.as_deref()).map(|u| u.motion_params.kind);
    let passenger = Passenger {
        mobile: motion.is_some_and(|k| k != MotionKind::None),
        air: motion == Some(MotionKind::Air),
        commander: is(unit, "COMMAND"),
        class: w.get(unit).and_then(|e| e.unit.as_deref()).map_or(1, |u| u.transport_class),
    };
    let slot_classes = w
        .get(transport)
        .and_then(|t| t.cold.skeleton.as_ref())
        .map_or(0, |sk| sk.bones.iter().filter_map(|b| slot_class(&b.name)).fold(0u8, |acc, c| acc | 1 << (c - 1)));
    let transport_caps = w.get(transport).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.blueprint_command_caps & 0x100 != 0);
    let carrier = Carrier {
        transport: transport_caps || is(transport, "PODSTAGINGPLATFORM"),
        staging: is(transport, "AIRSTAGINGPLATFORM"), carries_commander: is(transport, "CANTRANSPORTCOMMANDER"), slot_classes, generic_up_to, storage };
    (passenger, carrier)
}

/// `transport` has a free slot `unit` fits (`TransportHasSpaceFor`).
pub fn has_space_for(lua: &Lua, transport: EntityId, unit: EntityId) -> bool {
    !free_slots(lua, transport, class_of(lua, unit)).is_empty()
}

pub(crate) fn set_state(lua: &Lua, unit: EntityId, name: &str, on: bool) {
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.states.set(UnitStates::index(name).expect("unit state"), on);
    }
}

fn pop(lua: &Lua, unit: EntityId) {
    if let Some(mut c) = lua.app_data_mut::<Commands>() {
        c.pop_front(unit);
    }
}

fn xz_dist2(a: FxVec3, b: FxVec3) -> Fx {
    let (dx, dz) = (a.x - b.x, a.z - b.z);
    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
}

/// How close (XZ) a unit must get to board: both radii plus a margin.
pub fn reach(lua: &Lua, transport: EntityId, unit: EntityId) -> Fx {
    let w = entity::world(lua);
    let r = |id| w.get(id).and_then(|e| e.unit.as_deref()).map_or(Fx::ONE, |u| u.radius);
    r(transport) + r(unit) + Fx::from_num(3)
}

fn is_staging(lua: &Lua, unit: EntityId) -> bool {
    let index = entity::world(lua).get(unit).and_then(|e| e.category_index);
    match (crate::engine::api::categories::domain(lua).ok(), index) {
        (Some(d), Some(i)) => d.named.get("AIRSTAGINGPLATFORM").is_some_and(|s| s.contains(i)),
        _ => false,
    }
}

/// Attaches `unit` to `transport` at `bone` and runs the callbacks.
/// Externally carried cargo hangs below the attach point: the child's origin
/// sits its collision-box top under the bone (stock slings units underneath
/// the airframe). Air-staging docks keep `hang` false so aircraft sit on the
/// pad; carrier storage never reaches here.
fn attach(lua: &Lua, transport: EntityId, unit: EntityId, bone: u32, hang: bool) -> LuaResult<()> {
    {
        let mut w = entity::world(lua);
        w.attach(unit, Attachment { parent: transport, parent_bone: Some(bone), self_bone: None });
        if hang {
            use crate::engine::sim::entity::CollisionShape as S;
            let top = match w.get(unit).map(|e| e.collision_shape()) {
                Some(S::Box { center, half }) => center.y + half.y,
                Some(S::Sphere { center, radius }) => center.y + radius,
                _ => Fx::ZERO,
            };
            w.set_parent_offset(unit, FxVec3 { x: Fx::ZERO, y: -top.max(Fx::ZERO), z: Fx::ZERO });
        } else {
            w.set_parent_offset(unit, FxVec3::default());
        }
    }
    set_state(lua, unit, "Attached", true);
    // Passengers ride in the air (FAF's drop logic waits for the change
    // back on landing).
    if !is_staging(lua, transport) {
        crate::engine::api::movement::set_layer(lua, unit, crate::engine::sim::layer::Layer::Air)?;
    }
    if is_staging(lua, transport) {
        set_state(lua, unit, "Refueling", true);
    }
    let name = bone_name(lua, transport, bone);
    if let (Some(t), Some(u)) = (object(lua, transport)?, object(lua, unit)?) {
        UnitCallbacks::on_transport_attach(lua, &t, "unit", name, Obj::new(u))?;
    }
    Ok(())
}

/// An aircraft leaving air staging: its service stops and its repair
/// request is dropped (moho, once it's airborne again).
fn end_service(lua: &Lua, unit: EntityId) {
    let request = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()).and_then(|u| {
        u.refueling = false;
        u.staging_repair.take()
    });
    if let (Some(id), Some(mut events)) = (request, lua.app_data_mut::<crate::engine::sim::economy::EconomyEvents>()) {
        events.remove(id);
    }
}

fn bone_name(lua: &Lua, entity: EntityId, bone: u32) -> String {
    entity::world(lua).get(entity).and_then(|e| e.cold.skeleton.as_ref().and_then(|s| s.bones.get(bone as usize).map(|b| b.name.clone()))).unwrap_or_default()
}

/// Detaches `unit` from `transport`, sets it on the surface below, and runs
/// the callbacks.
fn detach(lua: &Lua, transport: EntityId, unit: EntityId) -> LuaResult<()> {
    end_service(lua, unit);
    let bone = {
        let mut w = entity::world(lua);
        let Some(e) = w.get_mut(unit) else { return Ok(()) };
        let bone = e.attachment.and_then(|a| a.parent_bone);
        e.attachment = None;
        let p = e.position();
        let air = e.unit.as_deref().is_some_and(|u| u.motion_params.kind == crate::engine::sim::movement::MotionKind::Air);
        drop(w);
        let y = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map_or(p.y, |t| t.surface_at(p.x, p.z));
        // Aircraft leave a platform at its height and fly off; ground units
        // are put down.
        if !air {
            entity::world(lua).set_position(unit, FxVec3 { y, ..p });
        }
        bone
    };
    set_state(lua, unit, "Attached", false);
    set_state(lua, unit, "Refueling", false);
    let name = bone.map(|b| bone_name(lua, transport, b)).unwrap_or_default();
    if let (Some(t), Some(u)) = (object(lua, transport)?, object(lua, unit)?) {
        UnitCallbacks::on_transport_detach(lua, &t, "unit", name, Obj::new(u))?;
    }
    // Set down: back to the layer of where it landed (after the detach
    // callbacks, which expect the unit still airborne).
    let (kind, offset, p) = match entity::world(lua).get(unit) {
        Some(e) => (e.unit.as_deref().map(|u| u.motion_params.kind).unwrap_or_default(), e.unit.as_deref().map_or(Fx::ZERO, |u| u.motion_params.layer_change_offset), e.position()),
        None => return Ok(()),
    };
    let wet = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().is_some_and(|t| t.water_surface.is_some_and(|w| crate::engine::sim::layer::wet(kind, t.height_at(p.x, p.z), w, offset)));
    crate::engine::api::movement::set_layer(lua, unit, crate::engine::sim::layer::Layer::for_motion(kind, wet))?;
    Ok(())
}

/// A transport's real unit passengers, id order. Plain `AttachTo` children
/// are not cargo: the Fatboy's external-factory anchor `Entity` and its
/// `ZXA0003` dummy have no `OnDetachedFromTransport`.
pub(crate) fn passengers(lua: &Lua, transport: EntityId) -> Vec<EntityId> {
    let w = entity::world(lua);
    w.attached_to(transport).into_iter().filter(|&c| w.get(c).is_some_and(|e| !e.destroyed && e.unit.is_some())).collect()
}

/// A `SurfacingSub` under the water: moho's carrier and unload tasks surface
/// it before taking units in or letting them out.
fn submerged(lua: &Lua, unit: EntityId) -> bool {
    entity::world(lua)
        .get(unit)
        .and_then(|e| e.unit.as_deref())
        .is_some_and(|u| u.motion_params.surfacing_sub && u.layer == crate::engine::sim::layer::Layer::Sub)
}

/// Submerged `SurfacingSub`s that a transport task surfaces, asked for
/// before movement (the order pass, as moho's command tasks tick), so the
/// surfacing starts on the tick a `Dive` order would: the real game's
/// carrier first rises on the second tick after its load order
/// (`dev/probes/maps/probe_carrier`).
pub fn surface_tasks(lua: &Lua) {
    surface_carriers(lua);
    // Unloading (`CUnitUnloadUnits`, MohoEngine.dll 0x10210162): with
    // something to unload, surface before setting off.
    use CommandKind as K;
    let unloading: Vec<(EntityId, Option<crate::engine::sim::category::CategorySet>)> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return };
        c.units_doing(&[K::TransportUnloadUnits, K::TransportUnloadSpecificUnits])
            .into_iter()
            .filter_map(|u| c.current(u).map(|cmd| (u, if cmd.kind == K::TransportUnloadSpecificUnits { cmd.category.clone() } else { None })))
            .collect()
    };
    for (unit, filter) in unloading {
        if submerged(lua, unit) && passengers(lua, unit).into_iter().any(|c| in_category(lua, c, filter.as_ref())) {
            crate::engine::api::movement::request_surface(lua, unit);
        }
    }
}

/// Submerged carriers whose load orders are under way surface (moho's
/// `CUnitCarrierRetrieve`, MohoEngine.dll 0x101f14a3..0x101f1571: once every
/// unit of the load command has it as its current order, a `SurfacingSub`
/// carrier in the `Sub` layer calls `SetNewTargetLayer(Water)`; the units
/// only go in once it's out of the `Sub` layer, 0x101f1464).
fn surface_carriers(lua: &Lua) {
    let carriers: Vec<EntityId> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return };
        // Each load command once (its units share it).
        let mut loads: Vec<(crate::engine::sim::command::CommandId, EntityId)> = c
            .units_doing(&[CommandKind::TransportLoadUnits])
            .into_iter()
            .filter_map(|u| c.current(u))
            .filter_map(|cmd| match cmd.target {
                CommandTarget::Entity(t) => Some((cmd.id, t)),
                _ => None,
            })
            .collect();
        loads.sort_unstable();
        loads.dedup();
        let mut out: Vec<EntityId> = loads
            .into_iter()
            .filter(|&(id, _)| c.get(id).is_some_and(|cmd| cmd.units.iter().all(|&u| c.current(u).is_some_and(|cur| cur.id == id))))
            .map(|(_, carrier)| carrier)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };
    for carrier in carriers {
        if storage_slots(lua, carrier) > 0 && submerged(lua, carrier) {
            crate::engine::api::movement::request_surface(lua, carrier);
        }
    }
}

/// Transports (not staging platforms) that units are currently ordered
/// aboard, ascending: load orders name the transport as their target, while
/// a reverse-load order sits on the transport itself.
fn ordered_transports(lua: &Lua) -> Vec<EntityId> {
    let mut out: Vec<EntityId> = match lua.app_data_ref::<Commands>() {
        Some(c) => c
            .units_doing(&[CommandKind::TransportLoadUnits, CommandKind::TransportReverseLoadUnits])
            .into_iter()
            .filter_map(|u| c.current(u).map(|c| (u, c.kind, c.target)))
            .filter_map(|(u, kind, target)| match (kind, target) {
                (CommandKind::TransportReverseLoadUnits, _) => Some(u),
                (_, CommandTarget::Entity(t)) => Some(t),
                _ => None,
            })
            .collect(),
        None => return Vec::new(),
    };
    out.sort_unstable();
    out.dedup();
    out.retain(|&t| entity::world(lua).get(t).is_some_and(|e| !e.destroyed) && !is_staging(lua, t));
    out
}

/// Starts loading on transports newly ordered to take units.
fn begin_loading(lua: &Lua) -> LuaResult<()> {
    for transport in ordered_transports(lua) {
        if lua.app_data_ref::<Loading>().is_none_or(|l| l.0.contains_key(&transport)) {
            continue;
        }
        lua.app_data_mut::<Loading>().expect("Loading").0.insert(transport, 0);
        set_state(lua, transport, "TransportLoading", true);
        if let Some(t) = object(lua, transport)? {
            UnitCallbacks::on_transport_ordered(lua, &t, "unit")?;
            UnitCallbacks::on_start_transport_loading(lua, &t, "unit")?;
        }
    }
    Ok(())
}

/// Ends loading on transports with no load orders left.
fn end_loading(lua: &Lua) -> LuaResult<()> {
    let still = ordered_transports(lua);
    let done: Vec<(EntityId, u32)> = match lua.app_data_ref::<Loading>() {
        Some(l) => l.0.iter().filter(|(t, _)| still.binary_search(t).is_err()).map(|(t, n)| (*t, *n)).collect(),
        None => return Ok(()),
    };
    for (transport, boarded) in done {
        lua.app_data_mut::<Loading>().expect("Loading").0.remove(&transport);
        if entity::world(lua).get(transport).is_none_or(|e| e.destroyed) {
            continue;
        }
        set_state(lua, transport, "TransportLoading", false);
        if let Some(t) = object(lua, transport)? {
            if boarded == 0 {
                UnitCallbacks::on_transport_aborted(lua, &t, "unit")?;
            }
            UnitCallbacks::on_stop_transport_loading(lua, &t, "unit")?;
        }
    }
    Ok(())
}

/// `unit` boards `transport` at `bone`: the beam picks it up (`start_beam`;
/// the attach lands `BEAM_TICKS` ticks later).
/// `reverse`: the transport came to pick it up (its own reverse-load
/// order), so the passenger holds no load order of its own; else FAF's
/// refusal (slot taken in its own bookkeeping) is read off the passenger
/// still holding its load order.
fn board(lua: &Lua, transport: EntityId, unit: EntityId, bone: u32, reverse: bool) -> LuaResult<()> {
    if is_staging(lua, transport) {
        return attach(lua, transport, unit, bone, false);
    }
    start_beam(lua, transport, unit, bone, reverse)
}

/// Starts `unit` rising into `transport` at `bone`: beam effects on, and the
/// attach lands `BEAM_TICKS` ticks later (`beam_tick`). The boarder holds
/// still while it rises (moho's mover owns its motion; `Immobile`, cleared
/// at the attach or the abort), keeping its load order until then.
fn start_beam(lua: &Lua, transport: EntityId, unit: EntityId, bone: u32, reverse: bool) -> LuaResult<()> {
    let (t, u) = (object(lua, transport)?, object(lua, unit)?);
    if let (Some(t), Some(u)) = (&t, &u) {
        UnitCallbacks::on_start_transport_beam_up(lua, u, "unit", Obj::new(t.clone()), bone_name(lua, transport, bone))?;
    }
    // FAF refuses (slot taken in its own bookkeeping) by clearing the
    // unit's orders.
    let still_ordered = reverse || lua.app_data_ref::<Commands>().and_then(|c| c.current(unit).map(|c| c.kind)) == Some(CommandKind::TransportLoadUnits);
    if !still_ordered || entity::world(lua).get(unit).is_none_or(|e| e.destroyed) {
        return Ok(());
    }
    set_state(lua, unit, "Immobile", true);
    if let Some(mut beaming) = lua.app_data_mut::<Beaming>() {
        beaming.0.insert(unit, Beam { transport, bone, left: BEAM_TICKS });
    }
    Ok(())
}

/// Advances units on the beam: each rises toward its slot (cosine-eased, as
/// moho 0x101eb661), attaching when its countdown ends. The load order that
/// boarded it ends at the attach; a boarder whose transport died, whose slot
/// filled, or whose orders went away aborts (beam effects off, `Immobile`
/// cleared) and its surviving load order is left for `load_aboard` to
/// re-resolve next tick.
fn beam_tick(lua: &Lua) -> LuaResult<()> {
    let rising: Vec<(EntityId, Beam)> = match lua.app_data_ref::<Beaming>() {
        Some(b) => b.0.iter().map(|(u, beam)| (*u, *beam)).collect(),
        None => return Ok(()),
    };
    for (unit, beam) in rising {
        let Some(now) = lua.app_data_ref::<Beaming>().and_then(|b| b.0.get(&unit).copied()) else { continue };
        let gone = |id| entity::world(lua).get(id).is_none_or(|e| e.destroyed);
        // The slot filled underneath (or the transport died): abort and let
        // the load order pick another slot.
        let slot_free = !gone(beam.transport)
            && entity::world(lua).iter().filter(|e| !e.destroyed).filter_map(|e| e.attachment.filter(|a| a.parent == beam.transport).and_then(|a| a.parent_bone)).all(|b| b != beam.bone);
        if gone(unit) || !slot_free {
            stop_beam(lua, unit);
            continue;
        }
        if now.left == 0 {
            // (Refusal already checked at beam start; FAF may still have
            // cleared the orders mid-rise, in which case the attach still
            // lands but no order ends.)
            attach(lua, beam.transport, unit, beam.bone, true)?;
            set_state(lua, unit, "Immobile", false);
            // (`Loading` is always registered with the transport system;
            // benign without it, e.g. a bench driving `tick` on a bare state.)
            if let Some(mut loading) = lua.app_data_mut::<Loading>()
                && let Some(n) = loading.0.get_mut(&beam.transport) {
                *n += 1;
            }
            if let Some(u) = object(lua, unit)? {
                UnitCallbacks::on_stop_transport_beam_up(lua, &u, "unit")?;
            }
            // (The load order that boarded it ends in the order pass below:
            // the passenger reads attached, or the reverse-load order's
            // target does.)
            if let Some(mut beaming) = lua.app_data_mut::<Beaming>() {
                beaming.0.remove(&unit);
            }
            continue;
        }
        // Rise: cosine ease from the ground point to the slot over the
        // countdown (moho 0x101eb661: `0.5*cos(pi*0.1*t) + 0.5` with t
        // down from 10; here the ease runs 0 at the ground to 1 at the
        // slot). One world borrow at a time.
        let from = entity::world(lua).get(unit).map(|e| e.position());
        let to = slot_world(lua, beam.transport, beam.bone);
        let (Some(from), Some(to)) = (from, to) else {
            stop_beam(lua, unit);
            continue;
        };
        let frac = Fx::from_num(now.left) / Fx::from_num(BEAM_TICKS);
        let (_, cos) = crate::engine::sim::math::sin_cos(frac * Fx::from_num(std::f64::consts::PI));
        // 0 at the ground rising to 1 at the slot, as the countdown runs
        // down (moho 0x101eb661).
        let ease = Fx::from_num(0.5) * cos + Fx::from_num(0.5);
        let at = FxVec3 {
            x: from.x + (to.x - from.x) * ease,
            y: from.y + (to.y - from.y) * ease,
            z: from.z + (to.z - from.z) * ease,
        };
        entity::world(lua).set_position(unit, at);
        if let Some(mut beaming) = lua.app_data_mut::<Beaming>()
            && let Some(entry) = beaming.0.get_mut(&unit) {
            entry.left = now.left - 1;
        }
    }
    Ok(())
}

/// Beam effects off, rise cancelled, the hold released (the unit keeps its
/// orders and walks back into reach).
fn stop_beam(lua: &Lua, unit: EntityId) {
    if let Some(mut beaming) = lua.app_data_mut::<Beaming>() {
        beaming.0.remove(&unit);
    }
    set_state(lua, unit, "Immobile", false);
    if let Ok(Some(u)) = object(lua, unit) {
        let _ = UnitCallbacks::on_stop_transport_beam_up(lua, &u, "unit");
    }
}

/// World position of `transport`'s attach `bone` in its rest pose (see
/// `Entity::bone_world_position`; posed bones don't move transports).
fn slot_world(lua: &Lua, transport: EntityId, bone: u32) -> Option<FxVec3> {
    entity::world(lua).get(transport).map(|e| e.bone_world_position(Some(bone)))
}

/// The transport's brain hears it has no room (`OnTransportFull`).
fn transport_full(lua: &Lua, transport: EntityId) -> LuaResult<()> {
    let Some(army) = entity::world(lua).get(transport).and_then(|e| e.army) else { return Ok(()) };
    if let LuaValue::Table(b) = crate::engine::api::army::brain(lua, army)? {
        AIBrainCallbacks::on_transport_full(lua, &b, "brain")?;
    }
    Ok(())
}

/// How a boarding attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boarding {
    /// Not there yet (or nothing to board any more): the order stays.
    Waiting,
    /// Aboard (or the order went away underneath): the order is done.
    Done,
    /// No room: the order is done (and the brain heard `OnTransportFull`).
    Full,
}

/// Whether `unit` (at `pos`) has lowered to its `TransportHoverHeight`
/// above the ground — moho holds at THH for the tractor / drop
/// (`ShouldHoverInsteadOfLand`, 0x1029b210). THH 0 = no dip.
fn at_hover_height(lua: &Lua, unit: EntityId, pos: FxVec3) -> bool {
    let thh = entity::world(lua).get(unit).and_then(|e| e.unit.as_deref()).map_or(Fx::ZERO, |u| u.motion_params.air.transport_hover_height);
    if thh <= Fx::ZERO {
        return true;
    }
    let ground = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map_or(pos.y, |t| t.surface_at(pos.x, pos.z));
    pos.y - ground <= thh + Fx::from_num(0.5)
}

/// Boards `unit` onto `transport` once in reach (shared by load orders,
/// which the passenger owns, and reverse-load orders, which the transport
/// owns): carriers take it inside, others onto a fitting attach slot.
/// `reverse`: the passenger holds no load order of its own, and its queue
/// is left alone (it resumes it on detach); the caller pops the
/// transport's order itself from the outcome. Else pops `unit`'s own order
/// (id `id`) when done, unless FAF already cleared it.
fn load_aboard(lua: &Lua, transport: EntityId, unit: EntityId, id: crate::engine::sim::command::CommandId, reverse: bool) -> LuaResult<Boarding> {
    // (One world borrow at a time: a second `world()` while one lives is a
    // `BorrowMutError`.)
    let pos = entity::world(lua).get(unit).filter(|e| !e.destroyed).map(|e| e.position());
    let tpos = entity::world(lua).get(transport).filter(|e| !e.destroyed).map(|e| e.position());
    let done = |lua: &Lua| {
        if !reverse && lua.app_data_ref::<Commands>().and_then(|c| c.current(unit).map(|c| c.id)) == Some(id) {
            pop(lua, unit);
        }
    };
    let (Some(pos), Some(tpos)) = (pos, tpos) else {
        done(lua);
        return Ok(Boarding::Done);
    };
    if entity::world(lua).get(unit).is_some_and(|e| e.attachment.is_some()) {
        done(lua);
        return Ok(Boarding::Done);
    }
    // Already rising on the beam: the order stays until the attach lands
    // (`beam_tick` ends it).
    if lua.app_data_ref::<Beaming>().is_some_and(|b| b.0.contains_key(&unit)) {
        return Ok(Boarding::Waiting);
    }
    let r = reach(lua, transport, unit);
    if xz_dist2(pos, tpos) > r.saturating_mul(r) {
        return Ok(Boarding::Waiting);
    }
    // Reverse load: the transport lowers to its hover height before the
    // tractor starts (moho holds at THH, `ShouldHoverInsteadOfLand`).
    if reverse && !at_hover_height(lua, transport, tpos) {
        return Ok(Boarding::Waiting);
    }
    // One it can't carry (the shared rule, as the order check; asked once
    // in reach, not every tick of the walk): over.
    let (passenger, carrier) = carry_sides(lua, unit, transport);
    if !crate::engine::sim::carry::can_carry(&passenger, &carrier) {
        done(lua);
        return Ok(Boarding::Done);
    }
    // Carriers take it inside while there's room, once surfaced.
    let slots = storage_slots(lua, transport);
    if slots > 0 {
        if submerged(lua, transport) {
            return Ok(Boarding::Waiting);
        }
        if (stored_units(lua, transport).len() as u32) < slots {
            store(lua, transport, unit)?;
            done(lua);
            return Ok(Boarding::Done);
        }
        transport_full(lua, transport)?;
        done(lua);
        return Ok(Boarding::Full);
    }
    match free_slots(lua, transport, class_of(lua, unit)).first().copied() {
        // Boards over the next ticks: the order stays until the attach
        // lands (or FAF clears it, refusing the slot).
        Some(bone) => {
            board(lua, transport, unit, bone, reverse)?;
            if lua.app_data_ref::<Beaming>().is_none_or(|b| !b.0.contains_key(&unit)) {
                // Refused (or died) on the beam: the order ends.
                done(lua);
                return Ok(Boarding::Done);
            }
            Ok(Boarding::Waiting)
        }
        None => {
            tracing::debug!(unit, transport, "transport full; load order dropped");
            transport_full(lua, transport)?;
            done(lua);
            Ok(Boarding::Full)
        }
    }
}

/// Whether `filter` keeps `passenger` (`None` keeps everything).
fn in_category(lua: &Lua, passenger: EntityId, filter: Option<&crate::engine::sim::category::CategorySet>) -> bool {
    filter.is_none_or(|f| entity::world(lua).get(passenger).and_then(|e| e.category_index).is_some_and(|i| f.contains(i)))
}

/// Puts ashore the passengers of `unit` that `filter` keeps (id order):
/// stored ones released, the rest detached. `None` keeps all (a plain
/// unload); unload-specific passes its command's category.
fn unload_at(lua: &Lua, unit: EntityId, filter: Option<&crate::engine::sim::category::CategorySet>) -> LuaResult<()> {
    set_state(lua, unit, "TransportUnloading", true);
    for passenger in passengers(lua, unit) {
        if !in_category(lua, passenger, filter) {
            continue;
        }
        if entity::world(lua).get(passenger).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.stored_in == Some(unit)) {
            release(lua, unit, passenger)?;
        } else {
            detach(lua, unit, passenger)?;
        }
    }
    set_state(lua, unit, "TransportUnloading", false);
    pop(lua, unit);
    Ok(())
}

/// Ticks between a carrier's launch steps (the real game: probe_launch).
const LAUNCH_INTERVAL: u64 = 2;

/// One tick of a carrier's launch (moho's `CUnitCarrierLaunch`), from where
/// the carrier is, as the real game does it (`dev/probes/maps/probe_launch`):
/// - the first tick it reads `TransportUnloading` and the carried set (that
///   `filter` keeps) is ordered to guard the unload point, queues cleared,
///   under the water or not;
/// - a submerged `SurfacingSub` waits until it is up (`surface_tasks`
///   surfaces it), then a tick more;
/// - from two ticks after the start, one leaves every second tick, id
///   order, through the next launch point ([`release`]);
/// - empty, the order ends four ticks after the last one left.
///
/// An order with nothing aboard at the start ends at once (the caller).
fn carrier_launch(lua: &Lua, carrier: EntityId, id: crate::engine::sim::command::CommandId, at: FxVec3, filter: Option<&crate::engine::sim::category::CategorySet>) -> LuaResult<()> {
    let now = crate::engine::api::sim_time::current_tick(lua);
    let aboard: Vec<EntityId> = passengers(lua, carrier).into_iter().filter(|&c| in_category(lua, c, filter)).collect();
    let launch = lua.app_data_ref::<CarrierLaunches>().and_then(|l| l.0.get(&carrier).copied()).filter(|l| l.command == id);
    let Some(mut launch) = launch else {
        set_state(lua, carrier, "TransportUnloading", true);
        if let Some(mut c) = lua.app_data_mut::<Commands>() {
            for &u in &aboard {
                c.clear(u);
            }
        }
        crate::engine::api::command::issue(lua, &aboard, CommandKind::Guard, CommandTarget::Ground(at), None, 0);
        if let Some(mut l) = lua.app_data_mut::<CarrierLaunches>() {
            l.0.insert(carrier, Launch { command: id, ready_at: now + LAUNCH_INTERVAL, submerged: false, finishing: false });
        }
        return Ok(());
    };
    if submerged(lua, carrier) {
        launch.submerged = true;
    } else if launch.submerged {
        launch.submerged = false;
        launch.ready_at = launch.ready_at.max(now + 1);
    } else if now >= launch.ready_at {
        launch.ready_at = now + LAUNCH_INTERVAL;
        if let Some(&unit) = aboard.first() {
            if entity::world(lua).get(unit).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.stored_in == Some(carrier)) {
                release(lua, carrier, unit)?;
            } else {
                detach(lua, carrier, unit)?;
            }
        } else if !launch.finishing {
            launch.finishing = true;
        } else {
            set_state(lua, carrier, "TransportUnloading", false);
            if let Some(mut l) = lua.app_data_mut::<CarrierLaunches>() {
                l.0.remove(&carrier);
            }
            pop(lua, carrier);
            return Ok(());
        }
    }
    if let Some(mut l) = lua.app_data_mut::<CarrierLaunches>() {
        l.0.insert(carrier, launch);
    }
    Ok(())
}

pub fn tick(lua: &Lua) -> LuaResult<()> {
    use CommandKind as K;
    // Launches whose order is gone (stopped, the carrier died) are over.
    let ended: Vec<EntityId> = match (lua.app_data_ref::<Commands>(), lua.app_data_mut::<CarrierLaunches>()) {
        (Some(c), Some(mut l)) => {
            let ended = l.0.iter().filter(|&(&carrier, launch)| c.current(carrier).is_none_or(|cmd| cmd.id != launch.command)).map(|(&carrier, _)| carrier).collect();
            l.0.retain(|&carrier, launch| c.current(carrier).is_some_and(|cmd| cmd.id == launch.command));
            ended
        }
        _ => Vec::new(),
    };
    for carrier in ended {
        set_state(lua, carrier, "TransportUnloading", false);
    }
    begin_loading(lua)?;
    ferry_tasks(lua)?;
    crate::engine::api::ferry::waits(lua);
    crate::engine::api::ferry::routes(lua)?;
    ferry_states(lua);
    // Rises in progress first: finishes attach before the order pass reads
    // them (the load order that boarded each one ends on the attach).
    beam_tick(lua)?;
    let orders: Vec<(EntityId, CommandKind, CommandTarget, Option<crate::engine::sim::category::CategorySet>, crate::engine::sim::command::CommandId)> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(()) };
        c.units_doing(&[K::TransportLoadUnits, K::TransportReverseLoadUnits, K::TransportUnloadUnits, K::TransportUnloadSpecificUnits, K::DetachFromTransport])
            .into_iter()
            .filter_map(|u| c.current(u).map(|c| (u, c.kind, c.target, c.category.clone(), c.id)))
            .collect()
    };
    let waiting = crate::engine::api::ferry::beacon_orders(lua);
    for (unit, kind, target, category, id) in orders {
        let Some(pos) = entity::world(lua).get(unit).filter(|e| !e.destroyed).map(|e| e.position()) else { continue };
        match (kind, target) {
            // A load onto a ferry beacon waits there (`ferry::waits`).
            (K::TransportLoadUnits, CommandTarget::Entity(_)) if waiting.binary_search(&(unit, id)).is_ok() => {}
            // The passenger walks to the transport (`movement`); aboard once
            // in reach.
            (K::TransportLoadUnits, CommandTarget::Entity(transport)) => {
                load_aboard(lua, transport, unit, id, false)?;
            }
            // The transport flies out to the passenger (`movement`); it
            // boards once the transport is in reach. The transport owns
            // this order, so it ends on the outcome: aboard, gone, or no
            // room (else it would wait forever, crying full every tick).
            (K::TransportReverseLoadUnits, CommandTarget::Entity(passenger)) => {
                if entity::world(lua).get(passenger).is_none_or(|e| e.destroyed || e.attachment.is_some()) {
                    pop(lua, unit);
                    continue;
                }
                if load_aboard(lua, unit, passenger, id, true)? != Boarding::Waiting {
                    pop(lua, unit);
                }
            }
            // Moho's unload task (`CUnitUnloadUnits`, MohoEngine.dll
            // 0x102100f0) ends at once with nothing aboard (none of the
            // category, unloading specific units); a submerged
            // `SurfacingSub` surfaces before setting off (`surface_tasks`;
            // the movement waits for it).
            (K::TransportUnloadUnits | K::TransportUnloadSpecificUnits, CommandTarget::Ground(p)) => {
                let filter = if kind == K::TransportUnloadSpecificUnits { category.as_ref() } else { None };
                let launching = launches(lua, unit) && lua.app_data_ref::<CarrierLaunches>().is_some_and(|l| l.0.get(&unit).is_some_and(|l| l.command == id));
                if !launching && !passengers(lua, unit).into_iter().any(|c| in_category(lua, c, filter)) {
                    pop(lua, unit);
                    continue;
                }
                if launches(lua, unit) {
                    carrier_launch(lua, unit, id, p, filter)?;
                    continue;
                }
                if submerged(lua, unit) {
                    continue;
                }
                if xz_dist2(pos, p) > Fx::from_num(9) || !at_hover_height(lua, unit, pos) {
                    continue;
                }
                unload_at(lua, unit, filter)?;
            }
            // Bails out of the transport mid-flight (ordered on the
            // passenger); on the ground it is already over.
            (K::DetachFromTransport, _) => {
                // (Owned before detaching: the world borrow must end
                // before `detach` takes it.)
                let parent = entity::world(lua).get(unit).and_then(|e| e.attachment.map(|a| a.parent));
                if let Some(transport) = parent {
                    detach(lua, transport, unit)?;
                }
                pop(lua, unit);
            }
            _ => pop(lua, unit),
        }
    }
    end_loading(lua)?;
    staging(lua)
}

/// Transports flying a ferry route read `Ferrying` (cleared when the route
/// ends); the state pass only runs the delta, in id order.
fn ferry_states(lua: &Lua) {
    use CommandKind as K;
    // A current ferry order, or a route running (its legs queued ahead).
    let ferrying: Vec<EntityId> = match lua.app_data_ref::<Commands>() {
        Some(c) => {
            let mut v = c.units_doing(&[K::Ferry]);
            if let Some(r) = lua.app_data_ref::<crate::engine::api::ferry::FerryRoutes>() {
                v.extend(r.0.keys().copied());
            }
            v.sort_unstable();
            v.dedup();
            v
        }
        None => return,
    };
    let index = match UnitStates::index("Ferrying") {
        Some(i) => i,
        None => return,
    };
    let mut world = entity::world(lua);
    let mut was: Vec<EntityId> = Vec::new();
    for e in world.iter() {
        if e.unit.as_deref().is_some_and(|u| u.states.contains(index)) {
            was.push(e.id);
        }
    }
    for id in &ferrying {
        if was.binary_search(id).is_err()
            && let Some(u) = world.get_mut(*id).and_then(|e| e.unit.as_deref_mut()) {
            u.states.set(index, true);
        }
    }
    for id in &was {
        if ferrying.binary_search(id).is_err()
            && let Some(u) = world.get_mut(*id).and_then(|e| e.unit.as_deref_mut()) {
            u.states.set(index, false);
        }
    }
}

/// Which ferry order each ferrying transport has started (its task made).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct FerryTasks(std::collections::BTreeMap<EntityId, crate::engine::sim::command::CommandId>);

/// Moho's ferry task (`CUnitFerryTask` ctor, MohoEngine.dll 0x101f8b20), as
/// a transport's current order becomes a ferry order: the order's beacon
/// (its focus unit) if it has one; else, if the transport's `AI.BeaconName`
/// names a unit blueprint, that unit for its army at the order's point
/// (identity orientation; `CUnitCommand::CreateCommandFocusUnit` 0x102c36f0)
/// becomes it and the transport hears `OnFerryPointSet`. So one shared ferry
/// order has one beacon, made by its first transport. Orders done take their
/// beacons with them (`~CUnitCommand` 0x102c34e0). In unit id order.
fn ferry_tasks(lua: &Lua) -> LuaResult<()> {
    use CommandKind as K;
    let dropped = lua.app_data_mut::<Commands>().map(|mut c| c.take_dropped_focus()).unwrap_or_default();
    for beacon in dropped {
        if let Some(obj) = entity::object(lua, beacon)?
            && !obj.call_method::<bool>("BeenDestroyed", ())? {
            obj.call_method::<()>("Destroy", ())?;
        }
    }
    let starting: Vec<(EntityId, crate::engine::sim::command::CommandId, Option<EntityId>, CommandTarget)> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(()) };
        let ferrying = c.units_doing(&[K::Ferry]);
        let Some(mut tasks) = lua.app_data_mut::<FerryTasks>() else { return Ok(()) };
        tasks.0.retain(|u, _| ferrying.binary_search(u).is_ok());
        ferrying
            .iter()
            .filter_map(|&u| {
                let cmd = c.current(u)?;
                (tasks.0.insert(u, cmd.id) != Some(cmd.id)).then_some((u, cmd.id, cmd.focus, cmd.target))
            })
            .collect()
    };
    for (unit, id, focus, target) in starting {
        // The order's beacon already: the task links it, nothing more. (A
        // beacon made earlier this pass for the same order counts.)
        if focus.is_some() || lua.app_data_ref::<Commands>().and_then(|c| c.get(id)?.focus).is_some() {
            continue;
        }
        let (army, bp_id) = {
            let w = entity::world(lua);
            let Some((army, e)) = w.get(unit).and_then(|e| Some((e.army?, e))) else { continue };
            (army, e.cold.blueprint_id.clone())
        };
        let bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)?;
        let beacon = bp.get::<Option<LuaTable>>("AI")?.map(|a| a.get::<Option<String>>("BeaconName")).transpose()?.flatten().map(|n| n.to_lowercase());
        let Some(beacon) = beacon.filter(|n| !n.is_empty() && entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, n).is_ok()) else { continue };
        let at = match target {
            CommandTarget::Ground(p) => p,
            CommandTarget::Entity(e) => match entity::world(lua).get(e) {
                Some(e) => e.position(),
                None => continue,
            },
            _ => continue,
        };
        let made = crate::engine::api::unit::spawn_unit(lua, &beacon, army, at, crate::engine::sim::math::FxQuat::IDENTITY)?;
        let made = entity::entity_id(&made)?;
        if let Some(mut c) = lua.app_data_mut::<Commands>()
            && let Some(cmd) = c.get_mut(id) {
            cmd.focus = Some(made);
        }
        if let Some(obj) = entity::object(lua, unit)? {
            UnitCallbacks::on_ferry_point_set(lua, &Obj::<moho::Unit>::new(obj), &bp_id)?;
        }
    }
    Ok(())
}

/// Docked aircraft: moho's docked branch of the fuel tick
/// (`CUnitMotion::ProcessFuelLevels`, MohoEngine.dll 0x102982f0), and
/// leaving once given another order. Per tick:
/// - refuel `FuelRechargeRate` / `FuelUseTime` / 10 x the platform's
///   `RefuelingMultiplier` (capped at a full tank);
/// - `OnStartRefueling` when the service starts (fuel below full);
/// - damaged: an economy request for `RepairConsumeEnergy` /
///   `RepairConsumeMass`; each time it is paid in full, health +=
///   `RefuelingRepairAmount` / 10 and it asks again (moho stores the request
///   as given, a per-tick amount: the blueprint values are per tick);
/// - fuel above 0.99 and undamaged: the service stops (no callback).
fn staging(lua: &Lua) -> LuaResult<()> {
    let docked: Vec<(EntityId, EntityId)> = entity::world(lua)
        .iter()
        .filter(|e| !e.destroyed && e.unit.as_deref().is_some_and(|u| u.fuel_use_time > Fx::ZERO))
        .filter_map(|e| e.attachment.map(|a| (e.id, a.parent)))
        .collect();
    let mut started = Vec::new();
    let mut refuelled = Vec::new();
    let mut leaving = Vec::new();
    for (unit, platform) in docked {
        let service = entity::world(lua).get(platform).and_then(|e| e.unit.as_deref()).and_then(|u| u.staging.as_deref().cloned());
        let Some(service) = service else { continue };
        // Docked on a pad, another order takes it off. Held inside a carrier
        // it stays (with the order queued) until the carrier unloads: the
        // real game's stored interceptors ordered to move stay aboard
        // (probe_launch).
        let stored = entity::world(lua).get(unit).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.stored_in == Some(platform));
        if !stored && lua.app_data_ref::<Commands>().is_some_and(|c| !c.is_idle(unit)) {
            leaving.push((unit, platform));
            continue;
        }
        let mut events = lua.app_data_mut::<crate::engine::sim::economy::EconomyEvents>();
        let mut w = entity::world(lua);
        let Some(e) = w.get_mut(unit) else { continue };
        let damaged = e.health < e.max_health;
        let Some(u) = e.unit.as_deref_mut() else { continue };
        let old = u.fuel_ratio;
        if !u.refueling && old < Fx::ONE {
            u.refueling = true;
            started.push(unit);
        }
        let mut heal = Fx::ZERO;
        if let (true, Some(events)) = (damaged, events.as_deref_mut()) {
            let ask = |events: &mut crate::engine::sim::economy::EconomyEvents| events.create(unit, service.repair_energy, service.repair_mass, Fx::ZERO);
            match u.staging_repair {
                None => {
                    u.refueling = true;
                    u.staging_repair = Some(ask(events));
                }
                Some(id) if events.is_done(id) => {
                    events.remove(id);
                    heal = service.repair_amount / Fx::from_num(10);
                    u.staging_repair = Some(ask(events));
                }
                Some(_) => {}
            }
        }
        if u.refueling && old > Fx::from_num(0.99) && !damaged {
            u.refueling = false;
            if let (Some(id), Some(events)) = (u.staging_repair.take(), events.as_deref_mut()) {
                events.remove(id);
            }
        }
        let step = u.fuel_recharge / (u.fuel_use_time * Fx::from_num(10)) * service.refuel_multiplier;
        u.fuel_ratio = (old.max(Fx::ZERO) + step).min(Fx::ONE);
        if old <= Fx::ZERO && u.fuel_ratio > Fx::ZERO {
            refuelled.push(unit);
        }
        e.health = (e.health + heal).min(e.max_health);
    }
    for unit in started {
        if let Some(obj) = object(lua, unit)? {
            UnitCallbacks::on_start_refueling(lua, &obj, "unit")?;
        }
    }
    for unit in refuelled {
        if let Some(obj) = object(lua, unit)? {
            UnitCallbacks::on_got_fuel(lua, &obj, "unit")?;
        }
    }
    for (unit, platform) in leaving {
        detach(lua, platform, unit)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::slot_class;

    #[test]
    fn slot_sizes_from_bone_names() {
        // Names from uea0107, uaa0107, uea0104 and xsa0107.
        for (name, class) in [
            ("Left_Attachpoint_sml_02", Some(1)),
            ("Attachpoint_Small_01", Some(1)),
            ("Left_Attachpoint03", Some(1)),
            ("Attachpoint_Med_01", Some(2)),
            ("Attachpoint_Med01", Some(2)),
            ("Attachpoint_Lrg_02", Some(3)),
            ("Attachpoint_Lrg01", Some(3)),
            ("Attachpoint_Spr01", Some(4)),
            ("Turret_Muzzle", None),
        ] {
            assert_eq!(slot_class(name), class, "{name}");
        }
    }
}
