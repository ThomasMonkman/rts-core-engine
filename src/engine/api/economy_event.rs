//! `CreateEconomyEvent`, `EconomyEventIsDone`, `RemoveEconomyEvent`
//! (`sim::economy::EconomyEvents`). After each economy pass, an event that
//! advanced calls its Lua `callback(unit, progress)` (progress in (0, 1]),
//! in event id order.

use crate::lua_api::Obj;
use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::entity::{entity_id, object};
use crate::engine::api::{moho, script_object};
use crate::engine::sim::economy::EconomyEvents;
use crate::engine::sim::math::{from_lua, Fx};
use crate::lua_api::lua_fn;

#[derive(Clone, Copy, Debug)]
pub struct EconomyEventHandle(pub u32);
impl LuaUserData for EconomyEventHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

/// Progress last reported to each event's callback.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Reported(BTreeMap<u32, Fx>);

const CALLBACKS: &str = "economy_event_callbacks";

fn callbacks(lua: &Lua) -> LuaResult<LuaTable> {
    lua.named_registry_value(CALLBACKS)
}

fn event_id(v: &LuaTable) -> LuaResult<u32> {
    Ok(script_object::handle::<EconomyEventHandle>(v)?.0)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(EconomyEvents::default());
    lua.set_app_data(Reported::default());
    lua.set_named_registry_value(CALLBACKS, lua.create_table()?)?;
    lua_fn!(lua, global,
        /// Starts an economy event: spend `energy` and `mass` over `seconds` for `unit`.
        /// `callback(unit, progress)` runs after each economy pass in which it
        /// advanced (progress in (0, 1]). Returns the event handle.
        CreateEconomyEvent = |lua, (unit, energy, mass, seconds, callback): (Obj<moho::Unit>, f64, f64, f64, Option<LuaFunction>)| {
            let unit = entity_id(&unit)?;
            let id = lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").create(unit, from_lua(energy), from_lua(mass), from_lua(seconds));
            if let Some(f) = callback {
                callbacks(lua)?.raw_set(id, f)?;
            }
            script_object::create(lua, EconomyEventHandle(id), moho::class_table(lua, "EconomyEvent")?).map(Obj::<moho::EconomyEvent>::new)
        })?;
    lua_fn!(lua, global,
        EconomyEventIsDone = |lua, ev: Obj<moho::EconomyEvent>| Ok(lua.app_data_ref::<EconomyEvents>().expect("EconomyEvents").is_done(event_id(&ev)?)))?;
    lua_fn!(lua, global,
        RemoveEconomyEvent = |lua, (_unit, ev): (LuaValue, Option<Obj<moho::EconomyEvent>>)| {
            let Some(ev) = ev else { return Ok(()) };
            let id = event_id(&ev)?;
            lua.app_data_mut::<EconomyEvents>().expect("EconomyEvents").remove(id);
            lua.app_data_mut::<Reported>().expect("Reported").0.remove(&id);
            callbacks(lua)?.raw_set(id, LuaNil)?;
            Ok(())
        })?;
    Ok(())
}

/// After the economy pass: progress callbacks of events that advanced.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let advanced: Vec<(u32, crate::engine::sim::entity::EntityId, Fx)> = {
        let (Some(events), Some(mut reported)) = (lua.app_data_ref::<EconomyEvents>(), lua.app_data_mut::<Reported>()) else { return Ok(()) };
        reported.0.retain(|id, _| events.map.contains_key(id));
        events
            .map
            .iter()
            .filter_map(|(&id, ev)| {
                let p = ev.progress();
                let last = reported.0.insert(id, p).unwrap_or(Fx::ZERO);
                (p > last).then_some((id, ev.unit, p))
            })
            .collect()
    };
    if advanced.is_empty() {
        return Ok(());
    }
    let cbs = callbacks(lua)?;
    for (id, unit, p) in advanced {
        let Some(f) = cbs.raw_get::<Option<LuaFunction>>(id)? else { continue };
        let Some(obj) = object(lua, unit)? else { continue };
        if let Err(e) = f.call::<()>((obj, p.to_num::<f64>())) {
            tracing::warn!(target: "lua", "Error running economy event callback: {e}");
        }
    }
    Ok(())
}
