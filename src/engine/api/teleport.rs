//! Teleport orders (moho): `IssueTeleport`.
//!
//! When a unit's current order becomes a teleport (and the dispatcher
//! doesn't hold it: [`crate::engine::api::orders::held`]), moho puts it in
//! the `Teleporting` state and calls `OnTeleportUnit(teleporter, location,
//! orientation)`; FAF's script does the rest (charges energy, then `Warp`s
//! while the unit is still `Teleporting`, clearing `TeleportThread` when
//! done). The order completes once that thread is gone. An order that goes
//! away first (cleared, unit dead) gets `OnFailedTeleport`, and the state is
//! dropped so FAF's thread won't warp. An order with nowhere to go (no
//! target, or a target entity that's gone) ends at once rather than holding
//! the queue.
//!
//! Confirmed in the binary: the dispatch (MohoEngine.dll 0x101f470b) makes
//! the task with the teleporter = the unit itself and the orientation from
//! the unit's own transform; the task's start (0x101f5a50, state 0) sets
//! `Teleporting` (0x80000) and calls `OnTeleportUnit`. `IssueTeleportToBeacon`
//! is not a teleport at all: it issues an unload (`api::command`).
//!
//! Not done: moho's start first tests the destination for the unit's
//! footprint (0x10214f40) and fails the order there (`OnFailedTeleport`);
//! it passes the location with y = 0; and the running task (state 1) ends
//! once the unit is no longer `Immobile` (FAF's thread holds it immobile),
//! where ours waits for `TeleportThread` to go (the same for FAF's script).

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::entity;
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::unit_state::UnitStates;

/// Units teleporting, with the order that started it.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Teleports(BTreeMap<EntityId, CommandId>);

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Teleports::default());
    Ok(())
}

fn set_teleporting(lua: &Lua, unit: EntityId, on: bool) {
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.states.set(UnitStates::index("Teleporting").expect("unit state"), on);
    }
}

/// Once a tick: start new teleports, finish or fail running ones.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    // Teleport orders that just became current.
    let starting: Vec<(EntityId, CommandId, CommandTarget)> = match lua.app_data_ref::<Commands>() {
        Some(c) => {
            let running = lua.app_data_ref::<Teleports>().expect("Teleports");
            let world = entity::world(lua);
            c.units_doing(&[CommandKind::Teleport])
                .into_iter()
                .filter(|&u| !crate::engine::api::orders::held(&world, u))
                .filter_map(|u| c.current(u).map(|cmd| (u, cmd.id, cmd.target)))
                .filter(|(u, id, _)| running.0.get(u) != Some(id))
                .collect()
        }
        None => return Ok(()),
    };
    let running: Vec<(EntityId, CommandId)> = lua.app_data_ref::<Teleports>().expect("Teleports").0.iter().map(|(u, id)| (*u, *id)).collect();
    for (unit, id) in running {
        let alive = entity::world(lua).get(unit).is_some_and(|e| !e.destroyed);
        let current = lua.app_data_ref::<Commands>().and_then(|c| c.current(unit).map(|cmd| cmd.id)) == Some(id);
        let obj = if alive { entity::object(lua, unit)? } else { None };
        if !alive || !current {
            lua.app_data_mut::<Teleports>().expect("Teleports").0.remove(&unit);
            if let Some(o) = obj {
                set_teleporting(lua, unit, false);
                UnitCallbacks::on_failed_teleport(lua, &o, "unit")?;
            }
            continue;
        }
        // FAF's teleport thread finished (warped, or refused the jump).
        let thread_done = match &obj {
            Some(o) => o.get::<LuaValue>("TeleportThread")?.is_nil(),
            None => true,
        };
        if thread_done {
            lua.app_data_mut::<Teleports>().expect("Teleports").0.remove(&unit);
            set_teleporting(lua, unit, false);
            pop(lua, unit, id);
        }
    }
    for (unit, id, target) in starting {
        let Some(obj) = entity::object(lua, unit)? else { continue };
        let location = match target {
            CommandTarget::Ground(p) => Some(p),
            CommandTarget::Entity(e) => entity::world(lua).get(e).filter(|e| !e.destroyed).map(|e| e.position()),
            CommandTarget::None => None,
        };
        let Some(location) = location else {
            pop(lua, unit, id);
            continue;
        };
        let orientation = entity::world(lua).get(unit).map_or(crate::engine::sim::math::FxQuat::IDENTITY, |e| e.orientation);
        lua.app_data_mut::<Teleports>().expect("Teleports").0.insert(unit, id);
        set_teleporting(lua, unit, true);
        let teleporter = crate::lua_api::shapes::Loose::new(LuaValue::Table(obj.clone()));
        UnitCallbacks::on_teleport_unit(lua, &obj, "unit", teleporter, entity::vector(lua, location)?, entity::quaternion(lua, orientation)?)?;
    }
    Ok(())
}

fn pop(lua: &Lua, unit: EntityId, id: CommandId) {
    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
    if c.current(unit).is_some_and(|cmd| cmd.id == id) {
        c.pop_front(unit);
    }
}
