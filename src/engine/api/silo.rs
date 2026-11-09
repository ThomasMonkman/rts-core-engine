//! Missile silos (moho): nuke and tactical launchers build their missiles.
//!
//! A silo weapon (`CountedProjectile`) builds one missile at a time while
//! its unit's current order is the matching silo build order
//! (`IssueSiloBuildNuke` / `IssueSiloBuildTactical`, counted) or its unit is
//! in auto mode, and it holds fewer than `MaxProjectileStorage`. A missile
//! costs its projectile blueprint's `Economy` (`BuildCostEnergy`,
//! `BuildCostMass`) over `BuildTime / BuildRate` seconds, charged as an
//! economy event; meanwhile the unit is in the `SiloBuildingAmmo` state.
//! Callbacks: `OnSiloBuildStart(weapon)` when a missile starts,
//! `OnSiloBuildEnd(weapon)` when it's done or stopped, `OnNukeArmed()` when
//! a nuke is ready. Firing (launch orders, `weapon.rs`) uses the ammo.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::{entity, moho};
use crate::lua_api::{lua_method, Obj};
use crate::engine::sim::command::{CommandKind, Commands};
use crate::engine::sim::economy::EconomyEvents;
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::math::{from_lua, Fx};
use crate::engine::sim::unit_state::UnitStates;
use crate::engine::sim::weapon::{SiloKind, Weapons};

/// A missile being built: the silo weapon and its economy event.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct Job {
    weapon: u32,
    event: u32,
    /// What assisters have put in that hasn't bought a whole tick of the
    /// missile yet (energy, mass): moho's `mSegmentSpent`.
    #[serde(default)]
    banked: (Fx, Fx),
}

/// Missiles in progress by (unit, silo kind), and the ammo each launch order
/// started with (it's done once a missile leaves: FAF's weapon script takes
/// the ammo when it creates the missile).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct SiloJobs(BTreeMap<(EntityId, u8), Job>, BTreeMap<(EntityId, u8), u32>);

fn kind_key(kind: SiloKind) -> u8 {
    match kind {
        SiloKind::Nuke => 0,
        SiloKind::Tactical => 1,
    }
}

fn launch_order(kind: SiloKind) -> CommandKind {
    match kind {
        SiloKind::Nuke => CommandKind::Nuke,
        SiloKind::Tactical => CommandKind::Tactical,
    }
}

fn build_order(kind: SiloKind) -> CommandKind {
    match kind {
        SiloKind::Nuke => CommandKind::BuildSiloNuke,
        SiloKind::Tactical => CommandKind::BuildSiloTactical,
    }
}

fn set_building(lua: &Lua, unit: EntityId, on: bool) {
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.states.set(UnitStates::index("SiloBuildingAmmo").expect("unit state"), on);
    }
}

/// (energy, mass, build time) of a missile blueprint's `Economy`.
fn missile_cost(lua: &Lua, projectile: &str) -> LuaResult<(Fx, Fx, Fx)> {
    let bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Projectile, &projectile.to_lowercase())?;
    let econ = bp.get::<Option<LuaTable>>("Economy")?;
    let get = |k: &str| -> LuaResult<Fx> {
        Ok(match &econ {
            Some(e) => from_lua(e.get::<Option<f64>>(k)?.unwrap_or(0.0)),
            None => Fx::ZERO,
        })
    };
    Ok((get("BuildCostEnergy")?, get("BuildCostMass")?, get("BuildTime")?))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(SiloJobs::default());
    lua_method!(lua, moho::Unit,
        /// Stops missile production: the missile in progress is dropped (its
        /// progress lost, `OnSiloBuildEnd`), a current silo build order ends and
        /// auto mode goes off. FAF saves and restores auto mode around it when it
        /// restarts a missile (`StructureUnit.OnReceiveAdjacencyBuffTo`), so moho
        /// clears it (inferred).
        StopSiloBuild = |lua, this: Obj<moho::Unit>| {
        let unit = entity::entity_id(&this)?;
        if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
            u.auto_mode = false;
        }
        {
            let mut c = lua.app_data_mut::<Commands>().expect("Commands");
            if c.current(unit).is_some_and(|cmd| matches!(cmd.kind, CommandKind::BuildSiloNuke | CommandKind::BuildSiloTactical)) {
                c.pop_front(unit);
            }
        }
        let jobs: Vec<((EntityId, u8), Job)> = {
            let j = lua.app_data_ref::<SiloJobs>().expect("SiloJobs");
            j.0.range((unit, 0)..=(unit, u8::MAX)).map(|(k, v)| (*k, *v)).collect()
        };
        for (key, job) in jobs {
            lua.app_data_mut::<SiloJobs>().expect("SiloJobs").0.remove(&key);
            lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").remove(job.event);
            if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
                u.work_progress = Fx::ZERO;
            }
            set_building(lua, unit, false);
            if let Some(w) = crate::engine::api::weapon::weapon_object(lua, unit, job.weapon)? {
                UnitCallbacks::on_silo_build_end(lua, &this, "unit", Obj::new(w))?;
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// Sets the progress of `unit`'s missile in progress to `blocks` economy
/// ticks; false if none is in progress.
pub fn set_blocks(lua: &Lua, unit: EntityId, blocks: u32) -> bool {
    let job = lua.app_data_ref::<SiloJobs>().and_then(|j| j.0.range((unit, 0)..=(unit, u8::MAX)).next().map(|(_, j)| *j));
    let Some(job) = job else { return false };
    if let Some(e) = lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").map.get_mut(&job.event) {
        e.paid = Fx::from_num(blocks).min(e.ticks);
    }
    true
}

/// Whether `unit` is building a missile (`SiloBuildingAmmo`).
pub fn is_building(lua: &Lua, unit: EntityId) -> bool {
    let state = UnitStates::index("SiloBuildingAmmo").expect("unit state");
    entity::world(lua).get(unit).filter(|e| !e.destroyed).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.states.contains(state))
}

/// An assister puts `energy` and `mass` (this tick's) into `unit`'s missile
/// in progress (moho's `CAiSiloBuildImpl::SiloAssistWithResource`, FAF's
/// patched form): it's banked, and the missile advances by as many of its
/// build ticks as the bank buys (the smaller of the energy and mass
/// ratios, fractions too), capped at the ticks left; that much is taken
/// from the bank. Returns the missile's progress (0 with none in progress).
pub fn assist(lua: &Lua, unit: EntityId, energy: Fx, mass: Fx) -> Fx {
    let Some((key, mut job)) = lua.app_data_ref::<SiloJobs>().and_then(|j| j.0.range((unit, 0)..=(unit, u8::MAX)).next().map(|(k, j)| (*k, *j))) else {
        return Fx::ZERO;
    };
    let progress = {
        let mut events = lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents");
        let Some(ev) = events.map.get_mut(&job.event) else { return Fx::ZERO };
        job.banked.0 = job.banked.0.saturating_add(energy);
        job.banked.1 = job.banked.1.saturating_add(mass);
        // A free resource buys any number of ticks (moho's 0 cost divides
        // to infinity).
        let ratio = |bank: Fx, cost: Fx| if cost > Fx::ZERO { bank.checked_div(cost).unwrap_or(Fx::MAX) } else { Fx::MAX };
        let affordable = ratio(job.banked.0, ev.energy).min(ratio(job.banked.1, ev.mass));
        let advance = affordable.min(ev.ticks - ev.paid).max(Fx::ZERO);
        ev.paid += advance;
        job.banked.0 = (job.banked.0 - ev.energy.saturating_mul(advance)).max(Fx::ZERO);
        job.banked.1 = (job.banked.1 - ev.mass.saturating_mul(advance)).max(Fx::ZERO);
        ev.progress()
    };
    lua.app_data_mut::<SiloJobs>().expect("SiloJobs").0.insert(key, job);
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = progress;
    }
    progress
}

/// Once a tick, after the economy: start, advance and finish missiles.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let silos = match lua.app_data_ref::<Weapons>() {
        Some(w) => w.silos(),
        None => return Ok(()),
    };
    for (unit, index, kind, max, projectile) in silos {
        let key = (unit, kind_key(kind));
        let (alive, ammo, auto, paused, build_rate) = match entity::world(lua).get(unit).filter(|e| !e.destroyed && e.fraction_complete >= Fx::ONE) {
            Some(e) => match e.unit.as_deref() {
                Some(u) => (true, if kind == SiloKind::Nuke { u.nuke_ammo } else { u.tactical_ammo }, u.auto_mode, u.paused, u.build_rate),
                None => (false, 0, false, false, Fx::ZERO),
            },
            None => (false, 0, false, false, Fx::ZERO),
        };
        let current = lua.app_data_ref::<Commands>().and_then(|c| c.current(unit).map(|cmd| (cmd.kind, cmd.id)));
        // A launch order ends once a missile has left (ammo dropped).
        let launching = current.filter(|(k, _)| *k == launch_order(kind));
        let started_with = lua.app_data_ref::<SiloJobs>().and_then(|j| j.1.get(&key).copied());
        match (launching, started_with) {
            (Some(_), None) => {
                lua.app_data_mut::<SiloJobs>().expect("SiloJobs").1.insert(key, ammo);
            }
            (Some((_, id)), Some(before)) if ammo < before => {
                lua.app_data_mut::<SiloJobs>().expect("SiloJobs").1.remove(&key);
                {
                    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
                    if c.current(unit).is_some_and(|cmd| cmd.id == id) {
                        c.pop_front(unit);
                    }
                }
                if kind == SiloKind::Nuke
                    && let Some(u) = entity::object(lua, unit)? {
                    UnitCallbacks::on_nuke_launched(lua, &u, "unit")?;
                }
            }
            (None, Some(_)) => {
                lua.app_data_mut::<SiloJobs>().expect("SiloJobs").1.remove(&key);
            }
            _ => {}
        }
        let ordered = current.filter(|(k, _)| *k == build_order(kind));
        let wants = alive && (ordered.is_some() || auto) && ammo < max && !paused;
        let job = lua.app_data_ref::<SiloJobs>().and_then(|j| j.0.get(&key).copied());
        match job {
            // Stopped (order gone, auto off, full, paused, dead): drop it.
            Some(job) if !wants => {
                lua.app_data_mut::<SiloJobs>().expect("SiloJobs").0.remove(&key);
                lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").remove(job.event);
                if alive {
                    set_building(lua, unit, false);
                    if let (Some(u), Some(w)) = (entity::object(lua, unit)?, crate::engine::api::weapon::weapon_object(lua, unit, job.weapon)?) {
                        UnitCallbacks::on_silo_build_end(lua, &u, "unit", Obj::new(w))?;
                    }
                }
                // A full silo has nothing left to build for the order.
                if alive && ammo >= max
                    && let Some((_, id)) = ordered {
                    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
                    if c.current(unit).is_some_and(|cmd| cmd.id == id) {
                        c.pop_front(unit);
                    }
                }
            }
            Some(job) => {
                let (done, progress) = {
                    let ev = lua.app_data_ref::<EconomyEvents>().expect("EconomyEvents");
                    let e = ev.map.get(&job.event);
                    (e.is_none_or(|e| e.done()), e.map_or(Fx::ONE, |e| e.progress()))
                };
                if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
                    u.work_progress = progress;
                }
                if done {
                    lua.app_data_mut::<SiloJobs>().expect("SiloJobs").0.remove(&key);
                    lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").remove(job.event);
                    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
                        match kind {
                            SiloKind::Nuke => u.nuke_ammo += 1,
                            SiloKind::Tactical => u.tactical_ammo += 1,
                        }
                        u.work_progress = Fx::ZERO;
                    }
                    set_building(lua, unit, false);
                    let unit_obj = entity::object(lua, unit)?;
                    if let (Some(u), Some(w)) = (&unit_obj, crate::engine::api::weapon::weapon_object(lua, unit, job.weapon)?) {
                        UnitCallbacks::on_silo_build_end(lua, u, "unit", Obj::new(w))?;
                    }
                    if kind == SiloKind::Nuke
                        && let Some(u) = &unit_obj {
                        UnitCallbacks::on_nuke_armed(lua, u, "unit")?;
                    }
                    // A counted order counts down; the last one pops.
                    if let Some((_, id)) = ordered {
                        let mut c = lua.app_data_mut::<Commands>().expect("Commands");
                        if c.count_down(id) == 0 && c.current(unit).is_some_and(|cmd| cmd.id == id) {
                            c.pop_front(unit);
                        }
                    }
                }
            }
            None if wants => {
                let (energy, mass, build_time) = missile_cost(lua, &projectile)?;
                // FAF's script decides the rate and the adjacency discounts
                // ("used by the engine in silo build calculations",
                // `Unit.lua:1142`); fixed for the missile once started.
                let (build_rate, energy_mod, mass_mod) = match entity::object(lua, unit)? {
                    Some(u) => {
                        let call = |name: &str| -> LuaResult<Option<Fx>> {
                            Ok(match u.get::<Option<LuaFunction>>(name)? {
                                Some(f) => f.call::<Option<f64>>(&u)?.map(from_lua),
                                None => None,
                            })
                        };
                        (call("GetEconomyBuildRate")?.unwrap_or(build_rate), call("GetEnergyBuildAdjMod")?.unwrap_or(Fx::ONE), call("GetMassBuildAdjMod")?.unwrap_or(Fx::ONE))
                    }
                    None => (build_rate, Fx::ONE, Fx::ONE),
                };
                let (energy, mass) = (energy * energy_mod.max(Fx::ZERO), mass * mass_mod.max(Fx::ZERO));
                let seconds = if build_rate > Fx::ZERO { build_time / build_rate } else { build_time };
                let event = lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").create_fractional(unit, energy, mass, seconds);
                lua.app_data_mut::<SiloJobs>().expect("SiloJobs").0.insert(key, Job { weapon: index, event, banked: (Fx::ZERO, Fx::ZERO) });
                // A head start given while no missile was in progress.
                let head_start = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()).map_or(0, |u| std::mem::take(&mut u.silo_blocks));
                if head_start > 0 {
                    set_blocks(lua, unit, head_start);
                }
                set_building(lua, unit, true);
                if let (Some(u), Some(w)) = (entity::object(lua, unit)?, crate::engine::api::weapon::weapon_object(lua, unit, index)?) {
                    UnitCallbacks::on_silo_build_start(lua, &u, "unit", Obj::new(w))?;
                }
            }
            None => {}
        }
    }
    Ok(())
}
