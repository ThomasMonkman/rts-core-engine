//! Capture: units executing `Capture` orders on an enemy unit.
//!
//! Per tick (after movement has brought the captor into build range):
//! 1. Start: `OnStartCapture(target)` on the captor (`Capturing` state),
//!    `OnStartBeingCaptured(captor)` on the target (`BeingCaptured`).
//! 2. Progress: at the start, the captor's `GetCaptureCosts` (FAF's Lua:
//!    time, energy, mass) on the target and on each unit attached to it
//!    (cargo) are summed: ticks `trunc(ticks + max(1, time * 10))` per call,
//!    the costs as they are. An economy event asks `max(0, cost) / ticks` a
//!    tick, and each tick it's granted all of it the capture moves one whole
//!    tick on, so a capture drains the target's `BuildCostEnergy` and stalls
//!    with the army's energy (`AiUnitCapture.cpp`, MohoEngine.dll
//!    0x101efd32..0x101f00d9). Shown as the captor's work progress.
//! 3. Finish (progress 1): `OnStopCapture(target)` / `OnStopBeingCaptured
//!    (captor)`, then `target:OnCaptured(captor)` (FAF's script transfers it
//!    with `ChangeUnitArmy`), and the order pops.
//! A captor whose order changed, or that left range, fails:
//! `OnFailedCapture(target)` / `OnFailedBeingCaptured(captor)`, progress
//! lost. Uncapturable targets (`SetCapturable(false)`) and targets with no
//! capture time (`GetCaptureCosts` 0: unfinished units) end the order.
//! Captors are kept by id, so callbacks run in a fixed order.
//!
//! Inferred, not verified against the binary: each captor progresses on its
//! own (FAF's `CheckCaptor` syncs the bars through `SetWorkProgress`, which
//! ours doesn't read back), a paused captor holds (its event asks for
//! nothing), range = build range + 2.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::entity;
use crate::lua_api::Obj;
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::economy::EconomyEvents;
use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::Fx;
use crate::engine::sim::unit_state::UnitStates;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Job {
    target: EntityId,
    command: CommandId,
    /// The economy event paying for it, one whole tick at a time.
    event: u32,
}

/// Captors and their capture in progress.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Captures(BTreeMap<EntityId, Job>);

fn set_state(lua: &Lua, unit: EntityId, name: &str, on: bool) {
    let i = UnitStates::index(name).expect("unit state");
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.states.set(i, on);
    }
}

fn set_work(lua: &Lua, unit: EntityId, focus: Option<EntityId>, progress: Fx) -> LuaResult<()> {
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = progress;
    }
    crate::engine::api::unit::set_focus(lua, unit, focus)
}

fn pop_order(lua: &Lua, unit: EntityId, command: CommandId) {
    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
    if c.current(unit).is_some_and(|cmd| cmd.id == command) {
        c.pop_front(unit);
    }
}

fn in_reach(world: &EntityWorld, captor: EntityId, target: EntityId) -> bool {
    let (Some(c), Some(t)) = (world.get(captor).filter(|e| !e.destroyed), world.get(target).filter(|e| !e.destroyed)) else { return false };
    // The range touches the target's build skirt (a large structure is
    // captured from its edge, its centre needn't come in range).
    t.in_build_range(c.position(), c.unit.as_deref().map_or(Fx::from_num(5), |u| u.build_range) + Fx::from_num(2))
}

/// Ends a capture: `OnStop*` when it succeeded, `OnFailed*` otherwise.
fn end(lua: &Lua, captor: EntityId, success: bool) -> LuaResult<()> {
    let Some(job) = lua.app_data_mut::<Captures>().expect("Captures").0.remove(&captor) else { return Ok(()) };
    lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").remove(job.event);
    set_work(lua, captor, None, Fx::ZERO)?;
    set_state(lua, captor, "Capturing", false);
    set_state(lua, job.target, "BeingCaptured", false);
    let (c_obj, t_obj) = (entity::object(lua, captor)?, entity::object(lua, job.target)?);
    if let (Some(c), Some(t)) = (&c_obj, &t_obj) {
        if success {
            UnitCallbacks::on_stop_capture(lua, c, "unit", Obj::new(t.clone()))?;
            UnitCallbacks::on_stop_being_captured(lua, t, "unit", Obj::new(c.clone()))?;
        } else {
            UnitCallbacks::on_failed_capture(lua, c, "unit", Obj::new(t.clone()))?;
            UnitCallbacks::on_failed_being_captured(lua, t, "unit", Obj::new(c.clone()))?;
        }
    }
    if let Some(c) = &c_obj
        && entity::world(lua).get(captor).is_some_and(|e| !e.destroyed) {
        crate::engine::api::construction::stop_tracking(lua, c)?;
    }
    Ok(())
}

/// `GetCaptureCosts(target)` on the captor: (time in seconds, energy,
/// mass); time 0 = can't.
fn capture_costs(captor: &LuaTable, target: &LuaTable) -> LuaResult<(Fx, Fx, Fx)> {
    let Some(f) = captor.get::<Option<LuaFunction>>("GetCaptureCosts")? else { return Ok((Fx::ZERO, Fx::ZERO, Fx::ZERO)) };
    let fx = crate::engine::sim::math::from_lua;
    match f.call::<(Option<f64>, Option<f64>, Option<f64>)>((captor.clone(), target.clone())) {
        Ok((t, e, m)) => Ok((fx(t.unwrap_or(0.0)), fx(e.unwrap_or(0.0)), fx(m.unwrap_or(0.0)))),
        Err(e) => {
            tracing::warn!(target: "lua", "Error running GetCaptureCosts script in unit: {e}");
            Ok((Fx::ZERO, Fx::ZERO, Fx::ZERO))
        }
    }
}

/// Ticks a capture call adds: moho sums `max(1, time * 10)` onto the integer
/// count and truncates (0x101efd79..0x101efdac).
fn add_ticks(ticks: u32, time: Fx) -> u32 {
    (Fx::from_num(ticks) + (time * Fx::from_num(10)).max(Fx::ONE)).to_num::<u32>()
}

/// The whole capture's (ticks, energy, mass): the target's costs plus those
/// of each live unit attached to it; `None` when the target can't be
/// captured (time 0).
fn total_costs(lua: &Lua, captor: &LuaTable, target: EntityId, t_obj: &LuaTable) -> LuaResult<Option<(u32, Fx, Fx)>> {
    let (time, mut energy, mut mass) = capture_costs(captor, t_obj)?;
    if time <= Fx::ZERO {
        return Ok(None);
    }
    let mut ticks = add_ticks(0, time);
    let cargo: Vec<EntityId> = {
        let w = entity::world(lua);
        w.attached_to(target).into_iter().filter(|&c| w.get(c).is_some_and(|e| e.unit.is_some() && !e.destroyed)).collect()
    };
    for c in cargo {
        let Some(obj) = entity::object(lua, c)? else { continue };
        let (t, e, m) = capture_costs(captor, &obj)?;
        ticks = add_ticks(ticks, t);
        energy = energy.saturating_add(e);
        mass = mass.saturating_add(m);
    }
    Ok(Some((ticks, energy.max(Fx::ZERO), mass.max(Fx::ZERO))))
}

pub fn tick(lua: &Lua) -> LuaResult<()> {
    let orders: Vec<(EntityId, EntityId, CommandId)> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(()) };
        c.units_doing(&[CommandKind::Capture])
            .into_iter()
            .filter_map(|u| {
                let cmd = c.current(u)?;
                match (cmd.kind, cmd.target) {
                    (CommandKind::Capture, CommandTarget::Entity(t)) => Some((u, t, cmd.id)),
                    _ => None,
                }
            })
            .collect()
    };
    // Captures whose order is no longer current fail.
    let jobs: Vec<(EntityId, Job)> = lua.app_data_ref::<Captures>().map(|c| c.0.iter().map(|(&k, &v)| (k, v)).collect()).unwrap_or_default();
    for (captor, job) in jobs {
        let current = lua.app_data_ref::<Commands>().and_then(|c| c.current(captor).map(|c| c.id));
        if current != Some(job.command) {
            end(lua, captor, false)?;
        }
    }
    for (captor, target, command) in orders {
        let capturable = entity::world(lua).get(target).is_some_and(|t| t.unit.as_deref().is_some_and(|u| u.capturable));
        if !capturable {
            end(lua, captor, false)?;
            pop_order(lua, captor, command);
            continue;
        }
        let current = lua.app_data_ref::<Captures>().and_then(|c| c.0.get(&captor).copied());
        if !in_reach(&entity::world(lua), captor, target) {
            if current.is_some() {
                end(lua, captor, false)?;
            }
            continue;
        }
        let (Some(c_obj), Some(t_obj)) = (entity::object(lua, captor)?, entity::object(lua, target)?) else { continue };
        let job = match current {
            Some(j) => j,
            None => {
                let Some((ticks, energy, mass)) = total_costs(lua, &c_obj, target, &t_obj)? else {
                    pop_order(lua, captor, command);
                    continue;
                };
                let event = lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").create_ticks(captor, energy, mass, ticks);
                let job = Job { target, command, event };
                lua.app_data_mut::<Captures>().expect("Captures").0.insert(captor, job);
                set_work(lua, captor, Some(target), Fx::ZERO)?;
                set_state(lua, captor, "Capturing", true);
                set_state(lua, target, "BeingCaptured", true);
                crate::engine::api::construction::prepare_arm(lua, &c_obj)?;
                UnitCallbacks::on_start_capture(lua, &c_obj, "unit", Obj::new(t_obj.clone()))?;
                UnitCallbacks::on_start_being_captured(lua, &t_obj, "unit", Obj::new(c_obj.clone()))?;
                job
            }
        };
        let paused = entity::world(lua).get(captor).and_then(|e| e.unit.as_deref().map(|u| u.paused)).unwrap_or(false);
        let state = lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").map.get_mut(&job.event).map(|ev| {
            ev.held = paused;
            (ev.progress(), ev.done())
        });
        // Its event gone (it can't be): the capture fails.
        let Some((progress, done)) = state else {
            end(lua, captor, false)?;
            continue;
        };
        set_work(lua, captor, Some(target), progress)?;
        if !done {
            continue;
        }
        end(lua, captor, true)?;
        UnitCallbacks::on_captured(lua, &t_obj, "unit", Obj::new(c_obj.clone()))?;
        pop_order(lua, captor, command);
    }
    Ok(())
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Captures::default());
    Ok(())
}
