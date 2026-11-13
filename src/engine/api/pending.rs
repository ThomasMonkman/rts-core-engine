//! Engine globals (from FAF's `/engine/Sim.lua` + `/engine/Core.lua`
//! declarations) that aren't implemented yet. FAF captures many of them at
//! load time (`local X = X`, `SimHooks.lua` wrappers), so they must exist; each
//! raises a clear error if actually *called*, so nothing silently misbehaves.
//! Implementing one means deleting it from [`CORE`] or [`SIM`] and registering
//! the real function (registration order guarantees the real one wins).

use mlua::prelude::*;

/// `/engine/Core.lua`: both the sim and the user state have these.
pub const CORE: &[&str] = &[
    "AITarget",
    "BeginLoggingStats",
    "EndLoggingStats",
    "EntityCategoryEmpty",
    "GetBlueprint",
    "PointVector",
    "RPCSound",
    "Trace",
];

/// `/engine/Sim.lua`: the sim state only.
pub const SIM: &[&str] = &[
    "CoordinateAttacks",
    "CreateUnit2",
    "EntityCategoryCountAroundPosition",
    "GenerateRandomOrientation",
    "GetEntitiesInRect",
    "LUnitMove",
    "LUnitMoveNear",
    "PlayLoop",
    "SelectedUnit",
    "SetAlliedVictory",
    "SetArmyStatsSyncArmy",
    "SetTerrainType",
    "SetTerrainTypeRect",
    "SimConExecute",
    "StopLoop",
    "SubmitXMLArmyStats",
];

/// Sim methods moho registers that we've reviewed and decided to leave as
/// stubs: no FAF script or AI mod we test calls them (checked
/// against their Lua; where the original game did, the reason says so).
/// Each raises when called, saying so, so a mod that does shows up as new
/// rather than forgotten. Each entry: (class, methods, why).
pub const UNUSED: &[(&str, &[&str], &str)] = &[
    (
        "CAiAttackerImpl_methods",
        &[
            "AttackerWeaponsBusy", "CanAttackTarget", "FindBestEnemy", "ForceEngage", "GetDesiredTarget", "GetMaxWeaponRange",
            "GetPrimaryWeapon", "GetTargetWeapon", "GetUnit", "GetWeaponCount", "HasSlavedTarget", "IsTargetExempt", "IsTooClose",
            "IsWithinAttackRange", "ResetReportingState", "SetDesiredTarget", "Stop",
        ],
        "the engine's own unit-combat logic; scripts target through weapons, platoons and orders",
    ),
    ("unit_methods", &["GetAttacker"], "the only way to an attacker object (CAiAttackerImpl_methods)"),
    (
        "platoon_methods",
        &[
            "CanConsiderFormingPlatoon", "Destroy", "DisbandOnIdle", "FerryToLocation", "FindClosestUnitToBase", "FindFurthestUnit",
            "FindHighestValueUnit", "GetAIPlan", "GetFactionIndex", "GetFerryBeacons", "GetPersonality", "GetPlatoonLifetimeStats",
            "SwitchAIPlan", "UnloadUnitsAtLocation", "UseTeleporter",
        ],
        "scripts disband through the brain and ask the brain for its faction and personality",
    ),
    (
        "unit_methods",
        &[
            "AlterArmor", "CanPathToRect", "EnableManipulators", "GetCurrentMoveLocation",
            "HasMeleeSpaceAroundTarget", "HasValidTeleportDest", "IsValidTarget", "KillManipulator", "KillManipulators",
            "MeleeWarpAdjacentToTarget", "PrintCommandQueue", "RecoilImpulse", "ScaleGetBuiltEmitter", "SetIsValidTarget",
        ],
        "FAF's unit scripts manage armour, manipulators and targeting themselves; `RecoilImpulse` was only the original game's ship rocking (`ShipRock`) and `PrintCommandQueue` a commented-out debug line",
    ),
    (
        "navigator_methods",
        &["AtGoal", "BroadcastResumeTaskEvent", "CanPathToGoal", "FollowingLeader", "GetStatus", "HasGoodPath", "IgnoreFormation", "IsIgnorningFormation"],
        "scripts steer through orders and SetGoal; the `GetStatus` calls in FAF and the original game are on build conditions",
    ),
    (
        "aibrain_methods",
        &["FindClosestArmyWithBase", "FindUnit", "FindUnitToUpgrade", "SetCurrentPlan", "SetGreaterOf"],
        "every brain class defines its own `SetCurrentPlan` in Lua, and `FindUnit`'s only caller (`AIGetBuilder`) is dead code, in FAF and the original game",
    ),
    (
        "entity_methods",
        &["AddShooter", "AddWorldImpulse", "GetScale", "PushOver", "ReachedMaxShooters", "RemoveShooter"],
        "no script in FAF, the original game or the AI mods calls them",
    ),
    (
        "weapon_methods",
        &["ChangeDamageRadius", "ChangeDamageType", "DoInstaHit", "TransferTarget"],
        "no script in FAF, the original game or the AI mods calls them",
    ),
    ("AimManipulator", &["OnTarget", "SetAimHeadingOffset"], "no script in FAF, the original game or the AI mods calls them"),
    ("AnimationManipulator", &["SetDisableOnSignal", "SetOverwriteMode"], "no script in FAF, the original game or the AI mods calls them"),
    ("BoneEntityManipulator", &["SetPivot"], "no script in FAF, the original game or the AI mods calls it"),
    ("SlaveManipulator", &["SetMaxRate"], "no script in FAF, the original game or the AI mods calls it"),
    ("projectile_methods", &["SetVelocityRandomUpVector"], "no script in FAF, the original game or the AI mods calls it"),
];

/// Sim methods moho registers (MohoEngine.dll, or FA's exe where the DLL
/// lacks them: FA-Binary-Patches `include/LuaCFuncs.h`) that no FAF script
/// reaches, not reviewed yet: each moves to [`UNUSED`] or gets
/// implemented. Each raises when called. Implementing one means deleting it
/// here (a real registration replaces the stub either way). The user VM
/// stubs its methods from FAF's annotations at run time
/// (`ui::user_api::register_method_stubs`).
pub const TO_REVIEW: &[(&str, &[&str])] = &[
    // The campaign's transport platoons (ScenarioPlatoonAI); need platoon
    // transport orders.
    ("platoon_methods", &["LoadUnits", "UnloadAllAtLocation"]),
];

/// Registers [`UNUSED`]'s and [`TO_REVIEW`]'s stubs where the class has no
/// such method yet. Run after the real methods are registered.
pub fn register_methods(lua: &Lua) -> LuaResult<()> {
    let unused = UNUSED.iter().map(|&(class, names, _)| (class, names, true));
    let to_review = TO_REVIEW.iter().map(|&(class, names)| (class, names, false));
    for (class, names, reviewed) in unused.chain(to_review) {
        let table = crate::engine::api::moho::class_table(lua, class)?;
        for &name in names {
            if table.raw_get::<LuaValue>(name)? != LuaNil {
                continue;
            }
            let doc = if reviewed { "Not used by FAF's scripts: left unimplemented (raises when called)." } else { "Raises when called." };
            crate::lua_api::add_class_fn(lua, class, name, None, doc, crate::lua_api::Status::Stub, move |_, _: LuaMultiValue| -> LuaResult<()> {
                Err(LuaError::runtime(if reviewed {
                    format!("engine API '{class}:{name}' is not implemented: no FAF script uses it")
                } else {
                    format!("engine API '{class}:{name}' is not implemented yet")
                }))
            })?;
        }
    }
    Ok(())
}

/// The sim state's stubs: [`CORE`] and [`SIM`].
pub fn register(lua: &Lua) -> LuaResult<()> {
    register_core(lua)?;
    stub(lua, SIM)
}

/// The user state's stubs: [`CORE`] only.
pub fn register_core(lua: &Lua) -> LuaResult<()> {
    stub(lua, CORE)
}

fn stub(lua: &Lua, names: &'static [&'static str]) -> LuaResult<()> {
    let globals = lua.globals();
    for &name in names {
        crate::lua_api::register_fn(
            lua,
            &globals,
            "",
            name,
            None,
            "Raises when called.",
            crate::lua_api::Status::Stub,
            false,
            move |_, _: LuaMultiValue| -> LuaResult<()> {
                Err(LuaError::runtime(format!(
                    "engine API '{name}' is not implemented yet"
                )))
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn calling_pending_api_errors_with_its_name() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        // Any still-pending name (a fixed one breaks once it's implemented).
        let name = super::SIM[0];
        let err = lua.load(format!("{name}(nil, 1)")).exec().unwrap_err();
        assert!(err.to_string().contains(&format!("'{name}' is not implemented")), "{err}");
    }

    #[test]
    fn calling_pending_method_errors_with_its_name() {
        let lua = crate::lua_api::dump::sim_vm().unwrap();
        let (class, names) = super::TO_REVIEW[0];
        let err = lua.load(format!("moho.{class}.{}({{}})", names[0])).exec().unwrap_err();
        assert!(err.to_string().contains(&format!("'{class}:{}' is not implemented yet", names[0])), "{err}");
        let (class, names, _) = super::UNUSED[0];
        let err = lua.load(format!("moho.{class}.{}({{}})", names[0])).exec().unwrap_err();
        assert!(err.to_string().contains("no FAF script uses it"), "{err}");
    }

    #[test]
    fn user_state_gets_core_stubs_only() {
        let lua = mlua::Lua::new();
        super::register_core(&lua).unwrap();
        assert!(lua.globals().get::<mlua::Function>(super::CORE[0]).is_ok());
        assert!(lua.globals().get::<mlua::Value>(super::SIM[0]).unwrap().is_nil());
    }
}
