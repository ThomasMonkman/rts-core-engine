//! The player's fog of war in snapshots: with the game's `FogOfWar` option
//! on, a view army's snapshots carry only what its side knows (enemies seen
//! now, radar blips, remembered structures) and a fog texture whose
//! incremental row updates add up to the sim's current grids; with `'none'`
//! (or no view army) everything is there. Runs the default skirmish (two AI
//! armies) plus `dev/scenes/fight.lua`. Needs the FAF checkout (`../fa`) and game
//! data; fails without them (`RTS_SKIP_DATA_TESTS=1` skips).

use std::sync::{Arc, Mutex};

use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine::snapshot::{FogUpdate, Sighting, Snapshot};
use rts_core_engine::engine::{self, sim};

const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

/// Fails without the game data (`RTS_SKIP_DATA_TESTS=1` skips instead).
fn have_data() -> bool {
    rts_core_engine::test_data::require_game("fog_of_war")
}

fn game(fog: &str) -> engine::Engine {
    let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let session = engine::session::SessionConfig { script: Some("dev/scenes/fight.lua".into()), ticks: Some(0), fog_of_war: Some(fog.into()), ..Default::default() };
    engine.start(&session).expect("session");
    engine
}

/// The fog texture as the renderer would hold it after applying updates.
#[derive(Default)]
struct Texture {
    on: bool,
    cols: u32,
    rows: u32,
    levels: Vec<u8>,
    fulls: usize,
}

impl Texture {
    fn apply(&mut self, update: FogUpdate) {
        match update {
            FogUpdate::Off => self.on = false,
            FogUpdate::Grid { cols, rows, full, bands, .. } => {
                self.on = true;
                if full {
                    self.fulls += 1;
                    (self.cols, self.rows) = (cols, rows);
                    self.levels = vec![255; (cols * rows) as usize];
                }
                for (first, bytes) in bands {
                    let start = (first * cols) as usize;
                    self.levels[start..start + bytes.len()].copy_from_slice(&bytes);
                }
            }
        }
    }
}

/// Runs `ticks` ticks viewing as `army`, checking every snapshot with
/// `check` (against the sim as it is when the snapshot was made), and
/// returns the fog texture the updates built.
fn view(fog: &str, army: Option<u32>, ticks: u32, mut check: impl FnMut(&engine::Engine, &Snapshot)) -> (engine::Engine, Texture) {
    let mut engine = game(fog);
    let (sink, rx, request) = engine::snapshot::channel();
    request.lock().unwrap().view_army = army;
    engine.set_snapshot_sink(sink);
    let mut texture = Texture::default();
    for _ in 0..ticks {
        engine.tick().expect("tick");
        while let Ok(mut snap) = rx.try_recv() {
            drop(snap.ticket.take());
            // (The assets-only snapshot has no world.)
            if !snap.entities.is_empty() {
                check(&engine, &snap);
            }
            if let Some(f) = snap.fog.take() {
                texture.apply(f);
            }
        }
    }
    (engine, texture)
}

fn enemy_units(engine: &engine::Engine, army: u32) -> usize {
    let world = engine.sim_lua().app_data_ref::<sim::entity::EntityWorld>().unwrap();
    world.iter().filter(|e| !e.destroyed && e.unit.is_some() && e.army.is_some_and(|a| a != army) && e.fraction_complete > sim::math::Fx::ZERO).count()
}

/// 'explored': army 1's snapshots hold no enemy unit it doesn't detect,
/// blips only for radar / sonar contacts, and fewer enemies than exist.
#[test]
fn explored_hides_undetected_enemies() {
    if !have_data() {
        return;
    }
    let mut hidden_some = false;
    let (_, texture) = view("explored", Some(1), 300, |engine, snap| {
        let lua = engine.sim_lua();
        let world = lua.app_data_ref::<sim::entity::EntityWorld>().unwrap();
        let grids = lua.app_data_ref::<sim::vision::IntelGrids>().unwrap();
        let armies = lua.app_data_ref::<sim::army::Armies>().unwrap();
        let mut shown = 0;
        for v in &snap.entities {
            let Some(e) = world.get(v.id) else { continue };
            let Some(owner) = e.army.filter(|_| e.unit.is_some()) else { continue };
            if owner == 1 || armies.alliance(1, owner) == sim::army::Alliance::Ally {
                assert_eq!(v.sighting, Sighting::Seen);
                continue;
            }
            shown += 1;
            let d = grids.detect(1, e);
            match v.sighting {
                Sighting::Seen => assert!(d.seen, "enemy {} shown but not seen", v.id),
                Sighting::Blip => assert!(d.any() && !d.seen && !grids.ever_seen(1, v.id), "blip {} without a contact, or seen before", v.id),
                Sighting::Identified => assert!(d.any() && !d.seen && grids.ever_seen(1, v.id), "identified {} not a contact seen before", v.id),
                Sighting::Remembered => assert!(!d.seen, "remembered {} is in sight", v.id),
            }
        }
        drop((world, grids, armies));
        hidden_some |= shown < enemy_units(engine, 1);
    });
    assert!(hidden_some, "army 1 can't see every enemy");
    assert!(texture.on && texture.fulls == 1, "one full texture, then row updates");
    assert!(texture.levels.iter().all(|&l| l >= sim::vision::fog_level::EXPLORED), "'explored': nothing is black");
}

/// 'none', and observers: every unit is there and there's no fog.
#[test]
fn none_and_observers_see_everything() {
    if !have_data() {
        return;
    }
    for (fog, army) in [("none", Some(1)), ("explored", None)] {
        let (engine, texture) = view(fog, army, 150, |engine, snap| {
            let n = snap.entities.iter().filter(|v| v.sighting != Sighting::Seen).count();
            assert_eq!(n, 0, "{fog}: no blips or memories");
            let world = engine.sim_lua().app_data_ref::<sim::entity::EntityWorld>().unwrap();
            let units = world.iter().filter(|e| !e.destroyed && e.unit.is_some() && e.attachment.is_none()).count();
            let shown = snap.entities.iter().filter(|v| world.get(v.id).is_some_and(|e| e.unit.is_some() && e.attachment.is_none())).count();
            assert_eq!(shown, units, "{fog} {army:?}: every unit shown");
        });
        drop(engine);
        assert!(!texture.on, "{fog} {army:?}: fog off");
    }
}

/// 'unexplored': the texture starts black away from army 1's side, and its
/// row-by-row updates always equal a fresh build from the sim's grids.
#[test]
fn unexplored_starts_black_and_updates_add_up() {
    if !have_data() {
        return;
    }
    let (engine, texture) = view("unexplored", Some(1), 300, |_, _| {});
    let grids = engine.sim_lua().app_data_ref::<sim::vision::IntelGrids>().unwrap();
    let (cols, rows, _) = grids.dimensions();
    assert_eq!((texture.cols, texture.rows), (cols, rows));
    let mut fresh = vec![0u8; (cols * rows) as usize];
    for z in 0..rows {
        grids.fog_row(1, z, false, &mut fresh[(z * cols) as usize..((z + 1) * cols) as usize]);
    }
    assert!(texture.levels == fresh, "incremental rows == a full rebuild");
    let count = |l: u8| fresh.iter().filter(|&&x| x == l).count();
    use sim::vision::fog_level::*;
    assert!(count(UNEXPLORED) > fresh.len() / 2, "most of the map unexplored");
    assert!(count(VISIBLE) > 0, "its side sees something");
}
