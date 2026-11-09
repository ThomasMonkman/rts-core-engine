//! Render-state tests: a real engine feeds snapshots to a real `Scene` on a
//! windowless GPU device, and the tests assert on what the renderer works
//! out, not on pixels: turret barrels face their targets, effects sit on the
//! posed bones, every model and texture loads (no boxes left), map textures
//! load. GPU-independent results, so they hold on any platform; image
//! comparisons are a later layer.
//!
//! Skips without a GPU adapter; fails without the game data (`../fa`, the
//! FA install) unless `RTS_SKIP_DATA_TESTS=1`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::Vec3;

use crate::engine::snapshot::Snapshot;
use rts_core_engine::debug_ui::DebugUiState;
use rts_core_engine::engine;

use super::scene::Scene;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: None, force_fallback_adapter: false })
            .await
            .ok()?;
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("render tests"),
                required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
                required_limits: wgpu::Limits { max_sampled_textures_per_shader_stage: adapter.limits().max_sampled_textures_per_shader_stage.min(64), ..wgpu::Limits::default() },
                ..Default::default()
            })
            .await
            .ok()
    })
}

struct Harness {
    engine: engine::Engine,
    scene: Scene,
    /// The main thread's side (snapshots, cameras, picking) and what it
    /// hands the scene each frame.
    world: super::world::World,
    input: super::world::FrameInput,
    settings: super::world::RenderSettings,
    rx: std::sync::mpsc::Receiver<Snapshot>,
    /// The renderer's requests to the sim (inspector, placement queries).
    request: crate::engine::snapshot::SharedDebugRequest,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Harness {
    /// An engine running `script` on `map` (default SCMP_009) and a scene fed
    /// by it; `None` (test skipped) without a GPU, or without the data with
    /// `RTS_SKIP_DATA_TESTS=1` (otherwise missing data fails).
    fn new(script: Option<&str>, ai: bool, map: Option<&str>) -> Option<Self> {
        Self::new_in(script, ai, map, std::path::Path::new("../faf-vault"))
    }

    /// [`Harness::new`] mounting maps from `vault` (e.g. `dev/vault`, the
    /// stress maps).
    fn new_in(script: Option<&str>, ai: bool, map: Option<&str>, vault: &std::path::Path) -> Option<Self> {
        // Fails without the game data (`RTS_SKIP_DATA_TESTS=1` skips).
        if !rts_core_engine::test_data::require_game("render tests") {
            return None;
        }
        let Some((device, queue)) = device() else {
            eprintln!("skipping render tests: no GPU adapter");
            return None;
        };
        let mut engine = engine::Engine::new_with_vault(crate::INIT_LUA_PATH, crate::FA_PATH, Arc::new(Mutex::new(DebugUiState::new())), vault.exists().then_some(vault)).expect("engine");
        let (sink, rx, request) = crate::engine::snapshot::channel();
        engine.set_snapshot_sink(sink);
        let mut session = engine::session::SessionConfig { script: script.map(String::from), ticks: Some(0), create_initial_units: ai, ai, ai_armies: if map.is_some_and(|m| m.contains("stress")) { 0 } else { 2 }, ..Default::default() };
        if let Some(m) = map {
            session.scenario = m.to_owned();
        }
        engine.start(&session).expect("session");
        let mut scene = Scene::new(&device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb, 640, 480);
        scene.fixed_alpha = Some(1.0);
        scene.record_all_bones = true;
        scene.stage.particles.record_anchors = true;
        let world = super::world::World::new(640, 480);
        Some(Self { engine, scene, world, input: Default::default(), settings: Default::default(), rx, request, device, queue })
    }

    fn frame(&mut self) {
        for delta in self.world.receive(&self.rx, 4.0 / 3.0) {
            self.scene.apply(&self.device, delta);
        }
        self.sync();
        self.scene.prepare(&self.device, &self.queue);
        self.world.frame = self.scene.results();
        let bounds = self.scene.take_new_mesh_bounds();
        self.world.add_mesh_bounds(&bounds);
    }

    /// Hands the scene the world's cameras, views and tables with the
    /// test's input and settings (as the frame does).
    fn sync(&mut self) {
        self.world.fill_input(0, &mut self.input);
        self.input.settings = self.settings;
        self.scene.set_input(&self.device, self.input.clone());
    }

    /// This frame's range overlay rings for the main camera, with the
    /// test's input handed over first.
    fn range_groups(&mut self) -> Vec<super::ranges::Group> {
        self.sync();
        let camera = *self.world.camera().expect("camera");
        self.scene.range_groups(&camera)
    }

    /// The window is `w` x `h` (scene and world).
    fn resize(&mut self, w: u32, h: u32) {
        self.scene.resize(&self.device, w, h);
        self.world.resize(w, h);
    }

    /// `n` sim ticks, a frame after each.
    fn ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.engine.tick().expect("tick");
            self.frame();
        }
    }

    /// Frames until nothing is loading (models, textures, effects, map).
    fn settle(&mut self) {
        let start = Instant::now();
        loop {
            self.frame();
            if !self.scene.loading() {
                // One more: the last uploads land on the next prepare.
                self.frame();
                if !self.scene.loading() {
                    return;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(60), "assets still loading after 60 s");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Harness {
    /// The next session in the same process, as the engine thread runs them
    /// one after another (`main.rs`): a fresh engine on `map` takes over the
    /// renderer's snapshot channel. The scene isn't swapped (the presenter
    /// does that); tests of it read the world only (`world_frames`).
    fn next_session(&mut self, map: &str) {
        let (sink, _) = self.engine.release_renderer();
        let mut engine = engine::Engine::new_with_vault(crate::INIT_LUA_PATH, crate::FA_PATH, Arc::new(Mutex::new(DebugUiState::new())), Some(std::path::Path::new("../faf-vault"))).expect("engine");
        engine.set_snapshot_sink(sink.expect("the renderer's sink"));
        let session = engine::session::SessionConfig { ticks: Some(0), create_initial_units: false, ai: false, ai_armies: 2, scenario: map.to_owned(), ..Default::default() };
        engine.start(&session).expect("session");
        self.engine = engine;
    }

    /// `n` sim ticks, the world (not the scene) taking each snapshot.
    fn world_frames(&mut self, n: u32) {
        for _ in 0..n {
            self.engine.tick().expect("tick");
            drop(self.world.receive(&self.rx, 4.0 / 3.0));
        }
    }
}

/// Games one after another in one process (lobby, score, front end, the
/// next lobby): the minimap shows each game's own map, not the first's.
#[test]
fn minimap_shows_each_sessions_map() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let mut minimap = super::frame::MinimapPreview::default();
    h.world_frames(2);
    let first = minimap.take_new(&h.world).expect("SCMP_009's preview");
    assert!(minimap.take_new(&h.world).is_none(), "looked for once a session");
    // `close_session`, then the next game on another map.
    minimap.end_session();
    h.next_session("/maps/scmp_019/scmp_019_scenario.lua");
    h.world_frames(2);
    assert_eq!(h.world.session, 2);
    let second = minimap.take_new(&h.world).expect("the second session's preview");
    assert!(second.levels[0] != first.levels[0], "the minimap still shows the first session's map");
}

fn fatboy_id(h: &Harness) -> u32 {
    let world = h.engine.sim_lua().app_data_ref::<crate::engine::sim::entity::EntityWorld>().expect("world");
    world.iter().find(|e| e.cold.blueprint_id == "uel0401" && !e.destroyed).map(|e| e.id).expect("the scenario's Fatboy")
}

/// Every Fatboy turret that is on target has its drawn muzzle (posed bone,
/// as the renderer draws it) pointing at its target, within a few degrees
/// horizontally. Targets sit on the diagonals, off the side turrets' rest
/// facing, so a turret that doesn't turn fails.
#[test]
fn fatboy_turrets_face_their_targets() {
    let Some(mut h) = Harness::new(Some("tests/scenarios/fatboy.lua"), false, None) else { return };
    h.ticks(60);
    h.settle();
    h.ticks(5);
    let fatboy = fatboy_id(&h);
    let lua = h.engine.sim_lua();
    let world = lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>().expect("world");
    let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>().expect("weapons");
    let manips = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>().expect("manipulators");
    let mut checked = 0;
    let mut failures = Vec::new();
    for m in manips.of(fatboy) {
        let crate::engine::sim::manipulator::ManipulatorKind::Aim(a) = &m.kind else { continue };
        let Some(w) = weapons.get(fatboy, a.weapon_index) else { continue };
        let (Some(crate::engine::sim::weapon::WeaponTarget::Entity(t)), true, Some(muzzle)) = (w.target, w.on_target, a.muzzle_bone) else { continue };
        let Some(target) = world.get(t) else { continue };
        let Some(m) = h.scene.bone_world(fatboy, muzzle) else {
            failures.push(format!("{}: muzzle bone {muzzle} not drawn", a.label));
            continue;
        };
        let pos = m.transform_point3(Vec3::ZERO);
        let fwd = m.transform_vector3(Vec3::Z);
        let tp = target.position();
        let to = Vec3::new(tp.x.to_num(), tp.y.to_num(), tp.z.to_num()) - pos;
        let angle = |v: Vec3| v.x.atan2(v.z);
        let mut diff = (angle(fwd) - angle(to)).to_degrees().rem_euclid(360.0);
        if diff > 180.0 {
            diff = 360.0 - diff;
        }
        checked += 1;
        if diff > 10.0 {
            failures.push(format!("{}: drawn muzzle {diff:.1} degrees off its target", a.label));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(checked >= 4, "only {checked} turrets were on target to check");
}

/// Effects attached to the Fatboy's bones are placed on those bones as drawn
/// (posed), not where the sim's rest pose puts them.
#[test]
fn attached_effects_sit_on_posed_bones() {
    let Some(mut h) = Harness::new(Some("tests/scenarios/fatboy.lua"), false, None) else { return };
    h.ticks(60);
    h.settle();
    h.ticks(3);
    let fatboy = fatboy_id(&h);
    let effects = h.world.latest.as_ref().map_or_else(Vec::new, |s| s.effects.clone());
    let mut checked = 0;
    for e in effects.iter().filter(|e| e.attached && e.entity == fatboy && e.bone != u32::MAX) {
        let Some(&(anchor, _)) = h.scene.stage.particles.anchors.get(&e.id) else { continue };
        let bone = h.scene.bone_world(fatboy, e.bone).expect("effect bone drawn");
        let (_, r, t) = bone.to_scale_rotation_translation();
        let expected = t + r * Vec3::from_array(e.offset);
        assert!(anchor.distance(expected) < 1e-3, "effect {} on bone {}: anchor {anchor} vs posed bone {expected}", e.id, e.bone);
        checked += 1;
    }
    assert!(checked > 0, "no attached Fatboy effects to check");
}

/// A normal AI game on SCMP_009: after loading, nothing with a mesh is
/// still a box and no model, map or effect asset failed to load.
#[test]
fn stock_game_assets_all_load() {
    let Some(mut h) = Harness::new(None, true, None) else { return };
    h.ticks(400);
    h.settle();
    h.ticks(1);
    let failures = h.scene.asset_failures();
    assert!(failures.is_empty(), "{} assets failed to load:\n{}", failures.len(), failures.join("\n"));
    assert!(h.scene.stage.boxed.is_empty(), "{} entities still drawn as boxes: {:?}", h.scene.stage.boxed.len(), &h.scene.stage.boxed[..h.scene.stage.boxed.len().min(10)]);
    assert!(h.scene.map.is_ready(), "map textures not uploaded");
    // And every level of detail those meshes have (LOD1+ meshes and their
    // textures, or LOD0's where a level has none).
    h.scene.request_all_lods();
    h.settle();
    let failures = h.scene.asset_failures();
    assert!(failures.is_empty(), "{} LOD assets failed to load:
{}", failures.len(), failures.join("
"));
}

/// FAF vault maps (TTerrainXP, Terrain200) load all their map textures.
/// Fails without `../faf-vault` (`RTS_SKIP_DATA_TESTS=1` skips).
#[test]
fn vault_map_textures_load() {
    if !rts_core_engine::test_data::require("vault map textures", &["../faf-vault/maps"]) {
        return;
    }
    for map in ["/maps/adaptive_monument_valley.v0005/adaptive_monument_valley_scenario.lua", "/maps/texture_test_map.v0003/texture_test_map_scenario.lua"] {
        let Some(mut h) = Harness::new(None, false, Some(map)) else { return };
        h.ticks(2);
        h.settle();
        assert!(h.scene.map.is_ready(), "{map}: map textures not uploaded");
        let failures: Vec<String> = h.scene.map.failures().to_vec();
        assert!(failures.is_empty(), "{map}:\n{}", failures.join("\n"));
    }
}

/// GPU cost of the sun shadows: the same frame drawn offscreen with
/// shadows on and off, each frame waited for. Measurement, not a check:
/// `cargo test --release --bin rts-core-engine shadow_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn shadow_cost() {
    let Some(mut h) = Harness::new(None, true, None) else { return };
    h.ticks(600);
    h.settle();
    if let Some(c) = h.world.camera_mut() {
        c.target = Vec3::new(668.0, 18.0, 350.0);
        c.zoom = 60.0;
    }
    let target = h.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d { width: 1920, height: 1080, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    h.resize(1920, 1080);
    let time = |h: &mut Harness, on: bool| {
        h.settings.shadows = on;
        let mut best = f64::MAX;
        for _ in 0..40 {
            h.frame();
            let start = Instant::now();
            h.queue.submit(h.scene.draw(&h.device, &view, None, None).into_iter().map(|e| e.finish()));
            let _ = h.device.poll(wgpu::PollType::wait_indefinitely());
            best = best.min(start.elapsed().as_secs_f64() * 1000.0);
        }
        best
    };
    let off = time(&mut h, false);
    let on = time(&mut h, true);
    let prep = |h: &mut Harness, on: bool| {
        h.settings.shadows = on;
        (0..40)
            .map(|_| {
                let start = Instant::now();
                h.frame();
                start.elapsed().as_secs_f64() * 1000.0
            })
            .fold(f64::MAX, f64::min)
    };
    let (prep_off, prep_on) = (prep(&mut h, false), prep(&mut h, true));
    eprintln!("prepare (CPU): shadows off {prep_off:.2} ms, on {prep_on:.2} ms");
    eprintln!("frame (draw + GPU) at 1920x1080, {} entities: shadows off {off:.2} ms, on {on:.2} ms (+{:.2} ms); casters per cascade {:?}", h.scene.frame_entities.len(), on - off, h.scene.shadow_caster_counts());
}

/// Box select keeps the highest `SelectionPriority` in the box: engineers
/// (and the ACU, 3), a transport (2) and structures (5) are left out of a
/// box with tanks (1); with shift it takes everything, and a box of only
/// engineers selects them.
#[test]
fn box_select_leaves_out_engineers() {
    use super::player::{Event, Mods, PlayerControl};
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            for i, bp in {'uel0201', 'uel0201', 'uel0105', 'uel0001'} do
                local x, z = 386 + i * 4, 590
                table.insert(ids, tonumber(CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0):GetEntityId()))
            end
            table.insert(ids, tonumber(CreateUnitHPR('ueb1101', 'ARMY_1', 396, GetTerrainHeight(396, 600), 600, 0, 0, 0):GetEntityId()))
            table.insert(ids, tonumber(CreateUnitHPR('uea0107', 'ARMY_1', 404, GetTerrainHeight(404, 600) + 8, 600, 0, 0, 0):GetEntityId()))
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let (tanks, mut all) = (vec![ids[0].min(ids[1]), ids[0].max(ids[1])], ids.clone());
    all.sort_unstable();
    h.ticks(3);
    {
        let y = h.scene.height_at(396.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(396.0, y, 590.0);
        cam.zoom = 50.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let mut p = PlayerControl::new(None, Vec::new(), None);
    let (none, shift) = (Mods::default(), Mods { shift: true, ..Mods::default() });
    let drag = |p: &mut PlayerControl, h: &mut Harness, a: (f64, f64), b: (f64, f64), mods: Mods| {
        p.handle(&mut h.world, Event::LeftDown(a.0, a.1), mods, None);
        p.handle(&mut h.world, Event::LeftUp(b.0, b.1), mods, None);
    };
    drag(&mut p, &mut h, (1.0, 1.0), (639.0, 479.0), none);
    assert_eq!(p.selection, tanks, "tanks, engineer, ACU, a transport and a structure in the box: just the tanks");
    p.selection.clear();
    drag(&mut p, &mut h, (1.0, 1.0), (639.0, 479.0), shift);
    assert_eq!(p.selection, all, "with shift: everything, the structure too");
    // A box around just the engineer and the ACU.
    let at = |h: &Harness, x: f32| -> (f64, f64) {
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(x, h.scene.height_at(x, 590.0), 590.0), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    let (a, b) = (at(&h, 396.0), at(&h, 404.0));
    drag(&mut p, &mut h, (a.0, a.1 - 60.0), (b.0, b.1 + 60.0), none);
    let mut builders = vec![ids[2], ids[3]];
    builders.sort_unstable();
    assert_eq!(p.selection, builders, "only engineers in the box: them");
}

/// Drag selection meets the unit's mesh box, as moho's does (MohoEngine.dll
/// 0x104289d4): a box over only the edge of a land factory, its centre well
/// outside, takes it; the factory's mesh is wider than its collision box.
#[test]
fn box_select_takes_a_unit_the_box_only_partly_covers() {
    use super::player::{Event, Mods, PlayerControl};
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let id: u32 = h.engine.sim_lua().load("return tonumber(CreateUnitHPR('ueb0101', 'ARMY_1', 396, GetTerrainHeight(396, 590), 590, 0, 0, 0):GetEntityId())").eval().expect("spawn");
    h.ticks(3);
    {
        let y = h.scene.height_at(396.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(396.0, y, 590.0);
        cam.zoom = 50.0;
    }
    h.settle();
    h.frame();
    let e = *h.world.entities().iter().find(|e| e.id == id).expect("the factory");
    let bounds = h.world.mesh_bounds.get(e.mesh as usize).copied().flatten().expect("its mesh's bounds came back");
    let mesh_half = (bounds[3] - bounds[0]) * 0.5 * e.scale[0];
    assert!(mesh_half > 1.0, "a factory-sized mesh: {mesh_half}");
    let screen = [640.0, 480.0];
    let at = |h: &Harness, x: f32| -> (f64, f64) {
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(x, h.scene.height_at(x, 590.0), 590.0), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    // From just inside its right edge (0.6 of the mesh's half width out) to
    // well right of it: the centre is outside.
    let (a, b) = (at(&h, 396.0 + mesh_half * 0.6), at(&h, 396.0 + mesh_half * 3.0));
    let mut p = PlayerControl::new(None, Vec::new(), None);
    p.handle(&mut h.world, Event::LeftDown(a.0, a.1 - 30.0), Mods::default(), None);
    p.handle(&mut h.world, Event::LeftUp(b.0, b.1 + 30.0), Mods::default(), None);
    assert_eq!(p.selection, vec![id], "the box over its edge takes it");
    // A box wholly right of it: nothing.
    let (c, d) = (at(&h, 396.0 + mesh_half * 1.6), at(&h, 396.0 + mesh_half * 3.0));
    p.selection.clear();
    p.handle(&mut h.world, Event::LeftDown(c.0, c.1 - 30.0), Mods::default(), None);
    p.handle(&mut h.world, Event::LeftUp(d.0, d.1 + 30.0), Mods::default(), None);
    assert!(p.selection.is_empty(), "a box beside it: {:?}", p.selection);
}

/// A click picks by moho's click box (MohoEngine.dll 0x104343d1): a UEF
/// naval factory's mesh reaches ~22 below the water, ~2.5 above and ~7 each
/// side; its blueprint keeps the top 15% (`SelectionMeshUseTopAmount`) and
/// narrows it to 0.3 across (`SelectionMeshScaleX`). So its top picks it a
/// little way out, not 0.8 of its mesh's half width out, and not its hull
/// deep under the water.
#[test]
fn click_picks_a_naval_factory_by_its_top_only() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let id: u32 = h.engine.sim_lua().load("return tonumber(CreateUnitHPR('ueb0103', 'ARMY_1', 300, GetSurfaceHeight(300, 600), 600, 0, 0, 0):GetEntityId())").eval().expect("spawn");
    h.ticks(3);
    {
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(300.0, 17.5, 600.0);
        cam.zoom = 60.0;
    }
    h.settle();
    h.frame();
    let e = *h.world.entities().iter().find(|e| e.id == id).expect("the factory");
    let b = h.world.mesh_bounds.get(e.mesh as usize).copied().flatten().expect("its mesh's bounds came back");
    let s = e.scale;
    let p = Vec3::from_array(e.position);
    let screen = [640.0, 480.0];
    let px = |h: &Harness, q: Vec3| -> (f32, f32) {
        let [x, y] = h.world.camera().unwrap().project(q, screen).expect("on screen");
        (x, y)
    };
    // The top of the mesh, over its middle.
    let (x, y) = px(&h, p + Vec3::new(0.0, b[4] * s[1], 0.0));
    assert_eq!(h.world.pick(x, y), Some(id), "its top picks it");
    // At the top, a quarter of its mesh's half width out: inside the 0.3
    // scale; 0.8 out: outside it.
    let (x, y) = px(&h, p + Vec3::new(b[3] * s[0] * 0.25, b[4] * s[1], 0.0));
    assert_eq!(h.world.pick(x, y), Some(id), "a little way out picks it");
    let (x, y) = px(&h, p + Vec3::new(b[3] * s[0] * 0.8, b[4] * s[1], 0.0));
    assert_ne!(h.world.pick(x, y), Some(id), "past SelectionMeshScaleX doesn't");
    // Its hull halfway down, at the end nearer the camera (low on the
    // screen): below its click box.
    let near_end = if px(&h, p + Vec3::new(0.0, 0.0, b[2] * s[2])).1 > px(&h, p + Vec3::new(0.0, 0.0, b[5] * s[2])).1 { b[2] } else { b[5] };
    let hull = p + Vec3::new(0.0, b[1] * s[1] * 0.5, near_end * s[2] * 0.8);
    let (x, y) = px(&h, hull);
    assert_ne!(h.world.pick(x, y), Some(id), "its hull under the water doesn't");
}

/// Selection and right-click orders on a real scene: a box selects the six
/// tanks (the observer gets the army with most units in the box), a
/// double-click selects every tank of that type on screen, a right drag sends
/// a formation move facing the drag, a right click on the enemy sends an attack.
#[test]
fn box_select_and_right_click_orders() {
    use super::player::{Event, Mods, PlayerControl};
    use crate::engine::sim::command::{CommandKind, CommandTarget};
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            for i = 0, 5 do
                local x, z = 390 + math.mod(i, 3) * 4, 580 + math.floor(i / 3) * 4
                table.insert(ids, tonumber(CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0):GetEntityId()))
            end
            table.insert(ids, tonumber(CreateUnitHPR('uel0201', 'ARMY_2', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0):GetEntityId()))
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let mut ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let enemy = ids.pop().unwrap();
    ids.sort_unstable();
    h.ticks(3);
    {
        let y = h.scene.height_at(396.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(396.0, y, 590.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let (tx, rx) = std::sync::mpsc::channel();
    let mut p = PlayerControl::new(None, Vec::new(), Some(tx));
    let none = Mods::default();
    p.handle(&mut h.world, Event::LeftDown(1.0, 1.0), none, None);
    p.handle(&mut h.world, Event::LeftUp(639.0, 479.0), none, None);
    assert_eq!(p.selection, ids, "box over everything: army 1's six tanks");

    let at = |h: &Harness, x: f32, z: f32| -> (f64, f64) {
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(x, h.scene.height_at(x, z), z), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    // Click one tank, then double-click it: all six again.
    let tank = h.scene.frame_entities.iter().find(|e| e.id == ids[0]).unwrap().position;
    let (tx_, ty_) = at(&h, tank[0], tank[2]);
    p.handle(&mut h.world, Event::LeftDown(tx_, ty_), none, None);
    p.handle(&mut h.world, Event::LeftUp(tx_, ty_), none, None);
    assert_eq!(p.selection.len(), 1);
    p.handle(&mut h.world, Event::LeftDown(tx_, ty_), none, None);
    p.handle(&mut h.world, Event::LeftUp(tx_, ty_), none, None);
    assert_eq!(p.selection, ids, "double-click: every uel0201 of army 1 on screen");

    // Right drag from open ground eastwards: a plain move (the sim ignores
    // the formation fields for hand-issued orders; kept for replay format
    // stability, still sent with the drag's facing).
    let (ax, ay) = at(&h, 386.0, 596.0);
    let (bx, by) = at(&h, 392.0, 596.0);
    p.handle(&mut h.world, Event::RightDown(ax, ay), none, None);
    p.handle(&mut h.world, Event::RightUp(bx, by), none, None);
    let c = rx.try_recv().expect("a move order");
    assert_eq!((c.army, c.kind, c.units.clone(), c.queue), (1, CommandKind::Move, ids.clone(), false));
    assert_eq!(c.formation.as_deref(), Some("AttackFormation"));
    let facing = c.facing.expect("drag sets the facing");
    assert!((facing - 90).abs() <= 3, "facing {facing}");
    let CommandTarget::Ground(g) = c.target else { panic!("ground target") };
    let (gx, _, gz) = g.to_f64();
    assert!((gx - 386.0).abs() < 1.5 && (gz - 596.0).abs() < 1.5, "target where the drag started: ({gx:.1}, {gz:.1})");

    // Shift + right click on the enemy tank: a queued attack.
    let e = h.scene.frame_entities.iter().find(|e| e.id == enemy).unwrap().position;
    let (ex, ey) = at(&h, e[0], e[2]);
    let shift = Mods { shift: true, ..Mods::default() };
    p.handle(&mut h.world, Event::RightDown(ex, ey), shift, None);
    p.handle(&mut h.world, Event::RightUp(ex, ey), shift, None);
    let c = rx.try_recv().expect("an attack order");
    assert_eq!((c.kind, c.target, c.queue), (CommandKind::Attack, CommandTarget::Entity(enemy), true));
    assert!(rx.try_recv().is_err(), "one order per click");
}

/// Repro for issue #4: hovering a friendly commander must offer guard,
/// never attack. Spawns ACUs for ARMY_1 and allied ARMY_2 plus a tank, and
/// checks the context order over both commanders.
#[test]
fn unit_in_factory_is_not_selectable() {
    // A unit still being built in a factory can't be clicked, boxed or
    // otherwise selected; clicks on it go to the factory.
    use super::player::{Event, Mods, PlayerControl};
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let fac: u32 = h
        .engine
        .sim_lua()
        .load(
            r#"
            local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
            IssueBuildFactory({fac}, 'uel0201', 1)
            return tonumber(fac:GetEntityId())
            "#,
        )
        .eval()
        .expect("spawn");
    let mut built = None;
    for _ in 0..100 {
        h.ticks(1);
        built = h.scene.frame_entities.iter().find(|e| e.in_builder).map(|e| e.id);
        if built.is_some() {
            break;
        }
    }
    let built = built.expect("the factory starts a tank");
    {
        let y = h.scene.height_at(400.0, 600.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(400.0, y, 600.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let mut p = PlayerControl::new(None, Vec::new(), None);
    let none = Mods::default();
    p.handle(&mut h.world, Event::LeftDown(1.0, 1.0), none, None);
    p.handle(&mut h.world, Event::LeftUp(639.0, 479.0), none, None);
    assert_eq!(p.selection, vec![fac], "box over both: just the factory");

    let tank = h.scene.frame_entities.iter().find(|e| e.id == built).unwrap().position;
    let [x, y] = h.world.camera().unwrap().project(Vec3::from_array(tank), screen).expect("on screen");
    p.selection.clear();
    p.handle(&mut h.world, Event::LeftDown(x as f64, y as f64), none, None);
    p.handle(&mut h.world, Event::LeftUp(x as f64, y as f64), none, None);
    assert_eq!(p.selection, vec![fac], "a click on the tank picks the factory");

    // However it got into the selection (the interface's SelectUnits), it
    // drops out.
    p.selection = vec![built, fac];
    p.prune(&h.world);
    assert_eq!(p.selection, vec![fac]);
}

#[test]
fn friendly_commander_offers_guard_not_attack() {
    use super::player::PlayerControl;
    use crate::engine::sim::command::CommandKind;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            SetAlliance('ARMY_1', 'ARMY_2', 'Ally')
            local ids = {}
            local function spawn(bp, army, x, z)
                local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
                -- (Scripts' aircraft start at their flying height: on the ground.)
                Warp(u, { x, GetTerrainHeight(x, z), z })
                table.insert(ids, tonumber(u:GetEntityId()))
            end
            spawn('uel0001', 'ARMY_1', 396, 590)
            spawn('uel0001', 'ARMY_2', 404, 590)
            spawn('uel0201', 'ARMY_1', 390, 580)
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let [acu1, acu2, tank] = ids[..] else { panic!("three units") };
    h.ticks(3);
    {
        let y = h.scene.height_at(396.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(396.0, y, 590.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let (tx, _rx) = std::sync::mpsc::channel();
    let p = PlayerControl::new(Some(1), vec![tank], Some(tx));
    let screen = [640.0, 480.0];
    let at = |h: &Harness, x: f32, z: f32| -> (f64, f64) {
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(x, h.scene.height_at(x, z), z), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    for (id, what) in [(acu1, "own commander"), (acu2, "allied commander")] {
        let e = h.scene.frame_entities.iter().find(|e| e.id == id).unwrap().position;
        let (ex, ey) = at(&h, e[0], e[2]);
        assert_eq!(h.world.pick(ex as f32, ey as f32), Some(id), "cursor is over the {what}");
        assert_eq!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::Guard), "hovering {what} offers guard");
    }
}

/// Repro: right-clicking a transport with a loadable unit
/// selected walks it aboard, and right-clicking a unit with a transport
/// selected flies the transport out to pick it up (moho's
/// `GetRightMouseButtonOrder` answers `RULEUCC_CallTransport` for the first;
/// the reverse is the transport's own order onto the unit).
#[test]
fn transport_hover_offers_load_both_ways() {
    use super::player::PlayerControl;
    use crate::engine::sim::command::CommandKind;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            local function spawn(bp, army, x, z)
                local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
                -- (Scripts' aircraft start at their flying height: on the ground.)
                Warp(u, { x, GetTerrainHeight(x, z), z })
                table.insert(ids, tonumber(u:GetEntityId()))
            end
            spawn('uea0107', 'ARMY_1', 396, 590)
            spawn('uel0201', 'ARMY_1', 404, 590)
            spawn('uea0101', 'ARMY_1', 396, 598)
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let [transport, tank, scout] = ids[..] else { panic!("three units") };
    h.ticks(3);
    {
        let y = h.scene.height_at(400.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(400.0, y, 590.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let at = |h: &Harness, x: f32, z: f32| -> (f64, f64) {
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(x, h.scene.height_at(x, z), z), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    let (tx, _rx) = std::sync::mpsc::channel();
    // A tank ordered onto a transport walks aboard.
    let p = PlayerControl::new(Some(1), vec![tank], Some(tx));
    let e = h.scene.frame_entities.iter().find(|e| e.id == transport).unwrap().position;
    let (ex, ey) = at(&h, e[0], e[2]);
    assert_eq!(h.world.pick(ex as f32, ey as f32), Some(transport), "cursor is over the transport");
    assert_eq!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::TransportLoadUnits), "tank onto transport offers load");
    // A transport ordered onto a tank flies out to pick it up.
    let (tx, _rx) = std::sync::mpsc::channel();
    let p = PlayerControl::new(Some(1), vec![transport], Some(tx));
    let e = h.scene.frame_entities.iter().find(|e| e.id == tank).unwrap().position;
    let (ex, ey) = at(&h, e[0], e[2]);
    assert_eq!(h.world.pick(ex as f32, ey as f32), Some(tank), "cursor is over the tank");
    assert_eq!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::TransportReverseLoadUnits), "transport onto tank offers reverse load");
    // Not an aircraft either way (the shared carry rule, `sim::carry`: a
    // transport that isn't air staging takes no air unit).
    let (tx, _rx) = std::sync::mpsc::channel();
    let p = PlayerControl::new(Some(1), vec![scout], Some(tx));
    let e = h.scene.frame_entities.iter().find(|e| e.id == transport).unwrap().position;
    let (ex, ey) = at(&h, e[0], e[2]);
    assert_eq!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::Guard), "an aircraft onto a transport guards it");
    let (tx, _rx) = std::sync::mpsc::channel();
    let p = PlayerControl::new(Some(1), vec![transport], Some(tx));
    let e = h.scene.frame_entities.iter().find(|e| e.id == scout).unwrap().position;
    let (ex, ey) = at(&h, e[0], e[2]);
    assert_eq!(h.world.pick(ex as f32, ey as f32), Some(scout), "cursor is over the scout");
    assert_ne!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::TransportReverseLoadUnits), "no pick-up of an aircraft");
}

/// A right click on an own ferry beacon (moho's 0x103e7a40: CallTransport
/// when a selected non-air unit can wait there, 0x103e61d0, or the whole
/// selection is factories, 0x103e6970): a tank waits for the ferry, a
/// factory sends what it builds there; a transport doesn't board it.
#[test]
fn ferry_beacon_offers_load() {
    use super::player::PlayerControl;
    use crate::engine::sim::command::CommandKind;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            local function spawn(bp, x, z)
                local u = CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
                table.insert(ids, tonumber(u:GetEntityId()))
                return u
            end
            local ferry = spawn('uea0107', 380, 590)
            spawn('uel0201', 392, 580)
            spawn('ueb0101', 410.5, 580.5)
            IssueFerry({ ferry }, { 400, GetTerrainHeight(400, 596), 596 })
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let [ferry, tank, factory] = ids[..] else { panic!("three units") };
    h.ticks(3);
    let beacon: u32 = h.engine.sim_lua().load(format!("return tonumber(GetEntityById({ferry}):GetTransportFerryBeacon():GetEntityId())")).eval().expect("beacon");
    {
        let y = h.scene.height_at(398.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(398.0, y, 590.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let e = h.scene.frame_entities.iter().find(|e| e.id == beacon).expect("the beacon is drawn").position;
    let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(e[0], h.scene.height_at(e[0], e[2]), e[2]), screen).expect("on screen");
    let at = (sx as f64, sy as f64);
    assert_eq!(h.world.pick(at.0 as f32, at.1 as f32), Some(beacon), "cursor is over the beacon");
    let order = |selection: Vec<u32>| {
        let (tx, _rx) = std::sync::mpsc::channel();
        PlayerControl::new(Some(1), selection, Some(tx)).right_click_order(&h.world, at)
    };
    assert_eq!(order(vec![tank]), Some(CommandKind::TransportLoadUnits), "a tank waits for the ferry");
    assert_eq!(order(vec![factory]), Some(CommandKind::TransportLoadUnits), "a factory sends its units there");
    assert_ne!(order(vec![ferry]), Some(CommandKind::TransportLoadUnits), "a transport doesn't board it");
}

/// Repro: with an engineer selected, a Fatboy under
/// construction offers the repair (assist) cursor, not the transport cursor;
/// once built the guard cursor, never load: its dock pads (attachpoint
/// bones, class 1 = air staging) fit no engineer (T3 is class 3). A
/// transport whose bones fit the passenger's class still offers load both
/// ways (the test above).
#[test]
fn engineer_onto_half_built_fatboy_offers_repair() {
    use super::player::PlayerControl;
    use crate::engine::sim::command::CommandKind;
    use crate::engine::sim::math::Fx;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            table.insert(ids, tonumber(CreateUnitHPR('uel0401', 'ARMY_1', 420, GetTerrainHeight(420, 590), 590, 0, 0, 0):GetEntityId()))
            table.insert(ids, tonumber(CreateUnitHPR('uel0309', 'ARMY_1', 400, GetTerrainHeight(400, 590), 590, 0, 0, 0):GetEntityId()))
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let [fatboy, engineer] = ids[..] else { panic!("two units") };
    // Half-build the Fatboy (no construction job of its own, so the
    // fraction stays put).
    if let Some(mut w) = h.engine.sim_lua().app_data_mut::<crate::engine::sim::entity::EntityWorld>() {
        let e = w.get_mut(fatboy).expect("fatboy");
        assert_eq!(e.cold.blueprint_id, "uel0401");
        e.fraction_complete = Fx::from_num(0.5);
    }
    h.ticks(3);
    {
        let y = h.scene.height_at(420.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(420.0, y, 590.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let at = |h: &Harness, x: f32, z: f32| -> (f64, f64) {
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(x, h.scene.height_at(x, z), z), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    let (tx, _rx) = std::sync::mpsc::channel();
    let p = PlayerControl::new(Some(1), vec![engineer], Some(tx));
    let e = h.scene.frame_entities.iter().find(|e| e.id == fatboy).unwrap().position;
    let (ex, ey) = at(&h, e[0], e[2]);
    assert_eq!(h.world.pick(ex as f32, ey as f32), Some(fatboy), "cursor is over the half-built Fatboy");
    assert_eq!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::Repair), "engineer onto a half-built Fatboy offers repair");
    // Once it is built, the load cursor is back.
    if let Some(mut w) = h.engine.sim_lua().app_data_mut::<crate::engine::sim::entity::EntityWorld>() {
        w.get_mut(fatboy).expect("fatboy").fraction_complete = Fx::ONE;
    }
    h.ticks(1);
    // Once it is built, the load cursor does not come back for the engineer:
    // the Fatboy's dock pads are class 1 (its air staging), the T3 engineer
    // is class 3, so boarding is impossible and moho shows the guard cursor.
    assert_eq!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::Guard), "engineer onto a finished Fatboy offers guard");
}

/// Engineers right-clicked onto one of themselves (which is
/// building) assist it, and it keeps building. Moho's interface sends the
/// whole selection one guard order (MohoEngine.dll 0x103e89c0 ->
/// `ISSUE_Command`), and the sim refuses it for the guarded unit itself
/// before clearing that unit's queue (`UNIT_IssueCommand` 0x102cbd20 asks
/// 0x102ca420, which fails a guard whose target is the unit). Ours gave the
/// targeted engineer a move to the ground under the cursor, which cleared
/// its build.
#[test]
fn engineers_assisting_one_of_themselves_leave_its_build_alone() {
    use super::player::{Event, Mods, PlayerControl};
    use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
    use crate::engine::sim::input::Scheduled;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:GiveStorage('MASS', 50000) b:GiveStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local ids = {}
            for _, x in { 390, 396, 402 } do
                table.insert(ids, tonumber(CreateUnitHPR('uel0105', 'ARMY_1', x, GetTerrainHeight(x, 585), 585, 0, 0, 0):GetEntityId()))
            end
            IssueBuildMobile({ GetUnitById(tostring(ids[2])) }, { 396.5, GetTerrainHeight(396.5, 597.5), 597.5 }, 'ueb1101', {})
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let [left, builder, right] = ids[..] else { panic!("three engineers") };
    let focus = |h: &Harness, id: u32| h.engine.sim_lua().app_data_ref::<crate::engine::sim::entity::EntityWorld>().unwrap().get(id).and_then(|e| e.unit.as_deref()?.focus);
    let current = |h: &Harness, id: u32| h.engine.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| c.kind);
    for _ in 0..100 {
        h.ticks(1);
        if focus(&h, builder).is_some() {
            break;
        }
    }
    let structure = focus(&h, builder).expect("the engineer started its power generator");
    {
        let y = h.scene.height_at(396.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(396.0, y, 590.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let e = h.scene.frame_entities.iter().find(|e| e.id == builder).unwrap().position;
    let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(e[0], h.scene.height_at(e[0], e[2]), e[2]), screen).expect("on screen");
    let (ex, ey) = (sx as f64, sy as f64);
    assert_eq!(h.world.pick(ex as f32, ey as f32), Some(builder), "cursor is over the building engineer");

    let (tx, rx) = std::sync::mpsc::channel();
    let mut p = PlayerControl::new(Some(1), ids.clone(), Some(tx));
    assert_eq!(p.right_click_order(&h.world, (ex, ey)), Some(CommandKind::Guard), "the selection over one of its own offers assist");
    p.handle(&mut h.world, Event::RightDown(ex, ey), Mods::default(), None);
    p.handle(&mut h.world, Event::RightUp(ex, ey), Mods::default(), None);
    let sent: Vec<_> = rx.try_iter().collect();
    assert_eq!(sent.len(), 1, "one order for the selection: {sent:?}");
    let c = &sent[0];
    assert_eq!((c.kind, c.target), (CommandKind::Guard, CommandTarget::Entity(builder)), "a guard of the clicked engineer");
    assert_eq!(c.units, ids, "the whole selection, the guarded engineer too (as moho sends it)");

    let now = crate::engine::api::sim_time::current_tick(h.engine.sim_lua());
    crate::engine::api::player_input::schedule(h.engine.sim_lua(), sent.into_iter().enumerate().map(|(seq, command)| Scheduled { tick: now + 1, client: 0, seq: seq as u32, command }));
    for _ in 0..200 {
        h.ticks(1);
        if [left, right].iter().all(|&e| focus(&h, e).is_some()) {
            break;
        }
    }
    assert_eq!(current(&h, builder), Some(CommandKind::BuildMobile), "the guarded engineer keeps its build");
    assert_eq!(focus(&h, builder), Some(structure), "and keeps building it");
    for e in [left, right] {
        assert_eq!(current(&h, e), Some(CommandKind::Guard), "engineer {e} assists");
        assert_eq!(focus(&h, e), Some(structure), "engineer {e} helps build the power generator");
    }
}

/// The unit under the cursor when no other selected unit could guard it.
/// moho's right-click order is the whole selection's (its command caps
/// together, MohoEngine.dll 0x103e7a40): with others selected the engineer's
/// own Guard makes it a guard of the engineer for all of them (the Mercy,
/// with no Guard of its own, too: the sim's check asks no command cap of a
/// guard), which the sim refuses for the engineer; selected alone, moho
/// offers no guard (0x103e6ce0: the selection is only the unit under the
/// cursor) and it's a move to the ground there.
#[test]
fn the_unit_under_the_cursor_with_no_order_on_it_from_the_rest() {
    use super::player::{Event, Mods, PlayerControl};
    use crate::engine::sim::command::{CommandKind, CommandTarget};
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            local engineer = CreateUnitHPR('uel0105', 'ARMY_1', 396, GetTerrainHeight(396, 585), 585, 0, 0, 0)
            -- The Mercy (no RULEUCC_Guard) on the ground beside it.
            local mercy = CreateUnitHPR('daa0206', 'ARMY_1', 404, GetTerrainHeight(404, 585), 585, 0, 0, 0)
            Warp(mercy, { 404, GetTerrainHeight(404, 585), 585 })
            return { tonumber(engineer:GetEntityId()), tonumber(mercy:GetEntityId()) }
            "#,
        )
        .eval()
        .expect("spawn");
    let [engineer, mercy] = ids[..] else { panic!("two units") };
    h.ticks(3);
    {
        let y = h.scene.height_at(396.0, 590.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(396.0, y, 590.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let e = h.scene.frame_entities.iter().find(|e| e.id == engineer).unwrap().position;
    let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(e[0], h.scene.height_at(e[0], e[2]), e[2]), screen).expect("on screen");
    let (ex, ey) = (sx as f64, sy as f64);
    assert_eq!(h.world.pick(ex as f32, ey as f32), Some(engineer), "cursor is over the engineer");
    let click = |h: &mut Harness, selection: Vec<u32>| {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut p = PlayerControl::new(Some(1), selection, Some(tx));
        p.handle(&mut h.world, Event::RightDown(ex, ey), Mods::default(), None);
        p.handle(&mut h.world, Event::RightUp(ex, ey), Mods::default(), None);
        rx.try_iter().map(|c| (c.kind, c.units, matches!(c.target, CommandTarget::Ground(_)))).collect::<Vec<_>>()
    };
    assert_eq!(click(&mut h, vec![engineer, mercy]), vec![(CommandKind::Guard, vec![engineer, mercy], false)], "one guard of the engineer for both");
    assert_eq!(click(&mut h, vec![engineer]), vec![(CommandKind::Move, vec![engineer], true)], "alone, the engineer moves there");
}

/// One right-click order for a mixed selection (engineers and tanks), as
/// moho picks it from the selection's command caps together (MohoEngine.dll
/// 0x103e7a40) and sends it to every selected unit (0x103e89c0); the sim
/// refuses it to those that can't take it (0x102ca420), which keep their
/// orders. On a damaged own tank: guard (before repair, with the profile's
/// default right-click behaviour), all of them; with
/// `switch_right_click_behavior`: repair, refused to the tanks; held into a
/// formation drag: no guard, so repair (the damaged ally). On an enemy:
/// attack, all of them (an engineer can move to attack). On a structure
/// being built: repair,
/// refused to the tanks. On a rock: reclaim, refused to the tanks. On the
/// ground: move.
#[test]
fn a_mixed_selection_gets_one_order() {
    use super::player::{Event, Mods, PlayerControl};
    use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
    use crate::engine::sim::input::{PlayerCommand, Scheduled};
    use crate::engine::sim::math::Fx;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            local function spawn(bp, army, x, z)
                local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
                table.insert(ids, tonumber(u:GetEntityId()))
                return u
            end
            spawn('uel0105', 'ARMY_1', 386, 578)
            spawn('uel0105', 'ARMY_1', 390, 578)
            spawn('uel0201', 'ARMY_1', 394, 578)
            spawn('uel0201', 'ARMY_1', 398, 578)
            local damaged = spawn('uel0201', 'ARMY_1', 388, 592)
            damaged:SetHealth(damaged, damaged:GetMaxHealth() / 2)
            local enemy = spawn('uel0201', 'ARMY_2', 404, 592)
            enemy:SetWeaponEnabledByLabel('MainGun', false)
            enemy:SetCanTakeDamage(false)
            spawn('ueb1101', 'ARMY_1', 380.5, 600.5)
            local rock = CreatePropHPR('/env/evergreen/props/rocks/fieldstone01_prop.bp', 396, GetTerrainHeight(396, 600), 600, 0, 0, 0)
            table.insert(ids, tonumber(rock:GetEntityId()))
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let [e1, e2, t1, t2, damaged, enemy, building, rock] = ids[..] else { panic!("eight entities") };
    let selection = vec![e1, e2, t1, t2];
    // The power generator half built (no construction job of its own, so
    // the fraction stays put).
    if let Some(mut w) = h.engine.sim_lua().app_data_mut::<crate::engine::sim::entity::EntityWorld>() {
        w.get_mut(building).expect("building").fraction_complete = Fx::from_num(0.5);
    }
    h.ticks(3);
    {
        let y = h.scene.height_at(394.0, 592.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(394.0, y, 592.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let at = |h: &Harness, id: u32| -> (f64, f64) {
        let e = h.scene.frame_entities.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} drawn")).position;
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(e[0], h.scene.height_at(e[0], e[2]), e[2]), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    let current = |h: &Harness, id: u32| h.engine.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| (c.kind, c.target));
    // A right click at `(x, y)` (held for `hold` first), the orders it sent
    // applied to the sim.
    let click = |h: &mut Harness, (x, y): (f64, f64), switch: bool, hold: bool| -> Vec<PlayerCommand> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut p = PlayerControl::new(Some(1), selection.clone(), Some(tx));
        p.switch_right_click = switch;
        p.handle(&mut h.world, Event::RightDown(x, y), Mods::default(), None);
        if hold {
            std::thread::sleep(std::time::Duration::from_millis(550));
        }
        p.handle(&mut h.world, Event::RightUp(x, y), Mods::default(), None);
        let sent: Vec<PlayerCommand> = rx.try_iter().collect();
        let now = crate::engine::api::sim_time::current_tick(h.engine.sim_lua());
        crate::engine::api::player_input::schedule(h.engine.sim_lua(), sent.iter().cloned().enumerate().map(|(seq, command)| Scheduled { tick: now + 1, client: 0, seq: seq as u32, command }));
        h.ticks(2);
        sent
    };
    let one = |sent: &[PlayerCommand], kind: CommandKind, target: Option<u32>, what: &str| {
        assert_eq!(sent.len(), 1, "{what}: one order, {sent:?}");
        assert_eq!(sent[0].kind, kind, "{what}");
        assert_eq!(sent[0].units, selection, "{what}: to the whole selection");
        match target {
            Some(t) => assert_eq!(sent[0].target, CommandTarget::Entity(t), "{what}"),
            None => assert!(matches!(sent[0].target, CommandTarget::Ground(_)), "{what}: on the ground"),
        }
    };
    let ground = at(&h, damaged);
    let ground = (ground.0 + 60.0, ground.1 + 40.0);

    // The ground: a move for all.
    let sent = click(&mut h, ground, false, false);
    one(&sent, CommandKind::Move, None, "ground");
    let moving = current(&h, t1).expect("moving");
    assert_eq!(moving.0, CommandKind::Move);

    // A structure being built: repair, refused to the tanks.
    let p = at(&h, building);
    let sent = click(&mut h, p, false, false);
    one(&sent, CommandKind::Repair, Some(building), "being built");
    for e in [e1, e2] {
        assert_eq!(current(&h, e), Some((CommandKind::Repair, CommandTarget::Entity(building))), "engineer {e} repairs");
    }
    for t in [t1, t2] {
        assert_eq!(current(&h, t).map(|c| c.0), Some(CommandKind::Move), "tank {t} keeps its move");
    }

    // A rock: reclaim, refused to the tanks.
    let p = at(&h, rock);
    let sent = click(&mut h, p, false, false);
    one(&sent, CommandKind::Reclaim, Some(rock), "rock");
    assert_eq!(current(&h, e1), Some((CommandKind::Reclaim, CommandTarget::Entity(rock))), "the engineer reclaims");
    assert_eq!(current(&h, t1).map(|c| c.0), Some(CommandKind::Move), "the tank keeps its move");

    // A damaged own tank, the profile's switched right click: repair,
    // refused to the tanks.
    let p = at(&h, damaged);
    let sent = click(&mut h, p, true, false);
    one(&sent, CommandKind::Repair, Some(damaged), "damaged, switched");
    assert_eq!(current(&h, e1), Some((CommandKind::Repair, CommandTarget::Entity(damaged))), "the engineer repairs");
    assert_eq!(current(&h, t1).map(|c| c.0), Some(CommandKind::Move), "the tank keeps its move");

    // The same, held into a formation drag: no guard, so the damaged ally is repaired.
    let p = at(&h, damaged);
    let sent = click(&mut h, p, false, true);
    one(&sent, CommandKind::Repair, Some(damaged), "damaged, held");

    // The same, a plain click: guard, all of them.
    let p = at(&h, damaged);
    let sent = click(&mut h, p, false, false);
    one(&sent, CommandKind::Guard, Some(damaged), "damaged");
    for u in &selection {
        assert_eq!(current(&h, *u), Some((CommandKind::Guard, CommandTarget::Entity(damaged))), "{u} guards");
    }

    // An enemy: attack, all of them.
    let p = at(&h, enemy);
    let sent = click(&mut h, p, false, false);
    one(&sent, CommandKind::Attack, Some(enemy), "enemy");
    for u in &selection {
        assert_eq!(current(&h, *u), Some((CommandKind::Attack, CommandTarget::Entity(enemy))), "{u} attacks");
    }
}

/// Attack-move and patrol as moho's attack and patrol command modes
/// (MohoEngine.dll 0x103e8420 takes the mode's order; 0x103e89c0 cases 4
/// and 0x10): attack-move on the ground splits the selection (0x103e7960),
/// the mobile units not on ground fire attack-moving and the rest (a point
/// defense, a tank on ground fire) attacking the ground; on a unit the
/// whole selection attacks it (an own unit's attack refused by the sim);
/// patrol goes to the whole selection. The interface's attack mode
/// (`StartCommandMode order RULEUCC_Attack`) splits the same.
#[test]
fn attack_move_and_patrol_go_to_the_whole_selection() {
    use super::player::{Event, Mode, Mods, PlayerControl};
    use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
    use crate::engine::sim::input::{PlayerCommand, Scheduled};
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: Vec<u32> = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            local function spawn(bp, army, x, z)
                local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
                table.insert(ids, tonumber(u:GetEntityId()))
                return u
            end
            spawn('uel0201', 'ARMY_1', 388, 580)
            spawn('uel0201', 'ARMY_1', 394, 580):SetFireState(2)
            spawn('ueb2101', 'ARMY_1', 400.5, 580.5)
            spawn('uel0105', 'ARMY_1', 388, 594)
            local enemy = spawn('uel0201', 'ARMY_2', 404, 594)
            enemy:SetWeaponEnabledByLabel('MainGun', false)
            enemy:SetCanTakeDamage(false)
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let [mover, grounded, defense, own, enemy] = ids[..] else { panic!("five units") };
    let selection = vec![mover, grounded, defense];
    h.ticks(3);
    {
        let y = h.scene.height_at(394.0, 588.0);
        let cam = h.world.camera_mut().expect("camera");
        cam.target = Vec3::new(394.0, y, 588.0);
        cam.zoom = 70.0;
    }
    h.frame();
    let screen = [640.0, 480.0];
    let at = |h: &Harness, id: u32| -> (f64, f64) {
        let e = h.scene.frame_entities.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} drawn")).position;
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(e[0], h.scene.height_at(e[0], e[2]), e[2]), screen).expect("on screen");
        (sx as f64, sy as f64)
    };
    let current = |h: &Harness, id: u32| h.engine.sim_lua().app_data_ref::<Commands>().unwrap().current(id).map(|c| (c.kind, c.target));
    let apply = |h: &mut Harness, sent: &[PlayerCommand]| {
        let now = crate::engine::api::sim_time::current_tick(h.engine.sim_lua());
        crate::engine::api::player_input::schedule(h.engine.sim_lua(), sent.iter().cloned().enumerate().map(|(seq, command)| Scheduled { tick: now + 1, client: 0, seq: seq as u32, command }));
        h.ticks(2);
    };
    let summary = |sent: &[PlayerCommand]| -> Vec<(CommandKind, Vec<u32>, bool)> { sent.iter().map(|c| (c.kind, c.units.clone(), matches!(c.target, CommandTarget::Ground(_)))).collect() };
    let alt = Mods { alt: true, ..Mods::default() };
    let right = |h: &mut Harness, (x, y): (f64, f64), mods: Mods| -> Vec<PlayerCommand> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut p = PlayerControl::new(Some(1), selection.clone(), Some(tx));
        p.handle(&mut h.world, Event::RightDown(x, y), mods, None);
        p.handle(&mut h.world, Event::RightUp(x, y), mods, None);
        rx.try_iter().collect()
    };
    let o = at(&h, own);
    let ground = (o.0 + 30.0, o.1 + 60.0);

    // Attack-move on the ground: split.
    let sent = right(&mut h, ground, alt);
    let mut got = summary(&sent);
    got.sort_by_key(|c| c.0 as u8);
    assert_eq!(got, vec![(CommandKind::Attack, vec![grounded, defense], true), (CommandKind::AggressiveMove, vec![mover], true)], "{sent:?}");
    apply(&mut h, &sent);
    assert_eq!(current(&h, mover).map(|c| c.0), Some(CommandKind::AggressiveMove));
    assert_eq!(current(&h, defense).map(|c| c.0), Some(CommandKind::Attack), "the point defense attacks the ground");

    // The interface's attack mode on the ground: the same split.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut p = PlayerControl::new(Some(1), selection.clone(), Some(tx));
    p.issue_mode(&h.world, ground, CommandKind::Attack, None, None, false);
    let mut got = summary(&rx.try_iter().collect::<Vec<_>>());
    got.sort_by_key(|c| c.0 as u8);
    assert_eq!(got, vec![(CommandKind::Attack, vec![grounded, defense], true), (CommandKind::AggressiveMove, vec![mover], true)]);

    // Attack-move on an own unit: one attack for all, refused by the sim.
    let p = at(&h, own);
    let sent = right(&mut h, p, alt);
    assert_eq!(summary(&sent), vec![(CommandKind::Attack, selection.clone(), false)]);
    apply(&mut h, &sent);
    assert_eq!(current(&h, mover).map(|c| c.0), Some(CommandKind::AggressiveMove), "no attack on an own unit: keeps its attack-move");

    // On an enemy: all attack it.
    let p = at(&h, enemy);
    let sent = right(&mut h, p, alt);
    assert_eq!(summary(&sent), vec![(CommandKind::Attack, selection.clone(), false)]);
    apply(&mut h, &sent);
    for u in [mover, grounded, defense] {
        assert_eq!(current(&h, u), Some((CommandKind::Attack, CommandTarget::Entity(enemy))), "{u} attacks the enemy");
    }

    // Patrol: the whole selection.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut p = PlayerControl::new(Some(1), selection.clone(), Some(tx));
    p.mode = Mode::Patrol;
    p.handle(&mut h.world, Event::LeftDown(ground.0, ground.1), Mods::default(), None);
    assert_eq!(summary(&rx.try_iter().collect::<Vec<_>>()), vec![(CommandKind::Patrol, selection.clone(), true)]);
}

/// Largest distance between where the sim poses each bone of `id`
/// (`sim::pose`, used for muzzles and `GetPosition(bone)`) and where the
/// renderer draws it; how many bones were compared; the largest distance of a
/// drawn bone from its rest pose.
fn sim_vs_drawn_bones(h: &Harness, id: u32) -> (f32, usize, f32) {
    let lua = h.engine.sim_lua();
    let world = lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>().expect("world");
    let e = world.get(id).expect("entity");
    let bones = e.cold.skeleton.as_ref().map_or(0, |s| s.bones.len());
    let mut worst = (0.0f32, 0, 0.0f32);
    for b in 0..bones as u32 {
        let Some(m) = h.scene.bone_world(id, b) else { continue };
        let drawn = m.transform_point3(Vec3::ZERO);
        let (x, y, z) = crate::engine::api::entity::posed_bone(lua, e, Some(b)).0.to_f64();
        let d = drawn.distance(Vec3::new(x as f32, y as f32, z as f32));
        worst = (worst.0.max(d), worst.1 + 1, worst.2);
        // How far the pose moved it from rest (proves something was posed).
        let (rx, ry, rz) = e.bone_world_position(Some(b)).to_f64();
        worst.2 = worst.2.max(Vec3::new(rx as f32, ry as f32, rz as f32).distance(drawn));
    }
    worst
}

/// The sim poses bones where they are drawn: the Fatboy with its turrets
/// aimed (aim manipulators), and a walking Hunter bot mid-stride (walk
/// clip), so projectiles leave from the drawn muzzles.
#[test]
fn sim_bone_poses_match_drawn_bones() {
    let Some(mut h) = Harness::new(Some("tests/scenarios/fatboy.lua"), false, None) else { return };
    h.engine
        .sim_lua()
        .load("local u = CreateUnitHPR('url0106', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0) rawset(_G, 'TestBot', u) IssueMove({u}, {440, 0, 600})")
        .exec()
        .expect("bot");
    h.ticks(80);
    h.settle();
    h.ticks(3);
    let fatboy = fatboy_id(&h);
    let bot: u32 = h.engine.sim_lua().load("return tonumber(TestBot:GetEntityId())").eval().expect("bot id");
    {
        let m = h.engine.sim_lua().app_data_ref::<crate::engine::sim::manipulator::Manipulators>().expect("manipulators");
        let walking = m.of(bot).any(|m| matches!(&m.kind, crate::engine::sim::manipulator::ManipulatorKind::Animator(a) if a.clip.is_some() && a.time > crate::engine::sim::math::Fx::ZERO));
        assert!(walking, "the bot should be mid walk clip");
    }
    for (name, id) in [("fatboy", fatboy), ("hunter bot", bot)] {
        let (worst, n, moved) = sim_vs_drawn_bones(&h, id);
        assert!(n > 5, "{name}: only {n} bones drawn");
        assert!(moved > 0.05, "{name}: no bone is posed away from rest ({moved:.3}); nothing tested");
        // Slerp (renderer) vs normalised lerp (sim) between clip keyframes,
        // and f32 vs fixed point: a few hundredths at most.
        assert!(worst < 0.05, "{name}: a bone is {worst:.3} from where it is drawn");
    }
}

/// Frame cost with the 16-army stress (16k land units, SCMP_030-sized map):
/// CPU prepare (interpolation, instances, poses) and draw + GPU wait at
/// 1920x1080, zoomed out over the whole map and in over one army. Measurement:
/// `cargo test --release --bin rts-core-engine frame_cost_16k -- --ignored --nocapture`.
#[test]
#[ignore]
fn frame_cost_16k() {
    let Some(mut h) = Harness::new_in(Some("dev/scenes/stress/stress.lua"), false, Some("/maps/stress_16/stress_16_scenario.lua"), std::path::Path::new("dev/vault")) else { return };
    // `FRAME_COST_VIEW=<army>`: seen as that army, fog of war on (its
    // side's vision circles drawn).
    h.request.lock().unwrap().view_army = std::env::var("FRAME_COST_VIEW").ok().and_then(|v| v.parse().ok());
    h.ticks(60);
    h.settle();
    h.ticks(2);
    let target = h.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d { width: 1920, height: 1080, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    h.resize(1920, 1080);
    // As the game draws: only what's in view is posed and drawn.
    h.scene.record_all_bones = false;
    let units = h.scene.frame_entities.iter().filter(|e| e.kind == crate::engine::snapshot::ViewKind::Unit).count();
    let first = h.scene.frame_entities.iter().find(|e| e.kind == crate::engine::snapshot::ViewKind::Unit).map(|e| e.position).unwrap_or([0.0; 3]);
    let (w, d) = h.scene.map_size.unwrap_or((1024.0, 1024.0));
    for (name, at, dist) in [("whole map", Vec3::new(w / 2.0, 0.0, d / 2.0), w.max(d)), ("one army", Vec3::from_array(first), 120.0)] {
        if let Some(c) = h.world.camera_mut() {
            c.target = at;
            c.zoom = dist.clamp(c.min_zoom, c.max_zoom);
        }
        let mut prep = f64::MAX;
        let mut draw = f64::MAX;
        let iters: u32 = std::env::var("FRAME_COST_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
        for _ in 0..iters {
            let t = Instant::now();
            h.frame();
            prep = prep.min(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            h.queue.submit(h.scene.draw(&h.device, &view, None, None).into_iter().map(|e| e.finish()));
            let _ = h.device.poll(wgpu::PollType::wait_indefinitely());
            draw = draw.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        eprintln!("frame_cost_16k {name}: {units} units, {} entities ({} culled), {} vision circles; prepare {prep:.2} ms, draw + GPU {draw:.2} ms", h.scene.frame_entities.len(), h.scene.stage.culled, h.scene.vision_circles().len());
    }
}

/// Draws the scene offscreen at `w` x `h` and reads it back as RGBA8.
fn render_rgba(h: &mut Harness, w: u32, ht: u32) -> Vec<u8> {
    render_views_rgba(h, w, ht, Vec::new())
}

/// [`render_rgba`] with these world views (none: the main camera fills
/// the frame).
fn render_views_rgba(h: &mut Harness, w: u32, ht: u32, views: Vec<super::camera::View>) -> Vec<u8> {
    // The scene's pipelines are built for the harness's BGRA target.
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let target = h.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("golden"),
        size: wgpu::Extent3d { width: w, height: ht, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    h.resize(w, ht);
    if views.is_empty() {
        if let Some(c) = h.world.camera_mut() {
            c.aspect = w as f32 / ht as f32;
        }
    } else {
        h.world.set_views(views);
    }
    h.sync();
    h.scene.prepare(&h.device, &h.queue);
    let row = (w * 4).div_ceil(256) * 256;
    let buffer = h.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (row * ht) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let drawn = h.scene.draw(&h.device, &view, None, None);
    let mut enc = h.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(ht) } },
        wgpu::Extent3d { width: w, height: ht, depth_or_array_layers: 1 },
    );
    h.queue.submit(drawn.into_iter().map(|e| e.finish()).chain([enc.finish()]));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = h.device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((w * ht * 4) as usize);
    for y in 0..ht {
        for px in data[(y * row) as usize..(y * row + w * 4) as usize].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    out
}

/// Compares `rgba` with `tests/golden/images/<name>.png` (or writes it with
/// `UPDATE_GOLDEN=1`). Tolerant of GPU / driver rounding: mean channel
/// difference under 2, and under 1% of pixels off by more than 32 in any
/// channel. A failing frame is saved beside it as `<name>.actual.png`.
fn golden_image(name: &str, rgba: &[u8], w: u32, h: u32) {
    let dir = std::path::Path::new("tests/golden/images");
    let path = dir.join(format!("{name}.png"));
    let write = |p: &std::path::Path| {
        std::fs::create_dir_all(dir).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(p).unwrap()), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(rgba).unwrap();
    };
    if std::env::var("UPDATE_GOLDEN").is_ok() || !path.exists() {
        write(&path);
        eprintln!("golden image written: {}", path.display());
        return;
    }
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut want = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut want).unwrap();
    assert_eq!((info.width, info.height), (w, h), "{name}: golden size differs");
    let (mut sum, mut bad) = (0u64, 0usize);
    for (a, b) in rgba.chunks_exact(4).zip(want.chunks_exact(4)) {
        let d: Vec<u32> = (0..3).map(|i| (a[i] as i32 - b[i] as i32).unsigned_abs()).collect();
        sum += d.iter().map(|&x| x as u64).sum::<u64>();
        bad += d.iter().any(|&x| x > 32) as usize;
    }
    let pixels = (w * h) as usize;
    let mean = sum as f64 / (pixels * 3) as f64;
    let frac = bad as f64 / pixels as f64;
    if mean >= 2.0 || frac >= 0.01 {
        write(&dir.join(format!("{name}.actual.png")));
        panic!("{name}: frame differs from its golden (mean channel diff {mean:.2}, {:.2}% pixels off by > 32); see {name}.actual.png, re-record with UPDATE_GOLDEN=1 if intended", frac * 100.0);
    }
}

/// Golden images of fixed scenes (no AI, units holding fire, fixed
/// interpolation): the UEF start area close up (terrain shader, decals,
/// trees, models with team colour, sun shadows) and the whole map zoomed
/// out (strategic icons over the ground). Re-record after intended visual changes:
/// `UPDATE_GOLDEN=1 cargo test --bin rts-core-engine golden_images`.
#[test]
fn golden_images() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local ids = {'uel0201', 'uel0202', 'uel0303', 'uel0105', 'uel0101'}
            for i = 0, 9 do
                local x, z = 660 + math.mod(i, 5) * 4, 360 + math.floor(i / 5) * 5
                local u = CreateUnitHPR(ids[math.mod(i, 5) + 1], 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0.3 * i, 0)
                u:SetFireState(1)
            end
            for i = 0, 3 do
                local u = CreateUnitHPR('ual0201', 'ARMY_2', 670 + i * 4, GetTerrainHeight(670 + i * 4, 380), 380, 0, 3.1, 0)
                u:SetFireState(1)
            end
            "#,
        )
        .exec()
        .expect("spawn");
    h.ticks(3);
    h.settle();
    h.ticks(1);
    h.scene.record_all_bones = false;
    h.scene.frozen_time = Some(0.0);
    let (w, ht) = (480, 270);
    for (name, at, dist) in [("start_area", Vec3::new(668.0, 18.0, 366.0), 55.0), ("whole_map", Vec3::new(512.0, 18.0, 512.0), f32::MAX)] {
        if let Some(c) = h.world.camera_mut() {
            c.target = at;
            c.zoom = dist.min(c.max_zoom);
        }
        h.frame();
        h.settle();
        let rgba = render_rgba(&mut h, w, ht);
        golden_image(name, &rgba, w, ht);
    }
}

/// A camera turned and tilted by Space and the mouse (moho's spin): the
/// start area seen from the east, nearly level, against its golden; a tank
/// projects into the frame and the ground picked under it is the tank's.
#[test]
fn spun_camera_image() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local ids = {'uel0201', 'uel0202', 'uel0303', 'uel0105', 'uel0101'}
            for i = 0, 9 do
                local x, z = 660 + math.mod(i, 5) * 4, 360 + math.floor(i / 5) * 5
                local u = CreateUnitHPR(ids[math.mod(i, 5) + 1], 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0.3 * i, 0)
                u:SetFireState(1)
            end
            "#,
        )
        .exec()
        .expect("spawn");
    h.ticks(3);
    h.settle();
    h.ticks(1);
    h.scene.record_all_bones = false;
    h.scene.frozen_time = Some(0.0);
    let (w, ht) = (480, 270);
    if let Some(c) = h.world.camera_mut() {
        c.target = Vec3::new(668.0, 18.0, 366.0);
        c.zoom = 60.0;
        // A quarter turn to face west (from the east), tilted toward level.
        c.spin(-(w as f32) / 4.0, -40.0, w as f32);
    }
    h.settle();
    let rgba = render_rgba(&mut h, w, ht);
    golden_image("spun_camera", &rgba, w, ht);
    let cam = *h.world.camera().unwrap();
    assert!((cam.heading + std::f32::consts::FRAC_PI_2).abs() < 1e-3, "faces west: {}", cam.heading);
    let tank = h.scene.frame_entities.iter().find(|e| e.kind == crate::engine::snapshot::ViewKind::Unit).copied().expect("a tank");
    let [x, y] = cam.project(Vec3::from_array(tank.position) + Vec3::Y * 0.3, [w as f32, ht as f32]).expect("in front");
    assert!(x >= 0.0 && y >= 0.0 && x < w as f32 && y < ht as f32, "the tank is in view: {x}, {y}");
    let g = h.world.ground_at(x, y).expect("ground");
    assert!((g.x - tank.position[0]).abs() < 3.0 && (g.z - tank.position[2]).abs() < 3.0, "ground {g} under the tank");
}

/// Spread orders (FAF's spread attack, or units spread out): every unit has
/// its own waypoints and lines, drawn close up (waypoint icons following the
/// ground) and further out, against their goldens.
#[test]
fn spread_orders_command_graph_images() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            r#"
            local out = {}
            for i = 0, 47 do
                local x, z = 640 + math.mod(i, 8) * 8, 330 + math.floor(i / 8) * 8
                local u = CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
                u:SetFireState(1)
                u:SetSpeedMult(0)
                for w = 1, 3 do
                    local tx, tz = 620 + math.mod(i * 37 + w * 101, 110), 310 + math.mod(i * 53 + w * 71, 90)
                    IssueMove({ u }, Vector(tx, GetTerrainHeight(tx, tz), tz))
                end
                table.insert(out, tonumber(u:GetEntityId()))
            end
            return out
            "#,
        )
        .eval()
        .expect("spawn");
    let mut selection: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    selection.sort_unstable();
    h.input.selection = selection.clone();
    h.request.lock().unwrap().selection = selection;
    h.ticks(3);
    h.settle();
    h.ticks(2);
    h.scene.record_all_bones = false;
    h.scene.frozen_time = Some(0.0);
    let (w, ht) = (960, 540);
    for (name, dist) in [("spread_orders_close", 100.0), ("spread_orders_far", 350.0)] {
        if let Some(c) = h.world.camera_mut() {
            c.target = Vec3::new(668.0, 18.0, 352.0);
            c.zoom = dist;
        }
        h.frame();
        h.settle();
        let rgba = render_rgba(&mut h, w, ht);
        golden_image(name, &rgba, w, ht);
    }
}

/// Big selections' own range rings against their goldens: a blob of
/// tanks with artillery inside (minimum range lines), a ring of tanks whose
/// lines all cross its centre, and a hollow ring of tanks, close up and
/// further out; and the same with every grid cell's list overflowed.
#[test]
fn selected_range_ring_images() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            r#"
            local out = {}
            local function put(bp, x, z)
                local u = CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
                u:SetFireState(1)
                u:SetSpeedMult(0)
                table.insert(out, tonumber(u:GetEntityId()))
            end
            for i = 0, 119 do
                put('uel0201', 650 + math.mod(i, 12) * 3, 340 + math.floor(i / 12) * 3)
            end
            for i = 0, 5 do
                put('uel0103', 655 + i * 5, 352)
            end
            for i = 0, 31 do
                local a = i * math.pi / 16
                put('uel0201', 600 + 18 * math.cos(a), 400 + 18 * math.sin(a))
                put('uel0201', 730 + 40 * math.cos(a), 400 + 40 * math.sin(a))
            end
            return out
            "#,
        )
        .eval()
        .expect("spawn");
    let mut selection: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    selection.sort_unstable();
    h.input.selection = selection.clone();
    h.request.lock().unwrap().selection = selection;
    let kind = |k: &str| crate::engine::snapshot::RANGE_KINDS.iter().position(|n| *n == k).unwrap();
    let mut styles = vec![None; crate::engine::snapshot::RANGE_KINDS.len()];
    styles[kind("DIRECTFIRE")] = Some(rts_core_engine::ui::RangeStyle { colors: [[1.0, 0.17, 0.17, 0.0]; 3], inner: [0.02, 2.0], outer: [0.04, 4.0] });
    styles[kind("INDIRECTFIRE")] = Some(rts_core_engine::ui::RangeStyle { colors: [[1.0, 0.8, 0.1, 0.1]; 3], inner: [0.02, 2.0], outer: [0.04, 4.0] });
    h.input.range_styles = std::sync::Arc::new(styles);
    h.ticks(3);
    h.settle();
    h.ticks(2);
    h.scene.record_all_bones = false;
    h.scene.frozen_time = Some(0.0);
    h.settings.smaa = false;
    let (w, ht) = (960, 540);
    for (name, at, dist) in [("rings_blob_close", Vec3::new(668.0, 18.0, 356.0), 155.0), ("rings_blob_far", Vec3::new(668.0, 18.0, 370.0), 480.0), ("rings_circles", Vec3::new(665.0, 18.0, 400.0), 350.0)] {
        if let Some(c) = h.world.camera_mut() {
            c.target = at;
            c.zoom = dist;
        }
        h.frame();
        h.settle();
        let rgba = render_rgba(&mut h, w, ht);
        golden_image(name, &rgba, w, ht);
        // Every cell's list overflowed: each pixel tests all its group's
        // rings. The ring lines (opaque red and yellow) are the same pixels;
        // the rest of the scene can differ a little between two renders.
        let lists = render_rgba(&mut h, w, ht);
        h.scene.set_range_list_limit(Some(0));
        let over = render_rgba(&mut h, w, ht);
        let ring = |p: &[u8]| p[0] > 200 && p[2] < 100;
        let (mut lines, mut off) = (0, 0);
        for (a, b) in lists.chunks_exact(4).zip(over.chunks_exact(4)) {
            if ring(a) || ring(b) {
                lines += 1;
                assert_eq!(a, b, "{name}: the overflow path draws the lines the lists do");
            } else {
                off += (a != b) as usize;
            }
        }
        assert!(lines > 500, "{name}: ring lines drawn ({lines} pixels)");
        assert!(off * 100 < (w * ht) as usize, "{name}: {off} other pixels differ");
        h.scene.set_range_list_limit(None);
    }
}

/// FAF's split view: two world views side by side, each drawing its own
/// camera into its half (`WorldCamera` close on the UEF start area on the
/// left, `WorldCamera2` over the whole map on the right), matching their
/// golden (`UPDATE_GOLDEN=1` re-records it). Strategic icons are posed per
/// view (the zoomed-out right shows them, the close left doesn't), and a
/// point in a half is picked and found on the ground through that half's
/// camera.
#[test]
fn split_view_draws_each_camera_in_its_half() {
    use super::camera::View;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local ids = {'uel0201', 'uel0202', 'uel0303', 'uel0105', 'uel0101'}
            for i = 0, 9 do
                local x, z = 660 + math.mod(i, 5) * 4, 360 + math.floor(i / 5) * 5
                local u = CreateUnitHPR(ids[math.mod(i, 5) + 1], 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0.3 * i, 0)
                u:SetFireState(1)
            end
            "#,
        )
        .exec()
        .expect("spawn");
    h.ticks(3);
    h.settle();
    h.ticks(1);
    h.scene.record_all_bones = false;
    h.scene.frozen_time = Some(0.0);
    let (w, ht) = (480, 270);
    let views = vec![View { camera: "WorldCamera".into(), rect: [0.0, 0.0, 237.0, 270.0] }, View { camera: "WorldCamera2".into(), rect: [243.0, 0.0, 480.0, 270.0] }];
    h.resize(w, ht);
    h.world.set_views(views.clone());
    for (name, at, dist) in [("WorldCamera", Vec3::new(668.0, 18.0, 366.0), 31.0f32), ("WorldCamera2", Vec3::new(512.0, 18.0, 512.0), f32::MAX)] {
        let c = h.world.cameras.get_mut(name).expect("a camera per view");
        c.target = at;
        c.zoom = dist.min(c.max_zoom);
    }
    h.frame();
    h.settle();
    let rgba = render_views_rgba(&mut h, w, ht, views.clone());
    golden_image("split_view", &rgba, w, ht);
    // The gap between the views stays clear: no view draws past its own
    // rectangle. (Without the bloom, which blurs the whole frame, so a
    // bright icon at a view's edge glows a few pixels across.)
    h.settings.bloom = false;
    let rgba = render_views_rgba(&mut h, w, ht, views.clone());
    h.settings.bloom = true;
    let gap = (0..ht).map(|y| &rgba[((y * w + 240) * 4) as usize..((y * w + 240) * 4 + 3) as usize]).filter(|p| p.iter().any(|&c| c > 8)).count();
    assert_eq!(gap, 0, "the gap between the views was drawn over");
    // Icons per view: the far camera's list, not the close one's.
    assert_eq!(h.scene.icon_cameras, ["WorldCamera", "WorldCamera2"]);
    let own = |l: &Vec<super::scene::Icon>| l.iter().filter(|i| i.2 == 1).count();
    assert_eq!(own(&h.scene.icons[0]), 0, "the close view shows meshes, no icons");
    assert!(own(&h.scene.icons[1]) >= 10, "the whole-map view shows the tanks' icons: {}", own(&h.scene.icons[1]));
    // Each half picks and finds the ground through its own camera.
    let tank = h.scene.frame_entities.iter().find(|e| e.army == 1 && e.kind == crate::engine::snapshot::ViewKind::Unit).copied().expect("a tank");
    for (v, cam) in views.iter().map(|v| (v, *h.world.cameras.get(&v.camera).unwrap())) {
        let [x, y] = v.project(&cam, Vec3::from_array(tank.position) + Vec3::Y * 0.3).expect("in front");
        assert!(v.contains(x, y), "{}: the tank projects into its view", v.camera);
        let g = h.world.ground_at(x, y).expect("ground");
        assert!((g.x - tank.position[0]).abs() < 3.0 && (g.z - tank.position[2]).abs() < 3.0, "{}: ground {g} under the tank", v.camera);
    }
    let close = &views[0];
    let cam = *h.world.cameras.get(&close.camera).unwrap();
    let [x, y] = close.project(&cam, Vec3::from_array(tank.position) + Vec3::Y * 0.3).unwrap();
    assert!(h.world.pick(x, y).is_some(), "the close view picks the tank under the point");
}

/// With a renderer attached, each beat's `Sync` table reaches the scene
/// for the user layer (the AIs' units fill `Sync.UnitData` etc.).
#[test]
fn sim_sync_tables_reach_the_renderer() {
    let Some(mut h) = Harness::new(None, true, None) else { return };
    h.ticks(60);
    assert!(!h.world.pending_sync.is_empty(), "no Sync copies arrived");
}

/// The session's armies reach the scene for the user layer
/// (GetArmiesTable: names, factions, colours).
#[test]
fn armies_reach_the_renderer() {
    let Some(mut h) = Harness::new(None, true, None) else { return };
    h.ticks(2);
    let armies = h.world.armies.clone().expect("armies with the first snapshot");
    assert!(armies.len() >= 2 && armies.iter().all(|a| (1..=4).contains(&a.faction)), "{armies:?}");
}

/// The sim's alliances reach the scene for the user layer (IsAlly):
/// every army has its alliance with each other one set up at start.
#[test]
fn alliances_reach_the_renderer() {
    use crate::engine::sim::army::Alliance;
    let Some(mut h) = Harness::new(None, true, None) else { return };
    h.ticks(2);
    let a = h.world.alliances.clone().expect("alliances with the first snapshot");
    assert!(a.iter().any(|&(f, t, x)| (f, t) == (1, 2) && x == Alliance::Enemy), "{a:?}");
}

/// The sim's ScenarioInfo reaches the scene for the user layer, with its
/// options and the playable area moho adds.
#[test]
fn scenario_info_reaches_the_renderer() {
    use crate::engine::sync::SyncValue as V;
    let Some(mut h) = Harness::new(None, true, None) else { return };
    h.ticks(2);
    let info = h.world.scenario.clone().expect("scenario with the first snapshot");
    let V::Table(pairs) = &*info else { panic!("not a table") };
    let get = |k: &str| pairs.iter().find(|(key, _)| *key == V::Str(k.into())).map(|(_, v)| v.clone());
    assert_eq!(get("PlayableAreaWidth"), Some(V::Num(1024.0)));
    let Some(V::Table(opts)) = get("Options") else { panic!("no Options") };
    assert!(opts.iter().any(|(k, _)| *k == V::Str("Share".into())));
}

/// Build placement: the sim answers the renderer's query (open ground yes,
/// on top of a structure no) and the structure's mesh is drawn as FAF's
/// `UnitPlace` ghost; the structures around it show their skirts.
/// `GHOST_PNG=<dir>` saves the two frames to look at.
#[test]
fn build_placement_ghost() {
    use crate::render::scene::Ghost;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            for _, p in {{668, 366}, {668, 360}, {662, 366}, {662, 360}, {674, 356}} do
                CreateUnitHPR('ueb1101', 'ARMY_1', p[1], GetTerrainHeight(p[1], p[2]), p[2], 0, 0, 0)
            end
            "#,
        )
        .exec()
        .expect("spawn");
    h.ticks(2);
    let bp = h.world.blueprints.as_ref().unwrap().iter().position(|b| b == "ueb0101").unwrap() as u32;
    let mesh = h.world.blueprint_meshes.as_ref().unwrap()[bp as usize];
    assert_ne!(mesh.0, u32::MAX, "the land factory's mesh is sent for placement");
    h.scene.record_all_bones = false;
    if let Some(c) = h.world.camera_mut() {
        c.target = Vec3::new(672.0, 18.0, 366.0);
        c.zoom = 40.0;
    }
    for (name, x, want) in [("blocked", 668.0, false), ("open", 684.0, true)] {
        let p = Vec3::new(x, h.scene.height_at(x, 366.0), 366.0);
        h.request.lock().unwrap().placement = vec![("ueb0101".into(), p.to_array())];
        h.ticks(1);
        let answer = h.world.placement.first().cloned().expect("placement answered");
        assert_eq!((answer.0.as_str(), answer.1, answer.3), ("ueb0101", p.to_array(), want), "{name}");
        // The land factory's skirt round the spot, outlined like a queued
        // build's site.
        let site = answer.2.expect("the factory has a skirt");
        assert!(site[0] < p.x && site[2] > p.x && site[1] < p.z && site[3] > p.z, "{name}: skirt {site:?} round the spot");
        h.input.ghost = Some(Ghost { blueprint: bp, position: p, color: if want { [0.0, 216.0 / 255.0, 0.0] } else { [1.0, 0.15, 0.15] }, site: Some(site) });
        h.settle();
        assert!(h.scene.ghost_drawn(), "{name}: ghost drawn");
        assert!(h.scene.graph(0).expect("view prepared").sites.contains(&(site, false)), "{name}: its site outlined");
        // The power generators round it: their skirts outlined too.
        assert_eq!(h.scene.graph(0).expect("view prepared").nearby_sites.len(), 5, "{name}: the standing structures' skirts");
        if let Ok(dir) = std::env::var("GHOST_PNG") {
            let (w, ht) = (640, 360);
            let rgba = render_rgba(&mut h, w, ht);
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(std::path::Path::new(&dir).join(format!("ghost_{name}.png"))).unwrap()), w, ht);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&rgba).unwrap();
        }
    }
}

/// Units under construction wear FAF's `<mesh>_build` blueprint and draw
/// with their faction's build-in technique (fraction complete passed on),
/// the UEF build cube with `UEFBuildCube`; the pattern textures load.
/// Visual check: `cargo run -- --script dev/scenes/visual/build_effects.lua`.
#[test]
fn units_under_construction_use_build_shaders() {
    use crate::engine::api::mesh::BuildShader;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local brain = ArmyBrains[1]
            brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
            brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
            local function at(x, z) return {x, GetTerrainHeight(x, z), z} end
            local uef = CreateUnitHPR('uel0105', 'ARMY_1', 664, GetTerrainHeight(664, 366), 366, 0, 0, 0)
            IssueBuildMobile({uef}, at(668.5, 366.5), 'ueb0101', {})
            local cyb = CreateUnitHPR('url0105', 'ARMY_1', 664, GetTerrainHeight(664, 380), 380, 0, 0, 0)
            IssueBuildMobile({cyb}, at(668.5, 380.5), 'urb0101', {})
            local sera = CreateUnitHPR('xsl0105', 'ARMY_1', 678, GetTerrainHeight(678, 366), 366, 0, 0, 0)
            IssueBuildMobile({sera}, at(682.5, 366.5), 'xsb0101', {})
            "#,
        )
        .exec()
        .expect("spawn");
    if let Some(c) = h.world.camera_mut() {
        c.target = Vec3::new(668.0, 18.0, 373.0);
        c.zoom = 40.0;
    }
    h.ticks(60);
    h.settle();
    let drawn = h.scene.drawn_build_models();
    for style in [BuildShader::Uef, BuildShader::Cybran, BuildShader::Seraphim] {
        let f = drawn.iter().find(|(s, _)| *s == style).unwrap_or_else(|| panic!("{style:?} build model drawn: {drawn:?}")).1;
        assert!(f > 0.0 && f < 1.0, "{style:?} fraction complete {f}");
    }
    assert!(drawn.iter().any(|(s, _)| *s == BuildShader::UefBuildCube), "the UEF build cube: {drawn:?}");
    let failed: Vec<String> = h.scene.asset_failures().into_iter().filter(|f| f.starts_with("secondary")).collect();
    assert!(failed.is_empty(), "build pattern textures load: {failed:?}");
}

/// A Cybran structure going up hides what's behind it: moho draws
/// `CybranBuild` in its depth stage, so the real game shows the structure's
/// nearest surface at 40% over black, not over the ground. A scorch decal
/// laid under a half-built power generator darkens the ground beside it but
/// not the structure; ours used to blend the structure over the ground, so
/// the scorch showed through at 60% and the structure read as glass.
/// Visual check (and against the real game): `dev/probes/maps/probe_cybranbuild`.
#[test]
fn cybran_build_hides_what_is_behind_it() {
    use crate::engine::api::mesh::BuildShader;
    use crate::engine::sim::math::Fx;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local brain = ArmyBrains[1]
            brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
            brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
            local e = CreateUnitHPR('url0105', 'ARMY_1', 668, GetTerrainHeight(668, 370), 370, 0, 0, 0)
            IssueBuildMobile({e}, {668, GetTerrainHeight(668, 366), 366}, 'urb1101', {})
            ForkThread(function()
                while not (e:GetFocusUnit() and e:GetFocusUnit():GetFractionComplete() > 0.1) do WaitTicks(1) end
                ProbeFocus = e:GetFocusUnit()
                IssueClearCommands({e})
            end)
            "#,
        )
        .exec()
        .expect("spawn");
    h.ticks(80);
    let id: String = h.engine.sim_lua().load("return ProbeFocus:GetEntityId()").eval().expect("the structure going up");
    let id: u32 = id.parse().expect("entity id");
    // Half built; its engineer stopped, so the fraction stays put (and no
    // beams cross it).
    if let Some(mut w) = h.engine.sim_lua().app_data_mut::<crate::engine::sim::entity::EntityWorld>() {
        w.get_mut(id).expect("power generator").fraction_complete = Fx::from_num(0.5);
    }
    h.scene.record_all_bones = false;
    h.settings.smaa = false;
    let ground = h.scene.height_at(668.5, 366.5);
    if let Some(c) = h.world.camera_mut() {
        c.target = Vec3::new(668.5, ground, 366.5);
        c.zoom = 9.0;
    }
    h.ticks(30);
    h.settle();
    let drawn = h.scene.drawn_build_models();
    assert!(drawn.iter().any(|&(s, f)| s == BuildShader::Cybran && (f - 0.5).abs() < 0.01), "the Cybran build mesh draws at 50%: {drawn:?}");
    let (w, ht) = (480, 270);
    let bare = render_rgba(&mut h, w, ht);
    h.engine.sim_lua().load("CreateDecal({668.5, GetTerrainHeight(668.5, 366.5), 366.5}, 0, 'scorch_001_albedo', '', 'Albedo', 9, 9, 1000, 0, 1)").exec().expect("decal");
    h.ticks(1);
    h.settle();
    let scorched = render_rgba(&mut h, w, ht);
    if let Ok(dir) = std::env::var("CYBRAN_BUILD_PNG") {
        for (name, img) in [("bare", &bare), ("scorched", &scorched)] {
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(std::path::Path::new(&dir).join(format!("cybran_build_{name}.png"))).unwrap()), w, ht);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(img).unwrap();
        }
    }
    // Mean luma of the 7x7 pixels round world point (x, z).
    let patch = |img: &[u8], x: f32, z: f32| {
        let [sx, sy] = h.world.camera().unwrap().project(Vec3::new(x, h.scene.height_at(x, z), z), [w as f32, ht as f32]).expect("on screen");
        let mut sum = 0.0;
        for dy in -3..=3 {
            for dx in -3..=3 {
                let i = (((sy as i32 + dy) as u32 * w + (sx as i32 + dx) as u32) * 4) as usize;
                sum += 0.299 * img[i] as f32 + 0.587 * img[i + 1] as f32 + 0.114 * img[i + 2] as f32;
            }
        }
        sum / 49.0
    };
    // The ground beside it (outside the 2x2 skirt): the scorch drew.
    let (beside_bare, beside_scorched) = (patch(&bare, 670.5, 366.5), patch(&scorched, 670.5, 366.5));
    assert!(beside_scorched < beside_bare - 8.0, "the scorch darkens the ground beside the structure: {beside_bare:.1} -> {beside_scorched:.1}");
    // The structure itself: the same over either ground.
    let (on_bare, on_scorched) = (patch(&bare, 668.5, 366.5), patch(&scorched, 668.5, 366.5));
    assert!((on_bare - on_scorched).abs() < 2.0, "the ground doesn't show through the structure: {on_bare:.1} over bare ground, {on_scorched:.1} over the scorch");
}

/// Build beams draw with their texture wrapping along the beam (FA's
/// `BeamPS` samples `ParticleSampler0Wrap`): the Seraphim engineer's
/// `ser_plasma04` beams scroll by `VShift` and, clamped, showed only the
/// texture's black edge row. Visual check:
/// `cargo run -- --no-ai --script dev/scenes/visual/build_beams.lua --camera 394,610,36`.
#[test]
fn build_beams_wrap_their_texture() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local brain = ArmyBrains[1]
            brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
            brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
            local sera = CreateUnitHPR('xsl0105', 'ARMY_1', 668, GetTerrainHeight(668, 374), 374, 0, 0, 0)
            IssueBuildMobile({sera}, {668.5, GetTerrainHeight(668.5, 366.5), 366.5}, 'xsb0101', {})
            "#,
        )
        .exec()
        .expect("spawn");
    if let Some(c) = h.world.camera_mut() {
        c.target = Vec3::new(668.0, 18.0, 370.0);
        c.zoom = 30.0;
    }
    h.ticks(60);
    h.settle();
    let beams = h.scene.stage.particles.beam_draws();
    let seraphim: Vec<_> = beams.iter().filter(|(t, _)| t.contains("ser_plasma04")).collect();
    assert!(!seraphim.is_empty(), "the Seraphim build beams draw: {beams:?}");
    assert!(seraphim.iter().all(|(_, wrap)| *wrap), "beam textures wrap along the beam: {beams:?}");
}

/// The sim's ground decals reach the map: resource deposit markers (FAF's
/// `CreateSplat`, the hydrocarbon one a 16-bit DDS) and a structure's tarmac
/// (`CreateDecal`, relative texture names) draw, with their textures loaded.
#[test]
fn script_decals_draw() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine.sim_lua().load("CreateUnitHPR('ueb0101', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0)").exec().expect("spawn");
    h.ticks(5);
    h.settle();
    let (map, script) = h.scene.map.decal_counts();
    assert!(map > 0, "the map's decals draw");
    assert!(script >= 3, "markers and the factory's albedo + normal tarmac draw: {script} runs");
    let failed: Vec<String> = h.scene.asset_failures().into_iter().filter(|f| f.starts_with("decal")).collect();
    assert!(failed.is_empty(), "decal textures load: {failed:?}");
}

/// Zoomed out past `UI_ResourceLODCutoff` the map's deposits draw as
/// moho's resource icons (their textures load); zoomed in they don't.
#[test]
fn deposits_draw_resource_icons_zoomed_out() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(3);
    h.settle();
    let deposits = h.world.deposits.clone().expect("the map's deposits reach the scene");
    let &(_, x, z, _) = deposits.first().expect("a deposit");
    let at = Vec3::new(x as f32, 0.0, z as f32);
    let view = |h: &mut Harness, distance: f32| {
        let c = h.world.camera_mut().expect("camera");
        c.target = at;
        c.zoom = distance;
        h.frame();
        h.scene.deposit_icons(0)
    };
    assert!(view(&mut h, 400.0) > 0, "icons zoomed out");
    assert_eq!(view(&mut h, 20.0), 0, "no icons zoomed in");
    let failed: Vec<String> = h.scene.asset_failures().into_iter().filter(|f| f.starts_with("resource icon")).collect();
    assert!(failed.is_empty(), "icon textures load: {failed:?}");
}

/// The camera's LOD scale (`cam_SetLOD`, FAF's Level Of Detail option)
/// moves every LOD cutoff: a unit at the camera's target is `distance` deep,
/// so its mesh stops drawing at its last cutoff over the scale, and
/// the resource icons' cutoff moves with it.
#[test]
fn lod_scale_moves_the_cutoffs() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let id: String = h.engine.sim_lua().load("return CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0):GetEntityId()").eval().expect("spawn");
    let id: u32 = id.parse().expect("entity id");
    h.scene.record_all_bones = false;
    h.ticks(3);
    h.settle();
    let at = Vec3::new(400.0, h.scene.height_at(400.0, 580.0), 580.0);
    let drawn = |h: &mut Harness, distance: f32, scale: f32| {
        let c = h.world.camera_mut().expect("camera");
        (c.target, c.zoom, c.lod_scale) = (at, distance, scale);
        h.frame();
        h.scene.mesh_drawn(id)
    };
    // Its last cutoff at scale 1, to 5 units.
    let cutoff = (1..200).map(|i| i as f32 * 5.0).find(|&d| !drawn(&mut h, d, 1.0)).expect("the mesh stops drawing zoomed out") - 5.0;
    assert!(cutoff > 50.0, "drawn up close (last cutoff {cutoff})");
    let past = cutoff + 10.0;
    assert!(!drawn(&mut h, past, 1.0), "past the cutoff at Medium (1.0)");
    assert!(drawn(&mut h, past, 0.7), "drawn there at FAF's Extreme (0.7)");
    assert!(drawn(&mut h, cutoff, 1.0), "drawn at the cutoff at Medium");
    assert!(!drawn(&mut h, cutoff, 1.2), "not drawn there at Low (1.2)");
    // Resource icons show past `UI_ResourceLODCutoff` (75) on the same plane.
    let deposits = h.world.deposits.clone().expect("the map's deposits reach the scene");
    let &(_, x, z, _) = deposits.first().expect("a deposit");
    let icons = |h: &mut Harness, distance: f32, scale: f32| {
        let c = h.world.camera_mut().expect("camera");
        (c.target, c.zoom, c.lod_scale) = (Vec3::new(x as f32, 0.0, z as f32), distance, scale);
        h.frame();
        h.scene.deposit_icons(0)
    };
    assert!(icons(&mut h, 120.0, 1.0) > 0, "icons at 120 at Medium");
    assert_eq!(icons(&mut h, 120.0, 0.5), 0, "none at 120 with a scale of 0.5");
}

/// Trees sway: a pine whose LOD0 is `UndulatingNormalMappedAlpha`
/// draws with FA's undulating vertex motion, by game time: the same frame
/// again with no tick between is the same picture (it stands still while
/// the game is paused, whatever the wall clock does), and ticks later the
/// tree has moved.
#[test]
fn trees_sway_with_game_time() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine.sim_lua().load("CreateProp({400, GetTerrainHeight(400, 580), 580}, '/env/evergreen/props/trees/pine06_big_prop.bp')").exec().expect("spawn");
    h.scene.record_all_bones = false;
    h.ticks(3);
    h.settle();
    h.scene.frozen_time = Some(0.0);
    let at = Vec3::new(400.0, h.scene.height_at(400.0, 580.0) + 4.0, 580.0);
    let c = h.world.camera_mut().expect("camera");
    (c.target, c.zoom) = (at, 14.0);
    h.frame();
    let motion = crate::engine::api::mesh::VertexMotion::Undulating as u32;
    assert!(h.scene.drawn_vertex_motions().contains(&motion), "the pine draws with the undulating motion");
    let (w, ht) = (320, 240);
    let changed = |a: &[u8], b: &[u8]| a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| (0..3).any(|i| (p[i] as i32 - q[i] as i32).abs() > 16)).count();
    render_rgba(&mut h, w, ht);
    let first = render_rgba(&mut h, w, ht);
    std::thread::sleep(Duration::from_millis(50));
    let again = render_rgba(&mut h, w, ht);
    // (A stray pixel or two can differ between identical frames.)
    let still = changed(&first, &again);
    assert!(still <= 4, "no game time passed: the tree stands still ({still} pixels changed)");
    // sin^2 at 0.05 a tick: a sway every 63 ticks; 16 and 32 ticks on, at
    // least one of the two is well away from the first pose.
    let mut moved = 0;
    for _ in 0..2 {
        h.ticks(16);
        moved = moved.max(changed(&first, &render_rgba(&mut h, w, ht)));
    }
    assert!(moved > 100, "the tree moved with game time ({moved} pixels changed)");
}

/// Icon-less units draw nothing past their last LOD cutoff: the
/// Cybran build drones (`ura0001o`, one LOD, no strategic icon) drew small
/// army-coloured marker cubes zoomed out, reading as boxes; moho draws
/// nothing past the last cutoff. The mesh still draws zoomed in. (The
/// blueprint's cutoff of 215 is recalculated by FAF's `blueprints-lod.lua`
/// from the unit's size, so the drone's real single cutoff is about 56; the
/// distances sit either side of it.)
#[test]
fn iconless_units_draw_nothing_past_their_last_cutoff() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let id: String = h.engine.sim_lua().load("return CreateUnitHPR('ura0001o', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0):GetEntityId()").eval().expect("spawn");
    let id: u32 = id.parse().expect("entity id");
    h.scene.record_all_bones = false;
    h.ticks(3);
    h.settle();
    h.ticks(1);
    let at = Vec3::new(400.0, h.scene.height_at(400.0, 580.0), 580.0);
    let drawn = |h: &mut Harness, distance: f32| {
        let c = h.world.camera_mut().expect("camera");
        (c.target, c.zoom, c.lod_scale) = (at, distance, 1.0);
        h.frame();
        h.scene.mesh_drawn(id)
    };
    assert!(drawn(&mut h, 50.0), "the drone's mesh draws zoomed in");
    assert!(!drawn(&mut h, 300.0), "past its last cutoff the mesh doesn't draw");
    // The marker the LOD pass used to stand in for icon-less units drew
    // through the box pipeline; nothing may (the count is the box pass's
    // instance uploads).
    assert_eq!(h.scene.drawn_box_instances(), 0, "nothing stands in past the cutoff");
    assert!(!h.scene.stage.boxed.contains(&id), "no box stands in past the cutoff");
    let failures = h.scene.asset_failures();
    assert!(failures.is_empty(), "{} assets failed to load:\n{}", failures.len(), failures.join("\n"));
}

/// FAF's Level Of Detail option's console commands set the world camera's
/// LOD scale: `SC_CameraScaleLOD` by level (Extreme's 2.1 reads as High),
/// `cam_SetLOD` by camera name, `cam_DefaultLOD` only for cameras made after.
#[test]
fn level_of_detail_commands_set_the_lod_scale() {
    use crate::render::console::lod;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(1);
    h.settle();
    let scale = |h: &Harness| h.world.camera().expect("camera").lod_scale;
    assert_eq!(scale(&h), 1.0, "Medium by default");
    for (level, want) in [("0", 1.2), ("1", 1.0), ("2", 0.9), ("2.1", 0.9)] {
        lod(&mut h.world, "sc_camerascalelod", level);
        assert_eq!((scale(&h), h.world.default_lod), (want, want), "SC_CameraScaleLOD {level}");
    }
    lod(&mut h.world, "sc_camerascalelod", "3");
    assert_eq!(scale(&h), 0.9, "no level 3");
    lod(&mut h.world, "cam_setlod", "WorldCamera 0.70");
    assert_eq!(scale(&h), 0.7, "FAF's Extreme");
    lod(&mut h.world, "cam_setlod", "WorldCamera2 0.5");
    assert_eq!(scale(&h), 0.7, "another camera's name changes nothing here");
    lod(&mut h.world, "cam_defaultlod", "1.2");
    assert_eq!((scale(&h), h.world.default_lod), (0.7, 1.2), "the default is for new cameras");
}

/// Tread marks and footprints: FAF lays a moving tank's tread marks
/// (`TreadComponent`, once the engine reports the terrain type it starts on)
/// and a walking ACU's footprints (`OnAnimCollision`) as timed splats, which
/// reach the terrain renderer.
#[test]
fn tanks_and_commanders_mark_the_ground() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let spawn = "
        local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0)
        local acu = CreateUnitHPR('uel0001', 'ARMY_1', 420, GetTerrainHeight(420, 580), 580, 0, 0, 0)
        IssueMove({ tank }, Vector(400, GetTerrainHeight(400, 620), 620))
        IssueMove({ acu }, Vector(420, GetTerrainHeight(420, 620), 620))";
    h.engine.sim_lua().load(spawn).exec().expect("spawn");
    h.ticks(3);
    h.settle();
    h.ticks(60);
    h.settle();
    h.ticks(10);
    let textures = h.scene.map.tread_textures();
    assert!(textures.iter().any(|t| t.contains("tank_treads")), "tank tread marks: {textures:?}");
    assert!(textures.iter().any(|t| t.contains("commanderprint")), "ACU footprints: {textures:?}");
}

/// A one-off `CreateSplat` whose texture hasn't been used yet still lands
/// in a tread ring: its own request starts the texture's load, and it waits
/// for it instead of being dropped.
#[test]
fn a_first_splat_waits_for_its_texture() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(1);
    h.settle();
    assert_eq!(h.scene.map.tread_count("scorch_001"), 0, "no scorch splat yet");
    h.engine.sim_lua().load("CreateSplat({400, GetSurfaceHeight(400, 580), 580}, 0, 'scorch_001_albedo', 2, 2, 200, 60, -1)").exec().expect("splat");
    h.ticks(1);
    h.settle();
    h.ticks(3);
    assert_eq!(h.scene.map.tread_count("scorch_001"), 1, "the first splat of a new texture is laid");
}

/// The command graph: the selected units' queued orders draw as FAF's
/// styles say. Two tanks sharing a move and a patrol share their two
/// waypoints; a stop (no waypoint texture in FAF's params) draws none; the
/// params and every texture load.
#[test]
fn selected_orders_draw_the_command_graph() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            "local a = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0)
             local b = CreateUnitHPR('uel0201', 'ARMY_1', 404, GetTerrainHeight(404, 580), 580, 0, 0, 0)
             IssueMove({ a, b }, Vector(420, GetTerrainHeight(420, 640), 640))
             IssuePatrol({ a, b }, Vector(440, GetTerrainHeight(440, 600), 600))
             IssuePatrol({ a, b }, Vector(450, GetTerrainHeight(450, 580), 580))
             -- (Out of build range of both sites: 21 from the factory's, which
             -- it reaches from 14, its 5 + footprint 1 + skirt 8.)
             local e = CreateUnitHPR('uel0105', 'ARMY_1', 372, GetTerrainHeight(372, 590), 590, 0, 0, 0)
             IssueBuildMobile({ e }, Vector(390.5, GetTerrainHeight(390.5, 600.5), 600.5), 'ueb0101', {})
             IssueBuildMobile({ e }, Vector(400, GetTerrainHeight(400, 610), 610), 'ueb1101', {})
             return { tonumber(a:GetEntityId()), tonumber(b:GetEntityId()), tonumber(e:GetEntityId()) }",
        )
        .eval()
        .expect("spawn");
    let mut selection: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    selection.sort_unstable();
    h.input.selection = selection.clone();
    h.request.lock().unwrap().selection = selection;
    h.ticks(3);
    h.settle();
    h.ticks(2);
    let kinds: Vec<u8> = h.scene.graph(0).expect("view prepared").waypoints.iter().map(|w| w.kind).collect();
    use crate::engine::sim::command::CommandKind as K;
    assert_eq!(kinds.iter().filter(|&&k| matches!(K::from_u8(k), Some(K::Move | K::FormMove | K::Patrol | K::FormPatrol))).count(), 3, "one waypoint per shared order: {kinds:?}");
    assert_eq!(kinds.iter().filter(|&&k| K::from_u8(k) == Some(K::BuildMobile)).count(), 2, "{kinds:?}");
    // Build sites: the factory's 8x8 skirt (current), the generator's (queued).
    let mut sites = h.scene.graph(0).expect("view prepared").sites.clone();
    sites.sort_by_key(|s| std::cmp::Reverse(s.1));
    assert_eq!(sites.len(), 2, "{sites:?}");
    let ([x0, z0, x1, z1], current) = sites[0];
    assert!(current && (x1 - x0 - 8.0).abs() < 1e-3 && (z1 - z0 - 8.0).abs() < 1e-3, "factory skirt {:?}", sites[0]);
    assert!(!sites[1].1, "the generator is queued");
    // Ghosts for builds with no structure yet: the generator (and the
    // factory until its construction starts).
    assert_eq!(h.scene.queued_ghosts(), 2, "the factory and generator sites show ghosts");
    // ETAs grow down the queue.
    let etas: Vec<f32> = h.scene.graph(0).expect("view prepared").waypoints.iter().filter(|w| K::from_u8(w.kind) == Some(K::BuildMobile)).map(|w| w.eta).collect();
    assert!(etas.iter().all(|&t| t > 0.0), "{etas:?}");
    // The patrol loop closes: a line from the last patrol waypoint back to
    // the first (commands number in issue order).
    assert_eq!(h.scene.graph(0).expect("view prepared").brackets, 3, "a selection bracket per selected unit");
    let lines = &h.scene.graph(0).expect("view prepared").lines;
    assert!(lines.iter().any(|&(from, to)| from.is_some_and(|f| f > to)), "patrol loop line: {lines:?}");
    let failed: Vec<String> = h.scene.asset_failures().into_iter().filter(|f| f.starts_with("command graph")).collect();
    assert!(failed.is_empty(), "command graph assets load: {failed:?}");
}

/// Holding Shift shows existing orders (issue #9): the command graph draws
/// every unit of the player's army with orders, not only the selection's,
/// and never another army's.
#[test]
fn shift_shows_the_armys_orders() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            "local a = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0)
             local b = CreateUnitHPR('uel0201', 'ARMY_1', 430, GetTerrainHeight(430, 580), 580, 0, 0, 0)
             local c = CreateUnitHPR('uel0201', 'ARMY_2', 460, GetTerrainHeight(460, 580), 580, 0, 0, 0)
             IssueMove({ a }, Vector(400, GetTerrainHeight(400, 640), 640))
             IssueMove({ b }, Vector(430, GetTerrainHeight(430, 640), 640))
             IssueMove({ c }, Vector(460, GetTerrainHeight(460, 640), 640))
             return { tonumber(a:GetEntityId()), tonumber(b:GetEntityId()), tonumber(c:GetEntityId()) }",
        )
        .eval()
        .expect("spawn");
    let ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    h.input.selection = vec![a];
    {
        let mut r = h.request.lock().unwrap();
        r.selection = vec![a];
        r.view_army = Some(1);
    }
    h.ticks(2);
    let units = |h: &Harness| h.world.debug().map(|d| d.orders.iter().map(|(u, _)| *u).collect::<Vec<u32>>()).unwrap_or_default();
    assert_eq!(units(&h), vec![a], "without Shift: the selection's orders only");
    assert_eq!(h.scene.graph(0).expect("view prepared").waypoints.len(), 1);
    h.request.lock().unwrap().all_orders = true;
    h.ticks(2);
    assert_eq!(units(&h), vec![a, b], "Shift held: the army's orders, not the other army's ({c})");
    assert_eq!(h.scene.graph(0).expect("view prepared").waypoints.len(), 2, "both moves' waypoints draw");
    h.request.lock().unwrap().all_orders = false;
    h.ticks(2);
    assert_eq!(units(&h), vec![a], "Shift released: back to the selection");
    // An enemy selected: no command graph, and no queue for the
    // interface; observers see it.
    assert!(h.engine.sim_lua().load("return IsEnemy(1, 2)").eval::<bool>().unwrap());
    h.input.selection = vec![c];
    {
        let mut r = h.request.lock().unwrap();
        r.selection = vec![c];
        r.selected = Some(c);
    }
    h.ticks(2);
    assert!(units(&h).is_empty(), "the enemy's orders are hidden");
    let sel = h.world.debug().and_then(|d| d.selected.clone()).expect("inspected");
    assert!(sel.path.is_empty() && sel.orders.is_empty(), "and its path and orders in the inspector");
    let queue = |h: &Harness| h.world.debug().and_then(|d| d.units.iter().find(|u| u.id == c).map(|u| u.commands.len()));
    assert_eq!(queue(&h), Some(0), "its details come without its queue");
    h.request.lock().unwrap().view_army = None;
    h.ticks(2);
    assert_eq!(units(&h), vec![c], "an observer sees every army's orders");
    assert_eq!(queue(&h), Some(1));
}

/// Snapshots with a selected unit carry its build restrictions (the
/// interface's locked tech tabs) without re-borrowing the entity world: an
/// ACU's T2/T3 come through restricted.
#[test]
fn selected_unit_details_carry_build_restrictions() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let id: u32 = h.engine.sim_lua().load("local u = CreateUnitHPR('uel0001', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0) return tonumber(u:GetEntityId())").eval().expect("spawn");
    h.request.lock().unwrap().selection = vec![id];
    h.ticks(3);
    let detail = h.world.debug().and_then(|d| d.units.iter().find(|u| u.id == id).cloned()).expect("the ACU's details arrive");
    assert!(detail.restricted.iter().any(|b| b == "ueb1201"), "T2 power is restricted for a fresh ACU: {:?}", detail.restricted);
}

/// Props reach the renderer once, then as changes: one created mid-game
/// appears in the drawn entities and disappears when destroyed, and the
/// map's props are all there throughout.
#[test]
fn prop_changes_reach_the_scene() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(2);
    let props = |h: &Harness| h.scene.frame_entities.iter().filter(|e| e.kind == crate::engine::snapshot::ViewKind::Prop).count();
    let before = props(&h);
    assert!(before > 1000, "the map's props are drawn: {before}");
    let id: u32 = h
        .engine
        .sim_lua()
        .load("local p = CreatePropHPR('/env/common/props/hydrocarbonDeposit01_prop.bp', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0) _G.TestProp = p return tonumber(p:GetEntityId())")
        .eval()
        .expect("prop");
    h.ticks(2);
    assert!(h.scene.frame_entities.iter().any(|e| e.id == id), "the new prop is drawn");
    assert_eq!(props(&h), before + 1);
    h.engine.sim_lua().load("TestProp:Destroy()").exec().expect("destroy");
    h.ticks(2);
    assert!(!h.scene.frame_entities.iter().any(|e| e.id == id), "the destroyed prop is gone");
    assert_eq!(props(&h), before);
}


/// A weapon's overlay ring shows its `EffectiveRadius` when it has one: the
/// Fatboy's anti-air guns reach 55 but show 42.
#[test]
fn range_overlay_shows_effective_radius() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine.sim_lua().load("CreateUnitHPR('uel0401', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0)").exec().expect("spawn");
    h.ticks(2);
    let bps = h.world.blueprints.clone().unwrap();
    let bp = bps.iter().position(|b| b == "uel0401").unwrap();
    let ranges = h.world.blueprint_ranges.clone().expect("ranges sent");
    let aa = crate::engine::snapshot::RANGE_KINDS.iter().position(|k| *k == "ANTIAIR").unwrap() as u8;
    let radii: Vec<f32> = ranges[bp].iter().filter(|r| r.0 == aa).map(|r| r.2).collect();
    assert_eq!(radii, [42.0, 42.0]);
}

/// The strategic range overlays: with a direct fire filter on, a point
/// defence's range shows as a ring in the filter's colour on the ground at
/// its weapon's radius, and nothing changes without the filter.
#[test]
fn range_overlay_rings() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine.sim_lua().load("CreateUnitHPR('ueb2101', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0)").exec().expect("spawn");
    h.ticks(2);
    let bps = h.world.blueprints.clone().unwrap();
    let bp = bps.iter().position(|b| b == "ueb2101").unwrap();
    let ranges = h.world.blueprint_ranges.clone().expect("ranges sent");
    let direct = crate::engine::snapshot::RANGE_KINDS.iter().position(|k| *k == "DIRECTFIRE").unwrap() as u8;
    let radius = ranges[bp].iter().find(|r| r.0 == direct).expect("its gun's range").2;
    h.scene.record_all_bones = false;
    // The ring's own colour, not anti-aliasing's blend of its edge.
    h.settings.smaa = false;
    let ground = h.scene.height_at(668.0, 366.0);
    if let Some(c) = h.world.camera_mut() {
        c.target = Vec3::new(668.0, ground, 366.0);
        c.zoom = 265.0;
    }
    h.settle();
    let (w, ht) = (640, 360);
    let before = render_rgba(&mut h, w, ht);
    let red = [1.0, 0.17, 0.17, 0.0];
    h.input.range_filters = std::sync::Arc::new(vec![rts_core_engine::ui::OverlayFilter {
        name: "DirectFire".into(),
        colors: [red; 3],
        inner: [0.02, 2.0],
        outer: [0.04, 4.0],
        kinds: 1 << direct,
        members: (0..bps.len()).map(|i| i == bp).collect(),
    }]);
    let after = render_rgba(&mut h, w, ht);
    // A point on the ring, just inside its edge (the line's inside).
    let at = Vec3::new(668.0 + radius - 0.3, h.scene.height_at(668.0 + radius - 0.3, 366.0), 366.0);
    let [x, y] = h.world.camera().unwrap().project(at, [w as f32, ht as f32]).expect("on screen");
    let px = |img: &[u8]| {
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    let (b, a) = (px(&before), px(&after));
    assert!(a[0] > 200 && a[1] < 120 && a[2] < 120, "ring pixel red: before {b:?}, after {a:?}");
    // No tint inside the ring (the game leaves the ground as it is).
    let inside = Vec3::new(668.0 + radius * 0.5, h.scene.height_at(668.0 + radius * 0.5, 366.0), 366.0);
    let [ix, iy] = h.world.camera().unwrap().project(inside, [w as f32, ht as f32]).unwrap();
    let at = |img: &[u8]| {
        let i = ((iy as u32 * w + ix as u32) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    assert_eq!(at(&before), at(&after), "ground inside the ring unchanged");
    // The placement ghost shows its ranges with no overlay on, each kind in
    // its own overlay's colour.
    h.input.range_filters = std::sync::Arc::new(Vec::new());
    let mut styles = vec![None; crate::engine::snapshot::RANGE_KINDS.len()];
    styles[direct as usize] = Some(rts_core_engine::ui::RangeStyle { colors: [red; 3], inner: [0.02, 2.0], outer: [0.04, 4.0] });
    h.input.range_styles = std::sync::Arc::new(styles);
    let ghost_at = Vec3::new(668.0, h.scene.height_at(668.0, 366.0), 366.0);
    h.input.ghost = Some(crate::render::scene::Ghost { blueprint: bp as u32, position: ghost_at, color: [0.0, 0.85, 0.0], site: None });
    let ghosted = render_rgba(&mut h, w, ht);
    let g = px(&ghosted);
    assert!(g[0] > 200 && g[1] < 120 && g[2] < 120, "the ghost's ring: {g:?}");
    if let Ok(dir) = std::env::var("RANGES_PNG") {
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(std::path::Path::new(&dir).join("ranges.png")).unwrap()), w, ht);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&after).unwrap();
    }
}

/// The placement ghost of a unit with `AI.ShowAssistRangeOnSelect` (a
/// Cybran engineering station) draws its `GuardScanRadius` too (moho's ghost
/// rings, MohoEngine.dll 0x103b7e50), in the misc overlay's colour; a T1
/// engineer's ghost (no flag) doesn't, and neither do selected stations.
#[test]
fn placement_ghost_shows_its_assist_range() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(2);
    let bps = h.world.blueprints.clone().unwrap();
    let index = |id: &str| bps.iter().position(|b| b == id).unwrap();
    let assist = h.world.blueprint_assist_range.clone().expect("assist ranges sent");
    let station = assist[index("xrb0104")];
    assert!(station > 0.0, "the station's GuardScanRadius");
    assert_eq!(assist[index("uel0105")], 0.0, "no ShowAssistRangeOnSelect");
    let misc = crate::engine::snapshot::RANGE_KINDS.iter().position(|k| *k == "MISC").unwrap();
    let yellow = [0.69, 0.57, 0.0, 0.0];
    let mut styles = vec![None; crate::engine::snapshot::RANGE_KINDS.len()];
    styles[misc] = Some(rts_core_engine::ui::RangeStyle { colors: [yellow, yellow, yellow], inner: [0.02, 2.0], outer: [0.04, 4.0] });
    h.input.range_styles = std::sync::Arc::new(styles);
    let rings = |h: &mut Harness| -> Vec<([f32; 4], f32)> { h.range_groups().iter().flat_map(|g| g.rings.iter().map(move |r| (g.color, r.radii[0]))).collect() };
    let at = Vec3::new(668.0, h.scene.height_at(668.0, 366.0), 366.0);
    h.input.ghost = Some(crate::render::scene::Ghost { blueprint: index("xrb0104") as u32, position: at, color: [0.0, 0.85, 0.0], site: None });
    assert!(rings(&mut h).contains(&(yellow, station)), "the station's ghost: its assist ring");
    h.input.ghost = Some(crate::render::scene::Ghost { blueprint: index("uel0105") as u32, position: at, color: [0.0, 0.85, 0.0], site: None });
    assert!(!rings(&mut h).iter().any(|r| r.1 == station), "an engineer's ghost: none");
}

/// A structure still being built draws no rings: a radar upgrading
/// shows only its own ring, not the next tier's, until the upgrade is done.
#[test]
fn unfinished_upgrade_shows_no_range_ring() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
            b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
            local r = CreateUnitHPR('ueb3101', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0)
            IssueUpgrade({ r }, 'ueb3201')
            "#,
        )
        .exec()
        .expect("spawn");
    h.ticks(20);
    let bps = h.world.blueprints.clone().unwrap();
    let ranges = h.world.blueprint_ranges.clone().expect("ranges sent");
    let radar = crate::engine::snapshot::RANGE_KINDS.iter().position(|k| *k == "RADAR").unwrap() as u8;
    let radius = |id: &str| ranges[bps.iter().position(|b| b == id).unwrap()].iter().find(|r| r.0 == radar).expect("a radar ring").2;
    let (t1, t2) = (radius("ueb3101"), radius("ueb3201"));
    assert!(t2 > t1);
    let building = |h: &Harness| h.scene.frame_entities.iter().any(|e| bps.get(e.blueprint as usize).is_some_and(|b| b == "ueb3201") && e.build < 1.0);
    assert!(building(&h), "the T2 radar is being built");
    h.input.range_filters = std::sync::Arc::new(vec![rts_core_engine::ui::OverlayFilter {
        name: "Radar".into(),
        colors: [[0.1, 0.4, 0.5, 0.0]; 3],
        inner: [1.0, 1.0],
        outer: [0.02, 2.0],
        kinds: 1 << radar,
        members: bps.iter().map(|b| b == "ueb3101" || b == "ueb3201").collect(),
    }]);
    let radii = |h: &mut Harness| -> Vec<f32> { h.range_groups().iter().flat_map(|g| g.rings.iter().map(|r| r.radii[0])).collect() };
    assert_eq!(radii(&mut h), vec![t1], "only the T1 radar's ring while the upgrade is built");
}

/// Selected units show their build range with no overlay on (moho's
/// `range_RenderSelected`): an engineer's ring at FAF's overlay radius, in
/// the Build Range overlay's selected colour; none once the option is off.
#[test]
fn selected_engineer_shows_its_build_range() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let id: u32 = h.engine.sim_lua().load("return tonumber(CreateUnitHPR('uel0105', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0):GetEntityId())").eval().expect("spawn");
    h.ticks(2);
    let bps = h.world.blueprints.clone().unwrap();
    let misc = crate::engine::snapshot::RANGE_KINDS.iter().position(|k| *k == "MISC").unwrap();
    let radius = h.world.blueprint_ranges.clone().unwrap()[bps.iter().position(|b| b == "uel0105").unwrap()].iter().find(|r| r.0 as usize == misc).expect("build range").2;
    let yellow = [0.74, 0.61, 0.0, 0.03];
    let mut styles = vec![None; crate::engine::snapshot::RANGE_KINDS.len()];
    styles[misc] = Some(rts_core_engine::ui::RangeStyle { colors: [[0.69, 0.57, 0.0, 0.0], yellow, [0.79, 0.65, 0.0, 0.06]], inner: [0.02, 2.0], outer: [0.04, 4.0] });
    h.input.range_styles = std::sync::Arc::new(styles);
    let rings = |h: &mut Harness| -> Vec<([f32; 4], f32)> { h.range_groups().iter().flat_map(|g| g.rings.iter().map(move |r| (g.color, r.radii[0]))).collect() };
    assert!(rings(&mut h).is_empty(), "nothing selected, no overlay: no rings");
    h.input.selection = vec![id];
    assert_eq!(rings(&mut h), vec![(yellow, radius)], "its build range in the selected colour");
    h.settings.range_selected = false;
    assert!(rings(&mut h).is_empty(), "range_RenderSelected off");
}

/// Selected structures show every ring they have with no overlay on:
/// a point defence's direct fire, artillery's indirect fire with its minimum
/// range, a radar's radar; each in its kind's selected colour.
#[test]
fn selected_units_show_all_their_rings() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            r#"
            local ids = {}
            for i, bp in { 'ueb2101', 'ueb2303', 'ueb3101' } do
                local x = 640 + i * 15
                table.insert(ids, tonumber(CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, 366), 366, 0, 0, 0):GetEntityId()))
            end
            return ids
            "#,
        )
        .eval()
        .expect("spawn");
    let ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    h.ticks(2);
    let bps = h.world.blueprints.clone().unwrap();
    let ranges = h.world.blueprint_ranges.clone().unwrap();
    let kind = |k: &str| crate::engine::snapshot::RANGE_KINDS.iter().position(|n| *n == k).unwrap();
    // Each kind its own selected colour (red channel = kind) to tell them apart.
    let styles: Vec<_> = (0..crate::engine::snapshot::RANGE_KINDS.len()).map(|k| Some(rts_core_engine::ui::RangeStyle { colors: [[0.0; 4], [k as f32, 1.0, 0.0, 0.0], [k as f32, 2.0, 0.0, 0.0]], inner: [0.02, 2.0], outer: [0.04, 4.0] })).collect();
    h.input.range_styles = std::sync::Arc::new(styles);
    h.input.selection = ids.clone();
    let mut got: Vec<(usize, f32, f32)> = h.range_groups().iter().flat_map(|g| g.rings.iter().map(move |r| (g.color[0] as usize, r.radii[2], r.radii[0]))).collect();
    got.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let expect = |bp: &str, k: &str| -> (usize, f32, f32) {
        let r = ranges[bps.iter().position(|b| b == bp).unwrap()].iter().find(|r| r.0 as usize == kind(k)).copied().expect("has that ring");
        (kind(k), r.1, r.2)
    };
    let (pd, arty, radar) = (expect("ueb2101", "DIRECTFIRE"), expect("ueb2303", "INDIRECTFIRE"), expect("ueb3101", "RADAR"));
    assert!(arty.1 > 0.0, "artillery has a minimum range");
    for want in [pd, arty, radar] {
        assert!(got.contains(&want), "{want:?} in {got:?}");
    }
    assert!(h.range_groups().iter().all(|g| g.color[1] == 1.0), "all in the selected colours");
}

/// A shield's strength reaches the frame: its generator's view
/// carries the ratio FAF's shield sets as it takes damage (the shield bar),
/// and units without shields carry none.
#[test]
fn shield_strength_reaches_the_frame() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            r#"
            local b = ArmyBrains[1]
            b:__engine_SetStorage('ENERGY', 1e6) b:GiveResource('ENERGY', 1e6)
            local gen = CreateUnitHPR('ueb4202', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0)
            local pgen = CreateUnitHPR('ueb1101', 'ARMY_1', 676, GetTerrainHeight(676, 366), 366, 0, 0, 0)
            return { tonumber(gen:GetEntityId()), tonumber(pgen:GetEntityId()) }
            "#,
        )
        .eval()
        .expect("spawn");
    let (generator, pgen): (u32, u32) = (ids.raw_get(1).unwrap(), ids.raw_get(2).unwrap());
    h.ticks(80);
    let shield = |h: &Harness, id: u32| h.scene.frame_entities.iter().find(|e| e.id == id).map(|e| e.shield).unwrap();
    assert_eq!(shield(&h, generator), 1.0, "a full shield");
    assert_eq!(shield(&h, pgen), -1.0, "no shield");
    h.engine.sim_lua().load(format!("local s = GetUnitById('{generator}').MyShield s:ApplyDamage(nil, s:GetMaxHealth() / 2, Vector(0, 0, 0), 'Normal')")).exec().expect("damage");
    h.ticks(2);
    let s = shield(&h, generator);
    assert!(s > 0.4 && s < 0.6, "half its strength: {s}");
}

/// A strategic icon drawn by the interface pass leaves the pixels under
/// its transparent texels untouched: the icon textures carry grey colour
/// under alpha 0 around the shape (the `_selected` frame's footprint), and
/// a faint box there would mean the atlas, the sampler or the blend lets
/// that colour through.
#[test]
fn strategic_icon_transparent_texels_draw_nothing() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.frame();
    let vfs = h.world.models_vfs().expect("vfs");
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let mut ui = super::ui::UiRenderer::new(&h.device, format);
    ui.set_vfs(vfs);
    let (w, ht) = (64u32, 64u32);
    // The 16x16 icon at twice its size (a retina interface scale), tinted.
    let icon = "/textures/ui/common/game/strategicicons/icon_land1_directfire_rest.dds";
    let item = rts_core_engine::ui::controls::DrawItem::Quad { rect: [16.0, 16.0, 48.0, 48.0], texture: Some(icon.into()), uv: [0.0, 0.0, 1.0, 1.0], color: [0.1, 0.3, 0.9, 1.0], tiled: false };
    ui.prepare(&h.device, &h.queue, &[item], [w as f32, ht as f32]);
    let target = h.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("icon test"),
        size: wgpu::Extent3d { width: w, height: ht, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let row = (w * 4).div_ceil(256) * 256;
    let buffer = h.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (row * ht) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = h.device.create_command_encoder(&Default::default());
    // A flat mid-green ground.
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.2, g: 0.3, b: 0.1, a: 1.0 }), store: wgpu::StoreOp::Store } })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    ui.draw(&mut enc, &view);
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(ht) } },
        wgpu::Extent3d { width: w, height: ht, depth_or_array_layers: 1 },
    );
    h.queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = h.device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range();
    let px = |x: u32, y: u32| -> [u8; 3] {
        let i = (y * row + x * 4) as usize;
        [data[i + 2], data[i + 1], data[i]]
    };
    let ground = px(4, 4);
    // Texel (0, 0): white under alpha 0. Texel (2, 1) and (12, 10): grey
    // under alpha 0 (the box's footprint). Pixel centres of those texels at
    // twice the size.
    for (tx, ty) in [(0, 0), (2, 1), (12, 10), (2, 11), (15, 15)] {
        let p = px(16 + tx * 2 + 1, 16 + ty * 2 + 1);
        assert_eq!(p, ground, "texel ({tx}, {ty}) tinted the ground");
    }
    // The diamond's centre is drawn (the tint over the texture's grey).
    assert_ne!(px(32, 28), ground, "the icon itself drew nothing");
}

/// Every mip level of `view`'s texture, read back from the GPU.
fn texture_levels(h: &Harness, view: &wgpu::TextureView) -> Vec<Vec<u8>> {
    let tex = view.texture();
    let block = tex.format().block_copy_size(None).expect("a copyable format");
    let (bw, bh) = tex.format().block_dimensions();
    (0..tex.mip_level_count())
        .map(|l| {
            let size = tex.size().mip_level_size(l, wgpu::TextureDimension::D2);
            let (cols, rows) = (size.width.div_ceil(bw), size.height.div_ceil(bh));
            let row = (cols * block).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let buffer = h.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: u64::from(row * rows), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            let mut enc = h.device.create_command_encoder(&Default::default());
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture: tex, mip_level: l, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(rows) } },
                wgpu::Extent3d { width: cols * bw, height: rows * bh, depth_or_array_layers: 1 },
            );
            h.queue.submit([enc.finish()]);
            let slice = buffer.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            let _ = h.device.poll(wgpu::PollType::wait_indefinitely());
            let data = slice.get_mapped_range();
            (0..rows).flat_map(|y| data[(y * row) as usize..(y * row + cols * block) as usize].to_vec()).collect()
        })
        .collect()
}

/// A texture streamed from its file into GPU memory (`texture_load`) holds
/// exactly what the old path uploaded (read whole, `dds::read`,
/// `write_texture`): every mip level, byte for byte, for unit textures from
/// the game's archive (DXT1 / DXT5, small mips through the padded path)
/// and a decal and a 32-bit BGRA unit texture from FAF's checkout (the
/// latter compared after swapping red and blue back).
#[test]
fn streamed_textures_match_the_old_upload() {
    use rts_core_engine::engine::assets::dds;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(1);
    h.frame();
    let vfs = h.scene.vfs().expect("vfs").clone();
    let paths = [
        "/units/uel0201/uel0201_albedo.dds",
        "/units/uel0201/uel0201_specteam.dds",
        "/units/uel0201/uel0201_normalsts.dds",
        "/units/xsl0401/xsl0401_albedo.dds",
        "/env/common/decals/scorch_001_albedo.dds",
        // 32-bit BGRA (FAF): streamed as Bgra8, decoded to Rgba8 the old way.
        "/units/dra0202/dra0202_specteam.dds",
    ];
    let mut checked = 0;
    for path in paths {
        let Ok(bytes) = vfs.read().unwrap().read(path) else { continue };
        let old = super::models::upload_texture(&h.device, &h.queue, &dds::read(&bytes, true).unwrap(), true, "old").expect("old upload");
        let new = super::texture_load::load(&h.device, &h.queue, &vfs, path, true, true, "new").expect("streamed upload");
        let mut new_levels = texture_levels(&h, &new);
        if path.ends_with("dra0202_specteam.dds") {
            assert_eq!(new.texture().format(), wgpu::TextureFormat::Bgra8UnormSrgb, "{path} isn't streamed");
            new_levels.iter_mut().flat_map(|l| l.chunks_exact_mut(4)).for_each(|p| p.swap(0, 2));
            assert_eq!(old.texture().format(), wgpu::TextureFormat::Rgba8UnormSrgb, "{path}");
        } else {
            assert_eq!(old.texture().format(), new.texture().format(), "{path}");
        }
        assert_eq!(old.texture().mip_level_count(), new.texture().mip_level_count(), "{path}");
        assert!(texture_levels(&h, &old) == new_levels, "{path}: levels differ");
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} of the test textures were found");
}

/// A texture stored without mips gets its levels generated, and a second
/// load streams them from the mip cache: the same levels on the GPU, with
/// nothing generated again.
#[test]
fn cached_mips_upload_as_generated() {
    use rts_core_engine::engine::assets::mip_cache::MipCache;
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(1);
    h.frame();
    let vfs = h.scene.vfs().expect("vfs").clone();
    let dir = std::env::temp_dir().join(format!("rts-mip-cache-render-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = MipCache::new(dir.clone());
    // (Stored as one level in the base game.)
    let mut checked = 0;
    for path in ["/units/xsa0202/xsa0202_albedo.dds", "/env/crystalline/props/rocks/cryscrystal01_albedo.dds"] {
        let vfs = vfs.read().unwrap();
        if !vfs.contains(path) {
            continue;
        }
        let load = |h: &Harness| {
            let mut encoder = h.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let view = super::texture_load::load_with_cache(&h.device, &h.queue, &mut encoder, &vfs, path, true, true, "test", Some(&cache)).expect("load");
            h.queue.submit([encoder.finish()]);
            view
        };
        let generated = load(&h);
        let entries = std::fs::read_dir(&dir).map_or(0, |d| d.count());
        assert_eq!(entries, checked + 1, "{path}: generated levels cached");
        let cached = load(&h);
        assert!(generated.texture().mip_level_count() > 1, "{path}: levels generated");
        assert_eq!(generated.texture().mip_level_count(), cached.texture().mip_level_count(), "{path}");
        assert_eq!(generated.texture().format(), cached.texture().format(), "{path}");
        assert!(texture_levels(&h, &generated) == texture_levels(&h, &cached), "{path}: cached levels differ");
        checked += 1;
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(checked >= 1, "none of the test textures were found");
}

/// A texture through the shared cache is loaded once: every load of its
/// path (and colour space) gets the same texture.
#[test]
fn shared_textures_load_once() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(1);
    h.frame();
    let vfs = h.scene.vfs().expect("vfs").clone();
    let vfs = vfs.read().unwrap();
    let cache = super::texture_load::Cache::default();
    let path = "/units/uel0201/uel0201_albedo.dds";
    let a = cache.load(&h.device, &h.queue, &vfs, path, true, true, "a").expect("loads");
    let b = cache.load(&h.device, &h.queue, &vfs, path, true, true, "b").expect("loads");
    let linear = cache.load(&h.device, &h.queue, &vfs, path, false, true, "c").expect("loads");
    assert!(a.texture() == b.texture());
    assert!(a.texture() != linear.texture());
    assert!(cache.load(&h.device, &h.queue, &vfs, "/no/such.dds", true, true, "d").is_none());
}

/// A terrain change (a structure flattening its footprint) applied in
/// place, only the changed rows rewritten, leaves the same buffers, tile
/// bounds and height range as rebuilding the terrain whole.
#[test]
fn terrain_updates_in_place_like_a_full_upload() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(1);
    h.frame();
    let old = h.scene.terrain_view.clone().expect("terrain");
    let mut t = rts_core_engine::engine::snapshot::TerrainView { width: old.width, height: old.height, heights: old.heights.clone(), water: old.water, map: old.map.clone() };
    let stride = t.width + 1;
    // A raised rect, one in the same rows far off (two spans a row), one
    // on the far edge (clamped sample reads), one dug.
    for (x0, z0, x1, z1, y) in [(37, 41, 52, 49, 300.0), (400, 43, 409, 52, 120.0), (t.width - 3, t.height - 6, t.width, t.height, 1.0), (0, 0, 2, 0, -5.0)] {
        for z in z0..=z1 {
            for x in x0..=x1 {
                t.heights[(z * stride + x) as usize] = y;
            }
        }
    }
    let t = std::sync::Arc::new(t);
    let (updated, vertices, heights, bounds, range) = h.scene.apply_terrain(&h.device, &h.queue, t.clone(), true);
    assert!(updated, "not applied in place");
    let (_, full_vertices, full_heights, full_bounds, full_range) = h.scene.apply_terrain(&h.device, &h.queue, t, false);
    assert!(heights == full_heights, "heights differ");
    assert!(vertices == full_vertices, "vertices differ");
    assert_eq!(bounds, full_bounds);
    assert_eq!(range, full_range);
}

/// A terrain change arriving while the last is still being worked out off
/// the render thread waits for it, then applies against the result: the
/// terrain ends up as the newest heightfield, as if uploaded whole.
#[test]
fn terrain_changes_queued_behind_one_another_end_at_the_newest() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.ticks(1);
    h.frame();
    let base = h.scene.terrain_view.clone().expect("terrain");
    let flatten = |from: &rts_core_engine::engine::snapshot::TerrainView, x0: u32, z0: u32, y: f32| {
        let mut t = rts_core_engine::engine::snapshot::TerrainView { width: from.width, height: from.height, heights: from.heights.clone(), water: from.water, map: from.map.clone() };
        for z in z0..z0 + 8 {
            for x in x0..x0 + 8 {
                t.heights[(z * (t.width + 1) + x) as usize] = y;
            }
        }
        std::sync::Arc::new(t)
    };
    let first = flatten(&base, 20, 30, 200.0);
    let second = flatten(&first, 300, 400, 150.0);
    let third = flatten(&second, 21, 31, 50.0);
    h.scene.queue_terrain(&h.device, &h.queue, first);
    h.scene.queue_terrain(&h.device, &h.queue, second);
    h.scene.queue_terrain(&h.device, &h.queue, third.clone());
    let (_, vertices, heights, bounds, range) = h.scene.apply_terrain(&h.device, &h.queue, third.clone(), true);
    let (_, full_vertices, full_heights, full_bounds, full_range) = h.scene.apply_terrain(&h.device, &h.queue, third, false);
    assert!(heights == full_heights, "heights differ");
    assert!(vertices == full_vertices, "vertices differ");
    assert_eq!(bounds, full_bounds);
    assert_eq!(range, full_range);
}

/// A structure the view army scouted but can't see now keeps its last-seen
/// look (the mesh at rest pose, darkened by the fog at its spot, no bar or
/// ranges) and its strategic icon stays as the greyed blip; seen again, all
/// of it comes back live.
#[test]
fn scouted_structure_keeps_its_last_seen_look_and_greyed_icon() {
    use super::scene::{IconKind, shows_bar};
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            r#"
            local a = CreateUnitHPR('uel0001', 'ARMY_1', 668, GetTerrainHeight(668, 366), 366, 0, 0, 0)
            local b = CreateUnitHPR('ueb1101', 'ARMY_2', 674, GetTerrainHeight(674, 388), 388, 0, 0, 0)
            acu_harness_unit = a
            return { tonumber(a:GetEntityId()), tonumber(b:GetEntityId()) }
            "#,
        )
        .eval()
        .expect("spawn");
    let ids: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    let factory = ids[1];
    h.request.lock().unwrap().view_army = Some(1);
    h.ticks(3);
    h.settle();
    let at = |h: &Harness| h.scene.frame_entities.iter().find(|e| e.id == factory).copied().expect("the factory is drawn");
    let index = |h: &Harness| h.scene.frame_entities.iter().position(|e| e.id == factory).expect("the factory is drawn");
    let inside = |h: &mut Harness, distance: f32| {
        let c = h.world.camera_mut().expect("camera");
        c.target = Vec3::new(674.0, 0.0, 388.0);
        c.zoom = distance;
        h.frame();
    };
    inside(&mut h, 400.0);
    let seen = at(&h);
    assert_eq!(seen.sighting, crate::engine::snapshot::Sighting::Seen, "in the ACU's vision");
    assert!(shows_bar(&seen), "in sight, a bar");
    assert!(h.scene.mesh_drawn(factory), "in sight, the mesh draws");
    assert_eq!(h.scene.icons.iter().flatten().filter(|i| i.0 == factory).map(|i| i.4).collect::<Vec<_>>(), vec![IconKind::Full], "in sight, the strategic icon in the army's colour");
    // The ACU walks away; past its vision the factory becomes remembered.
    h.engine.sim_lua().load("IssueMove({ acu_harness_unit }, Vector(868, GetTerrainHeight(868, 366), 366))").exec().expect("move");
    let mut remembered = None;
    for _ in 0..300 {
        h.engine.tick().expect("tick");
        h.frame();
        let s = h.scene.frame_entities.iter().find(|e| e.id == factory).map(|e| e.sighting);
        if matches!(s, Some(crate::engine::snapshot::Sighting::Remembered)) {
            remembered = Some(true);
            break;
        }
        if s.is_none() {
            remembered = Some(false);
            break;
        }
    }
    assert_eq!(remembered, Some(true), "the factory went out of vision and stayed");
    let out = at(&h);
    assert_eq!(out.army, 2, "shown as the owner's");
    assert_eq!((out.pose_start, out.pose_len), (0, 0), "the last-seen rest pose");
    assert!(!shows_bar(&out), "out of sight, no bar");
    assert!(!h.scene.bars.contains(&(index(&h) as u32)), "not among the posed bars");
    // Zoomed in: the building remains as last seen (the fog's factor greys
    // it); zoomed out: its icon, greyed. Never an army colour while it's
    // out of sight.
    inside(&mut h, 30.0);
    assert!(h.scene.mesh_drawn(factory), "the last-seen building still draws");
    assert!(h.scene.icons.iter().flatten().all(|i| i.0 != factory || i.4 == IconKind::Remembered), "never a full icon while it's out of sight");
    inside(&mut h, 400.0);
    assert!(h.scene.mesh_drawn(factory), "the last-seen building still draws, zoomed out");
    assert_eq!(h.scene.icons.iter().flatten().filter(|i| i.0 == factory).map(|i| i.4).collect::<Vec<_>>(), vec![IconKind::Remembered], "zoomed out, one greyed icon");
    h.ticks(2);
    assert_eq!(at(&h).sighting, crate::engine::snapshot::Sighting::Remembered, "still there without a sighting");
}

/// Waypoint ETAs on a shared build: three engineers on one
/// build order count their build rates together (moho's command graph);
/// two more sent to help (repair orders on the structure) add theirs and
/// share the structure's one ETA, so it's drawn once. With the energy gone
/// the build stalls and its ETA holds (moho's leaves the economy out).
#[test]
fn shared_build_eta() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    let ids: mlua::Table = h
        .engine
        .sim_lua()
        .load(
            "local brain = ArmyBrains[1]
             brain:__engine_SetStorage('MASS', 5000) brain:GiveResource('MASS', 5000)
             brain:__engine_SetStorage('ENERGY', 5000) brain:GiveResource('ENERGY', 5000)
             local e = {}
             for i = 1, 5 do e[i] = CreateUnitHPR('uel0105', 'ARMY_1', 401 + i * 3, GetTerrainHeight(401 + i * 3, 592), 592, 0, 0, 0) end
             IssueBuildAllMobile({ e[1], e[2], e[3] }, Vector(410.5, GetTerrainHeight(410.5, 602.5), 602.5), 'ueb0102', {})
             _G.eta_engineers = e
             local out = {}
             for i = 1, 5 do out[i] = tonumber(e[i]:GetEntityId()) end
             return out",
        )
        .eval()
        .expect("spawn");
    let mut selection: Vec<u32> = ids.sequence_values::<u32>().map(Result::unwrap).collect();
    selection.sort_unstable();
    h.input.selection = selection.clone();
    h.request.lock().unwrap().selection = selection;
    h.ticks(10);
    h.settle();
    use crate::engine::sim::command::CommandKind as K;
    // (The air factory: BuildTime 300; UEF T1 engineers build at 5 each.)
    let done = |h: &mut Harness| -> f32 { h.engine.sim_lua().load("return _G.eta_engineers[1]:GetFocusUnit():GetFractionComplete()").eval::<f32>().expect("fraction") };
    let etas = |h: &mut Harness| -> Vec<(u8, f32)> { h.scene.graph(0).expect("view prepared").waypoints.iter().map(|w| (w.kind, w.eta)).collect() };
    h.ticks(2);
    let (left, got) = (1.0 - done(&mut h), etas(&mut h));
    let build: Vec<f32> = got.iter().filter(|w| K::from_u8(w.0) == Some(K::BuildMobile)).map(|w| w.1).collect();
    assert_eq!(build.len(), 1, "one waypoint for the shared order: {got:?}");
    // Three at work: 300 / 15 = 20 s for the whole (a tick or two behind the fraction).
    assert!((build[0] - left * 20.0).abs() < 0.4, "three engineers' summed rate: {build:?}, {left} left");
    // Two more help build it.
    h.engine.sim_lua().load("local e = _G.eta_engineers IssueRepair({ e[4], e[5] }, e[1]:GetFocusUnit())").exec().expect("repair");
    h.ticks(30);
    let (left, got) = (1.0 - done(&mut h), etas(&mut h));
    assert_eq!(got.len(), 2, "the build and the repair waypoints: {got:?}");
    assert_eq!(got[0].1, got[1].1, "one ETA for the structure: {got:?}");
    assert!((got[0].1 - left * 12.0).abs() < 0.4, "five engineers' summed rate (12 s for the whole): {got:?}, {left} left");
    // The energy runs out: the build stalls, its ETA holds still.
    h.engine.sim_lua().load("ArmyBrains[1]:TakeResource('ENERGY', ArmyBrains[1]:GetEconomyStored('ENERGY'))").exec().expect("drain");
    h.ticks(5);
    let held = etas(&mut h);
    h.ticks(20);
    let later = etas(&mut h);
    assert!(held.iter().zip(&later).all(|(a, b)| (a.1 - b.1).abs() < 0.25), "a stalled build's ETA holds: {held:?} then {later:?}");
}

/// Fog of war's clear ground: an ACU and two scouts of the view army
/// with overlapping vision are drawn as three circles round them (moho's
/// vision volumes), the enemy tank's vision not among them. With
/// `FOG_PNG=<dir>` the frame is saved as `fog_circles.png` (close) and
/// `fog_circles_far.png`.
#[test]
fn fog_circles_frame() {
    let Some(mut h) = Harness::new(None, false, None) else { return };
    h.engine
        .sim_lua()
        .load(
            r#"
            local function spawn(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
            spawn('uel0001', 'ARMY_1', 668, 366)
            spawn('uel0101', 'ARMY_1', 690, 356)
            spawn('uel0101', 'ARMY_1', 656, 384)
            spawn('uel0201', 'ARMY_2', 712, 392)
            "#,
        )
        .exec()
        .expect("spawn");
    h.request.lock().unwrap().view_army = Some(1);
    h.ticks(3);
    h.settle();
    for (name, zoom) in [("fog_circles", 90.0), ("fog_circles_far", 220.0)] {
        if let Some(c) = h.world.camera_mut() {
            c.target = Vec3::new(675.0, 0.0, 372.0);
            c.zoom = zoom;
        }
        h.frame();
        let mut circles = h.scene.vision_circles().to_vec();
        circles.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let at: Vec<[f32; 2]> = circles.iter().map(|c| [c[0].round(), c[1].round()]).collect();
        assert_eq!(at, vec![[656.0, 384.0], [668.0, 366.0], [690.0, 356.0]], "{name}: the view army's three sources, not the enemy's");
        assert!(circles.iter().all(|c| c[2] >= 20.0), "{name}: their vision radii {circles:?}");
        if let Ok(dir) = std::env::var("FOG_PNG") {
            let (w, ht) = (1280, 720);
            let rgba = render_rgba(&mut h, w, ht);
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap()), w, ht);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&rgba).unwrap();
        }
    }
}
