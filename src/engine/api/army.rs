//! Army brains: creation during session launch and `moho.aibrain_methods`.

use crate::lua_api::shapes::{self, Shaped};
use mlua::prelude::*;

use crate::engine::api::moho::{self, MohoClass};
use crate::engine::api::callbacks::AIBrainCallbacks;
use crate::engine::api::script_object;
use crate::lua_api::{Obj, Status, lua_fn, lua_method};
use crate::engine::sim::army::{Alliance, Armies, Army, Comparator, Economy, Resource, StatTrigger, StatValue};
use crate::engine::sim::math::{Fx, FxVec2};

/// `_c_object` of an AI brain.
#[derive(Clone, Copy, Debug)]
pub struct ArmyHandle(pub u32);
impl LuaUserData for ArmyHandle {}

/// The army this client views the game as; -1 = observer. Client-local, so
/// the sim must only use it for presentation (FAF uses it for Sync/UI data).
#[derive(Clone, Copy, Debug)]
pub struct FocusArmy(pub i32);

/// The session plays back a recorded replay (`SessionIsReplay`). Client-
/// local like [`FocusArmy`]: FAF reads it for Sync/UI data only.
#[derive(Clone, Copy, Debug, Default)]
pub struct Replay(pub bool);

/// Moho's sim console variable `AI_RunOpponentAI` (default true; no sim
/// console commands yet, so nothing turns it off).
pub(crate) fn opponent_ai_running(lua: &Lua) -> bool {
    lua.named_registry_value::<Option<bool>>("sim.ai_run_opponent_ai").ok().flatten().unwrap_or(true)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Armies::default());
    lua.set_app_data(FocusArmy(-1));
    lua_fn!(lua, global,
        /// Whether the armies start with their initial units (false when a
        /// save or a test places them; the session sets it).
        ShouldCreateInitialArmyUnits = |lua, ()| Ok(lua.named_registry_value::<Option<bool>>("sim.create_initial_units")?.unwrap_or(true)))?;
    lua_fn!(lua, global,
        /// Sets where `army` starts, in world units (x, z).
        SetArmyStart = |lua, (army, x, z): (shapes::Army, f64, f64)| {
            let index = resolve_army(lua, &army)?;
            let mut armies = lua.app_data_mut::<Armies>().expect("Armies not in app_data");
            if let Some(a) = armies.get_mut(index) {
                a.start = Some(FxVec2::from_f64(x, z));
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Sets the AI plans string handed to the army's `OnCreateAI`.
        SetArmyPlans = |lua, (army, plans): (shapes::Army, Option<String>)| {
            let index = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.plans = plans;
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Runs the brain's `OnCreateAI(plans)` now, for AI armies: FAF's
        /// `OnCreateArmyBrain` calls this and right after checks
        /// `brain.BrainType == 'AI'` to enable its AI systems.
        InitializeArmyAI = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            let (first, human, plans) = {
                let mut armies = lua.app_data_mut::<Armies>().expect("Armies");
                match armies.get_mut(index) {
                    Some(a) => (!std::mem::replace(&mut a.ai_initialized, true), a.human, a.plans.clone()),
                    None => return Ok(()),
                }
            };
            if first && !human
                && let LuaValue::Table(brain) = brain(lua, index)?
                && let Some(f) = brain.get::<Option<LuaFunction>>("OnCreateAI")? {
                f.call::<()>((brain, plans))?;
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Gives an army without a start marker (usually civilians, which get
        /// no initial unit) a start position. Moho's exact choice is
        /// undocumented; we use the map centre, which is deterministic.
        GenerateArmyStart = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            let size: LuaTable = lua.globals().get::<LuaTable>("ScenarioInfo")?.get("size")?;
            let (w, h): (f64, f64) = (size.get(1)?, size.get(2)?);
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.start = Some(FxVec2::from_f64(w / 2.0, h / 2.0));
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Army names in index order. Called during OnCreateArmyBrain, so it
        /// lists every configured army, not just those created so far.
        ListArmies = |lua, ()| {
            let scenario: LuaTable = lua.globals().get("ScenarioInfo")?;
            let setup: LuaTable = scenario.get("ArmySetup")?;
            let mut names: Vec<(u32, String)> = Vec::new();
            for pair in setup.pairs::<String, LuaTable>() {
                let (name, opts) = pair?;
                names.push((opts.get("ArmyIndex")?, name));
            }
            names.sort_unstable();
            Ok(names.into_iter().map(|(_, n)| n).collect::<Vec<_>>())
        })?;
    lua_fn!(lua, global,
        /// The army's brain script object.
        GetArmyBrain = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            brain_obj(lua, index)
        })?;
    lua_fn!(lua, global,
        /// Sets the army's stored mass and energy.
        SetArmyEconomy = |lua, (army, mass, energy): (shapes::Army, f64, f64)| {
            let index = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.economy.mass.stored = crate::engine::sim::math::from_lua(mass);
                a.economy.energy.stored = crate::engine::sim::math::from_lua(energy);
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Adds `cat` to the categories the army can't build.
        AddBuildRestriction = |lua, (army, cat): (shapes::Army, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            let index = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.build_restrictions = Some(match &a.build_restrictions {
                    Some(r) => r.union(&cat.0),
                    None => cat.0.clone(),
                });
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Deprecated in FAF and of unknown meaning; its only use scales
        /// shield damage by `1 - handicap`. 0: moho's is 0 unless the
        /// `DebugSetArmyHandicap` console command sets it (MohoEngine.dll
        /// 0x102e22c0, 0x102e3bc5), which ours lacks.
        ArmyGetHandicap = |_, _army: shapes::Army| Ok(0.0))?;
    lua_fn!(lua, global,
        /// Removes `cat` from the categories the army can't build.
        RemoveBuildRestriction = |lua, (army, cat): (shapes::Army, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            let index = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index)
                && let Some(r) = &a.build_restrictions {
                a.build_restrictions = Some(r.difference(&cat.0));
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The army's unit cap.
        GetArmyUnitCap = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            Ok(lua.app_data_ref::<Armies>().expect("Armies").get(index).map_or(0, |a| a.unit_cap))
        })?;
    lua_fn!(lua, global,
        /// Sets the army's unit cap (negative values become 0).
        SetArmyUnitCap = |lua, (army, cap): (shapes::Army, f64)| {
            let index = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.unit_cap = cap.max(0.0) as u32;
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Whether the army's units ignore the unit cap.
        SetIgnoreArmyUnitCap = |lua, (army, ignore): (shapes::Army, bool)| {
            let index = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.ignore_unit_cap = ignore;
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Sets the alliance (`'Ally'`, `'Enemy'`, `'Neutral'`) between two
        /// armies, in both directions.
        SetAlliance = |lua, (a1, a2, kind): (shapes::Army, shapes::Army, String)| {
            let (a1, a2) = (resolve_army(lua, &a1)?, resolve_army(lua, &a2)?);
            let a = Alliance::parse(&kind).ok_or_else(|| LuaError::runtime(format!("unknown alliance '{kind}'")))?;
            let mut armies = lua.app_data_mut::<Armies>().expect("Armies");
            armies.set_alliance_one_way(a1, a2, a);
            armies.set_alliance_one_way(a2, a1, a);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `SetAllianceOneWay(army1, army2, alliance)`: how army1 treats army2
        /// only (a campaign map's per-army `Alliances`).
        SetAllianceOneWay = |lua, (a1, a2, kind): (shapes::Army, shapes::Army, String)| {
            let (a1, a2) = (resolve_army(lua, &a1)?, resolve_army(lua, &a2)?);
            let a = Alliance::parse(&kind).ok_or_else(|| LuaError::runtime(format!("unknown alliance '{kind}'")))?;
            lua.app_data_mut::<Armies>().expect("Armies").set_alliance_one_way(a1, a2, a);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `SetArmyFactionIndex(army, index)`: 0-based (FAF passes the lobby's
        /// `Faction - 1`); the brain's `GetFactionIndex` is 1-based.
        SetArmyFactionIndex = |lua, (army, index): (shapes::Army, u32)| {
            let army = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(army) {
                a.faction = index + 1;
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `SetArmyColor(army, r, g, b)`: both of the army's colours, 0..255
        /// each (moho masks them to a byte).
        SetArmyColor = |lua, (army, r, g, b): (shapes::Army, i64, i64, i64)| {
            let army = resolve_army(lua, &army)?;
            let c = 0xFF00_0000 | ((r as u32 & 0xFF) << 16) | ((g as u32 & 0xFF) << 8) | (b as u32 & 0xFF);
            lua.app_data_mut::<Armies>().expect("Armies").set_colors(army, c, c);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `SetArmyColorIndex(army, index)`: both colours from FAF's
        /// `GameColors.PlayerColors`, 0-based (moho's `GetPlayerColor(index)`
        /// reads `PlayerColors[index + 1]`).
        SetArmyColorIndex = |lua, (army, index): (shapes::Army, i64)| {
            let army = resolve_army(lua, &army)?;
            let import: LuaFunction = lua.globals().get("import")?;
            let game_colors: LuaTable = import.call::<LuaTable>("/lua/gamecolors.lua")?.get("GameColors")?;
            let c = game_color(&game_colors, "PlayerColors", index + 1)?.ok_or_else(|| LuaError::runtime(format!("no player colour {index}")))?;
            lua.app_data_mut::<Armies>().expect("Armies").set_colors(army, c, c);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `SetArmyShowScore(army, show)`: whether the interface shows the
        /// army's score (campaign scripts hide their AI armies).
        SetArmyShowScore = |lua, (army, show): (shapes::Army, bool)| {
            let army = resolve_army(lua, &army)?;
            lua.app_data_mut::<Armies>().expect("Armies").set_show_score(army, show);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The army this client views the game as (1-based; -1 = observer).
        GetFocusArmy = |lua, ()| Ok(lua.app_data_ref::<FocusArmy>().map_or(-1, |f| f.0)))?;
    lua_fn!(lua, global,
        /// `SetFocusArmy(index)`: this client now views the game as army
        /// `index` (0-based; -1 observer), as when common army control merges
        /// its army into a teammate's. Client-local like [`FocusArmy`]: then
        /// FAF's `NoteFocusArmyChanged` re-syncs the UI's unit data, and the
        /// snapshot moves the view and the interface over.
        SetFocusArmy = |lua, index: i64| {
            let new = if index < 0 { -1 } else { index as i32 + 1 };
            let old = lua.app_data_ref::<FocusArmy>().map_or(-1, |f| f.0);
            if new == old {
                return Ok(());
            }
            lua.set_app_data(FocusArmy(new));
            // An engine callback: its errors are logged, not the caller's
            // (FAF's raises once a unit has died: `Unit.OnDestroy` leaves
            // `false` in the `UnitData` it walks).
            if let Some(note) = lua.globals().get::<Option<LuaFunction>>("NoteFocusArmyChanged")?
                && let Err(e) = note.call::<()>((new, old)) {
                tracing::warn!("NoteFocusArmyChanged: {e}");
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Whether the session plays back a recorded replay.
        SessionIsReplay = |lua, ()| Ok(lua.app_data_ref::<Replay>().is_some_and(|r| r.0)))?;
    register_brain_methods(lua)?;
    Ok(())
}

const BRAINS_KEY: &str = "engine.brains";

/// Registry table: army index -> brain script object.
/// A stat as `GetArmyStat` returns it.
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "ArmyStat", write_only, rename_all = "PascalCase")]
#[allow(dead_code)]
struct ArmyStat {
    /// A number or a string.
    value: LuaValue,
}

fn brains(lua: &Lua) -> LuaResult<LuaTable> {
    crate::engine::api::script_object::registry_table(lua, BRAINS_KEY)
}

/// The brain script object of army `index` (nil if none).
/// Once a tick after the economy: each army's stat triggers whose condition
/// holds fire once (`brain:OnStatsTrigger(name)`, army then (stat, name)
/// order) and are removed.
pub fn stat_triggers_tick(lua: &Lua) -> LuaResult<()> {
    let fired: Vec<(u32, Vec<String>)> = {
        let Some(mut armies) = lua.app_data_mut::<Armies>() else { return Ok(()) };
        armies.list.iter_mut().filter(|a| !a.stat_triggers.is_empty()).map(|a| (a.index, a.fire_stat_triggers())).filter(|(_, f)| !f.is_empty()).collect()
    };
    for (index, names) in fired {
        let LuaValue::Table(b) = brain(lua, index)? else { continue };
        for name in names {
            AIBrainCallbacks::on_stats_trigger(lua, &b, "brain", name)?;
        }
    }
    Ok(())
}

pub fn brain(lua: &Lua, index: u32) -> LuaResult<LuaValue> {
    brains(lua)?.raw_get(index)
}

/// [`brain`], declared as an `AIBrain` (nil when the army has none).
pub fn brain_obj(lua: &Lua, index: u32) -> LuaResult<Option<Obj<moho::AIBrain>>> {
    Ok(match brain(lua, index)? {
        LuaValue::Table(t) => Some(Obj::new(t)),
        _ => None,
    })
}

/// Native `moho.aibrain_methods`; `self` is the brain script object.
fn register_brain_methods(lua: &Lua) -> LuaResult<()> {
    lua_method!(lua, moho::AIBrain,
        /// Whether the army shares its resources with its allies.
        SetResourceSharing = |lua, (brain, on): (Obj<moho::AIBrain>, bool)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.resource_sharing = on;
            }
            Ok(())
        })?;
    /// Registers `brain:<name>(resource)` reading one economy value.
    fn econ_getter(
        lua: &Lua,
        name: &'static str,
        doc: &'static str,
        value: fn(&crate::engine::sim::economy::ResourceState) -> Fx,
    ) -> LuaResult<()> {
        crate::lua_api::add_class_fn(
            lua,
            moho::AIBrain::CLASS,
            name,
            Some(&[Some("this"), Some("resource")]),
            doc,
            Status::Implemented,
            move |lua, (brain, kind): (Obj<moho::AIBrain>, String)| {
                let ArmyHandle(index) = script_object::handle(&brain)?;
                let r = parse_resource(&kind)?;
                let armies = lua.app_data_ref::<Armies>().expect("Armies");
                Ok(armies.get(index).map_or(0.0, |a| value(a.economy.get(r)).to_num::<f64>()))
            },
        )
    }
    econ_getter(lua, "GetEconomyStoredRatio", "Stored / capacity of `resource` (`'MASS'` or `'ENERGY'`), 0 without capacity.", |r| if r.capacity > Fx::ZERO { r.stored.saturating_div(r.capacity) } else { Fx::ZERO })?;
    econ_getter(lua, "GetEconomyStored", "Stored amount of `resource` (`'MASS'` or `'ENERGY'`).", |r| r.stored)?;
    econ_getter(lua, "GetEconomyIncome", "Income of `resource` (`'MASS'` or `'ENERGY'`) per tick.", |r| r.income)?;
    econ_getter(lua, "GetEconomyRequested", "Requested amount of `resource` (`'MASS'` or `'ENERGY'`).", |r| r.requested)?;
    econ_getter(lua, "GetEconomyUsage", "Expense of `resource` (`'MASS'` or `'ENERGY'`).", |r| r.expense)?;
    econ_getter(lua, "GetEconomyTrend", "Income minus expense of `resource` (`'MASS'` or `'ENERGY'`).", |r| r.income - r.expense)?;
    lua_method!(lua, moho::AIBrain,
        /// Gives `amount` (clamped to at least 0) of `kind`: it's stored on
        /// the army's next economy tick, up to its storage (what doesn't fit
        /// overflows like income), so `GetEconomyStored` doesn't show it
        /// until then. Moho adds it to a pending pair beside the stored one
        /// (economy +0x8 vs +0x18, `CAiBrain:GiveResource` MohoEngine.dll
        /// 0x10179dc0, `GetEconomyStored` 0x10180690); FAF probe
        /// `probe_auditgaps` reads the old amount right after giving.
        GiveResource = |lua, (brain, kind, amount): (Obj<moho::AIBrain>, String, f64)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let r = parse_resource(&kind)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                let res = a.economy.get_mut(r);
                res.banked = res.banked.saturating_add(crate::engine::sim::math::from_lua(amount.max(0.0)));
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Takes at most what is stored; returns the amount actually taken.
        TakeResource = |lua, (brain, kind, amount): (Obj<moho::AIBrain>, String, f64)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let r = parse_resource(&kind)?;
            let mut armies = lua.app_data_mut::<Armies>().expect("Armies");
            let Some(a) = armies.get_mut(index) else { return Ok(0.0) };
            let res = a.economy.get_mut(r);
            let taken = crate::engine::sim::math::from_lua(amount.max(0.0)).min(res.stored);
            res.stored -= taken;
            Ok(taken.to_num::<f64>())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// This army's live units matching `category`, in entity id order.
        /// `needToBeIdle` is ignored (FAF notes it isn't functional in moho);
        /// `requireBuilt` drops units still under construction.
        GetListOfUnits = |lua,
         (brain, category, _need_to_be_idle, require_built): (
            Obj<moho::AIBrain>,
            LuaUserDataRef<crate::engine::api::categories::EntityCategory>,
            Option<bool>,
            Option<bool>,
        )| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let require_built = require_built.unwrap_or(false);
            let ids: Vec<u32> = crate::engine::api::entity::world(lua)
                .iter()
                .filter(|e| e.unit.is_some() && !e.destroyed && e.army == Some(index))
                .filter(|e| e.category_index.is_some_and(|i| category.0.contains(i)))
                .filter(|e| !require_built || e.fraction_complete >= Fx::ONE)
                .map(|e| e.id)
                .collect();
            crate::engine::api::entity::objects_table::<moho::Unit>(lua, &ids)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Whether opponent AI should run: moho's sim console variable
        /// `AI_RunOpponentAI` (default true; MohoEngine.dll 0x10177dd0, the
        /// variable 0x109c1f4c). AI mods (Uveso) wait on it.
        IsOpponentAIRunning = |lua, _this: Obj<moho::AIBrain>| Ok(opponent_ai_running(lua)))?;
    lua_method!(lua, moho::AIBrain,
        /// The army's idle structure factories (`FACTORY - MOBILE`, finished,
        /// nothing queued, not busy), id order; with a radius > 0 only those
        /// within it of `position` on the ground plane (MohoEngine.dll
        /// 0x1016b3d0; it also skips units failing one check not identified
        /// yet, see docs/research/verifying-against-the-binary.md).
        GetAvailableFactories = |lua, (brain, position, radius): (Obj<moho::AIBrain>, Option<shapes::Vector>, Option<f64>)| {
            use crate::engine::sim::unit_state::UnitStates;
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let domain = crate::engine::api::categories::domain(lua)?;
            let (Some(factory), mobile) = (domain.named.get("FACTORY"), domain.named.get("MOBILE")) else { return crate::engine::api::entity::objects_table::<moho::Unit>(lua, &[]) };
            let busy = UnitStates::index("Busy");
            let within = match (position, radius) {
                (Some(p), Some(r)) if r > 0.0 => Some((
                    crate::engine::sim::math::from_lua(p.raw_get::<f64>(1)?),
                    crate::engine::sim::math::from_lua(p.raw_get::<f64>(3)?),
                    crate::engine::sim::math::from_lua(r),
                )),
                _ => None,
            };
            let commands = lua.app_data_ref::<crate::engine::sim::command::Commands>();
            let ids: Vec<u32> = crate::engine::api::entity::world(lua)
                .iter()
                .filter(|e| !e.destroyed && e.army == Some(index) && e.fraction_complete >= Fx::ONE)
                .filter(|e| e.category_index.is_some_and(|i| factory.contains(i) && !mobile.is_some_and(|m| m.contains(i))))
                .filter(|e| e.unit.as_deref().is_some_and(|u| !busy.is_some_and(|b| u.states.contains(b))))
                .filter(|e| commands.as_ref().is_none_or(|c| c.queue(e.id).next().is_none()))
                .filter(|e| within.is_none_or(|(x, z, r)| {
                    let p = e.position();
                    let (dx, dz) = (p.x - x, p.z - z);
                    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= r.saturating_mul(r)
                }))
                .map(|e| e.id)
                .collect();
            drop(commands);
            crate::engine::api::entity::objects_table::<moho::Unit>(lua, &ids)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// How many of the army's finished units in `builder_category` are
        /// building or upgrading something in `buildee_category` (their focus
        /// unit; MohoEngine.dll 0x1017cf30). The campaign base managers ask.
        NumCurrentlyBuilding = |lua, (brain, buildee_category, builder_category): (
            Obj<moho::AIBrain>,
            LuaUserDataRef<crate::engine::api::categories::EntityCategory>,
            LuaUserDataRef<crate::engine::api::categories::EntityCategory>,
        )| {
            use crate::engine::sim::unit_state::UnitStates;
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let busy = [UnitStates::index("Building"), UnitStates::index("Upgrading")];
            let w = crate::engine::api::entity::world(lua);
            let in_category = |id: u32, cat: &crate::engine::api::categories::EntityCategory| {
                w.get(id).is_some_and(|e| e.unit.is_some() && e.category_index.is_some_and(|i| cat.0.contains(i)))
            };
            Ok(w.iter()
                .filter(|e| !e.destroyed && e.army == Some(index) && e.fraction_complete >= Fx::ONE)
                .filter(|e| e.category_index.is_some_and(|i| builder_category.0.contains(i)))
                .filter_map(|e| e.unit.as_deref())
                .filter(|u| busy.iter().flatten().any(|&s| u.states.contains(s)))
                .filter(|u| u.focus.is_some_and(|f| in_category(f, &buildee_category)))
                .count() as u32)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Whether any of the army's finished `ENGINEER` units (ACUs and SCUs
        /// included, factories not) is in unit state Building with a focus
        /// unit in `category`: what is under construction now, including
        /// assisting it, not queued orders (MohoEngine.dll 0x1017df10). RNGAI
        /// asks before starting another power or hydro build.
        IsAnyEngineerBuilding = |lua, (brain, category): (
            Obj<moho::AIBrain>,
            LuaUserDataRef<crate::engine::api::categories::EntityCategory>,
        )| {
            use crate::engine::sim::unit_state::UnitStates;
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let domain = crate::engine::api::categories::domain(lua)?;
            let (Some(engineer), Some(building)) = (domain.named.get("ENGINEER"), UnitStates::index("Building")) else { return Ok(false) };
            let w = crate::engine::api::entity::world(lua);
            let focus_in_category = |id: u32| w.get(id).is_some_and(|e| e.unit.is_some() && e.category_index.is_some_and(|i| category.0.contains(i)));
            Ok(w.iter()
                .filter(|e| !e.destroyed && e.army == Some(index) && e.fraction_complete >= Fx::ONE)
                .filter(|e| e.category_index.is_some_and(|i| engineer.contains(i)))
                .filter_map(|e| e.unit.as_deref())
                .any(|u| u.states.contains(building) && u.focus.is_some_and(focus_in_category)))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// How many units around `position` (within `radius` on the XZ plane)
        /// match `category`, optionally only those with the given alliance
        /// (see `GetUnitsAroundPoint`).
        GetNumUnitsAroundPoint = |lua, (brain, category, position, radius, alliance): (Obj<moho::AIBrain>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>, shapes::Vector, f64, Option<String>)| {
            Ok(units_around(lua, &brain, &category.0, &position, radius, alliance)?.len())
        })?;
    /// Units around a point for `GetUnitsAroundPoint` / `GetNumUnitsAroundPoint`:
    /// within an XZ circle, matching `category`, optionally only those with
    /// the given alliance to this brain; in entity id order. Takes intel into
    /// account (FAF's docs): units of other non-allied armies count only
    /// while this brain detects them (seen or on radar / sonar / omni).
    fn units_around(lua: &Lua, brain: &LuaTable, category: &crate::engine::sim::category::CategorySet, position: &LuaTable, radius: f64, alliance: Option<String>) -> LuaResult<Vec<u32>> {
        let ArmyHandle(index) = script_object::handle(brain)?;
        let alliance = alliance
            .map(|a| Alliance::parse(&a).ok_or_else(|| LuaError::runtime(format!("unknown alliance '{a}'"))))
            .transpose()?;
        let (cx, cz) = (crate::engine::sim::math::from_lua(position.raw_get::<f64>(1)?), crate::engine::sim::math::from_lua(position.raw_get::<f64>(3)?));
        let r = crate::engine::sim::math::from_lua(radius);
        let world = crate::engine::api::entity::world(lua);
        let armies = lua.app_data_ref::<Armies>().expect("Armies");
        let intel = lua.app_data_ref::<crate::engine::sim::vision::IntelGrids>();
        Ok(world
            .units_in_circle(cx, cz, r)
            .into_iter()
            .filter_map(|id| world.get(id))
            .filter(|e| e.category_index.is_some_and(|i| category.contains(i)))
            .filter(|e| match (alliance, e.army) {
                (None, _) => true,
                (Some(want), Some(army)) => armies.alliance(index, army) == want,
                (Some(_), None) => false,
            })
            .filter(|e| match (e.army, &intel) {
                (Some(army), Some(g)) if e.unit.is_some() && army != index && armies.alliance(index, army) != Alliance::Ally => g.detect(index, e).any(),
                _ => true,
            })
            .map(|e| e.id)
            .collect())
    }
    lua_method!(lua, moho::AIBrain,
        /// Units around `position` (within `radius` on the XZ plane) matching
        /// `category`, optionally only those with the given alliance to this
        /// brain; in entity id order. Units of other non-allied armies are
        /// listed only while this brain detects them.
        GetUnitsAroundPoint = |lua,
         (brain, category, position, radius, alliance): (Obj<moho::AIBrain>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>, shapes::Vector, f64, Option<String>)| {
            let ids = units_around(lua, &brain, &category.0, &position, radius, alliance)?;
            crate::engine::api::entity::objects_table::<moho::Unit>(lua, &ids)
        })?;
    /// Registers global `<name>(army1, army2)`: is army1's stance to army2 `want`?
    fn alliance_query(lua: &Lua, name: &'static str, doc: &'static str, want: Alliance) -> LuaResult<()> {
        crate::lua_api::register_fn(
            lua,
            &lua.globals(),
            "",
            name,
            Some(&[Some("army1"), Some("army2")]),
            doc,
            Status::Implemented,
            false,
            move |lua, (a, b): (shapes::Army, shapes::Army)| {
                let (a, b) = (resolve_army(lua, &a)?, resolve_army(lua, &b)?);
                Ok(lua.app_data_ref::<Armies>().expect("Armies").alliance(a, b) == want)
            },
        )
    }
    alliance_query(lua, "IsAlly", "Whether army1 is allied to army2.", Alliance::Ally)?;
    alliance_query(lua, "IsEnemy", "Whether army1 is an enemy of army2.", Alliance::Enemy)?;
    alliance_query(lua, "IsNeutral", "Whether army1 is neutral to army2.", Alliance::Neutral)?;
    lua_fn!(lua, global,
        /// Whether the army has left the game (defeated).
        ArmyIsOutOfGame = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            Ok(lua.app_data_ref::<Armies>().expect("Armies").get(index).is_some_and(|a| a.out_of_game))
        })?;
    lua_fn!(lua, global,
        /// A defeated army leaves the game (FAF's victory conditions, before
        /// `brain:OnDefeat`, which shares its unit cap): its players can't
        /// command it any more (`OkayToMessWithArmy`); `ArmyIsOutOfGame` and
        /// `brain:IsDefeated` say so.
        SetArmyOutOfGame = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").list.iter_mut().find(|a| a.index == index) {
                a.out_of_game = true;
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Whether the army is a civilian army.
        ArmyIsCivilian = |lua, army: shapes::Army| {
            let index = resolve_army(lua, &army)?;
            Ok(lua.app_data_ref::<Armies>().expect("Armies").get(index).is_some_and(|a| a.civilian))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Returns the enemy's brain (FAF calls brain methods on it), or nil.
        GetCurrentEnemy = |lua, brain_obj: Obj<moho::AIBrain>| {
            let ArmyHandle(index) = script_object::handle(&brain_obj)?;
            let enemy = lua.app_data_ref::<Armies>().expect("Armies").get(index).and_then(|a| a.current_enemy);
            match enemy {
                Some(e) => crate::engine::api::army::brain_obj(lua, e),
                None => Ok(None),
            }
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Sets the army's current enemy (an army or its brain), or clears it with nil.
        SetCurrentEnemy = |lua, (brain, enemy): (Obj<moho::AIBrain>, LuaValue)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let enemy = if enemy.is_nil() { None } else { Some(resolve_army(lua, &enemy)?) };
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.current_enemy = enemy;
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Sum of `stat` over the blueprints in `category` (BTreeMap range
        /// scan, so the summation order is fixed).
        GetBlueprintStat = |lua, (brain, stat, category): (Obj<moho::AIBrain>, String, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let armies = lua.app_data_ref::<Armies>().expect("Armies");
            let Some(a) = armies.get(index) else { return Ok(0.0) };
            Ok(a.blueprint_stats
                .range((std::borrow::Cow::Owned(stat.clone()), 0)..=(std::borrow::Cow::Owned(stat), u32::MAX))
                .filter(|((_, bp), _)| category.0.contains(*bp))
                .map(|(_, v)| *v)
                .sum::<f64>())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Returns the stat as `{ Value = v }` (FAF reads `.Value`), creating
        /// it with `default` (0 if absent) on first use. The stats moho keeps
        /// itself (economy, unit cap) come from the army's live state.
        GetArmyStat = |lua, (brain, name, default): (Obj<moho::AIBrain>, String, LuaValue)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let value = {
                if let Some(v) = engine_army_stat(lua, index, &name) {
                    let t = lua.create_table_with_capacity(0, 1)?;
                    t.raw_set("Value", v)?;
                    return Ok(Some(Shaped::<ArmyStat>::new(t)));
                }
                let mut armies = lua.app_data_mut::<Armies>().expect("Armies");
                let Some(a) = armies.get_mut(index) else { return Ok(None) };
                match a.stats.get(&name) {
                    Some(v) => v.clone(),
                    None => {
                        let v = if default.is_nil() { StatValue::Number(0.0) } else { stat_value(&default)? };
                        a.stats.insert(name, v.clone());
                        v
                    }
                }
            };
            let t = lua.create_table_with_capacity(0, 1)?;
            match value {
                StatValue::Number(n) => t.raw_set("Value", n)?,
                StatValue::Text(s) => t.raw_set("Value", s)?,
            }
            Ok(Some(Shaped::<ArmyStat>::new(t)))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Live units of this army in `category` (finished or not).
        GetCurrentUnits = |lua, (brain, category): (Obj<moho::AIBrain>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let w = crate::engine::api::entity::world(lua);
            Ok(w.iter()
                .filter(|e| e.kind == crate::engine::sim::entity::EntityKind::Unit && !e.destroyed && e.army == Some(index))
                .filter(|e| e.category_index.is_some_and(|c| category.0.contains(c)))
                .count())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Ticks left of the lobby's no-rush period (`NoRushOption`, minutes).
        GetNoRushTicks = |lua, _brain: Obj<moho::AIBrain>| {
            let minutes = lua
                .globals()
                .get::<LuaTable>("ScenarioInfo")?
                .get::<Option<LuaTable>>("Options")?
                .and_then(|o| o.get::<Option<String>>("NoRushOption").ok().flatten())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let tick = lua.globals().get::<LuaFunction>("GetGameTick")?.call::<i64>(())?;
            Ok((minutes * 600 - tick).max(0))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// The unit blueprint table for blueprint id `id`, or nil.
        GetUnitBlueprint = |lua, (_brain, id): (Obj<moho::AIBrain>, String)| {
            let reg = lua.app_data_ref::<crate::engine::api::blueprints::BlueprintRegistry>().expect("BlueprintRegistry");
            Ok(reg.get(crate::engine::api::blueprints::BlueprintKind::Unit, &id.to_lowercase()).map(|b| shapes::UnitBlueprint(b.table.clone())))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// `upgradeList` is a list of `{ from, to }` blueprint id pairs
        /// (`/lua/upgradetemplates.lua`); returns `to` for `unit`, else nil.
        FindUpgradeBP = |_, (_brain, unit, list): (Obj<moho::AIBrain>, String, Option<shapes::List<shapes::List<String>>>)| {
            let Some(list) = list else { return Ok(None) };
            for pair in list.sequence_values::<LuaValue>() {
                let LuaValue::Table(pair) = pair? else { continue };
                let from: Option<String> = pair.raw_get(1)?;
                if from.is_some_and(|f| f.eq_ignore_ascii_case(&unit)) {
                    return pair.raw_get::<Option<String>>(2);
                }
            }
            Ok(None)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// The army's index (1-based).
        GetArmyIndex = |_, brain: Obj<moho::AIBrain>| {
            Ok(script_object::handle::<ArmyHandle>(&brain)?.0)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Sets the army's script storage to `amount` of `kind` and none of
        /// the other resource (each call replaces the last; whole units).
        GiveStorage = |lua, (brain, kind, amount): (Obj<moho::AIBrain>, String, f64)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let r = parse_resource(&kind)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.economy.set_script_storage(r, crate::engine::sim::math::from_lua(amount));
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Test hook: sets the army's script storage of `kind` to `amount`,
        /// leaving the other resource's (`GiveStorage` can only give one).
        #[lua(internal)]
        __engine_SetStorage = |lua, (brain, kind, amount): (Obj<moho::AIBrain>, String, f64)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let r = parse_resource(&kind)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.economy.set_script_storage_half(r, crate::engine::sim::math::from_lua(amount));
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Sets a script-kept army stat to a number or string.
        SetArmyStat = |lua, (brain, name, value): (Obj<moho::AIBrain>, String, LuaValue)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let value = stat_value(&value)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.stats.insert(name, value);
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Calls `brain:OnStatsTrigger(name)` once when `stat` compares true
        /// against `value`. The category argument is ignored.
        SetArmyStatsTrigger = |lua,
         (brain, stat, name, compare, value, _category): (
            Obj<moho::AIBrain>,
            String,
            String,
            String,
            f64,
            Option<LuaValue>,
        )| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let compare = Comparator::parse(&compare)
                .ok_or_else(|| LuaError::runtime(format!("unknown comparator '{compare}'")))?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.stat_triggers.insert((stat, name), StatTrigger { compare, value });
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Trigger names are unique per army ("See `RemoveArmyStatsTrigger` to
        /// remove occupied names"), and FAF's energy manager removes a name
        /// under a stat other than the one it set it on, so the name alone
        /// decides.
        RemoveArmyStatsTrigger = |lua, (brain, _stat, name): (Obj<moho::AIBrain>, String, String)| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                a.stat_triggers.retain(|(_, n), _| *n != name);
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// The army's faction, 1-based (nil if the army is unknown).
        GetFactionIndex = |lua, brain: Obj<moho::AIBrain>| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            Ok(lua.app_data_ref::<Armies>().and_then(|a| a.get(index).map(|a| a.faction)))
        })?;
    lua_method!(lua, moho::AIBrain,
        /// The army's start position as `x, z` (nil, nil if it has none).
        GetArmyStartPos = |lua, brain: Obj<moho::AIBrain>| {
            let ArmyHandle(index) = script_object::handle(&brain)?;
            let start = lua.app_data_ref::<Armies>().and_then(|a| a.get(index).and_then(|a| a.start));
            Ok(match start {
                Some(p) => {
                    let (x, z) = p.to_f64();
                    (Some(x), Some(z))
                }
                None => (None, None),
            })
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Fraction of the map that is water.
        GetMapWaterRatio = |lua, _brain: Obj<moho::AIBrain>| {
            Ok(lua
                .app_data_ref::<crate::engine::sim::terrain::Terrain>()
                .map_or(0.0, |t| t.water_ratio))
        })?;
    Ok(())
}

/// A stat moho keeps up to date itself: the economy ones
/// (`Army::economy_stat`), `UnitCap_MaxCap` (the army's cap) and
/// `UnitCap_Current` (the cap cost of its units, finished or not, as the
/// cap check counts them). `None` for script-kept stats.
fn engine_army_stat(lua: &Lua, index: u32, name: &str) -> Option<f64> {
    let armies = lua.app_data_ref::<Armies>().expect("Armies");
    let army = armies.get(index)?;
    if let Some(v) = army.economy_stat(name) {
        return Some(v.to_num::<f64>());
    }
    match name {
        "UnitCap_MaxCap" => Some(army.unit_cap as f64),
        "UnitCap_Current" => {
            drop(armies);
            let used = crate::engine::api::entity::world(lua)
                .iter()
                .filter(|e| !e.destroyed && e.army == Some(index))
                .filter_map(|e| e.unit.as_deref().map(|u| u.cap_cost))
                .fold(Fx::ZERO, |a, b| a.saturating_add(b));
            Some(used.to_num::<f64>())
        }
        _ => None,
    }
}

fn stat_value(v: &LuaValue) -> LuaResult<StatValue> {
    match v {
        LuaValue::Integer(i) => Ok(StatValue::Number(*i as f64)),
        LuaValue::Number(n) => Ok(StatValue::Number(*n)),
        LuaValue::String(s) => Ok(StatValue::Text(s.to_str()?.to_owned())),
        other => Err(LuaError::runtime(format!("army stat must be a number or string, got {}", other.type_name()))),
    }
}

fn parse_resource(s: &str) -> LuaResult<Resource> {
    Resource::parse(s).ok_or_else(|| LuaError::runtime(format!("unknown resource type '{s}'")))
}

/// Resolves a moho `Army` argument: a 1-based index, an army name, or a brain.
pub fn resolve_army(lua: &Lua, value: &LuaValue) -> LuaResult<u32> {
    let armies = lua.app_data_ref::<Armies>().expect("Armies not in app_data");
    let found = match value {
        LuaValue::Integer(i) => armies.get(*i as u32).map(|a| a.index),
        LuaValue::Number(n) => armies.get(*n as u32).map(|a| a.index),
        LuaValue::String(s) => armies.by_name(&s.to_str()?).map(|a| a.index),
        LuaValue::Table(t) => Some(script_object::handle::<ArmyHandle>(t)?.0),
        _ => None,
    };
    found.ok_or_else(|| LuaError::runtime(format!("unknown army: {value:?}")))
}

/// `GameColors[list][index]` (1-based, as the lobby numbers them) as ARGB.
fn game_color(game_colors: &LuaTable, list: &str, index: i64) -> LuaResult<Option<u32>> {
    let Some(colors) = game_colors.get::<Option<LuaTable>>(list)? else { return Ok(None) };
    Ok(colors.raw_get::<Option<String>>(index)?.as_deref().and_then(crate::engine::api::colors::decode))
}

/// An army's colour and player colour as moho sets them up: civilians
/// `CivilianArmyColor`, others `ArmyColors[ArmyColor]` and
/// `PlayerColors[PlayerColor]`. Without them (a scenario's setup that
/// gives none), our palette's colour for the army.
fn setup_colors(game_colors: &LuaTable, opts: &LuaTable, index: u32, civilian: bool) -> LuaResult<(u32, u32)> {
    let fallback = crate::engine::api::colors::fallback_army_color(index);
    if civilian {
        let c = game_colors.get::<Option<String>>("CivilianArmyColor")?.as_deref().and_then(crate::engine::api::colors::decode).unwrap_or(fallback);
        return Ok((c, c));
    }
    let pick = |list: &str, field: &str| -> LuaResult<u32> {
        let Some(i) = opts.get::<Option<i64>>(field)? else { return Ok(fallback) };
        Ok(game_color(game_colors, list, i)?.unwrap_or(fallback))
    };
    Ok((pick("ArmyColors", "ArmyColor")?, pick("PlayerColors", "PlayerColor")?))
}

/// Creates one army + brain per `ScenarioInfo.ArmySetup` entry (in `ArmyIndex`
/// order) and calls `OnCreateArmyBrain(index, brain, name, nickname)` for each,
/// as moho does before `BeginSession`.
pub fn create_armies(lua: &Lua) -> LuaResult<()> {
    let scenario: LuaTable = lua.globals().get("ScenarioInfo")?;
    let setup: LuaTable = scenario.get("ArmySetup")?;
    // Lobby option `UnitCap` (a string, e.g. '1000'); moho's default 500.
    let unit_cap: u32 = scenario
        .get::<Option<LuaTable>>("Options")?
        .map(|o| o.get::<LuaValue>("UnitCap"))
        .transpose()?
        .and_then(|v| match v {
            LuaValue::String(s) => s.to_str().ok().and_then(|s| s.parse::<f64>().ok()),
            LuaValue::Integer(i) => Some(i as f64),
            LuaValue::Number(n) => Some(n),
            _ => None,
        })
        .map_or(500, |v| v.max(0.0) as u32);
    let mut entries: Vec<(u32, String, LuaTable)> = Vec::new();
    for pair in setup.pairs::<String, LuaTable>() {
        let (name, opts) = pair?;
        entries.push((opts.get("ArmyIndex")?, name, opts));
    }
    entries.sort_by_key(|(i, ..)| *i);
    let max = crate::engine::sim::army::MAX_ARMIES;
    if let Some((i, name, _)) = entries.iter().find(|(i, ..)| *i == 0 || *i > max) {
        return Err(LuaError::RuntimeError(format!("army {name} has index {i}: sessions support armies 1..={max}")));
    }

    // Brains are instances of FAF's AIBrain class (OnCreateArmyBrain may then
    // swap the metatable to an AI-specific class).
    let import: LuaFunction = lua.globals().get("import")?;
    let brain_class: LuaTable = import.call::<LuaTable>("/lua/aibrain.lua")?.get("AIBrain")?;
    let on_create: LuaFunction = lua.globals().get("OnCreateArmyBrain")?;
    // Moho reads the colours from `/lua/gameColors.lua` (with its hooks).
    let game_colors: LuaTable = import.call::<LuaTable>("/lua/gamecolors.lua")?.get("GameColors")?;
    // Each human army's player is a command source, in army order
    // (observers come after them).
    let mut human_sources = 0;
    for (index, name, opts) in entries {
        let nickname: String = opts.get::<Option<String>>("PlayerName")?.unwrap_or_else(|| name.clone());
        let human = opts.get::<Option<bool>>("Human")?.unwrap_or(false);
        let civilian = opts.get::<Option<bool>>("Civilian")?.unwrap_or(false);
        let (color, player_color) = setup_colors(&game_colors, &opts, index, civilian)?;
        let mut command_sources = std::collections::BTreeSet::new();
        if human {
            command_sources.insert(human_sources);
            human_sources += 1;
        }
        lua.app_data_mut::<Armies>().expect("Armies not in app_data").list.push(Army {
            index,
            name: name.clone(),
            nickname: nickname.clone(),
            civilian,
            human,
            faction: opts.get::<Option<u32>>("Faction")?.unwrap_or(1),
            color,
            player_color,
            show_score: !civilian,
            start: None,
            plans: None,
            ai_initialized: false,
            resource_sharing: true,
            economy: Economy::default(),
            stats: Default::default(),
            stat_triggers: Default::default(),
            build_restrictions: None,
            unit_cap,
            ignore_unit_cap: false,
            current_enemy: None,
            out_of_game: false,
            command_sources,
            blueprint_stats: Default::default(),
        });
        let brain: LuaTable = brain_class.call(())?;
        brain.raw_set(script_object::C_OBJECT, ArmyHandle(index))?;
        brains(lua)?.raw_set(index, brain.clone())?;
        crate::engine::api::platoon::create_army_pool(lua, index)?;
        tracing::info!(index, name, color = %format_args!("{color:08x}"), "creating army brain");
        on_create.call::<()>((index, brain.clone(), name, nickname))?;

        // "Called after SetupSession but before BeginSession" (aibrain.lua).
        // AI brains normally got `OnCreateAI` from `InitializeArmyAI` inside
        // `OnCreateArmyBrain`; cover scripts that never call it.
        let (plans, ai_done) = lua
            .app_data_ref::<Armies>()
            .and_then(|a| a.get(index).map(|a| (a.plans.clone(), a.ai_initialized)))
            .unwrap_or((None, false));
        let hook = match (human, ai_done) {
            (true, _) => Some("OnCreateHuman"),
            (false, false) => {
                if let Some(a) = lua.app_data_mut::<Armies>().expect("Armies").get_mut(index) {
                    a.ai_initialized = true;
                }
                Some("OnCreateAI")
            }
            (false, true) => None,
        };
        if let Some(f) = hook.map(|h| brain.get::<Option<LuaFunction>>(h)).transpose()?.flatten() {
            f.call::<()>((brain, plans))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn set_focus_army_is_zero_based_and_notes_the_change() {
        let lua = mlua::Lua::new();
        crate::engine::api::moho::register(&lua).unwrap();
        super::register(&lua).unwrap();
        let r: String = lua
            .load(
                r#"
                local notes = {}
                function NoteFocusArmyChanged(new, old) table.insert(notes, new .. '<' .. old) error('logged, not raised') end
                SetFocusArmy(2)
                local a = GetFocusArmy()
                SetFocusArmy(2)
                SetFocusArmy(-1)
                return a .. ';' .. GetFocusArmy() .. ';' .. table.concat(notes, ',')
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(r, "3;-1;3<-1,-1<3");
    }

    #[test]
    fn set_alliance_one_way_sets_one_direction() {
        use crate::engine::sim::army::{Alliance, Armies, Army};
        let lua = mlua::Lua::new();
        crate::engine::api::moho::register(&lua).unwrap();
        super::register(&lua).unwrap();
        {
            let mut armies = lua.app_data_mut::<Armies>().unwrap();
            for (index, name) in [(1, "Player"), (2, "UEF")] {
                armies.list.push(Army { index, name: name.into(), ..Default::default() });
            }
        }
        lua.load("SetAllianceOneWay('Player', 'UEF', 'Neutral')").exec().unwrap();
        let armies = lua.app_data_ref::<Armies>().unwrap();
        assert_eq!((armies.alliance(1, 2), armies.alliance(2, 1)), (Alliance::Neutral, Alliance::Enemy));
    }

    #[test]
    fn set_army_faction_index_is_zero_based() {
        use crate::engine::sim::army::{Armies, Army};
        let lua = mlua::Lua::new();
        crate::engine::api::moho::register(&lua).unwrap();
        super::register(&lua).unwrap();
        lua.app_data_mut::<Armies>().unwrap().list.push(Army { index: 1, name: "Player".into(), faction: 1, ..Default::default() });
        lua.load("SetArmyFactionIndex('Player', 2)").exec().unwrap();
        assert_eq!(lua.app_data_ref::<Armies>().unwrap().get(1).unwrap().faction, 3);
    }

    #[test]
    fn set_army_color_sets_both_colours() {
        use crate::engine::sim::army::{Armies, Army};
        let lua = mlua::Lua::new();
        crate::engine::api::moho::register(&lua).unwrap();
        super::register(&lua).unwrap();
        lua.app_data_mut::<Armies>().unwrap().list.push(Army { index: 1, name: "UEF".into(), ..Default::default() });
        let before = lua.app_data_ref::<Armies>().unwrap().display_version();
        lua.load("SetArmyColor('UEF', 41, 41, 225 + 256)").exec().unwrap();
        let armies = lua.app_data_ref::<Armies>().unwrap();
        let a = armies.get(1).unwrap();
        assert_eq!((a.color, a.player_color), (0xFF2929E1, 0xFF2929E1));
        assert_ne!(armies.display_version(), before);
    }

    #[test]
    fn set_army_color_index_reads_player_colors_zero_based() {
        use crate::engine::sim::army::{Armies, Army};
        let lua = mlua::Lua::new();
        crate::engine::api::moho::register(&lua).unwrap();
        super::register(&lua).unwrap();
        lua.app_data_mut::<Armies>().unwrap().list.push(Army { index: 1, name: "UEF".into(), ..Default::default() });
        lua.load("function import() return { GameColors = { PlayerColors = { 'FFe80a0a', 'ff901427' } } } end SetArmyColorIndex(1, 1)").exec().unwrap();
        let a = lua.app_data_ref::<Armies>().unwrap().get(1).unwrap().clone();
        assert_eq!((a.color, a.player_color), (0xFF901427, 0xFF901427));
    }

    #[test]
    fn set_army_show_score() {
        use crate::engine::sim::army::{Armies, Army};
        let lua = mlua::Lua::new();
        crate::engine::api::moho::register(&lua).unwrap();
        super::register(&lua).unwrap();
        lua.app_data_mut::<Armies>().unwrap().list.push(Army { index: 1, name: "UEF".into(), show_score: true, ..Default::default() });
        let before = lua.app_data_ref::<Armies>().unwrap().display_version();
        lua.load("SetArmyShowScore('UEF', false)").exec().unwrap();
        let armies = lua.app_data_ref::<Armies>().unwrap();
        assert!(!armies.get(1).unwrap().show_score);
        assert_ne!(armies.display_version(), before);
    }
}
