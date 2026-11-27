//! The session's armies as the interface asks about them: who is in the
//! game, who is allied to whom, the focus army's economy, the players as
//! session clients and command sources.
//!
//! The engine hands the interface these as tables (`UserVm::set_armies`,
//! `set_alliances`, `beat`): `__engine_armies` (by army, 1-based),
//! `__engine_alliances` (`[a][b]`: `Ally` / `Enemy` / `Neutral`),
//! `__engine_economy` and `__engine_resource_sharing` (by army). FAF gets
//! the armies and economy tables themselves, not copies. Render-thread only.

use mlua::prelude::*;

use crate::lua_api::lua_fn;
use crate::lua_api::shapes::{Army, List, Shaped};
use crate::ui::net_lobby::SessionClientData;
use crate::ui::shapes::truthy;
use crate::ui::signatures::{ArmiesTable, EconomyTotals};

/// The session's armies, by army (1-based); none yet: an empty table.
pub fn armies(lua: &Lua) -> LuaResult<LuaTable> {
    match lua.globals().raw_get::<Option<LuaTable>>("__engine_armies")? {
        Some(t) => Ok(t),
        None => lua.create_table(),
    }
}

/// The army the player sees as, 1-based (-1: an observer).
fn focus_army(lua: &Lua) -> LuaResult<i64> {
    lua.named_registry_value::<i64>("user.focus_army")
}

/// The human armies in army order: (army index, the army).
fn humans(lua: &Lua) -> LuaResult<Vec<(LuaValue, LuaTable)>> {
    let mut out = Vec::new();
    for pair in armies(lua)?.pairs::<LuaValue, LuaTable>() {
        let (index, army) = pair?;
        if truthy(&army.get::<LuaValue>("human")?) {
            out.push((index, army));
        }
    }
    Ok(out)
}

/// Is army `a` to army `b` what `relation` says? An army is its own ally;
/// armies the sim hasn't told of are enemies.
fn alliance(lua: &Lua, a: &LuaValue, b: &LuaValue, relation: &str) -> LuaResult<bool> {
    if a == b {
        return Ok(relation == "Ally");
    }
    let known = match lua.globals().raw_get::<Option<LuaTable>>("__engine_alliances")? {
        Some(all) => match all.get::<LuaValue>(a)? {
            LuaValue::Table(row) => row.get::<LuaValue>(b)?,
            _ => LuaNil,
        },
        None => LuaNil,
    };
    Ok(match known {
        LuaValue::String(s) => s.as_bytes() == relation.as_bytes(),
        other if truthy(&other) => false,
        _ => relation == "Enemy",
    })
}

/// A table with each of `keys` set to 0.
fn zeros(lua: &Lua, keys: &[&str]) -> LuaResult<LuaTable> {
    lua.create_table_from(keys.iter().map(|k| (*k, 0)))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// The session's armies: `armiesTable` (by army, 1-based) is the
        /// engine's own table, `focusArmy` as `GetFocusArmy`.
        GetArmiesTable = |lua, ()| {
            let armies = armies(lua)?;
            let t = lua.create_table()?;
            t.raw_set("numArmies", armies.raw_len())?;
            t.raw_set("focusArmy", focus_army(lua)?)?;
            t.raw_set("armiesTable", armies)?;
            Ok(Shaped::<ArmiesTable>::new(t))
        })?;
    lua_fn!(lua, global, IsAlly = |lua, (army1, army2): (Army, Army)| alliance(lua, &army1.0, &army2.0, "Ally"))?;
    lua_fn!(lua, global, IsEnemy = |lua, (army1, army2): (Army, Army)| alliance(lua, &army1.0, &army2.0, "Enemy"))?;
    lua_fn!(lua, global, IsNeutral = |lua, (army1, army2): (Army, Army)| alliance(lua, &army1.0, &army2.0, "Neutral"))?;
    lua_fn!(lua, global,
        /// The focus army's economy (all 0 before the first beat).
        GetEconomyTotals = |lua, ()| {
            if let Some(t) = lua.globals().raw_get::<Option<LuaTable>>("__engine_economy")? {
                return Ok(Shaped::<EconomyTotals>::new(t));
            }
            let t = lua.create_table()?;
            for key in ["stored", "maxStorage", "income", "lastUseRequested", "lastUseActual", "reclaimed"] {
                t.raw_set(key, zeros(lua, &["MASS", "ENERGY"])?)?;
            }
            Ok(Shaped::new(t))
        })?;
    lua_fn!(lua, global,
        /// The focus army's synced overflow flag (moho: false for an
        /// observer; on until the first beat, as a new army's is).
        GetResourceSharing = |lua, ()| {
            let focus = focus_army(lua)?;
            if focus < 1 {
                return Ok(false);
            }
            let flags = lua.globals().raw_get::<Option<LuaTable>>("__engine_resource_sharing")?;
            Ok(match flags.map(|f| f.get::<LuaValue>(focus)).transpose()? {
                None | Some(LuaNil) => true,
                Some(on) => truthy(&on),
            })
        })?;
    lua_fn!(lua, global,
        /// The session's clients: the network's in a networked game, else
        /// each human army's player as a local client. Watching (no human
        /// army): moho still lists the local client, an observer with no
        /// command source.
        GetSessionClients = |lua, ()| {
            let net: LuaValue = lua.globals().get::<LuaFunction>("__net_session_clients")?.call(())?;
            if let LuaValue::Table(net) = net {
                return Ok(List::<Shaped<SessionClientData>>::new(net));
            }
            let client = |name: LuaValue, uid: LuaValue, source: Option<usize>| -> LuaResult<LuaTable> {
                let t = lua.create_table()?;
                t.raw_set("name", name)?;
                t.raw_set("uid", uid)?;
                t.raw_set("connected", true)?;
                t.raw_set("local", true)?;
                t.raw_set("ejectedBy", lua.create_table()?)?;
                t.raw_set("authorizedCommandSources", lua.create_sequence_from(source)?)?;
                Ok(t)
            };
            let out = lua.create_table()?;
            for (i, (index, army)) in humans(lua)?.into_iter().enumerate() {
                let uid = lua.coerce_string(index)?.map_or(LuaNil, LuaValue::String);
                out.raw_push(client(army.get("nickname")?, uid, Some(i + 1))?)?;
            }
            if out.raw_len() == 0 {
                out.raw_push(client("Observer".into_lua(lua)?, "0".into_lua(lua)?, None)?)?;
            }
            Ok(List::new(out))
        })?;
    lua_fn!(lua, global,
        /// Command sources (1-based): each human army's player in army
        /// order, then observers (the sim's numbering, `command_source.rs`).
        SessionGetCommandSourceNames = |lua, ()| {
            let names = lua.create_table()?;
            for (_, army) in humans(lua)? {
                names.raw_push(army.get::<LuaValue>("nickname")?)?;
            }
            Ok(List::<String>::new(names))
        })?;
    lua_fn!(lua, global,
        /// This player's command source: their place among the human
        /// armies, or the one after them all for an observer.
        SessionGetLocalCommandSource = |lua, ()| {
            let focus = focus_army(lua)? as f64;
            let humans = humans(lua)?;
            let at = humans.iter().position(|(index, _)| matches!(index, LuaValue::Integer(i) if *i as f64 == focus) || matches!(index, LuaValue::Number(n) if *n == focus));
            Ok((at.unwrap_or(humans.len()) + 1) as f64)
        })?;
    Ok(())
}

/// Fills FAF's score data (`hotstats.scoreData`, which its score screen
/// waits for) with every playing army at zero, in the shape of FAF's sim
/// score table, unless the sim's has already arrived.
pub fn fill_score_data(lua: &Lua) -> LuaResult<()> {
    let import: LuaFunction = lua.globals().get("import")?;
    let score_data: LuaTable = import.call::<LuaTable>("/lua/ui/dialogs/hotstats.lua")?.get("scoreData")?;
    if ["interval", "current", "history"].iter().all(|k| score_data.get::<LuaValue>(*k).is_ok_and(|v| truthy(&v))) {
        return Ok(());
    }
    let group = |keys: &[&str], of: &[&str]| -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        for key in keys {
            t.raw_set(*key, zeros(lua, of)?)?;
        }
        Ok(t)
    };
    let current = lua.create_table()?;
    for pair in armies(lua)?.pairs::<LuaValue, LuaTable>() {
        let (index, army) = pair?;
        if truthy(&army.get::<LuaValue>("civilian")?) {
            continue;
        }
        let general = group(&["kills", "built", "lost"], &["count", "mass", "energy"])?;
        for key in ["score", "currentunits", "currentcap"] {
            general.raw_set(key, 0)?;
        }
        let resources = lua.create_table()?;
        resources.raw_set("massin", zeros(lua, &["total", "rate", "reclaimed", "reclaimRate"])?)?;
        resources.raw_set("massout", zeros(lua, &["total", "rate", "excess"])?)?;
        resources.raw_set("energyin", zeros(lua, &["total", "rate", "reclaimed", "reclaimRate"])?)?;
        resources.raw_set("energyout", zeros(lua, &["total", "rate", "excess"])?)?;
        resources.raw_set("storage", zeros(lua, &["storedMass", "storedEnergy", "maxMass", "maxEnergy"])?)?;
        let score = lua.create_table()?;
        score.raw_set("name", army.get::<LuaValue>("nickname")?)?;
        score.raw_set("general", general)?;
        score.raw_set("units", group(&["cdr", "land", "naval", "air", "experimental", "structures"], &["kills", "built", "lost"])?)?;
        score.raw_set("resources", resources)?;
        current.raw_set(index, score)?;
    }
    score_data.set("interval", 30)?;
    score_data.set("current", current)?;
    score_data.set("history", lua.create_table()?)
}
