//! End-to-end scenario tests: each `tests/scenarios/<name>.lua` runs FAF's
//! real unit scripts against the engine (upgrades, enhancements, capture,
//! reclaim, formations, bombing, flight, fuel, armor, transports) and
//! records its checks in `ScenarioResult`; the test steps the sim until the
//! scenario says it's done and fails with its failed checks. Plus an AI
//! smoke test: a normal AI game for a while must log no Lua errors.
//!
//! Scenarios run with the two armies as human slots nobody plays (no AI)
//! and no starting commanders, so only the scenario moves their units. Needs the FAF checkout (`../fa`) and game
//! data; fails without them (`RTS_SKIP_DATA_TESTS=1` skips). Run one: `cargo test --test scenarios
//! transport`.

use std::sync::{Arc, Mutex};

use mlua::prelude::*;
use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine;

const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

/// Fails without the game data (`RTS_SKIP_DATA_TESTS=1` skips instead).
fn have_data() -> bool {
    rts_core_engine::test_data::require_game("scenario tests")
}

fn start(script: Option<&str>, ai: bool) -> engine::Engine {
    let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let session = engine::session::SessionConfig {
        script: script.map(String::from),
        ticks: Some(0),
        create_initial_units: ai,
        ai,
        ..Default::default()
    };
    engine.start(&session).expect("session");
    engine
}

/// Runs `tests/scenarios/<name>.lua` for at most `max_ticks`; panics with the
/// scenario's failed checks (or if it never finished).
fn scenario(name: &str, max_ticks: u32) {
    if !have_data() {
        return;
    }
    let mut engine = start(Some(&format!("tests/scenarios/{name}.lua")), false);
    let result = |engine: &engine::Engine| -> Option<LuaTable> { engine.sim_lua().globals().raw_get("ScenarioResult").ok().flatten() };
    let mut ticks = 0;
    while ticks < max_ticks && !result(&engine).is_some_and(|r| r.get::<bool>("done").unwrap_or(false)) {
        engine.tick().expect("tick");
        ticks += 1;
    }
    let r = result(&engine).unwrap_or_else(|| panic!("{name}: the scenario never set ScenarioResult (did it load?)"));
    let list = |key: &str| -> Vec<String> { r.get::<LuaTable>(key).map(|t| t.sequence_values::<String>().filter_map(Result::ok).collect()).unwrap_or_default() };
    for n in list("notes") {
        eprintln!("{name}: {n}");
    }
    let failures = list("failures");
    assert!(r.get::<bool>("done").unwrap_or(false), "{name}: not done after {max_ticks} ticks; failures so far: {failures:?}");
    assert!(failures.is_empty(), "{name} failed after {ticks} ticks:\n  {}", failures.join("\n  "));
}

#[test]
fn bounded_props() {
    scenario("bounded_props", 30);
}

#[test]
fn game_tick_stages() {
    scenario("game_tick_stages", 150);
}

#[test]
fn platoon_destroy_brain() {
    scenario("platoon_destroy_brain", 30);
}

#[test]
fn water_targets_only() {
    scenario("water_targets_only", 400);
}

#[test]
fn auto_surface_attack() {
    scenario("auto_surface_attack", 400);
}

#[test]
fn carrier_surfacing() {
    scenario("carrier_surfacing", 2500);
}

/// Carriers launch their aircraft every second tick from their launch
/// points, without moving, the aircraft sent to guard the point.
#[test]
fn carrier_launch() {
    scenario("carrier_launch", 400);
}

#[test]
fn sub_dive_tilt() {
    scenario("sub_dive_tilt", 600);
}

/// The Fire Beetle detonates on an attack order, never at an
/// enemy it only sees.
#[test]
fn fire_beetle_attack() {
    scenario("fire_beetle_attack", 200);
}

/// `Issue*` orders leave out units without the order's command cap.
#[test]
fn order_command_caps() {
    scenario("order_command_caps", 30);
}

/// The Solace's torpedo clusters split above the water and their
/// torpedoes home on the target.
#[test]
fn torpedo_cluster_split() {
    scenario("torpedo_cluster_split", 300);
}

/// A structure built only in the Sub layer (the HARMS) is in Sub.
#[test]
fn sub_layer_structure() {
    scenario("sub_layer_structure", 200);
}

#[test]
fn torpedo_launch() {
    scenario("torpedo_launch", 300);
}

#[test]
fn dummy_weapons() {
    scenario("dummy_weapons", 30);
}

#[test]
fn beetle_ring() {
    scenario("beetle_ring", 80);
}

#[test]
fn upgrade() {
    scenario("upgrade", 4000);
}

#[test]
fn navigator_goal() {
    scenario("navigator_goal", 40);
}

#[test]
fn blip_identity() {
    scenario("blip_identity", 200);
}

#[test]
fn blueprint_regen() {
    scenario("blueprint_regen", 60);
}

#[test]
fn max_health() {
    scenario("max_health", 30);
}

#[test]
fn brain_factories() {
    scenario("brain_factories", 40);
}

#[test]
fn num_build_orders() {
    scenario("num_build_orders", 40);
}

#[test]
fn can_path_to() {
    scenario("can_path_to", 40);
}

#[test]
fn unit_toggles() {
    scenario("unit_toggles", 40);
}

#[test]
fn enhancement() {
    scenario("enhancement", 2000);
}

#[test]
fn capture() {
    scenario("capture", 1000);
}

#[test]
fn reclaim() {
    scenario("reclaim", 600);
}

/// Reclaiming a unit takes its health down with its fraction, so
/// one broken off partway is left as weak as it is see-through.
#[test]
fn reclaim_unit() {
    scenario("reclaim_unit", 1200);
}

#[test]
fn veterancy() {
    scenario("veterancy", 1500);
}

#[test]
fn staging_service() {
    scenario("staging_service", 400);
}

#[test]
fn bomb_lead() {
    scenario("bomb_lead", 300);
}

#[test]
fn air_combat_states() {
    scenario("air_combat_states", 500);
}

#[test]
fn air_lift_gains() {
    scenario("air_lift_gains", 100);
}

#[test]
fn projectile_position() {
    scenario("projectile_position", 100);
}

#[test]
fn projectile_spin() {
    scenario("projectile_spin", 100);
}

#[test]
fn projectile_bounce() {
    scenario("projectile_bounce", 100);
}

#[test]
fn projectile_speed_range() {
    scenario("projectile_speed_range", 100);
}

#[test]
fn ground_variation() {
    scenario("ground_variation", 50);
}

#[test]
fn guard_leash() {
    scenario("guard_leash", 600);
}

#[test]
fn face_to_build() {
    scenario("face_to_build", 200);
}

#[test]
fn formation() {
    scenario("formation", 600);
}

#[test]
fn bomber() {
    scenario("bomber", 600);
}

#[test]
fn gunship_circle() {
    scenario("gunship_circle", 1400);
}

#[test]
fn auto_land() {
    scenario("auto_land", 1200);
}

#[test]
fn land_bodies() {
    scenario("land_bodies", 800);
}

/// FAF's tread component gives a moving tank a thread scroller, and its
/// tracks scroll forward together on a straight run.
#[test]
fn treads() {
    if !have_data() {
        return;
    }
    let mut engine = start(Some("tests/scenarios/treads.lua"), false);
    for _ in 0..200 {
        engine.tick().expect("tick");
        if engine.sim_lua().globals().raw_get::<Option<String>>("TreadTank").unwrap().is_some() {
            break;
        }
    }
    let id: u32 = engine.sim_lua().globals().raw_get::<String>("TreadTank").expect("tank").parse().unwrap();
    let world = engine.sim_lua().app_data_ref::<engine::sim::entity::EntityWorld>().expect("world");
    let s = world.get(id).and_then(|e| e.cold.scroller).expect("a tread scroller");
    let (a, b) = (s.current.0.to_num::<f64>(), s.current.1.to_num::<f64>());
    assert!(a > 1.0 && (a - b).abs() < 0.2 * a, "tracks scrolled {a} / {b}");
    let plane: u32 = engine.sim_lua().globals().raw_get::<String>("PingPongPlane").expect("plane").parse().unwrap();
    let p = world.get(plane).and_then(|e| e.cold.scroller).expect("the interceptor's ping-pong scroller");
    assert!(matches!(p.kind, engine::sim::scroller::ScrollKind::PingPong { .. }), "{:?}", p.kind);
}

#[test]
fn aircraft() {
    scenario("aircraft", 800);
}

#[test]
fn air_crash() {
    scenario("air_crash", 1200);
}

#[test]
fn valid_bone() {
    scenario("valid_bone", 40);
}

#[test]
fn fuel() {
    scenario("fuel", 200);
}

#[test]
fn armor() {
    scenario("armor", 100);
}

#[test]
fn transport() {
    scenario("transport", 3000);
}

#[test]
fn beam() {
    scenario("beam", 400);
}

/// A collision beam's far end starts at its target when enabled, first fire
/// and refire, not at its last firing's end (Monkeylord laser).
#[test]
fn beam_refire() {
    scenario("beam_refire", 400);
}

/// The Novax satellite's beam reaches the ground from its elevation, and the
/// satellite holds its height on station.
#[test]
fn novax() {
    scenario("novax", 1500);
}

/// Every Fatboy weapon (artillery, riot guns, AA) engages dummy targets.
#[test]
fn fatboy() {
    scenario("fatboy", 800);
}

/// A big plain move follows a shared flow field and everyone arrives.
#[test]
fn flow() {
    scenario("flow", 800);
}

/// A formation sent against a cliff edge (east of the UEF base) or the
/// shore (north) folds its blocked slots in to open ground, and the tanks
/// end on their slots. Before, slots were laid out round the order's point
/// while the block stopped at the nearest reachable one, and the crowd rule
/// let members stop a crowd's width short: median slot error ~10, p90 ~14
/// at the cliff.
#[test]
fn formation_edges() {
    use rts_core_engine::engine::sim::command::{CommandTarget, Commands};
    use rts_core_engine::engine::sim::entity::EntityWorld;
    if !have_data() {
        return;
    }
    for (x, z) in [(690, 470), (600, 330)] {
        let mut engine = start(None, false);
        let lua = engine.sim_lua();
        lua.globals().set("FormationEdgesTarget", lua.create_sequence_from([x as f64, 0.0, z as f64]).unwrap()).unwrap();
        lua.load(std::fs::read_to_string("tests/scenarios/formation_edges.lua").unwrap()).exec().expect("script");
        // Each tank's slot while its order lasts (the last one is where it
        // should end up).
        let mut slots: std::collections::BTreeMap<u32, (f64, f64)> = Default::default();
        for _ in 0..700 {
            engine.tick().expect("tick");
            let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
            for u in c.busy_units().collect::<Vec<_>>() {
                let Some(cmd) = c.current(u) else { continue };
                let (CommandTarget::Ground(p), Some(s)) = (cmd.target, cmd.slot(u)) else { continue };
                let base = cmd.travel.get(s.group as usize).filter(|t| t.planned).map_or(p, |t| t.target);
                slots.insert(u, ((base.x + s.dx).to_num(), (base.z + s.dz).to_num()));
            }
        }
        let w = engine.sim_lua().app_data_ref::<EntityWorld>().unwrap();
        let c = engine.sim_lua().app_data_ref::<Commands>().unwrap();
        assert_eq!(slots.len(), 24, "every tank had a slot");
        let mut err: Vec<f64> = slots
            .iter()
            .map(|(&u, &(sx, sz))| {
                let (px, _, pz) = w.get(u).unwrap().position().to_f64();
                ((px - sx).powi(2) + (pz - sz).powi(2)).sqrt()
            })
            .collect();
        err.sort_by(f64::total_cmp);
        let busy = slots.keys().filter(|&&u| !c.is_idle(u)).count();
        let (median, p90) = (err[err.len() / 2], err[err.len() * 9 / 10]);
        eprintln!("formation_edges ({x}, {z}): slot error median {median:.1}, p90 {p90:.1}, max {:.1}; {busy} still moving", err[err.len() - 1]);
        assert!(median < 1.5 && p90 < 5.0, "({x}, {z}): tanks off their slots: median {median:.1}, p90 {p90:.1}");
        assert!(busy <= 3, "({x}, {z}): {busy} tanks never settled");
    }
}

/// Units sent off Seton's shore cliffs and back never drive onto a cell
/// their class can't stand on.
#[test]
fn shore_cliff() {
    scenario("shore_cliff", 1400);
}

/// A formation travels as a wide block, not a ball.
#[test]
fn formation_travel() {
    scenario("formation_travel", 800);
}

/// Engineers assisting a factory from the same side spread out where they
/// park instead of stacking on one point, and stay put.
#[test]
fn assist_spread() {
    scenario("assist_spread", 500);
}

#[test]
fn footprints() {
    scenario("footprints", 800);
}

#[test]
fn overcharge() {
    scenario("overcharge", 200);
}

#[test]
fn silo() {
    scenario("silo", 900);
}

#[test]
fn air_motion() {
    scenario("air_motion", 150);
}

#[test]
fn hover_lean() {
    scenario("hover_lean", 120);
}

/// Units ease into a stop, never speeding up again on the last
/// ticks of a move.
#[test]
fn arrival_ease() {
    scenario("arrival_ease", 260);
}

#[test]
fn collision_check() {
    scenario("collision_check", 200);
}

#[test]
fn shield() {
    scenario("shield", 300);
}

#[test]
fn health_changed() {
    scenario("health_changed", 60);
}

#[test]
fn economy_tracking() {
    scenario("economy_tracking", 160);
}

#[test]
fn failed_halt_attached() {
    scenario("failed_halt_attached", 200);
}

#[test]
fn teleport() {
    scenario("teleport", 300);
}

#[test]
fn water_crossing() {
    scenario("water_crossing", 80);
}

#[test]
fn terrain_type() {
    scenario("terrain_type", 300);
}

#[test]
fn focus_refuel() {
    scenario("focus_refuel", 260);
}

#[test]
fn trees() {
    scenario("trees", 220);
}

#[test]
fn intel_change() {
    scenario("intel_change", 1300);
}

#[test]
fn unit_cap() {
    scenario("unit_cap", 120);
}

/// Natural decay: an abandoned construction loses progress and gets
/// `OnDecayed` at none left.
#[test]
fn decay() {
    scenario("decay", 400);
}

/// `ChangeUnitArmy` past the receiver's unit cap: refused, the giver hears
/// `OnFailedUnitTransfer`.
#[test]
fn failed_transfer() {
    scenario("failed_transfer", 60);
}

/// The prebuilt-units start: a factory, generators and extractors, finished
/// and apart, near the start spot.
#[test]
fn prebuilt() {
    scenario("prebuilt", 60);
}

#[test]
fn sacrifice() {
    scenario("sacrifice", 360);
}

/// Weapon aim: hit rates against still and moving targets (leading,
/// artillery arcs, hull-fixed weapons turning the unit, homing missiles).
#[test]
fn aim() {
    scenario("aim", 30000);
}

/// Shells splitting into fragments at their detonation height (the Lobo's,
/// the Salvation's two-stage split).
#[test]
fn fragments() {
    scenario("fragments", 3000);
}

#[test]
fn flak_burst() {
    scenario("flak_burst", 400);
}

#[test]
fn intercept() {
    scenario("intercept", 3000);
}

/// Torpedo defence: anti-torpedo weapons damage or deflect incoming torpedoes
/// (the UEF Smart Charge never firing is noted, not checked).
#[test]
fn torpedo_intercept() {
    scenario("torpedo_intercept", 2500);
}

#[test]
fn carrier() {
    scenario("carrier", 500);
}

#[test]
fn footfall() {
    scenario("footfall", 200);
}

#[test]
fn adjacency() {
    scenario("adjacency", 100);
}

#[test]
fn builder_arm() {
    scenario("builder_arm", 900);
}

#[test]
fn placement() {
    scenario("placement", 20);
}

#[test]
fn deposit_placement() {
    scenario("deposit_placement", 20);
}

/// Collects `WARN` / `ERROR` events from Lua (script errors) on this thread.
#[derive(Clone, Default)]
struct LuaErrors(Arc<Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LuaErrors {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let level = *event.metadata().level();
        if level > tracing::Level::WARN || event.metadata().target() != "lua" {
            return;
        }
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                if f.name() == "message" {
                    self.0 = format!("{v:?}");
                }
            }
        }
        let mut m = Message(String::new());
        event.record(&mut m);
        self.0.lock().unwrap().push(m.0);
    }
}

/// Messages from FAF's own Lua that aren't engine faults: blueprint
/// warnings at load and the AI's debug chatter.
fn expected_noise(message: &str) -> bool {
    ["Preferences file is not found", "Overriding the x axis of collision box", "AI DEBUG", "ACU kill detected"].iter().any(|n| message.contains(n))
}

/// The default AI game (two adaptive AIs, Seton's Clutch) for 2000 ticks
/// (3:20 of game time): no Lua errors, no stalls.
#[test]
fn ai_game_runs_clean() {
    use tracing_subscriber::prelude::*;
    if !have_data() {
        return;
    }
    let errors = LuaErrors::default();
    let subscriber = tracing_subscriber::registry().with(errors.clone());
    tracing::subscriber::with_default(subscriber, || {
        let mut engine = start(None, true);
        for _ in 0..2000 {
            engine.tick().expect("tick");
        }
        // Fire-and-forget effects (muzzle flashes, explosions) and those of
        // dead units must not pile up (129 alive of ~2300 with expiry; 538
        // without, growing with game length).
        let p = engine.sim_lua().app_data_ref::<rts_core_engine::engine::sim::presentation::Presentation>().expect("presentation");
        eprintln!("live effects after 2000 ticks: {} (created {})", p.effects().len(), p.next_effect_id);
        assert!(p.effects().len() < 300, "{} effects alive of {} created", p.effects().len(), p.next_effect_id);
    });
    let errors: Vec<String> = errors.0.lock().unwrap().iter().filter(|m| !expected_noise(m)).cloned().collect();
    assert!(errors.is_empty(), "{} Lua errors in the AI game, first ones:\n  {}", errors.len(), errors.iter().take(5).cloned().collect::<Vec<_>>().join("\n  "));
}

/// Cost of FAF's `formations.lua` laying out a big group (what a 1000-unit
/// formation order spends in LuaThreads): mixed T1-T3 tanks and bots, each
/// call on a rotated list so FAF's result cache never hits. Timing only:
/// `cargo test --release --test scenarios formation_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn formation_cost() {
    if !have_data() {
        return;
    }
    let engine = start(None, false);
    let lua = engine.sim_lua();
    let run: LuaFunction = lua
        .load(
            r#"
            local ids = {'uel0201', 'uel0202', 'uel0303', 'uel0106', 'uel0307', 'uel0111'}
            local units = {}
            for i = 0, 999 do
                local x, z = 300 + math.mod(i, 40) * 3, 300 + math.floor(i / 40) * 3
                table.insert(units, CreateUnitHPR(ids[math.mod(i, table.getn(ids)) + 1], 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0))
            end
            local f = import('/lua/formations.lua')
            local function list(shift)
                local l = {}
                for i = 0, 999 do l[i] = units[math.mod(i + shift, 1000) + 1] end
                return l
            end
            local k = 0
            return function() k = k + 1 return f.AttackFormation(list(k)) end
            "#,
        )
        .eval()
        .expect("formation script");
    run.call::<LuaValue>(()).expect("warm-up");
    let t = std::time::Instant::now();
    let n: u32 = std::env::var("FORMATION_COST_CALLS").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    for _ in 0..n {
        run.call::<LuaValue>(()).expect("formation");
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    eprintln!("formation_cost: AttackFormation of 1000 units: {ms:.2} ms per call");
}

#[test]
fn factory_rolloff() {
    scenario("factory_rolloff", 700);
}

#[test]
fn experimental_build() {
    scenario("experimental_build", 400);
}

#[test]
fn aa_attack_ground() {
    scenario("aa_attack_ground", 200);
}

#[test]
fn czar_beam() {
    scenario("czar_beam", 250);
}

#[test]
fn factory_upgrade() {
    scenario("factory_upgrade", 200);
}

#[test]
fn shared_build() {
    scenario("shared_build", 700);
}

#[test]
fn naval_structures() {
    scenario("naval_structures", 800);
}

#[test]
fn turret_reset() {
    scenario("turret_reset", 150);
}

#[test]
fn transport_death() {
    scenario("transport_death", 300);
}

#[test]
fn ship_sinks() {
    scenario("ship_sinks", 700);
}

#[test]
fn fatboy_death() {
    scenario("fatboy_death", 450);
}

/// Unloading (transport.rs passengers) detaches only cargo, not the Fatboy's
/// external-factory anchor chain.
#[test]
fn fatboy_unload() {
    scenario("fatboy_unload", 600);
}

#[test]
fn air_crash_cleanup() {
    scenario("air_crash_cleanup", 500);
}

#[test]
fn effect_keepalive() {
    scenario("effect_keepalive", 100);
}

#[test]
fn site_clearing() {
    scenario("site_clearing", 1000);
}

/// A unit standing where a structure is ordered is moved clear first, and
/// the build waits until the site is free.
#[test]
fn site_clearing_units() {
    scenario("site_clearing_units", 1000);
}

/// Units built, lost and killed, per army and per blueprint, as moho keeps
/// them for the end-of-game screen.
#[test]
fn unit_stats() {
    scenario("unit_stats", 1500);
}

/// FAF's score thread (`/lua/sim/score.lua`) fills the scoreboard's eco
/// columns from the economy stats moho keeps (`Economy_TotalProduced_*`
/// etc. through `GetArmyStat`): after a minute of an AI game every army has
/// income, spending, storage, a unit count and a score.
#[test]
fn scoreboard_eco_from_army_stats() {
    if !have_data() {
        return;
    }
    let mut engine = start(None, true);
    for _ in 0..600 {
        engine.tick().expect("tick");
    }
    let report: String = engine
        .sim_lua()
        .load(
            r#"
            local bad = {}
            for index, s in import('/lua/sim/score.lua').scoreData.current do
                local r, g = s.resources, s.general
                local function want(ok, what) if not ok then table.insert(bad, index .. ': ' .. what) end end
                want(r.massin.total > 0 and r.massin.rate > 0, 'mass income ' .. r.massin.total .. ' / ' .. r.massin.rate)
                want(r.energyin.total > 0 and r.energyin.rate > 0, 'energy income ' .. r.energyin.total .. ' / ' .. r.energyin.rate)
                want(r.massout.total > 0, 'mass spent ' .. r.massout.total)
                want(r.storage.maxMass > 0 and r.storage.maxEnergy > 0, 'storage')
                want(g.currentunits > 0 and g.currentcap > 0, 'unit cap ' .. g.currentunits .. '/' .. g.currentcap)
                want(g.score > 0, 'score ' .. g.score)
                local brain = ArmyBrains[index]
                want(brain:GetArmyStat('Economy_Stored_Energy', 0).Value == brain:GetEconomyStored('ENERGY'), 'stored stat')
            end
            return table.concat(bad, '; ')
            "#,
        )
        .eval()
        .expect("score data");
    assert!(report.is_empty(), "{report}");
}

/// What the score thread syncs to the UI (`Sync.Score`): the local
/// player's (and allies') full score; an enemy's only its general score,
/// no resources (FAF's score.lua keys this on the sim's `GetFocusArmy`).
#[test]
fn scoreboard_hides_enemy_eco() {
    if !have_data() {
        return;
    }
    // Played as army 1 (`--player 1`), against the AI.
    let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let session = engine::session::SessionConfig { ticks: Some(0), create_initial_units: true, ai: true, human_army: Some(1), ..Default::default() };
    engine.start(&session).expect("session");
    // Keep the last Sync.Score before the beat's reset clears it.
    engine
        .sim_lua()
        .load("local reset = ResetSyncTable; ResetSyncTable = function() if Sync.Score then LastScore = Sync.Score end; reset() end")
        .exec()
        .expect("wrap ResetSyncTable");
    for _ in 0..100 {
        engine.tick().expect("tick");
    }
    let report: String = engine
        .sim_lua()
        .load(
            r#"
            local bad = {}
            local focus = GetFocusArmy()
            if focus == -1 then return 'GetFocusArmy is -1 (observer) for a player' end
            if SessionIsReplay() then return 'SessionIsReplay in a live game' end
            if not LastScore then return 'no Sync.Score sent' end
            local enemies = 0
            for index, brain in ArmyBrains do
                if not ArmyIsCivilian(index) then
                    local s = LastScore[index]
                    if not s then
                        table.insert(bad, index .. ': missing')
                    elseif index == focus or IsAlly(focus, index) then
                        if not (s.resources and s.resources.massin.rate) then table.insert(bad, index .. ': own/allied eco hidden') end
                    else
                        enemies = enemies + 1
                        if s.resources then table.insert(bad, index .. ': enemy eco visible') end
                    end
                end
            end
            if enemies == 0 then table.insert(bad, 'no enemy army to check') end
            return table.concat(bad, '; ')
            "#,
        )
        .eval()
        .expect("score sync");
    assert!(report.is_empty(), "{report}");
}

#[test]
fn thrust() {
    scenario("thrust", 200);
}

#[test]
fn overcharge_pause() {
    scenario("overcharge_pause", 400);
}

#[test]
fn attack_vectors() {
    scenario("attack_vectors", 200);
}

#[test]
fn pause() {
    scenario("pause", 600);
}

#[test]
fn navigator() {
    scenario("navigator", 600);
}

#[test]
fn split_prop() {
    scenario("split_prop", 100);
}

#[test]
fn restore_build_restrictions() {
    scenario("restore_build_restrictions", 100);
}

#[test]
fn stop_silo_build() {
    scenario("stop_silo_build", 200);
}

#[test]
fn carrier_storage() {
    scenario("carrier_storage", 100);
}

#[test]
fn staging_death() {
    scenario("staging_death", 600);
}

#[test]
fn damage_buff() {
    scenario("damage_buff", 100);
}

#[test]
fn fire_weapon() {
    scenario("fire_weapon", 100);
}

#[test]
fn debris_toss() {
    scenario("debris_toss", 100);
}

#[test]
fn collision_extents() {
    scenario("collision_extents", 100);
}

#[test]
fn bone_direction() {
    scenario("bone_direction", 100);
}

#[test]
fn battleship_barrels() {
    scenario("battleship_barrels", 300);
}

#[test]
fn naval_factory_arms() {
    scenario("naval_factory_arms", 100);
}

#[test]
fn alt_footprint() {
    scenario("alt_footprint", 1000);
}

#[test]
fn relative_position() {
    scenario("relative_position", 100);
}

#[test]
fn clear_focus() {
    scenario("clear_focus", 100);
}

#[test]
fn stats_trigger() {
    scenario("stats_trigger", 100);
}

#[test]
fn is_blip() {
    scenario("is_blip", 100);
}

#[test]
fn silo_economy() {
    scenario("silo_economy", 300);
}

#[test]
fn rebuild_bonus_ids() {
    scenario("rebuild_bonus_ids", 800);
}

#[test]
fn initial_rally() {
    scenario("initial_rally", 1700);
}

#[test]
fn occupy_rects() {
    scenario("occupy_rects", 500);
}

#[test]
fn rebuild_wreck() {
    scenario("rebuild_wreck", 400);
}

#[test]
fn assist_enhancement() {
    scenario("assist_enhancement", 7000);
}

#[test]
fn build_restriction_check() {
    scenario("build_restriction_check", 100);
}

/// A structure destroyed while being built stops its builders with
/// `OnStopBuild`, which takes their build beams down.
#[test]
fn build_target_destroyed() {
    scenario("build_target_destroyed", 600);
}

/// An engineer's assist ends once it has nothing to help with and an order
/// queued behind it, so a shift-queued build takes over.
#[test]
fn engineer_assist_queue() {
    scenario("engineer_assist_queue", 1500);
}

#[test]
fn deposits_around() {
    scenario("deposits_around", 50);
}

#[test]
fn point_defense_hits() {
    scenario("point_defense_hits", 4000);
}

#[test]
fn water_fire_only() {
    scenario("water_fire_only", 1000);
}

#[test]
fn cruiser_missiles() {
    scenario("cruiser_missiles", 500);
}

/// A player whose commander dies (assassination) is defeated: FAF's victory
/// condition takes the army out of the game (`SetArmyOutOfGame`) and calls
/// `brain:OnDefeat`, which shares its unit cap among its surviving allies
/// (lobby option `ShareUnitCap`, FAF's default `allies`).
#[test]
fn defeated_players_share_their_unit_cap() {
    if !have_data() {
        return;
    }
    let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let session = engine::session::SessionConfig { ticks: Some(0), create_initial_units: true, ai: true, ai_armies: 4, teams: 2, victory: Some("demoralization".into()), ..Default::default() };
    engine.start(&session).expect("session");
    for _ in 0..20 {
        engine.tick().expect("tick");
    }
    let before: String = engine
        .sim_lua()
        .load(
            r#"
            -- An army with a commander and an ally.
            for i, b in ArmyBrains do
                local acu = b:GetListOfUnits(categories.COMMAND, false)[1]
                local allied = false
                for j, _ in ArmyBrains do if j ~= i and not ArmyIsCivilian(j) and IsAlly(i, j) then allied = true end end
                if acu and allied and not rawget(_G, 'Victim') then rawset(_G, 'Victim', i) acu:Kill() end
            end
            local o = { 'victim ' .. tostring(rawget(_G, 'Victim')) }
            for i, b in ArmyBrains do if not ArmyIsCivilian(i) then
                local allies = {}
                for j, _ in ArmyBrains do if j ~= i and IsAlly(i, j) then table.insert(allies, j) end end
                table.insert(o, i .. '=' .. GetArmyUnitCap(i) .. ' acus ' .. table.getn(b:GetListOfUnits(categories.COMMAND, false) or {}) .. ' allies [' .. table.concat(allies, ',') .. ']')
            end end
            return table.concat(o, ' ')
            "#,
        )
        .eval()
        .expect("kill");
    assert!(!before.starts_with("victim nil"), "{before}");
    for _ in 0..100 {
        engine.tick().expect("tick");
    }
    let report: String = engine
        .sim_lua()
        .load(
            r#"
            local bad = {}
            local Victim = rawget(_G, 'Victim')
            if not ArmyBrains[Victim]:IsDefeated() then table.insert(bad, 'the victim is not defeated') end
            if not ArmyIsOutOfGame(Victim) then table.insert(bad, 'the victim is still in the game') end
            local allies, share = 0, 0
            for i, b in ArmyBrains do
                if i ~= Victim and not ArmyIsCivilian(i) and IsAlly(Victim, i) then allies = allies + 1 end
            end
            for i, b in ArmyBrains do
                if i ~= Victim and not ArmyIsCivilian(i) then
                    local want = IsAlly(Victim, i) and 500 + 500 / allies or 500
                    if math.abs(GetArmyUnitCap(i) - want) > 1 then table.insert(bad, i .. ': cap ' .. GetArmyUnitCap(i) .. ', want ' .. want) end
                end
            end
            if allies == 0 then table.insert(bad, 'the victim has no allies') end
            return table.concat(bad, '; ')
            "#,
        )
        .eval()
        .expect("caps");
    assert!(report.is_empty(), "before {before}: {report}");
}

/// A 1v1 under demoralization: one commander dies, FAF's victory condition
/// declares the other army the winner and calls `EndGame`. `IsGameOver`
/// turns true, and from the end of that tick the sim no longer advances
/// (moho's `AdvanceBeat` skips every later beat). No Lua errors on the way.
/// Unplayed slots, not AIs: a defeated AI's `ArmyPool` distress thread
/// errors in FAF's own `platoon.lua` (a separate issue).
#[test]
fn killing_a_commander_ends_the_game() {
    use tracing_subscriber::prelude::*;
    if !have_data() {
        return;
    }
    let errors = LuaErrors::default();
    let subscriber = tracing_subscriber::registry().with(errors.clone());
    tracing::subscriber::with_default(subscriber, || {
        let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
        let session = engine::session::SessionConfig { ticks: Some(0), create_initial_units: true, ai: false, ai_armies: 2, teams: 2, victory: Some("demoralization".into()), ..Default::default() };
        engine.start(&session).expect("session");
        for _ in 0..20 {
            engine.tick().expect("tick");
        }
        let lua = engine.sim_lua();
        let victim: i64 = lua
            .load(
                r#"
                for i, b in ArmyBrains do
                    local acu = not ArmyIsCivilian(i) and b:GetListOfUnits(categories.COMMAND, false)[1]
                    if acu then acu:Kill() return i end
                end
                "#,
            )
            .eval()
            .expect("kill");
        assert!(!lua.load("return IsGameOver()").eval::<bool>().unwrap(), "over before the victory condition ran");
        let mut over_at = None;
        for _ in 0..400 {
            engine.tick().expect("tick");
            if engine.sim_lua().load("return IsGameOver()").eval::<bool>().unwrap() {
                over_at = Some(engine.sim_lua().load("return GetGameTick()").eval::<i64>().unwrap());
                break;
            }
        }
        let over_at = over_at.expect("the game never ended");
        eprintln!("game over at tick {over_at}");
        let report: String = engine
            .sim_lua()
            .load(format!(
                r#"
                local bad = {{}}
                if not ArmyBrains[{victim}]:IsDefeated() then table.insert(bad, 'the victim is not defeated') end
                for i, b in ArmyBrains do
                    if i ~= {victim} and not ArmyIsCivilian(i) and b.Status ~= 'Victory' then table.insert(bad, i .. ' status ' .. tostring(b.Status)) end
                end
                return table.concat(bad, '; ')
                "#
            ))
            .eval()
            .expect("results");
        assert!(report.is_empty(), "{report}");
        for _ in 0..50 {
            engine.tick().expect("tick");
        }
        let after: i64 = engine.sim_lua().load("return GetGameTick()").eval().unwrap();
        assert_eq!(after, over_at, "the sim kept running after the game ended");
    });
    let errors: Vec<String> = errors.0.lock().unwrap().iter().filter(|m| !expected_noise(m)).cloned().collect();
    assert!(errors.is_empty(), "{} Lua errors, first ones:\n  {}", errors.len(), errors.iter().take(5).cloned().collect::<Vec<_>>().join("\n  "));
}

#[test]
fn ferry_beacon() {
    scenario("ferry_beacon", 100);
}

#[test]
fn ferry_service() {
    scenario("ferry_service", 3000);
}

#[test]
fn air_attitude() {
    scenario("air_attitude", 300);
}

#[test]
fn air_ground() {
    scenario("air_ground", 300);
}

#[test]
fn firing_solution() {
    scenario("firing_solution", 200);
}

#[test]
fn dual_arm_aim() {
    scenario("dual_arm_aim", 300);
}

#[test]
fn retarget_on_miss() {
    scenario("retarget_on_miss", 700);
}

#[test]
fn build_site_snap() {
    scenario("build_site_snap", 200);
}

#[test]
fn mobile_placement() {
    scenario("mobile_placement", 100);
}

#[test]
fn dispatch_hold() {
    scenario("dispatch_hold", 1200);
}
