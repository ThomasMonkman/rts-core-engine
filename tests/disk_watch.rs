//! `--enable-disk-watch` end to end: a running game with a mod whose files
//! are edited on disk. The edited blueprint reaches units made after the
//! reload (through FAF's `ReloadBlueprint`), the edited sim module is what
//! `import` gives next (FAF's `dirty_module`), and the game's recording is
//! marked as reloaded and refused when loaded.
//!
//! Its own test binary: disk watching is a process-wide switch, and the
//! other tests' game files must not be watched. Needs the FAF checkout and
//! game data (`RTS_SKIP_DATA_TESTS=1` skips).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::prelude::*;
use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine;

const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

/// A vault with one sim mod: a blueprint merged into the UEF engineer and a
/// module with a value.
fn vault(health: u32, value: u32) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("disk_watch_vault_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let m = dir.join("mods/reloadtest");
    std::fs::create_dir_all(m.join("units")).unwrap();
    std::fs::create_dir_all(m.join("lua")).unwrap();
    std::fs::write(m.join("mod_info.lua"), "name = 'Reload test'\nuid = 'reload-test'\nversion = 1\nauthor = 'tests'\ndescription = ''\n").unwrap();
    write_mod(&dir, health, value);
    dir
}

fn write_mod(vault: &Path, health: u32, value: u32) {
    let m = vault.join("mods/reloadtest");
    std::fs::write(m.join("units/uel0105_merge.bp"), format!("UnitBlueprint {{ Merge = true, BlueprintId = 'uel0105', Defense = {{ MaxHealth = {health} }} }}\n")).unwrap();
    std::fs::write(m.join("lua/reloadtest.lua"), format!("Value = {value}\n")).unwrap();
}

fn eval<T: FromLuaMulti>(engine: &engine::Engine, code: &str) -> T {
    engine.sim_lua().load(code).eval().unwrap_or_else(|e| panic!("{code}: {e}"))
}

#[test]
fn edited_mod_files_reload_into_a_running_game() {
    if !rts_core_engine::test_data::require_game("disk watch test") {
        return;
    }
    let vault = vault(1234, 1);
    let replay = vault.join("game.rtsreplay");
    engine::disk_watch::enable();
    let mut engine = engine::Engine::new_with_vault(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new())), Some(&vault)).expect("engine");
    engine.record_commands(&replay).expect("record");
    let session = engine::session::SessionConfig { mods: vec!["reload-test".into()], ticks: Some(0), create_initial_units: false, ai: false, ..Default::default() };
    engine.start(&session).expect("session");
    engine.tick().unwrap();

    let make = "local u = CreateUnitHPR('uel0105', ListArmies()[1], 10, 0, 10, 0, 0, 0) return u";
    let before: LuaTable = eval(&engine, make);
    engine.sim_lua().globals().set("Before", before).unwrap();
    assert_eq!(eval::<f64>(&engine, "return Before:GetMaxHealth()"), 1234.0, "the mod's blueprint is merged in");
    assert_eq!(eval::<i64>(&engine, "return import('/mods/reloadtest/lua/reloadtest.lua').Value"), 1);

    // Edit both files, then play until the sim has the new blueprint.
    write_mod(&vault, 4321, 2);
    let deadline = Instant::now() + Duration::from_secs(20);
    while eval::<f64>(&engine, "return __blueprints.uel0105.Defense.MaxHealth") != 4321.0 {
        assert!(Instant::now() < deadline, "the edited blueprint never reached the sim");
        std::thread::sleep(Duration::from_millis(50));
        engine.tick().unwrap();
    }

    let after: LuaTable = eval(&engine, make);
    engine.sim_lua().globals().set("After", after).unwrap();
    assert_eq!(eval::<f64>(&engine, "return After:GetMaxHealth()"), 4321.0, "a unit made after the reload has the new blueprint");
    assert_eq!(eval::<f64>(&engine, "return Before:GetMaxHealth()"), 1234.0, "one made before keeps what it was made with (as in moho)");
    assert_eq!(eval::<i64>(&engine, "return import('/mods/reloadtest/lua/reloadtest.lua').Value"), 2, "import runs the edited module");

    // The recording says so, and won't load.
    let text = std::fs::read_to_string(&replay).unwrap();
    assert!(engine::replay::reloaded_at(&text).is_some(), "the replay is marked:\n{text}");
    let err = engine.replay_commands(&replay).expect_err("a reloaded game's replay is refused");
    assert!(err.contains("reloaded"), "{err}");
    let _ = std::fs::remove_dir_all(&vault);
}
