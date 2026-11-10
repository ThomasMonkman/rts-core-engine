//! `CAiBrain:GetPersonality` and `moho.aipersonality_methods`.
//!
//! Moho's personality tables live in the binary; FAF ships none, and
//! scenario saves set `personality = ''`. Until real data turns up, every
//! brain gets the same neutral personality: emphases/tendencies 0.5 (no
//! preference), sizes 1 (no scaling), `AdjustDelay` returns the delay
//! unchanged. All values are inferred placeholders, not moho's.

use crate::lua_api::shapes;
use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::army::{resolve_army, ArmyHandle};
use crate::engine::api::moho::{self, MohoClass};
use crate::engine::api::script_object;
use crate::lua_api::{Obj, Status, lua_fn, lua_method};

/// `_c_object` of a personality: its army.
#[derive(Clone, Copy, Debug)]
pub struct PersonalityHandle(pub u32);
impl LuaUserData for PersonalityHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

/// Personality names set by `SetArmyAIPersonality`, by army.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct PersonalityNames(BTreeMap<u32, String>);

const OBJECTS_KEY: &str = "engine.personalities";

/// Neutral 0..1 tendencies (emphases, frequencies, "driven" values).
const NEUTRAL: &[&str] = &[
    "GetAirUnitsEmphasis", "GetTankUnitsEmphasis", "GetBotUnitsEmphasis", "GetSeaUnitsEmphasis",
    "GetSpecialtyForcesEmphasis", "GetSupportUnitsEmphasis", "GetDirectDamageEmphasis",
    "GetInDirectDamageEmphasis", "GetSurvivalEmphasis", "GetAttackFrequency",
    "GetRepeatAttackFrequency", "GetCounterForces", "GetIntelGathering", "GetCoordinatedAttacks",
    "GetExpansionDriven", "GetTechAdvancement", "GetUpgradesDriven", "GetDefenseDriven",
    "GetEconomyDriven", "GetFactoryTycoon", "GetIntelBuildingTycoon", "GetSuperWeaponTendency",
    "GetTeamSupport", "GetFormationUse", "GetTargetSpread", "GetQuittingTendency", "GetChatFrequency",
];

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(PersonalityNames::default());
    lua_fn!(lua, global,
        /// Records the AI personality name chosen for `army` (read back by
        /// `personality:GetPersonalityName`).
        SetArmyAIPersonality = |lua, (army, name): (shapes::Army, String)| {
            let army = resolve_army(lua, &army)?;
            lua.app_data_mut::<PersonalityNames>().expect("PersonalityNames").0.insert(army, name);
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// One personality object per brain, created on first request.
        GetPersonality = |lua, brain: Obj<moho::AIBrain>| {
            let ArmyHandle(army) = script_object::handle(&brain)?;
            let cache = match lua.named_registry_value::<Option<LuaTable>>(OBJECTS_KEY)? {
                Some(t) => t,
                None => {
                    let t = lua.create_table()?;
                    lua.set_named_registry_value(OBJECTS_KEY, t.clone())?;
                    t
                }
            };
            if let Some(obj) = cache.raw_get::<Option<LuaTable>>(army)? {
                return Ok(Obj::<moho::AIPersonality>::new(obj));
            }
            let obj = script_object::create(lua, PersonalityHandle(army), moho::class_table(lua, "aipersonality_methods")?)?;
            cache.raw_set(army, obj.clone())?;
            Ok(Obj::new(obj))
        })?;
    for &name in NEUTRAL {
        crate::lua_api::add_class_fn(
            lua,
            moho::AIPersonality::CLASS,
            name,
            None,
            "Placeholder tendency: 0.5 (no preference) for every brain.",
            Status::Implemented,
            |_, _this: Obj<moho::AIPersonality>| Ok(0.5),
        )?;
    }
    for name in ["GetArmySize", "GetPlatoonSize"] {
        crate::lua_api::add_class_fn(
            lua,
            moho::AIPersonality::CLASS,
            name,
            None,
            "Placeholder size multiplier: 1 (no scaling) for every brain.",
            Status::Implemented,
            |_, _this: Obj<moho::AIPersonality>| Ok(1.0),
        )?;
    }
    lua_method!(lua, moho::AIPersonality,
        /// Placeholder: 2 for every brain.
        GetDifficulty = |_, _this: Obj<moho::AIPersonality>| Ok(2.0))?;
    lua_method!(lua, moho::AIPersonality,
        /// Returns `delay` unchanged (placeholder).
        AdjustDelay = |_, (_this, delay, _factor): (Obj<moho::AIPersonality>, f64, Option<f64>)| Ok(delay))?;
    for name in ["GetFavouriteStructures", "GetFavouriteUnits"] {
        crate::lua_api::add_class_fn(
            lua,
            moho::AIPersonality::CLASS,
            name,
            None,
            "Placeholder: an empty table.",
            Status::Implemented,
            |lua, _this: Obj<moho::AIPersonality>| lua.create_table().map(shapes::List::<String>::new),
        )?;
    }
    lua_method!(lua, moho::AIPersonality,
        /// Placeholder: the empty string.
        GetChatPersonality = |_, _this: Obj<moho::AIPersonality>| Ok(String::new()))?;
    lua_method!(lua, moho::AIPersonality,
        /// The name set by `SetArmyAIPersonality` for this brain's army, or the empty string.
        GetPersonalityName = |lua, this: Obj<moho::AIPersonality>| {
            let PersonalityHandle(army) = script_object::handle(&this)?;
            Ok(lua.app_data_ref::<PersonalityNames>().and_then(|n| n.0.get(&army).cloned()).unwrap_or_default())
        })?;
    Ok(())
}
