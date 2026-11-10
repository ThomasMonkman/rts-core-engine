//! Declared Lua signatures for registrations without types: the stubs
//! (`pending.rs`, class stubs) and functions taking `LuaMultiValue`. A stub
//! accepts anything at run time, so its signature can't come from its
//! closure; these give the generated definitions (`--dump-api`) real
//! parameter types instead of `...: any`. Nothing here runs.
//!
//! Drafted from FAF's `/engine` annotations (community documentation, not
//! ground truth): for a stub, what scripts are expected to pass. For an
//! implemented function only the parameters are used (its typed returns are
//! its closure's). When you implement a stub, its closure's types replace
//! the signature and `tests/api_types.rs` asks you to delete it here.
//! Globals both VMs have (FAF's `Core.lua`) are here; the user VM's own are
//! in `src/ui/signatures.rs`.

#![allow(unused_imports)]

use mlua::prelude::*;

use crate::engine::api::categories::EntityCategory;
use crate::engine::api::moho;
use crate::engine::sim::layer::Layer;
use crate::engine::api::sound::SoundHandle;
use crate::lua_api::shapes::{Army, Blueprint, Bone, Color, Quaternion, Rect, Vector, Vector2};
use crate::lua_api::{Obj, signatures};

signatures! {
    global {
        BeginLoggingStats(filename: String);
        CoordinateAttacks();
        CreateUnit2(blueprint_id: String, army: Army, layer: Layer, x: f64, z: f64, heading: f64) -> Obj<moho::Unit>;
        DrawCircle(center: Vector, diameter: f64, color: Color);
        DrawLine(point_a: Vector, point_b: Vector, color: Color);
        DrawLinePop(start: Vector, finish: Vector, color: Color);
        EndGame();
        EndLoggingStats(exit: Option<bool>);
        EntityCategoryCountAroundPosition(category: EntityCategory, position: Vector) -> f64;
        EntityCategoryEmpty(category: EntityCategory) -> bool;
        GenerateRandomOrientation() -> Quaternion;
        GetBlueprint(object: LuaValue) -> Blueprint;
        GetEntitiesInRect(rectangle: Rect) -> Option<Vec<Obj<moho::Entity>>>;
        GetReclaimablesInRect(rectangle: Rect) -> Option<LuaTable>;
        GetReclaimablesInRect(x0: f64, z0: f64, x1: f64, z1: f64) -> Option<LuaTable>;
        GetUnitsInRect(rectangle: Rect) -> Option<Vec<Obj<moho::Unit>>>;
        GetUnitsInRect(x0: f64, z0: f64, x1: f64, z1: f64) -> Option<Vec<Obj<moho::Unit>>>;
        LUnitMove(unit: Obj<moho::Unit>, target: Vector);
        LUnitMoveNear(unit: Obj<moho::Unit>, target: Vector, range: f64);
        MetaImpact(instigator: Obj<moho::Unit>, location: Vector, max_radius: f64, amount: f64, affects_category: Option<EntityCategory>, damage_friendly: Option<bool>);
        PlayLoop(manager: LuaValue, sound: SoundHandle) -> SoundHandle;
        PointVector(px: f64, py: f64, pz: f64, vx: f64, vy: f64, vz: f64) -> (Vector, Vector);
        RPCSound(sound: LuaTable);
        LOG(out: LuaValue, rest: mlua::Variadic<LuaValue>);
        SPEW(out: LuaValue, rest: mlua::Variadic<LuaValue>);
        SelectedUnit() -> Obj<moho::Unit>;
        SetAlliedVictory(state: bool);
        SetArmyStatsSyncArmy(army: Army);
        SetTerrainType(x: f64, z: f64, type_: LuaTable);
        SetTerrainTypeRect(rect: Rect, type_: LuaTable);
        SimConExecute(command: String);
        StopLoop(manager: LuaValue, handle: SoundHandle);
        WaitFor(object: LuaValue);
        SubmitXMLArmyStats();
        Trace(enable: bool);
        // The Luau shims (`src/engine/lua/shims/`): implemented, typed here.
        /// `a + b`, componentwise.
        VAdd(a: Vector, b: Vector) -> Vector;
        /// `a - b`, componentwise.
        VDiff(a: Vector, b: Vector) -> Vector;
        /// The distance between (x1, y1) and (x2, y2).
        VDist2(x1: f64, y1: f64, x2: f64, y2: f64) -> f64;
        /// The squared distance between (x1, y1) and (x2, y2).
        VDist2Sq(x1: f64, y1: f64, x2: f64, y2: f64) -> f64;
        /// The distance between `a` and `b`.
        VDist3(a: Vector, b: Vector) -> f64;
        /// The squared distance between `a` and `b`.
        VDist3Sq(a: Vector, b: Vector) -> f64;
        /// The dot product of `a` and `b`.
        VDot(a: Vector, b: Vector) -> f64;
        /// `v * s`, componentwise.
        VMult(v: Vector, s: f64) -> Vector;
        /// The 2D cross product of `a` and `b` on the XZ plane: `a.z * b.x - a.x * b.z`.
        VPerpDot(a: Vector, b: Vector) -> f64;
        Basename(full_path: String, strip_extension: Option<bool>) -> String;
        Rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect;
        STR_GetTokens(str: String, delimiter: String) -> Vec<String>;
        STR_Utf8Len(str: String) -> i64;
        STR_Utf8SubString(str: String, start: i64, count: i64) -> String;
        STR_itox(int: i64) -> String;
        STR_xtoi(str: String) -> i64;
        WARN(out: LuaValue, rest: mlua::Variadic<LuaValue>);
    }
    // `FindClosestArmyWithBase`..`SetGreaterOf` are stubs, from moho's
    // bindings (MohoEngine.dll 0x101781d0..0x1017fc70; the two
    // `GetNumPlatoons*` from SupremeCommander.exe 0x593970, 0x5937e0).
    moho::AIBrain {
        FindClosestArmyWithBase(alliance: String) -> Option<Obj<moho::AIBrain>>;
        FindUnit(category: EntityCategory, need_to_be_idle: bool) -> Option<Obj<moho::Unit>>;
        /// A unit and its upgrade blueprint.
        FindUnitToUpgrade(upgrade_list: LuaTable) -> (LuaValue, LuaValue);
        /// The plan's conversion isn't identified (passed on as is).
        SetCurrentPlan(plan: LuaValue);
        SetGreaterOf(stat_name: String, value: f64);
        /// `category` and `compare_type` may come in either order; the 8-argument
        /// form adds a filter function, `module`'s export `filter`.
        PickBestAttackVector(platoon: Obj<moho::Platoon>, squad: String, alliance: String, category: EntityCategory, compare_type: String) -> Vector;
        PickBestAttackVector(platoon: Obj<moho::Platoon>, squad: String, alliance: String, category: EntityCategory, compare_type: String, module: String, filter: String) -> Vector;
    }
    // Stubs, from moho's bindings (MohoEngine.dll 0x101c6ad0..0x101c8420):
    // the argument-count check and each argument's conversion; a `target` is
    // read by moho's AI-target reader (0x101cfbf0: an entity or a position)
    // and given back by its writer (0x101cfab0). Docs are moho's help
    // strings. No script FAF, the original game or the AI mods ship calls
    // them; the attacker is the engine's own unit-combat logic.
    moho::CAiAttackerImpl {
        /// Whether the attacker has any weapon currently attacking an enemy.
        AttackerWeaponsBusy() -> bool;
        /// Loops through the weapons to see if the target can be attacked.
        CanAttackTarget(target: LuaValue) -> bool;
        /// Finds the best enemy target for a weapon.
        FindBestEnemy(range: f64) -> LuaValue;
        /// Forces it to engage the enemy target. Moho checks for exactly one
        /// argument, then reads a unit from argument 2 anyway.
        ForceEngage();
        /// The desired target.
        GetDesiredTarget() -> LuaValue;
        /// The longest range of a weapon that isn't manual fire.
        GetMaxWeaponRange() -> f64;
        /// The primary weapon's index.
        GetPrimaryWeapon() -> f64;
        /// The index of a weapon that can attack `target`.
        GetTargetWeapon(target: LuaValue) -> f64;
        /// The unit this attacker is bound to.
        GetUnit() -> Obj<moho::Unit>;
        /// The count of weapons.
        GetWeaponCount() -> f64;
        /// The target of a slaved weapon that has one.
        HasSlavedTarget() -> LuaValue;
        /// Whether the entity is exempt from being attacked.
        IsTargetExempt(entity: Obj<moho::Entity>) -> bool;
        /// Whether the target is too close to the weapons.
        IsTooClose(target: LuaValue) -> bool;
        /// Whether the target is within any weapon's range (from `position`,
        /// when given).
        IsWithinAttackRange(target: LuaValue) -> bool;
        IsWithinAttackRange(position: Vector, target: LuaValue) -> bool;
        /// Resets the reporting state.
        ResetReportingState();
        /// Sets the desired target.
        SetDesiredTarget(target: LuaValue);
        /// Stops the attacker.
        Stop();
    }
    // Stubs, from moho's bindings (MohoEngine.dll 0x102fe9c0..0x103045c0):
    // the argument-count check and each argument's conversion (string, enum
    // through `SCR_GetEnum`, boolean, category, `Vector3`, unit). FA's exe
    // patches change none of them. FAF's annotations give
    // `CanConsiderFormingPlatoon`, `FindClosestUnitToBase`,
    // `FindHighestValueUnit` and `SwitchAIPlan` no parameters. The order
    // methods return a table (of their commands). `GetPlatoonLifetimeStats`
    // is in FA's exe only, not traced, so undeclared.
    // Stubs, from moho's bindings (MohoEngine.dll 0x10195800..0x101964e0).
    moho::Navigator {
        AtGoal() -> bool;
        BroadcastResumeTaskEvent();
        /// Moho checks for exactly one argument, then reads a vector from
        /// argument 2 anyway.
        CanPathToGoal() -> bool;
        FollowingLeader() -> bool;
        GetStatus() -> f64;
        HasGoodPath() -> bool;
        IgnoreFormation(ignore: bool);
        IsIgnorningFormation() -> bool;
    }
    // Stubs, from moho's bindings (MohoEngine.dll 0x102b4390..0x102b66d0).
    moho::Weapon {
        ChangeDamageRadius(value: f64);
        ChangeDamageType(type_name: String);
        DoInstaHit(bone: Bone, r: f64, g: f64, b: f64, glow: f64, width: f64, texture: String, lifetime: f64);
        /// Transfers the target from one weapon to another; `other`'s reader
        /// (0x102b1d90) isn't identified.
        TransferTarget(other: LuaValue);
    }
    // Stubs, from moho's bindings (MohoEngine.dll 0x1021c050..0x102d72f0).
    moho::AimManipulator {
        OnTarget() -> bool;
        SetAimHeadingOffset(offset: f64);
    }
    moho::AnimationManipulator {
        SetDisableOnSignal(disable: bool);
        SetOverwriteMode(overwrite: bool);
    }
    moho::BoneEntityManipulator {
        /// The pivot point of the attached bone.
        SetPivot(x: f64, y: f64, z: f64);
    }
    moho::SlaveManipulator {
        SetMaxRate(degrees_per_second: f64);
    }
    moho::Platoon {
        /// `template` isn't converted where it's read (passed on as is).
        CanConsiderFormingPlatoon(template: LuaValue, name: String) -> bool;
        Destroy(squad: Option<String>);
        DisbandOnIdle();
        FerryToLocation(position: Vector) -> LuaTable;
        FindClosestUnitToBase(squad: String, alliance: String, can_attack: bool, category: EntityCategory) -> Option<Obj<moho::Unit>>;
        FindFurthestUnit(squad: String, alliance: String, can_attack: bool, category: EntityCategory) -> Option<Obj<moho::Unit>>;
        FindHighestValueUnit(squad: String, alliance: String, can_attack: bool, category: EntityCategory) -> Option<Obj<moho::Unit>>;
        GetAIPlan() -> LuaValue;
        GetFactionIndex() -> f64;
        GetFerryBeacons() -> LuaTable;
        GetPersonality() -> LuaValue;
        LoadUnits(category: EntityCategory) -> LuaTable;
        SwitchAIPlan(plan: LuaValue);
        UnloadAllAtLocation(position: Vector) -> LuaTable;
        UnloadUnitsAtLocation(category: EntityCategory, position: Vector) -> LuaTable;
        UseTeleporter(teleporter: Obj<moho::Unit>, squad: Option<String>) -> LuaTable;
    }
    moho::CollisionBeamEntity {
        SetBeamFx(beam_emitter: Obj<moho::IEffect>, check_collision: bool);
    }
    moho::Entity {
        SinkAway(dy: f64);
        // Stubs, from moho's bindings (MohoEngine.dll 0x10272210..0x102d6ec0).
        /// `shooter`'s reader (0x101b12e0) isn't identified.
        AddShooter(shooter: LuaValue);
        AddWorldImpulse(ix: f64, iy: f64, iz: f64, px: f64, py: f64, pz: f64);
        /// The current draw scale.
        GetScale() -> (f64, f64, f64);
        PushOver(nx: f64, ny: f64, nz: f64, depth: f64);
        ReachedMaxShooters() -> bool;
        /// `shooter`'s reader (0x10260240) isn't identified.
        RemoveShooter(shooter: LuaValue);
    }
    // Stubs, from moho's bindings: MohoEngine.dll 0x102a2580..0x102ab8d0,
    // and for `CanPathToRect`, `GetCurrentMoveLocation`, `PrintCommandQueue`
    // (FA's exe only) SupremeCommander.exe 0x6d27f0, 0x6d5560, 0x6d4f20.
    // FA's exe patches change none of them.
    moho::Unit {
        AlterArmor(damage_type: String, multiplier: f64);
        /// Both corners read by the exe's vector reader (0x4d6ba0).
        CanPathToRect(a: Vector, b: Vector) -> bool;
        EnableManipulators(bone: Bone, enable: bool);
        /// The unit's tactical attack manager (see `CAiAttackerImpl`).
        GetAttacker() -> Obj<moho::CAiAttackerImpl>;
        GetCurrentMoveLocation() -> Vector;
        /// The target's footprint is read, so it's an entity.
        HasMeleeSpaceAroundTarget(target: Obj<moho::Entity>) -> bool;
        HasValidTeleportDest() -> bool;
        IsValidTarget() -> bool;
        KillManipulator(manipulator: Obj<moho::Manipulator>);
        KillManipulators(bone: Bone);
        MeleeWarpAdjacentToTarget(target: Obj<moho::Entity>);
        /// Logs the unit's command queue ("*AI DEBUG: Command List for ...").
        PrintCommandQueue();
        RecoilImpulse(x: f64, y: f64, z: f64);
        ScaleGetBuiltEmitter(emitter: Obj<moho::IEffect>);
        SetIsValidTarget(valid: bool);
    }
    moho::Projectile {
        SetDamage(amount: Option<f64>, radius: Option<f64>);
        /// Stub, from moho's binding (MohoEngine.dll 0x10283de0).
        SetVelocityRandomUpVector();
    }
    moho::RotateManipulator {
        ClearFollowBone();
        SetFollowBone(bone: Bone);
    }
}
