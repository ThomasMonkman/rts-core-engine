//! Every script callback the sim calls on its objects, in one place: the
//! name (written only here), the arguments and their types. Call sites use
//! the generated functions (`UnitCallbacks::on_stop_being_built(lua, &obj,
//! what, builder, layer)`), so a misspelt callback or a wrong argument is a
//! compile error instead of a silently skipped `run_script`. The blocks are
//! collected for `--dump-api` (`type UnitCallbacks = { ... }` in Luau,
//! `---@class UnitCallbacks` in EmmyLua).
//!
//! `what` names the object in the log when the callback raises (moho's
//! "Error running <name> script in <what>").

use mlua::prelude::*;

use crate::engine::api::moho;
use crate::engine::sim::layer::Layer;
use crate::lua_api::shapes::{DamageType, Instigator, Loose, Quaternion, TerrainType, Vector};
use crate::lua_api::{Obj, callbacks};

callbacks! {
    /// Callbacks on any entity's script object (units, props, projectiles,
    /// shields, ...).
    moho::Entity {
        /// After the script object of a blueprint-less entity is created.
        OnCreate();
        /// The entity is being destroyed.
        OnDestroy();
        /// Killed: `instigator` (or nil), the damage type, the overkill ratio.
        OnKilled(instigator: Instigator, kind: DamageType, overkill: f64);
        /// Damaged by `amount` after armour, only when that is more than 0
        /// (MohoEngine.dll `DealDamage` 0x10308181; `direction` is a unit
        /// vector).
        OnDamage(instigator: Instigator, amount: f64, direction: Vector, kind: DamageType);
        /// Fully reclaimed by `reclaimer`.
        OnReclaimed(reclaimer: Obj<moho::Unit>);
        /// The engine destroying the object through its script (`self:Destroy()`).
        Destroy();
    }

    /// Callbacks on unit script objects.
    moho::Unit {
        OnPreCreate();
        OnCreate();
        /// The unit changed layer (`old` is `"None"` at creation).
        OnLayerChange(new: &str, old: &str);
        /// The terrain type under the unit changed (terrain type tables).
        OnTerrainTypeChange(new: Loose<Option<TerrainType>>, old: Loose<Option<TerrainType>>);
        OnStartBeingBuilt(builder: Obj<moho::Unit>, layer: Layer);
        /// Construction finished (`builder` nil when spawned complete).
        OnStopBeingBuilt(builder: Option<Obj<moho::Unit>>, layer: Layer);
        /// A crashing aircraft hit `with` (`"Terrain"` / `"Water"`).
        OnImpact(with: &str);
        OnMotionHorzEventChange(new: &str, old: &str);
        OnMotionVertEventChange(new: &str, old: &str);
        OnRunOutOfFuel();
        OnGotFuel();
        OnFailedTeleport();
        /// Teleport starts: the beacon (or nil), destination and orientation.
        OnTeleportUnit(teleporter: Loose<Option<Obj<moho::Unit>>>, location: Vector, orientation: Quaternion);
        /// `adjacent` touches this structure; `trigger` is the one just finished.
        OnAdjacentTo(adjacent: Obj<moho::Unit>, trigger: Obj<moho::Unit>);
        OnNotAdjacentTo(adjacent: Obj<moho::Unit>);
        OnFerryPointSet();
        OnStartSacrifice(target: Obj<moho::Unit>);
        OnStopSacrifice(target: Obj<moho::Unit>);
        OnSpecialAction(location: Vector);
        OnStartReclaim(target: Obj<moho::Entity>);
        /// Reclaim ended (`target` nil when used up).
        OnStopReclaim(target: Option<Obj<moho::Entity>>);
        OnAddToStorage(carrier: Obj<moho::Unit>);
        OnRemoveFromStorage(carrier: Obj<moho::Unit>);
        OnTransportAttach(bone: String, unit: Obj<moho::Unit>);
        OnTransportDetach(bone: String, unit: Obj<moho::Unit>);
        OnStartRefueling();
        OnTransportOrdered();
        OnStartTransportLoading();
        OnTransportAborted();
        OnStopTransportLoading();
        OnStartTransportBeamUp(transport: Obj<moho::Unit>, bone: String);
        OnStopTransportBeamUp();
        OnPrepareArmToBuild();
        OnStartBuilderTracking();
        OnStopBuilderTracking();
        OnFailedToBuild();
        /// `order`: `"MobileBuild"`, `"FactoryBuild"`, `"Upgrade"`, `"Repair"`, ...
        OnStartBuild(target: Obj<moho::Unit>, order: &str);
        OnStopBuild(target: Obj<moho::Unit>, order: &str);
        OnFailedToBeBuilt();
        OnBuildProgress(target: Obj<moho::Unit>, old: f64, new: f64);
        OnBeingBuiltProgress(builder: Obj<moho::Unit>, old: f64, new: f64);
        OnDecayed();
        /// A repair task ended.
        ClearWork();
        /// An assister joins `target`'s enhancement.
        InheritWork(target: Obj<moho::Unit>);
        OnSiloBuildStart(weapon: Obj<moho::Weapon>);
        OnSiloBuildEnd(weapon: Obj<moho::Weapon>);
        OnNukeLaunched();
        OnNukeArmed();
        OnProductionActive();
        OnProductionInActive();
        OnConsumptionActive();
        OnConsumptionInActive();
        OnPaused();
        OnUnpaused();
        OnScriptBitSet(bit: u16);
        OnScriptBitClear(bit: u16);
        OnAutoModeOn();
        OnAutoModeOff();
        OnStartRepeatQueue();
        OnStopRepeatQueue();
        OnAssignedFocusEntity();
        /// Health changed by a quarter step (fractions of max health).
        OnHealthChanged(new: f64, old: f64);
        /// A unit attached to this one (cargo) was killed.
        OnAttachedKilled(attached: Obj<moho::Unit>);
        /// First seen by `army`.
        OnDetectedBy(army: u32);
        /// Damaged (more than 0, after armour) by an instigator of army
        /// `army` (1-based; MohoEngine.dll `DealDamage` 0x10307708). FAF's is
        /// empty.
        OnDamageBy(army: u32);
        /// Armour multiplied a hit of `kind` by 2 or more (`DealDamage`
        /// 0x10307761; FAF defines none).
        OnExtraDamageDealt(kind: DamageType);
        OnStartCapture(target: Obj<moho::Unit>);
        OnStopCapture(target: Obj<moho::Unit>);
        OnFailedCapture(target: Obj<moho::Unit>);
        OnStartBeingCaptured(captor: Obj<moho::Unit>);
        OnStopBeingCaptured(captor: Obj<moho::Unit>);
        OnFailedBeingCaptured(captor: Obj<moho::Unit>);
        OnCaptured(captor: Obj<moho::Unit>);
        /// A foot-plant bone touched the ground (local offset).
        OnAnimCollision(bone: String, x: f64, y: f64, z: f64);
        OnAnimTerrainCollision(bone: String, x: f64, y: f64, z: f64);
    }

    /// Callbacks on prop script objects.
    moho::Prop {
        /// A unit walked into a single tree: the push direction and depth.
        OnCollision(other: Obj<moho::Unit>, nx: f64, ny: f64, nz: f64, depth: f64);
    }

    /// Callbacks on projectile script objects.
    moho::Projectile {
        OnCreate(in_water: bool);
        OnEnterWater();
        OnExitWater();
        /// `kind`: `"Terrain"`, `"Water"`, `"Unit"`, `"Shield"`, `"Prop"`, ...
        OnImpact(kind: &str, target: Loose<Option<Obj<moho::Entity>>>);
        OnDestroy();
    }

    /// Callbacks on collision beam script objects.
    moho::CollisionBeamEntity {
        OnCreate();
        OnEnable();
        OnDisable();
        OnImpact(kind: &str, target: Loose<Option<Obj<moho::Entity>>>);
    }

    /// Callbacks on weapon script objects.
    moho::Weapon {
        OnCreate();
        OnDestroy();
        OnFire();
        OnGotTarget();
        OnLostTarget();
        OnHaltFire();
        /// An overcharge order took this weapon (`api::overcharge`).
        OnEnableWeapon();
        /// An overcharge order ended without firing this weapon.
        OnDisableWeapon();
        /// The aim bone's label (`FireControl` or the first aim bone).
        OnStartTracking(label: String);
        OnStopTracking(label: String);
    }

    /// Callbacks on platoon script objects.
    moho::Platoon {
        OnCreate(plan: &str);
        OnUnitsAddedToPlatoon();
        OnDestroy();
    }

    /// Callbacks on AI brain script objects.
    moho::AIBrain {
        OnStatsTrigger(name: String);
        OnTransportFull();
        OnUnitCapLimitReached();
        OnSpawnPreBuiltUnits();
        OnFailedUnitTransfer();
        OnPlayBusyStagingPlatformsVO();
        OnPlayNoStagingPlatformsVO();
    }

    /// Callbacks on script task objects (`CUnitScriptTask`).
    moho::ScriptTask {
        OnCreate(order: Option<LuaTable>);
        OnDestroy();
    }
}
