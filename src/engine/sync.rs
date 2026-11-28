//! The sim's `Sync` table carried to the user layer: each beat the sim's
//! scripts fill `Sync` (`/lua/SimSync.lua`: unit data, scores, alerts, ...),
//! and moho hands a copy to the user layer's `OnSync` before
//! `ResetSyncTable` clears it. The copy crosses threads as a plain value
//! tree ([`SyncValue`]); engine objects in it (units) become their entity ids.
//! Windowed runs only: headless runs never copy it.

use mlua::prelude::*;

/// A Lua value without functions, threads or userdata.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SyncValue {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// Key / value pairs (traversal order: the user layer only reads it).
    Table(Vec<(SyncValue, SyncValue)>),
}

/// Deepest table nesting copied (Sync tables are shallow; this stops cycles).
const MAX_DEPTH: u32 = 16;

impl SyncValue {
    pub fn from_lua(v: &LuaValue, depth: u32) -> SyncValue {
        match v {
            LuaValue::Boolean(b) => SyncValue::Bool(*b),
            LuaValue::Integer(i) => SyncValue::Num(*i as f64),
            LuaValue::Number(n) => SyncValue::Num(*n),
            LuaValue::String(s) => SyncValue::Str(s.to_string_lossy()),
            LuaValue::Table(t) if depth < MAX_DEPTH => {
                // An engine object (unit, prop): its entity id.
                if let Ok(LuaValue::UserData(ud)) = t.raw_get::<LuaValue>(crate::engine::api::script_object::C_OBJECT)
                    && let Ok(h) = ud.borrow::<crate::engine::api::entity::EntityHandle>() {
                    return SyncValue::Num(h.0 as f64);
                }
                SyncValue::Table(t.pairs::<LuaValue, LuaValue>().filter_map(|p| p.ok()).map(|(k, v)| (SyncValue::from_lua(&k, depth + 1), SyncValue::from_lua(&v, depth + 1))).filter(|(k, _)| *k != SyncValue::Nil).collect())
            }
            _ => SyncValue::Nil,
        }
    }

    pub fn to_lua(&self, lua: &Lua) -> LuaResult<LuaValue> {
        Ok(match self {
            SyncValue::Nil => LuaValue::Nil,
            SyncValue::Bool(b) => LuaValue::Boolean(*b),
            SyncValue::Num(n) => LuaValue::Number(*n),
            SyncValue::Str(s) => LuaValue::String(lua.create_string(s)?),
            SyncValue::Table(pairs) => {
                let t = lua.create_table_with_capacity(0, pairs.len())?;
                for (k, v) in pairs {
                    t.raw_set(k.to_lua(lua)?, v.to_lua(lua)?)?;
                }
                LuaValue::Table(t)
            }
        })
    }

    /// Whether this beat's `Sync` sets `Sync.CampaignMode`.
    pub fn sets_campaign_mode(&self) -> bool {
        let SyncValue::Table(pairs) = self else { return false };
        pairs.iter().any(|(k, v)| matches!(k, SyncValue::Str(k) if k == "CampaignMode") && !matches!(v, SyncValue::Nil | SyncValue::Bool(false)))
    }

    pub fn is_empty_table(&self) -> bool {
        matches!(self, SyncValue::Table(p) if p.is_empty())
    }
}

/// Sync copies waiting for the next delivered snapshot (app data, windowed
/// runs only).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct PendingSync(pub Vec<SyncValue>);

/// End of beat, before `ResetSyncTable`: copies `Sync` for the user layer
/// if a renderer is attached.
pub fn capture(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<crate::engine::snapshot::SnapshotSink>().is_none() {
        return Ok(());
    }
    let sync: LuaValue = lua.globals().raw_get("Sync")?;
    let value = SyncValue::from_lua(&sync, 0);
    if value.is_empty_table() || value == SyncValue::Nil {
        return Ok(());
    }
    if lua.app_data_ref::<PendingSync>().is_none() {
        lua.set_app_data(PendingSync::default());
    }
    if let Some(mut p) = lua.app_data_mut::<PendingSync>() {
        // A renderer far behind: keep the latest few hundred beats.
        if p.0.len() >= 600 {
            p.0.remove(0);
        }
        p.0.push(value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_without_functions() {
        let lua = Lua::new();
        let t: LuaTable = lua.load("return { a = 1, b = 'x', c = { true, 2.5 }, f = function() end, [3] = 'three' }").eval().unwrap();
        let v = SyncValue::from_lua(&LuaValue::Table(t), 0);
        let back: LuaTable = match v.to_lua(&lua).unwrap() {
            LuaValue::Table(t) => t,
            _ => panic!(),
        };
        lua.globals().set("T", back).unwrap();
        let ok: bool = lua.load("return T.a == 1 and T.b == 'x' and T.c[1] == true and T.c[2] == 2.5 and T.f == nil and T[3] == 'three'").eval().unwrap();
        assert!(ok);
        // Cycles stop at the depth limit.
        let c: LuaTable = lua.load("local t = {} t.self = t return t").eval().unwrap();
        let _ = SyncValue::from_lua(&LuaValue::Table(c), 0);
    }
}
