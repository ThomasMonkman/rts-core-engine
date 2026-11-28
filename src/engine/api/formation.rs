//! Formation orders: asks FAF's `/lua/formations.lua` for the layout and
//! turns it into per-unit slots (`sim::formation`).
//!
//! Like the engine, surface units and air units are laid out by separate
//! calls (FAF's `CalculateSizes` comment), each scaled by its own largest
//! footprint + 2 and capped at its own slowest unit. The function gets the
//! units as a 0-based array (`GetCachedResults` compares from index 0).
//! Runs once per order, not per tick. Any failure (unknown formation, script
//! error) leaves the order without slots: units then go to the target as for
//! a plain order.

use mlua::prelude::*;

use crate::engine::api::entity::{object, world};
use crate::engine::sim::command::FormationSlot;
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::formation::{assign, heading_towards, Member, RawSlot, Travel};
use crate::engine::sim::math::{from_lua, Fx, FxVec3};
use crate::engine::sim::movement::MotionKind;

/// Names meaning "no formation".
fn is_none(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "" | "none" | "noformation")
}

/// Slots for `units` in formation `name` around `target`, and each formed
/// group's travelling formation (index = `FormationSlot::group`). `degrees`
/// is the final facing (0 = south, 90 = east); `None` faces the direction
/// of travel.
pub fn plan(lua: &Lua, units: &[EntityId], name: Option<&str>, target: FxVec3, degrees: Option<f64>) -> LuaResult<(Vec<FormationSlot>, Vec<Travel>)> {
    let Some(name) = name.filter(|n| !is_none(n)) else { return Ok(Default::default()) };
    let import: LuaFunction = lua.globals().get("import")?;
    let module: LuaTable = match import.call("/lua/formations.lua") {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "formations.lua failed to load; ordering without formation");
            return Ok(Default::default());
        }
    };
    let Some(layout) = module.get::<Option<LuaFunction>>(name)? else {
        tracing::warn!(name, "unknown formation; ordering without formation");
        return Ok(Default::default());
    };
    // Surface and air groups, each in id order.
    let (mut surface, mut air) = (Vec::new(), Vec::new());
    let blueprints: LuaTable = lua.globals().raw_get("__blueprints")?;
    {
        let w = world(lua);
        let mut ids = units.to_vec();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let Some(e) = w.get(id).filter(|e| !e.destroyed) else { continue };
            let Some(u) = e.unit.as_deref() else { continue };
            let m = Member { id, position: e.position(), category: e.category_index, max_speed: u.motion_params.max_speed.saturating_mul(u.motion_params.kind.speed_mult(u.motion.speed)) };
            // FAF's `Footprint.SizeMax` (blueprints-units.lua): what its
            // `CalculateSizes` compensates for. FAF computes it before the
            // engine fills in default footprints, so blueprints without a
            // `Footprint` (most tanks, the Fatboy) count as 1 there; using
            // the filled-in size scaled such groups by up to 10/3.
            let size_max = blueprints
                .raw_get::<Option<LuaTable>>(e.cold.blueprint_id.as_str())?
                .and_then(|bp| bp.raw_get::<Option<LuaTable>>("Footprint").ok().flatten())
                .and_then(|fp| fp.raw_get::<Option<f64>>("SizeMax").ok().flatten());
            let footprint = size_max.map_or(u.radius * 2, from_lua);
            match u.motion_params.kind {
                MotionKind::None => {}
                MotionKind::Air => air.push((m, footprint)),
                _ => surface.push((m, footprint)),
            }
        }
    }
    let mut out = Vec::with_capacity(units.len());
    let mut travel = Vec::new();
    for (surface_group, group) in [(true, surface), (false, air)] {
        if group.len() < 2 {
            continue;
        }
        let members: Vec<Member> = group.iter().map(|g| g.0).collect();
        let largest = group.iter().map(|g| g.1).max().unwrap_or(Fx::ONE);
        let scale = largest + Fx::from_num(2);
        let heading = match degrees {
            Some(d) => from_lua(d.to_radians()),
            None => heading_towards(&members, target),
        };
        let list = lua.create_table_with_capacity(0, members.len())?;
        for (i, m) in members.iter().enumerate() {
            if let Some(obj) = object(lua, m.id)? {
                list.raw_set(i as i64, obj)?;
            }
        }
        let result: LuaTable = match layout.call(list) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(name, error = %e, "formation script failed; ordering without formation");
                return Ok(Default::default());
            }
        };
        let mut slots = Vec::with_capacity(members.len());
        for entry in result.sequence_values::<LuaValue>() {
            let LuaValue::Table(t) = entry? else { continue };
            let filter = match t.raw_get::<LuaValue>(3)? {
                LuaValue::UserData(u) => u.borrow::<crate::engine::api::categories::EntityCategory>().ok().map(|c| c.0.clone()),
                _ => None,
            };
            slots.push(RawSlot {
                x: from_lua(t.raw_get::<Option<f64>>(1)?.unwrap_or(0.0)),
                z: from_lua(t.raw_get::<Option<f64>>(2)?.unwrap_or(0.0)),
                filter,
                rotate: t.raw_get::<Option<bool>>(5)?.unwrap_or(true),
            });
        }
        tracing::debug!(name, units = members.len(), slots = slots.len(), "formation planned");
        let assigned = assign(target, heading, scale, &slots, &members, travel.len() as u8);
        let cap = assigned.first().map_or(Fx::ZERO, |s| s.speed_cap);
        let mut t = Travel::new(target, heading, cap, surface_group);
        t.issued = crate::engine::api::sim_time::current_tick(lua);
        // The layout's real spacing: the closest pair of consecutive slots
        // (FAF lists a row's slots side by side), at least the scale.
        t.spacing = assigned
            .windows(2)
            .map(|w| crate::engine::sim::math::sqrt((w[1].lx - w[0].lx).saturating_mul(w[1].lx - w[0].lx).saturating_add((w[1].lz - w[0].lz).saturating_mul(w[1].lz - w[0].lz))))
            .filter(|d| *d > Fx::ONE / 2)
            .min()
            .unwrap_or(scale)
            .max(scale);
        travel.push(t);
        out.extend(assigned);
    }
    Ok((out, travel))
}
