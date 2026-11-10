//! Platoon script objects (`/lua/platoon.lua` Platoon instances) and the
//! brain / platoon methods that manage them.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::sim::entity::EntityId;

use crate::engine::api::army::ArmyHandle;
use crate::engine::api::callbacks::PlatoonCallbacks;
use crate::engine::api::{moho, script_object};
use crate::lua_api::{Obj, lua_method};
use crate::engine::sim::platoon::{PlatoonId, Platoons};

#[derive(Clone, Copy, Debug)]
pub struct PlatoonHandle(pub PlatoonId);
impl LuaUserData for PlatoonHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

const PLATOONS_KEY: &str = "engine.platoons";

fn objects(lua: &Lua) -> LuaResult<LuaTable> {
    crate::engine::api::script_object::registry_table(lua, PLATOONS_KEY)
}

fn platoons(lua: &Lua) -> mlua::AppDataRefMut<'_, Platoons> {
    lua.app_data_mut::<Platoons>().expect("Platoons not in app_data")
}

/// Live member units of platoon `this` whose squad passes `squad`, in
/// insertion order (moho drops dead units from platoons).
pub(crate) fn live_members(lua: &Lua, this: &LuaTable, squad: impl Fn(&str) -> bool) -> LuaResult<Vec<EntityId>> {
    let id = platoon_id(this)?;
    let members: Vec<EntityId> = match platoons(lua).get(id) {
        Some(p) => p.units.iter().filter(|(_, s)| squad(s)).map(|(u, _)| *u).collect(),
        None => return Ok(Vec::new()),
    };
    let world = crate::engine::api::entity::world(lua);
    Ok(members.into_iter().filter(|u| world.get(*u).is_some_and(|e| !e.destroyed)).collect())
}

pub fn platoon_id(obj: &LuaTable) -> LuaResult<PlatoonId> {
    Ok(script_object::handle::<PlatoonHandle>(obj)?.0)
}

/// Creates a platoon for `army`, its Lua object, and runs `OnCreate(plan)`.
pub fn make_platoon(lua: &Lua, army: u32, label: &str, plan: &str) -> LuaResult<Obj<moho::Platoon>> {
    let id = platoons(lua).create(army, label.to_owned(), plan.to_owned());
    let import: LuaFunction = lua.globals().get("import")?;
    let class: LuaTable = import.call::<LuaTable>("/lua/platoon.lua")?.get("Platoon")?;
    let obj: LuaTable = class.call(())?;
    obj.raw_set(script_object::C_OBJECT, PlatoonHandle(id))?;
    objects(lua)?.raw_set(id, obj.clone())?;
    PlatoonCallbacks::on_create(lua, &obj, "platoon", plan)?;
    Ok(Obj::new(obj))
}

/// Adds a new unit to `army`'s `ArmyPool` (no-op if the army has none).
pub fn add_to_pool(lua: &Lua, army: u32, unit: EntityId) {
    let mut p = platoons(lua);
    if let Some(pool) = p.by_unique_name(army, "ArmyPool") {
        p.assign(unit, pool, "Unassigned");
    }
}

/// One squad line of a platoon template: `{ category, min, max, squad,
/// formation }`.
struct SquadSpec {
    /// Category set, or a single blueprint's domain index.
    filter: SquadFilter,
    min: usize,
    /// `None` = no limit (negative max in the template).
    max: Option<usize>,
    squad: String,
}

enum SquadFilter {
    Category(crate::engine::sim::category::CategorySet),
    Blueprint(Option<u32>),
}

fn parse_template(lua: &Lua, template: &LuaTable, multiplier: f64) -> LuaResult<(String, String, Vec<SquadSpec>)> {
    let name: String = template.raw_get::<Option<String>>(1)?.unwrap_or_default();
    let plan: String = template.raw_get::<Option<String>>(2)?.unwrap_or_default();
    let domain = crate::engine::api::categories::domain(lua)?;
    let mut squads = Vec::new();
    for line in template.clone().sequence_values::<LuaValue>().skip(2) {
        let LuaValue::Table(line) = line? else { continue };
        let filter = match line.raw_get::<LuaValue>(1)? {
            LuaValue::UserData(ud) => SquadFilter::Category(ud.borrow::<crate::engine::api::categories::EntityCategory>()?.0.clone()),
            LuaValue::String(s) => SquadFilter::Blueprint(domain.index_of(&s.to_str()?.to_lowercase())),
            _ => continue,
        };
        let min = line.raw_get::<Option<f64>>(2)?.unwrap_or(0.0).max(0.0) as usize;
        let max = line.raw_get::<Option<f64>>(3)?.unwrap_or(-1.0);
        let max = (max >= 0.0).then(|| (max * multiplier.max(1.0)) as usize);
        let squad = line.raw_get::<Option<String>>(4)?.unwrap_or_else(|| "Unassigned".into());
        squads.push(SquadSpec { filter, min, max, squad });
    }
    Ok((name, plan, squads))
}

/// Picks units from platoon `source` for each template squad: live members
/// matching the squad's filter (within `radius` of `location` when given),
/// nearest first then by id, each unit used once. `None` if a squad can't
/// reach its minimum.
fn select_for_template(
    lua: &Lua,
    source: &LuaTable,
    squads: &[SquadSpec],
    location: Option<crate::engine::sim::math::FxVec3>,
    radius: Option<f64>,
) -> LuaResult<Option<Vec<(EntityId, String)>>> {
    use crate::engine::sim::math::Fx;
    let members = live_members(lua, source, |_| true)?;
    let world = crate::engine::api::entity::world(lua);
    let r2 = radius.map(|r| crate::engine::sim::math::from_lua(r).saturating_mul(crate::engine::sim::math::from_lua(r)));
    let dist2 = |id: EntityId| -> Fx {
        match (location, world.get(id)) {
            (Some(l), Some(e)) => {
                let (dx, dz) = (e.position().x - l.x, e.position().z - l.z);
                dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
            }
            _ => Fx::ZERO,
        }
    };
    let mut candidates: Vec<(Fx, EntityId)> = members
        .into_iter()
        .map(|id| (dist2(id), id))
        .filter(|&(d, _)| match (location, r2) {
            (Some(_), Some(r2)) => d <= r2,
            _ => true,
        })
        .collect();
    candidates.sort_unstable();
    let mut used = std::collections::BTreeSet::new();
    let mut picked = Vec::new();
    for spec in squads {
        let mut n = 0;
        for &(_, id) in &candidates {
            if spec.max.is_some_and(|m| n >= m) {
                break;
            }
            if used.contains(&id) {
                continue;
            }
            let cat = world.get(id).and_then(|e| e.category_index);
            let ok = match &spec.filter {
                SquadFilter::Category(set) => cat.is_some_and(|c| set.contains(c)),
                SquadFilter::Blueprint(b) => b.is_some() && cat == *b,
            };
            if ok {
                used.insert(id);
                picked.push((id, spec.squad.clone()));
                n += 1;
            }
        }
        if n < spec.min {
            return Ok(None);
        }
    }
    Ok(Some(picked))
}

fn read_location(v: Option<&LuaTable>) -> LuaResult<Option<crate::engine::sim::math::FxVec3>> {
    match v {
        Some(t) => Ok(Some(crate::engine::sim::math::FxVec3::from_f64(t.raw_get(1)?, t.raw_get::<Option<f64>>(2)?.unwrap_or(0.0), t.raw_get(3)?))),
        None => Ok(None),
    }
}

/// Gives a new brain its `ArmyPool` platoon, as moho does, with the plan
/// `PoolAI` (MohoEngine.dll 0x102d90ca: the platoon `"Pool"`, plan
/// `"PoolAI"`, then uniquely named `ArmyPool`): `OnCreate('PoolAI')`. FAF's
/// `Platoon` has no `PoolAI`, so nothing runs; the base game's and LOUD's
/// do (LOUD forks `self[plan]` for any plan but `'none'`).
pub fn create_army_pool(lua: &Lua, army: u32) -> LuaResult<()> {
    let pool = make_platoon(lua, army, "ArmyPool", "PoolAI")?;
    let id = platoon_id(&pool)?;
    if let Some(p) = platoons(lua).get_mut(id) {
        p.unique_name = Some("ArmyPool".into());
    }
    Ok(())
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Platoons::default());

    lua_method!(lua, moho::AIBrain,
        /// Whether `platoon` is a live platoon. A platoon whose last unit died this tick is gone for
        /// scripts already (it is destroyed at the tick's end): FAF follows a true `PlatoonExists`
        /// with `GetPlatoonPosition`, which would be nil.
        PlatoonExists = |lua, (_brain, platoon): (Obj<moho::AIBrain>, LuaValue)| {
            let LuaValue::Table(t) = platoon else { return Ok(false) };
            let Ok(id) = platoon_id(&t) else { return Ok(false) };
            // A platoon whose last unit died this tick is gone for scripts
            // already (it's destroyed at the tick's end): FAF follows a true
            // `PlatoonExists` with `GetPlatoonPosition`, which would be nil.
            let ps = platoons(lua);
            Ok(ps.get(id).is_some() && !ps.is_doomed(id))
        })?;
    lua_method!(lua, moho::Platoon,
        /// The brain's `IsOpponentAIRunning` (the same console variable;
        /// MohoEngine.dll 0x102fe860). The original game's platoon AI asks.
        IsOpponentAIRunning = |lua, _this: Obj<moho::Platoon>| Ok(crate::engine::api::army::opponent_ai_running(lua)))?;
    lua_method!(lua, moho::Platoon,
        /// The platoon's live member units, in insertion order.
        GetPlatoonUnits = |lua, this: Obj<moho::Platoon>| {
            let ids = live_members(lua, &this, |_| true)?;
            crate::engine::api::entity::objects_table::<moho::Unit>(lua, &ids)
        })?;
    lua_method!(lua, moho::Platoon,
        /// Squad names compare case-insensitively ('Attack' / 'attack').
        /// Stored per squad for the platoon's own target searches; unit weapons
        /// keep their own `SetTargetingPriorities` (inferred).
        SetPrioritizedTargetList = |lua, (this, squad, list): (Obj<moho::Platoon>, String, shapes::List<crate::engine::api::categories::EntityCategory>)| {
            let mut cats = Vec::new();
            for v in list.into_inner().sequence_values::<LuaUserDataRef<crate::engine::api::categories::EntityCategory>>() {
                cats.push(v?.0.clone());
            }
            let id = platoon_id(&this)?;
            if let Some(p) = platoons(lua).get_mut(id) {
                match p.target_priorities.iter_mut().find(|(s, _)| s.eq_ignore_ascii_case(&squad)) {
                    Some(entry) => entry.1 = cats,
                    None => p.target_priorities.push((squad, cats)),
                }
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Platoon,
        /// The live members of the named squad (case-insensitive), in insertion order.
        GetSquadUnits = |lua, (this, squad): (Obj<moho::Platoon>, String)| {
            let ids = live_members(lua, &this, |s| s.eq_ignore_ascii_case(&squad))?;
            crate::engine::api::entity::objects_table::<moho::Unit>(lua, &ids)
        })?;
    lua_method!(lua, moho::Platoon,
        /// Mean member position (fixed-point), nil when empty.
        GetPlatoonPosition = |lua, this: Obj<moho::Platoon>| {
            use crate::engine::sim::math::{Fx, FxVec3};
            let ids = live_members(lua, &this, |_| true)?;
            if ids.is_empty() {
                return Ok(None);
            }
            let mut sum = FxVec3::default();
            {
                let world = crate::engine::api::entity::world(lua);
                for id in &ids {
                    if let Some(e) = world.get(*id) {
                        sum = sum + e.position();
                    }
                }
            }
            let n = Fx::from_num(ids.len() as i32);
            let mean = FxVec3 { x: sum.x / n, y: sum.y / n, z: sum.z / n };
            Ok(Some(crate::engine::api::entity::vector(lua, mean)?))
        })?;
    lua_method!(lua, moho::Platoon,
        /// The brain of the platoon's army, or nil if the platoon is gone.
        GetBrain = |lua, this: Obj<moho::Platoon>| {
            let id = script_object::handle::<PlatoonHandle>(&this)?.0;
            let army = platoons(lua).get(id).map(|p| p.army);
            match army {
                Some(army) => crate::engine::api::army::brain_obj(lua, army),
                None => Ok(None),
            }
        })?;
    /// Units back to the army pool, platoon gone, `OnDestroy` on it.
    fn disband(lua: &Lua, id: PlatoonId) -> LuaResult<()> {
        destroy(lua, id)
    }
    lua_method!(lua, moho::AIBrain,
        /// Disbands the platoon: its units go back to the army pool and `OnDestroy` runs on it.
        DisbandPlatoon = |lua, (_brain, platoon): (Obj<moho::AIBrain>, Obj<moho::Platoon>)| {
            disband(lua, platoon_id(&platoon)?)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Disbands the army's platoon holding that unique name, if any.
        DisbandPlatoonUniquelyNamed = |lua, (brain, name): (Obj<moho::AIBrain>, String)| {
            let ArmyHandle(army) = script_object::handle(&brain)?;
            let id = platoons(lua).by_unique_name(army, &name);
            match id {
                Some(id) => disband(lua, id),
                None => Ok(()),
            }
        })?;
    lua_method!(lua, moho::Platoon,
        /// How many live members match `category`.
        PlatoonCategoryCount = |lua, (this, category): (Obj<moho::Platoon>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            let members = live_members(lua, &this, |_| true)?;
            let w = crate::engine::api::entity::world(lua);
            Ok(members.into_iter().filter_map(|id| w.get(id)).filter(|e| e.category_index.is_some_and(|c| category.0.contains(c))).count())
        },
    )?;
    lua_method!(lua, moho::Platoon,
        /// Average position of the squad's live units (nil if none).
        GetSquadPosition = |lua, (this, squad): (Obj<moho::Platoon>, String)| {
            use crate::engine::sim::math::{Fx, FxVec3};
            let members = live_members(lua, &this, |s| s == squad)?;
            if members.is_empty() {
                return Ok(None);
            }
            let sum = {
                let w = crate::engine::api::entity::world(lua);
                members.iter().filter_map(|&id| w.get(id)).fold(FxVec3::default(), |a, e| a + e.position())
            };
            let n = Fx::from_num(members.len());
            crate::engine::api::entity::vector(lua, FxVec3 { x: sum.x / n, y: sum.y / n, z: sum.z / n }).map(Some)
        })?;
    lua_method!(lua, moho::Platoon,
        /// Names are unique per army: a platoon already holding the name loses it.
        UniquelyNamePlatoon = |lua, (this, name): (Obj<moho::Platoon>, String)| {
            let id = platoon_id(&this)?;
            let mut ps = platoons(lua);
            let Some(army) = ps.get(id).map(|p| p.army) else { return Ok(()) };
            if let Some(old) = ps.by_unique_name(army, &name)
                && let Some(p) = ps.get_mut(old) {
                p.unique_name = None;
            }
            if let Some(p) = ps.get_mut(id) {
                p.unique_name = Some(name);
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Platoon,
        /// The platoon's unique name, or nil.
        GetPlatoonUniqueName = |lua, this: Obj<moho::Platoon>| {
            let id = platoon_id(&this)?;
            Ok(platoons(lua).get(id).and_then(|p| p.unique_name.clone()))
        })?;
    lua_method!(lua, moho::Platoon,
        /// The nearest unit (to the squad's centre) of `category` with that
        /// alliance to the platoon's army; ties to the lowest id.
        FindClosestUnit = |lua, (this, squad, alliance, _can_attack, category): (Obj<moho::Platoon>, String, String, Option<bool>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            use crate::engine::sim::army::{Alliance, Armies};
            use crate::engine::sim::math::Fx;
            let want = Alliance::parse(&alliance).ok_or_else(|| LuaError::runtime(format!("unknown alliance '{alliance}'")))?;
            let id = platoon_id(&this)?;
            let Some(army) = platoons(lua).get(id).map(|p| p.army) else { return Ok(None) };
            let mut members = live_members(lua, &this, |s| s.eq_ignore_ascii_case(&squad))?;
            if members.is_empty() {
                members = live_members(lua, &this, |_| true)?;
            }
            let best = {
                let w = crate::engine::api::entity::world(lua);
                let Some(centre) = members.first().and_then(|&m| w.get(m)).map(|e| e.position()) else { return Ok(None) };
                let armies = lua.app_data_ref::<Armies>().expect("Armies");
                w.iter()
                    .filter(|e| e.kind == crate::engine::sim::entity::EntityKind::Unit && !e.destroyed)
                    .filter(|e| e.category_index.is_some_and(|c| category.0.contains(c)))
                    .filter(|e| e.army.is_some_and(|a| armies.alliance(army, a) == want))
                    .map(|e| {
                        let (dx, dz) = (e.position().x - centre.x, e.position().z - centre.z);
                        (dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)), e.id)
                    })
                    .min_by_key(|&(d, id): &(Fx, u32)| (d, id))
                    .map(|(_, id)| id)
            };
            match best {
                Some(id) => crate::engine::api::entity::object_as::<moho::Unit>(lua, id),
                None => Ok(None),
            }
        },
    )?;
    lua_method!(lua, moho::Platoon,
        /// A squad unit has an enabled weapon that may fire at the target's
        /// current layer (`SetFireTargetLayerCaps`).
        CanAttackTarget = |lua, (this, squad, target): (Obj<moho::Platoon>, String, Obj<moho::Entity>)| {
            let members = live_members(lua, &this, |s| s.eq_ignore_ascii_case(&squad))?;
            let Ok(target) = crate::engine::api::entity::entity_id(&target) else { return Ok(false) };
            Ok(can_attack(lua, &members, target))
        })?;
    lua_method!(lua, moho::Platoon,
        /// The nearest unit (to `position`, within `radius`) with that alliance to
        /// the platoon's army, trying the squad's `SetPrioritizedTargetList`
        /// categories in order; ties to the lowest id. Without a priority list
        /// nothing matches (inferred).
        FindPrioritizedUnit = |lua, (this, squad, alliance, can_attack_only, position, radius): (Obj<moho::Platoon>, String, String, Option<bool>, shapes::Vector, f64)| {
            use crate::engine::sim::army::{Alliance, Armies};
            use crate::engine::sim::math::{from_lua, Fx};
            let want = Alliance::parse(&alliance).ok_or_else(|| LuaError::runtime(format!("unknown alliance '{alliance}'")))?;
            let id = platoon_id(&this)?;
            let Some((army, priorities)) = platoons(lua).get(id).map(|p| {
                let cats = p.target_priorities.iter().find(|(s, _)| s.eq_ignore_ascii_case(&squad)).map(|(_, c)| c.clone()).unwrap_or_default();
                (p.army, cats)
            }) else {
                return Ok(None);
            };
            let members = if can_attack_only.unwrap_or(false) { live_members(lua, &this, |s| s.eq_ignore_ascii_case(&squad))? } else { Vec::new() };
            let (cx, cz) = (from_lua(position.raw_get::<Option<f64>>(1)?.unwrap_or(0.0)), from_lua(position.raw_get::<Option<f64>>(3)?.unwrap_or(0.0)));
            let near = crate::engine::api::entity::world(lua).units_in_circle(cx, cz, from_lua(radius));
            for cat in &priorities {
                // Candidates under the world borrow, the attack check after
                // (it reads the world itself).
                let mut found: Vec<(Fx, u32)> = {
                    let w = crate::engine::api::entity::world(lua);
                    let armies = lua.app_data_ref::<Armies>().expect("Armies");
                    near.iter()
                        .filter_map(|&u| w.get(u))
                        .filter(|e| !e.destroyed && e.category_index.is_some_and(|c| cat.contains(c)))
                        .filter(|e| e.army.is_some_and(|a| armies.alliance(army, a) == want))
                        .map(|e| {
                            let (dx, dz) = (e.position().x - cx, e.position().z - cz);
                            (dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)), e.id)
                        })
                        .collect()
                };
                found.sort_unstable();
                let best = found.into_iter().map(|(_, u)| u).find(|&u| members.is_empty() || can_attack(lua, &members, u));
                if let Some(u) = best {
                    return crate::engine::api::entity::object_as::<moho::Unit>(lua, u);
                }
            }
            Ok(None)
        },
    )?;
    lua_method!(lua, moho::Platoon,
        /// Sets (or clears with nil) the formation the platoon moves in.
        SetPlatoonFormationOverride = |lua, (this, f): (Obj<moho::Platoon>, Option<String>)| {
            let id = platoon_id(&this)?;
            if let Some(p) = platoons(lua).get_mut(id) {
                p.formation = f;
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Platoon,
        /// How many live members match `category` within `radius` of `pos` (XZ plane).
        PlatoonCategoryCountAroundPosition = |lua, (this, category, pos, radius): (Obj<moho::Platoon>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>, shapes::Vector, f64)| {
            let p = read_location(Some(&pos))?.unwrap_or_default();
            let r2 = crate::engine::sim::math::from_lua(radius * radius);
            let members = live_members(lua, &this, |_| true)?;
            let w = crate::engine::api::entity::world(lua);
            Ok(members
                .into_iter()
                .filter_map(|id| w.get(id))
                .filter(|e| e.category_index.is_some_and(|c| category.0.contains(c)))
                .filter(|e| {
                    let (dx, dz) = (e.position().x - p.x, e.position().z - p.z);
                    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= r2
                })
                .count())
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// `platoon` is a platoon object or the unique name of one; `units` a list
        /// of units (or one unit). Units leave their previous platoon.
        AssignUnitsToPlatoon = |lua, (brain, platoon, units, squad, formation): (Obj<moho::AIBrain>, shapes::PlatoonRef, shapes::Units, Option<String>, Option<String>)| {
            let ArmyHandle(army) = script_object::handle(&brain)?;
            let target = match &*platoon {
                LuaValue::Table(t) => platoon_id(t)?,
                LuaValue::String(s) => match platoons(lua).by_unique_name(army, &s.to_str()?) {
                    Some(id) => id,
                    None => return Ok(()),
                },
                _ => return Err(LuaError::runtime("AssignUnitsToPlatoon: expected a platoon or platoon name")),
            };
            let ids: Vec<EntityId> = match &*units {
                LuaValue::Table(t) if t.raw_get::<LuaValue>(script_object::C_OBJECT)?.is_nil() => t
                    .clone()
                    .sequence_values::<LuaTable>()
                    .filter_map(|u| u.ok().and_then(|u| crate::engine::api::entity::entity_id(&u).ok()))
                    .collect(),
                LuaValue::Table(u) => vec![crate::engine::api::entity::entity_id(u)?],
                _ => Vec::new(),
            };
            let squad = squad.unwrap_or_else(|| "Unassigned".into());
            {
                let mut p = platoons(lua);
                for id in ids {
                    p.assign(id, target, &squad);
                }
                // The platoon moves in the last formation it was given
                // (inferred: moho keeps one per squad).
                if let (Some(f), Some(pl)) = (formation, p.get_mut(target)) {
                    pl.formation = Some(f);
                }
            }
            if let Some(obj) = objects(lua)?.raw_get::<Option<LuaTable>>(target)? {
                PlatoonCallbacks::on_units_added_to_platoon(lua, &obj, "platoon")?;
            }
            Ok(())
        },
    )?;
    // Sum of the live members' threat of `threatType`, for members in
    // `category` (`...AroundPosition`: only those within `radius` of `pos`).
    fn platoon_threat(lua: &Lua, this: &LuaTable, ty: String, category: &crate::engine::sim::category::CategorySet, area: Option<(crate::engine::sim::math::FxVec3, f64)>) -> LuaResult<f64> {
        use crate::engine::sim::math::Fx;
        use crate::engine::sim::threat::ThreatType;
        // Platoon threat names are the blueprint levels ('Surface' = the
        // units' SurfaceThreatLevel, ...); note 'Air' here is AntiAir, not the
        // brain's air-unit category type.
        let ty = match ty.as_str() {
            "Surface" => ThreatType::AntiSurface,
            "Air" => ThreatType::AntiAir,
            "Sub" => ThreatType::AntiSub,
            other => ThreatType::parse(other).ok_or_else(|| LuaError::runtime(format!("unknown threat type '{other}'")))?,
        };
        let members = live_members(lua, this, |_| true)?;
        let w = crate::engine::api::entity::world(lua);
        let r2 = area.map(|(_, r)| crate::engine::sim::math::from_lua(r * r));
        let total = members
            .into_iter()
            .filter_map(|id| w.get(id))
            .filter(|e| e.category_index.is_some_and(|c| category.contains(c)))
            .filter(|e| match area {
                Some((p, _)) => {
                    let (dx, dz) = (e.position().x - p.x, e.position().z - p.z);
                    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= r2.unwrap_or(Fx::ZERO)
                }
                None => true,
            })
            .filter_map(|e| e.unit.as_deref().map(|u| u.threat[ty as usize]))
            .fold(Fx::ZERO, |a, b| a.saturating_add(b));
        Ok(total.to_num::<f64>())
    }
    lua_method!(lua, moho::Platoon,
        /// Sum of the live members' threat of type `ty` ('Surface', 'Air', 'Sub' are the blueprint
        /// levels, otherwise a threat type name) for members in `category`.
        CalculatePlatoonThreat = |lua, (this, ty, category): (Obj<moho::Platoon>, String, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            platoon_threat(lua, &this, ty, &category.0, None)
        },
    )?;
    lua_method!(lua, moho::Platoon,
        /// Like `CalculatePlatoonThreat`, counting only members within `radius` of `pos` (XZ plane).
        CalculatePlatoonThreatAroundPosition = |lua, (this, ty, category, pos, radius): (Obj<moho::Platoon>, String, LuaUserDataRef<crate::engine::api::categories::EntityCategory>, shapes::Vector, f64)| {
            let p = read_location(Some(&pos))?.unwrap_or_default();
            platoon_threat(lua, &this, ty, &category.0, Some((p, radius)))
        },
    )?;
    lua_method!(lua, moho::Platoon,
        /// Could `FormPlatoon` take units from this platoon for `template`?
        CanFormPlatoon = |lua, (this, template, multiplier, location, radius): (Obj<moho::Platoon>, shapes::PlatoonTemplate, Option<f64>, Option<shapes::Vector>, Option<f64>)| {
            let (_, _, squads) = parse_template(lua, &template, multiplier.unwrap_or(1.0))?;
            Ok(select_for_template(lua, &this, &squads, read_location(location.as_deref())?, radius)?.is_some())
        },
    )?;
    lua_method!(lua, moho::Platoon,
        /// Moves the template's units out of this platoon into a new one (label
        /// = template name, plan = template plan) and runs its
        /// `OnUnitsAddedToPlatoon`; nil if the template can't be filled.
        FormPlatoon = |lua, (this, template, multiplier, location, radius): (Obj<moho::Platoon>, shapes::PlatoonTemplate, Option<f64>, Option<shapes::Vector>, Option<f64>)| {
            let (name, plan, squads) = parse_template(lua, &template, multiplier.unwrap_or(1.0))?;
            let Some(picked) = select_for_template(lua, &this, &squads, read_location(location.as_deref())?, radius)? else {
                return Ok(None);
            };
            let army = platoons(lua).get(platoon_id(&this)?).map(|p| p.army).unwrap_or(0);
            let platoon = make_platoon(lua, army, &name, &plan)?;
            let id = platoon_id(&platoon)?;
            {
                let mut p = platoons(lua);
                for (unit, squad) in &picked {
                    p.assign(*unit, id, squad);
                }
            }
            PlatoonCallbacks::on_units_added_to_platoon(lua, &platoon, "platoon")?;
            Ok(Some(platoon))
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// How many of the army's platoons have this name (`MakePlatoon`'s first
        /// argument, a formed platoon's template name), ignoring case; 0 when
        /// the name isn't a string. Moho counts its army's platoon list with
        /// `_stricmp` (FA exe 0x58ce30 -> `CArmyImpl` vtable +0x84, 0x700770).
        GetNumPlatoonsTemplateNamed = |lua, (brain, name): (Obj<moho::AIBrain>, LuaValue)| {
            let ArmyHandle(army) = script_object::handle(&brain)?;
            let LuaValue::String(name) = name else { return Ok(0.0) };
            let name = name.to_string_lossy();
            Ok(platoons(lua).of_army(army).filter(|p| p.label.eq_ignore_ascii_case(&name)).count() as f64)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// The army's platoons in creation order, without its ArmyPool (inferred).
        GetPlatoonsList = |lua, brain: Obj<moho::AIBrain>| {
            let ArmyHandle(army) = script_object::handle(&brain)?;
            let ids: Vec<PlatoonId> = platoons(lua).of_army(army).filter(|p| p.unique_name.as_deref() != Some("ArmyPool")).map(|p| p.id).collect();
            let reg = objects(lua)?;
            let out = lua.create_table_with_capacity(ids.len(), 0)?;
            let mut n = 0;
            for id in ids {
                if let Some(obj) = reg.raw_get::<Option<LuaTable>>(id)? {
                    n += 1;
                    out.raw_set(n, obj)?;
                }
            }
            Ok(shapes::List::<Obj<moho::Platoon>>::new(out))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Creates a platoon for the brain's army with the given label and plan (empty when nil) and
        /// runs its `OnCreate`.
        MakePlatoon = |lua, (brain, label, plan): (Obj<moho::AIBrain>, Option<String>, Option<String>)| {
            let ArmyHandle(army) = script_object::handle(&brain)?;
            make_platoon(lua, army, label.as_deref().unwrap_or(""), plan.as_deref().unwrap_or(""))
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// The army's platoon holding that unique name, or nil.
        GetPlatoonUniquelyNamed = |lua, (brain, name): (Obj<moho::AIBrain>, String)| {
            let ArmyHandle(army) = script_object::handle(&brain)?;
            let id = platoons(lua).by_unique_name(army, &name);
            match id {
                Some(id) => Ok(objects(lua)?.raw_get::<Option<LuaTable>>(id)?.map(Obj::<moho::Platoon>::new)),
                None => Ok(None),
            }
        },
    )?;
    Ok(())
}

/// Disbands platoon `id`: units back to the army pool, `OnDestroy` on its
/// script object (FAF's clears its trash, killing the platoon's threads),
/// then the platoon gone. Also how platoons end when their last unit dies
/// (`entity::compact`). `OnDestroy` runs while the platoon still exists, so
/// `self:GetBrain()` there is its brain: FAF's `DoDestroyCallbacks` and
/// LOUD's `OnDestroy` hand it to every destroy callback (LOUD releases its
/// builders' instances with it). An `OnDestroy` that disbands the
/// platoon again finds no script object and only removes it.
pub fn destroy(lua: &Lua, id: PlatoonId) -> LuaResult<()> {
    let Some(p) = platoons(lua).get(id).cloned() else { return Ok(()) };
    if p.unique_name.as_deref() == Some("ArmyPool") {
        return Ok(());
    }
    {
        let mut ps = platoons(lua);
        if let Some(pool) = ps.by_unique_name(p.army, "ArmyPool") {
            for (u, _) in &p.units {
                ps.assign(*u, pool, "Unassigned");
            }
        }
    }
    let obj = objects(lua)?.raw_get::<Option<LuaTable>>(id)?;
    objects(lua)?.raw_set(id, LuaNil)?;
    let result = match obj {
        Some(obj) => PlatoonCallbacks::on_destroy(lua, &obj, "platoon"),
        None => Ok(()),
    };
    platoons(lua).remove(id);
    result
}


/// Any of `units` has an enabled weapon allowed to fire at `target`'s layer.
fn can_attack(lua: &Lua, units: &[EntityId], target: EntityId) -> bool {
    let Some(layer) = crate::engine::api::entity::world(lua).get(target).and_then(|e| e.unit.as_deref()).map(|u| u.layer.mask()) else {
        return false;
    };
    let Some(weapons) = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>() else { return false };
    weapons.iter().any(|w| w.enabled && units.contains(&w.unit) && w.fire_target_layers.intersects(layer))
}
