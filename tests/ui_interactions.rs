//! Scripted interface interactions: FAF's real interaction code (every
//! hotkey action, draggers, panels) driven in the game interface with units
//! selected, failing on any Lua warning it logs. Placeholders that warn
//! ("not implemented yet") and script errors both count, so these find what
//! a player hits that headless sim runs never reach. Needs the FAF checkout
//! and game data.

use std::sync::{Arc, Mutex, OnceLock};

use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine;
use rts_core_engine::ui::{UnitInfo, UserVm};

const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

/// Warnings logged, with the thread that logged them (tests run in
/// parallel; each reads its own).
fn captured() -> &'static Mutex<Vec<(std::thread::ThreadId, String)>> {
    static LOG: OnceLock<Mutex<Vec<(std::thread::ThreadId, String)>>> = OnceLock::new();
    LOG.get_or_init(Default::default)
}

struct Capture;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if *event.metadata().level() > tracing::Level::WARN {
            return;
        }
        struct Text(String);
        impl tracing::field::Visit for Text {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                if field.name() == "message" {
                    self.0.push_str(&format!("{value:?}"));
                } else {
                    self.0.push_str(&format!("{}={value:?}", field.name()));
                }
            }
        }
        let mut text = Text(String::new());
        event.record(&mut text);
        captured().lock().unwrap().push((std::thread::current().id(), text.0));
    }
}

/// This thread's warnings since the last call.
fn take_warnings() -> Vec<String> {
    let me = std::thread::current().id();
    let mut log = captured().lock().unwrap();
    let (mine, rest): (Vec<_>, Vec<_>) = log.drain(..).partition(|(t, _)| *t == me);
    *log = rest;
    mine.into_iter().map(|(_, m)| m).collect()
}

/// The game interface with a few of army 1's units known and the camera
/// over them (for picking), or `None` when skipping without game data
/// (`RTS_SKIP_DATA_TESTS=1`; otherwise it fails).
fn hud() -> Option<UserVm> {
    use tracing_subscriber::layer::SubscriberExt;
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Capture));
    });
    // Fails without the game data (`RTS_SKIP_DATA_TESTS=1` skips instead).
    if !rts_core_engine::test_data::require_game("UI interaction tests") {
        return None;
    }
    let engine = engine::Engine::new(INIT_LUA_PATH, FA_PATH, Arc::new(Mutex::new(DebugUiState::new()))).expect("engine");
    let vm = UserVm::with_cache(engine.vfs.clone(), engine.script_cache.clone()).expect("user vm");
    vm.init().expect("userInit.lua");
    // What the game hands the interface before it starts (render::frame):
    // the armies (1 against 2), their alliances, the scenario, the focus.
    use rts_core_engine::engine::sim::army::Alliance;
    use rts_core_engine::engine::snapshot::ArmyInfo;
    use rts_core_engine::engine::sync::SyncValue as V;
    vm.set_screen(1024.0, 768.0).unwrap();
    vm.set_armies(&[
        ArmyInfo { index: 1, name: "ARMY_1".into(), nickname: "Player".into(), faction: 1, human: true, civilian: false },
        ArmyInfo { index: 2, name: "ARMY_2".into(), nickname: "Rival".into(), faction: 3, human: false, civilian: false },
    ])
    .unwrap();
    vm.set_alliances(&[(1, 2, Alliance::Enemy), (2, 1, Alliance::Enemy)]).unwrap();
    let s = |x: &str| V::Str(x.into());
    vm.set_scenario(&V::Table(vec![
        (s("name"), s("Seton's Clutch")),
        (s("description"), s("A test map")),
        (s("type"), s("skirmish")),
        (s("size"), V::Table(vec![(V::Num(1.0), V::Num(1024.0)), (V::Num(2.0), V::Num(1024.0))])),
        (s("PlayableAreaWidth"), V::Num(1024.0)),
        (s("PlayableAreaHeight"), V::Num(1024.0)),
        (s("Options"), V::Table(vec![(s("Share"), s("ShareUntilDeath")), (s("Victory"), s("demoralization"))])),
    ]))
    .unwrap();
    vm.set_focus_army(1).unwrap();
    vm.start_hud().expect("game interface");
    // A camera 80 above (400, 600) looking straight down at flat ground.
    let eye = glam::Vec3::new(400.0, 100.0, 600.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, glam::Vec3::new(400.0, 20.0, 600.0), glam::Vec3::NEG_Z);
    let proj = glam::camera::rh::proj::directx::perspective(1.0, 1024.0 / 768.0, 1.0, 2000.0);
    let terrain = rts_core_engine::engine::snapshot::TerrainView { width: 1024, height: 1024, heights: vec![20.0; 1025 * 1025], water: None, map: None };
    vm.set_pick_views(vec![rts_core_engine::ui::world_pick::PickView {
        camera: "WorldCamera".into(),
        origin: [0.0, 0.0],
        view_proj: proj * view,
        inv_view_proj: (proj * view).inverse(),
        screen: [1024.0, 768.0],
        ui_scale: 1.0,
        target_y: 20.0,
        distance: 80.0,
        terrain: Some(Arc::new(terrain)),
    }]);
    let unit = |id: u32, blueprint: &str, x: f32| UnitInfo { id, blueprint: blueprint.into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [x, 20.0, 600.0], work: 0.0, shield: 0.0, detail: None };
    vm.update_units(&[unit(1, "uel0001", 390.0), unit(2, "uel0105", 395.0), unit(3, "uel0201", 400.0), unit(4, "ueb0101", 410.0), unit(5, "uea0203", 405.0)]).unwrap();
    // The mouse over the middle of the screen (the world view).
    vm.lua().load("rawset(_G, '__engine_mouse_x', 512) rawset(_G, '__engine_mouse_y', 384)").exec().unwrap();
    for _ in 0..420 {
        vm.frame(1.0 / 60.0).unwrap();
    }
    let boot = take_warnings();
    if std::env::var("UI_BOOT_WARNINGS").is_ok() {
        for w in &boot {
            eprintln!("BOOT {}", w.chars().take(300).collect::<String>());
        }
    }
    Some(vm)
}

fn frames(vm: &UserVm, n: usize) {
    for _ in 0..n {
        vm.frame(1.0 / 60.0).unwrap();
    }
}

/// Every hotkey action FAF binds (`/lua/keymap/keyactions.lua`, 460), run as
/// the key map runs it (`ConExecute`) with an engineer, a tank, a gunship
/// and the commander selected. Each warning is reported with the actions
/// that logged it; the known gaps listed in the test are let through.
#[test]
fn every_key_action_runs_clean() {
    let Some(vm) = hud() else { return };
    let actions: Vec<(String, String)> = vm
        .lua()
        .load(
            r#"
            local out = {}
            for name, a in import('/lua/keymap/keyactions.lua').keyActions do
                table.insert(out, name .. '\t' .. a.action)
            end
            table.sort(out)
            return out
            "#,
        )
        .eval::<Vec<String>>()
        .expect("key actions")
        .into_iter()
        .filter_map(|l| l.split_once('\t').map(|(n, a)| (n.to_string(), a.to_string())))
        .collect();
    assert!(actions.len() > 400, "FAF's key actions: {}", actions.len());
    let mut by_warning: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (name, action) in &actions {
        vm.set_selection(&[1, 2, 3, 5]).unwrap();
        let ran = vm.lua().load("ConExecute(...)").call::<()>(action.as_str());
        if let Err(e) = ran {
            by_warning.entry(format!("error: {e}")).or_default().push(name.clone());
        }
        frames(&vm, 3);
        // Close what it opened (dialogs, command modes) before the next.
        let _ = vm.lua().load("import('/lua/ui/game/commandmode.lua').EndCommandMode(true)").exec();
        for w in take_warnings() {
            // One line per distinct warning (the first line, without the
            // stack traceback).
            let first = w.lines().next().unwrap_or("").to_string();
            by_warning.entry(first).or_default().push(name.clone());
        }
    }
    // Known gaps, each a feature of its own (docs/guides/audits.md section 3);
    // anything else fails.
    const KNOWN_GAPS: [(&str, &str); 4] = [
        ("'GetSpecialFilePath'", "save / load (quick save)"),
        ("'InternalSaveGame'", "save / load (quick save)"),
        ("'RenderOverlayEconomy'", "renderer: the economy overlay"),
        ("'TeamColorMode'", "renderer: team colours"),
    ];
    by_warning.retain(|w, _| !KNOWN_GAPS.iter().any(|(name, _)| w.contains(name)));
    let report: String = by_warning
        .iter()
        .map(|(w, names)| format!("{} action(s): {w}\n    {}\n", names.len(), names.join(", ")))
        .collect();
    assert!(by_warning.is_empty(), "{} distinct warnings from key actions:\n{report}", by_warning.len());
}

/// FAF's radial dragger (area orders, the ping and range tools) measures
/// from where the drag started to where it is (`UnProject`).
#[test]
fn radial_dragger_runs_clean() {
    let Some(vm) = hud() else { return };
    let radius: f64 = vm
        .lua()
        .load(
            r#"
            local view = import('/lua/ui/game/worldview.lua').viewLeft
            local Dragger = import('/lua/ui/controls/draggers/radial.lua').RadialDragger
            local got
            local d = Dragger(view, function(origin, radius) got = radius end, 'LBUTTON', 0)
            d:OnMove(612, 384)
            d:OnRelease(612, 384)
            return got
            "#,
        )
        .eval()
        .expect("dragger");
    assert!(radius > 0.0, "a radius: {radius}");
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// The unit under the mouse (`GetRolloverInfo`): FAF's hover panel and
/// hotkeys that act on it (upgrade, copy queue) run clean.
#[test]
fn rollover_info_runs_clean() {
    let Some(vm) = hud() else { return };
    vm.set_rollover(Some(3)).unwrap();
    let bp: String = vm.lua().load("return GetRolloverInfo().blueprintId").eval().expect("rollover info");
    assert_eq!(bp, "uel0201");
    // The engine builds it, not FAF's selectedinfo.lua (LOUD has none).
    let built: bool = vm
        .lua()
        .load(
            r#"
            local real = import
            import = function(path, ...)
                if path == '/lua/keymap/selectedinfo.lua' then error('selectedinfo.lua imported') end
                return real(path, ...)
            end
            local ok, info = pcall(GetRolloverInfo)
            import = real
            return ok and info.entityId == '3' and info.armyIndex == info.userUnit:GetArmy() - 1
                and type(info.teamColor) == 'string' and info.massProduced == math.floor(info.massProduced)
            "#,
        )
        .eval()
        .expect("rollover info without selectedinfo.lua");
    assert!(built, "GetRolloverInfo builds its own table");
    frames(&vm, 30);
    for action in ["UI_Lua import('/lua/ui/game/hotkeys/upgrade-structure.lua').UpgradeStructure()", "UI_Lua import('/lua/keymap/misckeyactions.lua').SetWeaponPrioritiesToUnitType()"] {
        let _ = vm.lua().load("ConExecute(...)").call::<()>(action);
        frames(&vm, 3);
    }
    vm.set_rollover(None).unwrap();
    let none: bool = vm.lua().load("return GetRolloverInfo() == nil").eval().unwrap();
    assert!(none, "nothing under the mouse");
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// The interface's `GetDepositsAroundPoint` (hotbuild's context templates)
/// answers from the deposits the engine hands it.
#[test]
fn deposits_around_point_runs_clean() {
    let Some(vm) = hud() else { return };
    vm.set_deposits(Arc::new(vec![(1, 400.5, 600.5, 1.0), (2, 420.0, 600.0, 3.0)]));
    let (mass, all): (usize, usize) = vm.lua().load("return table.getn(GetDepositsAroundPoint(400, 600, 5, 1)), table.getn(GetDepositsAroundPoint(400, 600, 50, 0))").eval().unwrap();
    assert_eq!((mass, all), (1, 2));
    {
        let action = "UI_Lua import('/lua/ui/game/hotkeys/context-based-templates.lua').Cycle()";
        let _ = vm.lua().load("ConExecute(...)").call::<()>(action);
        frames(&vm, 3);
    }
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// Build templates: FAF makes one from selected structures, reads it back,
/// and arms it for placement (`SetActiveBuildTemplate`) with build mode.
#[test]
fn build_templates_run_clean() {
    let Some(vm) = hud() else { return };
    vm.set_selection(&[4]).unwrap();
    frames(&vm, 3);
    let n: i64 = vm
        .lua()
        .load(
            r#"
            GenerateBuildTemplateFromSelection()
            local t = GetActiveBuildTemplate()
            ClearBuildTemplates()
            return table.getn(t)
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(n, 3, "width, height and one structure");
    vm.lua()
        .load("import('/lua/ui/game/commandmode.lua').StartCommandMode('build', { name = 'ueb1101' }) SetActiveBuildTemplate({ 4, 2, { 'ueb1101', 1, 0, 0 }, { 'ueb1101', 2, 2, 0 } })")
        .exec()
        .unwrap();
    let items = vm.build_template().unwrap().expect("armed template");
    assert_eq!(items, vec![("ueb1101".to_string(), 0.0, 0.0), ("ueb1101".to_string(), 2.0, 0.0)]);
    vm.end_command_mode(true).unwrap();
    assert!(vm.build_template().unwrap().is_none(), "ending build mode clears it (FAF's commandmode)");
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// The waypoint under the mouse (`GetHighlightCommand`): hotbuild's context
/// templates take a queued build's blueprint from it.
#[test]
fn highlight_command_runs_clean() {
    let Some(vm) = hud() else { return };
    vm.set_highlight_command(Some(([410.0, 20.0, 600.0], 8, 42, Some("ueb1101".into())))).unwrap();
    let bp: String = vm.lua().load("return GetHighlightCommand().blueprintId").eval().unwrap();
    assert_eq!(bp, "ueb1101");
    let _ = vm.lua().load("ConExecute(...)").call::<()>("UI_Lua import('/lua/ui/game/hotkeys/context-based-templates.lua').Cycle()");
    frames(&vm, 3);
    vm.set_highlight_command(None).unwrap();
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// Session flags: a game is active, and replay / observer follow what the
/// engine says about this client.
#[test]
fn session_flags() {
    let Some(vm) = hud() else { return };
    let (active, replay, observer): (bool, bool, bool) = vm.lua().load("return SessionIsActive(), SessionIsReplay(), IsObserver()").eval().unwrap();
    assert!(active && !replay && !observer);
    vm.set_session_flags(true, true).unwrap();
    let (replay, observer): (bool, bool) = vm.lua().load("return SessionIsReplay(), IsObserver()").eval().unwrap();
    assert!(replay && observer);
}

/// Animated bitmaps (FAF's multi-frame buttons): `Play` steps through the
/// frames at the frame rate, calls `OnAnimationFrame` for each, and
/// `OnAnimationFinished` at the end of a non-looping pattern; `Loop` keeps
/// it going; ping-pong goes there and back.
#[test]
fn bitmap_animation_plays() {
    let Some(vm) = hud() else { return };
    let script = r#"
        local Bitmap = import('/lua/maui/bitmap.lua').Bitmap
        local tex = '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds'
        local b = Bitmap(GetFrame(0), { tex, tex, tex })
        local seen, done = {}, false
        b.OnAnimationFrame = function(self, f) table.insert(seen, f) end
        b.OnAnimationFinished = function(self) done = true end
        b:SetFrameRate(10)
        b:SetPingPongPattern()
        b:Play()
        rawset(_G, 'AnimTest', function() return table.concat(seen, ','), done end)
    "#;
    vm.lua().load(script).exec().unwrap();
    frames(&vm, 60); // 1 s at 10 frames a second: 0,1,2,1,0 then done
    let (seen, done): (String, bool) = vm.lua().load("return AnimTest()").eval().unwrap();
    assert_eq!(seen, "1,2,1,0");
    assert!(done, "finished");
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// World points to interface coordinates (`UIWorldView:Project`): the
/// camera's look-at point lands mid-screen, and FAF's reclaim labels,
/// off-screen indicators and notify overlay place their controls with it.
#[test]
fn world_view_project() {
    let Some(vm) = hud() else { return };
    let (x, y): (f32, f32) = vm.lua().load("local p = import('/lua/ui/game/worldview.lua').viewLeft:Project(Vector(400, 20, 600)) return p.x, p.y").eval().unwrap();
    assert!((x - 512.0).abs() < 1.0 && (y - 384.0).abs() < 1.0, "{x}, {y}");
    let n: usize = vm.lua().load("return table.getn(import('/lua/ui/game/worldview.lua').viewLeft:ProjectMultiple({ Vector(400, 20, 600), Vector(410, 20, 600) }))").eval().unwrap();
    assert_eq!(n, 2);
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// The order panel's unit toggles and lists read the selected units'
/// details, and setting auto mode goes to the sim as a toggle.
#[test]
fn unit_toggles_read_details_and_set() {
    use rts_core_engine::engine::snapshot::UnitDetail;
    let Some(vm) = hud() else { return };
    let detail = UnitDetail { id: 4, auto_mode: true, submerged: false, cargo: vec![2], assisters: vec![3], ..Default::default() };
    vm.update_units(&[UnitInfo { id: 4, blueprint: "ueb0101".into(), army: 1, health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, position: [410.0, 20.0, 600.0], work: 0.0, shield: 0.0, detail: Some(detail) }]).unwrap();
    let (auto, sub, cargo, assist): (bool, i64, String, String) = vm
        .lua()
        .load(
            r#"
            local u = { GetUnitById(4) }
            local function ids(list) local o = {} for _, x in list do table.insert(o, x:GetEntityId()) end return table.concat(o, ',') end
            return GetIsAutoMode(u), GetIsSubmerged(u), ids(GetAttachedUnitsList(u)), ids(GetAssistingUnitsList(u))
            "#,
        )
        .eval()
        .unwrap();
    assert!(auto);
    assert_eq!(sub, 1);
    assert_eq!((cargo.as_str(), assist.as_str()), ("2", "3"));
    vm.take_orders().unwrap();
    vm.lua().load("SetAutoMode({ GetUnitById(4) }, false)").exec().unwrap();
    let orders = vm.take_orders().unwrap();
    assert!(orders.iter().any(|o| o.toggle == Some(rts_core_engine::engine::sim::input::Toggle::AutoMode(false))), "{orders:?}");
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// moho's `Border` (the console dialog, radio buttons): corners at their
/// size, sides between them, `BorderWidth` / `BorderHeight` from the sides.
#[test]
fn border_draws_round_its_rectangle() {
    let Some(vm) = hud() else { return };
    let before = vm.draw_list().unwrap().len();
    let (bw, bh): (f32, f32) = vm
        .lua()
        .load(
            r#"
            local Border = import('/lua/maui/border.lua').Border
            local LayoutHelpers = import('/lua/maui/layouthelpers.lua')
            local tex = '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds'
            local b = Border(GetFrame(0))
            b:SetNewTextures(tex, tex, tex, tex, tex, tex)
            LayoutHelpers.AtLeftTopIn(b, GetFrame(0), 100, 100)
            b.Width:Set(300) b.Height:Set(200)
            rawset(_G, 'TestBorder', b)
            return b.BorderWidth(), b.BorderHeight()
            "#,
        )
        .eval()
        .unwrap();
    assert!(bw > 0.0 && bh > 0.0, "{bw} {bh}");
    let after = vm.draw_list().unwrap().len();
    assert!(after >= before + 8, "four corners and four sides: {before} -> {after}");
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// Game speed in moho's steps: the interface reads it, hears a change
/// (`NoteGameSpeedChanged`), and asks for one (`SetGameSpeed`).
#[test]
fn game_speed() {
    let Some(vm) = hud() else { return };
    vm.lua().load("local m = import('/lua/ui/uimain.lua') local f = m.NoteGameSpeedChanged m.NoteGameSpeedChanged = function(c, s) rawset(_G, 'Heard', s) return f(c, s) end").exec().unwrap();
    vm.set_game_speed(0, 0.0, 1).unwrap();
    // Asked for +3, running at +1 (a slow machine): both steps, as FAF's
    // clock shows them ("+3 / +1").
    vm.set_game_speed(3, 1.0, 1).unwrap();
    let (speed, rate, heard): (i64, f64, i64) = vm.lua().load("return GetGameSpeed(), GetSimRate(), Heard").eval().unwrap();
    assert_eq!((speed, heard), (3, 3));
    assert!((rate - 1.0).abs() < 1e-6);
    vm.lua().load("SetGameSpeed(-2)").exec().unwrap();
    assert_eq!(vm.take_speed_request().unwrap(), Some(-2));
    frames(&vm, 3);
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// Interface decals (`UserDecal`, FAF's range rings under the cursor):
/// made, moved, scaled and destroyed, handed to the renderer when changed.
#[test]
fn user_decals() {
    let Some(vm) = hud() else { return };
    vm.user_decals().unwrap();
    vm.lua()
        .load(
            r#"
            local UserDecal = import('/lua/user/userdecal.lua').UserDecal
            local d = UserDecal()
            d:SetTexture('/textures/ui/common/game/AreaTargetDecal/weapon_icon_small.dds')
            d:SetScale({ 10, 1, 10 })
            d:SetPosition(Vector(400, 20, 600))
            rawset(_G, 'TestDecal', d)
            "#,
        )
        .exec()
        .unwrap();
    let decals = vm.user_decals().unwrap().expect("changed");
    assert_eq!(decals.len(), 1);
    assert_eq!(decals[0].position, [395.0, 20.0, 595.0], "corner-placed about the centre");
    assert!(vm.user_decals().unwrap().is_none(), "unchanged");
    vm.lua().load("TestDecal:Destroy()").exec().unwrap();
    assert_eq!(vm.user_decals().unwrap().map(|d| d.len()), Some(0));
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// The cheat menu (Alt-F2, `PopupCreateUnitMenu`): FAF's spawn dialog
/// opens, and a build placed in its command mode with no units selected
/// reaches FAF's `OnCommandIssued`, which spawns through
/// `SimCallback CheatSpawnUnit` (moho issues the build with no units).
#[test]
fn cheat_menu_spawns_on_a_unitless_build() {
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget};
    use rts_core_engine::engine::sim::math::FxVec3;
    let Some(vm) = hud() else { return };
    frames(&vm, 3);
    vm.console("PopupCreateUnitMenu").unwrap();
    frames(&vm, 2);
    let open: bool = vm.lua().load("return import('/lua/ui/dialogs/createunit.lua') ~= nil").eval().unwrap();
    assert!(open);
    vm.lua()
        .load("import('/lua/ui/game/commandmode.lua').StartCommandMode('build', { name = 'uel0201', cheat = true, army = 1, count = 2, vet = 0, yaw = 0, rand = 0 })")
        .exec()
        .unwrap();
    vm.command_issued(CommandKind::BuildMobile, CommandTarget::Ground(FxVec3::from_f64(100.0, 20.0, 100.0)), false, Some("uel0201"), &[], &[]).unwrap();
    let orders = vm.take_orders().unwrap();
    let spawn = orders.iter().filter_map(|o| o.callback.as_ref()).map(|c| format!("{c:?}")).find(|c| c.contains("CheatSpawnUnit"));
    assert!(spawn.is_some(), "no CheatSpawnUnit callback: {orders:?}");
    // Alt-F2 again closes it.
    vm.console("PopupCreateUnitMenu").unwrap();
    frames(&vm, 2);
    let warnings = take_warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// Where a frame's interface draw list goes: controls in the tree, items
/// drawn, a quiet frame's draw list against a beat frame's (a sim tick's
/// economy and `OnBeat` handlers dirty the panels' layouts). Run with
/// `--nocapture --ignored --release` and read the numbers.
#[test]
#[ignore]
fn draw_list_cost() {
    use rts_core_engine::engine::snapshot::ArmyEconomy;
    use std::time::Instant;
    let Some(vm) = hud() else { return };
    frames(&vm, 10);
    let (total, hidden) = vm.control_count();
    let n = 200;
    let t = Instant::now();
    let mut items = 0;
    for _ in 0..n {
        items = vm.draw_list().unwrap().len();
    }
    let quiet = t.elapsed().as_secs_f64() * 1e3 / n as f64;
    let (mut beat_ms, mut draw_ms, mut frame_ms) = (0.0, 0.0, 0.0);
    let ticks = 20u64;
    for tick in 1..=ticks {
        let m = tick as f32 * 37.0;
        let eco = ArmyEconomy { army: 1, mass: [m, 4000.0, m / 3.0, 5.0, 5.0, 0.0], energy: [m * 10.0, 10000.0, m, 50.0, 50.0, 0.0], resource_sharing: true };
        let t = Instant::now();
        vm.beat(tick * 10, &[eco]).unwrap();
        beat_ms += t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        vm.frame(1.0 / 60.0).unwrap();
        frame_ms += t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        vm.draw_list().unwrap();
        draw_ms += t.elapsed().as_secs_f64() * 1e3;
        frames(&vm, 5);
    }
    eprintln!("controls {total} ({hidden} hidden), items drawn {items}");
    eprintln!("quiet draw list {quiet:.3} ms ({:.2} us per item)", quiet * 1e3 / items.max(1) as f64);
    eprintln!("beat frame: beat {:.3} ms, frame {:.3} ms, draw list {:.3} ms", beat_ms / ticks as f64, frame_ms / ticks as f64, draw_ms / ticks as f64);
}

/// The cheat menu opens at the top left, over the minimap: clicks on its
/// list are the dialog's, not a minimap jump of the camera.
#[test]
fn cheat_menu_over_the_minimap_takes_its_clicks() {
    let Some(vm) = hud() else { return };
    frames(&vm, 3);
    let (l, t, r, b): (f32, f32, f32, f32) = vm
        .lua()
        .load("local m = import('/lua/ui/game/minimap.lua').controls.miniMap return m.Left(), m.Top(), m.Right(), m.Bottom()")
        .eval()
        .unwrap();
    let (x, y) = ((l + r) / 2.0, (t + b) / 2.0);
    assert!(vm.hit_minimap(x, y).unwrap(), "minimap at ({l}, {t})-({r}, {b}) takes clicks");
    vm.console("PopupCreateUnitMenu").unwrap();
    frames(&vm, 2);
    assert!(!vm.hit_minimap(x, y).unwrap(), "the cheat menu covers ({x}, {y})");
    vm.console("PopupCreateUnitMenu").unwrap();
    frames(&vm, 2);
    assert!(vm.hit_minimap(x, y).unwrap(), "closed: the minimap's again");
}
