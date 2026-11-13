// Determinism: the clippy.toml bans (libm floats, wall clock, hash map
// iteration) hold here outside tests; an #[allow] at a use says why it's safe.
#![cfg_attr(not(test), warn(clippy::disallowed_methods, clippy::disallowed_types))]

pub mod armor;
pub mod army;
pub mod attack_vectors;
pub mod audio;
pub mod adjacency;
pub mod beam;
pub mod blip;
pub mod blueprints;
pub mod formation;
pub mod build;
pub mod callbacks;
pub mod capture;
pub mod categories;
pub mod colors;
pub mod core;
pub mod command;
pub mod construction;
pub mod command_source;
pub mod player_input;
pub mod damage;
pub mod debug;
pub mod debug_draw;
pub mod deposits;
pub mod destroyed;
pub mod disk_find_files;
pub mod effect;
pub mod disk_get_file_info;
pub mod disk_to_local;
pub mod dofile_or_script;
pub mod economy_event;
pub mod entity;
pub mod exists;
pub mod file_collapse_path;
pub mod globals;
pub mod io;
pub mod loader;
pub mod log;
pub mod manipulator;
pub mod mesh;
pub mod moho;
pub mod movement;
pub mod navigation;
pub mod nil_metatable;
pub mod orders;
pub mod overcharge;
pub mod math;
pub mod os;
pub mod replay;
pub mod path_utils;
pub mod pending;
pub mod personality;
pub mod platoon;
pub mod projectile;
pub mod presentation;
pub mod profiling;
pub mod random;
pub mod reclaim;
pub mod repr;
pub mod script_object;
pub mod sh;
pub mod signatures;
pub mod sound;
pub mod script_task;
pub mod shims;
pub mod sacrifice;
pub mod special_action;
pub mod silo;
pub mod teleport;
pub mod sim_time;
pub mod spec_footprints;
pub mod terrain;
pub mod thread_metatable;
pub mod threads;
pub mod threat;
pub mod transport;
pub mod ferry;
pub mod unit;
pub mod unit_stats;
pub mod weapon;

pub fn register_safe(lua: &mlua::Lua) -> mlua::Result<()> {
    // First, so every real implementation registered below replaces its stub.
    pending::register(lua)?;
    globals::register(lua)?;
    log::register(lua)?;
    dofile_or_script::register(lua)?;
    repr::register_safe(lua)?;
    spec_footprints::register_safe(lua)?;
    // io::register(lua)?;
    // os::register(lua)?;
    // sh::register(lua)?;
    shims::register(lua)?;
    thread_metatable::register(lua)?;
    math::register(lua)?;
    colors::register(lua)?;
    disk_find_files::register(lua)?;
    debug::register(lua)?;
    loader::register(lua)?;
    disk_to_local::register(lua)?;
    sound::register(lua)?;
    nil_metatable::register(lua)?;
    // FAF's `factions.lua` indexes `GetFactions(true)`'s `true` with each
    // active mod's uid (sim mods' blueprint load).
    nil_metatable::register_booleans(lua)?;
    nil_metatable::register_number_metatable(lua)?;
    nil_metatable::register_functions(lua)?;
    disk_get_file_info::register(lua)?;
    blueprints::register(lua)?;
    // Up front: registering a mesh later then needs no container borrow
    // (the snapshot registers structure meshes while holding app data).
    lua.set_app_data(mesh::MeshRegistry::default());
    threads::register(lua)?;
    // The sim's yields wait as moho's do (the interface's count frames).
    lua.app_data_mut::<crate::engine::sim::scheduler::Scheduler>().expect("Scheduler not in app_data").set_moho_waits(true);
    random::register(lua)?;
    file_collapse_path::register(lua)?;
    audio::register(lua)?;
    exists::register(lua)?;
    path_utils::register(lua)?;
    moho::register(lua)?;
    core::register(lua)?;
    categories::register(lua)?;
    navigation::register(lua)?;
    army::register(lua)?;
    command_source::register(lua)?;
    terrain::register(lua)?;
    deposits::register(lua)?;
    entity::register(lua)?;
    presentation::register(lua)?;
    effect::register(lua)?;
    profiling::register(lua)?;
    platoon::register(lua)?;
    attack_vectors::register(lua)?;
    sim_time::register(lua)?;
    unit::register(lua)?;
    beam::register(lua)?;
    debug_draw::register(lua)?;
    script_task::register(lua)?;
    economy_event::register(lua)?;
    silo::register(lua)?;
    teleport::register(lua)?;
    overcharge::register(lua)?;
    transport::register(lua)?;
    armor::register(lua)?;
    blip::register(lua)?;
    weapon::register(lua)?;
    projectile::register(lua)?;
    damage::register(lua)?;
    threat::register(lua)?;
    replay::register(lua)?;
    personality::register(lua)?;
    build::register(lua)?;
    command::register(lua)?;
    player_input::register(lua)?;
    movement::register(lua)?;
    mesh::register(lua)?;
    construction::register(lua)?;
    reclaim::register(lua)?;
    capture::register(lua)?;
    manipulator::register(lua)?;
    destroyed::register(lua)?;
    // Last: only where no real method was registered.
    pending::register_methods(lua)?;
    remove_disk_access(lua)
}

/// Replaces Luau's `require` (it loads `.lua` / `.luau` files from anywhere
/// on disk) with one that raises an error, and removes `package`. FAF loads
/// code through `import` / `doscript` (the VFS), so map and mod scripts then
/// reach only the game's mounted files. `require` stays defined: FAF's
/// strict globals make reading an unset one an error, and `dkson.lua`
/// keeps a local copy of it.
pub fn remove_disk_access(lua: &mlua::Lua) -> mlua::Result<()> {
    crate::lua_api::lua_fn!(lua, global_raw,
        /// Always raises: scripts load through `import`, not from disk.
        require = |_, name: mlua::Value| -> mlua::Result<()> {
            Err(mlua::Error::runtime(format!("require({}) is disabled: scripts load through import", name.to_string().unwrap_or_default())))
        })?;
    lua.globals().raw_set("package", mlua::Value::Nil)
}

/// The init VM's API (it runs FAF's init file to learn the mounts, then is
/// thrown away): reads from the real file system, as moho's init does, but
/// only of Lua files, and nothing that writes or deletes.
pub fn register_unsafe(lua: &mlua::Lua) -> mlua::Result<()> {
    remove_disk_access(lua)?;
    globals::register(lua)?;
    log::register(lua)?;
    dofile_or_script::register_unsafe(lua)?;
    io::register_unsafe(lua)?;
    os::register_unsafe(lua)?;
    sh::register_unsafe(lua)?;
    shims::register(lua)?;
    Ok(())
}
