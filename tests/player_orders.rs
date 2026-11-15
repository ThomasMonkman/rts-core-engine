//! Player commands end to end: commands enter through the sim's command
//! stream (as the renderer sends them), are applied at their tick, checked
//! against the sending army, and replay exactly from a recording.
//!
//! Needs the FAF checkout and game data, like the scenario tests (fails
//! without them; `RTS_SKIP_DATA_TESTS=1` skips).

use std::sync::{Arc, Mutex};

use mlua::prelude::*;
use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine;
use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget, Commands};
use rts_core_engine::engine::sim::entity::EntityWorld;
use rts_core_engine::engine::sim::input::{PlayerCommand, Scheduled, SimCallback};
use rts_core_engine::engine::sim::lua_data::LuaData;
use rts_core_engine::engine::sim::math::FxVec3;

const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

/// Fails without the game data (`RTS_SKIP_DATA_TESTS=1` skips instead).
fn have_data() -> bool {
    rts_core_engine::test_data::require_game("player order tests")
}

/// A session with no AI; army 1 gets six tanks, army 2 one tank to shoot at.
/// Returns (engine, army 1 tanks, army 2 tank).
fn setup(record: Option<&std::path::Path>) -> (engine::Engine, Vec<u32>, u32) {
    let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    if let Some(p) = record {
        engine.record_commands(p).expect("record");
    }
    let session = engine::session::SessionConfig { ticks: Some(0), create_initial_units: false, ai: false, ..Default::default() };
    engine.start(&session).expect("session");
    let ids: LuaTable = engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            for i = 0, 5 do
                local x, z = 390 + math.mod(i, 3) * 3, 580 + math.floor(i / 3) * 3
                local u = CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
                table.insert(ids, tonumber(u:GetEntityId()))
            end
            local e = CreateUnitHPR('uel0201', 'ARMY_2', 440, GetTerrainHeight(440, 600), 600, 0, 0, 0)
            table.insert(ids, tonumber(e:GetEntityId()))
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let mut ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let enemy = ids.pop().unwrap();
    (engine, ids, enemy)
}

fn tick_now(engine: &engine::Engine) -> u64 {
    rts_core_engine::engine::api::sim_time::current_tick(engine.sim_lua())
}

fn send(engine: &engine::Engine, tick: u64, seq: u32, command: PlayerCommand) {
    rts_core_engine::engine::api::player_input::schedule(engine.sim_lua(), [Scheduled { tick, client: 0, seq, command }]);
}

fn position(engine: &engine::Engine, id: u32) -> (f64, f64) {
    let w = engine.sim_lua().app_data_ref::<EntityWorld>().unwrap();
    let (x, _, z) = w.get(id).unwrap().position().to_f64();
    (x, z)
}

fn ground(x: f64, z: f64) -> CommandTarget {
    CommandTarget::Ground(FxVec3::from_f64(x, 0.0, z))
}

fn move_to(army: u32, units: &[u32], x: f64, z: f64) -> PlayerCommand {
    // What the renderer sends for a right-click: a plain move, no
    // formation (FAF's UI only ever sends RULEUCC_Move).
    PlayerCommand { army, kind: CommandKind::Move, units: units.to_vec(), target: ground(x, z), queue: false, formation: None, facing: None, blueprint: None, count: 0, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None }
}

/// A hand-issued multi-unit move goes out as a plain `Move` to every unit
/// (FAF's UI only ever sends `RULEUCC_Move`): no slots, no travelling
/// formation. Formations come only from sim-side callers (`IssueFormMove`,
/// platoons).
#[test]
fn hand_issued_multi_unit_move_is_a_plain_move() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, move_to(1, &tanks, 420.0, 640.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
    for &id in &tanks {
        let kinds: Vec<CommandKind> = c.queue(id).map(|c| c.kind).collect();
        assert_eq!(kinds, vec![CommandKind::Move], "tank {id} beelines, never forms up");
        assert!(c.queue(id).all(|c| c.formation.is_empty() && c.travel.is_empty()), "tank {id} has no slots");
    }
}

#[test]
fn plain_move_from_the_command_stream_arrives() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, move_to(1, &tanks, 420.0, 640.0));
    for _ in 0..400 {
        engine.tick().unwrap();
    }
    let pts: Vec<(f64, f64)> = tanks.iter().map(|&id| position(&engine, id)).collect();
    for (i, p) in pts.iter().enumerate() {
        let d = ((p.0 - 420.0).powi(2) + (p.1 - 640.0).powi(2)).sqrt();
        assert!(d < 15.0, "tank {i} at {p:?}, {d:.1} from the goal");
    }
}

/// The player's patrol to one point loops: the interface's leg back to
/// where the unit started follows it (`Command::patrol_home`), where a
/// script's single `IssuePatrol` point just ends (tests/scenarios
/// patrol_single).
#[test]
fn players_single_point_patrol_loops_back() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let t = tick_now(&engine) + 1;
    let start = position(&engine, tanks[0]);
    send(&engine, t, 0, PlayerCommand { kind: CommandKind::Patrol, ..move_to(1, &tanks[..1], 405.0, 580.0) });
    let (mut back, mut given) = (false, false);
    for _ in 0..400 {
        engine.tick().unwrap();
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        let q: Vec<_> = c.queue(tanks[0]).map(|x| (x.kind, x.target)).collect();
        given |= !q.is_empty();
        assert!(!given || !q.is_empty(), "the patrol never ends");
        if let [(CommandKind::Patrol, CommandTarget::Ground(p))] = q[..] {
            let (x, _, z) = p.to_f64();
            back |= (x - start.0).abs() < 1.0 && (z - start.1).abs() < 1.0;
        }
    }
    assert!(given && back, "a leg back to the start followed the first");
}

#[test]
fn commands_only_move_the_senders_units() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, enemy) = setup(None);
    let before: Vec<(f64, f64)> = tanks.iter().map(|&id| position(&engine, id)).collect();
    let t = tick_now(&engine) + 1;
    // Army 2 tries to order army 1's tanks; army 1 tries to order army 2's.
    send(&engine, t, 0, move_to(2, &tanks, 420.0, 640.0));
    send(&engine, t, 1, move_to(1, &[enemy], 300.0, 300.0));
    for _ in 0..50 {
        engine.tick().unwrap();
    }
    let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
    for (&id, b) in tanks.iter().zip(&before) {
        assert!(c.is_idle(id), "tank {id} took another army's order");
        assert_eq!(position(&engine, id), *b);
    }
    assert!(c.is_idle(enemy));
}

/// A queued waypoint dragged in the command graph (moho's
/// `SetCommandTarget`): the shared order moves for every unit on it and they
/// go to the new spot; another army can't move it, and a ground order can't
/// be dragged onto a unit.
#[test]
fn a_dragged_waypoint_moves_the_order() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, enemy) = setup(None);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, move_to(1, &tanks, 420.0, 640.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    let id = engine.sim_lua().app_data_ref::<Commands>().unwrap().current(tanks[0]).unwrap().id;
    let drag = |army: u32, target: CommandTarget| PlayerCommand { units: Vec::new(), target, retarget: Some(id), ..move_to(army, &[], 0.0, 0.0) };
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, drag(2, ground(300.0, 300.0)));
    send(&engine, t, 1, drag(1, CommandTarget::Entity(enemy)));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(engine.sim_lua().app_data_ref::<Commands>().unwrap().get(id).unwrap().target, ground(420.0, 640.0), "neither moved it");
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, drag(1, ground(400.0, 520.0)));
    for _ in 0..400 {
        engine.tick().unwrap();
    }
    for (i, &tank) in tanks.iter().enumerate() {
        let p = position(&engine, tank);
        let d = ((p.0 - 400.0).powi(2) + (p.1 - 520.0).powi(2)).sqrt();
        assert!(d < 15.0, "tank {i} at {p:?}, {d:.1} from the moved waypoint");
    }
}

#[test]
fn queued_orders_append_and_stop_clears() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, enemy) = setup(None);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, move_to(1, &tanks, 420.0, 640.0));
    let mut attack = PlayerCommand { kind: CommandKind::Attack, target: CommandTarget::Entity(enemy), queue: true, ..move_to(1, &tanks, 0.0, 0.0) };
    send(&engine, t, 1, attack.clone());
    engine.tick().unwrap();
    engine.tick().unwrap();
    {
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        let kinds: Vec<CommandKind> = c.queue(tanks[0]).map(|c| c.kind).collect();
        assert_eq!(kinds, vec![CommandKind::Move, CommandKind::Attack]);
    }
    // Without shift, the attack replaces the queue.
    attack.queue = false;
    send(&engine, tick_now(&engine), 2, attack);
    engine.tick().unwrap();
    {
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        let kinds: Vec<CommandKind> = c.queue(tanks[0]).map(|c| c.kind).collect();
        assert_eq!(kinds, vec![CommandKind::Attack]);
    }
    let stop = PlayerCommand { kind: CommandKind::Stop, target: CommandTarget::None, ..move_to(1, &tanks, 0.0, 0.0) };
    send(&engine, tick_now(&engine), 3, stop);
    engine.tick().unwrap();
    let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
    assert!(tanks.iter().all(|&id| c.is_idle(id)));
}

#[test]
fn attack_order_kills_the_target() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, enemy) = setup(None);
    let attack = PlayerCommand { kind: CommandKind::Attack, target: CommandTarget::Entity(enemy), ..move_to(1, &tanks, 0.0, 0.0) };
    send(&engine, tick_now(&engine) + 1, 0, attack);
    let alive = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(enemy).is_some_and(|x| !x.destroyed);
    for _ in 0..600 {
        if !alive(&engine) {
            break;
        }
        engine.tick().unwrap();
    }
    assert!(!alive(&engine), "six tanks should kill one in a minute");
}

#[test]
fn recorded_commands_replay_to_the_same_state() {
    if !have_data() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("rts-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("orders.txt");
    let run = |record: Option<&std::path::Path>, replay: Option<&std::path::Path>| -> u64 {
        let (mut engine, tanks, enemy) = setup(record);
        if let Some(r) = replay {
            assert_eq!(engine.replay_commands(r).unwrap().0, 3);
        } else {
            let t = tick_now(&engine);
            send(&engine, t + 1, 0, move_to(1, &tanks[..3], 420.0, 640.0));
            send(&engine, t + 20, 1, PlayerCommand { kind: CommandKind::Attack, target: CommandTarget::Entity(enemy), ..move_to(1, &tanks[3..], 0.0, 0.0) });
            send(&engine, t + 25, 2, PlayerCommand { kind: CommandKind::Patrol, queue: true, ..move_to(1, &tanks[..3], 400.0, 600.0) });
        }
        for _ in 0..300 {
            engine.tick().unwrap();
        }
        engine.checksum()
    };
    let recorded = run(Some(&file), None);
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().filter(|l| !l.starts_with('#')).count(), 3, "recording:\n{text}");
    assert!(text.starts_with("# rts-core-engine replay 1\n# build "), "a header naming the build: {text}");
    let replayed = run(None, Some(&file));
    assert_eq!(recorded, replayed, "replay diverged; recording:\n{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A game recorded into the replays folder (the default recording) replays
/// from the file alone: its header gives the session (here an AI game with
/// a seed), so the replay ends in the same state.
#[test]
fn default_recording_replays_from_the_file_alone() {
    if !have_data() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("rts-replays-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let session = engine::session::SessionConfig { seed: 7, ai_armies: 2, ticks: Some(0), human_army: Some(1), ..Default::default() };
    let new_engine = || engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    // Played: the human army's ACU walks off while the AI plays.
    let recorded = {
        let mut engine = new_engine();
        engine.record_into(&dir);
        engine.start(&session).expect("session");
        let acu: u32 = engine.sim_lua().load("return tonumber(ArmyBrains[1]:GetListOfUnits(categories.COMMAND, false)[1]:GetEntityId())").eval().unwrap();
        send(&engine, tick_now(&engine) + 1, 0, move_to(1, &[acu], 300.0, 300.0));
        for _ in 0..300 {
            engine.tick().unwrap();
        }
        engine.checksum()
    };
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(files.len(), 1, "one recording: {files:?}");
    assert!(files[0].to_string_lossy().ends_with("_scmp_009.rtsreplay"), "named by time and map: {:?}", files[0]);
    // Replayed: nothing but the file.
    let replayed = {
        let mut engine = new_engine();
        let (n, header) = engine.replay_commands(&files[0]).unwrap();
        assert_eq!(n, 1);
        let recorded_session = header.session.expect("the header's session");
        assert_eq!(recorded_session.seed, 7);
        engine.start(&engine::session::SessionConfig { ticks: Some(0), ..recorded_session }).expect("session");
        for _ in 0..300 {
            engine.tick().unwrap();
        }
        engine.checksum()
    };
    assert_eq!(recorded, replayed, "the replay diverged");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Builds from the command stream (what the construction panel sends): an
/// engineer builds a structure at a point; a factory queues units.
#[test]
fn build_orders_from_the_command_stream() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local eng = CreateUnitHPR('uel0105', 'ARMY_1', 420, GetTerrainHeight(420, 620), 620, 0, 0, 0)
            local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            return { tonumber(eng:GetEntityId()), tonumber(fac:GetEntityId()) }
            "#,
        )
        .eval()
        .unwrap();
    let (eng, fac): (u32, u32) = (ids.raw_get(1).unwrap(), ids.raw_get(2).unwrap());
    let t = tick_now(&engine) + 1;
    let build = PlayerCommand { kind: CommandKind::BuildMobile, units: vec![eng], target: ground(426.5, 620.5), queue: false, formation: None, facing: None, blueprint: Some("ueb1101".into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    let factory = PlayerCommand { kind: CommandKind::BuildFactory, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some("uel0105".into()), count: 2, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, t, 0, build);
    send(&engine, t, 1, factory);
    let count = |e: &engine::Engine, bp: &str| -> usize { e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().filter(|x| !x.destroyed && x.cold.blueprint_id == bp && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).count() };
    for _ in 0..1200 {
        engine.tick().unwrap();
        if count(&engine, "ueb1101") >= 1 && count(&engine, "uel0105") >= 3 {
            break;
        }
    }
    assert_eq!(count(&engine, "ueb1101"), 1, "the engineer built the power generator");
    assert!(count(&engine, "uel0105") >= 3, "the factory built two engineers (plus the first)");
}

/// Unit settings from the command stream (the order panel's toggles) go
/// through the units' own methods: fire state, pause, and script bits.
#[test]
fn toggles_from_the_command_stream() {
    use rts_core_engine::engine::sim::input::Toggle;
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let t = tick_now(&engine) + 1;
    let base = PlayerCommand { kind: CommandKind::Stop, target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: None, count: 0, ..move_to(1, &tanks[..2], 0.0, 0.0) };
    send(&engine, t, 0, PlayerCommand { toggle: Some(Toggle::FireState(1)), ..base.clone() });
    send(&engine, t, 1, PlayerCommand { toggle: Some(Toggle::Paused(true)), ..base.clone() });
    send(&engine, t, 2, PlayerCommand { toggle: Some(Toggle::ScriptBit(3, true)), ..base.clone() });
    engine.tick().unwrap();
    engine.tick().unwrap();
    let w = engine.sim_lua().app_data_ref::<EntityWorld>().unwrap();
    for &id in &tanks[..2] {
        let u = w.get(id).unwrap().unit.as_deref().unwrap();
        assert_eq!((u.fire_state, u.paused, u.script_bits & (1 << 3) != 0), (1, true, true), "tank {id}");
    }
    let other = w.get(tanks[2]).unwrap().unit.as_deref().unwrap();
    assert_eq!((other.fire_state, other.paused), (0, false), "unselected tank untouched");
}

/// Repeat build from the command stream (`ProcessInfo('SetRepeatQueue')`):
/// FAF hears `OnStartRepeatQueue`, and a factory with a one-unit order keeps
/// building it (each finished build goes back on its queue).
#[test]
fn repeat_queue_from_the_command_stream() {
    use rts_core_engine::engine::sim::input::{decode, encode, Toggle};
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let fac: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            rawset(_G, 'RepeatCalls', 0)
            fac.OnStartRepeatQueue = function(self) RepeatCalls = RepeatCalls + 1 end
            return tonumber(fac:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let t = tick_now(&engine) + 1;
    let toggle = PlayerCommand { kind: CommandKind::Stop, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: Some(Toggle::RepeatQueue(true)), script: Vec::new(), callback: None, category: None, retarget: None };
    let line = encode(&Scheduled { tick: t, client: 0, seq: 0, command: toggle.clone() });
    assert_eq!(decode(&line).unwrap().command, toggle, "the toggle round-trips through the replay format");
    send(&engine, t, 0, toggle);
    let build = PlayerCommand { kind: CommandKind::BuildFactory, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some("uel0105".into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, t, 1, build);
    let count = |e: &engine::Engine| -> usize { e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().filter(|x| !x.destroyed && x.cold.blueprint_id == "uel0105" && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).count() };
    for _ in 0..2400 {
        engine.tick().unwrap();
        if count(&engine) >= 2 {
            break;
        }
    }
    let calls: u32 = engine.sim_lua().globals().get("RepeatCalls").unwrap();
    assert_eq!(calls, 1, "OnStartRepeatQueue once");
    assert!(count(&engine) >= 2, "a one-unit order on repeat built more than one");
}

/// A script order from the command stream (the enhancement tab's): the ACU
/// builds AdvancedEngineering through FAF's EnhanceTask.
#[test]
fn enhancement_from_the_command_stream() {
    if !have_data() {
        return;
    }
    let (mut engine, _, _) = setup(None);
    let acu: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local acu = CreateUnitHPR('uel0001', 'ARMY_1', 400, GetTerrainHeight(400, 640), 640, 0, 0, 0)
            rawset(_G, 'TestAcu', acu)
            return tonumber(acu:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    for _ in 0..5 {
        engine.tick().unwrap();
    }
    let script = vec![("Enhancement".to_string(), "AdvancedEngineering".to_string()), ("TaskName".to_string(), "EnhanceTask".to_string())];
    let cmd = PlayerCommand { kind: CommandKind::Script, units: vec![acu], target: CommandTarget::None, queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: None, script, callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, cmd);
    let done = |e: &engine::Engine| e.sim_lua().load("return TestAcu:HasEnhancement('AdvancedEngineering')").eval::<bool>().unwrap();
    for _ in 0..1600 {
        engine.tick().unwrap();
        if done(&engine) {
            break;
        }
    }
    assert!(done(&engine), "the ACU finished AdvancedEngineering");
}

/// A move order to a factory sets its rally point (keeping its build
/// queue), and what it builds then heads there.
#[test]
fn move_order_to_a_factory_sets_its_rally_point() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let fac: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            return tonumber(fac:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let t = tick_now(&engine) + 1;
    let build = PlayerCommand { kind: CommandKind::BuildFactory, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some("uel0201".into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, t, 0, build);
    send(&engine, t + 1, 0, PlayerCommand { formation: None, facing: None, ..move_to(1, &[fac], 470.0, 660.0) });
    engine.tick().unwrap();
    engine.tick().unwrap();
    engine.tick().unwrap();
    let queued = engine.sim_lua().app_data_ref::<Commands>().unwrap().queue(fac).map(|c| c.kind).collect::<Vec<_>>();
    assert_eq!(queued, vec![CommandKind::BuildFactory], "the move didn't replace the build");
    let rally: Vec<f64> = engine.sim_lua().load("local p = GetEntityById(...):GetRallyPoint() return p and { p[1], p[3] } or {}").call(fac).unwrap();
    assert_eq!(rally, vec![470.0, 660.0]);
    let tank = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().find(|x| !x.destroyed && x.cold.blueprint_id == "uel0201" && x.army == Some(1) && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE && x.id > fac).map(|x| x.id);
    let mut arrived = false;
    for _ in 0..1500 {
        engine.tick().unwrap();
        if let Some(id) = tank(&engine) {
            let (x, z) = position(&engine, id);
            if (x - 470.0).abs() < 4.0 && (z - 660.0).abs() < 4.0 {
                arrived = true;
                break;
            }
        }
    }
    assert!(arrived, "the new tank went to the rally point");
}

/// A factory's rally orders are a queue of their own.
/// A move then shift-queued patrol points given to a factory all stay (in
/// its factory command queue, not its build queue), a new unit takes every
/// one of them (sharing the factory's commands) and patrols, a move without
/// Shift replaces them, and `IssueClearFactoryCommands` empties them
/// without touching the builds.
#[test]
fn factory_rally_orders_queue_and_pass_to_what_it_builds() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let fac: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            return tonumber(fac:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let t = tick_now(&engine) + 1;
    let patrol = |x: f64, z: f64| PlayerCommand { kind: CommandKind::Patrol, queue: true, ..move_to(1, &[fac], x, z) };
    send(&engine, t, 0, move_to(1, &[fac], 470.0, 640.0));
    send(&engine, t, 1, patrol(490.0, 640.0));
    send(&engine, t, 2, patrol(490.0, 670.0));
    let build = PlayerCommand { kind: CommandKind::BuildFactory, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some("uel0201".into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, t, 3, build);
    engine.tick().unwrap();
    engine.tick().unwrap();
    let rally_orders = |e: &engine::Engine, unit: u32| -> Vec<(u32, CommandKind, (i64, i64))> {
        let c = e.sim_lua().app_data_ref::<Commands>().unwrap();
        c.factory_queue(unit)
            .map(|cmd| match cmd.target {
                CommandTarget::Ground(p) => (cmd.id, cmd.kind, (p.x.to_num::<f64>().round() as i64, p.z.to_num::<f64>().round() as i64)),
                _ => (cmd.id, cmd.kind, (0, 0)),
            })
            .collect()
    };
    let factory_orders = rally_orders(&engine, fac);
    assert_eq!(
        factory_orders.iter().map(|o| (o.1, o.2)).collect::<Vec<_>>(),
        vec![(CommandKind::Move, (470, 640)), (CommandKind::Patrol, (490, 640)), (CommandKind::Patrol, (490, 670))],
        "the move and both shift-queued patrol points are kept, in order"
    );
    {
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        assert_eq!(c.queue(fac).map(|x| x.kind).collect::<Vec<_>>(), vec![CommandKind::BuildFactory], "the build queue is the factory's own");
        assert!(c.is_idle(fac) || c.current(fac).is_some_and(|x| x.kind == CommandKind::BuildFactory));
    }
    let rally: Vec<f64> = engine.sim_lua().load("local p = GetEntityById(...):GetRallyPoint() return p and { p[1], p[3] } or {}").call(fac).unwrap();
    assert_eq!(rally, vec![470.0, 640.0], "GetRallyPoint: the first rally order");

    // The first tank out takes all three, sharing the factory's commands.
    let tank = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().find(|x| !x.destroyed && x.cold.blueprint_id == "uel0201" && x.army == Some(1) && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE && x.id > fac).map(|x| x.id);
    let mut first = None;
    for _ in 0..1500 {
        engine.tick().unwrap();
        first = tank(&engine);
        if first.is_some() {
            break;
        }
    }
    let first = first.expect("a tank was built");
    engine.tick().unwrap();
    let ids: Vec<u32> = factory_orders.iter().map(|o| o.0).collect();
    {
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        let queued: Vec<u32> = c.queue(first).map(|x| x.id).collect();
        assert!(queued.ends_with(&ids), "the new tank's queue ends with the factory's commands: {queued:?} vs {ids:?}");
        for &id in &ids {
            assert!(c.get(id).unwrap().units.contains(&fac) && c.get(id).unwrap().units.contains(&first), "shared by the factory and the tank");
        }
    }
    // It goes on to patrol the two points (round and round).
    let near = |p: (f64, f64), x: f64, z: f64| (p.0 - x).abs() < 4.0 && (p.1 - z).abs() < 4.0;
    let (mut at_a, mut at_b, mut back_at_a) = (false, false, false);
    for _ in 0..3000 {
        engine.tick().unwrap();
        let p = position(&engine, first);
        if near(p, 490.0, 640.0) {
            if at_b {
                back_at_a = true;
                break;
            }
            at_a = true;
        }
        if at_a && near(p, 490.0, 670.0) {
            at_b = true;
        }
    }
    assert!(at_a && at_b && back_at_a, "the tank patrols between the factory's patrol points");
    assert_eq!(rally_orders(&engine, fac).len(), 3, "the factory keeps its rally orders");

    // A move without Shift replaces them; clearing empties them.
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, move_to(1, &[fac], 460.0, 690.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(rally_orders(&engine, fac).iter().map(|o| (o.1, o.2)).collect::<Vec<_>>(), vec![(CommandKind::Move, (460, 690))]);
    {
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        assert!(c.queue(first).any(|x| x.kind == CommandKind::Patrol), "the tank already out keeps its patrol");
    }
    engine.sim_lua().load("local f = GetEntityById(...) IssueBuildFactory({ f }, 'uel0201', 3) IssueClearFactoryCommands({ f })").call::<()>(fac).unwrap();
    assert!(rally_orders(&engine, fac).is_empty());
    let has_build = engine.sim_lua().app_data_ref::<Commands>().unwrap().queue(fac).any(|x| x.kind == CommandKind::BuildFactory);
    assert!(has_build, "IssueClearFactoryCommands leaves the builds");
    let none: bool = engine.sim_lua().load("return GetEntityById(...):GetRallyPoint() == nil").call(fac).unwrap();
    assert!(none, "no rally point once cleared");
}

/// A factory's rally orders reach the command graph, for the
/// factory selected and, with Shift held, unselected; and its details for
/// the interface (`GetCommandQueue`).
#[test]
fn factory_rally_orders_reach_the_command_graph() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let fac: u32 = engine.sim_lua().load("return tonumber(CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0):GetEntityId())").eval().unwrap();
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, move_to(1, &[fac], 470.0, 640.0));
    send(&engine, t, 1, PlayerCommand { kind: CommandKind::Patrol, queue: true, ..move_to(1, &[fac], 490.0, 640.0) });
    engine.tick().unwrap();
    engine.tick().unwrap();
    let (sink, rx, request) = engine::snapshot::channel();
    engine.set_snapshot_sink(sink);
    let snapshot = |engine: &mut engine::Engine| {
        let mut last = None;
        for _ in 0..3 {
            engine.tick().unwrap();
            while let Ok(mut s) = rx.try_recv() {
                drop(s.ticket.take());
                if !s.entities.is_empty() {
                    last = Some(s);
                }
            }
        }
        last.expect("a snapshot")
    };
    let kinds = |s: &engine::snapshot::Snapshot| -> Vec<u8> { s.debug.orders.iter().find(|(u, _)| *u == fac).map(|(_, m)| m.iter().map(|m| m.kind).collect()).unwrap_or_default() };
    let expected = vec![CommandKind::Move as u8, CommandKind::Patrol as u8];
    {
        let mut r = request.lock().unwrap();
        r.view_army = Some(1);
        r.selection = vec![fac];
    }
    let s = snapshot(&mut engine);
    assert_eq!(kinds(&s), expected, "the selected factory's waypoints");
    let detail = s.debug.units.iter().find(|d| d.id == fac).expect("its details");
    assert_eq!(detail.factory_commands.iter().map(|c| c.0).collect::<Vec<_>>(), expected);
    {
        let mut r = request.lock().unwrap();
        r.selection.clear();
        r.all_orders = true;
    }
    let s = snapshot(&mut engine);
    assert_eq!(kinds(&s), expected, "Shift shows an unselected factory's waypoints");
    request.lock().unwrap().all_orders = false;
    let s = snapshot(&mut engine);
    assert!(kinds(&s).is_empty(), "not without Shift");
}

#[test]
fn special_action_calls_the_units_hook() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    engine
        .sim_lua()
        // (The tank has no `RULEUCC_SpecialAction`, without which moho
        // refuses the order: 0x102ca420 case 38.)
        .load("SpecialActions = {} local u = GetEntityById(...) u:AddCommandCap('RULEUCC_SpecialAction') u.OnSpecialAction = function(self, at) table.insert(SpecialActions, at) end")
        .call::<()>(tanks[0])
        .expect("hook");
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, PlayerCommand { kind: CommandKind::SpecialAction, formation: None, facing: None, ..move_to(1, &tanks[..1], 410.0, 620.0) });
    for _ in 0..5 {
        engine.tick().unwrap();
    }
    let calls: LuaTable = engine.sim_lua().globals().get("SpecialActions").unwrap();
    assert_eq!(calls.raw_len(), 1, "OnSpecialAction once");
    let at: LuaTable = calls.raw_get(1).unwrap();
    assert_eq!((at.raw_get::<f64>(1).unwrap(), at.raw_get::<f64>(3).unwrap()), (410.0, 620.0));
    assert!(engine.sim_lua().app_data_ref::<Commands>().unwrap().is_idle(tanks[0]), "the order pops");
}

/// The construction panel's queue edits from the command stream: a factory
/// building five engineers is cut to two (taken from the back), given one
/// more, then cut to none, which cancels the engineer in progress.
#[test]
fn build_counts_change_from_the_command_stream() {
    use rts_core_engine::engine::sim::input::Toggle;
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let fac: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            return tonumber(fac:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let order = |toggle: Option<Toggle>, count| PlayerCommand { kind: if toggle.is_some() { CommandKind::Stop } else { CommandKind::BuildFactory }, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: toggle.is_none().then(|| "uel0105".into()), count, army: 1, toggle, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, order(None, 5));
    let queued = |e: &engine::Engine| -> u32 { e.sim_lua().app_data_ref::<Commands>().unwrap().queue(fac).map(|c| c.count).sum() };
    let building = |e: &engine::Engine| -> usize { e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().filter(|x| !x.destroyed && x.cold.blueprint_id == "uel0105" && x.fraction_complete < rts_core_engine::engine::sim::math::Fx::ONE).count() };
    for _ in 0..20 {
        engine.tick().unwrap();
    }
    assert_eq!((queued(&engine), building(&engine)), (5, 1), "building the first of five");
    send(&engine, tick_now(&engine) + 1, 0, order(Some(Toggle::BuildCount(1, -3)), 0));
    send(&engine, tick_now(&engine) + 1, 1, order(Some(Toggle::BuildCount(1, 1)), 0));
    for _ in 0..3 {
        engine.tick().unwrap();
    }
    assert_eq!((queued(&engine), building(&engine)), (3, 1), "five less three plus one, still building");
    send(&engine, tick_now(&engine) + 1, 0, order(Some(Toggle::BuildCount(1, -3)), 0));
    for _ in 0..5 {
        engine.tick().unwrap();
    }
    assert_eq!((queued(&engine), building(&engine)), (0, 0), "all gone, the engineer in progress with them");
}

/// A guard order whose units include the guarded unit (a selection of
/// engineers right-clicked onto one of themselves) skips that unit and
/// leaves its queue alone, as moho's `UNIT_IssueCommand` (0x102cbd20) does:
/// its per-unit check (0x102ca420) fails a guard of the unit itself before
/// the queue is cleared. The others assist its build.
#[test]
fn a_guard_given_to_the_guarded_unit_leaves_its_orders() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:GiveStorage('MASS', 50000) b:GiveStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local ids = {}
            for _, x in { 390, 396, 402 } do
                table.insert(ids, tonumber(CreateUnitHPR('uel0105', 'ARMY_1', x, GetTerrainHeight(x, 600), 600, 0, 0, 0):GetEntityId()))
            end
            IssueBuildMobile({ GetUnitById(tostring(ids[2])) }, { 396.5, GetTerrainHeight(396.5, 612.5), 612.5 }, 'ueb1101', {})
            return ids
            "#,
        )
        .eval()
        .unwrap();
    let ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let builder = ids[1];
    let focus = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).and_then(|x| x.unit.as_deref()?.focus);
    let current = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| c.kind);
    for _ in 0..100 {
        engine.tick().unwrap();
        if focus(&engine, builder).is_some() {
            break;
        }
    }
    let structure = focus(&engine, builder).expect("the engineer started its power generator");
    let guard = PlayerCommand { kind: CommandKind::Guard, units: ids.clone(), target: CommandTarget::Entity(builder), queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, guard);
    for _ in 0..200 {
        engine.tick().unwrap();
        if [ids[0], ids[2]].iter().all(|&e| focus(&engine, e).is_some()) {
            break;
        }
    }
    assert_eq!(current(&engine, builder), Some(CommandKind::BuildMobile), "the guarded engineer keeps its build");
    assert_eq!(focus(&engine, builder), Some(structure));
    for e in [ids[0], ids[2]] {
        assert_eq!(current(&engine, e), Some(CommandKind::Guard), "engineer {e} assists");
        assert_eq!(focus(&engine, e), Some(structure), "engineer {e} helps build it");
    }
}

/// One order for a mixed selection: moho sends the whole
/// selection the one order and its per-unit check (MohoEngine.dll
/// 0x102ca420) refuses it to the units that can't take it, which keep
/// their queues: tanks (not `REPAIR`) told to repair keep moving, an attack
/// on an own unit is refused, an engineer (mobile) takes an attack, and
/// the same move given again to the same units leaves their order be.
#[test]
fn units_refused_an_order_keep_their_queue() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, enemy) = setup(None);
    let engineer: u32 = engine
        .sim_lua()
        .load(format!(
            "local d = GetEntityById({}) d:SetHealth(d, d:GetMaxHealth() / 2)
             return tonumber(CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 590), 590, 0, 0, 0):GetEntityId())",
            tanks[5]
        ))
        .eval()
        .unwrap();
    let order = |kind: CommandKind, units: Vec<u32>, target: CommandTarget| PlayerCommand { kind, units, target, ..move_to(1, &[], 0.0, 0.0) };
    let current = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| (c.id, c.kind, c.target));
    send(&engine, tick_now(&engine) + 1, 0, move_to(1, &tanks[..2], 420.0, 640.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    let moving = current(&engine, tanks[0]).expect("moving");
    // Repair the damaged tank: the engineer takes it, the tanks keep moving.
    send(&engine, tick_now(&engine) + 1, 0, order(CommandKind::Repair, vec![tanks[0], tanks[1], engineer], CommandTarget::Entity(tanks[5])));
    // An attack on an own tank: refused.
    send(&engine, tick_now(&engine) + 1, 1, order(CommandKind::Attack, vec![tanks[2]], CommandTarget::Entity(tanks[5])));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, tanks[0]), Some(moving), "a tank keeps its move");
    assert_eq!(current(&engine, engineer).map(|c| c.1), Some(CommandKind::Repair), "the engineer repairs");
    assert_eq!(current(&engine, tanks[2]), None, "no attack on an own unit");
    // An engineer can be given an attack (it can move).
    send(&engine, tick_now(&engine) + 1, 0, order(CommandKind::Attack, vec![engineer, tanks[3]], CommandTarget::Entity(enemy)));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, engineer).map(|c| c.1), Some(CommandKind::Attack), "the engineer takes the attack");
    // The same move again: the tanks' order isn't replaced.
    send(&engine, tick_now(&engine) + 1, 0, move_to(1, &tanks[..2], 420.0, 640.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, tanks[0]), Some(moving), "the same move leaves the order be");
    // To another point it's a new order.
    send(&engine, tick_now(&engine) + 1, 0, move_to(1, &tanks[..2], 430.0, 640.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_ne!(current(&engine, tanks[0]).map(|c| c.0), Some(moving.0), "a new move replaces it");
}

/// An order on a point outside the playable area is refused (moho's
/// per-unit check, MohoEngine.dll 0x102ca6f6: `FUN_102167b0` on the
/// playable rect, edges included; an army that ignores it may go anywhere
/// on the map): player and script orders alike, and the unit keeps its
/// orders.
#[test]
fn orders_outside_the_playable_area_are_refused() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    engine.sim_lua().load("SetPlayableRect(300, 500, 420, 620)").exec().unwrap();
    let current = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| (c.kind, c.target));
    send(&engine, tick_now(&engine) + 1, 0, move_to(1, &tanks[..1], 410.0, 600.0));
    send(&engine, tick_now(&engine) + 1, 1, move_to(1, &tanks[1..2], 450.0, 600.0));
    send(&engine, tick_now(&engine) + 1, 2, move_to(1, &tanks[2..3], 420.0, 620.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, tanks[0]), Some((CommandKind::Move, ground(410.0, 600.0))), "inside");
    assert_eq!(current(&engine, tanks[1]), None, "outside: refused");
    assert_eq!(current(&engine, tanks[2]), Some((CommandKind::Move, ground(420.0, 620.0))), "on the edge");
    // The tank inside keeps its move when told to go outside.
    send(&engine, tick_now(&engine) + 1, 0, move_to(1, &tanks[..1], 450.0, 600.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, tanks[0]), Some((CommandKind::Move, ground(410.0, 600.0))), "keeps its move");
    // A script order too.
    engine.sim_lua().load(format!("IssueMove({{ GetEntityById({}) }}, {{ 450, 0, 600 }})", tanks[3])).exec().unwrap();
    assert_eq!(current(&engine, tanks[3]), None, "a script move outside: refused");
    // An army that ignores the playable area goes anywhere on the map.
    engine.sim_lua().load("SetIgnorePlayableRect(1, true)").exec().unwrap();
    send(&engine, tick_now(&engine) + 1, 0, move_to(1, &tanks[1..2], 450.0, 600.0));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, tanks[1]), Some((CommandKind::Move, ground(450.0, 600.0))), "ignoring the playable area");
}

/// A load onto a transport that can't carry the unit is refused (moho's
/// per-unit check asks the transport's `TransportCanCarryUnit`,
/// MohoEngine.dll 0x101d3140; ours the shared `sim::carry` rule): an
/// aircraft onto a T1 transport, a commander onto one that isn't
/// `CANTRANSPORTCOMMANDER`, a load onto a unit that carries nothing; a tank
/// onto the T1 transport and the commander onto the T2 one are taken, and
/// a transport ordered onto an aircraft is refused.
#[test]
fn loads_a_transport_cant_carry_are_refused() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local function spawn(bp, x, z) return tonumber(CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0):GetEntityId()) end
            return { spawn('uea0107', 410, 600), spawn('uea0104', 420, 600), spawn('uea0101', 400, 610), spawn('uel0001', 405, 590) }
            "#,
        )
        .eval()
        .unwrap();
    let ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let (t1, t2, scout, acu) = (ids[0], ids[1], ids[2], ids[3]);
    let order = |kind: CommandKind, units: Vec<u32>, target: u32| PlayerCommand { kind, units, target: CommandTarget::Entity(target), ..move_to(1, &[], 0.0, 0.0) };
    let current = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| (c.kind, c.target));
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, order(CommandKind::TransportLoadUnits, vec![tanks[0], scout, acu], t1));
    send(&engine, t, 1, order(CommandKind::TransportLoadUnits, vec![tanks[1]], tanks[2]));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, tanks[0]), Some((CommandKind::TransportLoadUnits, CommandTarget::Entity(t1))), "a tank boards the T1 transport");
    assert_eq!(current(&engine, scout), None, "an aircraft doesn't");
    assert_eq!(current(&engine, acu), None, "nor a commander");
    assert_eq!(current(&engine, tanks[1]), None, "a tank carries nothing");
    send(&engine, tick_now(&engine) + 1, 0, order(CommandKind::TransportLoadUnits, vec![acu], t2));
    send(&engine, tick_now(&engine) + 1, 1, order(CommandKind::TransportReverseLoadUnits, vec![t1], scout));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, acu), Some((CommandKind::TransportLoadUnits, CommandTarget::Entity(t2))), "the commander boards the T2 transport");
    assert_eq!(current(&engine, t1), None, "the transport won't pick up an aircraft");
    // A ferry route: only for a transport (0x102ca420 case 17).
    engine.sim_lua().load(format!("IssueFerry({{ GetEntityById({}), GetEntityById({}) }}, {{ 380, 0, 560 }})", tanks[3], t1)).exec().unwrap();
    assert_eq!(current(&engine, tanks[3]), None, "a tank takes no ferry route");
    assert_eq!(current(&engine, t1).map(|c| c.0), Some(CommandKind::Ferry), "the transport does");
}

/// moho's per-unit order check (MohoEngine.dll 0x102ca420) also refuses a
/// guard of a unit that guards the unit, and the check runs for script
/// orders too (`IssueGuard` goes through `UNIT_IssueCommand`): a refused
/// unit keeps its orders.
#[test]
fn a_guard_of_a_guard_or_of_itself_is_refused() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let (a, b) = (tanks[0], tanks[1]);
    let guard = |units: Vec<u32>, target: u32| PlayerCommand { kind: CommandKind::Guard, units, target: CommandTarget::Entity(target), queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    // b moves off; a guards b.
    send(&engine, tick_now(&engine) + 1, 0, move_to(1, &[b], 420.0, 640.0));
    send(&engine, tick_now(&engine) + 1, 1, guard(vec![a], b));
    engine.tick().unwrap();
    engine.tick().unwrap();
    let current = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| (c.kind, c.target));
    assert_eq!(current(&engine, a), Some((CommandKind::Guard, CommandTarget::Entity(b))));
    // b told to guard a, which guards it: refused, b keeps moving.
    send(&engine, tick_now(&engine) + 1, 0, guard(vec![b], a));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, b), Some((CommandKind::Move, ground(420.0, 640.0))), "b keeps its move");
    // The same from a script, and a script guard of the unit itself.
    let lua = engine.sim_lua();
    lua.load(format!("local a, b = GetUnitById('{a}'), GetUnitById('{b}') IssueGuard({{ b }}, a) IssueGuard({{ b }}, b) IssueRepair({{ b }}, b)")).exec().unwrap();
    assert_eq!(lua.app_data_ref::<Commands>().unwrap().queue(b).count(), 1, "nothing queued behind b's move");
}

/// Engineers assisting a commander help with what it assists: the
/// ACU assists an extractor's upgrade, and the engineers guarding the ACU
/// work on the upgrade too.
#[test]
fn engineers_assisting_the_acu_help_what_it_assists() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local mex = CreateUnitHPR('ueb1103', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            local acu = CreateUnitHPR('uel0001', 'ARMY_1', 446, GetTerrainHeight(446, 640), 640, 0, 0, 0)
            local e1 = CreateUnitHPR('uel0105', 'ARMY_1', 452, GetTerrainHeight(452, 636), 636, 0, 0, 0)
            local e2 = CreateUnitHPR('uel0105', 'ARMY_1', 452, GetTerrainHeight(452, 644), 644, 0, 0, 0)
            IssueUpgrade({ mex }, 'ueb1202')
            return { tonumber(mex:GetEntityId()), tonumber(acu:GetEntityId()), tonumber(e1:GetEntityId()), tonumber(e2:GetEntityId()) }
            "#,
        )
        .eval()
        .unwrap();
    let ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let (mex, acu, engineers) = (ids[0], ids[1], &ids[2..]);
    let guard = |units: Vec<u32>, target: u32| PlayerCommand { kind: CommandKind::Guard, units, target: CommandTarget::Entity(target), queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    // The engineers assist the ACU first, then the ACU assists the upgrade.
    send(&engine, tick_now(&engine) + 1, 0, guard(engineers.to_vec(), acu));
    for _ in 0..5 {
        engine.tick().unwrap();
    }
    send(&engine, tick_now(&engine) + 1, 0, guard(vec![acu], mex));
    let focus = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).and_then(|x| x.unit.as_deref()?.focus);
    let upgrade = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().find(|x| !x.destroyed && x.cold.blueprint_id == "ueb1202").map(|x| x.id);
    for _ in 0..200 {
        engine.tick().unwrap();
        if engineers.iter().all(|&e| focus(&engine, e).is_some() && focus(&engine, e) == upgrade(&engine)) {
            break;
        }
    }
    let up = upgrade(&engine).expect("upgrading");
    assert_eq!(focus(&engine, acu), Some(up), "the ACU assists the upgrade");
    for &e in engineers {
        assert_eq!(focus(&engine, e), Some(up), "engineer {e} helps the upgrade through the ACU");
    }
}

/// Upgrades queued from the construction panel: a T1 extractor
/// upgrading to T2 is clicked for T3, and FAF's panel sends the chain T2,
/// T3 queued behind the upgrade in progress. The T2 extractor carries on
/// to T3.
#[test]
fn queued_upgrades_carry_on_to_the_next_tier() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let mex: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local mex = CreateUnitHPR('ueb1103', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            return tonumber(mex:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let upgrade = |bp: &str| PlayerCommand { kind: CommandKind::Upgrade, units: vec![mex], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some(bp.into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, upgrade("ueb1202"));
    for _ in 0..20 {
        engine.tick().unwrap();
    }
    send(&engine, tick_now(&engine) + 1, 0, upgrade("ueb1202"));
    send(&engine, tick_now(&engine) + 1, 1, upgrade("ueb1302"));
    engine.tick().unwrap();
    engine.tick().unwrap();
    let queue: Vec<String> = engine.sim_lua().app_data_ref::<Commands>().unwrap().queue(mex).filter_map(|c| c.blueprint.clone()).collect();
    assert_eq!(queue, ["ueb1202", "ueb1302"], "T3 queued behind the upgrade in progress, T2 not twice");
    let finished = |e: &engine::Engine, bp: &str| -> usize { e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().filter(|x| !x.destroyed && x.cold.blueprint_id == bp && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).count() };
    for _ in 0..6000 {
        engine.tick().unwrap();
        if finished(&engine, "ueb1302") == 1 {
            break;
        }
    }
    assert_eq!((finished(&engine, "ueb1103"), finished(&engine, "ueb1202"), finished(&engine, "ueb1302")), (0, 0, 1), "upgraded through to T3");
}

/// Next-tier units queued on an upgrading factory: a T1 land factory
/// upgrading to T2 is given a T2 tank; the T2 factory builds it.
#[test]
fn units_queued_behind_a_factory_upgrade_are_built_by_the_new_factory() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let fac: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            return tonumber(fac:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let order = |kind, bp: &str| PlayerCommand { kind, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some(bp.into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, order(CommandKind::Upgrade, "ueb0201"));
    send(&engine, tick_now(&engine) + 1, 1, order(CommandKind::BuildFactory, "uel0202"));
    let finished = |e: &engine::Engine, bp: &str| -> usize { e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().filter(|x| !x.destroyed && x.cold.blueprint_id == bp && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).count() };
    for _ in 0..8000 {
        engine.tick().unwrap();
        if finished(&engine, "uel0202") == 1 {
            break;
        }
    }
    assert_eq!((finished(&engine, "ueb0101"), finished(&engine, "ueb0201"), finished(&engine, "uel0202")), (0, 1, 1), "upgraded, then the T2 factory built the tank");
}

/// An upgraded factory keeps its rally orders: the new factory takes
/// the old one's factory command queue (the same, shared commands, as
/// moho's `NotifyUpgrade`), so its `GetRallyPoint`, its queue for the
/// interface (`GetCommandQueue`) and the units it then builds follow them,
/// and a unit built before the upgrade keeps patrolling.
#[test]
fn upgraded_factory_keeps_its_rally_orders() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let fac: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            return tonumber(CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0):GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let t = tick_now(&engine) + 1;
    let order = |kind, bp: &str| PlayerCommand { kind, units: vec![fac], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some(bp.into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    let patrol = |x: f64, z: f64| PlayerCommand { kind: CommandKind::Patrol, queue: true, ..move_to(1, &[fac], x, z) };
    send(&engine, t, 0, move_to(1, &[fac], 470.0, 640.0));
    send(&engine, t, 1, patrol(490.0, 640.0));
    send(&engine, t, 2, patrol(490.0, 670.0));
    send(&engine, t, 3, order(CommandKind::BuildFactory, "uel0201"));
    send(&engine, t, 4, order(CommandKind::Upgrade, "ueb0201"));
    send(&engine, t, 5, order(CommandKind::BuildFactory, "uel0202"));
    engine.tick().unwrap();
    engine.tick().unwrap();
    let rally_ids = |e: &engine::Engine, unit: u32| -> Vec<u32> { e.sim_lua().app_data_ref::<Commands>().unwrap().factory_queue(unit).map(|c| c.id).collect() };
    let ids = rally_ids(&engine, fac);
    assert_eq!(ids.len(), 3);
    // (Made after the factory: `setup`'s own tanks are older.)
    let built = |e: &engine::Engine, bp: &str| -> Option<u32> { e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().find(|x| !x.destroyed && x.id >= fac && x.cold.blueprint_id == bp && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).map(|x| x.id) };
    let mut t2_factory = None;
    for _ in 0..8000 {
        engine.tick().unwrap();
        if built(&engine, "ueb0101").is_none() {
            t2_factory = built(&engine, "ueb0201");
            if t2_factory.is_some() {
                break;
            }
        }
    }
    let t2_factory = t2_factory.expect("upgraded");
    let t1_tank = built(&engine, "uel0201").expect("the tank built before the upgrade");
    assert_eq!(rally_ids(&engine, t2_factory), ids, "the upgraded factory has the same rally orders");
    let rally: Vec<f64> = engine.sim_lua().load("local p = GetEntityById(...):GetRallyPoint() return p and { p[1], p[3] } or {}").call(t2_factory).unwrap();
    assert_eq!(rally, vec![470.0, 640.0]);
    {
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        let tank_queue: Vec<(u32, CommandKind)> = c.queue(t1_tank).map(|x| (x.id, x.kind)).collect();
        assert!(tank_queue.iter().any(|&(id, kind)| ids.contains(&id) && kind == CommandKind::Patrol), "the earlier tank still patrols on the shared orders: {tank_queue:?} vs {ids:?}");
    }
    // Its queue for the interface (a FACTORY's `GetCommandQueue`).
    let (sink, rx, request) = engine::snapshot::channel();
    engine.set_snapshot_sink(sink);
    {
        let mut r = request.lock().unwrap();
        r.view_army = Some(1);
        r.selection = vec![t2_factory];
    }
    let mut detail = None;
    for _ in 0..3 {
        engine.tick().unwrap();
        while let Ok(mut s) = rx.try_recv() {
            drop(s.ticket.take());
            if let Some(d) = s.debug.units.iter().find(|d| d.id == t2_factory) {
                detail = Some(d.factory_commands.iter().map(|c| c.0).collect::<Vec<_>>());
            }
        }
    }
    assert_eq!(detail, Some(vec![CommandKind::Move as u8, CommandKind::Patrol as u8, CommandKind::Patrol as u8]));
    // The T2 tank it builds next takes them.
    let mut t2_tank = None;
    for _ in 0..4000 {
        engine.tick().unwrap();
        t2_tank = built(&engine, "uel0202");
        if t2_tank.is_some() {
            break;
        }
    }
    let t2_tank = t2_tank.expect("the T2 tank");
    engine.tick().unwrap();
    let queued: Vec<u32> = engine.sim_lua().app_data_ref::<Commands>().unwrap().queue(t2_tank).map(|c| c.id).collect();
    assert!(queued.ends_with(&ids), "the new tank follows the rally orders: {queued:?} vs {ids:?}");
}

/// Engineers sharing a build keep no progress once it's done: the
/// first to finish the structure stops the others, whose progress bars
/// (`GetWorkProgress`) must go with it.
#[test]
fn shared_build_leaves_no_progress_behind() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local ids = {}
            for i = 0, 2 do
                local z = 616 + i * 4
                table.insert(ids, tonumber(CreateUnitHPR('uel0105', 'ARMY_1', 420, GetTerrainHeight(420, z), z, 0, 0, 0):GetEntityId()))
            end
            return ids
            "#,
        )
        .eval()
        .unwrap();
    let engineers: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let build = PlayerCommand { kind: CommandKind::BuildMobile, units: engineers.clone(), target: ground(426.5, 620.5), queue: false, formation: None, facing: None, blueprint: Some("ueb1101".into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, build);
    let finished = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().any(|x| !x.destroyed && x.cold.blueprint_id == "ueb1101" && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE);
    let progress = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).and_then(|x| x.unit.as_deref().map(|u| u.work_progress.to_num::<f64>())).unwrap();
    let mut shared = false;
    for _ in 0..1200 {
        engine.tick().unwrap();
        shared |= engineers.iter().filter(|&&e| progress(&engine, e) > 0.0).count() > 1;
        if finished(&engine) {
            break;
        }
    }
    assert!(finished(&engine), "built");
    assert!(shared, "more than one engineer worked on it");
    engine.tick().unwrap();
    for &e in &engineers {
        assert_eq!(progress(&engine, e), 0.0, "engineer {e} keeps no progress");
    }
}

/// An engineer assisting (Repair) a structure another engineer is building
/// shows the structure's progress as its own: moho's build task
/// helper writes the target's fraction complete into every helper's work
/// lane, not only the builder's.
#[test]
fn assisting_engineer_shows_the_targets_progress() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local ids = {}
            for i = 0, 1 do
                local z = 616 + i * 4
                table.insert(ids, tonumber(CreateUnitHPR('uel0105', 'ARMY_1', 420, GetTerrainHeight(420, z), z, 0, 0, 0):GetEntityId()))
            end
            return ids
            "#,
        )
        .eval()
        .unwrap();
    let (builder, helper): (u32, u32) = (ids.raw_get(1).unwrap(), ids.raw_get(2).unwrap());
    let build = PlayerCommand { kind: CommandKind::BuildMobile, units: vec![builder], target: ground(426.5, 620.5), queue: false, formation: None, facing: None, blueprint: Some("ueb1101".into()), count: 1, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, build);
    let structure = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().find(|x| !x.destroyed && x.cold.blueprint_id == "ueb1101").map(|x| (x.id, x.fraction_complete.to_num::<f64>()));
    let progress = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).and_then(|x| x.unit.as_deref().map(|u| u.work_progress.to_num::<f64>())).unwrap();
    let mut target = None;
    for _ in 0..600 {
        engine.tick().unwrap();
        if let Some((id, _)) = structure(&engine).filter(|s| s.1 > 0.0) {
            target = Some(id);
            break;
        }
    }
    let target = target.expect("construction started");
    let repair = PlayerCommand { kind: CommandKind::Repair, units: vec![helper], target: CommandTarget::Entity(target), queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 1, repair);
    let mut tracked = 0;
    for _ in 0..1200 {
        engine.tick().unwrap();
        let Some((_, fraction)) = structure(&engine) else { break };
        if fraction >= 1.0 {
            break;
        }
        let helping = engine.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(helper).and_then(|x| x.unit.as_deref().map(|u| u.focus)).flatten() == Some(target);
        if helping {
            let p = progress(&engine, helper);
            assert!(p > 0.0 && (p - fraction).abs() < 0.05, "helper's progress {p} tracks the structure's {fraction}");
            tracked += 1;
        }
    }
    assert!(tracked > 5, "the helper assisted for a while ({tracked} ticks)");
    engine.tick().unwrap();
    assert_eq!(progress(&engine, helper), 0.0, "the helper keeps no progress once it is built");
}

/// An engineer assisting a missile silo while it builds a missile helps
/// build it and shows the silo's progress as its own (moho's build
/// task helper copies the silo's work progress for a silo assist).
#[test]
fn engineer_assisting_a_silo_shows_the_missiles_progress() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local silo = CreateUnitHPR('ueb2108', 'ARMY_1', 426, GetTerrainHeight(426, 620), 620, 0, 0, 0)
            local alone = CreateUnitHPR('ueb2108', 'ARMY_1', 446, GetTerrainHeight(446, 640), 640, 0, 0, 0)
            local eng = CreateUnitHPR('uel0105', 'ARMY_1', 421, GetTerrainHeight(421, 620), 620, 0, 0, 0)
            IssueSiloBuildTactical({silo, alone}, 1)
            IssueGuard({eng}, silo)
            return { tonumber(silo:GetEntityId()), tonumber(alone:GetEntityId()), tonumber(eng:GetEntityId()) }
            "#,
        )
        .eval()
        .unwrap();
    let (silo, alone, eng): (u32, u32, u32) = (ids.raw_get(1).unwrap(), ids.raw_get(2).unwrap(), ids.raw_get(3).unwrap());
    let unit = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).and_then(|x| x.unit.as_deref().map(|u| (u.work_progress.to_num::<f64>(), u.tactical_ammo, u.focus))).unwrap();
    let mut tracked = 0;
    for _ in 0..1200 {
        engine.tick().unwrap();
        let (silo_progress, ammo, _) = unit(&engine, silo);
        if ammo > 0 {
            break;
        }
        let (progress, _, focus) = unit(&engine, eng);
        if focus == Some(silo) && silo_progress > 0.0 {
            assert_eq!(progress, silo_progress, "the engineer shows the silo's progress");
            tracked += 1;
        }
    }
    assert_eq!(unit(&engine, silo).1, 1, "the missile was built");
    assert!(tracked > 5, "the engineer assisted for a while ({tracked} ticks)");
    assert_eq!(unit(&engine, alone).1, 0, "the engineer's help built it sooner than a silo alone");
    engine.tick().unwrap();
    assert_eq!(unit(&engine, eng).0, 0.0, "the engineer keeps no progress once the missile is built");
}

/// A Kennel's drone assists and repairs by itself: it repairs a
/// damaged structure in the Kennel's radius, then flies home and goes idle.
#[test]
fn kennel_drone_repairs_what_is_near_and_comes_home() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local kennel = CreateUnitHPR('xeb0104', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            local hurt = CreateUnitHPR('ueb1101', 'ARMY_1', 448, GetTerrainHeight(448, 640), 640, 0, 0, 0)
            hurt:AdjustHealth(nil, -400)
            return { tonumber(kennel:GetEntityId()), tonumber(hurt:GetEntityId()) }
            "#,
        )
        .eval()
        .unwrap();
    let (kennel, hurt): (u32, u32) = (ids.raw_get(1).unwrap(), ids.raw_get(2).unwrap());
    let drone = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().find(|x| !x.destroyed && x.cold.blueprint_id == "xea3204").map(|x| x.id);
    let health = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).map(|x| (x.health, x.max_health)).unwrap();
    let mut repaired_by_drone = false;
    for _ in 0..600 {
        engine.tick().unwrap();
        let (h, max) = health(&engine, hurt);
        repaired_by_drone |= h >= max && drone(&engine).is_some();
        let home = drone(&engine).is_some_and(|d| {
            let (dp, kp) = (position(&engine, d), position(&engine, kennel));
            engine.sim_lua().app_data_ref::<Commands>().unwrap().is_idle(d) && (dp.0 - kp.0).hypot(dp.1 - kp.1) < 2.0
        });
        if repaired_by_drone && home {
            break;
        }
    }
    let (h, max) = health(&engine, hurt);
    assert!(h >= max, "the drone repaired it: {h:?} / {max:?}");
    let d = drone(&engine).expect("the Kennel made its drone");
    assert!(engine.sim_lua().app_data_ref::<Commands>().unwrap().is_idle(d), "then went idle");
    let (dp, kp) = (position(&engine, d), position(&engine, kennel));
    assert!((dp.0 - kp.0).hypot(dp.1 - kp.1) < 2.0, "back over the Kennel: {dp:?} vs {kp:?}");
}

/// A passenger ordered out of its transport (`DetachFromTransport`, as the
/// interface sends it) bails mid-flight: set down on the ground, detached,
/// its order done.
#[test]
fn detach_from_transport_bails_the_passenger_out() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let transport: u32 = engine
        .sim_lua()
        .load(format!(
            "DetachTransport = CreateUnitHPR('uea0107', 'ARMY_1', 400, 40, 600, 0, 0, 0) \
             DetachTank = GetUnitById('{t0}') \
             IssueTransportLoad({{ DetachTank }}, DetachTransport) \
             return tonumber(DetachTransport:GetEntityId())",
            t0 = tanks[0],
        ))
        .eval()
        .expect("transport");
    let attached = |e: &engine::Engine| {
        e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(tanks[0]).is_some_and(|x| x.attachment.is_some_and(|a| a.parent == transport))
    };
    for _ in 0..200 {
        engine.tick().unwrap();
        if attached(&engine) {
            break;
        }
    }
    assert!(attached(&engine), "the tank boarded first");
    // Fly off, then order the passenger out mid-flight.
    send(&engine, tick_now(&engine) + 1, 0, PlayerCommand { kind: CommandKind::Move, target: ground(460.0, 600.0), ..move_to(1, &[transport], 0.0, 0.0) });
    for _ in 0..20 {
        engine.tick().unwrap();
    }
    let detach = PlayerCommand { kind: CommandKind::DetachFromTransport, target: CommandTarget::None, ..move_to(1, &[tanks[0]], 0.0, 0.0) };
    send(&engine, tick_now(&engine), 1, detach);
    for _ in 0..30 {
        engine.tick().unwrap();
    }
    assert!(!attached(&engine), "the tank left the transport");
    assert!(engine.sim_lua().app_data_ref::<Commands>().unwrap().is_idle(tanks[0]), "its detach order is done");
    let (x, y, z) = engine.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(tanks[0]).unwrap().position().to_f64();
    let _ = (x, z);
    assert!(y < 40.0, "set down from the air, not left hanging: y={y}");
}

fn callback(func: &str, args: LuaData, units: &[u32]) -> PlayerCommand {
    PlayerCommand {
        kind: CommandKind::Stop,
        target: CommandTarget::None,
        formation: None,
        facing: None,
        callback: Some(SimCallback { func: func.into(), args, with_selection: !units.is_empty() }),
        ..move_to(1, units, 0.0, 0.0)
    }
}

/// `SimCallback` runs FAF's `DoCallback(Func, Args, units)` as its sender's
/// command source: the args arrive exactly, the units as sim units (nil
/// without a selection), and FAF's `SecureUnits` only lets a player touch
/// their own army's units (cheats off).
#[test]
fn sim_callbacks_run_as_their_command_source() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, enemy) = setup(None);
    engine.sim_lua().set_app_data(rts_core_engine::engine::api::command_source::Cheats { enabled: false, cheaters: Vec::new() });
    engine
        .sim_lua()
        .load(
            r#"
            SeenCallbacks = {}
            local module = import('/lua/SimCallbacks.lua')
            RealDoCallback = module.DoCallback
            module.DoCallback = function(name, data, units)
                table.insert(SeenCallbacks, { name = name, data = data, units = units and table.getn(units) or 'nil',
                    source = GetCurrentCommandSource(), own = OkayToMessWithArmy(1), other = OkayToMessWithArmy(2) })
            end
            "#,
        )
        .exec()
        .unwrap();
    let t = tick_now(&engine) + 1;
    let args = LuaData::Table(vec![(LuaData::number(1.0), LuaData::number(0.1)), (LuaData::string("Name"), LuaData::string("ping"))]);
    send(&engine, t, 0, callback("EngineTest", args, &[tanks[0], tanks[1], enemy]));
    rts_core_engine::engine::api::player_input::schedule(engine.sim_lua(), [Scheduled { tick: t, client: 1, seq: 0, command: callback("EngineTest", LuaData::Nil, &[]) }]);
    engine.tick().unwrap();
    engine.tick().unwrap();
    let seen: LuaTable = engine.sim_lua().globals().get("SeenCallbacks").unwrap();
    assert_eq!(seen.raw_len(), 2);
    let first: LuaTable = seen.get(1).unwrap();
    let data: LuaTable = first.get("data").unwrap();
    assert_eq!(data.get::<f64>(1).unwrap().to_bits(), 0.1f64.to_bits());
    assert_eq!(data.get::<String>("Name").unwrap(), "ping");
    assert_eq!(first.get::<String>("name").unwrap(), "EngineTest");
    // Every live unit of the selection, whatever its army.
    assert_eq!(first.get::<i64>("units").unwrap(), 3);
    assert_eq!(first.get::<i64>("source").unwrap(), 1);
    assert!(first.get::<bool>("own").unwrap() && !first.get::<bool>("other").unwrap());
    let second: LuaTable = seen.get(2).unwrap();
    assert_eq!(second.get::<String>("units").unwrap(), "nil");
    assert_eq!(second.get::<LuaValue>("data").unwrap(), LuaValue::Nil);
    assert_eq!(second.get::<i64>("source").unwrap(), 2);
    assert!(!second.get::<bool>("own").unwrap() && second.get::<bool>("other").unwrap());

    // FAF's ClearCommands: army 2's player can't clear army 1's tanks; army 1's can.
    engine.sim_lua().load("import('/lua/SimCallbacks.lua').DoCallback = RealDoCallback").exec().unwrap();
    send(&engine, tick_now(&engine), 1, move_to(1, &tanks, 420.0, 640.0));
    engine.tick().unwrap();
    let clear = callback("ClearCommands", LuaData::Table(Vec::new()), &tanks);
    rts_core_engine::engine::api::player_input::schedule(engine.sim_lua(), [Scheduled { tick: tick_now(&engine), client: 1, seq: 1, command: clear.clone() }]);
    engine.tick().unwrap();
    assert!(!engine.sim_lua().app_data_ref::<Commands>().unwrap().is_idle(tanks[0]), "another player's callback must not touch army 1");
    send(&engine, tick_now(&engine), 2, clear);
    engine.tick().unwrap();
    let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
    assert!(tanks.iter().all(|&id| c.is_idle(id)));
}

#[test]
fn paused_upgrade_assisted_by_engineer_advances_progress_and_completes() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: mlua::Table = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local mex = CreateUnitHPR('ueb1103', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
            local eng = CreateUnitHPR('uel0105', 'ARMY_1', 443, GetTerrainHeight(443, 640), 640, 0, 0, 0)
            IssueUpgrade({ mex }, 'ueb1202')
            return { tonumber(mex:GetEntityId()), tonumber(eng:GetEntityId()) }
            "#,
        )
        .eval()
        .unwrap();
    let (mex, eng): (u32, u32) = (ids.raw_get(1).unwrap(), ids.raw_get(2).unwrap());
    engine.tick().unwrap();
    let pause = PlayerCommand { kind: CommandKind::Pause, units: vec![mex], target: CommandTarget::None, queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, category: None, toggle: Some(rts_core_engine::engine::sim::input::Toggle::Paused(true)), script: Vec::new(), callback: None, retarget: None };
    let guard = PlayerCommand { kind: CommandKind::Guard, units: vec![eng], target: CommandTarget::Entity(mex), queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, category: None, toggle: None, script: Vec::new(), callback: None, retarget: None };
    send(&engine, tick_now(&engine) + 1, 0, pause);
    send(&engine, tick_now(&engine) + 1, 1, guard);
    let progress = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).and_then(|x| x.unit.as_deref().map(|u| u.work_progress.to_num::<f64>()));
    let paused = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(id).and_then(|x| x.unit.as_deref().map(|u| u.paused)).unwrap_or(false);
    let finished = |e: &engine::Engine, bp: &str| -> usize { e.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().filter(|x| !x.destroyed && x.cold.blueprint_id == bp && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).count() };
    for _ in 0..5 {
        engine.tick().unwrap();
    }
    assert!(paused(&engine, mex), "mex is paused");
    let initial_progress = progress(&engine, mex).expect("mex exists");
    let mut saw_progress_advance = false;
    for _ in 0..4000 {
        engine.tick().unwrap();
        if let Some(p) = progress(&engine, mex)
            && p > initial_progress && p < 1.0 {
            saw_progress_advance = true;
            assert!(paused(&engine, mex), "mex stays paused while progress advances");
        }
        if finished(&engine, "ueb1103") == 0 && finished(&engine, "ueb1202") == 1 {
            break;
        }
    }
    assert!(saw_progress_advance, "paused mex work_progress advanced while assisted");
    assert_eq!((finished(&engine, "ueb1103"), finished(&engine, "ueb1202")), (0, 1), "paused upgrade completed via engineer assist");
}

/// A player's factory build order (`BuildFactory`, added to the queue as
/// the construction panel does).
fn factory_build(factory: u32, bp: &str, count: u32) -> PlayerCommand {
    PlayerCommand { kind: CommandKind::BuildFactory, units: vec![factory], target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: Some(bp.into()), count, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None }
}

/// A right-click assist (`Guard`; `IssueFactoryAssist` issues the same).
fn guard_order(units: Vec<u32>, target: u32, queue: bool) -> PlayerCommand {
    PlayerCommand { kind: CommandKind::Guard, units, target: CommandTarget::Entity(target), queue, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None }
}

/// Finished, live units of blueprint `bp`.
fn finished_units(engine: &engine::Engine, bp: &str) -> Vec<u32> {
    engine.sim_lua().app_data_ref::<EntityWorld>().unwrap().iter().filter(|x| !x.destroyed && x.cold.blueprint_id == bp && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).map(|x| x.id).collect()
}

fn queue_of(engine: &engine::Engine, unit: u32) -> Vec<(CommandKind, Option<String>, u32)> {
    engine.sim_lua().app_data_ref::<Commands>().unwrap().queue(unit).map(|cmd| (cmd.kind, cmd.blueprint.clone(), cmd.count)).collect()
}

fn focus_of(engine: &engine::Engine, unit: u32) -> Option<u32> {
    engine.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(unit).and_then(|x| x.unit.as_deref()?.focus)
}

/// Spawns factories of army 1 (blueprint, x, z) with resources to spare;
/// returns their ids.
fn spawn_factories(engine: &engine::Engine, factories: &[(&str, f64, f64)]) -> Vec<u32> {
    let mut script = String::from(
        "local b = ArmyBrains[1]
        b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
        b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
        local ids = {}\n",
    );
    for (bp, x, z) in factories {
        script.push_str(&format!("table.insert(ids, tonumber(CreateUnitHPR('{bp}', 'ARMY_1', {x}, GetTerrainHeight({x}, {z}), {z}, 0, 0, 0):GetEntityId()))\n"));
    }
    script.push_str("return ids");
    let ids: mlua::Table = engine.sim_lua().load(script).eval().unwrap();
    ids.sequence_values::<u32>().map(Result::unwrap).collect()
}

/// A factory assisting another: moho's guard task for a
/// FACTORY (MohoEngine.dll 0x101fd140) takes units one at a time off the
/// main factory's queue — the main's order counts down — and builds them
/// from its own pad, never beaming onto the main or what it builds. The
/// queue is shared, not copied: four ordered, four built between them,
/// and the assister's products head for the main factory's rally point.
#[test]
fn factory_assist_shares_the_queue_of_the_main() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids = spawn_factories(&engine, &[("ueb0101", 430.0, 620.0), ("ueb0101", 450.0, 620.0)]);
    let (main, sub) = (ids[0], ids[1]);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, factory_build(main, "uel0105", 4));
    send(&engine, t, 1, guard_order(vec![sub], main, false));
    // The main factory's rally point (a move order to it).
    send(&engine, t + 1, 0, move_to(1, &[main], 470.0, 690.0));
    for _ in 0..3 {
        engine.tick().unwrap();
    }
    assert_eq!(queue_of(&engine, sub), vec![(CommandKind::Guard, None, 0)], "the assister copies nothing into its own queue");
    assert_eq!(queue_of(&engine, main), vec![(CommandKind::BuildFactory, Some("uel0105".into()), 3)], "it took one of the main's four");
    let sub_pad = position(&engine, sub);
    let mut from_sub = Vec::new();
    for _ in 0..3000 {
        engine.tick().unwrap();
        if let Some(f) = focus_of(&engine, sub) {
            assert_ne!(f, main, "the assister never works on the main factory itself");
            assert!(engine.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(f).is_some_and(|x| x.cold.blueprint_id == "uel0105"), "the assister builds engineers of its own");
            if !from_sub.contains(&f) {
                from_sub.push(f);
            }
        }
        if finished_units(&engine, "uel0105").len() == 4 && engine.sim_lua().app_data_ref::<Commands>().unwrap().is_idle(main) && focus_of(&engine, sub).is_none() {
            break;
        }
    }
    assert_eq!(finished_units(&engine, "uel0105").len(), 4, "four ordered, four built between the two factories");
    assert!(engine.sim_lua().app_data_ref::<Commands>().unwrap().is_idle(main), "the main's queue is spent");
    assert!(!from_sub.is_empty() && from_sub.len() < 4, "the assister built some of them, the main the rest: {from_sub:?}");
    for _ in 0..100 {
        engine.tick().unwrap();
    }
    assert_eq!(finished_units(&engine, "uel0105").len(), 4, "nothing more once the queue is spent");
    assert_eq!(queue_of(&engine, sub), vec![(CommandKind::Guard, None, 0)], "the assister stands at its assist order");
    // The assister's product rolled off its own pad towards the main
    // factory's rally point.
    let product = from_sub[0];
    let (x, z) = position(&engine, product);
    assert!((x - sub_pad.0).abs() > 4.0 || (z - sub_pad.1).abs() > 4.0, "the assister's product rolled off");
    let heading: Vec<(CommandKind, f64, f64)> = engine
        .sim_lua()
        .app_data_ref::<Commands>()
        .unwrap()
        .queue(product)
        .filter_map(|cmd| match cmd.target {
            CommandTarget::Ground(p) => Some((cmd.kind, p.x.to_num::<f64>(), p.z.to_num::<f64>())),
            _ => None,
        })
        .collect();
    let at_rally = heading.iter().any(|&(k, x, z)| k == CommandKind::Move && (x - 470.0).abs() < 0.01 && (z - 690.0).abs() < 0.01);
    let (px, pz) = position(&engine, product);
    assert!(at_rally || ((px - 470.0).abs() < 6.0 && (pz - 690.0).abs() < 6.0), "the assister's product headed for the main factory's rally point: {heading:?} at ({px}, {pz})");
}

/// A land factory assisting an air factory can build nothing of
/// its queue (`CanBuild`), so it takes nothing — and it doesn't stop:
/// what is added to its own queue is built behind the assist order, which
/// stays at the front.
#[test]
fn land_factory_assisting_an_air_factory_still_builds_its_own_queue() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids = spawn_factories(&engine, &[("ueb0102", 430.0, 620.0), ("ueb0101", 460.0, 620.0)]);
    let (air, land) = (ids[0], ids[1]);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, factory_build(air, "uea0101", 3));
    send(&engine, t, 1, guard_order(vec![land], air, false));
    for _ in 0..20 {
        engine.tick().unwrap();
    }
    assert_eq!(queue_of(&engine, land), vec![(CommandKind::Guard, None, 0)], "the land factory took none of the air factory's queue");
    assert_eq!(focus_of(&engine, land), None, "and works on nothing of the air factory's");
    // The player queues an engineer on the assisting land factory.
    send(&engine, tick_now(&engine) + 1, 0, factory_build(land, "uel0105", 1));
    let mut built_by_land = None;
    for _ in 0..1500 {
        engine.tick().unwrap();
        if let Some(f) = focus_of(&engine, land) {
            built_by_land = Some(f);
        }
        if finished_units(&engine, "uel0105").len() == 1 && finished_units(&engine, "uea0101").len() == 3 {
            break;
        }
    }
    assert_eq!(finished_units(&engine, "uel0105").len(), 1, "the assisting land factory built the engineer queued on it");
    assert_eq!(built_by_land.map(|f| finished_units(&engine, "uel0105").contains(&f)), Some(true), "built by the land factory");
    assert_eq!(finished_units(&engine, "uea0101").len(), 3, "the air factory built its own three");
    assert_eq!(queue_of(&engine, land), vec![(CommandKind::Guard, None, 0)], "its order done, the land factory is back at its assist");
}

/// A factory told to assist something that isn't a factory — an
/// engineer here — never repairs it and goes on building what is queued on
/// it. moho refuses the order outright (0x102ca420: a `FACTORY` may only
/// guard a `FACTORY`), so the factory keeps its queue and no assist order
/// joins it (before, ours took the order and the factory sat on it).
#[test]
fn factory_assisting_a_unit_keeps_building_and_never_repairs() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids = spawn_factories(&engine, &[("ueb0101", 430.0, 620.0), ("uel0105", 437.0, 620.0)]);
    let (factory, engineer) = (ids[0], ids[1]);
    engine.sim_lua().load(format!("local e = GetEntityById({engineer}) e:SetHealth(e, e:GetMaxHealth() / 2)")).exec().unwrap();
    let health = |e: &engine::Engine| e.sim_lua().app_data_ref::<EntityWorld>().unwrap().get(engineer).unwrap().health;
    let before = health(&engine);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, guard_order(vec![factory], engineer, false));
    send(&engine, t, 1, factory_build(factory, "uel0105", 2));
    for _ in 0..1500 {
        engine.tick().unwrap();
        assert_ne!(focus_of(&engine, factory), Some(engineer), "the factory never works on the unit it assists");
        if finished_units(&engine, "uel0105").len() == 3 {
            break;
        }
    }
    assert_eq!(finished_units(&engine, "uel0105").len(), 3, "the factory built the two engineers queued on it");
    assert_eq!(health(&engine), before, "no repair from a factory");
    assert_eq!(queue_of(&engine, factory), vec![], "the assist order was refused");
}

/// Factories assisting each other round a cycle: none has a unit to spare
/// (one order for one unit, being built), so the one engineer ordered is
/// built once, and all stand down at their assists — no chasing round the
/// cycle. (Three of them: a guard of a factory whose guard task guards the
/// one ordered is refused, MohoEngine.dll 0x102ca420, so two can't make one
/// this way: `a_guard_of_a_guard_or_of_itself_is_refused`.)
#[test]
fn factories_assisting_each_other_do_not_chase_queues() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids = spawn_factories(&engine, &[("ueb0101", 430.0, 640.0), ("ueb0101", 450.0, 640.0), ("ueb0101", 440.0, 660.0)]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, factory_build(a, "uel0105", 1));
    send(&engine, t + 1, 0, guard_order(vec![b], a, false));
    send(&engine, t + 2, 0, guard_order(vec![c], b, false));
    send(&engine, t + 3, 0, guard_order(vec![a], c, true));
    for _ in 0..1400 {
        engine.tick().unwrap();
    }
    for id in [a, b, c] {
        assert_eq!(queue_of(&engine, id), vec![(CommandKind::Guard, None, 0)], "factory {id} stands down at its assist");
    }
    assert_eq!(finished_units(&engine, "uel0105").len(), 1, "the one engineer ordered, built once");
}

/// A factory assisting a repeating factory takes from its queue too, and
/// the unit it took comes round again on the main's queue: the repeat loop
/// keeps its length.
#[test]
fn factory_assist_on_a_repeating_factory_keeps_the_loop() {
    use rts_core_engine::engine::sim::input::Toggle;
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids = spawn_factories(&engine, &[("ueb0101", 430.0, 620.0), ("ueb0101", 450.0, 620.0)]);
    let (main, sub) = (ids[0], ids[1]);
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, PlayerCommand { kind: CommandKind::Stop, units: vec![main], target: CommandTarget::None, queue: false, formation: None, facing: None, blueprint: None, count: 0, army: 1, toggle: Some(Toggle::RepeatQueue(true)), script: Vec::new(), callback: None, category: None, retarget: None });
    send(&engine, t + 1, 0, factory_build(main, "uel0105", 1));
    send(&engine, t + 1, 1, factory_build(main, "uel0105", 1));
    send(&engine, t + 2, 0, guard_order(vec![sub], main, false));
    let mut sub_built = false;
    for _ in 0..2000 {
        engine.tick().unwrap();
        let units: u32 = queue_of(&engine, main).iter().filter(|q| q.0 == CommandKind::BuildFactory).map(|q| q.2).sum();
        if tick_now(&engine) > t + 3 {
            assert_eq!(units, 2, "the repeating main's loop keeps its two units");
        }
        sub_built |= focus_of(&engine, sub).is_some();
        if finished_units(&engine, "uel0105").len() >= 4 {
            break;
        }
    }
    assert!(sub_built, "the assister took from the repeating queue");
    assert!(finished_units(&engine, "uel0105").len() >= 4, "the loop goes on");
}

/// FAF's assist-to-cap (`RingWithStorages`, sent when engineers assist a
/// T2/T3 extractor): the engineer queues a mass storage on each of the
/// extractor's four sides and builds them.
#[test]
fn ring_with_storages_caps_an_extractor() {
    if !have_data() {
        return;
    }
    let (mut engine, _tanks, _) = setup(None);
    let ids: LuaTable = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local mex = CreateUnitHPR('ueb1202', 'ARMY_1', 380.5, GetTerrainHeight(380.5, 570.5), 570.5, 0, 0, 0)
            local eng = CreateUnitHPR('uel0105', 'ARMY_1', 374, GetTerrainHeight(374, 570), 570, 0, 0, 0)
            return { tonumber(mex:GetEntityId()), tonumber(eng:GetEntityId()) }
            "#,
        )
        .eval()
        .unwrap();
    let (mex, eng): (u32, u32) = (ids.raw_get(1).unwrap(), ids.raw_get(2).unwrap());
    let args = LuaData::Table(vec![(LuaData::string("target"), LuaData::string(&mex.to_string()))]);
    send(&engine, tick_now(&engine) + 1, 0, callback("RingWithStorages", args, &[eng]));
    engine.tick().unwrap();
    engine.tick().unwrap();
    // A storage on each side (after reclaiming any props in the way).
    let queued: i64 = engine.sim_lua().load("local n = 0 for _, c in GetEntityById(...):GetCommandQueue() do if c.blueprintId == 'ueb1106' then n = n + 1 end end return n").call(eng).unwrap();
    assert_eq!(queued, 4, "a storage on each side");
    let storages = |e: &engine::Engine| -> Vec<(f64, f64)> {
        let w = e.sim_lua().app_data_ref::<EntityWorld>().unwrap();
        let mut at: Vec<_> = w.iter().filter(|x| !x.destroyed && x.cold.blueprint_id == "ueb1106" && x.fraction_complete >= rts_core_engine::engine::sim::math::Fx::ONE).map(|x| {
            let (x, _, z) = x.position().to_f64();
            (x, z)
        }).collect();
        at.sort_by(|a, b| a.partial_cmp(b).unwrap());
        at
    };
    for _ in 0..3000 {
        engine.tick().unwrap();
        if storages(&engine).len() == 4 {
            break;
        }
    }
    assert_eq!(storages(&engine), vec![(378.5, 570.5), (380.5, 568.5), (380.5, 572.5), (382.5, 570.5)]);
}

/// Loads onto a ferry beacon (moho's per-unit check 0x102ca420 case 22 and
/// the interface's factory command for factories, 0x103e89c0 case 0x200):
/// a tank waits there; a transport (`TRANSPORTATION`) is refused, its own
/// route's beacon too; a factory's order goes on its factory command queue,
/// and the tank it builds joins it and waits at the beacon.
#[test]
fn loads_onto_a_ferry_beacon() {
    if !have_data() {
        return;
    }
    let (mut engine, tanks, _) = setup(None);
    let ids = spawn_factories(&engine, &[("ueb0101", 430.0, 620.0), ("uea0107", 410.0, 600.0), ("uea0107", 414.0, 604.0)]);
    let (factory, ferry, other) = (ids[0], ids[1], ids[2]);
    engine.sim_lua().load(format!("IssueFerry({{ GetEntityById({ferry}) }}, {{ 440, 0, 640 }}) IssueFerry({{ GetEntityById({ferry}) }}, {{ 470, 0, 660 }})")).exec().unwrap();
    for _ in 0..3 {
        engine.tick().unwrap();
    }
    let beacon: u32 = engine.sim_lua().load(format!("return tonumber(GetEntityById({ferry}):GetTransportFerryBeacon():GetEntityId())")).eval().unwrap();
    let load = |units: Vec<u32>| PlayerCommand { kind: CommandKind::TransportLoadUnits, units, target: CommandTarget::Entity(beacon), ..move_to(1, &[], 0.0, 0.0) };
    let current = |e: &engine::Engine, id: u32| e.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| (c.kind, c.target));
    let t = tick_now(&engine) + 1;
    send(&engine, t, 0, load(vec![tanks[0], other]));
    send(&engine, t, 1, load(vec![ferry]));
    send(&engine, t, 2, load(vec![factory]));
    send(&engine, t, 3, factory_build(factory, "uel0201", 1));
    engine.tick().unwrap();
    engine.tick().unwrap();
    assert_eq!(current(&engine, tanks[0]), Some((CommandKind::TransportLoadUnits, CommandTarget::Entity(beacon))), "the tank waits at the beacon");
    let waiting: bool = engine.sim_lua().load(format!("return GetEntityById({}):IsUnitState('WaitForFerry')", tanks[0])).eval().unwrap();
    assert!(waiting, "in WaitForFerry");
    assert_eq!(current(&engine, other), None, "a transport isn't ferried");
    assert_eq!(current(&engine, ferry).map(|c| c.0), Some(CommandKind::Move), "the ferry keeps its route (heading for its beacon), not a load onto it");
    let factory_queue: Vec<CommandKind> = engine.sim_lua().app_data_ref::<Commands>().unwrap().factory_queue(factory).map(|c| c.kind).collect();
    assert_eq!(factory_queue, vec![CommandKind::TransportLoadUnits], "the factory's load is a factory command");
    let first_new = ids.iter().max().copied().unwrap() + 1;
    let built = |e: &engine::Engine| finished_units(e, "uel0201").into_iter().filter(|&u| u >= first_new).collect::<Vec<_>>();
    for _ in 0..1500 {
        engine.tick().unwrap();
        if !built(&engine).is_empty() {
            break;
        }
    }
    let new = *built(&engine).first().expect("the factory built a tank");
    let mut joined = false;
    for _ in 0..200 {
        engine.tick().unwrap();
        let queue: Vec<(CommandKind, CommandTarget)> = engine.sim_lua().app_data_ref::<Commands>().unwrap().queue(new).map(|c| (c.kind, c.target)).collect();
        if queue.contains(&(CommandKind::TransportLoadUnits, CommandTarget::Entity(beacon))) {
            joined = true;
            break;
        }
    }
    assert!(joined, "the new tank goes to wait at the beacon");
}
