//! Ferry routes and the units waiting at their beacons.
//!
//! A transport's ferry order is a route: its first `Ferry` command (whose
//! beacon `transport::ferry_tasks` makes) and the `Ferry` commands queued
//! straight after it. moho's ferry task (`CUnitFerryTask`, update
//! MohoEngine.dll 0x101fa160) runs it for as long as the order stands:
//! - to the beacon (state 4, 0x101f9fa0), then at the beacon (state 0,
//!   0x101f9730): it picks up the units waiting there (0x101f90d0: its
//!   army's units in `WaitForFerry`, not assigned, not waiting for or riding
//!   a transport, that it can carry and has room for, waiting at this
//!   beacon) and, once it holds cargo and has nothing left to pick up, sets
//!   off;
//! - out along the route (state 1, 0x101f9ab0), to the last point, where it
//!   unloads (state 2, 0x101f9c10), back along it (state 3, 0x101f9e00),
//!   and to the beacon again.
//! moho takes each leg as a subtask; ours queues it as an order ahead of the
//! ferry order (`Commands::issue_front`) and steps on when it's done.
//!
//! A unit ordered to load onto a ferry beacon (the right click, a factory's
//! rally to one, `CPlatoon:UseFerryBeacon`) waits there (moho's
//! `CUnitWaitForFerryTask`, update 0x101fa920: `WaitForFerry`, walks to the
//! beacon; once a transport is assigned it boards it (the load task); the
//! order ends when it gets off again, or if boarding fails, or the beacon
//! goes). Ours: the boarding is an order ahead of the beacon order.
//!
//! Per tick, in unit id order, from `transport::tick`.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::entity;
use crate::engine::api::transport;
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::math::{Fx, FxVec3};

/// Where a ferrying transport is in its loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Phase {
    /// Heading for the beacon (moho state 4).
    ToBeacon,
    /// At the beacon, picking up (state 0).
    AtBeacon,
    /// Along the route, then unloading at its end (states 1, 2).
    Out,
    /// Back along it (state 3).
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Route {
    /// The route's first ferry command (the beacon's).
    pub command: CommandId,
    pub phase: Phase,
    /// The route point being flown to (moho's task +0x34).
    pub index: u32,
    /// The leg queued ahead of the ferry order, while it runs.
    pub pending: Option<CommandId>,
}

/// Ferrying transports.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct FerryRoutes(pub(crate) BTreeMap<EntityId, Route>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Wait {
    pub beacon: EntityId,
    /// The unit's load order onto the beacon.
    pub command: CommandId,
    /// The transport picking it up (moho's `GetAssignedTransport`).
    pub transport: Option<EntityId>,
    /// Its order boarding that transport.
    pub boarding: Option<CommandId>,
    /// Aboard (from then, getting off ends the wait).
    pub riding: bool,
}

/// Units waiting at ferry beacons.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct FerryWaits(pub(crate) BTreeMap<EntityId, Wait>);

pub(crate) fn register(lua: &Lua) {
    lua.set_app_data(FerryRoutes::default());
    lua.set_app_data(FerryWaits::default());
}

fn is(lua: &Lua, id: EntityId, name: &str) -> bool {
    let Ok(d) = crate::engine::api::categories::domain(lua) else { return false };
    let index = entity::world(lua).get(id).and_then(|e| e.category_index);
    d.named.get(name).zip(index).is_some_and(|(s, i)| s.contains(i))
}

/// Whether `id` is a live ferry beacon (`FERRYBEACON`).
pub(crate) fn is_beacon(lua: &Lua, id: EntityId) -> bool {
    entity::world(lua).get(id).is_some_and(|e| !e.destroyed && !e.cold.killed) && is(lua, id, "FERRYBEACON")
}

fn position(lua: &Lua, id: EntityId) -> Option<FxVec3> {
    entity::world(lua).get(id).filter(|e| !e.destroyed).map(|e| e.position())
}

fn commands(lua: &Lua) -> mlua::AppDataRefMut<'_, Commands> {
    lua.app_data_mut::<Commands>().expect("Commands not in app_data")
}

/// The units waiting at beacons: new ones join, finished ones leave.
pub(crate) fn waits(lua: &Lua) {
    if lua.app_data_ref::<Commands>().is_none() {
        return;
    }
    // Loads onto a beacon that aren't waiting yet. (One category and world
    // look-up for the pass: every loading unit is scanned each tick.)
    let new: Vec<(EntityId, EntityId, CommandId)> = {
        let Ok(domain) = crate::engine::api::categories::domain(lua) else { return };
        let Some(beacons) = domain.named.get("FERRYBEACON") else { return };
        let c = commands(lua);
        let w = lua.app_data_ref::<FerryWaits>();
        let world = entity::world(lua);
        c.units_doing(&[CommandKind::TransportLoadUnits])
            .into_iter()
            .filter(|u| w.as_ref().is_none_or(|w| !w.0.contains_key(u)))
            .filter_map(|u| match c.current(u)?.target {
                CommandTarget::Entity(b) => Some((u, b, c.current(u)?.id)),
                _ => None,
            })
            .filter(|&(_, b, _)| world.get(b).is_some_and(|e| !e.destroyed && !e.cold.killed && e.category_index.is_some_and(|i| beacons.contains(i))))
            .collect()
    };
    for (unit, beacon, command) in new {
        if let Some(mut w) = lua.app_data_mut::<FerryWaits>() {
            w.0.insert(unit, Wait { beacon, command, transport: None, boarding: None, riding: false });
        }
        transport::set_state(lua, unit, "WaitForFerry", true);
    }
    let all: Vec<(EntityId, Wait)> = lua.app_data_ref::<FerryWaits>().map(|w| w.0.iter().map(|(&u, &w)| (u, w)).collect()).unwrap_or_default();
    for (unit, mut wait) in all {
        let alive = entity::world(lua).get(unit).is_some_and(|e| !e.destroyed && !e.cold.killed);
        let queued = commands(lua).queue(unit).any(|c| c.id == wait.command);
        if !alive || !queued {
            end_wait(lua, unit, wait.command, false);
            continue;
        }
        if !is_beacon(lua, wait.beacon) {
            end_wait(lua, unit, wait.command, true);
            continue;
        }
        let Some(_) = wait.transport else { continue };
        let attached = entity::world(lua).get(unit).is_some_and(|e| e.attachment.is_some());
        if attached {
            if !wait.riding {
                wait.riding = true;
                if let Some(mut w) = lua.app_data_mut::<FerryWaits>() {
                    w.0.insert(unit, wait);
                }
            }
            continue;
        }
        if wait.riding {
            // Delivered.
            end_wait(lua, unit, wait.command, true);
            continue;
        }
        let boarding = wait.boarding.is_some_and(|b| commands(lua).queue(unit).any(|c| c.id == b));
        if !boarding {
            end_wait(lua, unit, wait.command, true);
        }
    }
}

/// The wait ends (moho's wait task returns -1): its beacon gone, boarding
/// failed, or off the transport again; with `remove_order`, the beacon
/// order goes too.
fn end_wait(lua: &Lua, unit: EntityId, command: CommandId, remove_order: bool) {
    if remove_order {
        commands(lua).remove_from_queue(unit, command);
    }
    if let Some(mut w) = lua.app_data_mut::<FerryWaits>() {
        w.0.remove(&unit);
    }
    transport::set_state(lua, unit, "WaitForFerry", false);
}

/// The transport's route: the points of its ferry commands from `command`
/// on (moho's 0x101f9890 list, the consecutive ferry orders).
fn route(lua: &Lua, unit: EntityId, command: CommandId) -> Vec<FxVec3> {
    let targets: Vec<CommandTarget> = {
        let c = commands(lua);
        c.queue(unit).skip_while(|cmd| cmd.id != command).take_while(|cmd| cmd.kind == CommandKind::Ferry).map(|cmd| cmd.target).collect()
    };
    targets
        .into_iter()
        .filter_map(|t| match t {
            CommandTarget::Ground(p) => Some(p),
            CommandTarget::Entity(e) => position(lua, e),
            CommandTarget::None => None,
        })
        .collect()
}

/// Steps each ferrying transport's loop.
pub(crate) fn routes(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<Commands>().is_none() {
        return Ok(());
    }
    // Ferrying: a current ferry order, or one behind the leg queued for it.
    let ferrying: Vec<(EntityId, CommandId)> = {
        let c = commands(lua);
        let routes = lua.app_data_ref::<FerryRoutes>();
        let mut out: BTreeMap<EntityId, CommandId> = BTreeMap::new();
        for u in c.units_doing(&[CommandKind::Ferry]) {
            if let Some(cmd) = c.current(u) {
                out.insert(u, cmd.id);
            }
        }
        if let Some(r) = routes.as_ref() {
            for (&u, route) in &r.0 {
                let current = c.current(u).map(|cmd| cmd.id);
                if current.is_some() && current == route.pending && c.queue(u).any(|cmd| cmd.id == route.command) {
                    out.insert(u, route.command);
                }
            }
        }
        out.into_iter().collect()
    };
    if let Some(mut r) = lua.app_data_mut::<FerryRoutes>() {
        r.0.retain(|u, route| ferrying.iter().any(|&(f, c)| f == *u && c == route.command));
    }
    for (unit, command) in ferrying {
        let existing = lua.app_data_ref::<FerryRoutes>().and_then(|r| r.0.get(&unit).copied());
        let mut route_state = match existing {
            Some(r) => r,
            // moho's task starts out (state 1) with cargo aboard, else for
            // the beacon (state 4).
            None => {
                let loaded = !transport::passengers(lua, unit).is_empty();
                Route { command, phase: if loaded { Phase::Out } else { Phase::ToBeacon }, index: 1, pending: None }
            }
        };
        if let Some(p) = route_state.pending {
            if commands(lua).queue(unit).any(|c| c.id == p) {
                save(lua, unit, route_state);
                continue;
            }
            route_state.pending = None;
        }
        step(lua, unit, &mut route_state)?;
        save(lua, unit, route_state);
    }
    Ok(())
}

fn save(lua: &Lua, unit: EntityId, route: Route) {
    if let Some(mut r) = lua.app_data_mut::<FerryRoutes>() {
        r.0.insert(unit, route);
    }
}

fn leg(lua: &Lua, unit: EntityId, kind: CommandKind, at: FxVec3) -> CommandId {
    commands(lua).issue_front(unit, kind, CommandTarget::Ground(at), None, 0)
}

fn step(lua: &Lua, unit: EntityId, r: &mut Route) -> LuaResult<()> {
    let points = route(lua, unit, r.command);
    let Some(&start) = points.first() else { return Ok(()) };
    let beacon = commands(lua).get(r.command).and_then(|c| c.focus).filter(|&b| is_beacon(lua, b));
    match r.phase {
        // To the beacon (moho moves unless already in reach: 0x101f9fa0).
        Phase::ToBeacon => {
            let at = beacon.and_then(|b| position(lua, b)).unwrap_or(start);
            let near = position(lua, unit).is_some_and(|p| {
                let (dx, dz) = (p.x - at.x, p.z - at.z);
                dx * dx + dz * dz <= Fx::from_num(4)
            });
            if !near {
                r.pending = Some(leg(lua, unit, CommandKind::Move, at));
            }
            r.phase = Phase::AtBeacon;
        }
        Phase::AtBeacon => {
            if let Some(b) = beacon {
                pick_up(lua, unit, b);
            }
            let boarding = lua.app_data_ref::<FerryWaits>().is_some_and(|w| w.0.values().any(|w| w.transport == Some(unit) && !w.riding));
            if !boarding && !transport::passengers(lua, unit).is_empty() {
                r.phase = Phase::Out;
                r.index = 1;
            }
        }
        // Out along the route to its last point, unloading there (0x101f9ab0,
        // 0x101f9c10).
        Phase::Out => {
            let n = points.len() as u32;
            if r.index + 1 < n {
                r.pending = Some(leg(lua, unit, CommandKind::Move, points[r.index as usize]));
                r.index += 1;
            } else {
                r.index = r.index.min(n - 1);
                r.pending = Some(leg(lua, unit, CommandKind::TransportUnloadUnits, points[r.index as usize]));
                r.phase = Phase::Back;
            }
        }
        // Back along it, then to the beacon (0x101f9e00).
        Phase::Back => {
            if r.index > 0 {
                r.index -= 1;
                r.pending = Some(leg(lua, unit, CommandKind::Move, points[r.index as usize]));
            } else {
                r.phase = Phase::ToBeacon;
            }
        }
    }
    Ok(())
}

/// The units waiting at `beacon` that `transport` takes, in id order (moho
/// collects its army's in its unit list's order): not assigned yet, not
/// aboard, that it can carry (the shared rule, `sim::carry`) and has room
/// for (each class's free slots, or storage, less those already taken this
/// pass). Each boards it (an order ahead of its beacon order).
fn pick_up(lua: &Lua, transport_id: EntityId, beacon: EntityId) {
    let army = entity::world(lua).get(transport_id).and_then(|e| e.army);
    let waiting: Vec<EntityId> = lua
        .app_data_ref::<FerryWaits>()
        .map(|w| w.0.iter().filter(|(_, w)| w.beacon == beacon && w.transport.is_none()).map(|(&u, _)| u).collect())
        .unwrap_or_default();
    if waiting.is_empty() {
        return;
    }
    let storage = transport::storage_slots(lua, transport_id);
    let mut room_storage = storage.saturating_sub(transport::stored_units(lua, transport_id).len() as u32) as i64;
    let mut room: [i64; 4] = [1, 2, 3, 4].map(|class| transport::free_slots(lua, transport_id, class).len() as i64);
    for unit in waiting {
        let ok = {
            let w = entity::world(lua);
            w.get(unit).is_some_and(|e| e.army == army && e.attachment.is_none() && !e.destroyed)
        };
        if !ok {
            continue;
        }
        let (passenger, carrier) = transport::carry_sides(lua, unit, transport_id);
        if !crate::engine::sim::carry::can_carry(&passenger, &carrier) {
            continue;
        }
        let class = transport::class_of(lua, unit).clamp(1, 4) as usize - 1;
        let fits = if storage > 0 { room_storage > 0 } else { room[class] > 0 };
        if !fits {
            continue;
        }
        if storage > 0 {
            room_storage -= 1;
        } else {
            room[class] -= 1;
        }
        let boarding = commands(lua).issue_front(unit, CommandKind::TransportLoadUnits, CommandTarget::Entity(transport_id), None, 0);
        if let Some(mut w) = lua.app_data_mut::<FerryWaits>()
            && let Some(wait) = w.0.get_mut(&unit) {
            wait.transport = Some(transport_id);
            wait.boarding = Some(boarding);
        }
    }
}

/// How many units wait at beacons, and how many of them ride a transport
/// (tests, diagnostics).
pub fn waiting(lua: &Lua) -> (usize, usize) {
    lua.app_data_ref::<FerryWaits>().map_or((0, 0), |w| (w.0.len(), w.0.values().filter(|w| w.riding).count()))
}

/// The waiting units and their beacon orders, in unit order (the
/// transport pass leaves those orders to [`waits`]).
pub(crate) fn beacon_orders(lua: &Lua) -> Vec<(EntityId, CommandId)> {
    lua.app_data_ref::<FerryWaits>().map(|w| w.0.iter().map(|(&u, w)| (u, w.command)).collect()).unwrap_or_default()
}
