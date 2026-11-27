//! The player's selection as the interface sees it, and the lists of units
//! its panels ask for.
//!
//! Selecting from the interface is synchronous, as in moho: the new
//! selection is what `GetSelectedUnits()` returns on the next line and what
//! selection orders given after it go to (FAF selects a factory, issues a
//! build and reselects in one frame: construction.lua, hotbuild.lua). The
//! engine adopts it as the player's selection at the end of the frame
//! (`UserVm::take_selection_request`), which then runs `OnSelectionChanged`.
//!
//! The selection is a list of unit objects (`__engine_selection`, nil when
//! empty), which the engine also sets from the renderer's
//! (`UserVm::set_selection`). The units' details are `units.lua`'s
//! (`__engine_unit_data`). Render-thread only.

use std::collections::BTreeSet;

use mlua::prelude::*;

use crate::engine::api::moho;
use crate::lua_api::shapes::{List, Loose};
use crate::lua_api::{Obj, lua_fn};
use crate::ui::shapes::truthy;

const SELECTION: &str = "__engine_selection";
const REQUEST: &str = "__engine_select_request";

type Units = List<Obj<moho::UserUnit>>;

/// Every value of a table, as `for _, v in t` visits them.
fn values(t: Option<&LuaTable>) -> LuaResult<Vec<LuaValue>> {
    let Some(t) = t else { return Ok(Vec::new()) };
    t.pairs::<LuaValue, LuaValue>().map(|p| p.map(|(_, v)| v)).collect()
}

/// The unit objects of entity ids (`GetUnitById`), leaving out those the
/// interface doesn't know.
fn units_by_id(lua: &Lua, ids: impl IntoIterator<Item = LuaValue>) -> LuaResult<LuaTable> {
    let by_id: LuaFunction = lua.globals().get("GetUnitById")?;
    let out = lua.create_table()?;
    for id in ids {
        let unit: LuaValue = by_id.call(id)?;
        if truthy(&unit) {
            out.raw_push(unit)?;
        }
    }
    Ok(out)
}

/// Makes `units` the selection: each living unit once, in entity id order.
fn select(lua: &Lua, units: Vec<LuaValue>) -> LuaResult<()> {
    let mut seen = BTreeSet::new();
    let mut list: Vec<(f64, LuaTable)> = Vec::new();
    for unit in units {
        let LuaValue::Table(unit) = unit else { continue };
        let id = match unit.raw_get::<LuaValue>("_id")? {
            LuaValue::Integer(i) => i as f64,
            LuaValue::Number(n) => n,
            _ => continue,
        };
        if seen.contains(&id.to_bits()) || unit.get::<LuaFunction>("IsDead")?.call::<bool>(&unit)? {
            continue;
        }
        seen.insert(id.to_bits());
        list.push((id, unit));
    }
    list.sort_by(|a, b| a.0.total_cmp(&b.0));
    let globals = lua.globals();
    if list.is_empty() {
        globals.raw_set(SELECTION, LuaNil)?;
    } else {
        globals.raw_set(SELECTION, lua.create_sequence_from(list.iter().map(|(_, u)| u))?)?;
    }
    globals.raw_set(REQUEST, lua.create_sequence_from(list.iter().map(|(_, u)| u))?)
}

/// The details (`__engine_unit_data`) of a unit object, if it has any.
fn detail(data: &LuaTable, unit: &LuaValue) -> LuaResult<Option<LuaTable>> {
    let LuaValue::Table(unit) = unit else { return Err(LuaError::runtime(format!("expected a unit, got {}", unit.type_name()))) };
    Ok(match data.get::<LuaValue>(unit.get::<LuaValue>("_id")?)? {
        LuaValue::Table(d) => Some(d),
        _ => None,
    })
}

/// How many of `units` have the detail `flag` set, and how many don't.
fn count(lua: &Lua, units: Option<&LuaTable>, flag: &str) -> LuaResult<(usize, usize)> {
    let data: LuaTable = lua.globals().raw_get("__engine_unit_data")?;
    let (mut set, mut unset) = (0, 0);
    for unit in values(units)? {
        match detail(&data, &unit)? {
            Some(d) if truthy(&d.get::<LuaValue>(flag)?) => set += 1,
            _ => unset += 1,
        }
    }
    Ok((set, unset))
}

/// The units listed (by entity id) in the detail `field` of any of
/// `units`, each once.
fn gather(lua: &Lua, units: Option<&LuaTable>, field: &str) -> LuaResult<LuaTable> {
    let data: LuaTable = lua.globals().raw_get("__engine_unit_data")?;
    let mut ids: Vec<LuaValue> = Vec::new();
    for unit in values(units)? {
        let Some(d) = detail(&data, &unit)? else { continue };
        let LuaValue::Table(list) = d.get::<LuaValue>(field)? else { continue };
        for id in values(Some(&list))? {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    units_by_id(lua, ids)
}

/// The focus army's idle, finished units in a category expression; nil
/// when none.
fn idle(lua: &Lua, category: &str) -> LuaResult<Option<Units>> {
    let ids: Vec<u32> = lua.globals().get::<LuaFunction>("__engine_idle_units")?.call(category)?;
    let units = units_by_id(lua, ids.into_iter().map(|id| LuaValue::Number(f64::from(id))))?;
    Ok((units.raw_len() > 0).then(|| List::new(units)))
}

/// Units added to the selection from the interface (a transport's carried
/// units in the construction panel), by entity id. Kept; orders don't go
/// to them yet.
#[derive(Default)]
struct ExtraSelection(BTreeSet<u32>);

fn extra(lua: &Lua) -> mlua::AppDataRefMut<'_, ExtraSelection> {
    if lua.app_data_ref::<ExtraSelection>().is_none() {
        lua.set_app_data(ExtraSelection::default());
    }
    lua.app_data_mut::<ExtraSelection>().expect("just set")
}

fn entity_id(unit: &Loose<Option<Obj<moho::UserUnit>>>) -> Option<u32> {
    match &**unit {
        LuaValue::Table(t) => t.raw_get::<Option<u32>>("_id").ok().flatten(),
        _ => None,
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(ExtraSelection::default());
    lua_fn!(lua, global,
        /// The player's selection: a new table each call, nil when nothing is
        /// selected, as moho's. FAF adds to the table it gets
        /// (`AppendSetToSelection`) and tells calls apart by table identity
        /// (selection sets' double tap).
        GetSelectedUnits = |lua, ()| {
            let Some(selection) = lua.globals().raw_get::<Option<LuaTable>>(SELECTION)? else { return Ok(None) };
            let units: Vec<LuaValue> = selection.sequence_values::<LuaValue>().collect::<LuaResult<_>>()?;
            if units.is_empty() {
                return Ok(None);
            }
            lua.create_sequence_from(units).map(|t| Some(Units::new(t)))
        })?;
    lua_fn!(lua, global,
        /// Selects these units (nil: none) at once. Dead units are left out.
        SelectUnits = |lua, units: Option<Units>| select(lua, values(units.as_deref())?))?;
    lua_fn!(lua, global,
        /// Adds these units to the selection.
        AddSelectUnits = |lua, selection: Option<Units>| {
            let mut all = values(lua.globals().raw_get::<Option<LuaTable>>(SELECTION)?.as_ref())?;
            all.extend(values(selection.as_deref())?);
            select(lua, all)
        })?;
    lua_fn!(lua, global,
        /// Selects the player's units in a category expression (moho's own
        /// rules: `__engine_select_by_category`), at once like `SelectUnits`.
        /// None found: the selection is left as it is.
        UISelectionByCategory = |lua, (expression, add_to_cur_sel, in_view_frustum, nearest_to_mouse, must_be_idle): (String, bool, bool, bool, bool)| {
            let pick: LuaFunction = lua.globals().get("__engine_select_by_category")?;
            let Some(ids) = pick.call::<Option<Vec<u32>>>((expression, add_to_cur_sel, in_view_frustum, nearest_to_mouse, must_be_idle))? else { return Ok(()) };
            let units = units_by_id(lua, ids.into_iter().map(|id| LuaValue::Number(f64::from(id))))?;
            select(lua, values(Some(&units))?)
        })?;
    lua_fn!(lua, global,
        AddToSessionExtraSelectList = |lua, unit: Loose<Option<Obj<moho::UserUnit>>>| {
            if let Some(id) = entity_id(&unit) {
                extra(lua).0.insert(id);
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        RemoveFromSessionExtraSelectList = |lua, unit: Loose<Option<Obj<moho::UserUnit>>>| {
            if let Some(id) = entity_id(&unit) {
                extra(lua).0.remove(&id);
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        ClearSessionExtraSelectList = |lua, ()| {
            extra(lua).0.clear();
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The focus army's idle engineers (not commanders), nil when none
        /// (avatars.lua).
        GetIdleEngineers = |lua, ()| idle(lua, "ENGINEER - COMMAND"))?;
    lua_fn!(lua, global,
        /// The focus army's idle factories (gates too: FAF filters those out
        /// itself), nil when none.
        GetIdleFactories = |lua, ()| idle(lua, "FACTORY STRUCTURE"))?;
    lua_fn!(lua, global,
        /// Any of the units is in auto mode.
        GetIsAutoMode = |lua, units: Option<Units>| Ok(count(lua, units.as_deref(), "autoMode")?.0 > 0))?;
    lua_fn!(lua, global,
        /// Any of the units surfaces by itself.
        GetIsAutoSurfaceMode = |lua, units: Option<Units>| Ok(count(lua, units.as_deref(), "autoSurface")?.0 > 0))?;
    lua_fn!(lua, global,
        /// -1 all submerged, 1 none, 0 a mix (or no units).
        GetIsSubmerged = |lua, units: Option<Units>| {
            Ok(match count(lua, units.as_deref(), "submerged")? {
                (1.., 0) => -1,
                (0, 1..) => 1,
                _ => 0,
            })
        })?;
    lua_fn!(lua, global,
        /// The units carried by any of `units`, each once.
        GetAttachedUnitsList = |lua, units: Option<Units>| gather(lua, units.as_deref(), "cargo").map(Units::new))?;
    lua_fn!(lua, global,
        /// The units assisting any of `units`, each once.
        GetAssistingUnitsList = |lua, units: Option<Units>| gather(lua, units.as_deref(), "assisters").map(Units::new))?;
    Ok(())
}
