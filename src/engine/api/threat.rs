//! AI brain threat queries (`moho.aibrain_methods`) over `sim::threat`.
//!
//! `rings` / `radius` arguments count threat-map cells around the queried
//! cell (0 = that cell only), which is how FAF uses them (e.g.
//! `GetThreatAtPosition(pos, 1, ...)`, `GetThreatsAroundPosition(pos, 16, ...)`),
//! despite the annotations calling it game units. `armyIndex` picks one army's
//! threat; without it, every enemy of the brain counts.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::army::ArmyHandle;
use crate::engine::api::{entity, moho, script_object};
use crate::lua_api::{Obj, lua_method};
use crate::engine::sim::army::{Alliance, Armies};
use crate::engine::sim::math::FxVec3;
use crate::engine::sim::threat::{ThreatMap, ThreatType};

fn threat_type(t: Option<String>) -> LuaResult<ThreatType> {
    match t {
        None => Ok(ThreatType::Overall),
        Some(s) => ThreatType::parse(&s).ok_or_else(|| LuaError::runtime(format!("unknown threat type '{s}'"))),
    }
}

fn rings(v: f64) -> u32 {
    v.max(0.0) as u32
}

fn position(t: &LuaTable) -> LuaResult<FxVec3> {
    Ok(FxVec3::from_f64(t.raw_get(1)?, t.raw_get::<Option<f64>>(2)?.unwrap_or(0.0), t.raw_get(3)?))
}

/// The map update ([`ThreatMap::update`]) with every brain's knowledge of
/// other armies: allied units exactly, enemy units it detects now,
/// structures it has seen (`sim::vision`); the rest from its memory.
fn update(lua: &Lua, map: &mut ThreatMap) {
    use crate::engine::sim::threat::Knowledge;
    let now = crate::engine::api::sim_time::current_tick(lua);
    let world = entity::world(lua);
    let (Some(armies), Some(grids)) = (lua.app_data_ref::<Armies>(), lua.app_data_ref::<crate::engine::sim::vision::IntelGrids>()) else {
        map.update(&world, now, &[], &|_, _| Knowledge::Exact);
        return;
    };
    let brains: Vec<u32> = armies.list.iter().map(|a| a.index).collect();
    let structures = crate::engine::api::categories::domain(lua).ok().and_then(|d| d.named.get("STRUCTURE").cloned());
    let knows = |brain: u32, e: &crate::engine::sim::entity::Entity| {
        let Some(owner) = e.army else { return Knowledge::Unknown };
        if armies.alliance(brain, owner) == Alliance::Ally {
            return Knowledge::Exact;
        }
        if grids.detect(brain, e).any() {
            return Knowledge::Detected;
        }
        let structure = structures.as_ref().zip(e.category_index).is_some_and(|(s, i)| s.contains(i));
        if structure && grids.ever_seen(brain, e.id) { Knowledge::SeenStructure } else { Knowledge::Unknown }
    };
    // The fakes each brain believes in, and those it knows are fake.
    let fakes: Vec<_> = grids.fakes().iter().flat_map(|f| brains.iter().filter(|&&b| f.believed_by(b) || f.known_by(b)).map(move |&b| (b, f.id, f.jammer, f.position, f.believed_by(b)))).collect();
    map.update_with_fakes(&world, now, &brains, &knows, &fakes);
}

/// Makes sure the map has had an update (queries before the first one).
pub(crate) fn refresh(lua: &Lua, map: &mut ThreatMap) {
    if !map.is_built() {
        update(lua, map);
    }
}

/// Runs `f` on the (refreshed) threat map with the army filter for `brain`.
/// `allies_count_zero`: a requested army allied to the brain contributes
/// nothing (`GetHighestThreatPosition`'s rule).
fn query<R>(
    lua: &Lua,
    brain: &LuaTable,
    army: Option<u32>,
    allies_count_zero: bool,
    f: impl FnOnce(&ThreatMap, u32, &dyn Fn(u32) -> bool) -> R,
) -> LuaResult<R> {
    let ArmyHandle(me) = script_object::handle(brain)?;
    let mut map = lua.app_data_mut::<ThreatMap>().ok_or_else(|| LuaError::runtime("threat map: no map loaded"))?;
    refresh(lua, &mut map);
    let armies = lua.app_data_ref::<Armies>().expect("Armies");
    let alliance = |a: u32| armies.alliance(me, a);
    let counts = |a: u32| match army {
        Some(x) => a == x && !(allies_count_zero && alliance(x) == Alliance::Ally),
        None => alliance(a) == Alliance::Enemy,
    };
    Ok(f(&map, me, &counts))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_method!(lua, moho::AIBrain,
        /// Centre of the cell (plus `ring` neighbours) with the most threat (y = 0), and
        /// that threat; ties (and an empty map) go to the cell nearest the brain's start.
        GetHighestThreatPosition = |lua, (brain, ring, _restriction, ty, army): (Obj<moho::AIBrain>, f64, Option<bool>, Option<String>, Option<u32>)| {
            let ty = threat_type(ty)?;
            let ArmyHandle(me) = script_object::handle(&brain)?;
            let start = lua.app_data_ref::<Armies>().and_then(|a| a.get(me).and_then(|a| a.start)).unwrap_or_default();
            let (pos, t) = query(lua, &brain, army, true, |m, me, counts| m.highest(rings(ring), ty, me, counts, start))?;
            Ok((entity::vector(lua, pos)?, t.to_num::<f64>()))
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// Threat of type `ty` (default overall) within `radius` threat-map cells of `pos`, from the
        /// brain's enemies (or only army `army`).
        GetThreatAtPosition = |lua, (brain, pos, radius, _restriction, ty, army): (Obj<moho::AIBrain>, shapes::Vector, f64, Option<bool>, Option<String>, Option<u32>)| {
            let (ty, p) = (threat_type(ty)?, position(&pos)?);
            let t = query(lua, &brain, army, false, |m, me, counts| m.threat_at(p, rings(radius), ty, me, counts))?;
            Ok(t.to_num::<f64>())
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// `{ {x, z, threat}, ... }` for cells with threat, row-major.
        GetThreatsAroundPosition = |lua, (brain, pos, radius, _restriction, ty, army): (Obj<moho::AIBrain>, shapes::Vector, f64, Option<bool>, Option<String>, Option<u32>)| {
            let (ty, p) = (threat_type(ty)?, position(&pos)?);
            let cells = query(lua, &brain, army, false, |m, me, counts| m.around(p, rings(radius), ty, me, counts))?;
            let out = lua.create_table_with_capacity(cells.len(), 0)?;
            for (i, (x, z, t)) in cells.into_iter().enumerate() {
                let row = lua.create_table_with_capacity(3, 0)?;
                row.raw_set(1, x.to_num::<f64>())?;
                row.raw_set(2, z.to_num::<f64>())?;
                row.raw_set(3, t.to_num::<f64>())?;
                out.raw_set(i + 1, row)?;
            }
            Ok(shapes::List::<Vec<f64>>::new(out))
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// Threat of type `ty` (default overall) along the line from `a` to `b`, from the brain's
        /// enemies (or only army `army`).
        GetThreatBetweenPositions = |lua, (brain, a, b, _restriction, ty, army): (Obj<moho::AIBrain>, shapes::Vector, shapes::Vector, Option<bool>, Option<String>, Option<u32>)| {
            let (ty, a, b) = (threat_type(ty)?, position(&a)?, position(&b)?);
            let t = query(lua, &brain, army, false, |m, me, counts| m.between(a, b, ty, me, counts))?;
            Ok(t.to_num::<f64>())
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// Threat the brain believes is at `pos` from the next map update on, losing `decay` x
        /// `threat` per update while positive (a negative amount stays and subtracts).
        AssignThreatAtPosition = |lua, (brain, pos, threat, decay, ty): (Obj<moho::AIBrain>, shapes::Vector, f64, Option<f64>, Option<String>)| {
            let ArmyHandle(me) = script_object::handle(&brain)?;
            let (ty, p) = (threat_type(ty)?, position(&pos)?);
            if let Some(mut m) = lua.app_data_mut::<ThreatMap>() {
                m.assign(me, p, crate::engine::sim::math::from_lua(threat), crate::engine::sim::math::from_lua(decay.unwrap_or(0.0)), ty);
            }
            Ok(())
        },
    )?;
    Ok(())
}

/// Once per tick: the map update on moho's 30-tick grid.
pub fn tick(lua: &Lua) {
    if !crate::engine::api::sim_time::current_tick(lua).is_multiple_of(crate::engine::sim::threat::UPDATE_TICKS) {
        return;
    }
    if let Some(mut m) = lua.app_data_mut::<ThreatMap>() {
        update(lua, &mut m);
    }
}
