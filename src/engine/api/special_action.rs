//! Special action orders (moho `UNITCOMMAND_SpecialAction`, the interface's
//! `RULEUCC_SpecialAction` cursor): no `Issue*` function, only player
//! orders to a point. A unit whose current order is one gets
//! `OnSpecialAction(location)` and the order pops. No FAF unit has the
//! command cap; mods use the hook.
//!
//! Confirmed in the binary: the dispatch (MohoEngine.dll 0x101f4629) calls
//! `OnSpecialAction(target position)` with no task, so the unit acts where
//! it stands, at once.

use mlua::prelude::*;

use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::entity;
use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};

/// Once a tick: units whose current order is a special action.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let orders: Vec<_> = match lua.app_data_ref::<Commands>() {
        Some(c) => c
            .units_doing(&[CommandKind::SpecialAction])
            .into_iter()
            .filter(|&u| !crate::engine::api::orders::held(&entity::world(lua), u))
            .filter_map(|u| c.current(u).map(|cmd| (u, cmd.id, cmd.target)))
            .collect(),
        None => return Ok(()),
    };
    for (unit, id, target) in orders {
        {
            let mut c = lua.app_data_mut::<Commands>().expect("Commands");
            if c.current(unit).is_some_and(|cmd| cmd.id == id) {
                c.pop_front(unit);
            }
        }
        let location = match target {
            CommandTarget::Ground(p) => p,
            CommandTarget::Entity(e) => match entity::world(lua).get(e) {
                Some(e) => e.position(),
                None => continue,
            },
            CommandTarget::None => continue,
        };
        if let Some(obj) = entity::object(lua, unit)? {
            UnitCallbacks::on_special_action(lua, &obj, "unit", entity::vector(lua, location)?)?;
        }
    }
    Ok(())
}
