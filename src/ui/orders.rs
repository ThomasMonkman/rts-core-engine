//! Orders and unit settings given from the interface (its buttons, panels
//! and hotkeys): converted as they are given and queued in the VM's app
//! data, oldest first, for the engine to turn into player commands
//! (`UserVm::take_orders`). `SimCallback`s (`user_api`) join the same queue,
//! so everything reaches the sim in the order given. Render-thread only.
//!
//! Orders to "the selection" go to the selection as it is when given
//! (`SelectUnits` is synchronous), not at the end of the frame. A list of
//! units given as nil leaves the order to the selection when it is taken.

use mlua::prelude::*;

use crate::engine::api::moho;
use crate::engine::sim::input::Toggle;
use crate::lua_api::shapes::{FireState, List, Loose};
use crate::lua_api::{Obj, lua_fn};
use crate::ui::UiOrder;

/// The orders given since they were last taken.
#[derive(Default)]
struct Pending(Vec<UiOrder>);

fn pending(lua: &Lua) -> mlua::AppDataRefMut<'_, Pending> {
    if lua.app_data_ref::<Pending>().is_none() {
        lua.set_app_data(Pending::default());
    }
    lua.app_data_mut::<Pending>().expect("just set")
}

/// Queues an order.
pub fn push(lua: &Lua, order: UiOrder) {
    pending(lua).0.push(order);
}

/// The orders given since the last call, oldest first.
pub fn take(lua: &Lua) -> Vec<UiOrder> {
    std::mem::take(&mut pending(lua).0)
}

/// The entity ids of a list of unit objects (anything else in it is skipped).
pub fn ids(units: &LuaTable) -> Vec<u32> {
    units.sequence_values::<LuaTable>().filter_map(|u| u.ok()?.raw_get::<Option<u32>>("_id").ok().flatten()).collect()
}

/// The selection as the interface sees it now.
pub fn selection(lua: &Lua) -> LuaResult<Vec<u32>> {
    Ok(lua.globals().raw_get::<Option<LuaTable>>("__engine_selection")?.map(|t| ids(&t)).unwrap_or_default())
}

type Units = Option<List<Obj<moho::UserUnit>>>;

fn listed(units: &Units) -> Option<Vec<u32>> {
    units.as_ref().map(|t| ids(t))
}

/// One unit as a list (none when it isn't a unit object).
fn one(unit: &LuaValue) -> Vec<u32> {
    match unit {
        LuaValue::Table(t) => t.raw_get::<Option<u32>>("_id").ok().flatten().into_iter().collect(),
        _ => Vec::new(),
    }
}

/// A script order's string fields, by key.
fn script(params: &LuaValue) -> Vec<(String, String)> {
    let LuaValue::Table(t) = params else { return Vec::new() };
    let mut kv: Vec<(String, String)> = t
        .pairs::<String, LuaValue>()
        .filter_map(|p| p.ok())
        .filter_map(|(k, v)| match v {
            LuaValue::String(s) => Some((k, s.to_str().ok()?.to_string())),
            _ => None,
        })
        .collect();
    kv.sort();
    kv
}

fn order(units: Option<Vec<u32>>, command: Option<String>, params: &LuaValue, clear: Option<bool>) -> UiOrder {
    UiOrder { command: command.unwrap_or_default(), units, count: 1, clear: clear.unwrap_or(false), script: script(params), ..Default::default() }
}

fn build(units: Option<Vec<u32>>, command: Option<String>, blueprint: Option<String>, count: Option<f64>, clear: Option<bool>) -> UiOrder {
    UiOrder { command: command.unwrap_or_default(), units, blueprint, count: count.map_or(1, |c| c.max(1.0) as u32), clear: clear.unwrap_or(false), ..Default::default() }
}

fn setting(units: Option<Vec<u32>>, toggle: Toggle) -> UiOrder {
    UiOrder { units, count: 1, toggle: Some(toggle), ..Default::default() }
}

/// A fire state's index: its name, else the number it is (or reads as).
fn fire_index(lua: &Lua, state: FireState) -> LuaResult<Option<f64>> {
    let state = state.into_inner();
    if let LuaValue::String(s) = &state {
        match &*s.as_bytes() {
            b"ReturnFire" => return Ok(Some(0.0)),
            b"HoldFire" => return Ok(Some(1.0)),
            b"HoldGround" => return Ok(Some(2.0)),
            _ => {}
        }
    }
    lua.coerce_number(state)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Pending::default());
    lua_fn!(lua, global,
        /// `RULEUCC_Move` to `UNITCOMMAND_Move`.
        GetUnitCommandFromCommandCap = |_, cap: String| Ok(format!("UNITCOMMAND_{}", cap.strip_prefix("RULEUCC_").unwrap_or(&cap))))?;
    lua_fn!(lua, global,
        /// An order for the selection. `lua_params`: a script order's table.
        IssueCommand = |lua, (command, lua_params, clear): (Option<String>, LuaValue, Option<bool>)| {
            let units = selection(lua)?;
            push(lua, order(Some(units), command, &lua_params, clear));
            Ok(())
        })?;
    lua_fn!(lua, global,
        IssueUnitCommand = |lua, (units, command, lua_params, clear): (Units, Option<String>, LuaValue, Option<bool>)| {
            push(lua, order(listed(&units), command, &lua_params, clear));
            Ok(())
        })?;
    lua_fn!(lua, global,
        IssueUnitCommandToUnit = |lua, (unit, command, lua_params, clear): (Loose<Obj<moho::UserUnit>>, Option<String>, LuaValue, Option<bool>)| {
            push(lua, order(Some(one(&unit)), command, &lua_params, clear));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// A build order for the selection (a factory's build, an upgrade):
        /// `count` of `blueprintid` (at least 1).
        IssueBlueprintCommand = |lua, (command, blueprintid, count, clear): (Option<String>, Option<String>, Option<f64>, Option<bool>)| {
            let units = selection(lua)?;
            push(lua, build(Some(units), command, blueprintid, count, clear));
            Ok(())
        })?;
    lua_fn!(lua, global,
        IssueBlueprintCommandToUnits = |lua, (units, command, blueprintid, count, clear): (Units, Option<String>, Option<String>, Option<f64>, Option<bool>)| {
            push(lua, build(listed(&units), command, blueprintid, count, clear));
            Ok(())
        })?;
    lua_fn!(lua, global,
        IssueBlueprintCommandToUnit = |lua, (unit, command, blueprintid, count, clear): (Loose<Obj<moho::UserUnit>>, Option<String>, Option<String>, Option<f64>, Option<bool>)| {
            push(lua, build(Some(one(&unit)), command, blueprintid, count, clear));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Sends the selection to dock.
        IssueDockCommand = |lua, clear: Option<bool>| {
            let units = selection(lua)?;
            push(lua, order(Some(units), Some("UNITCOMMAND_Dock".into()), &LuaNil, clear));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Anything that isn't a fire state: `ReturnFire`.
        SetFireState = |lua, (units, fire_state): (Units, FireState)| {
            let state = fire_index(lua, fire_state)?.unwrap_or(0.0);
            push(lua, setting(listed(&units), Toggle::FireState(state as i32)));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `fire_state` is the units' current fire state (FAF's cycle hotkey
        /// passes `GetFireState`, -1 for a mix): they move on to the next of
        /// the three, as the sim's `Unit:ToggleFireState` (probe_ui_units C).
        ToggleFireState = |lua, (units, fire_state): (Units, FireState)| {
            let current = fire_index(lua, fire_state)?.unwrap_or(-1.0);
            push(lua, setting(listed(&units), Toggle::FireState(((current + 1.0) % 3.0) as i32)));
            Ok(())
        })?;
    lua_fn!(lua, global,
        SetPaused = |lua, (units, pause): (Units, bool)| {
            push(lua, setting(listed(&units), Toggle::Paused(pause)));
            Ok(())
        })?;
    lua_fn!(lua, global,
        SetPausedOfUnit = |lua, (unit, pause): (Loose<Obj<moho::UserUnit>>, bool)| {
            push(lua, setting(Some(one(&unit)), Toggle::Paused(pause)));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `state` is the bit's current state: it flips.
        ToggleScriptBit = |lua, (units, bit, state): (Units, Option<f64>, bool)| {
            push(lua, setting(listed(&units), Toggle::ScriptBit(bit.unwrap_or(0.0) as u8, !state)));
            Ok(())
        })?;
    lua_fn!(lua, global,
        SetAutoMode = |lua, (units, mode): (Units, bool)| {
            push(lua, setting(listed(&units), Toggle::AutoMode(mode)));
            Ok(())
        })?;
    lua_fn!(lua, global,
        SetAutoSurfaceMode = |lua, (units, mode): (Units, bool)| {
            push(lua, setting(listed(&units), Toggle::AutoSurfaceMode(mode)));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// A setting of one unit, for the unit methods written in Luau
        /// (`units.lua`): `repeat` (the build queue repeats), `vo` (the busy,
        /// else the missing, staging platform voice-over) or `count`
        /// (`value` more or fewer of the build at queue `index`).
        #[lua(internal)]
        __engine_unit_setting = |lua, (unit, kind, value, index): (Loose<Obj<moho::UserUnit>>, String, f64, Option<f64>)| {
            let toggle = match kind.as_str() {
                "repeat" => Toggle::RepeatQueue(value != 0.0),
                "vo" => Toggle::StagingVo(value != 0.0),
                "count" => Toggle::BuildCount(index.unwrap_or(0.0) as u16, value as i32),
                _ => return Err(LuaError::runtime(format!("__engine_unit_setting: unknown setting '{kind}'"))),
            };
            push(lua, setting(Some(one(&unit)), toggle));
            Ok(())
        })?;
    Ok(())
}
