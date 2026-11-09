//! Armor: per-armor-type damage multipliers (`Unit:GetArmorMult`), applied by
//! the engine to every `Damage` / `DamageArea` / `DamageRing` hit on a unit
//! before its `OnDamage` runs (FAF's `shield.lua`: "like armor damage, first
//! multiply by armor reduction... see SimDamage.cpp DealDamage").
//!
//! The table is FAF's `/lua/armordefinition.lua` (`armordefinition`: a list
//! of `{ armorType, 'DamageType mult', ... }`), read once on first use. A
//! unit's armor type is its blueprint `Defense.ArmorType`, matched without
//! regard to case, the first definition of a name winning (moho's
//! `ARMOR_GetArmorDefinations`, MohoEngine.dll 0x10152f40: `_stricmp` at
//! 0x101530d6, stopping at the first match; `XAC2101`'s `'STRUCTURE'` gets
//! `Structure`). Damage types are matched with case (a `std::map`,
//! 0x102893e8). Damage types an armor doesn't list, and units without a
//! known armor type, take full damage, with no `Default` fallback
//! (`Unit::ProcessArmorOnDamage` 0x102893a0 leaves the amount unchanged;
//! `GetArmorMult` gives 1, 0x102894c8).

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::entity::{entity_id, world};
use crate::engine::api::moho;
use crate::engine::sim::entity::EntityId;
use crate::lua_api::{Obj, lua_method};

/// armor type (lowercase) -> damage type -> multiplier.
#[derive(Default)]
pub struct ArmorTable {
    loaded: bool,
    types: BTreeMap<String, BTreeMap<String, f64>>,
}

fn ensure_loaded(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<ArmorTable>().is_some_and(|t| t.loaded) {
        return Ok(());
    }
    let mut defs: Option<LuaTable> = lua.globals().raw_get("armordefinition")?;
    if defs.is_none() {
        if let Some(doscript) = lua.globals().raw_get::<Option<LuaFunction>>("doscript")?
            && let Err(e) = doscript.call::<()>("/lua/armordefinition.lua") {
            tracing::warn!(error = %e, "armordefinition.lua failed to load; all armor takes full damage");
        }
        defs = lua.globals().raw_get("armordefinition")?;
    }
    let mut types = BTreeMap::new();
    if let Some(defs) = defs {
        for entry in defs.sequence_values::<LuaValue>() {
            let LuaValue::Table(entry) = entry? else { continue };
            let mut rows = entry.sequence_values::<String>();
            let Some(name) = rows.next().transpose()? else { continue };
            let mut mults = BTreeMap::new();
            for row in rows {
                let row = row?;
                let mut parts = row.split_whitespace();
                if let (Some(kind), Some(Ok(m))) = (parts.next(), parts.next().map(str::parse::<f64>)) {
                    mults.insert(kind.to_string(), m);
                }
            }
            types.entry(name.to_ascii_lowercase()).or_insert(mults);
        }
    }
    lua.set_app_data(ArmorTable { loaded: true, types });
    Ok(())
}

/// Multiplier `unit` takes from `damage_type` (1 when unknown).
pub fn mult(lua: &Lua, unit: EntityId, damage_type: &str) -> LuaResult<f64> {
    ensure_loaded(lua)?;
    let armor = world(lua).get(unit).and_then(|e| e.unit.as_deref()).and_then(|u| u.armor_type.clone());
    let Some(armor) = armor else { return Ok(1.0) };
    let table = lua.app_data_ref::<ArmorTable>().expect("ArmorTable");
    // `armor_type` is kept lowercase (`api::unit`), as the table's names.
    Ok(table.types.get(&*armor).and_then(|t| t.get(damage_type)).copied().unwrap_or(1.0))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_method!(lua, moho::Unit, GetArmorMult = |lua, (this, kind): (Obj<moho::Unit>, Option<String>)| {
        mult(lua, entity_id(&this)?, kind.as_deref().unwrap_or("Normal"))
    })?;
    Ok(())
}
