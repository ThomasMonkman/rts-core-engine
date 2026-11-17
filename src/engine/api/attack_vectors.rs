//! Brain attack vectors: `SetUpAttackVectorsToArmy`, `GetAttackVectors`,
//! `PickBestAttackVector` (FAF's AIs read them for base patrol points and
//! attack planning).
//!
//! As moho does (vanilla `MohoEngine.dll` 0x1016c290, 0x1016c770): the map is
//! cut into 32 x 32 cells. A cell is occupied when a unit of the current
//! enemy's army in the category lies in its box (centre +- 16, inclusive).
//! Every empty cell, in row order, gets one vector per occupied cell among
//! its 3 x 3 neighbours: the empty cell's centre (y = 0) and the step to the
//! neighbour's centre. So the vectors are approach points at the edge of
//! enemy territory. Moho reads the army's own unit list, not intel, and no
//! threat.

use mlua::prelude::*;

use crate::engine::api::army::ArmyHandle;
use crate::engine::api::categories::EntityCategory;
use crate::engine::api::{entity, moho, script_object};
use crate::lua_api::shapes::{self, Shaped};
use crate::lua_api::{Obj, lua_method};
use crate::engine::sim::army::{Alliance, Armies};
use crate::engine::sim::category::CategorySet;
use crate::engine::sim::entity::{EntityId, EntityKind};
use crate::engine::sim::math::{from_lua, Fx};

/// Side of a grid cell (world units).
const CELL: i32 = 32;
/// Reach of `HighestValue` / `LeastDefended` around a vector's target.
const VALUE_RADIUS: i32 = 32;

/// One attack vector: a cell centre `(px, 0, pz)` and the step `(vx, 0, vz)`
/// to the enemy cell next to it. All whole numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttackVector {
    pub px: i32,
    pub pz: i32,
    pub vx: i32,
    pub vz: i32,
}

/// Per army, the vectors of its last `SetUpAttackVectorsToArmy`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct AttackVectors(pub std::collections::BTreeMap<u32, Vec<AttackVector>>);

/// The vectors around `enemy`'s units in `category` on a map `width` x
/// `height`. `positions` are those units' (x, z).
pub fn compute(width: u32, height: u32, positions: impl Iterator<Item = (Fx, Fx)>) -> Vec<AttackVector> {
    let (cols, rows) = ((width >> 5) as i32, (height >> 5) as i32);
    if cols <= 0 || rows <= 0 {
        return Vec::new();
    }
    let mut occupied = vec![false; (cols * rows) as usize];
    let cell = Fx::from_num(CELL);
    for (x, z) in positions {
        // The boxes are inclusive, so a unit on a cell edge occupies both.
        let span = |v: Fx, n: i32| -> (i32, i32) {
            let i = (v / cell).floor().to_num::<i32>();
            let lo = if v == Fx::from_num(i) * cell { i - 1 } else { i };
            (lo.max(0), i.min(n - 1))
        };
        let ((x0, x1), (z0, z1)) = (span(x, cols), span(z, rows));
        for row in z0..=z1 {
            for col in x0..=x1 {
                occupied[(row * cols + col) as usize] = true;
            }
        }
    }
    let mut out = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            if occupied[(row * cols + col) as usize] {
                continue;
            }
            for nr in (row - 1).max(0)..=(row + 1).min(rows - 1) {
                for nc in (col - 1).max(0)..=(col + 1).min(cols - 1) {
                    if occupied[(nr * cols + nc) as usize] {
                        out.push(AttackVector { px: col * CELL + CELL / 2, pz: row * CELL + CELL / 2, vx: (nc - col) * CELL, vz: (nr - row) * CELL });
                    }
                }
            }
        }
    }
    out
}

/// One attack vector as scripts see it: the cell centre `p` and the step `v`
/// to the enemy cell (`y` always 0).
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "AttackVector", write_only)]
#[allow(dead_code)]
struct AttackVectorRow {
    px: i32,
    py: i32,
    pz: i32,
    vx: i32,
    vy: i32,
    vz: i32,
}

fn vector_table(lua: &Lua, v: &AttackVector) -> LuaResult<LuaTable> {
    let t = lua.create_table_with_capacity(0, 6)?;
    t.raw_set("px", v.px)?;
    t.raw_set("py", 0)?;
    t.raw_set("pz", v.pz)?;
    t.raw_set("vx", v.vx)?;
    t.raw_set("vy", 0)?;
    t.raw_set("vz", v.vz)?;
    Ok(t)
}

/// `ECompareType` of `PickBestAttackVector`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Compare {
    Closest,
    Furthest,
    HighestValue,
    LeastDefended,
}

impl Compare {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "Closest" => Self::Closest,
            "Furthest" => Self::Furthest,
            "HighestValue" => Self::HighestValue,
            "LeastDefended" => Self::LeastDefended,
            _ => return None,
        })
    }
}

fn dist2(ax: Fx, az: Fx, bx: Fx, bz: Fx) -> Fx {
    let (dx, dz) = (ax - bx, az - bz);
    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(AttackVectors::default());
    lua_method!(lua, moho::AIBrain,
        /// `category` defaults to `STRUCTURE - MOBILE`. With no current enemy it
        /// does nothing (the old vectors stay).
        SetUpAttackVectorsToArmy = |lua, (brain, category): (Obj<moho::AIBrain>, Option<LuaUserDataRef<EntityCategory>>)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let Some(enemy) = lua.app_data_ref::<Armies>().expect("Armies").get(index).and_then(|a| a.current_enemy) else { return Ok(()) };
            let category: CategorySet = match category {
                Some(c) => c.0.clone(),
                None => crate::engine::api::categories::domain(lua)?.parse("STRUCTURE - MOBILE").map_err(LuaError::runtime)?,
            };
            let (width, height) = {
                let t = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().ok_or_else(|| LuaError::runtime("no terrain"))?;
                (t.width, t.height)
            };
            let vectors = {
                let world = entity::world(lua);
                let positions = world
                    .iter()
                    .filter(|e| e.kind == EntityKind::Unit && !e.destroyed && e.army == Some(enemy))
                    .filter(|e| e.category_index.is_some_and(|i| category.contains(i)))
                    .map(|e| (e.position().x, e.position().z));
                compute(width, height, positions)
            };
            lua.app_data_mut::<AttackVectors>().expect("AttackVectors").0.insert(index, vectors);
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// `{ {px, py, pz, vx, vy, vz}, ... }`, or nil when there are none.
        GetAttackVectors = |lua, brain: Obj<moho::AIBrain>| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let vectors = lua.app_data_ref::<AttackVectors>().expect("AttackVectors").0.get(&index).cloned().unwrap_or_default();
            if vectors.is_empty() {
                return Ok(None);
            }
            let t = lua.create_table_with_capacity(vectors.len(), 0)?;
            for (i, v) in vectors.iter().enumerate() {
                t.raw_set(i + 1, vector_table(lua, v)?)?;
            }
            Ok(Some(shapes::List::<Shaped<AttackVectorRow>>::new(t)))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// `(platoon, squad, alliance, compareType, category[, module, function])`
        /// (FA's order; FAF's only caller, dead code, swaps the last two, so either
        /// order is read). The optional `import(module)[function](squadX, squadZ,
        /// targetX, targetZ)` vetoes vectors; its errors are logged and allow the
        /// vector, as in FA. Closest, Furthest and LeastDefended give the
        /// first vector left; HighestValue one into the cell of most value;
        /// nil for a platoon made this tick or the one before.
        PickBestAttackVector = |lua, args: LuaMultiValue| {
            let args: Vec<LuaValue> = args.into_iter().collect();
            if args.len() == 7 {
                return Err(LuaError::runtime(format!("Error in CAiBrain::PickBestAttackVector: Expected 6 or 8 arguments, got {} instead.", args.len())));
            }
            let arg = |i: usize| args.get(i).cloned().unwrap_or(LuaNil);
            let brain = LuaTable::from_lua(arg(0), lua)?;
            let platoon = LuaTable::from_lua(arg(1), lua)?;
            let squad = String::from_lua(arg(2), lua)?;
            let alliance = String::from_lua(arg(3), lua)?;
            let alliance = Alliance::parse(&alliance).ok_or_else(|| LuaError::runtime(format!("unknown alliance '{alliance}'")))?;
            let (mut compare, mut category) = (None, None);
            for v in [arg(4), arg(5)] {
                match v {
                    LuaValue::String(s) => compare = Compare::parse(&s.to_str()?),
                    LuaValue::UserData(u) => category = Some(u.borrow::<EntityCategory>()?.0.clone()),
                    _ => {}
                }
            }
            let compare = compare.unwrap_or(Compare::Closest);
            let filter = match (arg(6), arg(7)) {
                (LuaValue::String(m), LuaValue::String(f)) => {
                    let module: LuaTable = lua.globals().get::<LuaFunction>("import")?.call(m)?;
                    module.get::<Option<LuaFunction>>(f)?
                }
                _ => None,
            };
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let vectors = lua.app_data_ref::<AttackVectors>().expect("AttackVectors").0.get(&index).cloned().unwrap_or_default();
            let has_enemy = lua.app_data_ref::<Armies>().expect("Armies").get(index).and_then(|a| a.current_enemy).is_some();
            let members: Vec<EntityId> = crate::engine::api::platoon::live_members(lua, &platoon, |s| s.eq_ignore_ascii_case(&squad))?;
            // A platoon made the tick before gets nothing yet: the real
            // game's squad, made at t60, got nil for every pick at t61, and
            // picks three ticks after making it worked (dev/probes
            // probe_re3_ai AV L1, probe_re3_ai2). (Whether the platoon's age
            // or its units' counts isn't separated: both were made together.)
            let fresh = {
                let id = crate::engine::api::platoon::live_id(lua, &platoon)?;
                let made = crate::engine::api::platoon::platoons(lua).get(id).map_or(0, |p| p.created);
                crate::engine::api::sim_time::current_tick(lua) < made + 2
            };
            if fresh || !has_enemy || members.is_empty() || vectors.is_empty() {
                return Ok(LuaNil);
            }
            // The squad's centre, and the vectors whose target every squad unit
            // could stand on (FA tests the target point; by layer, so the enemy
            // structures there don't count).
            let (sx, sz, candidates) = {
                let world = entity::world(lua);
                let nav = lua.app_data_ref::<crate::engine::sim::nav::NavGrid>();
                let units: Vec<_> = members.iter().filter_map(|&id| world.get(id)).collect();
                let n = Fx::from_num(units.len());
                let (sx, sz) = units.iter().fold((Fx::ZERO, Fx::ZERO), |(x, z), e| (x + e.position().x, z + e.position().z));
                let fits = |v: &AttackVector| {
                    let (x, z) = (v.px + v.vx, v.pz + v.vz);
                    x >= 0 && z >= 0 && units.iter().all(|e| match (&nav, e.unit.as_deref()) {
                        (Some(nav), Some(u)) => crate::engine::sim::movement::nav_class(u, nav).is_none_or(|c| nav.terrain_suits(c, x as u32, z as u32)),
                        _ => true,
                    })
                };
                (sx / n, sz / n, vectors.iter().copied().filter(fits).collect::<Vec<_>>())
            };
            let mut allowed = Vec::with_capacity(candidates.len());
            for v in candidates {
                let ok = match &filter {
                    Some(f) => match f.call::<LuaValue>((sx.to_num::<f64>(), sz.to_num::<f64>(), v.px + v.vx, v.pz + v.vz)) {
                        Ok(r) => !matches!(r, LuaNil | LuaValue::Boolean(false)),
                        Err(e) => {
                            tracing::warn!("Error running user-supplied callback in CAiBrain::PickBestAttackVector: {e}");
                            true
                        }
                    },
                    None => true,
                };
                if ok {
                    allowed.push(v);
                }
            }
            // Units of the category with that alliance to this army: id,
            // position, blueprint (one pass, id order).
            let matching: Vec<(EntityId, Fx, Fx, String)> = {
                let world = entity::world(lua);
                let armies = lua.app_data_ref::<Armies>().expect("Armies");
                world
                    .iter()
                    .filter(|e| e.kind == EntityKind::Unit && !e.destroyed)
                    .filter(|e| category.as_ref().is_none_or(|c| e.category_index.is_some_and(|i| c.contains(i))))
                    .filter(|e| e.army.is_some_and(|a| armies.alliance(index, a) == alliance))
                    .map(|e| (e.id, e.position().x, e.position().z, e.cold.blueprint_id.clone()))
                    .collect()
            };
            // Closest, Furthest and LeastDefended give the first vector
            // whatever the squad's position or the intel: the real game's
            // did at all 10 squad positions, land and air squads alike
            // (dev/probes probe_re3_ai2, probe_re3_ai3; ours had picked by
            // straight-line distance). HighestValue goes for the cell of the
            // most value; which of its vectors the real game picks isn't
            // straight-line distance (not decoded): the closest here.
            let picked = match compare {
                Compare::Closest | Compare::Furthest | Compare::LeastDefended => allowed.first().copied(),
                Compare::HighestValue => {
                    // Blueprint value (energy + mass cost), read once per blueprint.
                    let mut values: std::collections::BTreeMap<&str, Fx> = std::collections::BTreeMap::new();
                    for (_, _, _, bp_id) in &matching {
                        if values.contains_key(bp_id.as_str()) {
                            continue;
                        }
                        let bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp_id)?;
                        let value = match bp.get::<Option<LuaTable>>("Economy")? {
                            Some(e) => from_lua(e.get::<Option<f64>>("BuildCostEnergy")?.unwrap_or(0.0)) + from_lua(e.get::<Option<f64>>("BuildCostMass")?.unwrap_or(0.0)),
                            None => Fx::ZERO,
                        };
                        values.insert(bp_id, value);
                    }
                    let r2 = Fx::from_num(VALUE_RADIUS * VALUE_RADIUS);
                    let mut best: Option<(Fx, Fx, AttackVector)> = None;
                    for v in &allowed {
                        let (x, z) = (Fx::from_num(v.px + v.vx), Fx::from_num(v.pz + v.vz));
                        let score = matching
                            .iter()
                            .filter(|&&(_, ux, uz, _)| dist2(ux, uz, x, z) <= r2)
                            .map(|(_, _, _, bp)| values.get(bp.as_str()).copied().unwrap_or(Fx::ZERO))
                            .fold(Fx::ZERO, |a, b| a + b);
                        let d = dist2(x, z, sx, sz);
                        let better = match best {
                            None => score > Fx::ZERO,
                            Some((b, bd, _)) => score > b || (score == b && d < bd),
                        };
                        if better {
                            best = Some((score, d, *v));
                        }
                    }
                    // Nothing of value anywhere: the first.
                    best.map(|(_, _, v)| v).or_else(|| allowed.first().copied())
                }
            };
            match picked {
                Some(v) => Ok(LuaValue::Table(vector_table(lua, &v)?)),
                None => Ok(LuaNil),
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(v: i32) -> Fx {
        Fx::from_num(v)
    }

    #[test]
    fn vectors_ring_an_occupied_cell() {
        // One unit in cell (2, 2) of a 128 x 128 map (4 x 4 cells): each of
        // its 8 neighbours gets one vector pointing at it, in row order.
        let v = compute(128, 128, [(fx(80), fx(80))].into_iter());
        assert_eq!(v.len(), 8);
        assert_eq!(v[0], AttackVector { px: 48, pz: 48, vx: 32, vz: 32 });
        assert_eq!(v[7], AttackVector { px: 112, pz: 112, vx: -32, vz: -32 });
        assert!(v.iter().all(|a| (a.px + a.vx, a.pz + a.vz) == (80, 80)));
    }

    #[test]
    fn a_unit_on_an_edge_occupies_both_cells() {
        // x = 64 is the edge of cells 1 and 2 (boxes are inclusive).
        let v = compute(128, 128, [(fx(64), fx(16))].into_iter());
        let targets: std::collections::BTreeSet<_> = v.iter().map(|a| (a.px + a.vx, a.pz + a.vz)).collect();
        assert_eq!(targets, [(48, 16), (80, 16)].into_iter().collect());
    }

    #[test]
    fn no_units_no_vectors() {
        assert!(compute(1024, 1024, std::iter::empty()).is_empty());
    }
}
