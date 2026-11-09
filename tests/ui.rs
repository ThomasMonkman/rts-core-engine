//! The user layer (moho's UI Lua state) with FAF's real scripts: it boots
//! `/lua/userInit.lua`, and FAF's `maui` classes build controls whose
//! layout comes out of the draw list. Needs the FAF checkout and game data.

use std::sync::{Arc, Mutex};

use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine;
use rts_core_engine::ui::controls::DrawItem;
use rts_core_engine::ui::UserVm;

const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

fn vm() -> Option<UserVm> {
    let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).with_max_level(tracing::Level::WARN).try_init();
    // Fails without the game data (`RTS_SKIP_DATA_TESTS=1` skips instead).
    if !rts_core_engine::test_data::require_game("UI tests") {
        return None;
    }
    let engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let vm = UserVm::with_cache(engine.vfs.clone(), engine.script_cache.clone()).expect("user vm");
    vm.init().expect("userInit.lua");
    Some(vm)
}

#[test]
fn user_init_runs() {
    let Some(_vm) = vm() else { return };
}

/// FAF's in-game tabs label their exit `_Exit_to_Windows`: Windows
/// keeps that; elsewhere it reads like the main menu's `_Exit`.
#[test]
fn in_game_exit_label_matches_the_platform() {
    let Some(vm) = vm() else { return };
    let loc = |key: &str| -> String {
        vm.lua().load(format!("return LOC('{key}')")).eval().expect("LOC")
    };
    #[cfg(windows)]
    assert_eq!(loc("<LOC _Exit_to_Windows>"), "Exit to Windows");
    #[cfg(not(windows))]
    {
        assert_eq!(loc("<LOC _Exit_to_Windows>"), loc("<LOC _Exit>Exit"));
        assert_eq!(loc("<LOC _Exit_to_Windows>Exit to FAF"), loc("<LOC _Exit>Exit"));
    }
}

/// A fresh profile has FAF's option defaults, as the front end leaves it:
/// worldview reads the selection thresholds unguarded.
#[test]
fn fresh_profile_has_option_defaults() {
    let Some(vm) = vm() else { return };
    let tolerance: i64 = vm
        .lua()
        .load("return import('/lua/user/prefs.lua').GetFieldFromCurrentProfile('options').selection_threshold_regular")
        .eval()
        .expect("options");
    assert_eq!(tolerance, 10);
}

/// `GetOptions` reads the current profile's options, where FAF's options
/// dialog writes them (`Prefs.SetOption` -> `SetToCurrentProfile`).
#[test]
fn get_options_reads_the_profile() {
    let Some(vm) = vm() else { return };
    let eta: Option<bool> = vm
        .lua()
        .load("import('/lua/user/prefs.lua').SetOption('display_eta', false) return GetOptions('display_eta')")
        .eval()
        .expect("options");
    assert_eq!(eta, Some(false));
}

/// `Prefs.SetOption` runs the option's setter: `GetPreference` hands out
/// copies, so optionslogic sees the new value differ from the stored one.
#[test]
fn set_option_applies_the_option() {
    let Some(vm) = vm() else { return };
    vm.take_console().unwrap(); // (FAF's start-up settings)
    vm.lua().load("import('/lua/user/prefs.lua').SetOption('strat_icon_scale', 2)").exec().unwrap();
    assert_eq!(vm.take_console().unwrap(), vec!["ui_StrategicIconScale 2".to_string()]);
    let scale: f64 = vm.lua().load("return GetOptions('strat_icon_scale')").eval().unwrap();
    assert_eq!(scale, 2.0);
}

/// FAF's Bitmap / Text / LayoutHelpers place controls where FAF's layout
/// says: a bitmap at its texture's size, docked to the frame's corner, and a
/// text right of it sized by its font.
#[test]
fn maui_layout_comes_out_in_the_draw_list() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.lua()
        .load(
            r#"
            local Bitmap = import('/lua/maui/bitmap.lua').Bitmap
            local Text = import('/lua/maui/text.lua').Text
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local frame = GetFrame(0)
            local bg = Bitmap(frame, '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds')
            LayoutHelpers.AtLeftTopIn(bg, frame, 10, 20)
            local label = Text(bg)
            label:SetFont('Arial', 14)
            label:SetText('Mass 123')
            label:SetColor('ffb7e75f')
            LayoutHelpers.RightOf(label, bg, 4)
            "#,
        )
        .exec()
        .expect("build UI");
    let items = vm.draw_list().unwrap();
    eprintln!("{items:#?}");
    let quad = items.iter().find_map(|i| match i {
        DrawItem::Quad { rect, texture: Some(t), .. } => Some((rect, t)),
        _ => None,
    });
    let (rect, tex) = quad.expect("the bitmap draws");
    assert!(tex.contains("econ_bmp_m"));
    assert_eq!(rect[0], 10.0);
    assert_eq!(rect[1], 20.0);
    assert!(rect[2] > rect[0] && rect[3] > rect[1], "sized by its texture: {rect:?}");
    let text = items.iter().find_map(|i| match i {
        DrawItem::Text { rect, text, .. } => Some((rect, text)),
        _ => None,
    });
    let (trect, s) = text.expect("the text draws");
    assert_eq!(s, "Mass 123");
    assert!(trect[0] >= rect[2], "right of the bitmap: {trect:?} vs {rect:?}");
    assert!(trect[2] - trect[0] > 30.0, "width from the font: {trect:?}");
}

/// `GetResourceSharing` reads the focus army's overflow flag from the beat;
/// an observer gets false, as in moho.
#[test]
fn resource_sharing_is_the_focus_armys() {
    let Some(vm) = vm() else { return };
    let econ = |army, resource_sharing| rts_core_engine::engine::snapshot::ArmyEconomy { army, mass: [0.0; 6], energy: [0.0; 6], resource_sharing };
    vm.beat(1, &[econ(1, true), econ(2, false)]).expect("beat");
    let ask = |focus: i64| -> bool {
        vm.lua().load(format!("SetFocusArmy({focus}) return GetResourceSharing()")).eval().unwrap()
    };
    assert!(ask(0), "army 1 shares");
    assert!(!ask(1), "army 2 turned it off");
    assert!(!ask(-1), "observer");
}

/// FAF's own economy bar (`/lua/ui/game/economy.lua`, built as part of the
/// game interface) shows the economy after a beat.
#[test]
fn faf_economy_bar_builds() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("FAF economy bar");
    // Past the loading screen and the opening animations.
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).expect("frame");
    }
    let items = vm.draw_list().unwrap();
    eprintln!("economy bar: {} draw items", items.len());
    assert!(items.len() > 5);
    // A beat with an economy: FAF's beat function writes the numbers.
    let econ = rts_core_engine::engine::snapshot::ArmyEconomy { army: 1, mass: [500.0, 650.0, 1.3, 2.0, 0.8, 120.0], energy: [3000.0, 4000.0, 20.0, 5.0, 5.0, 0.0], resource_sharing: true };
    vm.beat(50, &[econ]).expect("beat");
    vm.frame(0.1).expect("frame");
    let texts = |vm: &rts_core_engine::ui::UserVm| -> Vec<String> {
        vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
            DrawItem::Text { text, .. } => Some(text),
            _ => None,
        }).collect()
    };
    let shown = texts(&vm);
    eprintln!("economy texts: {shown:?}");
    assert!(shown.iter().any(|t| t.contains("500")), "stored mass shown: {shown:?}");
    assert!(shown.iter().any(|t| t == "120"), "reclaimed mass total shown: {shown:?}");
    // 3 more mass reclaimed each tick: +30/s in the reclaim column, and
    // the income column still shows the 13/s generated: the sim's income
    // includes the last tick's reclaim (4.3 a tick), which FAF's panel
    // takes back out (its first beat counts the whole starting total as one
    // tick's reclaim, so look at the third).
    for (tick, total) in [(51, 123.0), (52, 126.0)] {
        let econ = rts_core_engine::engine::snapshot::ArmyEconomy { mass: [500.0, 650.0, 4.3, 2.0, 0.8, total], ..econ };
        vm.beat(tick, &[econ]).expect("beat");
        vm.frame(0.1).expect("frame");
    }
    let shown = texts(&vm);
    eprintln!("economy texts: {shown:?}");
    assert!(shown.iter().any(|t| t == "+30"), "reclaim rate: {shown:?}");
    assert!(shown.iter().any(|t| t == "+13"), "generated income unchanged by reclaim: {shown:?}");
}

#[test]
#[ignore]
fn debug_doscript() {
    let Some(vm) = vm() else { return };
    let path = std::env::var("UI_DOSCRIPT").unwrap_or_default();
    let r: String = vm.lua().load(format!("local ok, e = pcall(import, '{path}') return tostring(ok) .. ' ' .. tostring(e)")).eval().unwrap();
    eprintln!("DOSCRIPT {r}");
}

/// FAF's whole in-game interface (`gamemain.CreateUI`), unchanged.
#[test]
fn faf_game_ui_builds() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("the game interface");
    // Seven seconds of frames: the loading screen fades (1.5 s + 2 s), then
    // the interface's opening animations run.
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).expect("frame");
    }
    let items = vm.draw_list().unwrap();
    eprintln!("game UI: {} draw items", items.len());
    assert!(items.len() > 50);
}

/// Mouse input: a FAF `Button` gets its click (press posts a dragger,
/// release over it fires `OnClick`), clicks over the world view are the
/// world's, clicks on panels are the interface's.
#[test]
fn clicks_reach_faf_buttons_and_the_world_gets_the_rest() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.lua()
        .load(
            r#"
            local Button = import('/lua/maui/button.lua').Button
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local tex = '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds'
            local b = Button(GetFrame(0), tex, tex, tex, tex)
            LayoutHelpers.AtLeftTopIn(b, GetFrame(0), 600, 300)
            b.Width:Set(40) b.Height:Set(20)
            b.Depth:Set(100000)
            rawset(_G, 'Clicked', 0)
            b.OnClick = function(self) rawset(_G, 'Clicked', rawget(_G, 'Clicked') + 1) end
            "#,
        )
        .exec()
        .unwrap();
    let left = Modifiers { left: true, ..Default::default() };
    assert!(vm.mouse(MouseEvent::Move, 610.0, 305.0, Modifiers::default()).unwrap(), "over the button");
    assert!(vm.mouse(MouseEvent::Press(1), 610.0, 305.0, left).unwrap());
    assert!(vm.mouse(MouseEvent::Release(1), 610.0, 305.0, Modifiers::default()).unwrap());
    let clicked: i64 = vm.lua().globals().raw_get("Clicked").unwrap();
    assert_eq!(clicked, 1, "OnClick ran once");
    // Mid-screen, away from panels: the world view, so not the interface's.
    assert!(!vm.mouse(MouseEvent::Press(1), 640.0, 500.0, left).unwrap(), "clicks on the world go to the world");
    vm.mouse(MouseEvent::Release(1), 640.0, 500.0, Modifiers::default()).unwrap();
    // The economy bar (top left) is the interface's.
    assert!(vm.mouse(MouseEvent::Press(1), 60.0, 20.0, left).unwrap(), "clicks on a panel stay in the interface");
}

/// Selecting units runs FAF's `OnSelectionChanged`: the unit panels (orders,
/// construction, unit view) appear for a selected engineer.
#[test]
fn selecting_an_engineer_shows_faf_unit_panels() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let before = vm.draw_list().unwrap().len();
    vm.update_units(&[UnitInfo { id: 7, blueprint: "uel0105".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.set_selection(&[7]).expect("OnSelectionChanged");
    for _ in 0..60 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let after = vm.draw_list().unwrap().len();
    eprintln!("draw items: {before} without a selection, {after} with an engineer selected");
    assert!(after > before + 20, "the unit panels appeared ({before} -> {after})");
    vm.set_selection(&[]).expect("deselect");
}

/// Orders from the interface come out for the engine: a factory build
/// (`IssueBlueprintCommand`), a stop, and FAF's command mode for a targeted
/// order.
#[test]
fn interface_orders_reach_the_engine() {
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    vm.lua()
        .load(
            r#"
            IssueBlueprintCommand('UNITCOMMAND_BuildFactory', 'uel0201', 3, false)
            IssueCommand(GetUnitCommandFromCommandCap('RULEUCC_Stop'))
            import('/lua/ui/game/commandmode.lua').StartCommandMode('order', { name = 'RULEUCC_Attack' })
            "#,
        )
        .exec()
        .unwrap();
    let orders = vm.take_orders().unwrap();
    assert_eq!(orders.len(), 2, "{orders:?}");
    assert_eq!((orders[0].command.as_str(), orders[0].blueprint.as_deref(), orders[0].count), ("UNITCOMMAND_BuildFactory", Some("uel0201"), 3));
    assert_eq!(rts_core_engine::ui::command_kind(&orders[1].command), Some(rts_core_engine::engine::sim::command::CommandKind::Stop));
    assert!(vm.take_orders().unwrap().is_empty(), "taken once");
    assert_eq!(vm.command_mode().unwrap(), Some(("order".to_string(), "RULEUCC_Attack".to_string())));
    vm.end_command_mode(true).unwrap();
    assert_eq!(vm.command_mode().unwrap(), None);
}

/// An order given in FAF's command mode is reported back
/// (`OnCommandIssued`): FAF ends the mode, unless the order was shift-queued.
#[test]
fn issued_orders_end_command_mode() {
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget};
    use rts_core_engine::engine::sim::math::FxVec3;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    // (The interface fades in over its first seconds.)
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    // The mouse over the world (FAF's area-attack dragger hit-tests views).
    vm.mouse(rts_core_engine::ui::controls::MouseEvent::Move, 640.0, 500.0, Default::default()).unwrap();
    let start = "import('/lua/ui/game/commandmode.lua').StartCommandMode('order', { name = 'RULEUCC_Attack' })";
    vm.lua().load(start).exec().unwrap();
    let at = CommandTarget::Ground(FxVec3::from_f64(100.0, 10.0, 100.0));
    vm.command_issued(CommandKind::Attack, at, true, None, &[], &[]).unwrap();
    assert!(vm.command_mode().unwrap().is_some(), "a queued order keeps the mode");
    vm.command_issued(CommandKind::Attack, at, false, None, &[], &[]).unwrap();
    assert_eq!(vm.command_mode().unwrap(), None, "an order ends it");
}

/// Pause both ways: FAF's pause button asks the engine
/// (`SessionRequestPause`), and an engine pause runs FAF's OnPause /
/// OnUserPause (`SessionIsPaused` follows).
#[test]
fn pause_reaches_both_sides() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    vm.lua().load("SessionRequestPause()").exec().unwrap();
    assert_eq!(vm.take_pause_request().unwrap(), Some(true));
    assert_eq!(vm.take_pause_request().unwrap(), None, "taken once");
    vm.set_paused(true).expect("FAF's OnPause / OnUserPause run");
    assert!(vm.lua().load("return SessionIsPaused()").eval::<bool>().unwrap());
    vm.set_paused(false).expect("FAF's OnResume runs");
    assert!(!vm.lua().load("return SessionIsPaused()").eval::<bool>().unwrap());
}

/// Shift's command graph and a window resize reach FAF without errors
/// (`OnCommandGraphShow`, `OnApplicationResize`).
#[test]
fn command_graph_and_resize_run() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    vm.command_graph_show(true).expect("FAF's OnCommandGraphShow(true)");
    vm.command_graph_show(false).expect("FAF's OnCommandGraphShow(false)");
    vm.application_resized(1600.0, 900.0).expect("FAF's OnApplicationResize");
}

/// Keys run FAF's key map: S on a selected tank is FAF's stop (an order
/// for the engine); unbound keys are nobody's.
#[test]
fn keys_run_faf_key_actions() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    // The first frame unlocks input (FAF's OnFirstUpdate ends the start-up input lock).
    vm.frame(1.0 / 60.0).unwrap();
    vm.update_units(&[UnitInfo { id: 3, blueprint: "uel0201".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.set_selection(&[3]).unwrap();
    vm.take_orders().unwrap();
    assert!(vm.key("S", false, false, false).unwrap(), "S is bound");
    let orders = vm.take_orders().unwrap();
    assert!(orders.iter().any(|o| rts_core_engine::ui::command_kind(&o.command) == Some(rts_core_engine::engine::sim::command::CommandKind::Stop)), "{orders:?}");
    assert!(!vm.key("Pause", true, true, true).unwrap_or(false), "Ctrl-Alt-Shift-Pause isn't bound");
    // Ctrl-1 / 1: FAF's control group 1 holds and restores the selection.
    assert!(vm.key("1", true, false, false).unwrap(), "Ctrl-1 sets group 1");
    vm.set_selection(&[]).unwrap();
    vm.take_selection_request().unwrap();
    assert!(vm.key("1", false, false, false).unwrap(), "1 recalls group 1");
    assert_eq!(vm.take_selection_request().unwrap(), Some(vec![3]), "group 1 reselected");
}

/// A player's Game.prefs key bindings apply (FAF's keymapper merges the
/// profile's UserKeyMap over the preset): K bound to stop stops; console
/// actions (control groups) run through FAF's selection sets.
#[test]
fn game_prefs_key_bindings_apply() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    let path = std::env::temp_dir().join(format!("rts-core-engine-test-{}.prefs", std::process::id()));
    std::fs::write(&path, "profile = {\n    current = 'player',\n    profiles = {\n        player = { Name = 'Player', UserKeyMap = { ['K'] = 'stop' } },\n    },\n}\n").unwrap();
    vm.load_prefs(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    // The first frame unlocks input (FAF's OnFirstUpdate ends the start-up input lock).
    vm.frame(1.0 / 60.0).unwrap();
    vm.update_units(&[UnitInfo { id: 3, blueprint: "uel0201".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.set_selection(&[3]).unwrap();
    vm.take_orders().unwrap();
    assert!(vm.key("K", false, false, false).unwrap(), "K is bound by the prefs");
    let orders = vm.take_orders().unwrap();
    assert!(orders.iter().any(|o| rts_core_engine::ui::command_kind(&o.command) == Some(rts_core_engine::engine::sim::command::CommandKind::Stop)), "{orders:?}");
    // (A profile's UserKeyMap is the whole map, as in FAF: S isn't bound.)
    assert!(!vm.key("S", false, false, false).unwrap());
    // Engine console commands are handed to the renderer.
    vm.take_console().unwrap(); // (FAF's start-up settings)
    vm.console("UI_SelectByCategory +idle ENGINEER").unwrap();
    assert_eq!(vm.take_console().unwrap(), vec!["UI_SelectByCategory +idle ENGINEER".to_string()]);
}

/// FAF's camera API reads the renderer's camera and asks it to move: zoom
/// slider keys (W zooms out), SetZoom, UIZoomTo (control group double tap),
/// and ConExecute reaches the console (FAF's zoom speed option).
#[test]
fn camera_api_drives_the_camera() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    // The first frame unlocks input (FAF's OnFirstUpdate ends the start-up input lock).
    vm.frame(1.0 / 60.0).unwrap();
    vm.set_camera_states(&[rts_core_engine::ui::CameraState { name: "WorldCamera".into(), focus: [500.0, 20.0, 500.0], zoom: 200.0, min: 8.0, max: 1100.0, pitch: 60f32.to_radians(), heading: std::f32::consts::PI }]).unwrap();
    // FAF's world view asks for its zoom-out room (gamemain.defaultZoom);
    // the minimap's own 1.0 doesn't reach the main camera.
    let start = vm.take_camera_requests().unwrap();
    assert_eq!(start.iter().filter_map(|r| r.max_zoom_mult).collect::<Vec<_>>(), [1.4]);
    let zoom: f64 = vm.lua().load("return GetCamera('WorldCamera'):GetZoom()").eval().unwrap();
    assert_eq!(zoom, 200.0);
    // A live camera (FAF's reclaim labels only update while it is).
    let destroyed: bool = vm.lua().load("return IsDestroyed(GetCamera('WorldCamera'))").eval().unwrap();
    assert!(!destroyed, "the world camera reads as destroyed");
    vm.lua().load("GetCamera('WorldCamera'):SetZoom(80, 0)").exec().unwrap();
    let r = vm.take_camera_requests().unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].zoom, r[0].camera.as_deref()), (Some(80.0), Some("WorldCamera")));
    assert!(vm.key("W", false, false, false).unwrap(), "W is FAF's zoom out");
    let r = vm.take_camera_requests().unwrap();
    assert!(r.iter().any(|r| r.zoom.is_some_and(|z| z > 200.0)), "zoomed out: {r:?}");
    vm.update_units(&[UnitInfo { id: 3, blueprint: "uel0201".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [300.0, 20.0, 700.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.lua().load("UIZoomTo({ GetUnitById('3') }, 0)").exec().unwrap();
    let r = vm.take_camera_requests().unwrap();
    assert_eq!(r.last().and_then(|r| r.focus).map(|f| (f[0], f[2])), Some((300.0, 700.0)));
    // TrackEntities takes user units or the sim's entity ids
    // (`usercamera.lua`'s CAMERA_TRACK_ENTITIES), for any named camera.
    // (A world view makes its camera, as moho's do.)
    vm.lua().load("local Control = import('/lua/maui/control.lua').Control; local V = ClassUI(moho.UIWorldView, Control) {}; V(GetFrame(0), 'PiP1', 1, false)").exec().unwrap();
    vm.lua().load("GetCamera('PiP1'):TrackEntities({ GetUnitById('3'), '7', 9 }, 40, 2)").exec().unwrap();
    let r = vm.take_camera_requests().unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].camera.as_deref(), r[0].track.as_deref(), r[0].zoom, r[0].seconds), (Some("PiP1"), Some(&[3, 7, 9][..]), Some(40.0), 2.0));
    vm.lua().load("GetCamera('PiP1'):TrackEntities({})").exec().unwrap();
    assert!(vm.take_camera_requests().unwrap().is_empty(), "nothing to follow asks for nothing");
    vm.take_console().unwrap(); // (FAF's start-up settings)
    vm.lua().load("ConExecute('cam_ZoomAmount 0.5')").exec().unwrap();
    assert_eq!(vm.take_console().unwrap(), vec!["cam_ZoomAmount 0.5".to_string()]);
}

/// `SnapTo` / `MoveTo` hand the camera their orientation (heading, pitch,
/// radians; the roll is unused), as FAF's camera markers, zoom pop and
/// `RestoreCameraPosition` give it, and `SaveSettings` reads the pitch back
/// in radians for `RestoreSettings` / `MoveTo` (moho 0x7b02d0, 0x7b44f0,
/// 0x7b46e0).
#[test]
fn camera_orientation_reaches_the_camera() {
    let Some(vm) = vm() else { return };
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    vm.frame(1.0 / 60.0).unwrap();
    vm.set_camera_states(&[rts_core_engine::ui::CameraState { name: "WorldCamera".into(), focus: [500.0, 20.0, 500.0], zoom: 200.0, min: 8.0, max: 1100.0, pitch: 1.2, heading: std::f32::consts::PI }]).unwrap();
    vm.take_camera_requests().unwrap();
    // A table (a marker's orientation) or a Vector (FAF's zoom pop).
    vm.lua().load("GetCamera('WorldCamera'):SnapTo({ 386, 21, 700 }, { 1.5, 0.2, 0 }, 18)").exec().unwrap();
    vm.lua().load("GetCamera('WorldCamera'):SnapTo(Vector(10, 0, 20), Vector(3.1, 1.22, 0), 80)").exec().unwrap();
    vm.lua().load("GetCamera('WorldCamera'):MoveTo({ 300, 0, 300 }, { -1.5, 0.6, 0 }, 50, 2)").exec().unwrap();
    // `MoveTo(rect, seconds)` (FAF's SimSync) has no orientation.
    vm.lua().load("GetCamera('WorldCamera'):MoveTo({ 300, 0, 300 }, 5.0)").exec().unwrap();
    let r = vm.take_camera_requests().unwrap();
    assert_eq!(r.len(), 4);
    assert_eq!((r[0].focus, r[0].zoom, r[0].seconds, r[0].rotation), (Some([386.0, 21.0, 700.0]), Some(18.0), 0.0, Some((1.5, 0.2))));
    assert_eq!((r[1].focus, r[1].rotation), (Some([10.0, 0.0, 20.0]), Some((3.1, 1.22))));
    assert_eq!((r[2].zoom, r[2].seconds, r[2].rotation), (Some(50.0), 2.0, Some((-1.5, 0.6))));
    assert_eq!(r[3].rotation, None);
    // SaveSettings -> RestoreSettings round-trips the heading and pitch.
    let pitch: f64 = vm.lua().load("return GetCamera('WorldCamera'):SaveSettings().Pitch").eval().unwrap();
    assert!((pitch - 1.2).abs() < 1e-6, "radians: {pitch}");
    vm.lua().load("local c = GetCamera('WorldCamera'); c:RestoreSettings(c:SaveSettings())").exec().unwrap();
    let r = vm.take_camera_requests().unwrap();
    assert_eq!((r[0].focus, r[0].zoom, r[0].rotation), (Some([500.0, 20.0, 500.0]), Some(200.0), Some((std::f32::consts::PI, 1.2))));
}

/// FAF's minimap window shows its world view as a minimap draw item.
#[test]
fn faf_minimap_is_drawn() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let state: String = vm
        .lua()
        .load(
            r#"
            local mm = import('/lua/ui/game/minimap.lua')
            local c = mm.controls
            local out = {}
            local m = c.miniMap
            if not m then return 'no minimap control' end
            local p = m
            while p do
                table.insert(out, string.format('%s hidden=%s %s,%s %sx%s', tostring(p:GetName()), tostring(p:IsHidden()), tostring(p.Left()), tostring(p.Top()), tostring(p.Width()), tostring(p.Height())))
                p = p:GetParent()
            end
            return table.concat(out, ' | ')
            "#,
        )
        .eval()
        .unwrap();
    eprintln!("minimap: {state}");
    let items = vm.draw_list().unwrap();
    assert!(items.iter().any(|i| matches!(i, DrawItem::Minimap { .. })), "no minimap item: {state}");
}

/// FAF's split view (`borders.SplitMapGroup(true)`, its key action and the
/// map dialog's button): two world views side by side, `WorldCamera` on the
/// left and `WorldCamera2` on the right, each a draw item the renderer draws
/// its camera into; each camera object asks for its own moves, and the
/// topmost view under a point is the one there. Back to one view after.
#[test]
fn split_view_shows_two_world_cameras() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1600.0, 900.0).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..60 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let worlds = |vm: &UserVm| -> Vec<(String, [f32; 4])> {
        vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
            DrawItem::World { rect, camera } => Some((camera, rect)),
            _ => None,
        }).collect()
    };
    let one = worlds(&vm);
    assert_eq!(one.len(), 1, "one main view: {one:?}");
    assert_eq!((one[0].0.as_str(), one[0].1), ("WorldCamera", [0.0, 0.0, 1600.0, 900.0]), "the main view fills the screen");
    vm.lua().load("import('/lua/ui/game/borders.lua').SplitMapGroup(true, true)").exec().unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    let two = worlds(&vm);
    assert_eq!(two.iter().map(|w| w.0.as_str()).collect::<Vec<_>>(), ["WorldCamera", "WorldCamera2"], "{two:?}");
    let (left, right) = (two[0].1, two[1].1);
    assert!(left[0] == 0.0 && right[2] == 1600.0 && left[2] < 800.0 && right[0] > 800.0, "side by side: {two:?}");
    vm.lua().load("GetCamera('WorldCamera2'):SetZoom(90, 0)").exec().unwrap();
    let r = vm.take_camera_requests().unwrap();
    assert!(r.iter().any(|r| r.zoom == Some(90.0) && r.camera.as_deref() == Some("WorldCamera2")), "{r:?}");
    let cameras: (String, String) = vm.lua().load("local at = import('/lua/ui/game/worldview.lua').GetTopmostWorldViewAt local l, r = at(100, 450), at(1500, 450) return l._cameraName, r._cameraName").eval().unwrap();
    assert_eq!(cameras, ("WorldCamera".to_string(), "WorldCamera2".to_string()), "the view under each point");
    vm.lua().load("import('/lua/ui/game/borders.lua').SplitMapGroup(false, true)").exec().unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    assert_eq!(worlds(&vm).len(), 1, "back to one view");
}

/// A second head (FAF's Secondary Adapter): `GetNumRootFrames` is 2, and
/// as the game interface starts FAF's `multihead.CreateSecondView` clears
/// head 1 and fills it with a `CameraHead2` world view: head 1's own draw
/// list, at head 1's size, while head 0 keeps the main view. Mouse input
/// from head 1 hits only its controls; `ClearFrame(1)` empties it.
#[test]
fn second_head_shows_camera_head2() {
    let Some(vm) = vm() else { return };
    vm.set_heads(2);
    vm.set_screen(1600.0, 900.0).unwrap();
    vm.set_head_screen(1, 1280.0, 720.0).unwrap();
    assert_eq!(vm.lua().load("return GetNumRootFrames()").eval::<i64>().unwrap(), 2);
    vm.start_hud().expect("game interface");
    for _ in 0..60 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let worlds = |items: Vec<DrawItem>| -> Vec<(String, [f32; 4])> {
        items.into_iter().filter_map(|i| match i {
            DrawItem::World { rect, camera } => Some((camera, rect)),
            _ => None,
        }).collect()
    };
    assert_eq!(worlds(vm.draw_list().unwrap()), [("WorldCamera".to_string(), [0.0, 0.0, 1600.0, 900.0])], "head 0: the main view");
    assert_eq!(worlds(vm.head_draw_list(1).unwrap()), [("CameraHead2".to_string(), [0.0, 0.0, 1280.0, 720.0])], "head 1: the second view, filling it");
    let head: i64 = vm.lua().load("local v = import('/lua/ui/game/multihead.lua').view return v:GetRootFrame() == GetFrame(1) and 1 or 0").eval().unwrap();
    assert_eq!(head, 1, "the view's root frame is head 1's");
    // Which view FAF's `GetTopmostWorldViewAt` finds under head 1's mouse:
    // (both heads' views pass `HitTest` there).
    vm.lua().load("ClearFrame(1)").exec().unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    assert!(vm.head_draw_list(1).unwrap().is_empty(), "ClearFrame(1) empties head 1");
    assert_eq!(worlds(vm.draw_list().unwrap()).len(), 1, "and leaves head 0");
}

/// A selected ACU's overcharge pause reaches `UserUnit:IsOverchargePaused`
/// (FAF's orders panel greys the overcharge button with it).
#[test]
fn overcharge_pause_reaches_the_interface() {
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    let acu = |overcharge_paused| UnitInfo { id: 7, blueprint: "uel0001".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: Some(UnitDetail { id: 7, overcharge_paused, ..Default::default() }) };
    let read = || vm.lua().load("return __engine_unit(7):IsOverchargePaused()").eval::<bool>().unwrap();
    vm.update_units(&[acu(true)]).unwrap();
    assert!(read());
    vm.update_units(&[acu(false)]).unwrap();
    assert!(!read());
}

/// A factory's `UserUnit:GetCommandQueue()` is its factory command queue
/// (its rally orders), as moho's for a FACTORY: FAF's `rallypoint.lua`
/// flags the last of them. Its builds stay out of it (they still feed
/// the construction panel and `IsIdle`); another unit's is its own queue.
#[test]
fn a_factorys_command_queue_is_its_rally_orders() {
    use rts_core_engine::engine::sim::command::CommandKind;
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    let build = (CommandKind::BuildFactory as u8, Some("uel0201".to_string()), 2, [0.0; 3]);
    let rally = vec![(CommandKind::Move as u8, [410.0, 20.0, 600.0]), (CommandKind::Patrol as u8, [430.0, 20.0, 600.0]), (CommandKind::Patrol as u8, [430.0, 20.0, 620.0])];
    let factory = UnitInfo { id: 7, blueprint: "ueb0101".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: Some(UnitDetail { id: 7, commands: vec![build.clone()], factory_commands: rally, ..Default::default() }) };
    let tank = UnitInfo { id: 8, blueprint: "uel0201".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 610.0], work: 0.0, shield: 0.0, detail: Some(UnitDetail { id: 8, commands: vec![(CommandKind::Move as u8, None, 0, [500.0, 20.0, 500.0])], ..Default::default() }) };
    vm.update_units(&[factory, tank]).unwrap();
    let queue = |id: u32| -> Vec<String> {
        vm.lua()
            .load("local out = {} for _, c in __engine_unit(...):GetCommandQueue() do table.insert(out, c.type .. ' ' .. c.position[1] .. ' ' .. c.position[3]) end return out")
            .call(id)
            .unwrap()
    };
    assert_eq!(queue(7), vec!["Move 410 600", "Patrol 430 600", "Patrol 430 620"]);
    assert_eq!(queue(8), vec!["Move 500 500"]);
    let idle: bool = vm.lua().load("return __engine_unit(7):IsIdle()").eval().unwrap();
    assert!(!idle, "building: busy");
}

/// A unit killed but still falling or sinking (`Dying`) leaves the
/// interface on the next beat: `UserUnit:IsDead()` turns true, while a live
/// unit held beside it stays.
#[test]
fn dying_units_are_dead_to_the_interface() {
    use rts_core_engine::engine::snapshot::{EntityView, Sighting, ViewKind};
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    let view = |id, kind| EntityView { id, kind, army: 1, position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], center: [0.0; 3], half: [1.0; 3], health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, work: 0.0, shield: -1.0, blueprint: 0, mesh: 0, scale: [1.0; 3], pose_start: 0, pose_len: 0, sighting: Sighting::Seen, in_builder: false, attached: false, scroll: [0.0; 4], order_flags: 0 };
    let info = |e: &EntityView| UnitInfo { id: e.id, blueprint: "uea0102".into(), army: e.army, health: e.health, max_health: e.max_health, build_rate: e.build_rate, build: e.build, position: e.position, work: 0.0, shield: 0.0, detail: None };
    let dead = |id: u32| vm.lua().load(format!("return __engine_unit({id}):IsDead()")).eval::<bool>().unwrap();
    let frame = [view(7, ViewKind::Unit), view(8, ViewKind::Unit)];
    vm.update_units(&frame.iter().map(info).collect::<Vec<_>>()).unwrap();
    assert!(!dead(7) && !dead(8));
    // Unit 8 is shot down: the next snapshot draws it falling.
    let frame = [view(7, ViewKind::Unit), view(8, ViewKind::Dying)];
    vm.refresh_held_units(&[], |ids| frame.iter().filter(|e| ids.contains(&e.id) && e.is_interface_unit()).map(info).collect()).unwrap();
    assert!(!dead(7));
    assert!(dead(8));
}

/// Category tests on UserUnits read the unit's blueprint (the engine's
/// direct path through its unit data): FAF's panels filter whole
/// selections, and a dead unit is in no category.
#[test]
fn user_units_answer_category_tests() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    let info = |id, bp: &str| UnitInfo { id, blueprint: bp.into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: None };
    vm.update_units(&[info(3, "uel0201"), info(4, "uea0102")]).unwrap();
    let eval = |code: &str| vm.lua().load(code).eval::<String>().unwrap();
    let units = "local u = { __engine_unit(3), __engine_unit(4) }";
    assert_eq!(eval(&format!("{units} local out = '' for _, x in EntityCategoryFilterDown(categories.AIR, u) do out = out .. x:GetEntityId() end return out")), "4");
    assert_eq!(eval(&format!("{units} return tostring(EntityCategoryContains(categories.LAND, u[1])) .. tostring(EntityCategoryContains(categories.LAND, u[2]))")), "truefalse");
    assert_eq!(eval("return tostring(__engine_unit(3):IsInCategory('TECH1'))"), "true");
    vm.remove_units(&[3]).unwrap();
    assert_eq!(eval("return tostring(EntityCategoryContains(categories.LAND, __engine_unit(3)))"), "false");
}

/// Unit details reach FAF's API: a factory's build orders stack into the
/// construction panel's queue, a unit with orders isn't idle, and its work
/// progress and fire state read back.
#[test]
fn unit_details_feed_the_panels() {
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    let bf = rts_core_engine::engine::sim::command::CommandKind::BuildFactory as u8;
    let detail = UnitDetail {
        id: 9,
        commands: vec![(bf, Some("uel0201".into()), 2, [0.0; 3]), (bf, Some("uel0201".into()), 1, [0.0; 3]), (bf, Some("uel0105".into()), 1, [0.0; 3])],
        fire_state: 1,
        ..Default::default()
    };
    vm.update_units(&[UnitInfo { id: 9, blueprint: "ueb0101".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [0.0; 3], work: 0.25, shield: 0.0, detail: Some(detail) }]).unwrap();
    let (queue, idle, work, fire): (String, bool, f64, i64) = vm
        .lua()
        .load(
            r#"
            local f = __engine_unit(9)
            local q = SetCurrentFactoryForQueueDisplay(f)
            local s = {}
            for _, e in q do table.insert(s, e.id .. 'x' .. e.count) end
            return table.concat(s, ','), f:IsIdle(), f:GetWorkProgress(), GetFireState({ f })
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(queue, "uel0201x3,uel0105x1");
    assert!(!idle);
    assert!((work - 0.25).abs() < 1e-6);
    assert_eq!(fire, 1);
    // A tank done: FAF hears the new queue (OnQueueChanged).
    vm.lua()
        .load("local gm = import('/lua/ui/game/gamemain.lua') local orig = gm.OnQueueChanged gm.OnQueueChanged = function(q) rawset(_G, 'NewQueue', q) return orig(q) end")
        .exec()
        .unwrap();
    let detail = UnitDetail { id: 9, commands: vec![(bf, Some("uel0201".into()), 2, [0.0; 3]), (bf, Some("uel0105".into()), 1, [0.0; 3])], ..Default::default() };
    vm.update_units(&[UnitInfo { id: 9, blueprint: "ueb0101".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: Some(detail) }]).unwrap();
    let heard: String = vm.lua().load("local s = {} for _, e in (NewQueue or {}) do table.insert(s, e.id .. 'x' .. e.count) end return table.concat(s, ',')").eval().unwrap();
    assert_eq!(heard, "uel0201x2,uel0105x1", "OnQueueChanged with the new queue");
    // The panel's queue edits go to the sim for the shown factory, by entry.
    vm.take_orders().unwrap();
    vm.lua().load("IncreaseBuildCountInQueue(2, 5) DecreaseBuildCountInQueue(1, 1)").exec().unwrap();
    use rts_core_engine::engine::sim::input::Toggle;
    let edits: Vec<_> = vm.take_orders().unwrap().into_iter().map(|o| (o.units, o.toggle)).collect();
    assert_eq!(edits, [(Some(vec![9]), Some(Toggle::BuildCount(2, 5))), (Some(vec![9]), Some(Toggle::BuildCount(1, -1)))]);
}

/// Unit statistics (veterancy level / experience, ...) reach the interface:
/// FAF's unit panels read them through `UserUnit:GetStat(name, default)`.
#[test]
fn unit_stats_read_back_through_get_stat() {
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    let detail = UnitDetail {
        id: 3,
        stats: vec![("VetExperience".into(), 112.0), ("VetLevel".into(), 1.0)],
        ..Default::default()
    };
    vm.update_units(&[UnitInfo { id: 3, blueprint: "uel0201".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: Some(detail) }]).unwrap();
    let (level, exp, missing): (f64, f64, f64) = vm
        .lua()
        .load("local u = __engine_unit(3) return u:GetStat('VetLevel', 0).Value, u:GetStat('VetExperience', 0).Value, u:GetStat('Nope', 42).Value")
        .eval()
        .unwrap();
    assert_eq!((level, exp, missing), (1.0, 112.0, 42.0));
}

/// The sim's Sync reaches FAF's OnSync: a `PrintText` entry is printed on
/// screen by FAF's text display.
#[test]
fn sim_sync_reaches_faf_on_sync() {
    use rts_core_engine::engine::sync::SyncValue as V;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let s = |x: &str| V::Str(x.into());
    let entry = V::Table(vec![(s("text"), s("Hello from the sim")), (s("size"), V::Num(20.0)), (s("color"), s("ffffffff")), (s("duration"), V::Num(5.0)), (s("location"), s("center"))]);
    let sync = V::Table(vec![(s("PrintText"), V::Table(vec![(V::Num(1.0), entry)]))]);
    vm.sync(&sync).expect("OnSync");
    for _ in 0..30 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let shown = vm.draw_list().unwrap().into_iter().any(|i| matches!(i, DrawItem::Text { text, .. } if text.contains("Hello from the sim")));
    assert!(shown, "FAF printed the sim's text");
}

/// Cost of a UI frame with FAF's whole interface up and a unit selected
/// (user threads, OnFrame, draw list). Timing only:
/// `cargo test --release --test ui ui_frame_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn ui_frame_cost() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1920.0, 1080.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.update_units(&[UnitInfo { id: 7, blueprint: "uel0105".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.set_selection(&[7]).unwrap();
    // The mouse over the world: FAF's cursor update runs every frame.
    vm.mouse(rts_core_engine::ui::controls::MouseEvent::Move, 960.0, 700.0, Default::default()).unwrap();
    let t = std::time::Instant::now();
    let mut items = 0;
    let n: u32 = std::env::var("UI_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    for _ in 0..n {
        vm.frame(1.0 / 60.0).unwrap();
        items = vm.draw_list().unwrap().len();
    }
    eprintln!("ui_frame_cost: {:.3} ms per frame (frame + draw list), {items} draw items", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
}

/// A selection made while the interface is still behind its loading screen
/// shows its unit panels once the interface is up.
#[test]
fn early_selection_shows_unit_panels() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    vm.update_units(&[UnitInfo { id: 7, blueprint: "uel0001".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    vm.set_selection(&[7]).unwrap();
    for _ in 0..480 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let with = vm.draw_list().unwrap().len();
    vm.set_selection(&[]).unwrap();
    for _ in 0..30 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let without = vm.draw_list().unwrap().len();
    eprintln!("early selection: {with} items selected, {without} without");
    assert!(with > without + 20, "panels for the early selection ({without} -> {with})");
}

/// An enemy unit selected: FAF's unit view looks its army up in
/// GetArmiesTable and shows the owner's nickname (it failed with an empty
/// armies table: `armyData.faction` on nil).
#[test]
fn unit_view_names_an_enemy_units_owner() {
    use rts_core_engine::engine::snapshot::ArmyInfo;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[
        ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false },
        ArmyInfo { index: 2, name: "ARMY_2".into(), nickname: "Rival".into(), faction: 3, human: false, civilian: false },
    ])
    .unwrap();
    // The view shows the selected unit (not just the hovered one) with
    // FAF's "enhanced unit view" option, off by default.
    vm.lua().load("import('/lua/user/prefs.lua').SetOption('gui_enhanced_unitview', 1)").exec().unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.update_units(&[UnitInfo { id: 8, blueprint: "url0107".into(), army: 2, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.set_selection(&[8]).unwrap();
    for _ in 0..10 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let texts: Vec<String> = vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
        DrawItem::Text { text, .. } => Some(text),
        _ => None,
    }).collect();
    assert!(texts.iter().any(|t| t.contains("Rival")), "owner's nickname in the unit view: {texts:?}");
}

/// With the session's scenario info, FAF's score panel builds (it failed on
/// a missing `Options.Share`) and shows the map name.
#[test]
fn score_panel_shows_the_scenario() {
    use rts_core_engine::engine::sync::SyncValue as V;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    let s = |x: &str| V::Str(x.into());
    let info = V::Table(vec![
        (s("name"), s("Seton's Clutch")),
        (s("description"), s("A test map")),
        (s("type"), s("skirmish")),
        (s("size"), V::Table(vec![(V::Num(1.0), V::Num(1024.0)), (V::Num(2.0), V::Num(1024.0))])),
        (s("PlayableAreaWidth"), V::Num(1024.0)),
        (s("PlayableAreaHeight"), V::Num(1024.0)),
        (s("Options"), V::Table(vec![(s("Share"), s("ShareUntilDeath")), (s("Victory"), s("sandbox"))])),
    ]);
    vm.set_scenario(&info).unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let texts: Vec<String> = vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
        DrawItem::Text { text, .. } => Some(text),
        _ => None,
    }).collect();
    assert!(texts.iter().any(|t| t.contains("Seton")), "map name in the score panel: {texts:?}");
}

/// A FAF StatusBar's bar follows its value: the bar is drawn from its
/// Left / Right edges (which the bar drives), not its texture width.
#[test]
fn status_bars_follow_their_value() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.lua()
        .load(
            r#"
            local StatusBar = import('/lua/maui/statusbar.lua').StatusBar
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local tex = '/textures/ui/common/game/unit-build-over-panel/healthbar_bg.dds'
            local bar = StatusBar(GetFrame(0), 0, 100, false, false, tex, tex, true)
            LayoutHelpers.AtLeftTopIn(bar, GetFrame(0), 100, 100)
            bar.Width:Set(200) bar.Height:Set(10)
            rawset(_G, 'TestBar', bar)
            "#,
        )
        .exec()
        .unwrap();
    let width_at = |v: f64| -> f32 {
        vm.lua().load(format!("TestBar:SetValue({v})")).exec().unwrap();
        let quads: Vec<[f32; 4]> = vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
            DrawItem::Quad { rect, .. } => Some(rect),
            _ => None,
        }).collect();
        // The bar is the second quad (the background is drawn first).
        quads.get(1).map_or(-1.0, |r| r[2] - r[0])
    };
    let (full, half) = (width_at(100.0), width_at(50.0));
    assert!((full - 200.0).abs() < 1.0 && (half - 100.0).abs() < 1.0, "bar width at 100: {full}, at 50: {half}");
}

/// Selecting a factory fills FAF's construction panel with the units it
/// can build: their icons are drawn.
#[test]
fn factory_selection_shows_build_icons() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(800.0, 600.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[rts_core_engine::engine::snapshot::ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false }]).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..362 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.update_units(&[UnitInfo { id: 9, blueprint: "ueb0101".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    // FAF's startup cleared the selection; the live loop takes that request.
    vm.take_selection_request().unwrap();
    vm.set_selection(&[9]).unwrap();
    // The live loop: the screen size every frame, a beat per sim tick (10
    // a second) with the unit's data refreshed. The panel stays up across
    // them (re-setting an unchanged screen size used to dirty the layout,
    // and FAF's grid clears itself when its edge goes dirty).
    let factory = || UnitInfo { id: 9, blueprint: "ueb0101".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: Some(Default::default()) };
    for f in 0..60 {
        if f % 6 == 0 {
            vm.update_units(&[factory()]).unwrap();
            vm.beat(f / 6, &[]).unwrap();
        }
        vm.set_screen(800.0, 600.0).unwrap();
        vm.frame(1.0 / 60.0).unwrap();
        assert_eq!(vm.take_selection_request().unwrap(), None, "the interface left the selection alone (frame {f})");
    }
    let icons: Vec<String> = vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
        DrawItem::Quad { texture: Some(t), .. } if t.contains("/icons/units/") => Some(t),
        _ => None,
    }).collect();
    eprintln!("unit icons: {icons:?}");
    assert!(icons.iter().any(|t| t.contains("uel0105")), "the factory's engineer is offered: {icons:?}");
}

/// IsAlly / IsEnemy / IsNeutral answer from the sim's alliances (FAF's
/// score panel colours allied lines with them every beat).
#[test]
fn alliances_answer_is_ally() {
    use rts_core_engine::engine::sim::army::Alliance;
    let Some(vm) = vm() else { return };
    vm.set_alliances(&[(1, 2, Alliance::Ally), (2, 1, Alliance::Ally), (1, 3, Alliance::Neutral)]).unwrap();
    let r: (bool, bool, bool, bool, bool) = vm.lua().load("return IsAlly(1, 2), IsAlly(3, 3), IsNeutral(1, 3), IsEnemy(1, 4), IsAlly(1, 3)").eval().unwrap();
    assert_eq!(r, (true, true, true, true, false));
}

/// Unit blueprints carry moho's defaults for fields they leave out: FAF's
/// build-button tooltip formats them (`%d` on nil failed on rollover).
#[test]
fn blueprints_carry_moho_defaults() {
    let Some(vm) = vm() else { return };
    let r: (f64, f64, f64, String) = vm
        .lua()
        .load(
            r#"
            local bp = __blueprints.ueb0101
            return bp.Intel.WaterVisionRadius, bp.Defense.RegenRate, bp.General.CapCost,
                LOCF("<LOC uvd_0013>Vision: %d, Underwater Vision: %d, Regen: %.3g, Cap Cost: %.3g",
                    bp.Intel.VisionRadius, bp.Intel.WaterVisionRadius, bp.Defense.RegenRate, bp.General.CapCost)
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!((r.0, r.1, r.2), (10.0, 0.0, 1.0));
    assert!(r.3.contains("20") && r.3.contains("10"), "{}", r.3);
}

/// FAF's unit tooltip (shown when hovering a build button) runs for every
/// buildable unit without errors: it formats blueprint fields moho fills
/// with defaults.
#[test]
fn build_tooltips_show_for_every_buildable_unit() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[rts_core_engine::engine::snapshot::ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false }]).unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let (count, errors): (i64, String) = vm
        .lua()
        .load(
            r#"
            local Show = import('/lua/ui/game/unitviewdetail.lua').Show
            local ids = {}
            for id, bp in __blueprints do
                if type(id) == 'string' and bp.Categories and bp.CategoriesHash then
                    for _, c in bp.Categories do
                        if string.find(c, '^BUILTBY') then table.insert(ids, id) break end
                    end
                end
            end
            table.sort(ids)
            local errors, seen = {}, {}
            for _, id in ids do
                local ok, e = xpcall(function() Show(__blueprints[id], nil, id) end, function(m) return tostring(m) .. ' @@ ' .. string.gsub(debug.traceback(), '\n', ' / ') end)
                if not ok then
                    local key = string.gsub(tostring(e), ' @@ .*', '')
                    if not seen[key] then seen[key] = true table.insert(errors, id .. ': ' .. tostring(e)) end
                end
            end
            return table.getn(ids), table.concat(errors, '\n')
            "#,
        )
        .eval()
        .unwrap();
    assert!(count > 300, "buildable units: {count}");
    assert!(errors.is_empty(), "tooltip errors:\n{errors}");
}

/// The category functions take the user layer's units: FAF's construction
/// panel splits an engineer's buildables into tech tabs only when
/// `EntityCategoryContains(ENGINEER + FACTORY, selection[1])` holds.
#[test]
fn engineer_buildables_split_into_tech_tabs() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[rts_core_engine::engine::snapshot::ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false }]).unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.take_selection_request().unwrap();
    vm.update_units(&[UnitInfo { id: 7, blueprint: "uel0105".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.set_selection(&[7]).unwrap();
    let (contains, shown, other): (bool, i64, String) = vm
        .lua()
        .load(
            r#"
            local u = __engine_unit(7)
            local grid = import('/lua/ui/game/construction.lua').controls.choices
            local n, other = 0, {}
            for _, item in grid.DisplayData or {} do
                if item.type == 'item' then
                    n = n + 1
                    if not EntityCategoryContains(categories.TECH1, item.id) then table.insert(other, item.id) end
                end
            end
            return EntityCategoryContains(categories.ENGINEER, u), n, table.concat(other, ',')
            "#,
        )
        .eval()
        .unwrap();
    assert!(contains, "EntityCategoryContains takes a UserUnit");
    assert!(shown > 5, "the T1 tab shows the engineer's structures ({shown})");
    assert!(other.is_empty(), "upgrades (T2 / T3) in the T1 tab: {other}");
}

/// Clicking a structure's build icon with an engineer selected arms FAF's
/// build command mode for it (the world click then places it). Needs the
/// `UpgradesFrom = 'none'` default: nil, the click was taken for an upgrade.
#[test]
fn clicking_a_build_icon_arms_build_mode() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[rts_core_engine::engine::snapshot::ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false }]).unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.take_selection_request().unwrap();
    vm.update_units(&[UnitInfo { id: 7, blueprint: "uel0105".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    vm.set_selection(&[7]).unwrap();
    for _ in 0..10 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let rect = vm.draw_list().unwrap().into_iter().find_map(|i| match i {
        DrawItem::Quad { texture: Some(t), rect, .. } if t.ends_with("/ueb1101_icon.dds") => Some(rect),
        _ => None,
    });
    let rect = rect.expect("the power generator's build icon is drawn");
    let (x, y) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let left = Modifiers { left: true, ..Default::default() };
    assert!(vm.mouse(MouseEvent::Move, x, y, Modifiers::default()).unwrap(), "over the icon");
    vm.frame(1.0 / 60.0).unwrap();
    vm.mouse(MouseEvent::Press(1), x, y, left).unwrap();
    vm.mouse(MouseEvent::Release(1), x, y, Modifiers::default()).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    let mode = vm.command_mode().unwrap();
    assert_eq!(mode, Some(("build".to_string(), "ueb1101".to_string())));
}

/// The sim's build restrictions lock the construction panel's tech tabs: an
/// ACU (T2/T3 restricted until its engineering upgrade, as FAF's script
/// does) offers only T1, and the tabs follow when the restriction lifts.
#[test]
fn build_restrictions_lock_tech_tabs() {
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(800.0, 600.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[rts_core_engine::engine::snapshot::ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false }]).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..362 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let restricted: Vec<String> = vm
        .lua()
        .load("local ids = EntityCategoryGetUnitList(categories.UEF * (categories.BUILTBYTIER2COMMANDER + categories.BUILTBYTIER3COMMANDER)) table.sort(ids) return ids")
        .eval()
        .unwrap();
    assert!(!restricted.is_empty());
    let acu = |restricted: Vec<String>| UnitInfo { id: 5, blueprint: "uel0001".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: Some(UnitDetail { id: 5, restricted, ..Default::default() }) };
    vm.update_units(&[acu(restricted.clone())]).unwrap();
    vm.take_selection_request().unwrap();
    vm.set_selection(&[5]).unwrap();
    assert!(!vm.update_units(&[acu(restricted.clone())]).unwrap(), "unchanged restrictions don't refresh");
    let tabs = |vm: &UserVm| -> (bool, bool, bool, bool) {
        vm.lua()
            .load(
                r#"
                local c = import('/lua/ui/game/construction.lua')
                local _, _, buildable = GetUnitCommandData({ __engine_unit(5) })
                return c.GetTabByID('t1'):IsDisabled(), c.GetTabByID('t2'):IsDisabled(),
                    EntityCategoryContains(buildable, 'ueb1101'), EntityCategoryContains(buildable, 'ueb1201')
                "#,
            )
            .eval()
            .unwrap()
    };
    assert_eq!(tabs(&vm), (false, true, true, false), "(t1 disabled, t2 disabled, can build T1 power, can build T2 power)");
    // The upgrade lifts the restriction: the panel refreshes in place.
    assert!(vm.update_units(&[acu(Vec::new())]).unwrap());
    vm.refresh_selection().unwrap();
    assert_eq!(tabs(&vm), (false, false, true, true));
}

/// `GetUnitCommandData` over a mixed selection: the union of the
/// blueprints' command and toggle caps (sorted, each once) and of what each
/// can build; a dead unit adds nothing, and no units build nothing.
#[test]
fn unit_command_data_unions_the_selection() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    let info = |id, bp: &str| UnitInfo { id, blueprint: bp.into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: None };
    vm.update_units(&[info(1, "uel0201"), info(2, "uel0201"), info(3, "uea0102"), info(4, "uel0105"), info(5, "uel0105")]).unwrap();
    vm.remove_units(&[5]).unwrap();
    let (ok, why): (bool, String) = vm
        .lua()
        .load(
            r#"
            -- Registration leaves the caps as text (a name, or a number for
            -- several); FAF keeps the order table as `CommandCapsHash`.
            local function caps(ids, field)
                local seen, out = {}, {}
                for _, id in ids do
                    local general = __blueprints[id].General
                    local set = general.CommandCapsHash
                    if field == 'ToggleCaps' then
                        local text = general.ToggleCaps
                        set = {}
                        if tonumber(text) == nil then set[text] = true elseif tonumber(text) ~= 0 then error('several toggles: ' .. text) end
                    end
                    for cap, on in set do
                        if on and not seen[cap] then seen[cap] = true table.insert(out, cap) end
                    end
                end
                table.sort(out)
                return table.concat(out, ',')
            end
            local units = {}
            for i = 1, 5 do units[i] = __engine_unit(i) end
            local orders, toggles, buildable = GetUnitCommandData(units)
            local ids = { 'uel0201', 'uea0102', 'uel0105' }
            if table.concat(orders, ',') ~= caps(ids, 'CommandCaps') then return false, 'orders ' .. table.concat(orders, ',') end
            if table.concat(toggles, ',') ~= caps(ids, 'ToggleCaps') then return false, 'toggles ' .. table.concat(toggles, ',') end
            local _, _, engineer = GetUnitCommandData({ __engine_unit(4) })
            if not (buildable == engineer and EntityCategoryContains(buildable, 'ueb1101')) then return false, 'buildable' end
            local o, t, none = GetUnitCommandData({})
            if table.getn(o) ~= 0 or table.getn(t) ~= 0 or EntityCategoryContains(none, 'ueb1101') then return false, 'empty' end
            return true, ''
            "#,
        )
        .eval()
        .unwrap();
    assert!(ok, "{why}");
}

/// A selected unit's detail tables are rebuilt only when their part
/// changed: the same queue keeps its table from beat to beat, a changed
/// economy is rewritten, and a unit whose data was dropped gets all of it
/// back when it returns.
#[test]
fn unchanged_unit_details_keep_their_tables() {
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    let unit = |econ: f32| UnitInfo {
        id: 9,
        blueprint: "uel0105".into(),
        army: 1,
        health: 1.0,
        max_health: 100.0,
        build_rate: 0.0,
        build: 1.0,
        position: [0.0; 3],
        work: 0.0,
        shield: 0.0,
        detail: Some(UnitDetail { id: 9, commands: vec![(1, None, 1, [10.0, 0.0, 20.0])], econ: [econ, 0.0, 0.0, 0.0, 0.0, 0.0], stats: vec![("VetLevel".into(), 1.0)], ..Default::default() }),
    };
    let eval = |code: &str| vm.lua().load(code).eval::<String>().unwrap();
    vm.update_units(&[unit(2.0)]).unwrap();
    eval("rawset(_G, '__q', __engine_unit_data[9].queue) rawset(_G, '__e', __engine_unit_data[9].econ) return ''");
    vm.update_units(&[unit(2.0)]).unwrap();
    assert_eq!(eval("return tostring(rawequal(__q, __engine_unit_data[9].queue)) .. tostring(rawequal(__e, __engine_unit_data[9].econ))"), "truetrue", "unchanged parts keep their tables");
    vm.update_units(&[unit(5.0)]).unwrap();
    assert_eq!(eval("return tostring(rawequal(__q, __engine_unit_data[9].queue)) .. ' ' .. __engine_unit_data[9].econ.massConsumed"), "true 5", "only the changed part is rewritten");
    vm.remove_units(&[9]).unwrap();
    vm.update_units(&[unit(5.0)]).unwrap();
    assert_eq!(eval("local d = __engine_unit_data[9] return table.getn(d.queue) .. ' ' .. d.econ.massConsumed .. ' ' .. d.stats.VetLevel"), "1 5 1", "a returning unit gets every part again");
}

/// `GetUnitById` answers for any unit of the frame, not just selected ones
/// (FAF's notify overlay looks up the unit doing an enhancement), and a
/// unit gone from the frame reads as dead.
#[test]
fn get_unit_by_id_finds_unselected_units() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_world(vec![UnitInfo { id: 42, blueprint: "uel0001".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [100.0, 20.0, 200.0], work: 0.0, shield: 0.0, detail: None }]);
    let (found, x, missing): (bool, f64, bool) = vm
        .lua()
        .load("local u = GetUnitById('42') local p = u and u:GetPosition() return u ~= nil, p and p[1] or -1, GetUnitById(43) == nil")
        .eval()
        .unwrap();
    assert!(found && missing);
    assert!((x - 100.0).abs() < 1e-3);
    assert_eq!(vm.known_units().unwrap(), vec![42]);
    vm.remove_units(&[42]).unwrap();
    let dead: bool = vm.lua().load("return __engine_unit(42):IsDead()").eval().unwrap();
    assert!(dead);
}

/// Chat to the local client in a game without a network reaches it (FAF's
/// notify overlay sends the ACU upgrade ETA to its own player), once,
/// as a copy; a message for no local client doesn't.
#[test]
fn chat_to_the_local_client_loops_back_without_a_network() {
    let Some(vm) = vm() else { return };
    let got: String = vm
        .lua()
        .load(
            r#"
            local msg = { NotifyOverlay = true, data = { progress = 12 } }
            SessionSendChatMessage(1, msg)
            msg.data.progress = 99
            SessionSendChatMessage({ 7 }, { lost = true })
            SessionSendChatMessage({ to = 'all' })
            local out = {}
            for _, m in __net_chat_poll() do
                table.insert(out, m.sender .. ':' .. tostring(m.data.data and m.data.data.progress or m.data.to))
            end
            local again = table.getn(__net_chat_poll())
            return table.concat(out, ',') .. ' ' .. again
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(got, "Observer:12,Observer:all 0");
}

/// An unselected ACU building an enhancement: `GetWorkProgress` on its
/// UserUnit follows the sim's progress beat by beat (FAF's ACU
/// upgrade ETA polls it, and only showed once the ACU was selected).
#[test]
fn unselected_acu_work_progress_follows_the_sim() {
    use rts_core_engine::engine::sim::entity::EntityWorld;
    use rts_core_engine::engine::snapshot::{self, EntityView};
    use rts_core_engine::ui::UnitInfo;
    if !rts_core_engine::test_data::require_game("UI tests") {
        return;
    }
    let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let session = engine::session::SessionConfig { ticks: Some(0), create_initial_units: false, ai: false, ..Default::default() };
    engine.start(&session).expect("session");
    let acu: u32 = engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local acu = CreateUnitHPR('uel0001', 'ARMY_1', 400, GetTerrainHeight(400, 640), 640, 0, 0, 0)
            IssueScript({acu}, { TaskName = 'EnhanceTask', Enhancement = 'AdvancedEngineering' })
            return tonumber(acu:GetEntityId())
            "#,
        )
        .eval()
        .unwrap();
    let vm = UserVm::with_cache(engine.vfs.clone(), engine.script_cache.clone()).expect("user vm");
    vm.init().expect("userInit.lua");
    // A beat as the frame gives it (`render/frame.rs`): every unit of the
    // snapshot for `GetUnitById`, the held ones refreshed; none selected.
    let mut views: Vec<EntityView> = Vec::new();
    let mut beat = |engine: &engine::Engine| {
        snapshot::build(&engine.sim_lua().app_data_ref::<EntityWorld>().unwrap(), None, &mut views);
        let info = |e: &EntityView| UnitInfo::from_view(e, "uel0001".into());
        vm.set_world(views.iter().filter(|e| e.army != 0 && e.is_interface_unit()).map(info).collect());
        vm.refresh_held_units(&[], |ids| views.iter().filter(|e| ids.binary_search(&e.id).is_ok() && e.is_interface_unit()).map(info).collect()).unwrap();
    };
    let sim_work = |engine: &engine::Engine| engine.sim_lua().load(format!("return GetEntityById({acu}):GetWorkProgress()")).eval::<f64>().unwrap();
    let ui_work = |vm: &UserVm| vm.lua().load(format!("return GetUnitById('{acu}'):GetWorkProgress()")).eval::<f64>().unwrap();
    let mut seen = Vec::new();
    for _ in 0..6 {
        for _ in 0..30 {
            engine.tick().unwrap();
        }
        beat(&engine);
        let (sim, ui) = (sim_work(&engine), ui_work(&vm));
        assert!((sim - ui).abs() < 1e-3, "the interface's progress {ui} is the sim's {sim}");
        seen.push(ui);
    }
    assert_eq!(vm.lua().load("return table.getn(GetSelectedUnits() or {})").eval::<i64>().unwrap(), 0, "never selected");
    assert!(seen[0] > 0.0 && seen.windows(2).all(|w| w[1] > w[0]), "progress advances beat by beat: {seen:?}");
}

/// Order feedback: an issued move gets FAF's Move marker
/// (`AddCommandFeedbackBlip`), drawn until its 0.7 s are up.
#[test]
fn issued_orders_leave_feedback_markers() {
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget};
    use rts_core_engine::engine::sim::math::FxVec3;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    vm.frame(1.0 / 60.0).unwrap();
    let at = CommandTarget::Ground(FxVec3::from_f64(100.0, 10.0, 120.0));
    vm.command_issued(CommandKind::Move, at, false, None, &[], &[]).unwrap();
    let meshes = vm.interface_meshes();
    let marker = meshes.iter().find(|m| m.mesh.eq_ignore_ascii_case("/meshes/game/Move_lod0.scm")).unwrap_or_else(|| panic!("a move marker: {meshes:?}"));
    assert_eq!(marker.shader, "CommandFeedback");
    assert_eq!([marker.position[0], marker.position[2]], [100.0, 120.0]);
    for _ in 0..60 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    // (The world border, which the interface also adds, stays.)
    assert!(!vm.interface_meshes().iter().any(|m| m.shader == "CommandFeedback"), "gone after its duration");
}

/// The world border (FAF's Render World Border, `UpdateWorldBorderState` on
/// starting the interface): the skin's border mesh round the map, centred,
/// scaled by the map's width x its blueprint's `UniformScale`, and gone
/// after `MapBorderClear`.
#[test]
fn world_border_frames_the_map() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    let border: Vec<_> = vm.interface_meshes().into_iter().filter(|m| m.shader == "MapImager").collect();
    assert!(!border.is_empty(), "a border mesh");
    let size: (f32, f32) = vm.lua().load("local s = SessionGetScenarioInfo().size return s[1], s[2]").eval().unwrap();
    for m in &border {
        assert!(m.mesh.starts_with("/meshes/game/map-border_") && m.mesh.ends_with("_lod0.scm"), "{m:?}");
        assert_eq!([m.position[0], m.position[2]], [size.0 / 2.0, size.1 / 2.0]);
        assert_eq!(m.scale[0], size.0 / 512.0);
    }
    vm.lua().load("MapBorderClear()").exec().unwrap();
    assert!(!vm.interface_meshes().iter().any(|m| m.shader == "MapImager"));
}

/// `WorldMesh` (FAF's rally flags): drawn once shown and placed, until
/// destroyed.
#[test]
fn world_meshes_follow_their_methods() {
    let Some(vm) = vm() else { return };
    let make = "rawset(_G, 'TestMesh', import('/lua/ui/controls/worldmesh.lua').WorldMesh())
        TestMesh:SetMesh({ MeshName = '/meshes/game/Rally_lod0.scm', TextureName = '/meshes/game/Rally_albedo.dds', ShaderName = 'RallyPoint', UniformScale = 0.1 })
        TestMesh:SetLifetimeParameter(10)
        TestMesh:SetStance(Vector(50, 20, 60))";
    vm.lua().load(make).exec().unwrap();
    assert!(vm.interface_meshes().is_empty(), "hidden until shown");
    vm.lua().load("TestMesh:SetHidden(false)").exec().unwrap();
    let meshes = vm.interface_meshes();
    assert_eq!(meshes.len(), 1, "{meshes:?}");
    assert_eq!((meshes[0].shader.as_str(), meshes[0].position, meshes[0].scale[0], meshes[0].lifetime), ("RallyPoint", [50.0, 20.0, 60.0], 0.1, 10.0));
    vm.lua().load("TestMesh:Destroy()").exec().unwrap();
    assert!(vm.interface_meshes().is_empty());
}

/// FAF's cinematics lock a world view's input: the world stops getting the
/// mouse there until it's unlocked.
#[test]
fn locked_world_view_takes_the_mouse_from_the_world() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let left = Modifiers { left: true, ..Default::default() };
    let press = |vm: &UserVm| {
        let took = vm.mouse(MouseEvent::Press(1), 640.0, 500.0, left).unwrap();
        vm.mouse(MouseEvent::Release(1), 640.0, 500.0, Modifiers::default()).unwrap();
        took
    };
    assert!(!press(&vm), "the world gets clicks on an unlocked view");
    let locked: bool = vm.lua().load("local wv = import('/lua/ui/game/worldview.lua') wv.viewLeft:LockInput() return wv.IsInputLocked()").eval().unwrap();
    assert!(locked);
    assert!(press(&vm), "a locked view keeps the click from the world");
    let locked: bool = vm.lua().load("local wv = import('/lua/ui/game/worldview.lua') wv.viewLeft:UnlockInput() return wv.IsInputLocked()").eval().unwrap();
    assert!(!locked);
    assert!(!press(&vm), "unlocked again");
}

/// A scrollbar scrolls the control set with `SetScrollable`, draws its
/// background and a thumb over the part of the range in view, drags with
/// its thumb and pages on a press beside it.
#[test]
fn scrollbar_scrolls_its_scrollable_and_draws_the_thumb() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.lua()
        .load(
            r#"
            local Group = import('/lua/maui/group.lua').Group
            local Scrollbar = import('/lua/maui/scrollbar.lua').Scrollbar
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local list = Group(GetFrame(0))
            list.top = 0
            list.GetScrollValues = function(self, axis) return 0, 100, self.top, self.top + 25 end
            list.ScrollLines = function(self, axis, delta) self.axis = axis self.top = self.top + delta end
            list.ScrollPages = function(self, axis, delta) self.top = self.top + delta * 25 end
            local dir = '/textures/ui/common/game/economic-overlay/'
            local bar = Scrollbar(GetFrame(0), 'Vert')
            LayoutHelpers.AtLeftTopIn(bar, GetFrame(0), 100, 100)
            bar.Width:Set(20) bar.Height:Set(400)
            bar:SetNewTextures(dir .. 'econ_bmp_m.dds', dir .. 'econ_bmp_l.dds', nil, nil)
            bar:SetScrollable(list)
            bar:DoScrollLines(10)
            bar:DoScrollPages(1)
            rawset(_G, 'List', list)
            "#,
        )
        .exec()
        .unwrap();
    let (top, axis): (f64, String) = vm.lua().load("return List.top, List.axis").eval().unwrap();
    assert_eq!((top, axis.as_str()), (35.0, "Vert"), "ten lines, then a page of 25");
    let quads: Vec<([f32; 4], String)> = vm
        .draw_list()
        .unwrap()
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Quad { rect, texture: Some(t), .. } if t.contains("econ_bmp_") => Some((rect, t)),
            _ => None,
        })
        .collect();
    assert_eq!(quads.len(), 2, "background and thumb: {quads:?}");
    assert_eq!(quads[0].0, [100.0, 100.0, 120.0, 500.0], "background fills the bar");
    // In view: 35..60 of 0..100, so 140..240 of the 400 pixel bar.
    assert_eq!(quads[1].0, [100.0, 240.0, 120.0, 340.0], "thumb over the visible part");
    // Grab the thumb 10 px down and drag it 100 px: a quarter of the range.
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    vm.lua().load("List.ScrollSetTop = function(self, axis, top) self.top = top end").exec().unwrap();
    let left = Modifiers { left: true, ..Default::default() };
    vm.mouse(MouseEvent::Press(1), 110.0, 250.0, left).unwrap();
    vm.mouse(MouseEvent::Move, 110.0, 350.0, left).unwrap();
    vm.mouse(MouseEvent::Move, 700.0, 350.0, left).unwrap(); // off the bar: still held
    vm.mouse(MouseEvent::Release(1), 700.0, 350.0, Modifiers::default()).unwrap();
    let top: f64 = vm.lua().load("return List.top").eval().unwrap();
    assert_eq!(top, 60.0, "dragged a quarter of the bar down");
    // A press on the bar below the thumb pages down.
    vm.mouse(MouseEvent::Press(1), 110.0, 490.0, left).unwrap();
    vm.mouse(MouseEvent::Release(1), 110.0, 490.0, Modifiers::default()).unwrap();
    let top: f64 = vm.lua().load("return List.top").eval().unwrap();
    assert_eq!(top, 85.0, "a page (25) down");
}

/// A popup opened over another (the key bindings dialog's "press a key" and
/// confirm dialogs) sits above it, so its buttons get the clicks: FAF's
/// `Popup` places itself at `GetTopmostDepth() + 10`.
#[test]
fn popup_over_the_key_bindings_dialog_gets_the_clicks() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let (x, y): (f32, f32) = vm
        .lua()
        .load(
            r#"
            import('/lua/ui/dialogs/keybindings.lua').CreateUI()
            rawset(_G, 'Answered', false)
            local d = import('/lua/ui/uiutil.lua').QuickDialog(GetFrame(0), 'Sure?', 'Yes', function() rawset(_G, 'Answered', true) end, 'No', nil, nil, nil, true, { worldCover = false })
            local b = d.content._button1 or d._button1
            return (b.Left() + b.Right()) / 2, (b.Top() + b.Bottom()) / 2
            "#,
        )
        .eval()
        .unwrap();
    for _ in 0..5 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let left = Modifiers { left: true, ..Default::default() };
    vm.mouse(MouseEvent::Move, x, y, Modifiers::default()).unwrap();
    vm.mouse(MouseEvent::Press(1), x, y, left).unwrap();
    vm.mouse(MouseEvent::Release(1), x, y, Modifiers::default()).unwrap();
    let answered: bool = vm.lua().globals().raw_get("Answered").unwrap();
    assert!(answered, "the confirm dialog's button at ({x}, {y}) got the click");
}

/// A quick second press on the same control is a `ButtonDClick` (the key
/// bindings dialog opens its "press a key" popup on one); the next press is
/// a plain one again.
#[test]
fn second_quick_press_is_a_double_click() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.lua()
        .load(
            r#"
            local g = import('/lua/maui/group.lua').Group(GetFrame(0))
            import('/lua/maui/layouthelpers.lua').AtLeftTopIn(g, GetFrame(0), 100, 100)
            g.Width:Set(50) g.Height:Set(50) g.Depth:Set(100000)
            rawset(_G, 'Events', {})
            g.HandleEvent = function(self, e)
                if e.Type ~= 'MouseMotion' and e.Type ~= 'MouseEnter' then table.insert(Events, e.Type) end
                return true
            end
            "#,
        )
        .exec()
        .unwrap();
    let left = Modifiers { left: true, ..Default::default() };
    for _ in 0..3 {
        vm.mouse(MouseEvent::Press(1), 120.0, 120.0, left).unwrap();
        vm.mouse(MouseEvent::Release(1), 120.0, 120.0, Modifiers::default()).unwrap();
    }
    vm.frame(1.0).unwrap();
    vm.mouse(MouseEvent::Press(1), 120.0, 120.0, left).unwrap();
    let events: Vec<String> = vm.lua().load("return Events").eval().unwrap();
    assert_eq!(events, ["ButtonPress", "ButtonRelease", "ButtonDClick", "ButtonRelease", "ButtonPress", "ButtonRelease", "ButtonPress"], "a second apart: no double click");
}

/// The control with the keyboard focus gets key presses as `KeyDown`
/// (moho's `KeyCode`, Windows' `RawKeyCode`); one that blocks keeps them
/// from the key map, one that doesn't shares them.
#[test]
fn keyboard_focus_gets_key_downs() {
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    // The first frame unlocks input (FAF's OnFirstUpdate ends the start-up input lock).
    vm.frame(1.0 / 60.0).unwrap();
    vm.lua()
        .load(
            r#"
            local Group = import('/lua/maui/group.lua').Group
            rawset(_G, 'Keys', {})
            local function taker(blocks)
                local g = Group(GetFrame(0))
                g.HandleEvent = function(self, e)
                    if e.Type == 'KeyDown' then table.insert(Keys, string.format('%d/%d/%s', e.KeyCode, e.RawKeyCode, tostring(e.Modifiers.Ctrl or false))) end
                end
                g:AcquireKeyboardFocus(blocks)
                return g
            end
            rawset(_G, 'Sharing', taker(false))
            rawset(_G, 'Blocking', taker(true))
            "#,
        )
        .exec()
        .unwrap();
    assert!(vm.key("F1", true, false, false).unwrap(), "a blocking focus keeps the key");
    vm.lua().load("Blocking:AbandonKeyboardFocus()").exec().unwrap();
    vm.key("S", false, false, false).unwrap();
    vm.lua().load("Sharing:Destroy()").exec().unwrap();
    vm.key("Esc", false, false, false).unwrap();
    let keys: Vec<String> = vm.lua().load("return Keys").eval().unwrap();
    assert_eq!(keys, ["342/112/true", "83/83/false"], "F1 to the blocker, S to the next; nothing once it's gone");
}

/// FAF's `Edit`: a click focuses it, typed characters go in at the caret,
/// Backspace and the arrows edit, Enter / Esc reach its callbacks (Esc
/// clears, and goes on up as a `KeyDown`), hotkeys don't fire while it has
/// the focus, and a click elsewhere takes the focus away.
#[test]
fn edits_take_typing() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.lua()
        .load(
            r#"
            local Group = import('/lua/maui/group.lua').Group
            local Edit = import('/lua/maui/edit.lua').Edit
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local box = Group(GetFrame(0))
            LayoutHelpers.AtLeftTopIn(box, GetFrame(0), 100, 100)
            box.Width:Set(300) box.Height:Set(30) box.Depth:Set(100000)
            rawset(_G, 'Log', {})
            box.HandleEvent = function(self, e) if e.Type == 'KeyDown' then table.insert(Log, 'parent ' .. e.KeyCode) end end
            local edit = Edit(box)
            LayoutHelpers.FillParent(edit, box)
            edit:SetFont('Arial', 14)
            edit:SetMaxChars(8)
            edit.OnTextChanged = function(self, new, old) table.insert(Log, old .. '>' .. new) end
            edit.OnEnterPressed = function(self, text) table.insert(Log, 'enter ' .. text) end
            edit.OnLoseKeyboardFocus = function(self) table.insert(Log, 'lost') end
            rawset(_G, 'TheEdit', edit)
            "#,
        )
        .exec()
        .unwrap();
    let left = Modifiers { left: true, ..Default::default() };
    let click = |x, y| {
        vm.mouse(MouseEvent::Press(1), x, y, left).unwrap();
        vm.mouse(MouseEvent::Release(1), x, y, Modifiers::default()).unwrap();
    };
    assert!(!vm.typing());
    click(250.0, 110.0);
    assert!(vm.typing(), "a click focuses the edit");
    assert!(vm.text("héllo").unwrap());
    vm.key("Backspace", false, false, false).unwrap();
    vm.key("LeftArrow", false, false, false).unwrap();
    vm.text("X").unwrap();
    assert!(vm.key("S", false, false, false).unwrap(), "the edit keeps hotkeys");
    vm.text("1234567").unwrap(); // past MaxChars
    let text: String = vm.lua().load("return TheEdit:GetText()").eval().unwrap();
    assert_eq!(text, "hélX123l", "edited at the caret, up to MaxChars (8)");
    let caret = vm.draw_list().unwrap().into_iter().any(|i| matches!(i, DrawItem::Quad { rect, texture: None, .. } if rect[2] - rect[0] == 1.0));
    assert!(caret, "the focused edit draws its caret");
    vm.key("Enter", false, false, false).unwrap();
    vm.key("Esc", false, false, false).unwrap();
    assert!(vm.typing(), "Esc clears but keeps the focus");
    click(700.0, 500.0);
    assert!(!vm.typing(), "a click elsewhere takes the focus");
    let log: Vec<String> = vm.lua().load("return Log").eval().unwrap();
    let log: Vec<&str> = log.iter().map(String::as_str).skip(5).collect();
    assert_eq!(log, ["héllo>héll", "parent 8", "parent 316", "héll>hélXl", "hélXl>hélX1l", "hélX1l>hélX12l", "hélX12l>hélX123l", "enter hélX123l", "parent 13", "parent 27", "lost"]);
}

/// Enter opens FAF's chat with its edit focused; Enter sends what's typed
/// (the box empties), and Enter on an empty box closes the chat.
#[test]
fn chat_opens_and_takes_typing() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    assert!(vm.key("Enter", false, false, false).unwrap());
    assert!(vm.typing(), "chat's edit has the focus");
    // FAF 3839's chat (`/lua/ui/game/chat/`) keeps its window private; what
    // Enter sends goes through `ChatController.Send`.
    vm.lua().load("local c = import('/lua/ui/game/chat/ChatController.lua') c.Send = function(text) Sent = text end").exec().unwrap();
    vm.text("gl hf").unwrap();
    vm.key("Enter", false, false, false).unwrap();
    let sent: Option<String> = vm.lua().load("return Sent").eval().unwrap();
    assert_eq!(sent.as_deref(), Some("gl hf"));
    assert!(vm.typing(), "still open");
    // Enter on an empty box (it emptied when it sent) closes the chat.
    vm.key("Enter", false, false, false).unwrap();
    assert!(!vm.typing(), "closed");
}

/// The wheel over a panel is the interface's: the control under it gets
/// `WheelRotation` (120 a notch); FAF's HUD clusters hand it back to the
/// world (`ZoomScale`), which zooms the camera by as many notches.
#[test]
fn wheel_over_panels_goes_to_the_interface() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.lua()
        .load(
            r#"
            local g = import('/lua/maui/group.lua').Group(GetFrame(0))
            import('/lua/maui/layouthelpers.lua').AtLeftTopIn(g, GetFrame(0), 600, 300)
            g.Width:Set(50) g.Height:Set(50) g.Depth:Set(100000)
            g.HandleEvent = function(self, e) if e.Type == 'WheelRotation' then rawset(_G, 'Wheel', e.WheelRotation .. '/' .. e.WheelDelta) return true end end
            "#,
        )
        .exec()
        .unwrap();
    assert!(vm.mouse(MouseEvent::Wheel(-120.0), 610.0, 310.0, Modifiers::default()).unwrap());
    let wheel: String = vm.lua().globals().raw_get("Wheel").unwrap();
    assert_eq!(wheel, "-120/120");
    vm.take_camera_requests().unwrap();
    // FAF's HUD clusters pass the wheel on through ForwardMouseWheelInput.
    vm.lua().load("import('/lua/ui/game/worldview.lua').ForwardMouseWheelInput({ MouseX = 640, MouseY = 400, WheelRotation = 240, WheelDelta = 120 })").exec().unwrap();
    let zooms: Vec<Option<f32>> = vm.take_camera_requests().unwrap().into_iter().map(|r| r.wheel).collect();
    assert_eq!(zooms, [Some(2.0)], "two notches on to the world");
}

/// FAF's combo box (the options dialog's drop-downs): a click opens its
/// item list, whose rows are drawn; a click on a row picks it and closes
/// the list; a click elsewhere closes it (`uimain.OnMouseButtonPress`).
#[test]
fn combo_drop_downs_open_pick_and_close() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface"); // (the skin's colours)
    vm.lua()
        .load(
            r#"
            local Combo = import('/lua/ui/controls/combo.lua').Combo
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local c = Combo(GetFrame(0), 14, 5, nil, nil)
            LayoutHelpers.AtLeftTopIn(c, GetFrame(0), 400, 200)
            c.Width:Set(200)
            c.Depth:Set(GetFrame(0):GetTopmostDepth() + 1)
            c:AddItems({ 'Low', 'Medium', 'High' }, 1)
            rawset(_G, 'Picked', nil)
            c.OnClick = function(self, index, text) rawset(_G, 'Picked', text) end
            rawset(_G, 'TheCombo', c)
            "#,
        )
        .exec()
        .unwrap();
    let (cx, cy, open): (f32, f32, bool) = vm.lua().load("local c = TheCombo return (c.Left() + c.Right()) / 2, (c.Top() + c.Bottom()) / 2, not c._dropdown:IsHidden()").eval().unwrap();
    assert!(!open, "starts closed");
    let left = Modifiers { left: true, ..Default::default() };
    let click = |x: f32, y: f32| {
        vm.mouse(MouseEvent::Move, x, y, Modifiers::default()).unwrap();
        vm.mouse(MouseEvent::Press(1), x, y, left).unwrap();
        vm.mouse(MouseEvent::Release(1), x, y, Modifiers::default()).unwrap();
        vm.frame(1.0).unwrap(); // (no double clicks)
    };
    click(cx, cy);
    let (open, top, row_h, list_left): (bool, f32, f32, f32) = vm.lua().load("local c = TheCombo return not c._dropdown:IsHidden(), c._list.Top(), c._list:GetRowHeight(), c._list.Left()").eval().unwrap();
    assert!(open, "a click opens it");
    let rows: Vec<String> = vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
        DrawItem::Text { text, rect, .. } if rect[1] >= top - 0.5 && (rect[0] - list_left).abs() < 0.5 => Some(text),
        _ => None,
    }).collect();
    assert_eq!(rows, ["Low", "Medium", "High"], "its rows are drawn");
    click(cx, top + row_h * 2.5);
    let (picked, open, label): (Option<String>, bool, String) = vm.lua().load("local c = TheCombo return Picked, not c._dropdown:IsHidden(), c._text:GetText()").eval().unwrap();
    assert_eq!((picked.as_deref(), open, label.as_str()), (Some("High"), false, "High"), "the third row picked, the list closed");
    click(cx, cy);
    click(900.0, 600.0);
    let open: bool = vm.lua().load("return not TheCombo._dropdown:IsHidden()").eval().unwrap();
    assert!(!open, "a click outside closes it");
}

/// FAF picks the cursor (`OnUpdateCursor` on the world view under the
/// mouse, each frame): its default over the world, the order's cursor in
/// command mode (animated ones frame by frame), the default again over a
/// panel.
#[test]
fn faf_picks_the_cursor() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.mouse(MouseEvent::Move, 640.0, 500.0, Modifiers::default()).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    use rts_core_engine::ui::cursor::{Texture, Wanted};
    let texture = |vm: &UserVm| match vm.cursor() {
        Wanted::Texture(t) => t,
        other => panic!("no cursor texture: {other:?}"),
    };
    assert_eq!(texture(&vm), Texture { path: "/textures/ui/common/game/cursors/selectable.dds".into(), hotspot: [2.0, 2.0] });
    vm.lua().load("import('/lua/ui/game/commandmode.lua').StartCommandMode('order', { name = 'RULEUCC_Move' })").exec().unwrap();
    for _ in 0..3 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let path = texture(&vm).path;
    assert!(path.starts_with("/textures/ui/common/game/cursors/move-") && path.ends_with(".dds"), "the move cursor's frames: {path}");
    vm.mouse(MouseEvent::Move, 60.0, 20.0, Modifiers::default()).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    let path = texture(&vm).path;
    assert_eq!(path, "/textures/ui/common/game/cursors/selectable.dds", "over a panel: the default");
}

/// FAF's range overlay toggles reach the renderer as filters: colours (glow
/// from `AA`), thicknesses, the blueprints covered and the range kinds taken
/// in (the anti-air filter covers a destroyer but takes only anti-air
/// ranges, not its guns).
#[test]
fn range_overlay_filters_for_the_renderer() {
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    vm.lua()
        .load("SetOverlayFilter('AntiAir', categories.OVERLAYANTIAIR, '0029def2', '0852eafc', '1063efff', 0.02, 2.0, 0.04, 4.0) SetOverlayFilters({ 'AntiAir' })")
        .exec()
        .unwrap();
    // Every unit blueprint, as the renderer passes them.
    let mut ids: Vec<String> = vm.lua().load("return EntityCategoryGetUnitList(categories.ALLUNITS)").eval().unwrap();
    ids.sort();
    let filters = vm.take_overlay_filters(&ids).unwrap().expect("changed");
    assert_eq!(filters.len(), 1);
    let f = &filters[0];
    assert_eq!(f.name, "AntiAir");
    let member = |id: &str| f.members[ids.iter().position(|i| i == id).unwrap()];
    assert_eq!([member("uel0104"), member("uel0201"), member("ues0201")], [true, false, true], "AA tank and destroyer, not the Striker");
    let antiair = 1 << rts_core_engine::engine::snapshot::RANGE_KINDS.iter().position(|k| *k == "ANTIAIR").unwrap();
    assert_eq!(f.kinds, antiair, "anti-air ranges only");
    assert_eq!(f.colors[1][3], 8.0 / 255.0, "selected glow 08");
    assert!((f.colors[0][0] - 0x29 as f32 / 255.0).abs() < 1e-6);
    assert_eq!((f.inner, f.outer), ([0.02, 2.0], [0.04, 4.0]));
    assert!(vm.take_overlay_filters(&ids).unwrap().is_none(), "unchanged: nothing new");
    vm.lua().load("SetOverlayFilters({})").exec().unwrap();
    assert_eq!(vm.take_overlay_filters(&ids).unwrap(), Some(vec![]), "all off");
}

/// Each range kind's own style (the placement ghost's rings) comes from
/// FAF's `rangeoverlayparams.lua`: direct fire red, radar teal.
#[test]
fn range_styles_from_faf() {
    let Some(vm) = vm() else { return };
    vm.start_hud().expect("game interface");
    let styles = vm.range_styles().unwrap();
    let kind = |k: &str| rts_core_engine::engine::snapshot::RANGE_KINDS.iter().position(|n| *n == k).unwrap();
    let direct = styles[kind("DIRECTFIRE")].expect("direct fire");
    assert_eq!(direct.colors[0], [1.0, 0x2c as f32 / 255.0, 0x2c as f32 / 255.0, 0.0]);
    assert_eq!(direct.colors[1], [1.0, 0x52 as f32 / 255.0, 0x53 as f32 / 255.0, 8.0 / 255.0], "selected: brighter, a little glow");
    assert_eq!(direct.outer, [0.04, 4.0]);
    let radar = styles[kind("RADAR")].expect("radar");
    assert_eq!(radar.colors[0][..3], [0x15 as f32 / 255.0, 0x6f as f32 / 255.0, 0x79 as f32 / 255.0]);
    assert!(styles.iter().all(Option::is_some), "every kind has an overlay");
}

/// PNG interface textures (FAF's hotbuild key label background) decode and
/// size their bitmaps, as DDS ones do.
#[test]
fn png_ui_textures_load() {
    let Some(vm) = vm() else { return };
    let (w, h) = rts_core_engine::ui::controls::texture_dimensions(vm.lua(), "/textures/ui/bg.png");
    assert!(w > 0 && h > 0, "layout size {w}x{h}");
    let vfs = vm.lua().app_data_ref::<Arc<std::sync::RwLock<rts_core_engine::engine::vfs::Vfs>>>().unwrap().clone();
    let bytes = vfs.read().unwrap().read("/textures/ui/bg.png").unwrap();
    let t = rts_core_engine::engine::assets::dds::read_image(&bytes).expect("decodes");
    assert_eq!((t.width, t.height), (w, h));
    assert_eq!(t.levels[0].len(), (w * h * 4) as usize, "one RGBA level");
}

/// Moho's `OnHide`: returning true keeps it from that control's own
/// children only (siblings still get it), and a parent's alpha isn't passed
/// down. FAF's economy bar relies on both for its income/reclaim columns.
#[test]
fn on_hide_prunes_one_branch_and_alpha_is_per_control() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.lua()
        .load(
            r#"
            local Group = import('/lua/maui/group.lua').Group
            local Text = import('/lua/maui/text.lua').Text
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            calls = {}
            local root = Group(GetFrame(0))
            LayoutHelpers.FillParent(root, GetFrame(0))
            local first = Group(root)
            first.OnHide = function(self, hidden) table.insert(calls, 'first') return true end
            local inner = Group(first)
            inner.OnHide = function(self, hidden) table.insert(calls, 'inner') end
            local second = Group(root)
            second.OnHide = function(self, hidden) table.insert(calls, 'second') end
            root:Hide()
            root:Show()
            local faded = Group(GetFrame(0))
            LayoutHelpers.FillParent(faded, GetFrame(0))
            local label = Text(faded)
            label:SetFont('Arial', 14)
            label:SetText('still opaque')
            LayoutHelpers.AtLeftTopIn(label, faded, 10, 10)
            faded:SetAlpha(0)
            "#,
        )
        .exec()
        .unwrap();
    let calls: Vec<String> = vm.lua().load("return calls").eval::<Vec<String>>().unwrap();
    assert_eq!(calls, ["first", "second", "first", "second"]);
    let alpha = vm.draw_list().unwrap().into_iter().find_map(|i| match i {
        DrawItem::Text { text, color, .. } if text == "still opaque" => Some(color[3]),
        _ => None,
    });
    assert_eq!(alpha, Some(1.0));
}

/// Esc in the main menu reaches FAF's escape handlers (the key map's
/// `escape`, which the front end has): the exit question, and Esc again
/// answers no. The lobby's changelog (FAF opens it on a new version) closes
/// with its X button and with Esc (from the lobby's chat box).
#[test]
fn front_end_escape_and_the_lobby_changelog() {
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    let Some(vm) = vm() else { return };
    vm.set_screen(1024.0, 768.0).unwrap();
    vm.start_front_end().expect("main menu");
    let run = |seconds: f32| {
        for _ in 0..(seconds * 60.0) as u32 {
            vm.frame(1.0 / 60.0).unwrap();
        }
    };
    run(7.0);
    // FAF's exit question is a QuickDialog (kept in a local): watch for it.
    vm.lua()
        .load("local UIUtil = import('/lua/ui/uiutil.lua') local quick = UIUtil.QuickDialog UIUtil.QuickDialog = function(...) local d = quick(...) rawset(_G, 'TestDialog', d) return d end")
        .exec()
        .unwrap();
    let exit_question = || -> bool { vm.lua().load("local d = rawget(_G, 'TestDialog') return d ~= nil and not IsDestroyed(d)").eval().unwrap() };
    assert!(vm.key("Esc", false, false, false).unwrap(), "Esc is bound in the main menu");
    run(0.5);
    assert!(exit_question(), "Esc asks whether to exit");
    vm.key("Esc", false, false, false).unwrap();
    run(0.5);
    assert!(!exit_question(), "Esc again answers no");
    vm.lua()
        .load("import('/lua/user/prefs.lua').SetToCurrentProfile('MenuTutorialPrompt', true) import('/lua/ui/menus/main.lua').ButtonSkirmish()")
        .exec()
        .unwrap();
    run(3.0);
    let open = || -> bool { vm.lua().load("return import('/lua/ui/lobby/changelog/changelogdialog.lua').ChangelogInstance and true or false").eval().unwrap() };
    assert!(open(), "the lobby opened the changelog");
    let (x, y): (f32, f32) = vm
        .lua()
        .load("local b = import('/lua/ui/lobby/changelog/changelogdialog.lua').ChangelogInstance.HeaderEscapeButton return (b.Left() + b.Right()) / 2, (b.Top() + b.Bottom()) / 2")
        .eval()
        .unwrap();
    let left = Modifiers { left: true, ..Default::default() };
    vm.mouse(MouseEvent::Move, x, y, Modifiers::default()).unwrap();
    vm.mouse(MouseEvent::Press(1), x, y, left).unwrap();
    vm.mouse(MouseEvent::Release(1), x, y, Modifiers::default()).unwrap();
    run(0.5);
    assert!(!open(), "the X closed it");

    vm.lua().load("import('/lua/ui/lobby/changelog/changelogdialog.lua').CreateChangelogDialog(GetFrame(0))").exec().unwrap();
    run(0.5);
    assert!(open());
    assert!(vm.key("Esc", false, false, false).unwrap(), "Esc is bound in the front end");
    run(0.5);
    assert!(!open(), "Esc closed it");
}

/// `GetArmyAvatars`: the focus army's units whose blueprint has
/// `General.QuickSelectPriority` (FAF's ACUs), which FAF's first update
/// zooms to and its avatar panel shows; nil without any.
#[test]
fn army_avatars_are_the_focus_armys_commanders() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_focus_army(1).unwrap();
    let unit = |id, blueprint: &str, army| UnitInfo { id, blueprint: blueprint.into(), army, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [100.0, 20.0, 200.0], work: 0.0, shield: 0.0, detail: None };
    vm.set_world(vec![unit(5, "uel0105", 1), unit(7, "uel0001", 1), unit(9, "ual0001", 2)]);
    let ids: Vec<String> = vm.lua().load("local out = {} for _, u in GetArmyAvatars() or {} do table.insert(out, u:GetEntityId() .. ' ' .. tostring(u:IsInCategory('COMMAND'))) end return out").eval().unwrap();
    assert_eq!(ids, ["7 true"], "army 1's ACU, not its engineer or army 2's ACU");
    vm.set_world(vec![unit(5, "uel0105", 1)]);
    assert!(vm.lua().load("return GetArmyAvatars() == nil").eval::<bool>().unwrap(), "nil without one");
}

/// The lobby's map preview (`MapPreview`) shows the map's own preview image
/// (`SetTextureFromMap`) when the scenario names no image file.
#[test]
fn map_preview_draws_the_maps_image() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1024.0, 768.0).unwrap();
    let (own, from_map): (bool, bool) = vm
        .lua()
        .load(
            r#"
            local MapPreview = import('/lua/ui/controls/mappreview.lua').MapPreview
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local preview = MapPreview(GetFrame(0))
            LayoutHelpers.AtLeftTopIn(preview, GetFrame(0), 10, 10)
            preview.Width:Set(200) preview.Height:Set(200)
            return preview:SetTexture(''), preview:SetTextureFromMap('/maps/scmp_009/scmp_009.scmap')
            "#,
        )
        .eval()
        .unwrap();
    assert!(!own && from_map);
    let drawn = vm.draw_list().unwrap().into_iter().any(|i| matches!(i, DrawItem::Quad { texture: Some(t), rect, .. } if t == "#scmap:/maps/scmp_009/scmp_009.scmap" && rect == [10.0, 10.0, 210.0, 210.0]));
    assert!(drawn, "the preview draws the map's image");
}

/// The profile's sound options set the volumes at start (moho's
/// `optionslogic.Apply(true)`); without an audio device they still read back.
#[test]
fn profile_volumes_apply() {
    let Some(vm) = vm() else { return };
    let (music, vo, world): (f32, f32, f32) = vm
        .lua()
        .load(
            "local Prefs = import('/lua/user/prefs.lua') \
             local o = Prefs.GetFromCurrentProfile('options') or {} \
             o.music_volume = 40 o.vo_volume = 0 o.fx_volume = 75 \
             Prefs.SetToCurrentProfile('options', o)",
        )
        .exec()
        .and_then(|_| vm.apply_startup_options())
        .and_then(|_| vm.lua().load("return GetVolume('Music'), GetVolume('VO'), GetVolume('World')").eval())
        .expect("sound options");
    assert_eq!((music, vo, world), (0.4, 0.0, 0.75));
}

/// The profile's Level Of Detail applies at start too: FAF's setter hands
/// the engine `SC_CameraScaleLOD <level>` (the camera's LOD scale).
#[test]
fn profile_level_of_detail_applies() {
    let Some(vm) = vm() else { return };
    vm.take_console().unwrap();
    vm.lua()
        .load(
            "local Prefs = import('/lua/user/prefs.lua') \
             local o = Prefs.GetFromCurrentProfile('options') or {} \
             o.level_of_detail = 2 \
             Prefs.SetToCurrentProfile('options', o)",
        )
        .exec()
        .and_then(|_| vm.apply_startup_options())
        .expect("options");
    let console = vm.take_console().unwrap();
    assert!(console.iter().any(|l| l == "SC_CameraScaleLOD 2"), "{console:?}");
}

/// An enhancement ordered from the interface reaches FAF's
/// `OnCommandIssued` with its `LuaParams`, so the enhancement queue (the
/// construction panel's enhancement item and progress bar) has it.
#[test]
fn enhancement_orders_reach_the_enhancement_queue() {
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget};
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    vm.set_world(vec![UnitInfo { id: 42, blueprint: "uel0001".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [100.0, 20.0, 200.0], work: 0.0, shield: 0.0, detail: None }]);
    vm.frame(1.0 / 60.0).unwrap();
    let script = [("TaskName".to_string(), "EnhanceTask".to_string()), ("Enhancement".to_string(), "AdvancedEngineering".to_string())];
    vm.command_issued(CommandKind::Script, CommandTarget::None, false, None, &[42], &script).unwrap();
    let queued: Option<String> = vm
        .lua()
        .load("local q = import('/lua/ui/notify/enhancementqueue.lua').getEnhancementQueue()['42'] return q and q[1] and q[1].Name")
        .eval()
        .unwrap();
    assert_eq!(queued.as_deref(), Some("<LOC enhancements_0037>Tech 2 Engineering Suite"));
}

/// Engineers ordered to assist a finished T2 extractor cap it with mass
/// storages: the guard names the extractor's blueprint, so FAF's
/// `OnGuardIssued` hands it to `capping.lua`, which asks the sim to ring it.
#[test]
fn assisting_a_tech2_extractor_caps_it_with_storages() {
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget};
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    let unit = |id, bp: &str, x| UnitInfo { id, blueprint: bp.into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [x, 20.0, 200.0], work: 0.0, shield: 0.0, detail: None };
    vm.set_world(vec![unit(42, "uel0105", 100.0), unit(43, "ueb1202", 110.0)]);
    vm.frame(1.0 / 60.0).unwrap();
    vm.set_selection(&[42]).unwrap();
    // The order table as FAF's exe hands it over (FAF probe probe_guardcmd):
    // the guarded unit's blueprint, its id and position.
    vm.lua().load("local cm = import('/lua/ui/game/commandmode.lua') local f = cm.OnCommandIssued cm.OnCommandIssued = function(c) SeenCommand = c return f(c) end").exec().unwrap();
    vm.command_issued(CommandKind::Guard, CommandTarget::Entity(43), false, None, &[42], &[]).unwrap();
    let seen: String = vm.lua().load("local c = SeenCommand return c.Blueprint .. ' ' .. c.Target.Type .. ' ' .. c.Target.EntityId .. ' ' .. c.Target.Position[1]").eval().unwrap();
    assert_eq!(seen, "ueb1202 Entity 43 110");
    let callbacks: Vec<_> = vm.take_orders().unwrap().into_iter().filter_map(|o| o.callback.map(|c| (c.func, o.units))).collect();
    assert!(callbacks.iter().any(|(func, units)| func == "RingWithStorages" && units.as_deref() == Some(&[42][..])), "{callbacks:?}");
}

/// Enhancement orders are in the unit's command queue as soon as they're
/// given (moho's UserUnit queue has them at once; the sim's details come a
/// snapshot later): FAF's construction panel counts the queue's Script
/// commands against its enhancement queue at OnCommandIssued and dropped
/// both queued enhancements when they weren't there yet. The
/// prediction gives way to the sim's queue when it changes, or after 2 s.
#[test]
fn enhancement_orders_stay_queued_until_the_sim_confirms() {
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget};
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.start_hud().expect("game interface");
    let acu = |commands: Vec<(u8, Option<String>, u32, [f32; 3])>| UnitInfo { id: 42, blueprint: "uel0001".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [100.0, 20.0, 200.0], work: 0.0, shield: 0.0, detail: Some(UnitDetail { id: 42, commands, ..Default::default() }) };
    vm.set_world(vec![acu(vec![])]);
    vm.update_units(&[acu(vec![])]).unwrap();
    vm.set_selection(&[42]).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    // What the enhancement tab writes into the blueprint's entries when it opens.
    vm.lua().load("for name, t in __blueprints['uel0001'].Enhancements do if type(t) == 'table' and t.Slot then t.ID = name t.UnitID = 'uel0001' end end").exec().unwrap();
    for enhancement in ["AdvancedEngineering", "Shield"] {
        let script = [("TaskName".to_string(), "EnhanceTask".to_string()), ("Enhancement".to_string(), enhancement.to_string())];
        vm.command_issued(CommandKind::Script, CommandTarget::None, true, None, &[42], &script).unwrap();
    }
    let state = || -> String {
        vm.lua()
            .load(
                "local q = import('/lua/ui/notify/enhancementqueue.lua').getEnhancementQueue()['42'] or {} \
                 local c = {} for _, e in __engine_unit(42):GetCommandQueue() do table.insert(c, e.type) end \
                 local n = {} for _, e in q do table.insert(n, e.ID) end \
                 return table.concat(n, ',') .. ' | ' .. table.concat(c, ',')",
            )
            .eval()
            .unwrap()
    };
    assert_eq!(state(), "AdvancedEngineering,Shield | Script,Script");
    // The sim hasn't confirmed them yet: the same (empty) details keep the prediction.
    vm.update_units(&[acu(vec![])]).unwrap();
    assert_eq!(state(), "AdvancedEngineering,Shield | Script,Script");
    // The sim's queue arrives: its commands replace the prediction.
    let script = CommandKind::Script as u8;
    vm.update_units(&[acu(vec![(script, None, 1, [0.0; 3]), (script, None, 1, [0.0; 3])])]).unwrap();
    assert_eq!(state(), "AdvancedEngineering,Shield | Script,Script");
    // An order the sim never confirms goes after 2 s.
    vm.command_issued(CommandKind::Script, CommandTarget::None, false, None, &[42], &[]).unwrap();
    assert_eq!(state().split(" | ").nth(1), Some("Script"), "an unqueued order replaces the queue");
    for _ in 0..150 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.update_units(&[acu(vec![(script, None, 1, [0.0; 3]), (script, None, 1, [0.0; 3])])]).unwrap();
    assert_eq!(state().split(" | ").nth(1), Some("Script,Script"));
}

/// Hovering a structure in the build menu shows FAF's extended tooltip:
/// its description, armour, weapons (`gui_render_armament_detail`) and
/// vision lines, laid out on screen.
#[test]
fn build_tooltip_shows_weapon_details() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[rts_core_engine::engine::snapshot::ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false }]).unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let report: String = vm
        .lua()
        .load(
            r#"
            local detail = import('/lua/ui/game/unitviewdetail.lua')
            detail.Show(__blueprints['ueb2101'], nil, 'ueb2101')
            local V = detail.View
            local D = V and V.Description
            if not D then return 'no Description (uvd_format ' .. tostring(Prefs.GetOption('uvd_format')) .. ')' end
            local lines = {}
            for i, t in D.Value or {} do
                if not t:IsHidden() and t:GetText() ~= '' then table.insert(lines, t:GetText()) end
            end
            return string.format('hidden=%s view_hidden=%s at %d,%d %dx%d | %s', tostring(D:IsHidden()), tostring(V:IsHidden()), D.Left(), D.Top(), D.Width(), D.Height(), table.concat(lines, ' / '))
            "#,
        )
        .eval()
        .unwrap();
    eprintln!("{report}");
    assert!(report.contains("hidden=false"), "{report}");
    assert!(report.contains("DPS"), "the weapon line: {report}");
}

/// The game ends: `SessionIsGameOver` turns true and FAF's
/// `uimain.NoteGameOver` runs once (focus army -1, "Game over." announced);
/// the score screen's `SessionEndGame` asks the engine to leave, taken once.
#[test]
fn game_over_reaches_the_interface() {
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    let lua = vm.lua();
    assert!(!lua.load("return SessionIsGameOver()").eval::<bool>().unwrap());
    // FAF's own handler, raising rather than logging its errors.
    lua.load("import('/lua/ui/uimain.lua').NoteGameOver()").exec().expect("NoteGameOver");
    vm.set_focus_army(1).unwrap();
    vm.set_game_over().unwrap();
    assert!(lua.load("return SessionIsGameOver()").eval::<bool>().unwrap());
    assert_eq!(lua.load("return GetFocusArmy()").eval::<i64>().unwrap(), -1, "NoteGameOver makes everyone an observer");
    vm.set_focus_army(1).unwrap();
    vm.set_game_over().unwrap();
    assert_eq!(lua.load("return GetFocusArmy()").eval::<i64>().unwrap(), 1, "noted once");
    assert!(!vm.take_session_left().unwrap());
    lua.load("SessionEndGame()").exec().unwrap();
    assert!(vm.take_session_left().unwrap());
    assert!(!vm.take_session_left().unwrap(), "taken once");
    // FAF's score screen's Continue (`ExitGame`) returns to the main menu,
    // not an exit of the engine (`UserVm::take_menu_request`).
    lua.load("ExitGame()").exec().unwrap();
    assert!(vm.take_session_left().unwrap());
    assert!(vm.take_menu_request().unwrap());
    assert!(!vm.take_menu_request().unwrap(), "taken once");
    assert!(!vm.exit_requested().unwrap(), "ExitGame returns to the menu, not out of the engine");
}

/// Ending a session with no game over (FAF's `/end-mission`, quitting):
/// moho hands the interface its score data at a session's end, which FAF's
/// score screen waits for (`hotstats.scoreData`); without it its wait
/// thread would spin forever and the end-game screen never shows. The
/// engine fills it (`SessionEndGame`) in the shape of FAF's sim score
/// table, and the score screen then builds.
#[test]
fn session_end_gives_the_score_screen_its_data() {
    use rts_core_engine::engine::snapshot::ArmyInfo;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[
        ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false },
        ArmyInfo { index: 2, name: "ARMY_2".into(), nickname: "Rival".into(), faction: 3, human: false, civilian: false },
    ])
    .unwrap();
    vm.start_hud().expect("game interface");
    let ready = || {
        vm.lua()
            .load("local s = import('/lua/ui/dialogs/hotstats.lua').scoreData return not not (s.interval and s.current and s.history)")
            .eval::<bool>()
            .unwrap_or(false)
    };
    assert!(!ready(), "nothing has score data before the session ends");
    vm.lua().load("SessionEndGame()").exec().unwrap();
    assert!(ready(), "SessionEndGame gives the score screen its data");
    let names: String = vm
        .lua()
        .load("local s = import('/lua/ui/dialogs/hotstats.lua').scoreData local names = {} for _, c in s.current do table.insert(names, tostring(c.name)) end return table.concat(names, ', ')")
        .eval()
        .expect("scores");
    assert!(names.contains("Player") && names.contains("Rival"), "the armies' names in the score data: {names}");
    // The score screen FAF's score tab opens: it waits (its thread pumps
    // frames) for the data, then builds the dialog.
    vm.lua().load("import('/lua/ui/game/tabs.lua').EndGame()").exec().unwrap();
    for _ in 0..90 {
        vm.frame(1.0 / 60.0).expect("score frames");
    }
    assert!(
        vm.lua().load("return import('/lua/ui/dialogs/score.lua').scoreScreenActive").eval::<bool>().unwrap(),
        "the score screen is up"
    );
    let texts: Vec<String> = vm
        .draw_list()
        .unwrap()
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t.contains("Continue")), "the score screen's dialog built: {texts:?}");
}

/// `SetFocusArmy` takes a 0-based index (FAF's score board passes
/// `armyID - 1`); `GetFocusArmy` is 1-based.
#[test]
fn set_focus_army_is_zero_based() {
    let Some(vm) = vm() else { return };
    let r: (i64, i64) = vm.lua().load("SetFocusArmy(0) local a = GetFocusArmy() SetFocusArmy(-1) return a, GetFocusArmy()").eval().expect("focus");
    assert_eq!(r, (1, -1));
    assert_eq!(vm.focus_army(), None);
}

/// A profile list as FAF writes it (`profiles` an array, `current` an
/// index): `GetPreference('profile.profiles.1.options')` reaches it, so the
/// options survive init (FAF's `GetCurrent` would otherwise fill a fresh
/// set of defaults over them) and `GetOptions` reads them.
#[test]
fn preferences_reach_profiles_by_index() {
    if !rts_core_engine::test_data::require_game("UI tests") {
        return;
    }
    let engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let vm = UserVm::with_cache(engine.vfs.clone(), engine.script_cache.clone()).expect("user vm");
    let p = std::env::temp_dir().join(format!("profiles-by-index-{}.prefs", std::process::id()));
    std::fs::write(&p, "profile = {\n current = 1,\n profiles = {\n { Name = 'tester', options = { secondary_adapter = '1920,1080,60', vsync = 0 } },\n },\n}\n").unwrap();
    vm.load_prefs(&p).unwrap();
    let _ = std::fs::remove_file(&p);
    vm.init().expect("userInit.lua");
    let read: (String, i64, String) = vm.lua().load("return GetOptions('secondary_adapter'), GetOptions('vsync'), import('/lua/user/prefs.lua').GetCurrentProfile().Name").eval().unwrap();
    assert_eq!(read, ("1920,1080,60".to_string(), 0, "tester".to_string()));
}

/// Picture-in-picture popups from the sim's requests: a player sees their
/// own side's and allies' events, never an enemy's; observers see all; a
/// reason turned off shows nothing; the same event replaces its popup.
#[test]
fn pip_requests_follow_the_focus_army() {
    use rts_core_engine::engine::sim::army::Alliance;
    let Some(vm) = vm() else { return };
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_alliances(&[(1, 3, Alliance::Ally), (3, 1, Alliance::Ally)]).unwrap();
    vm.start_hud().expect("game interface");
    vm.frame(1.0 / 60.0).unwrap();
    let shown = |code: &str| -> i64 {
        vm.lua()
            .load(format!("local PiP = import('/lua/ui/game/pip.lua') PiP.SetSlots(0) PiP.SetSlots(4) {code} return table.getn(PiP.Popups())"))
            .eval()
            .unwrap()
    };
    let request = |army: u32, reason: &str, id: u32| format!("{{ army = {army}, reason = '{reason}', entities = {{ {id} }}, position = {{ 100, 20, 100 }} }}");
    let send = |list: &[String]| format!("PiP.OnRequests({{ {} }})", list.join(", "));
    assert_eq!(shown(&send(&[request(1, "acu_death", 5)])), 1, "own commander");
    assert_eq!(shown(&send(&[request(3, "experimental_started", 6)])), 1, "an ally's experimental");
    assert_eq!(shown(&send(&[request(2, "acu_death", 7)])), 0, "an enemy's: never");
    assert_eq!(shown(&send(&[request(1, "acu_death", 5), request(1, "acu_death", 5)])), 1, "the same event replaces its popup");
    assert_eq!(shown(&format!("PiP.Reasons.acu_death = false {}", send(&[request(1, "acu_death", 5)]))), 0, "turned off");
    let keys: String = vm.lua().load("local items = import('/lua/options/options.lua').options.ui.items local keys = {} for _, it in items do if string.find(it.key or '', '^pip_') then table.insert(keys, it.key) end end return table.concat(keys, ',')").eval().unwrap();
    assert_eq!(keys, "pip_acu_death,pip_experimental_started", "the Interface tab's options");
    let option_off = "local Prefs = import('/lua/user/prefs.lua') local o = Prefs.GetFromCurrentProfile('options') or {} o.pip_experimental_started = 0 Prefs.SetToCurrentProfile('options', o)";
    assert_eq!(shown(&format!("{option_off} {}", send(&[request(1, "experimental_started", 9)]))), 0, "the player's option off");
    vm.set_focus_army(-1).unwrap();
    assert_eq!(shown(&format!("PiP.Reasons.acu_death = {{ zoom = 40 }} {}", send(&[request(2, "acu_death", 7), request(4, "mod_event", 8)]))), 2, "observers see every side, and a mod's reason");
}

/// The create-unit (cheat) dialog sorts units by layer from the registered
/// `Physics.BuildOnLayerCaps` text: naval structures go under sea,
/// not land. Only the two Land+Air structures (`'17'`) aren't identified:
/// FAF's `GetLayerGroup` takes `17 mod 16 = 1` for impossible, in the real
/// game too.
#[test]
fn create_unit_dialog_identifies_every_layer() {
    let Some(vm) = vm() else { return };
    let (unknown, naval, extractor, sonar): (String, String, String, String) = vm
        .lua()
        .load(
            r#"
            local unknown = {}
            local alert = _ALERT
            _ALERT = function(_, id, cap) table.insert(unknown, tostring(id) .. '=' .. tostring(cap)) end
            local logic = import('/lua/ui/dialogs/createunit.logic.lua')
            for id, bp in __blueprints do
                if type(id) == 'string' and bp.Physics and bp.Physics.MotionType then logic.GetLayerGroup(id) end
            end
            _ALERT = alert
            table.sort(unknown)
            return table.concat(unknown, ' '), logic.GetLayerGroup('ueb0103'), logic.GetLayerGroup('uab1103'), logic.GetLayerGroup('ueb3102')
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!((unknown.as_str(), naval.as_str(), extractor.as_str(), sonar.as_str()), ("ueb5208=17 urb5206=17", "sea", "amph", "sea"));
}

/// FAF's draggable build queue (option `gui_draggable_queue`, off by
/// default): pressing a factory's queue entry, moving over another and
/// releasing reorders the queue (construction.lua's `MoveItemInQueue` /
/// `UpdateBuildList`: the queue cut back to the first moved entry, then
/// built again in the new order).
#[test]
fn dragging_a_queue_entry_reorders_the_factory_queue() {
    use rts_core_engine::engine::snapshot::UnitDetail;
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.lua().load("import('/lua/user/prefs.lua').SetOption('gui_draggable_queue', 1)").exec().unwrap();
    vm.set_screen(1280.0, 720.0).unwrap();
    vm.set_focus_army(1).unwrap();
    vm.set_armies(&[rts_core_engine::engine::snapshot::ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false }]).unwrap();
    vm.start_hud().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    vm.take_selection_request().unwrap();
    let bf = rts_core_engine::engine::sim::command::CommandKind::BuildFactory as u8;
    let detail = UnitDetail {
        id: 9,
        commands: vec![(bf, Some("uel0201".into()), 2, [0.0; 3]), (bf, Some("uel0105".into()), 1, [0.0; 3]), (bf, Some("uel0101".into()), 1, [0.0; 3])],
        ..Default::default()
    };
    let factory = UnitInfo { id: 9, blueprint: "ueb0101".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [400.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: Some(detail) };
    vm.update_units(std::slice::from_ref(&factory)).unwrap();
    vm.set_selection(&[9]).unwrap();
    for f in 0..30 {
        if f % 6 == 0 {
            vm.update_units(std::slice::from_ref(&factory)).unwrap();
            vm.beat(f / 6, &[]).unwrap();
        }
        vm.frame(1.0 / 60.0).unwrap();
    }
    let icons: Vec<(String, [f32; 4])> = vm.draw_list().unwrap().into_iter().filter_map(|i| match i {
        DrawItem::Quad { texture: Some(t), rect, .. } if t.contains("/icons/units/") => Some((t, rect)),
        _ => None,
    }).collect();
    eprintln!("unit icons: {icons:?}");
    // The queue row: the lowest row of icons (the build tab sits above).
    let top = icons.iter().map(|(_, r)| r[1]).fold(f32::MIN, f32::max);
    let queue: Vec<&(String, [f32; 4])> = icons.iter().filter(|(_, r)| (r[1] - top).abs() < 1.0).collect();
    let centre = |id: &str| {
        let (_, r) = queue.iter().find(|(t, _)| t.contains(id)).unwrap_or_else(|| panic!("{id} in the queue row: {queue:?}"));
        ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
    };
    let (from, to) = (centre("uel0201"), centre("uel0101"));
    vm.take_orders().unwrap();
    let left = Modifiers { left: true, ..Default::default() };
    vm.mouse(MouseEvent::Move, from.0, from.1, Modifiers::default()).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    vm.mouse(MouseEvent::Press(1), from.0, from.1, left).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    vm.mouse(MouseEvent::Move, (from.0 + to.0) / 2.0, from.1, left).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    vm.mouse(MouseEvent::Move, to.0, to.1, left).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    vm.mouse(MouseEvent::Release(1), to.0, to.1, Modifiers::default()).unwrap();
    vm.frame(1.0 / 60.0).unwrap();
    let orders: Vec<String> = vm.take_orders().unwrap().into_iter().map(|o| match o.toggle {
        Some(t) => format!("{t:?}"),
        None => format!("{} {} x{}", o.command, o.blueprint.unwrap_or_default(), o.count),
    }).collect();
    eprintln!("orders: {orders:?}");
    assert_eq!(
        orders,
        [
            "BuildCount(3, -1)",
            "BuildCount(2, -1)",
            "BuildCount(1, -2)",
            "UNITCOMMAND_BuildFactory uel0105 x1",
            "UNITCOMMAND_BuildFactory uel0101 x1",
            "UNITCOMMAND_BuildFactory uel0201 x2",
        ]
    );
}

/// A unit's maximum health and build rate changed by scripts (here 5000
/// instead of a scout's 29, and 45): the interface's `GetMaxHealth` /
/// `GetHealth` / `GetBuildRate` follow the sim, not the blueprint.
#[test]
fn ui_unit_values_follow_the_sim() {
    use rts_core_engine::ui::UnitInfo;
    let Some(vm) = vm() else { return };
    vm.update_units(&[UnitInfo { id: 5, blueprint: "uel0101".into(), army: 1, health: 0.25, max_health: 5000.0, build_rate: 45.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: None }]).unwrap();
    let got: (f64, f64, f64) = vm.lua().load("local u = __engine_unit(5) return u:GetHealth(), u:GetMaxHealth(), u:GetBuildRate()").eval().unwrap();
    assert_eq!(got, (1250.0, 5000.0, 45.0));
}

/// What the sim changes reaches the interface's UserUnit: `SetBuildRate`
/// (an ACU's engineering upgrade) through the unit's snapshot view, and
/// `SetCustomName` (AI and campaign commanders) through the snapshots'
/// name list; a name given in the interface (FAF's rename dialog) wins.
#[test]
fn sim_build_rate_and_names_reach_the_interface() {
    use rts_core_engine::ui::UnitInfo;
    if !rts_core_engine::test_data::require_game("UI tests") {
        return;
    }
    let mut engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let session = engine::session::SessionConfig { ticks: Some(0), create_initial_units: false, ai: false, ..Default::default() };
    engine.start(&session).expect("session");
    let (sink, rx, _request) = engine::snapshot::channel();
    engine.set_snapshot_sink(sink);
    let acu: u32 = engine
        .sim_lua()
        .load("local acu = CreateUnitHPR('uel0001', 'ARMY_1', 400, GetTerrainHeight(400, 640), 640, 0, 0, 0) acu:SetBuildRate(45) acu:SetCustomName('Brick') return tonumber(acu:GetEntityId())")
        .eval()
        .unwrap();
    let (mut view, mut names) = (None, None);
    for _ in 0..3 {
        engine.tick().expect("tick");
        while let Ok(mut snap) = rx.try_recv() {
            drop(snap.ticket.take());
            view = snap.entities.iter().find(|e| e.id == acu).copied().or(view);
            names = snap.custom_names.take().or(names);
        }
    }
    let view = view.expect("the ACU in a snapshot");
    let names = names.expect("the names sent");
    assert_eq!(view.build_rate, 45.0);
    assert_eq!(&names[..], &[(acu, "Brick".to_string())]);

    let vm = UserVm::with_cache(engine.vfs.clone(), engine.script_cache.clone()).expect("user vm");
    vm.init().expect("userInit.lua");
    vm.update_units(&[UnitInfo::from_view(&view, "uel0001".into())]).unwrap();
    vm.set_custom_names(&names).unwrap();
    let got: (f64, String) = vm.lua().load(format!("local u = GetUnitById('{acu}') return u:GetBuildRate(), u:GetCustomName()")).eval().unwrap();
    assert_eq!(got, (45.0, "Brick".to_string()));
    let renamed: String = vm.lua().load(format!("local u = GetUnitById('{acu}') u:SetCustomName('Mine') return u:GetCustomName()")).eval().unwrap();
    assert_eq!(renamed, "Mine");
}
