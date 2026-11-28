//! Overcharge orders (moho): `IssueOverCharge` and the player's
//! `RULEUCC_Overcharge`.
//!
//! Moho runs an overcharge order as its attack task with the overcharge flag
//! (`CUnitAttackTargetTask`, MohoEngine.dll 0x101ded00..0x101e0060). As the
//! order becomes current, the task takes the unit's first weapon whose
//! blueprint has `OverChargeWeapon` and calls its script's
//! `OnEnableWeapon` (FAF's `OverchargeWeapon` turns itself on there, and
//! the main gun off when it can overcharge); a unit without one drops the
//! order. The weapon is pointed at the order's target and, once it's on
//! target, the task fires it itself (its `OnFire`; the script decides
//! whether a shot leaves, `CanOvercharge`), whatever its reload or fire
//! state, then waits 10 ticks and the order is done. An order that ends
//! before firing (cleared, target gone) calls `OnDisableWeapon`; one that
//! fired doesn't (the script's own `PauseOvercharge` does). FAF probe
//! `probe_auditgaps`: one shot 2 or 3 ticks after the order, the queue
//! empty 9 ticks after the shot; with too little energy, no shot and the
//! order gone about 11 ticks in.
//!
//! The overcharge damage and its energy drain are the projectile script's
//! (`OverchargeProjectile`), through `CreateEconomyEvent` and `WaitFor`.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::callbacks::WeaponCallbacks;
use crate::engine::api::{entity, moho};
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::weapon::{OrderedTarget, WeaponTarget, Weapons};
use crate::lua_api::Obj;

/// Ticks from the task firing the weapon to the order completing (the
/// task's wait after its fire call, MohoEngine.dll 0x101e0031).
pub const DONE_AFTER_FIRE: u64 = 10;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct Task {
    order: CommandId,
    weapon: u32,
    /// The tick the task fired the weapon.
    fired: Option<u64>,
}

/// Running overcharge orders by unit.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct OverchargeTasks(BTreeMap<EntityId, Task>);

impl OverchargeTasks {
    /// The weapon targets the running orders point their overcharge weapons
    /// at, for the weapon step.
    pub fn ordered<'a>(&'a self, commands: &'a Commands) -> impl Iterator<Item = (EntityId, OrderedTarget)> + 'a {
        self.0.iter().filter_map(|(&u, t)| {
            let target = match commands.current(u).filter(|c| c.id == t.order)?.target {
                CommandTarget::Entity(id) => WeaponTarget::Entity(id),
                CommandTarget::Ground(p) => WeaponTarget::Ground(p),
                CommandTarget::None => return None,
            };
            Some((u, OrderedTarget { target, silo: None, overcharge: Some(t.weapon), in_range: true }))
        })
    }

    /// The weapon step fired weapon (`unit`, `index`): the task's shot.
    pub fn fired(&mut self, unit: EntityId, index: u32, tick: u64) {
        if let Some(t) = self.0.get_mut(&unit).filter(|t| t.weapon == index && t.fired.is_none()) {
            t.fired = Some(tick);
        }
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(OverchargeTasks::default());
    Ok(())
}

fn weapon(lua: &Lua, unit: EntityId, index: u32) -> LuaResult<Option<Obj<moho::Weapon>>> {
    Ok(crate::engine::api::weapon::weapon_object(lua, unit, index)?.map(Obj::new))
}

/// Once a tick, before the weapon step: start overcharge orders that just
/// became current, complete those that fired long enough ago, and end those
/// whose order went away.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let Some(commands) = lua.app_data_ref::<Commands>() else { return Ok(()) };
    let now = crate::engine::api::sim_time::current_tick(lua);
    let mut ended: Vec<(EntityId, Task)> = Vec::new();
    let mut done: Vec<(EntityId, CommandId)> = Vec::new();
    let starting: Vec<(EntityId, CommandId)> = {
        let mut tasks = lua.app_data_mut::<OverchargeTasks>().expect("OverchargeTasks");
        tasks.0.retain(|&u, t| {
            if commands.current(u).is_none_or(|c| c.id != t.order) {
                ended.push((u, *t));
                return false;
            }
            if t.fired.is_some_and(|f| now >= f + DONE_AFTER_FIRE) {
                done.push((u, t.order));
                return false;
            }
            true
        });
        commands
            .units_doing(&[CommandKind::OverCharge])
            .into_iter()
            .filter_map(|u| commands.current(u).map(|c| (u, c.id)))
            .filter(|(u, id)| tasks.0.get(u).is_none_or(|t| t.order != *id))
            .collect()
    };
    drop(commands);
    if !done.is_empty() {
        let mut c = lua.app_data_mut::<Commands>().expect("Commands");
        for (u, id) in done {
            if c.current(u).is_some_and(|cmd| cmd.id == id) {
                c.pop_front(u);
            }
        }
    }
    for (unit, t) in ended {
        let alive = entity::world(lua).get(unit).is_some_and(|e| !e.destroyed);
        if alive && t.fired.is_none()
            && let Some(w) = weapon(lua, unit, t.weapon)? {
            WeaponCallbacks::on_disable_weapon(lua, &w, "weapon")?;
        }
    }
    for (unit, order) in starting {
        let index = lua.app_data_ref::<Weapons>().and_then(|w| w.of(unit).iter().find(|s| s.overcharge).map(|s| s.index));
        let Some(index) = index else {
            // No overcharge weapon: the task gives up at once.
            let mut c = lua.app_data_mut::<Commands>().expect("Commands");
            if c.current(unit).is_some_and(|cmd| cmd.id == order) {
                c.pop_front(unit);
            }
            continue;
        };
        lua.app_data_mut::<OverchargeTasks>().expect("OverchargeTasks").0.insert(unit, Task { order, weapon: index, fired: None });
        if let Some(w) = weapon(lua, unit, index)? {
            WeaponCallbacks::on_enable_weapon(lua, &w, "weapon")?;
        }
    }
    Ok(())
}
