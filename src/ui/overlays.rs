//! Range overlay filters (the military / intel rings): FAF defines filters
//! by name (`SetOverlayFilter`: the units it covers and how its rings look)
//! and then chooses the active set (`SetOverlayFilters`). The renderer takes
//! the active ones when they change (`UserVm::take_overlay_filters`).
//! Render-thread only.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::categories::{self, EntityCategory};
use crate::engine::sim::category::CategorySet;
use crate::lua_api::lua_fn;
use crate::lua_api::shapes::List;
use crate::ui::OverlayFilter;

/// A filter as FAF defined it.
#[derive(Clone)]
struct Filter {
    /// The units it covers (none given: the filter is never shown).
    categories: Option<CategorySet>,
    /// Normal, selected and rollover colours.
    colors: [Option<String>; 3],
    inner: [f32; 2],
    outer: [f32; 2],
}

/// The overlay filters (app data of the user VM).
#[derive(Default)]
struct Overlays {
    filters: BTreeMap<String, Filter>,
    /// The names chosen, in the order given.
    active: Vec<String>,
    /// Moves with every change.
    version: u64,
    /// The version, and how many blueprints, the renderer last took.
    seen: Option<(u64, usize)>,
}

fn state(lua: &Lua) -> mlua::AppDataRefMut<'_, Overlays> {
    if lua.app_data_ref::<Overlays>().is_none() {
        lua.set_app_data(Overlays::default());
    }
    lua.app_data_mut::<Overlays>().expect("just set")
}

/// The active filters when they, or the number of `blueprints`, changed
/// since the last call (`None`: unchanged): colours, thicknesses, which of
/// `blueprints` each covers, and which range kinds (`OVERLAY<kind>`
/// categories) it takes in: those whose every blueprint it covers, so the
/// anti-air filter doesn't also draw a destroyer's guns.
pub fn take_changed(lua: &Lua, blueprints: &[String]) -> LuaResult<Option<Vec<OverlayFilter>>> {
    let active: Vec<(String, Filter, CategorySet)> = {
        let mut s = state(lua);
        let key = (s.version, blueprints.len());
        if s.seen == Some(key) {
            return Ok(None);
        }
        s.seen = Some(key);
        s.active
            .iter()
            .filter_map(|name| {
                let f = s.filters.get(name)?;
                Some((name.clone(), f.clone(), f.categories.clone()?))
            })
            .collect()
    };
    if active.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let domain = categories::domain(lua)?;
    let indices: Vec<Option<u32>> = blueprints.iter().map(|id| domain.index_of(id)).collect();
    let covers = |set: &CategorySet| -> Vec<bool> { indices.iter().map(|i| i.is_some_and(|i| set.contains(i))).collect() };
    // Each range kind's blueprints (`None`: no such category).
    let by_name: Option<LuaTable> = lua.globals().get("categories")?;
    let mut kinds: Vec<Option<Vec<bool>>> = Vec::new();
    for kind in crate::engine::snapshot::RANGE_KINDS {
        let value = match &by_name {
            Some(t) => t.get::<LuaValue>(format!("OVERLAY{kind}"))?,
            None => LuaNil,
        };
        kinds.push(match value {
            LuaValue::UserData(ud) => ud.borrow::<EntityCategory>().ok().map(|c| covers(&c.0)),
            _ => None,
        });
    }
    let color = |c: &Option<String>| super::overlay_color(c.as_deref().unwrap_or_default());
    Ok(Some(
        active
            .into_iter()
            .map(|(name, f, set)| {
                let members = covers(&set);
                let mut mask = 0u16;
                for (k, of_kind) in kinds.iter().enumerate() {
                    let Some(of_kind) = of_kind else { continue };
                    let any = of_kind.iter().any(|&b| b);
                    let all = of_kind.iter().zip(&members).all(|(&kind, &member)| !kind || member);
                    if any && all {
                        mask |= 1 << k;
                    }
                }
                OverlayFilter { name, colors: [color(&f.colors[0]), color(&f.colors[1]), color(&f.colors[2])], inner: f.inner, outer: f.outer, kinds: mask, members }
            })
            .collect(),
    ))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Overlays::default());
    lua_fn!(lua, global,
        /// Defines (or redefines) the filter `overlay`: the units whose
        /// ranges it shows, its ring colours (`AARRGGBB`, AA the glow) and
        /// the thickness of the minimum (`inner`) and maximum (`outer`)
        /// range rings, zoomed in and zoomed out.
        SetOverlayFilter = |lua, (overlay, categories, normal_color, select_color, rollover_color, inner1, inner2, outer1, outer2): (String, Option<LuaUserDataRef<EntityCategory>>, Option<String>, Option<String>, Option<String>, Option<f32>, Option<f32>, Option<f32>, Option<f32>)| {
            let filter = Filter {
                categories: categories.map(|c| c.0.clone()),
                colors: [normal_color, select_color, rollover_color],
                inner: [inner1.unwrap_or(0.0), inner2.unwrap_or(0.0)],
                outer: [outer1.unwrap_or(0.0), outer2.unwrap_or(0.0)],
            };
            let mut s = state(lua);
            s.filters.insert(overlay, filter);
            s.version += 1;
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The filters shown, by name (nil: none).
        SetOverlayFilters = |lua, list: Option<List<String>>| {
            let mut names = Vec::new();
            if let Some(list) = list {
                for pair in list.pairs::<LuaValue, LuaValue>() {
                    if let Ok(name) = String::from_lua(pair?.1, lua) {
                        names.push(name);
                    }
                }
            }
            let mut s = state(lua);
            s.active = names;
            s.version += 1;
            Ok(())
        })?;
    Ok(())
}
