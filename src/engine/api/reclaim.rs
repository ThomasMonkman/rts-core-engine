//! Reclaim: units executing `Reclaim` orders on a prop or unit, and units
//! that can reclaim picking up props as they go on an aggressive move or
//! patrol (how FAF's AI reclaims: `IssueAggressiveMove` over a reclaim cell).
//!
//! Per tick (after movement has brought the reclaimer into range):
//! 1. Start: `OnStartReclaim(target)` on the reclaimer; it gets the
//!    `Reclaiming` state and the target `BeingReclaimed`.
//! 2. Progress: `time, energy, mass` from the reclaimer's
//!    `GetReclaimCosts(target)` (recomputed every tick, as FAF's Lua
//!    expects); a prop's or unfinished unit's fraction complete drops by
//!    `dt / time`, and the reclaimer's army gains that fraction of `mass` /
//!    `energy`. An unfinished unit's health goes down with its fraction (at
//!    most max health x fraction), and a damaged one's reclaim starts from
//!    its health share (its fraction drops to it), as moho's reclaim task
//!    does (MohoEngine.dll 0x10209e95, 0x1020a06b; not checked against the
//!    real game). Shown as the reclaimer's work progress (the share of the
//!    target consumed, as builders mirror their site's fraction).
//!    A finished unit instead loses max health x `dt / time` a tick, keeps
//!    its fraction and pays nothing ([`drain`]); one broken off partway
//!    stays with the health it has left.
//! 3. Finish (fraction 0): `OnReclaimed(reclaimer)` on the target (FAF's
//!    script destroys it), `OnStopReclaim(nil)` on the reclaimer; a Reclaim
//!    order pops, a move order carries on. A finished unit is done when no
//!    more than a tick's drain is left: `OnReclaimed(reclaimer)`, its
//!    script's `CreateWreckageProp(0)`, and the reclaimer goes on to the
//!    wreck under the same order (dev/probes `probe_unit_reclaim2`).
//! A job belongs to the order that started it: when the reclaimer's current
//! order changes, or the target leaves range, it stops with
//! `OnStopReclaim(target)`. Units marked `SetReclaimable(false)` end a
//! Reclaim order at once. Auto-reclaimers (RULEUCC_Reclaim, current order an
//! aggressive move / patrol, not already reclaiming) look for a prop with
//! reclaim value in range every `SCAN_TICKS`, staggered by id; movement holds
//! them while a job runs (`reclaiming`). Everything runs in reclaimer id
//! order.
//!
//! Resources per fraction, for units as well as props, are moho's: the
//! reclaim task (MohoEngine.dll 0x10209d30..0x1020a1a5) gains `(energy,
//! mass) / ticks x (fraction actually taken / step)` a tick, with ticks
//! `max(1, time x 10)`.
//!
//! Inferred, not verified against the binary: range = build range + 2
//! (moho tests the reclaimer's `MaxBuildDistance`, from a distance not
//! traced), auto-reclaim only takes props already in range (no detours).

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::callbacks::{EntityCallbacks, UnitCallbacks};
use crate::engine::api::entity;
use crate::lua_api::Obj;
use crate::engine::sim::army::Armies;
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::Fx;
use crate::engine::sim::unit_state::UnitStates;

/// Auto-reclaimers look for a prop every this many ticks.
const SCAN_TICKS: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Job {
    target: EntityId,
    /// The order this job serves: a Reclaim order, or the aggressive move /
    /// patrol an auto-reclaim happens during.
    command: CommandId,
    /// Started by a Reclaim order (pops when done) rather than automatically.
    ordered: bool,
}

/// Reclaimers and what each is reclaiming.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Reclaims(BTreeMap<EntityId, Job>);

impl Reclaims {
    /// Units with a reclaim running (movement holds auto-reclaimers).
    pub fn reclaiming(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.0.keys().copied()
    }
}

fn set_state(lua: &Lua, unit: EntityId, name: &str, on: bool) {
    let i = UnitStates::index(name).expect("unit state");
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.states.set(i, on);
    }
}

fn pop_order(lua: &Lua, unit: EntityId, command: CommandId) {
    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
    if c.current(unit).is_some_and(|cmd| cmd.id == command) {
        c.pop_front(unit);
    }
}

/// Ends a reclaim: clears the states and calls `OnStopReclaim(target)`
/// (`nil` once the target is used up).
fn stop(lua: &Lua, reclaimer: EntityId, used_up: bool) -> LuaResult<()> {
    let Some(job) = lua.app_data_mut::<Reclaims>().expect("Reclaims").0.remove(&reclaimer) else { return Ok(()) };
    if let Some(u) = entity::world(lua).get_mut(reclaimer).and_then(|e| e.unit.as_deref_mut()) {
        u.focus = None;
    }
    set_state(lua, reclaimer, "Reclaiming", false);
    set_state(lua, job.target, "BeingReclaimed", false);
    if let Some(r) = entity::object(lua, reclaimer)? {
        let t = if used_up { None } else { entity::object(lua, job.target)? };
        UnitCallbacks::on_stop_reclaim(lua, &r, "unit", t.map(Obj::new))?;
        if entity::world(lua).get(reclaimer).is_some_and(|e| !e.destroyed) {
            crate::engine::api::construction::stop_tracking(lua, &r)?;
        }
    }
    // Its work is over (after `OnStopReclaim`, which may read it), as in construction.
    if let Some(u) = entity::world(lua).get_mut(reclaimer).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = Fx::ZERO;
    }
    Ok(())
}

/// `GetReclaimCosts(target)` on the reclaimer: (time in seconds, energy, mass).
fn costs(reclaimer: &LuaTable, target: &LuaTable) -> LuaResult<(Fx, Fx, Fx)> {
    let Some(f) = reclaimer.get::<Option<LuaFunction>>("GetReclaimCosts")? else { return Ok((Fx::ZERO, Fx::ZERO, Fx::ZERO)) };
    match f.call::<(Option<f64>, Option<f64>, Option<f64>)>((reclaimer.clone(), target.clone())) {
        Ok((t, e, m)) => {
            let fx = |v: Option<f64>| crate::engine::sim::math::from_lua(v.unwrap_or(0.0));
            Ok((fx(t), fx(e), fx(m)))
        }
        Err(e) => {
            tracing::warn!(target: "lua", "Error running GetReclaimCosts script in unit: {e}");
            Ok((Fx::ZERO, Fx::ZERO, Fx::ZERO))
        }
    }
}

/// Reach for reclaiming: build range + 2.
fn reach(world: &EntityWorld, unit: EntityId) -> Fx {
    world.get(unit).and_then(|e| e.unit.as_deref().map(|u| u.build_range)).unwrap_or(Fx::from_num(5)) + Fx::from_num(2)
}

fn in_reach(world: &EntityWorld, reclaimer: EntityId, target: EntityId) -> bool {
    let (Some(r), Some(t)) = (world.get(reclaimer).filter(|e| !e.destroyed), world.get(target).filter(|e| !e.destroyed)) else { return false };
    // The range touches the target's build skirt (a large wreck is
    // reclaimed from its edge); props, without unit data, by their centre.
    t.in_build_range(r.position(), reach(world, reclaimer))
}

/// Whether a prop still has something to reclaim: FAF keeps the values on
/// the script object (`Prop:SetMaxReclaimValues`).
pub(crate) fn has_value(obj: &LuaTable) -> LuaResult<bool> {
    let n = |k: &str| -> LuaResult<f64> { Ok(obj.raw_get::<Option<f64>>(k)?.unwrap_or(0.0)) };
    Ok(n("MaxMassReclaim")? + n("MaxEnergyReclaim")? > 0.0)
}

/// Auto-reclaim candidates due a scan this tick: (unit, current order).
fn auto_candidates(lua: &Lua, tick: u64, busy: &BTreeMap<EntityId, Job>) -> Vec<(EntityId, CommandId)> {
    let reclaim_cap = 1u32 << crate::engine::sim::unit_state::COMMAND_CAPS.iter().position(|c| *c == "RULEUCC_Reclaim").expect("cap");
    let Some(c) = lua.app_data_ref::<Commands>() else { return Vec::new() };
    let w = entity::world(lua);
    use CommandKind as K;
    c.units_doing(&[K::AggressiveMove, K::FormAggressiveMove, K::Patrol, K::FormPatrol])
        .into_iter()
        .filter(|u| !busy.contains_key(u) && (tick + *u as u64).is_multiple_of(SCAN_TICKS))
        .filter_map(|u| {
            let cmd = c.current(u)?;
            use CommandKind as K;
            if !matches!(cmd.kind, K::AggressiveMove | K::FormAggressiveMove | K::Patrol | K::FormPatrol) {
                return None;
            }
            let e = w.get(u).filter(|e| !e.destroyed && e.fraction_complete >= Fx::ONE)?;
            (e.unit.as_deref()?.command_caps & reclaim_cap != 0).then_some((u, cmd.id))
        })
        .collect()
}

/// The nearest prop in reach with reclaim value (ties: lowest id).
fn pick_prop(lua: &Lua, unit: EntityId, buf: &mut Vec<EntityId>) -> LuaResult<Option<EntityId>> {
    let ranked: Vec<(Fx, EntityId)> = {
        let w = entity::world(lua);
        let Some(p) = w.get(unit).map(|e| e.position()) else { return Ok(None) };
        w.props_in_circle_into(p.x, p.z, reach(&w, unit), buf);
        let mut v: Vec<(Fx, EntityId)> = buf
            .iter()
            .filter_map(|&id| {
                let e = w.get(id).filter(|e| e.fraction_complete > Fx::ZERO)?;
                let q = e.position();
                let (dx, dz) = (q.x - p.x, q.z - p.z);
                Some((dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)), id))
            })
            .collect();
        v.sort_unstable();
        v
    };
    for (_, id) in ranked {
        if let Some(obj) = entity::object(lua, id)?
            && has_value(&obj)? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

pub fn tick(lua: &Lua) -> LuaResult<()> {
    let tick = lua.app_data_ref::<crate::engine::sim::scheduler::Scheduler>().map_or(0, |s| s.tick());
    let ordered: Vec<(EntityId, EntityId, CommandId)> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(()) };
        c.units_doing(&[CommandKind::Reclaim])
            .into_iter()
            .filter_map(|u| {
                let cmd = c.current(u)?;
                match (cmd.kind, cmd.target) {
                    (CommandKind::Reclaim, CommandTarget::Entity(t)) => Some((u, t, cmd.id)),
                    _ => None,
                }
            })
            .collect()
    };
    // Jobs whose order is no longer current stop.
    let jobs: Vec<(EntityId, Job)> = lua.app_data_ref::<Reclaims>().map(|r| r.0.iter().map(|(&k, &v)| (k, v)).collect()).unwrap_or_default();
    for (unit, job) in jobs {
        let current = lua.app_data_ref::<Commands>().and_then(|c| c.current(unit).map(|c| c.id));
        if current != Some(job.command) {
            stop(lua, unit, false)?;
        }
    }
    // Ordered reclaims: (re)target the order's entity.
    // A unit is reclaimable when its script says so (`SetReclaimable`), else
    // when its blueprint has `RECLAIMABLE` (commanders don't); props always.
    let reclaimable_set = if ordered.is_empty() { None } else { crate::engine::api::categories::domain(lua)?.named.get("RECLAIMABLE").cloned() };
    for (reclaimer, target, command) in ordered {
        let reclaimable = entity::world(lua).get(target).is_some_and(|t| match t.unit.as_deref() {
            None => true,
            Some(u) => u.reclaimable.unwrap_or_else(|| t.category_index.zip(reclaimable_set.as_ref()).is_some_and(|(i, s)| s.contains(i))),
        });
        if !reclaimable {
            stop(lua, reclaimer, false)?;
            pop_order(lua, reclaimer, command);
            continue;
        }
        let current = lua.app_data_ref::<Reclaims>().and_then(|r| r.0.get(&reclaimer).copied());
        if current.map(|j| j.target) != Some(target) {
            if current.is_some() {
                stop(lua, reclaimer, false)?;
            }
            if in_reach(&entity::world(lua), reclaimer, target) {
                start(lua, reclaimer, Job { target, command, ordered: true })?;
            }
        }
    }
    // Auto-reclaim: pick up props in reach during aggressive moves / patrols.
    let busy = lua.app_data_ref::<Reclaims>().map(|r| r.0.clone()).unwrap_or_default();
    let mut buf = Vec::new();
    for (unit, command) in auto_candidates(lua, tick, &busy) {
        if let Some(target) = pick_prop(lua, unit, &mut buf)? {
            start(lua, unit, Job { target, command, ordered: false })?;
        }
    }
    // Progress every running job.
    let dt = Fx::ONE / Fx::from_num(10);
    let jobs: Vec<(EntityId, Job)> = lua.app_data_ref::<Reclaims>().map(|r| r.0.iter().map(|(&k, &v)| (k, v)).collect()).unwrap_or_default();
    for (reclaimer, job) in jobs {
        if !in_reach(&entity::world(lua), reclaimer, job.target) {
            stop(lua, reclaimer, false)?;
            continue;
        }
        // Paused reclaimers reclaim at speed 0 (FA-Binary-Patches #19).
        let (army, paused) = {
            let w = entity::world(lua);
            let r = w.get(reclaimer);
            (r.and_then(|e| e.army), r.and_then(|e| e.unit.as_deref().map(|u| u.paused)).unwrap_or(false))
        };
        if paused {
            continue;
        }
        let (Some(r_obj), Some(t_obj)) = (entity::object(lua, reclaimer)?, entity::object(lua, job.target)?) else { continue };
        let (time, energy, mass) = costs(&r_obj, &t_obj)?;
        let step = if time > Fx::ZERO { dt / time } else { Fx::ONE };
        if finished_unit(&entity::world(lua), job.target) {
            drain(lua, reclaimer, job, step, &r_obj, &t_obj)?;
            continue;
        }
        let (taken, left) = {
            let mut w = entity::world(lua);
            let Some(t) = w.get_mut(job.target) else { continue };
            let taken = step.min(t.fraction_complete);
            t.fraction_complete -= taken;
            // A unit's health goes with it (moho sets it to max health x
            // min(health share, fraction) every tick, MohoEngine.dll
            // 0x1020a06b): one broken off partway is as weak as it looks.
            if t.unit.is_some() {
                t.health = t.health.min(t.max_health.saturating_mul(t.fraction_complete));
            }
            (taken, t.fraction_complete)
        };
        // The work bar mirrors the target's consumed share, as builders
        // mirror their site's fraction complete.
        if let Some(u) = entity::world(lua).get_mut(reclaimer).and_then(|e| e.unit.as_deref_mut()) {
            u.work_progress = Fx::ONE - left;
        }
        if let (Some(army), Some(mut armies)) = (army, lua.app_data_mut::<Armies>())
            && let Some(a) = armies.get_mut(army) {
            for (r, v) in [(&mut a.economy.mass, mass), (&mut a.economy.energy, energy)] {
                r.add_reclaimed(v.saturating_mul(taken));
            }
        }
        if left <= Fx::ZERO {
            stop(lua, reclaimer, true)?;
            EntityCallbacks::on_reclaimed(lua, &t_obj, "entity", Obj::new(r_obj.clone()))?;
            if job.ordered {
                pop_order(lua, reclaimer, job.command);
            }
        }
    }
    Ok(())
}

/// A finished unit: reclaiming drains its health and pays nothing.
fn finished_unit(world: &EntityWorld, target: EntityId) -> bool {
    world.get(target).is_some_and(|t| t.unit.is_some() && t.fraction_complete >= Fx::ONE)
}

/// One tick of reclaiming a finished unit: its health drops by max health x
/// `step` and the army gains nothing (the fraction stays 1). When no more
/// than a step is left, the unit is reclaimed: `OnReclaimed(reclaimer)`,
/// then its script's `CreateWreckageProp(0)`, then the engine destroys it,
/// and a Reclaim order goes on to the wreck, without a new
/// `OnStartReclaim` (MohoEngine.dll 0x10209a86..0x10209b9c). From the real
/// game (dev/probes `probe_unit_reclaim2`): a T1 tank drains 11.28 a tick
/// with nothing gained, its full wreck (45.36 mass) is left whatever its
/// health was, and the engineer reclaims that under the same order.
fn drain(lua: &Lua, reclaimer: EntityId, mut job: Job, step: Fx, r_obj: &LuaTable, t_obj: &LuaTable) -> LuaResult<()> {
    let reclaimed = {
        let mut w = entity::world(lua);
        let Some(t) = w.get_mut(job.target) else { return Ok(()) };
        let take = t.max_health.saturating_mul(step);
        if t.health > take {
            t.health -= take;
            false
        } else {
            true
        }
    };
    if !reclaimed {
        return Ok(());
    }
    EntityCallbacks::on_reclaimed(lua, t_obj, "entity", Obj::new(r_obj.clone()))?;
    let wreck = match t_obj.get::<Option<LuaFunction>>("CreateWreckageProp")? {
        Some(f) => match f.call::<LuaValue>((t_obj.clone(), 0.0)) {
            Ok(LuaValue::Table(prop)) => entity::entity_id(&Obj::<crate::engine::api::moho::Entity>::new(prop)).ok(),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(target: "lua", "Error running CreateWreckageProp script in unit: {e}");
                None
            }
        },
        None => None,
    };
    let old = job.target;
    entity::destroy(lua, &Obj::new(t_obj.clone()))?;
    set_state(lua, old, "BeingReclaimed", false);
    let wreck = wreck.filter(|&p| entity::world(lua).get(p).is_some_and(|e| !e.destroyed));
    let Some(prop) = wreck else {
        stop(lua, reclaimer, true)?;
        if job.ordered {
            pop_order(lua, reclaimer, job.command);
        }
        return Ok(());
    };
    // On to the wreck, under the same order.
    job.target = prop;
    if job.ordered {
        lua.app_data_mut::<Commands>().expect("Commands").set_target(job.command, CommandTarget::Entity(prop));
    }
    lua.app_data_mut::<Reclaims>().expect("Reclaims").0.insert(reclaimer, job);
    crate::engine::api::unit::set_focus(lua, reclaimer, Some(prop))
}

/// Starts a job: states, focus, work progress (the target's consumed
/// share), `OnStartReclaim(target)`.
fn start(lua: &Lua, reclaimer: EntityId, job: Job) -> LuaResult<()> {
    let (Some(r_obj), Some(t_obj)) = (entity::object(lua, reclaimer)?, entity::object(lua, job.target)?) else { return Ok(()) };
    lua.app_data_mut::<Reclaims>().expect("Reclaims").0.insert(reclaimer, job);
    // A damaged unfinished unit is reclaimed from its health share: its
    // fraction drops to it first (moho's reclaim task, MohoEngine.dll
    // 0x10209e95). A finished one keeps its fraction (`drain`).
    if let Some(t) = entity::world(lua).get_mut(job.target).filter(|t| t.unit.is_some() && t.max_health > Fx::ZERO && t.fraction_complete < Fx::ONE) {
        t.fraction_complete = t.fraction_complete.min(t.health / t.max_health);
    }
    let done = entity::world(lua).get(job.target).map_or(Fx::ZERO, |t| Fx::ONE - t.fraction_complete);
    if let Some(u) = entity::world(lua).get_mut(reclaimer).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = done;
    }
    crate::engine::api::unit::set_focus(lua, reclaimer, Some(job.target))?;
    set_state(lua, reclaimer, "Reclaiming", true);
    set_state(lua, job.target, "BeingReclaimed", true);
    crate::engine::api::construction::prepare_arm(lua, &r_obj)?;
    UnitCallbacks::on_start_reclaim(lua, &r_obj, "unit", Obj::new(t_obj))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Reclaims::default());
    Ok(())
}
