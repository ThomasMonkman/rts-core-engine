//! The engine's own sim state in a keyframe: each type the sim keeps in the
//! VM's app data that changes after game start-up, saved with postcard.
//!
//! Left out, because the loading engine's own start-up makes them the same
//! (they don't change after it): `BlueprintRegistry`, `ArmorTable`,
//! `ClassTables`, `Keys`, `ModHooks`, `RegisteredFootprints`,
//! `BlockingTypes`, `MapPath`. Left out as caches filled from files, not
//! Lua: `SkeletonCache` (skeletons are resolved again), `SkirtCache`, and
//! `Clips`, saved by path only (posing reads only clips already loaded, so
//! they are loaded again). Left out as the session's or
//! the renderer's, not the sim's: the snapshot sink, the recorder, the input
//! link, lockstep, `ThreadOrigins` (Tracy zone names), `StageTimes`, the
//! watchdog. Caches that fill from Lua are saved (see the list).

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api;
use crate::engine::lua::heap_snapshot::LoadedHeap;
use crate::engine::sim;

/// Saves and loads each listed type: present in app data or not.
macro_rules! saved_types {
    ($($name:literal => $ty:ty),* $(,)?) => {
        fn save_plain(lua: &Lua, out: &mut BTreeMap<String, Vec<u8>>) -> LuaResult<()> {
            $(
                if let Some(v) = lua.app_data_ref::<$ty>() {
                    out.insert($name.to_string(), postcard::to_allocvec(&*v).map_err(|e| LuaError::runtime(format!("keyframe: saving {}: {e}", $name)))?);
                }
            )*
            Ok(())
        }

        fn load_plain(lua: &Lua, parts: &BTreeMap<String, Vec<u8>>) -> LuaResult<()> {
            $(
                match parts.get($name) {
                    Some(bytes) => {
                        let v: $ty = postcard::from_bytes(bytes).map_err(|e| LuaError::runtime(format!("keyframe: loading {}: {e}", $name)))?;
                        lua.set_app_data(v);
                    }
                    None => {
                        lua.remove_app_data::<$ty>();
                    }
                }
            )*
            Ok(())
        }
    };
}

saved_types! {
    "armies" => sim::army::Armies,
    "commands" => sim::command::Commands,
    "command_stream" => sim::input::CommandStream,
    "deposits" => sim::deposit::Deposits,
    "economy_events" => sim::economy::EconomyEvents,
    "entities" => sim::entity::EntityWorld,
    "intel" => sim::vision::IntelGrids,
    "manipulators" => sim::manipulator::Manipulators,
    "nav" => sim::nav::NavGrid,
    "platoons" => sim::platoon::Platoons,
    "presentation" => sim::presentation::Presentation,
    "projectile_icons" => sim::projectile::ProjectileIcons,
    "projectiles" => sim::projectile::Projectiles,
    "rng" => sim::rng::SimRng,
    "scrolled" => sim::scroller::Scrolled,
    "fall_motors" => sim::fall_down::FallMotors,
    "terrain" => sim::terrain::Terrain,
    "threat" => sim::threat::ThreatMap,
    "weapons" => sim::weapon::Weapons,
    "bounded_props" => sim::bounded_props::BoundedProps,
    "navigator" => sim::navigation::NavigatorConfig,
    "assists" => api::construction::Assists,
    "builds" => api::construction::Builds,
    "cap_blocked" => api::construction::CapBlocked,
    "face_to_build" => api::construction::FaceToBuild,
    "repair_tasks" => api::construction::RepairTasks,
    "guard_tasks" => api::construction::GuardTasks,
    "unfinished" => api::construction::Unfinished,
    "walk_outs" => api::construction::WalkOuts,
    "attack_vectors" => api::attack_vectors::AttackVectors,
    "beaming" => api::transport::Beaming,
    "loading" => api::transport::Loading,
    "ferry_tasks" => api::transport::FerryTasks,
    "ferry_routes" => api::ferry::FerryRoutes,
    "ferry_waits" => api::ferry::FerryWaits,
    "carrier_launches" => api::transport::CarrierLaunches,
    "beams" => api::beam::Beams,
    "captures" => api::capture::Captures,
    "cheats" => api::command_source::Cheats,
    "reported" => api::economy_event::Reported,
    "falling" => api::movement::Falling,
    "surface_requests" => api::movement::SurfaceRequests,
    "movement" => api::movement::MovementScratch,
    "game_over" => api::sim_time::GameOver,
    "personality_names" => api::personality::PersonalityNames,
    "reclaims" => api::reclaim::Reclaims,
    "script_tasks" => api::script_task::ScriptTasks,
    "silo_jobs" => api::silo::SiloJobs,
    "teleports" => api::teleport::Teleports,
    "overcharge_tasks" => api::overcharge::OverchargeTasks,
    "playable_rect" => api::terrain::PlayableRect,
    "ignore_playable_rect" => api::terrain::IgnorePlayableRect,
    "pending_sync" => crate::engine::sync::PendingSync,
    "hidden_bone_owners" => crate::engine::snapshot::HiddenBoneOwners,
    "meshes" => api::mesh::MeshRegistry,
    // Caches, saved anyway: filling one reads blueprints through Lua, which
    // can make objects; a loaded game must miss them where the saved one
    // would have, or every later gcid shifts.
    "footprints" => api::construction::Footprints,
    "decay_rates" => api::construction::DecayRates,
    "lifetimes" => api::effect::Lifetimes,
    "buildable" => api::build::BuildableCache,
}

/// Entities' skeletons are shared mesh assets, not saved: each unit and
/// prop gets its mesh's again (its `SetMesh` override, else its
/// blueprint's), as spawning and `SetMesh` gave it.
fn resolve_skeletons(lua: &Lua) -> LuaResult<()> {
    use sim::entity::EntityKind;
    let wanted: Vec<(sim::entity::EntityId, crate::engine::api::blueprints::BlueprintKind, String, Option<String>)> = match lua.app_data_ref::<sim::entity::EntityWorld>() {
        Some(w) => w
            .iter()
            .filter(|e| !e.destroyed && matches!(e.kind, EntityKind::Unit | EntityKind::Prop))
            .map(|e| (e.id, e.cold.blueprint_kind, e.cold.blueprint_id.clone(), e.cold.mesh_override.clone()))
            .collect(),
        None => return Ok(()),
    };
    for (id, kind, bp_id, mesh) in wanted {
        let skeleton = match mesh {
            Some(m) => api::mesh::skeleton_for_mesh(lua, &m)?,
            None => api::mesh::skeleton_for(lua, &api::entity::blueprint_table(lua, kind, &bp_id)?)?,
        };
        if let Some(mut w) = lua.app_data_mut::<sim::entity::EntityWorld>()
            && let Some(e) = w.get_mut(id) {
            e.cold.skeleton = skeleton;
        }
    }
    Ok(())
}

/// The engine's sim state, by name.
pub fn save(lua: &Lua) -> LuaResult<BTreeMap<String, Vec<u8>>> {
    let mut parts = BTreeMap::new();
    save_plain(lua, &mut parts)?;
    if let Some(c) = lua.app_data_ref::<sim::pose::Clips>() {
        parts.insert("clips".into(), postcard::to_allocvec(&c.paths()).map_err(LuaError::external)?);
    }
    if let Some(s) = lua.app_data_ref::<sim::scheduler::Scheduler>() {
        let saved = s.save(lua)?;
        parts.insert("scheduler".into(), postcard::to_allocvec(&saved).map_err(LuaError::external)?);
    }
    Ok(parts)
}

/// Puts `parts` back, the Lua objects they name from the loaded `heap`.
pub fn load(lua: &Lua, parts: &BTreeMap<String, Vec<u8>>, heap: &LoadedHeap) -> LuaResult<()> {
    // The renderer's prop tracking belongs to this engine, not the saved one.
    let track_props = lua.app_data_ref::<sim::entity::EntityWorld>().is_some_and(|w| w.tracks_props());
    load_plain(lua, parts)?;
    if let Some(mut w) = lua.app_data_mut::<sim::entity::EntityWorld>() {
        w.set_track_props(track_props);
    }
    resolve_skeletons(lua)?;
    if let Some(bytes) = parts.get("clips") {
        let paths: Vec<String> = postcard::from_bytes(bytes).map_err(LuaError::external)?;
        for path in paths {
            api::manipulator::clip_duration(lua, &path);
        }
    }

    if let Some(bytes) = parts.get("scheduler") {
        let saved: sim::scheduler::SavedScheduler = postcard::from_bytes(bytes).map_err(LuaError::external)?;
        let s = sim::scheduler::Scheduler::load(lua, &saved, heap)?;
        lua.set_app_data(s);
    }
    Ok(())
}
