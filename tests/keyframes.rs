//! Keyframes: the sim saved at a tick loads into a fresh engine of
//! the same session and carries on exactly as the original did.
//!
//! Needs the FAF checkout (`../fa`) and game data; fails without them
//! (`RTS_SKIP_DATA_TESTS=1` skips).

use std::sync::{Arc, Mutex};

use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine::{self, keyframe, sim};

const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

fn have_data() -> bool {
    rts_core_engine::test_data::require_game("keyframes")
}

/// The default skirmish (two AIs on Seton's Clutch), started and stopped
/// at its base.
fn started() -> engine::Engine {
    started_with(None, engine::session::SessionConfig { ticks: Some(0), ..Default::default() })
}

fn started_with(vault: Option<&str>, session: engine::session::SessionConfig) -> engine::Engine {
    let mut engine = engine::Engine::new_with_vault(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new())), vault.map(std::path::Path::new)).expect("engine");
    engine.start(&session).expect("session");
    engine
}

fn tick_of(engine: &engine::Engine) -> u64 {
    engine::api::sim_time::current_tick(engine.sim_lua())
}

/// Ticks `engine` to the end of the next collection tick at or after `at`
/// (where keyframes are taken).
fn to_collection_tick(engine: &mut engine::Engine, at: u64) {
    while tick_of(engine) < at || tick_of(engine) % sim::gc::EVERY != sim::gc::EVERY - 1 {
        engine.tick().expect("tick");
    }
}

/// The two keyframes' first difference, if any.
fn difference(a: &keyframe::Keyframe, b: &keyframe::Keyframe) -> Option<String> {
    if a.heap != b.heap {
        let at = a.heap.iter().zip(&b.heap).position(|(x, y)| x != y).unwrap_or(0);
        let what = rts_core_engine::engine::lua::heap_snapshot::locate(&a.heap, at);
        return Some(format!("heap ({} vs {} bytes, first at {at}: {what})", a.heap.len(), b.heap.len()));
    }
    if a.userdata != b.userdata {
        return Some("userdata payloads".into());
    }
    let names: std::collections::BTreeSet<&String> = a.state.keys().chain(b.state.keys()).collect();
    for name in names {
        if a.state.get(name) != b.state.get(name) {
            return Some(format!("state `{name}`"));
        }
    }
    None
}

/// Prints the objects and strings only one of the two heaps has.
fn report_contents(original: &engine::Engine, loaded: &engine::Engine) {
    use rts_core_engine::engine::lua::heap_snapshot::contents;
    let (oa, sa) = contents(original.sim_lua()).unwrap();
    let (ob, sb) = contents(loaded.sim_lua()).unwrap();
    let in_b: std::collections::BTreeSet<_> = ob.iter().collect();
    let in_a: std::collections::BTreeSet<_> = oa.iter().collect();
    eprintln!("objects only in the original: {:?}", oa.iter().filter(|o| !in_b.contains(o)).take(40).collect::<Vec<_>>());
    eprintln!("objects only in the loaded: {:?}", ob.iter().filter(|o| !in_a.contains(o)).take(40).collect::<Vec<_>>());
    let str_b: std::collections::BTreeSet<_> = sb.iter().collect();
    let str_a: std::collections::BTreeSet<_> = sa.iter().collect();
    eprintln!("strings only in the original: {:?}", sa.iter().filter(|s| !str_b.contains(s)).map(|s| String::from_utf8_lossy(s).into_owned()).take(40).collect::<Vec<_>>());
    let pa = rts_core_engine::engine::lua::heap_snapshot::protos(original.sim_lua()).unwrap();
    let pb = rts_core_engine::engine::lua::heap_snapshot::protos(loaded.sim_lua()).unwrap();
    eprintln!("protos only in the original: {:?}", pa.iter().filter(|p| !pb.contains(p)).take(20).collect::<Vec<_>>());
    eprintln!("protos only in the loaded: {:?}", pb.iter().filter(|p| !pa.contains(p)).take(20).collect::<Vec<_>>());
    eprintln!("strings only in the loaded: {:?}", sb.iter().filter(|s| !str_a.contains(s)).map(|s| String::from_utf8_lossy(s).into_owned()).take(40).collect::<Vec<_>>());
}

#[test]
fn a_loaded_keyframe_plays_on_as_the_original() {
    if !have_data() {
        return;
    }
    const AFTER: u64 = 400;
    let mut original = started();
    to_collection_tick(&mut original, 600);
    let at = tick_of(&original);
    let saved = keyframe::save(original.sim_lua(), at).expect("save");
    let state_bytes: usize = saved.state.values().map(Vec::len).sum();
    eprintln!("tick {at}: heap {} bytes, userdata {} bytes, state {state_bytes} bytes", saved.heap.len(), saved.userdata.len());

    let mut loaded = started();
    keyframe::load(loaded.sim_lua(), &saved).expect("load");
    assert_eq!(tick_of(&loaded), at, "the loaded sim is at the saved tick");
    assert_eq!(loaded.checksum(), original.checksum(), "checksum straight after the load");
    let resaved = keyframe::save(loaded.sim_lua(), at).expect("save the loaded sim");
    if saved.heap != resaved.heap {
        report_contents(&original, &loaded);
    }
    if let Some(d) = difference(&saved, &resaved) {
        panic!("the loaded sim saves differently: {d}");
    }

    use rts_core_engine::engine::lua::heap_snapshot::gcid_next;
    for _ in 0..AFTER {
        // The gcid counters part long before the checksums would: a lazy
        // object made on one side only.
        assert_eq!(gcid_next(loaded.sim_lua()).unwrap(), gcid_next(original.sim_lua()).unwrap(), "gcids part before tick {}", tick_of(&original) + 1);
        original.tick().expect("tick");
        loaded.tick().expect("tick");
        assert_eq!(loaded.checksum(), original.checksum(), "checksums part at tick {}", tick_of(&original));
    }
    // The whole state, Lua included, at the next collection tick.
    to_collection_tick(&mut original, at + AFTER);
    to_collection_tick(&mut loaded, at + AFTER);
    let end = tick_of(&original);
    let a = keyframe::save(original.sim_lua(), end).expect("save original");
    let b = keyframe::save(loaded.sim_lua(), end).expect("save loaded");
    if a.heap != b.heap {
        report_contents(&original, &loaded);
    }
    if let Some(d) = difference(&a, &b) {
        panic!("at tick {end} the sims differ: {d}");
    }
}

/// A keyframe taken just after scripts dropped cached category results
/// loads to the same heap. FAF caches `a * b` as `cache[a][b]`: `cache` has
/// weak keys, each row weak values (`/lua/system/categories.lua`). A dropped
/// temporary is then a dead key holding a row (`A * B * C`) or the key of a
/// dead value (`A - (B * C)`), and upstream Luau kept the row, or the key,
/// for one more collection: the saved heap held objects the load's
/// collection dropped (`RTS-CORE-ENGINE EPHEMERON`, `tests/lua_ephemerons.rs`).
/// AIs do this all game; the default skirmish's first minute doesn't.
#[test]
fn a_keyframe_after_dropped_category_results_loads_the_same() {
    if !have_data() {
        return;
    }
    let mut original = started();
    to_collection_tick(&mut original, 100);
    original.tick().expect("tick");
    original
        .sim_lua()
        .load(
            r#"
            local c = categories
            local _ = c.LAND * c.MOBILE * c.TECH1                       -- a row keyed by the temporary LAND * MOBILE
            _ = c.ALLUNITS - (c.AIR * c.TECH2)                          -- a temporary as the key of a dead result
            _ = (c.NAVAL + c.TECH3) - (c.STRUCTURE * (c.AIR + c.LAND))  -- and chains of both
            KeyframeTestKept = c.ALLUNITS - (c.NAVAL * c.TECH3)         -- held: its temporary stays, through the row
            "#,
        )
        .exec()
        .expect("category expressions");
    let now = tick_of(&original);
    to_collection_tick(&mut original, now);
    let at = tick_of(&original);
    let saved = keyframe::save(original.sim_lua(), at).expect("save");

    let loaded = started();
    keyframe::load(loaded.sim_lua(), &saved).expect("load");
    let resaved = keyframe::save(loaded.sim_lua(), at).expect("save the loaded sim");
    if saved.heap != resaved.heap {
        report_contents(&original, &loaded);
    }
    if let Some(d) = difference(&saved, &resaved) {
        panic!("the loaded sim saves differently: {d}");
    }
    // The held result is the cached one in both: the same object again.
    for engine in [&original, &loaded] {
        let hit: bool = engine.sim_lua().load("return KeyframeTestKept == categories.ALLUNITS - (categories.NAVAL * categories.TECH3)").eval().expect("cache hit");
        assert!(hit, "the held result left the cache");
    }
    use rts_core_engine::engine::lua::heap_snapshot::gcid_next;
    assert_eq!(gcid_next(loaded.sim_lua()).unwrap(), gcid_next(original.sim_lua()).unwrap(), "gcids part");
}

/// A keyframe taken mid-ferry (`tests/scenarios/ferry_service.lua`: units
/// riding the route on their beacon order, then delivered) loads and plays
/// on as a straight replay: the ferry routes and waits (`api::ferry`) are
/// saved.
#[test]
fn a_keyframe_mid_ferry_plays_on_as_the_original() {
    if !have_data() {
        return;
    }
    let session = || engine::session::SessionConfig { script: Some("tests/scenarios/ferry_service.lua".into()), ticks: Some(0), create_initial_units: false, ai: false, ..Default::default() };
    let mut original = started_with(None, session());
    let mut saves = Vec::new();
    // Riding (collection ticks: every 100), then delivered.
    let mut states = Vec::new();
    for at in [10, 150, 250, 350] {
        to_collection_tick(&mut original, at);
        let tick = tick_of(&original);
        states.push(engine::api::ferry::waiting(original.sim_lua()));
        saves.push((tick, keyframe::save(original.sim_lua(), tick).expect("save")));
        original.tick().expect("tick");
    }
    eprintln!("(waiting, riding) at the saves: {states:?}");
    assert!(states.iter().any(|&(w, _)| w > 0), "a save with units on a ferry wait: {states:?}");
    assert!(states.iter().any(|&(_, r)| r > 0), "a save with units riding: {states:?}");
    for (at, saved) in saves {
        let a = started_with(None, session());
        keyframe::load(a.sim_lua(), &saved).expect("load");
        let resaved = keyframe::save(a.sim_lua(), at).expect("save the loaded sim");
        if let Some(d) = difference(&saved, &resaved) {
            panic!("the sim loaded at {at} saves differently: {d}");
        }
        let mut fresh = started_with(None, session());
        while tick_of(&fresh) < at {
            fresh.tick().expect("tick");
        }
        // (`fresh` is a straight replay up to the save: it then matches the
        // loaded one tick for tick.)
        let mut loaded = started_with(None, session());
        keyframe::load(loaded.sim_lua(), &saved).expect("load");
        assert_eq!(loaded.checksum(), fresh.checksum(), "checksum straight after the load at {at}");
        for _ in 0..300 {
            fresh.tick().expect("tick");
            loaded.tick().expect("tick");
            assert_eq!(loaded.checksum(), fresh.checksum(), "checksums part at tick {}", tick_of(&fresh));
        }
    }
}

/// The same check late in a big game, with sizes and times: an 8v8 of AIs
/// on monument_valley_63 (from `../faf-vault`) saved at tick 18000 (30
/// minutes) by default. Slow (plays the game twice): run it on purpose,
///
/// ```sh
/// cargo test --release --test keyframes late_game -- --ignored --nocapture
/// KEYFRAME_AT=30000 KEYFRAME_AFTER=1000 cargo test --release --test keyframes late_game -- --ignored --nocapture
/// ```
#[test]
#[ignore = "plays a 30-minute 8v8 twice; run on purpose"]
fn late_game_keyframe_plays_on_as_the_original() {
    if !have_data() {
        return;
    }
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (at, after) = (env("KEYFRAME_AT", 18000), env("KEYFRAME_AFTER", 600));
    let vault = "../faf-vault";
    let session = engine::session::SessionConfig {
        scenario: "/maps/monument_valley_63/monument_valley_63_scenario.lua".into(),
        ai_armies: 16,
        auto_teams: engine::session::AutoTeams::from_key("tvsb"),
        ticks: Some(0),
        ..Default::default()
    };
    let mut original = started_with(Some(vault), session.clone());
    to_collection_tick(&mut original, at);
    let at = tick_of(&original);
    let t = std::time::Instant::now();
    let saved = keyframe::save(original.sim_lua(), at).expect("save");
    let save_ms = t.elapsed().as_secs_f64() * 1000.0;
    let mut parts: Vec<(&String, usize)> = saved.state.iter().map(|(k, v)| (k, v.len())).collect();
    parts.sort_by_key(|p| std::cmp::Reverse(p.1));
    let state_bytes: usize = parts.iter().map(|p| p.1).sum();
    eprintln!("tick {at}: saved in {save_ms:.0} ms; heap {:.1} MB, userdata {:.2} MB, state {:.1} MB", mb(saved.heap.len()), mb(saved.userdata.len()), mb(state_bytes));
    eprintln!("largest state parts: {:?}", parts.iter().take(12).map(|(k, n)| format!("{k} {:.2} MB", mb(*n))).collect::<Vec<_>>());

    let mut loaded = started_with(Some(vault), session);
    let t = std::time::Instant::now();
    keyframe::load(loaded.sim_lua(), &saved).expect("load");
    eprintln!("loaded in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    assert_eq!(loaded.checksum(), original.checksum(), "checksum straight after the load");
    let resaved = keyframe::save(loaded.sim_lua(), at).expect("save the loaded sim");
    if let Some(d) = difference(&saved, &resaved) {
        if saved.heap != resaved.heap {
            report_contents(&original, &loaded);
        }
        panic!("the loaded sim saves differently: {d}");
    }
    use rts_core_engine::engine::lua::heap_snapshot::gcid_next;
    for _ in 0..after {
        assert_eq!(gcid_next(loaded.sim_lua()).unwrap(), gcid_next(original.sim_lua()).unwrap(), "gcids part before tick {}", tick_of(&original) + 1);
        original.tick().expect("tick");
        loaded.tick().expect("tick");
        if loaded.checksum() != original.checksum() {
            let t = tick_of(&original);
            let a = keyframe::save(original.sim_lua(), t).expect("save original");
            let b = keyframe::save(loaded.sim_lua(), t).expect("save loaded");
            let parts: Vec<&String> = a.state.keys().filter(|k| a.state.get(*k) != b.state.get(*k)).collect();
            panic!("checksums part at tick {t}; state parts that differ: {parts:?}; heap {}", if a.heap == b.heap { "same" } else { "differs" });
        }
    }
    to_collection_tick(&mut original, at + after);
    to_collection_tick(&mut loaded, at + after);
    let end = tick_of(&original);
    let a = keyframe::save(original.sim_lua(), end).expect("save original");
    let b = keyframe::save(loaded.sim_lua(), end).expect("save loaded");
    if a.heap != b.heap {
        report_contents(&original, &loaded);
    }
    if let Some(d) = difference(&a, &b) {
        panic!("at tick {end} the sims differ: {d}");
    }
}

fn mb(n: usize) -> f64 {
    n as f64 / 1_048_576.0
}

/// A replay played straight, against the same replay resumed from a
/// keyframe another run of it wrote (a seek): the same game at the end.
#[test]
fn a_replay_resumed_from_a_keyframe_matches_one_played_straight() {
    if !have_data() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("keyframes-replay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let recording = dir.join("game.rtsreplay");
    {
        let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
        engine.record_commands(&recording).expect("record");
        engine.start(&engine::session::SessionConfig { ticks: Some(0), ..Default::default() }).expect("session");
        for _ in 0..1000 {
            engine.tick().expect("tick");
        }
    }
    // A replay engine: the recording's session, keyframes in `keyframes`.
    let replaying = |keyframes: Option<(u64, Option<u64>)>| {
        let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
        if let Some((every, resume)) = keyframes {
            engine.set_keyframes(dir.join("keyframes"), every, resume);
        }
        let (_, header) = engine.replay_commands(&recording).expect("replay");
        let session = engine::session::SessionConfig { ticks: Some(0), ..header.session.expect("the recording's session") };
        engine.start(&session).expect("session");
        engine
    };
    let mut straight = replaying(None);
    while tick_of(&straight) < 900 {
        straight.tick().expect("tick");
    }
    {
        let mut writing = replaying(Some((300, None)));
        while tick_of(&writing) < 700 {
            writing.tick().expect("tick");
        }
    }
    let store = keyframe::store::Store::new(dir.join("keyframes"), keyframe::recorder::FULL_EVERY);
    assert_eq!(store.entries().iter().map(|e| e.tick).collect::<Vec<_>>(), vec![299, 599], "keyframes every 300 ticks");
    let mut resumed = replaying(Some((300, Some(800))));
    assert_eq!(tick_of(&resumed), 599, "resumed from the latest keyframe before tick 800");
    while tick_of(&resumed) < 900 {
        resumed.tick().expect("tick");
    }
    assert_eq!(resumed.checksum(), straight.checksum(), "the resumed replay is the same game at tick 900");
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a watched replay's engine has that the run-ahead process's doesn't
/// (a renderer, real-time control, the stall watchdog) makes no objects
/// before the base: their keyframes load into each other.
#[test]
fn a_windowed_engine_has_the_same_base() {
    if !have_data() {
        return;
    }
    let plain = started();
    let mut windowed = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let (sink, _snapshots, _request) = engine::snapshot::channel();
    windowed.set_snapshot_sink(sink);
    let (_orders, link) = engine::sim::input::channel();
    windowed.set_input_link(link);
    windowed.set_sim_control(engine::snapshot::SimControl::new(100));
    windowed.set_pace(100);
    windowed.start_watchdog(engine::watchdog::Config { stall: std::time::Duration::from_secs(3600), exit_after: None }).expect("watchdog");
    windowed.start(&engine::session::SessionConfig { ticks: Some(0), ..Default::default() }).expect("session");
    let base = |e: &engine::Engine| e.sim_lua().app_data_ref::<keyframe::SessionBase>().map(|b| b.0);
    assert_eq!(base(&windowed), base(&plain));
    // And they play the same game, object for object (a keyframe the
    // run-ahead process writes is the game the viewer would have reached).
    let mut plain = plain;
    use rts_core_engine::engine::lua::heap_snapshot::gcid_next;
    for _ in 0..300 {
        plain.tick().expect("tick");
        windowed.tick().expect("tick");
        assert_eq!(gcid_next(windowed.sim_lua()).unwrap(), gcid_next(plain.sim_lua()).unwrap(), "gcids part at tick {}", tick_of(&plain));
        assert_eq!(windowed.checksum(), plain.checksum(), "checksums part at tick {}", tick_of(&plain));
    }
}
