//! Order resolution: ends orders whose purpose is gone, before the systems
//! that execute orders (movement, construction, weapons) run.
//!
//! - Attack / OverCharge / Guard / Repair / Reclaim / Capture on an entity
//!   that no longer exists (or was destroyed) complete.
//! - Attack orders on an entity no longer hostile complete.
//! - Pause / KillSelf / DestroySelf act at once and pop ([`instant`]).
//!
//! The dispatch hold ([`held`]): moho's dispatcher won't start a unit's
//! next order while the unit is being built, dead, `Attached` or
//! `BlockCommandQueue` (FAF sets that for a factory's roll-off and an ACU's
//! warp-in). The systems that start an order (construction, the instant
//! orders, teleport, script tasks, special actions, dives) check
//! it; one already running carries on. Movement acts only on an order the
//! dispatcher has started ([`dispatch`], `Commands::dispatched`).

use std::sync::LazyLock;

use mlua::prelude::*;

use crate::engine::api::entity;
use crate::engine::sim::army::{Alliance, Armies};
use crate::engine::sim::command::{CommandId, CommandKind as K, CommandTarget, Commands};
use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::Fx;
use crate::engine::sim::unit_state::UnitStates;

static HOLD_STATES: LazyLock<u64> = LazyLock::new(|| UnitStates::mask(&["Attached", "BlockCommandQueue"]));

/// Whether moho's dispatcher holds `unit`'s next order: it won't start one
/// while the unit is being built, dead, `Attached` or `BlockCommandQueue`
/// (`IAiCommandDispatchImpl::TaskTick`, MohoEngine.dll 0x101893a0: the
/// `IsBeingBuilt` / `IsDead` / `IsUnitState(14)` / `IsUnitState(42)` checks
/// at 0x10189486..0x101894c0 return before the next command's task is
/// made). Tasks already running carry on: only starting is held. A unit
/// that doesn't exist is held too (there's nothing to start).
pub(crate) fn held(world: &EntityWorld, unit: EntityId) -> bool {
    match world.get(unit) {
        Some(e) => e.destroyed || e.cold.killed || e.fraction_complete < Fx::ONE || e.unit.as_deref().is_some_and(|u| u.states.any(*HOLD_STATES)),
        None => true,
    }
}

pub fn tick(lua: &Lua) -> LuaResult<()> {
    dispatch(lua);
    resolve(lua);
    instant(lua)
}

/// Starts each new current command whose unit isn't held ([`held`]):
/// movement acts only on started ones (`Commands::dispatched`), so a held
/// unit's new move waits, as in the real game (`probe_queue`: a tank
/// ordered to move while `BlockCommandQueue` moves only once unblocked).
/// An attack order's weapon reset (moho's `SetDesiredTarget`) is the
/// weapon step's (`sim::weapon::advance_target`, `WeaponState::order_seen`).
fn dispatch(lua: &Lua) {
    let (Some(world), Some(mut commands)) = (lua.app_data_ref::<EntityWorld>(), lua.app_data_mut::<Commands>()) else { return };
    for u in commands.undispatched() {
        if held(&world, u) {
            commands.hold(u);
            continue;
        }
        commands.dispatch(u);
    }
    let now = crate::engine::api::sim_time::current_tick(lua);
    commands.cycle_ground_attacks(now);
}

/// Holds the new current commands of held units ([`held`]) before
/// movement, without starting any: an order a Lua thread gives this tick
/// (the script stage runs after dispatch) mustn't move a blocked unit for
/// a tick before the next dispatch holds it. Unheld ones still act at once.
pub fn hold_new(lua: &Lua) {
    let (Some(world), Some(mut commands)) = (lua.app_data_ref::<EntityWorld>(), lua.app_data_mut::<Commands>()) else { return };
    for u in commands.undispatched() {
        if held(&world, u) {
            commands.hold(u);
        }
    }
}

/// Moho's `IAiAttacker::AttackerStop` (MohoEngine.dll 0x10168390: the
/// attacker's desired target set to none, vt+0x20): the unit's weapons
/// drop their targets and search again. `IssueClearCommands` does it at
/// once (0x102cc466), a Stop order at its dispatch (0x101f5420); the real
/// game's weapon loses its target then (`probe_queue`).
pub(crate) fn attacker_stop(lua: &Lua, unit: EntityId) {
    let Some(mut weapons) = lua.app_data_mut::<crate::engine::sim::weapon::Weapons>() else { return };
    let indices: Vec<u32> = weapons.of(unit).iter().map(|w| w.index).collect();
    for i in indices {
        let w = weapons.get_mut(unit, i);
        w.target = None;
        w.target_from_order = false;
        w.check_countdown = 0;
    }
}

/// How long an attack on the ground goes on after its weapon fires before
/// the task ends (state 5 of moho's attack task waits 10 ticks, then ends:
/// MohoEngine.dll 0x101e002a..0x101e0053).
const GROUND_ATTACK_LINGER: u64 = 10;

/// A unit's weapon fired: an attack on the ground it's doing ends
/// [`GROUND_ATTACK_LINGER`] ticks on (and, with another command behind it,
/// goes to the back: `Commands::cycle_ground_attacks`).
pub(crate) fn weapon_fired(lua: &Lua, unit: EntityId) {
    let now = crate::engine::api::sim_time::current_tick(lua);
    let Some(mut c) = lua.app_data_mut::<Commands>() else { return };
    let Some(id) = c.dispatched(unit).filter(|cmd| matches!(cmd.kind, K::Attack | K::FormAttack) && matches!(cmd.target, CommandTarget::Ground(_))).map(|cmd| cmd.id) else { return };
    c.note_ground_fire(unit, id, now + GROUND_ATTACK_LINGER);
}

/// Ends orders whose purpose is gone.
fn resolve(lua: &Lua) {
    let (Some(world), Some(mut commands)) = (lua.app_data_ref::<EntityWorld>(), lua.app_data_mut::<Commands>()) else {
        return;
    };
    let armies = lua.app_data_ref::<Armies>();
    let done: Vec<EntityId> = commands
        .units_doing(&[K::Attack, K::FormAttack, K::OverCharge, K::Guard, K::Repair, K::Reclaim, K::Capture])
        .into_iter()
        .filter(|&u| {
            let Some(cmd) = commands.current(u) else { return false };
            let CommandTarget::Entity(t) = cmd.target else { return false };
            let targets_entity = matches!(cmd.kind, K::Attack | K::FormAttack | K::OverCharge | K::Guard | K::Repair | K::Reclaim | K::Capture);
            if !targets_entity {
                return false;
            }
            let Some(target) = world.get(t).filter(|e| !e.destroyed) else { return true };
            // An attack target that became friendly is no longer a target.
            matches!(cmd.kind, K::Attack | K::FormAttack)
                && match (world.get(u).and_then(|e| e.army), target.army, &armies) {
                    (Some(a), Some(b), Some(armies)) => armies.alliance(a, b) == Alliance::Ally,
                    _ => false,
                }
        })
        .collect();
    for u in done {
        commands.pop_front(u);
    }
}

/// Stop / Pause / KillSelf / DestroySelf as moho's dispatch runs them (the switch
/// at 0x101f4000, table 0x101f5378): no task, so the order pops and the
/// unit acts at once. Pause toggles `SetPaused(!paused)` (0x101f4026),
/// KillSelf is `Kill(self, "Damage", 0)` (0x101f534d -> 0x101f65f0),
/// DestroySelf `Entity::Destroy` (0x101f5356), Stop stops the attacker
/// ([`attacker_stop`], 0x101f401a -> 0x101f5420).
fn instant(lua: &Lua) -> LuaResult<()> {
    let orders: Vec<(EntityId, CommandId, K)> = {
        let (Some(world), Some(c)) = (lua.app_data_ref::<EntityWorld>(), lua.app_data_ref::<Commands>()) else { return Ok(()) };
        c.units_doing(&[K::Stop, K::KillSelf, K::DestroySelf, K::Pause])
            .into_iter()
            .filter(|&u| !held(&world, u))
            .filter_map(|u| c.current(u).map(|cmd| (u, cmd.id, cmd.kind)))
            .collect()
    };
    for (unit, id, kind) in orders {
        {
            let mut c = lua.app_data_mut::<Commands>().expect("Commands");
            if c.current(unit).is_none_or(|cmd| cmd.id != id) {
                continue;
            }
            c.pop_front(unit);
        }
        let Some(obj) = entity::object(lua, unit)? else { continue };
        match kind {
            K::Pause => {
                let paused = entity::world(lua).get(unit).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.paused);
                obj.call_method::<()>("SetPaused", !paused)?;
            }
            K::Stop => attacker_stop(lua, unit),
            K::KillSelf => obj.call_method::<()>("Kill", (obj.clone(), "Damage", 0.0))?,
            _ => obj.call_method::<()>("Destroy", ())?,
        }
    }
    Ok(())
}
