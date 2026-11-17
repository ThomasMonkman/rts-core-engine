//! Sacrifice orders (moho): `IssueSacrifice(units, target)` (Aeon engineers
//! and the ACU-class units with `RULEUCC_Sacrifice`).
//!
//! A unit whose current order is a sacrifice into a unit under construction
//! walks up to it (movement: to [`REACH`] of its skirt), then
//! `OnStartSacrifice(target)`; [`SACRIFICE_TICKS`] later it gives itself up:
//! the target's build progress (and health) rises by `min(mass *
//! SacrificeMassMult / targetMass, energy * SacrificeEnergyMult /
//! targetEnergy)` (FAF's `CalculateSacrificeBonus`, blueprint costs),
//! completing it if that reaches 1, then `OnStopSacrifice(target)` (FAF
//! destroys the sacrificer). A target that is gone or already finished ends
//! the order.
//!
//! The walk and the 9 ticks are the real game's (dev/probes
//! probe_unit_build G8: an Aeon engineer 4, 4 from a T1 generator's site
//! walks 12 ticks, `OnStartSacrifice`, then `OnStopSacrifice` 9 ticks later,
//! the target gaining only then). Inferred: the reach (about 2 from the
//! footprint's edge, as the real engineers reclaim and capture from), the
//! 9 ticks for every unit (one case), a sacrifice whose order goes away
//! before it ends just stopping, and blueprint costs (FAF's own helper adds
//! enhancements).

use mlua::prelude::*;

use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::entity;
use crate::lua_api::Obj;
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::math::{from_lua, Fx};

/// How close to the target's skirt a sacrificer walks (movement's goal for
/// the order) and must be to start.
pub const REACH: Fx = Fx::const_from_int(2);

/// Ticks from `OnStartSacrifice` to the sacrifice.
const SACRIFICE_TICKS: u64 = 9;

/// Sacrifices under way: the sacrificer's order and the tick it ends.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Sacrifices(std::collections::BTreeMap<EntityId, (CommandId, u64)>);

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Sacrifices::default());
    Ok(())
}

/// Blueprint `Economy` (BuildCostMass, BuildCostEnergy, SacrificeMassMult,
/// SacrificeEnergyMult) of a unit.
fn economy(lua: &Lua, unit: EntityId) -> LuaResult<(f64, f64, f64, f64)> {
    let bp_id = entity::world(lua).get(unit).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
    let bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)?;
    Ok(match bp.get::<Option<LuaTable>>("Economy")? {
        Some(e) => (
            e.get::<Option<f64>>("BuildCostMass")?.unwrap_or(0.0),
            e.get::<Option<f64>>("BuildCostEnergy")?.unwrap_or(0.0),
            e.get::<Option<f64>>("SacrificeMassMult")?.unwrap_or(0.0),
            e.get::<Option<f64>>("SacrificeEnergyMult")?.unwrap_or(0.0),
        ),
        None => (0.0, 0.0, 0.0, 0.0),
    })
}

/// Once a tick (after movement): sacrificers in reach of their target.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let now = crate::engine::api::sim_time::current_tick(lua);
    let orders: Vec<(EntityId, CommandId, CommandTarget)> = match lua.app_data_ref::<Commands>() {
        Some(c) => c
            .units_doing(&[CommandKind::Sacrifice])
            .into_iter()
            .filter_map(|u| c.current(u).map(|cmd| (u, cmd.id, cmd.target)))
            .collect(),
        None => return Ok(()),
    };
    // Sacrifices whose order went away stop.
    if let Some(mut s) = lua.app_data_mut::<Sacrifices>()
        && !s.0.is_empty() {
        s.0.retain(|u, (cmd, _)| orders.iter().any(|&(o, id, _)| o == *u && id == *cmd));
    }
    for (unit, id, target) in orders {
        let pop = |lua: &Lua| {
            if let Some(mut s) = lua.app_data_mut::<Sacrifices>() {
                s.0.remove(&unit);
            }
            let mut c = lua.app_data_mut::<Commands>().expect("Commands");
            if c.current(unit).is_some_and(|cmd| cmd.id == id) {
                c.pop_front(unit);
            }
        };
        let CommandTarget::Entity(target) = target else {
            pop(lua);
            continue;
        };
        let (in_reach, valid) = {
            let w = entity::world(lua);
            let (Some(u), Some(t)) = (w.get(unit).filter(|e| !e.destroyed), w.get(target).filter(|e| !e.destroyed)) else {
                drop(w);
                pop(lua);
                continue;
            };
            // (A little past the walk's goal, which it stops at.)
            (t.in_build_range(u.position(), REACH + Fx::from_num(0.5)), t.fraction_complete < Fx::ONE)
        };
        if !valid {
            pop(lua);
            continue;
        }
        let (Some(u_obj), Some(t_obj)) = (entity::object(lua, unit)?, entity::object(lua, target)?) else { continue };
        let due = lua.app_data_ref::<Sacrifices>().and_then(|s| s.0.get(&unit).map(|&(_, due)| due));
        match due {
            None if in_reach => {
                lua.app_data_mut::<Sacrifices>().expect("Sacrifices").0.insert(unit, (id, now + SACRIFICE_TICKS));
                UnitCallbacks::on_start_sacrifice(lua, &u_obj, "unit", Obj::new(t_obj))?;
                continue;
            }
            Some(due) if now >= due => {}
            _ => continue,
        }
        pop(lua);
        let (mass, energy, mass_mult, energy_mult) = economy(lua, unit)?;
        let (t_mass, t_energy, ..) = economy(lua, target)?;
        let ratio = |v: f64, of: f64| if of > 0.0 { v / of } else { f64::INFINITY };
        let bonus = from_lua(ratio(mass * mass_mult, t_mass).min(ratio(energy * energy_mult, t_energy)).min(1.0));
        let completed = {
            let mut w = entity::world(lua);
            match w.get_mut(target).filter(|t| !t.destroyed && t.fraction_complete < Fx::ONE) {
                Some(t) => {
                    let old = t.fraction_complete;
                    let new = (old + bonus).min(Fx::ONE);
                    t.fraction_complete = new;
                    t.health = (t.health + t.max_health * (new - old)).min(t.max_health);
                    new >= Fx::ONE
                }
                None => false,
            }
        };
        if completed {
            crate::engine::api::construction::complete_without_builder(lua, target, &t_obj, &u_obj)?;
        }
        UnitCallbacks::on_stop_sacrifice(lua, &u_obj, "unit", Obj::new(t_obj))?;
    }
    Ok(())
}
