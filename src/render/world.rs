//! The main thread's side of the renderer: the world as the sim's snapshots
//! describe it (armies, economy, blueprints, the map's heights), the world
//! cameras and views the player moves, and the renderer's latest frame
//! (interpolated entities, strategic icons) for picking and the interface.
//!
//! The GPU side ([`super::scene::Scene`]) gets what each snapshot brings it
//! in order ([`SceneDelta`]), what the main thread decides each frame
//! ([`FrameInput`]: cameras, views, the selection, ghosts, settings), and
//! hands back what it worked out ([`FrameResults`]).

use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};

use crate::engine::snapshot::{EntityView, Snapshot, TerrainView, ViewKind};

use super::camera::{Camera, Cameras, View};
use super::scene::Ghost;

/// Seconds per sim tick at normal speed.
const TICK_SECONDS: f32 = 0.1;

/// What one snapshot brings the GPU side, in order: the snapshot (shared
/// with the [`World`]) and what was taken out of it for the GPU.
#[derive(Clone)]
pub struct SceneDelta {
    pub snapshot: Arc<Snapshot>,
    /// When the main thread took it (the frames blend over the measured
    /// interval between snapshots).
    pub arrived: Instant,
    pub vfs: Option<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>,
    pub clips: Vec<(u32, String)>,
    pub meshes: Vec<(u32, crate::engine::api::mesh::RenderAsset)>,
    pub effect_assets: Vec<(u32, String)>,
    pub terrain: Option<Arc<TerrainView>>,
    pub fog: Option<crate::engine::snapshot::FogUpdate>,
    pub marks: Vec<super::terrain::TreadMark>,
    pub decals: Option<Arc<Vec<crate::engine::assets::scmap::Decal>>>,
    /// The first of a new session's engine (`Snapshot::new_session`): the
    /// head's scene starts afresh before taking it.
    pub new_session: bool,
}

/// The render settings the console and the options set (moho's console
/// variables); the GPU side applies what changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderSettings {
    /// SMAA on the finished frame (FAF's Anti-Aliasing option).
    pub smaa: bool,
    /// Sun shadows on (the debug switch).
    pub shadows: bool,
    /// `ren_ShadowLOD`: the LOD metric past which nothing is shadowed.
    pub shadow_lod: f32,
    /// `shadow_Fidelity` (0-3): Off draws none; Low fewer cascades.
    pub shadow_fidelity: u8,
    /// `ren_ShadowSize`: texels per cascade side.
    pub shadow_size: u32,
    /// `graphics_Fidelity` (0-2): which of FAF's shader tiers meshes use.
    pub fidelity: u8,
    /// `ren_Skydome`.
    pub sky: bool,
    /// `ren_MipSkipLevels` (FAF's Texture Detail).
    pub mip_skip: u32,
    /// `ui_AlwaysRenderStrategicIcons`.
    pub always_icons: bool,
    /// `range_RenderSelected` / `range_RenderHighlighted`.
    pub range_selected: bool,
    pub range_highlighted: bool,
    /// `ren_Bloom` and its blur scale.
    pub bloom: bool,
    pub bloom_blur: f32,
    /// Interface options the main thread draws by: `ui_RenderUnitBars`,
    /// `UI_ForceLifbarsOnEnemy`, `ui_StrategicIconScale`.
    pub render_bars: bool,
    pub force_enemy_bars: bool,
    pub icon_scale: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            smaa: true,
            shadows: true,
            shadow_lod: f32::INFINITY,
            shadow_fidelity: 2,
            shadow_size: super::shadows::SIZE,
            fidelity: 2,
            sky: true,
            mip_skip: 0,
            always_icons: false,
            range_selected: true,
            range_highlighted: true,
            bloom: true,
            bloom_blur: super::bloom::BLUR_SCALE,
            render_bars: true,
            force_enemy_bars: false,
            icon_scale: 1.0,
        }
    }
}

/// What the main thread decides for a frame: the GPU side draws the latest.
#[derive(Clone)]
pub struct FrameInput {
    pub cameras: Cameras,
    pub views: Vec<View>,
    /// The cursor (window pixels) and the unit under it.
    pub cursor: Option<[f32; 2]>,
    pub hovered: Option<u32>,
    /// Physical pixels per interface pixel.
    pub ui_scale: f32,
    /// The player's selection (sorted): their brackets.
    pub selection: Vec<u32>,
    /// The build placement ghost, and a template's other structures.
    pub ghost: Option<Ghost>,
    pub template_ghosts: Vec<Ghost>,
    /// A waypoint being dragged (or let go, until the sim's move shows): its
    /// command, where its node is now, and whether the button is still held
    /// (a build's site then shows as the placement ghost).
    pub command_drag: Option<(u32, [f32; 3], bool)>,
    /// Order feedback and rally flags from the interface.
    pub interface_meshes: Vec<rts_core_engine::ui::world_mesh::InterfaceMesh>,
    /// The interface's ground decals (range rings under the cursor).
    pub user_decals: Option<Arc<Vec<crate::engine::assets::scmap::Decal>>>,
    /// The active range overlay filters, and each kind's own style.
    pub range_filters: Arc<Vec<rts_core_engine::ui::OverlayFilter>>,
    pub range_styles: Arc<Vec<Option<rts_core_engine::ui::RangeStyle>>>,
    /// The army colours as RGB (index = army - 1).
    pub army_rgb: Arc<Vec<[f32; 3]>>,
    /// The sim's blueprint tables (`Snapshot::blueprints` and the rest).
    pub blueprints: Option<Arc<Vec<String>>>,
    pub blueprint_icons: Option<Arc<Vec<String>>>,
    pub blueprint_icon_zoom: Option<Arc<Vec<f32>>>,
    pub blueprint_icon_layer: Option<Arc<Vec<u8>>>,
    pub blueprint_meshes: Option<Arc<Vec<(u32, f32)>>>,
    pub blueprint_selection: Option<Arc<Vec<[f32; 6]>>>,
    pub blueprint_skirts: Option<Arc<Vec<Option<[f32; 4]>>>>,
    pub blueprint_ranges: Option<Arc<Vec<Vec<(u8, f32, f32)>>>>,
    pub blueprint_assist_range: Option<Arc<Vec<f32>>>,
    pub deposits: Option<Arc<Vec<(u8, f64, f64, f64)>>>,
    pub settings: RenderSettings,
    /// What the world overlays and strategic icons draw by.
    pub overlays: super::overlays::OverlayState,
}

impl Default for FrameInput {
    fn default() -> Self {
        Self {
            cameras: Cameras::default(),
            views: Vec::new(),
            cursor: None,
            hovered: None,
            ui_scale: 1.0,
            selection: Vec::new(),
            ghost: None,
            template_ghosts: Vec::new(),
            command_drag: None,
            interface_meshes: Vec::new(),
            user_decals: None,
            range_filters: Arc::new(Vec::new()),
            range_styles: Arc::new(Vec::new()),
            army_rgb: Arc::new(Vec::new()),
            blueprints: None,
            blueprint_icons: None,
            blueprint_icon_zoom: None,
            blueprint_icon_layer: None,
            blueprint_meshes: None,
            blueprint_selection: None,
            blueprint_skirts: None,
            blueprint_ranges: None,
            blueprint_assist_range: None,
            deposits: None,
            settings: RenderSettings::default(),
            overlays: Default::default(),
        }
    }
}

/// What a frame worked out that the main thread reads: the interpolated
/// entities (picking, selection, lifebars, the minimap), each view's
/// strategic icons and order waypoints, and counts for the status bar.
#[derive(Clone, Default)]
pub struct FrameResults {
    pub entities: Arc<Vec<EntityView>>,
    pub model_draws: usize,
    pub particle_draws: usize,
    pub particles: usize,
}

/// One head's (window's) world views.
struct Head {
    /// Its size (pixels).
    screen: [f32; 2],
    /// This frame's world views, drawn in order ([`World::set_head_views`]).
    views: Vec<View>,
    /// The strategic icons drawn last frame, bottom to top: id and window
    /// rectangle (clicks on an icon pick its unit).
    icon_rects: Vec<(u32, [f32; 4])>,
}

impl Head {
    fn new(w: u32, h: u32) -> Self {
        Self { screen: [w as f32, h as f32], views: vec![View::full(w as f32, h as f32)], icon_rects: Vec::new() }
    }
}

/// The main thread's world: see the module docs.
pub struct World {
    /// The heads (head 0: the main window; head 1: FAF's second head).
    heads: Vec<Head>,
    /// The head the cursor is on: picks, the view under a point and the
    /// camera a wheel or drag moves are its.
    active: usize,
    /// The world cameras by name (`WorldCamera` from the first tick; the
    /// interface's other world views add theirs).
    pub cameras: Cameras,
    /// `cam_DefaultLOD`: the LOD scale a new camera takes.
    pub default_lod: f32,
    /// Latest and previous snapshot.
    pub latest: Option<Arc<Snapshot>>,
    pub previous: Option<Arc<Snapshot>>,
    pub map_size: Option<(f32, f32)>,
    /// The map's heights, for picking and the interface.
    pub terrain_view: Option<Arc<TerrainView>>,
    vfs: Option<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>,
    pub blueprints: Option<Arc<Vec<String>>>,
    /// The same names shared out one by one (`blueprint_name`), for the
    /// interface's unit records.
    pub blueprint_names: Option<Arc<Vec<Arc<str>>>>,
    /// Command caps per blueprint (`snapshot::caps`), same index.
    pub blueprint_caps: Option<Arc<Vec<u16>>>,
    /// Each blueprint's `snapshot::order_categories` bits.
    pub blueprint_order_categories: Option<Arc<Vec<u16>>>,
    /// Cargo-slot classes (bits 1 / 2 / 4) and cargo class (1..3) per
    /// blueprint (same index): the load cursors only where boarding works.
    pub blueprint_transport: Option<Arc<Vec<(u8, u8)>>>,
    pub blueprint_selection_priority: Option<Arc<Vec<u8>>>,
    pub blueprint_icons: Option<Arc<Vec<String>>>,
    pub blueprint_icon_zoom: Option<Arc<Vec<f32>>>,
    pub blueprint_icon_layer: Option<Arc<Vec<u8>>>,
    pub blueprint_meshes: Option<Arc<Vec<(u32, f32)>>>,
    pub blueprint_selection: Option<Arc<Vec<[f32; 6]>>>,
    /// Each blueprint's pick and drag-selection box fields ([`Snapshot::blueprint_pick`]).
    pub blueprint_pick: Option<Arc<Vec<[f32; 5]>>>,
    /// The sim's meshes' bounds by render index (mesh space, rest pose: min
    /// xyz, max xyz), as the renderer loads them; `None` until then.
    pub mesh_bounds: Vec<Option<[f32; 6]>>,
    /// How far each loaded mesh reaches from its origin (mesh space): a
    /// cheap reject before a unit's box is worked out.
    mesh_reach: Vec<f32>,
    pub blueprint_lifebars: Option<Arc<Vec<[f32; 3]>>>,
    pub blueprint_skirts: Option<Arc<Vec<Option<[f32; 4]>>>>,
    pub blueprint_ranges: Option<Arc<Vec<Vec<(u8, f32, f32)>>>>,
    pub blueprint_assist_range: Option<Arc<Vec<f32>>>,
    /// The map's resource deposits (type, x, z, size).
    pub deposits: Option<Arc<Vec<(u8, f64, f64, f64)>>>,
    /// The sim's latest answer to the placement query (blueprint, spot,
    /// skirt, allowed).
    pub placement: Vec<(String, [f32; 3], Option<[f32; 4]>, bool)>,
    /// The sim's compiled-script cache, for the interface's VM.
    pub script_cache: Option<Arc<crate::engine::lua::script_cache::ScriptCache>>,
    /// The latest prop views (`Snapshot::props`), merged into every
    /// snapshot's entities on arrival.
    prop_views: Vec<EntityView>,
    /// Units, structures, projectiles and props in the latest snapshot.
    pub entity_counts: [usize; 4],
    /// The sim thread has gone (its snapshot channel closed).
    pub sim_ended: bool,
    pub tick: u64,
    pub sim_tick: u64,
    pub economy: Vec<crate::engine::snapshot::ArmyEconomy>,
    /// `Sync` tables received and not yet given to the user layer.
    pub pending_sync: Vec<crate::engine::sync::SyncValue>,
    pub game_over: bool,
    /// World sounds go here (the process's audio, if any), and the
    /// listener last sent.
    audio: Option<crate::audio::Audio>,
    listener: Option<crate::audio::Listener>,
    /// Sim time per tick (ms, smoothed) and recent snapshot arrivals.
    pub tick_ms: f32,
    arrivals: std::collections::VecDeque<(Instant, u64)>,
    pub armies: Option<Arc<Vec<crate::engine::snapshot::ArmyInfo>>>,
    pub local_army: Option<u32>,
    pub focus_army_changed: Option<i32>,
    pub upgraded: Vec<(u32, u32)>,
    /// Launches for cameras following their launcher (`frame::camera_follows`).
    pub camera_follows: Vec<rts_core_engine::engine::sim::presentation::CameraFollow>,
    /// Units' custom names (`Snapshot::custom_names`).
    pub custom_names: Option<Arc<Vec<(u32, String)>>>,
    pub army_display: Option<Arc<Vec<crate::engine::snapshot::ArmyDisplay>>>,
    pub team_colors: Option<Vec<u32>>,
    pub army_rgb: Arc<Vec<[f32; 3]>>,
    pub projectile_icons: Arc<Vec<Option<crate::engine::sim::projectile::ProjectileIcon>>>,
    pub alliances: Option<Arc<Vec<(u32, u32, crate::engine::sim::army::Alliance)>>>,
    pub scenario: Option<Arc<crate::engine::sync::SyncValue>>,
    /// The renderer's latest frame.
    pub frame: FrameResults,
    /// The last `pick`: (pixel, tick, view-projection, result).
    pick_cache: std::cell::Cell<Option<([f32; 2], u64, Mat4, Option<u32>)>>,
    /// Sessions started (`Snapshot::new_session` seen): which session the
    /// world shows.
    pub session: u64,
}

impl World {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            heads: vec![Head::new(w, h)],
            active: 0,
            cameras: Cameras::default(),
            default_lod: super::camera::DEFAULT_LOD,
            latest: None,
            previous: None,
            map_size: None,
            terrain_view: None,
            vfs: None,
            blueprints: None,
            blueprint_names: None,
            blueprint_caps: None,
            blueprint_order_categories: None,
            blueprint_transport: None,
            blueprint_selection_priority: None,
            blueprint_icons: None,
            blueprint_icon_zoom: None,
            blueprint_icon_layer: None,
            blueprint_meshes: None,
            blueprint_selection: None,
            blueprint_pick: None,
            mesh_bounds: Vec::new(),
            mesh_reach: Vec::new(),
            blueprint_lifebars: None,
            blueprint_skirts: None,
            blueprint_ranges: None,
            blueprint_assist_range: None,
            deposits: None,
            placement: Vec::new(),
            script_cache: None,
            prop_views: Vec::new(),
            entity_counts: [0; 4],
            sim_ended: false,
            tick: 0,
            sim_tick: 0,
            economy: Vec::new(),
            pending_sync: Vec::new(),
            game_over: false,
            audio: crate::audio::Audio::installed(),
            listener: None,
            tick_ms: 0.0,
            arrivals: Default::default(),
            armies: None,
            local_army: None,
            focus_army_changed: None,
            upgraded: Vec::new(),
            camera_follows: Vec::new(),
            custom_names: None,
            army_display: None,
            team_colors: None,
            army_rgb: Arc::new(Vec::new()),
            projectile_icons: Arc::new(Vec::new()),
            alliances: None,
            scenario: None,
            frame: FrameResults::default(),
            pick_cache: std::cell::Cell::new(None),
            session: 0,
        }
    }

    /// A new session's engine started (`Snapshot::new_session`): everything
    /// the last session sent goes (entities, terrain, blueprints, armies,
    /// cameras, queued Sync values); the heads, their views and the
    /// player's settings stay.
    pub fn begin_session(&mut self) {
        let fresh = Self {
            heads: std::mem::take(&mut self.heads),
            active: self.active,
            default_lod: self.default_lod,
            team_colors: self.team_colors.take(),
            audio: self.audio.take(),
            session: self.session,
            ..Self::new(1, 1)
        };
        *self = fresh;
        self.rebuild_army_rgb();
    }

    /// The main window is `w` x `h` (see `resize_head`).
    pub fn resize(&mut self, w: u32, h: u32) {
        self.resize_head(0, w, h);
    }

    /// Head `head`'s window is `w` x `h` (a new head is added): its views
    /// are laid out again (until the interface does, the main one fills
    /// the main window) and every camera's zoom kept within its new limit.
    pub fn resize_head(&mut self, head: usize, w: u32, h: u32) {
        while self.heads.len() <= head {
            self.heads.push(Head::new(w, h));
        }
        self.heads[head].screen = [w as f32, h as f32];
        self.set_head_views(head, Vec::new());
        if let Some((mw, mh)) = self.map_size {
            for (_, c) in self.cameras.iter_mut() {
                c.fit_map(mw, mh);
                c.zoom = c.zoom.min(c.max_zoom);
            }
        }
    }

    /// The main world camera (`WorldCamera`), once the world shows.
    pub fn camera(&self) -> Option<&Camera> {
        self.cameras.main()
    }

    pub fn camera_mut(&mut self) -> Option<&mut Camera> {
        self.cameras.main_mut()
    }

    /// This frame's world views on the head the cursor is on.
    pub fn views(&self) -> &[View] {
        &self.heads[self.active].views
    }

    /// This frame's world views on head `head` (none for a head not there).
    pub fn head_views(&self, head: usize) -> &[View] {
        self.heads.get(head).map_or(&[], |h| h.views.as_slice())
    }

    /// The cursor is on head `head` (picks and the view under a point are
    /// its from now on).
    pub fn set_active_head(&mut self, head: usize) {
        self.active = head.min(self.heads.len() - 1);
    }

    pub fn active_head(&self) -> usize {
        self.active
    }

    /// The strategic icons head `head` drew last frame.
    /// Meshes' bounds as the renderer loads them (`PresentOut::mesh_bounds`).
    pub fn add_mesh_bounds(&mut self, bounds: &[(u32, [f32; 6])]) {
        for &(index, b) in bounds {
            let i = index as usize;
            if self.mesh_bounds.len() <= i {
                self.mesh_bounds.resize(i + 1, None);
                self.mesh_reach.resize(i + 1, 0.0);
            }
            self.mesh_bounds[i] = Some(b);
            self.mesh_reach[i] = Vec3::new(b[0].abs().max(b[3].abs()), b[1].abs().max(b[4].abs()), b[2].abs().max(b[5].abs())).length();
        }
    }

    pub fn set_icon_rects(&mut self, head: usize, rects: Vec<(u32, [f32; 4])>) {
        if let Some(h) = self.heads.get_mut(head) {
            h.icon_rects = rects;
        }
    }

    /// The main window's views (see `set_head_views`).
    pub fn set_views(&mut self, views: Vec<View>) {
        self.set_head_views(0, views);
    }

    /// The views head `head` draws this frame (the interface's world
    /// views, back to front), each kept inside the window in whole pixels;
    /// none (no interface, or it shows none): on the main window the main
    /// camera fills it, on another head nothing is drawn (it's cleared). A
    /// view's camera is made when it first shows (once the map is known),
    /// with the main camera's settings, looking at the whole map; each
    /// camera's aspect follows its view.
    pub fn set_head_views(&mut self, head: usize, views: Vec<View>) {
        let Some([sw, sh]) = self.heads.get(head).map(|h| h.screen) else { return };
        let mut views: Vec<View> = views
            .into_iter()
            .map(|mut v| {
                let [l, t, r, b] = v.rect;
                v.rect = [l.round().clamp(0.0, sw), t.round().clamp(0.0, sh), r.round().clamp(0.0, sw), b.round().clamp(0.0, sh)];
                v
            })
            .filter(|v| v.rect[2] - v.rect[0] >= 1.0 && v.rect[3] - v.rect[1] >= 1.0)
            .collect();
        if views.is_empty() {
            let mut full = View::full(sw.max(1.0), sh.max(1.0));
            if head > 0 {
                // No camera has this name: the head is cleared.
                full.camera = String::new();
            }
            views.push(full);
        }
        for v in &views {
            if self.cameras.get(&v.camera).is_none() && !v.camera.is_empty()
                && let Some(c) = self.new_camera(v.aspect()) {
                tracing::info!(camera = %v.camera, "world camera made");
                self.cameras.insert(&v.camera, c);
            }
            let map = self.map_size;
            if let Some(c) = self.cameras.get_mut(&v.camera)
                && c.aspect != v.aspect() {
                c.aspect = v.aspect();
                if let Some((mw, mh)) = map {
                    c.fit_map(mw, mh);
                    c.zoom = c.zoom.min(c.max_zoom);
                }
            }
        }
        self.heads[head].views = views;
    }

    /// A camera for a view first shown: the whole map at `aspect`, with
    /// the main camera's console settings (`cam_ZoomAmount`, pitches, nearest
    /// zoom) and the default LOD scale. `None` before the map is known; the
    /// main camera itself is made with the first tick (`receive`).
    fn new_camera(&self, aspect: f32) -> Option<Camera> {
        let (w, h) = self.map_size?;
        let main = self.camera()?;
        let mut c = Camera::new(w, h);
        (c.zoom_amount, c.near_pitch, c.far_pitch, c.min_zoom) = (main.zoom_amount, main.near_pitch, main.far_pitch, main.min_zoom);
        c.aspect = aspect;
        c.fit_map(w, h);
        c.zoom = c.max_zoom;
        c.lod_scale = self.default_lod;
        Some(c)
    }

    /// The view under window pixel (x, y) and its camera: the topmost
    /// view there, else the first (points outside every view: a drag that
    /// left its view, the gap between split views).
    pub fn view_at(&self, x: f32, y: f32) -> Option<(&View, &Camera)> {
        let shown = |v: &&View| self.cameras.get(&v.camera).is_some();
        let views = self.views();
        let v = views.iter().rev().filter(|v| v.contains(x, y)).find(shown).or_else(|| views.iter().find(shown))?;
        Some((v, self.cameras.get(&v.camera)?))
    }

    /// Whether world point `p` shows in a world view (on any head).
    pub fn in_view(&self, p: Vec3) -> bool {
        self.heads.iter().flat_map(|h| &h.views).any(|v| self.cameras.get(&v.camera).and_then(|c| v.project(c, p)).is_some_and(|[x, y]| x >= v.rect[0] && y >= v.rect[1] && x <= v.rect[2] && y <= v.rect[3]))
    }

    /// The camera of the view under window pixel (x, y) (see `view_at`).
    pub fn camera_at_mut(&mut self, x: f32, y: f32) -> Option<&mut Camera> {
        let name = self.view_at(x, y)?.0.camera.clone();
        self.cameras.get_mut(&name)
    }

    /// The interpolated entities last frame.
    pub fn entities(&self) -> &[EntityView] {
        &self.frame.entities
    }

    /// Tells the audio thread where the camera is (3D sounds), when it moved.
    pub fn update_listener(&mut self) {
        let (Some(audio), Some(cam)) = (&self.audio, self.cameras.main()) else { return };
        let eye = cam.eye();
        let forward = (cam.target - eye).normalize_or(Vec3::NEG_Y);
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let l = crate::audio::Listener { position: eye.to_array(), forward: forward.to_array(), right: right.to_array(), camera_distance: cam.eye_distance() };
        if self.listener != Some(l) {
            audio.send(crate::audio::Command::Listener(l));
            self.listener = Some(l);
        }
    }

    /// Takes every waiting snapshot, keeping the newest two; returns what
    /// each brings the GPU side, in order. `aspect`: the window's, for the
    /// main camera when the world first shows.
    pub fn receive(&mut self, rx: &std::sync::mpsc::Receiver<Snapshot>, aspect: f32) -> Vec<SceneDelta> {
        let mut out = Vec::new();
        loop {
            let mut snap = match rx.try_recv() {
                Ok(snap) => snap,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.sim_ended = true;
                    break;
                }
            };
            // Taken: its channel slot is free for the sim again.
            drop(snap.ticket.take());
            // Another session's engine: what the last one sent goes, and
            // its snapshots not yet handed to the heads with it.
            if snap.new_session {
                self.begin_session();
                self.session += 1;
                out.clear();
            }
            let _s = rts_core_engine::prof_span!("snapshot: take fields");
            if let Some(v) = &snap.vfs {
                self.vfs = Some(v.clone());
            }
            if let Some(t) = &snap.terrain {
                self.map_size = Some((t.width as f32, t.height as f32));
                self.terrain_view = Some(t.clone());
            }
            // The world shows from the first tick (the terrain arrives
            // before it, while the session starts up: `send_terrain`).
            if self.camera().is_none()
                && snap.tick > 0
                && let Some(t) = &self.terrain_view
            {
                let mut c = Camera::new(t.width as f32, t.height as f32);
                c.aspect = self.heads[0].views.iter().find(|v| v.camera.eq_ignore_ascii_case(super::camera::MAIN)).map_or(aspect, View::aspect);
                c.fit_map(t.width as f32, t.height as f32);
                c.zoom = c.max_zoom;
                c.lod_scale = self.default_lod;
                self.cameras.insert(super::camera::MAIN, c);
            }
            if let Some(i) = snap.blueprint_icons.take() {
                self.blueprint_icons = Some(i);
            }
            if let Some(z) = snap.blueprint_icon_zoom.take() {
                self.blueprint_icon_zoom = Some(z);
            }
            if let Some(l) = snap.blueprint_icon_layer.take() {
                self.blueprint_icon_layer = Some(l);
            }
            // Timed splats: FAF's tread marks (`TreadComponent`) and
            // footprints, laid and faded by the terrain renderer.
            let marks = snap
                .splats
                .drain(..)
                .map(|sp| super::terrain::TreadMark {
                    texture: sp.texture,
                    centre: [sp.centre[0], sp.centre[2]],
                    heading: sp.heading,
                    size: sp.size,
                    born: (sp.created as f32 - 1.0) * TICK_SECONDS,
                    lifetime: sp.duration,
                    lod: sp.lod,
                })
                .collect();
            if let Some(c) = snap.script_cache.take() {
                self.script_cache = Some(c);
            }
            if let Some(r) = snap.blueprint_ranges.take() {
                self.blueprint_ranges = Some(r);
            }
            if let Some(r) = snap.blueprint_assist_range.take() {
                self.blueprint_assist_range = Some(r);
            }
            if let Some(s) = snap.blueprint_selection.take() {
                self.blueprint_selection = Some(s);
            }
            if let Some(p) = snap.blueprint_pick.take() {
                self.blueprint_pick = Some(p);
            }
            if let Some(l) = snap.blueprint_lifebars.take() {
                self.blueprint_lifebars = Some(l);
            }
            if let Some(s) = snap.blueprint_skirts.take() {
                self.blueprint_skirts = Some(s);
            }
            if let Some(m) = snap.blueprint_meshes.take() {
                self.blueprint_meshes = Some(m);
            }
            if !snap.placement.is_empty() {
                self.placement = std::mem::take(&mut snap.placement);
            }
            if let Some(c) = snap.blueprint_order_categories.take() {
                self.blueprint_order_categories = Some(c);
            }
            if let Some(c) = snap.blueprint_caps.take() {
                self.blueprint_caps = Some(c);
            }
            if let Some(t) = snap.blueprint_transport.take() {
                self.blueprint_transport = Some(t);
            }
            if let Some(e) = snap.blueprint_selection_priority.take() {
                self.blueprint_selection_priority = Some(e);
            }
            if let Some(b) = snap.blueprints.take() {
                self.blueprint_names = Some(Arc::new(b.iter().map(|s| Arc::from(s.as_str())).collect()));
                self.blueprints = Some(b);
            }
            if let Some(d) = snap.deposits.take() {
                self.deposits = Some(d);
            }
            for (i, icon) in snap.new_projectile_icons.drain(..) {
                let (i, icons) = (i as usize, Arc::make_mut(&mut self.projectile_icons));
                if icons.len() <= i {
                    icons.resize(i + 1, None);
                }
                icons[i] = Some(icon);
            }
            self.tick = snap.tick;
            self.sim_tick = snap.sim_tick;
            self.economy = std::mem::take(&mut snap.economy);
            self.pending_sync.append(&mut snap.sync);
            self.game_over |= snap.game_over;
            // World sounds (presentation: nothing goes back to the sim).
            if let Some(audio) = &self.audio {
                if !snap.sounds.is_empty() {
                    audio.send(crate::audio::Command::World(std::mem::take(&mut snap.sounds)));
                }
                audio.send(crate::audio::Command::Ambients(std::mem::take(&mut snap.ambients)));
            }
            if let Some(s) = snap.scenario.take() {
                self.scenario = Some(s);
            }
            if let Some(a) = snap.armies.take() {
                self.armies = Some(a);
                self.local_army = snap.local_army;
            }
            if let Some(a) = snap.alliances.take() {
                self.alliances = Some(a);
            }
            if let Some(n) = snap.custom_names.take() {
                self.custom_names = Some(n);
            }
            if let Some(c) = snap.army_display.take() {
                self.army_display = Some(c);
                self.rebuild_army_rgb();
            }
            if let Some(f) = snap.focus_army {
                self.focus_army_changed = Some(f);
            }
            self.upgraded.append(&mut snap.upgrades);
            self.camera_follows.append(&mut snap.camera_follows);
            // No user layer to take them (yet): keep the latest few hundred.
            if self.pending_sync.len() > 600 {
                let extra = self.pending_sync.len() - 600;
                self.pending_sync.drain(..extra);
            }
            self.tick_ms = if self.tick_ms == 0.0 { snap.tick_ms } else { self.tick_ms * 0.9 + snap.tick_ms * 0.1 };
            let arrived = Instant::now();
            self.arrivals.push_back((arrived, snap.tick));
            let _ = _s;
            let _s = rts_core_engine::prof_span!("snapshot: props");
            if let Some(p) = snap.props.take() {
                self.prop_views = p.as_ref().clone();
            }
            for (id, view) in snap.prop_changes.drain(..) {
                match (self.prop_views.binary_search_by_key(&id, |v| v.id), view) {
                    (Ok(i), Some(v)) => self.prop_views[i] = v,
                    (Ok(i), None) => {
                        self.prop_views.remove(i);
                    }
                    (Err(i), Some(v)) => self.prop_views.insert(i, v),
                    (Err(_), None) => {}
                }
            }
            merge_props(&mut snap.entities, &self.prop_views);
            let mut counts = [0usize; 4];
            for e in &snap.entities {
                match e.kind {
                    ViewKind::Unit => counts[0] += 1,
                    ViewKind::Structure => counts[1] += 1,
                    ViewKind::Projectile => counts[2] += 1,
                    ViewKind::Prop => counts[3] += 1,
                    ViewKind::Dying | ViewKind::Other => {}
                }
            }
            self.entity_counts = counts;
            let _ = _s;
            let delta = SceneDelta {
                arrived,
                vfs: snap.vfs.take(),
                clips: std::mem::take(&mut snap.new_clips),
                meshes: std::mem::take(&mut snap.new_meshes),
                effect_assets: std::mem::take(&mut snap.new_effect_assets),
                terrain: snap.terrain.take(),
                fog: snap.fog.take(),
                marks,
                decals: snap.decals.take(),
                new_session: snap.new_session,
                snapshot: Arc::new(snap),
            };
            // Replacing drops the snapshot before the previous one (once the
            // GPU side has let go of it too).
            self.previous = self.latest.replace(delta.snapshot.clone());
            out.push(delta);
        }
        out
    }

    /// Head `head`'s frame input from the world's side (cameras, its
    /// views, the sim's tables, army colours); the caller adds the
    /// interface's (cursor, selection, ghosts, overlays, settings).
    pub fn fill_input(&self, head: usize, input: &mut FrameInput) {
        input.cameras.clone_from(&self.cameras);
        input.views.clear();
        input.views.extend_from_slice(self.head_views(head));
        input.army_rgb = self.army_rgb.clone();
        input.blueprints = self.blueprints.clone();
        input.blueprint_icons = self.blueprint_icons.clone();
        input.blueprint_icon_zoom = self.blueprint_icon_zoom.clone();
        input.blueprint_icon_layer = self.blueprint_icon_layer.clone();
        input.blueprint_meshes = self.blueprint_meshes.clone();
        input.blueprint_selection = self.blueprint_selection.clone();
        input.blueprint_skirts = self.blueprint_skirts.clone();
        input.blueprint_ranges = self.blueprint_ranges.clone();
        input.blueprint_assist_range = self.blueprint_assist_range.clone();
        input.deposits = self.deposits.clone();
        input.overlays.lifebars = self.blueprint_lifebars.clone();
        input.overlays.alliances = self.alliances.clone();
        input.overlays.projectile_icons = self.projectile_icons.clone();
    }

    /// Blueprint `index`'s name, shared (empty when unknown).
    pub fn blueprint_name(&self, index: u32) -> Arc<str> {
        self.blueprint_names.as_ref().and_then(|n| n.get(index as usize).cloned()).unwrap_or_else(|| Arc::from(""))
    }

    /// Sim ticks per second over the last second (10 = real time).
    pub fn tick_rate(&mut self) -> f32 {
        let now = Instant::now();
        while self.arrivals.front().is_some_and(|(t, _)| now.duration_since(*t).as_secs_f32() > 1.0) {
            self.arrivals.pop_front();
        }
        match (self.arrivals.front(), self.arrivals.back()) {
            (Some(a), Some(b)) if b.1 > a.1 => (b.1 - a.1) as f32 / now.duration_since(a.0).as_secs_f32().max(1e-3),
            _ => 0.0,
        }
    }

    /// The game's file system, once the first snapshot brought it.
    pub fn models_vfs(&self) -> Option<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>> {
        self.vfs.clone()
    }

    /// Live effects in the latest snapshot.
    pub fn effect_count(&self) -> usize {
        self.latest.as_ref().map_or(0, |s| s.effects.len())
    }

    /// Debug extras of the latest snapshot.
    pub fn debug(&self) -> Option<&crate::engine::snapshot::DebugData> {
        self.latest.as_ref().map(|s| &s.debug)
    }

    /// Sets the interface's team colours (`None`: the mode is off).
    pub fn set_team_colors(&mut self, colors: Option<Vec<u32>>) {
        self.team_colors = colors;
        self.rebuild_army_rgb();
    }

    /// The colour each army is drawn in: the team colour while the mode is
    /// on and has one for it, else the session's, else our palette's.
    fn rebuild_army_rgb(&mut self) {
        use crate::engine::api::colors::{fallback_army_color, rgb};
        let display = self.army_display.as_deref().map_or(&[][..], |d| d.as_slice());
        let n = display.iter().map(|d| d.army).max().unwrap_or(0);
        let mut out: Vec<[f32; 3]> = (1..=n).map(|a| rgb(fallback_army_color(a))).collect();
        for d in display {
            if let Some(slot) = d.army.checked_sub(1).and_then(|i| out.get_mut(i as usize)) {
                *slot = rgb(d.color);
            }
        }
        if let Some(team) = &self.team_colors {
            for (slot, &c) in out.iter_mut().zip(team) {
                *slot = rgb(c);
            }
        }
        self.army_rgb = Arc::new(out);
    }

    /// The unit, structure or prop under window pixel (px, py): the nearest
    /// box the ray of the view's camera there hits (props last), in last
    /// frame's interpolated positions. Other entities (shield bubbles, the
    /// script entities FAF hangs on units for effects and beams) don't take
    /// clicks: they'd sit in front of the unit.
    pub fn pick(&self, px: f32, py: f32) -> Option<u32> {
        // A strategic icon covers its unit: the topmost under the point.
        if let Some(&(id, _)) = self.heads[self.active].icon_rects.iter().rev().find(|(_, r)| px >= r[0] && px < r[2] && py >= r[1] && py < r[3]) {
            return Some(id);
        }
        let (view, cam) = self.view_at(px, py)?;
        let view_proj = cam.view_proj();
        if let Some((at, tick, vp, hit)) = self.pick_cache.get()
            && at == [px, py] && tick == self.tick && vp == view_proj {
            return hit;
        }
        let [lx, ly] = view.local(px, py);
        let hit = self.pick_boxes(lx, ly, view.size(), view_proj);
        self.pick_cache.set(Some(([px, py], self.tick, view_proj, hit)));
        hit
    }

    /// Moho's click pick (`render::selection_box`): of the units whose
    /// click box (their mesh's, trimmed and scaled by the blueprint) the
    /// camera ray through (px, py) enters, the one whose position projects
    /// nearest (px, py); props (their collision boxes) only when no unit is
    /// hit. Boxes are at least 0.6 each side across, so small units stay
    /// clickable (ours: moho picks with a volume of a few pixels, a ray here).
    fn pick_boxes(&self, px: f32, py: f32, screen: [f32; 2], view_proj: Mat4) -> Option<u32> {
        use super::selection_box::{pick_box, ray_enters, unit_box};
        let inv = view_proj.inverse();
        let ndc = glam::Vec2::new(px / screen[0] * 2.0 - 1.0, 1.0 - py / screen[1] * 2.0);
        let near = inv.project_point3(ndc.extend(0.0));
        let far = inv.project_point3(ndc.extend(1.0));
        let dir = (far - near).normalize();
        let picks = self.blueprint_pick.as_deref();
        let mut best: Option<(bool, f32, u32)> = None;
        for e in self.entities() {
            // A unit being built in a factory isn't picked: the click goes
            // through to the factory around it.
            if !matches!(e.kind, ViewKind::Unit | ViewKind::Structure | ViewKind::Prop) || e.half == [0.0; 3] || e.in_builder {
                continue;
            }
            let prop = e.kind == ViewKind::Prop;
            let bounds = if prop { None } else { self.mesh_bounds.get(e.mesh as usize).copied().flatten() };
            // Off the ray by more than everything it can reach from its
            // position (padded): no hit (most entities, before the box is
            // worked out).
            // (Props: their whole collision box.)
            let pick = if prop { [0.0, 0.0, 1.0, 1.0, 1.0] } else { picks.and_then(|p| p.get(e.blueprint as usize).copied()).unwrap_or([0.0, 0.5, 1.0, 1.0, 1.0]) };
            let widen = pick[2].abs().max(pick[4].abs()).max(1.0);
            let reach = match bounds {
                Some(_) => self.mesh_reach[e.mesh as usize] * e.scale[0].abs().max(e.scale[1].abs()).max(e.scale[2].abs()),
                None => Vec3::from_array(e.center).length() + Vec3::from_array(e.half).length(),
            } * widen + 0.9;
            let to_origin = Vec3::from_array(e.position) - near;
            let along = to_origin.dot(dir);
            if (to_origin - dir * along).length_squared() > reach * reach {
                continue;
            }
            let b = unit_box(e.position, e.rotation, e.scale, e.center, e.half, bounds);
            let (mut min, mut max) = pick_box(&b, pick);
            for k in [0, 2] {
                let c = (min[k] + max[k]) * 0.5;
                let h = ((max[k] - min[k]) * 0.5).max(0.6);
                (min[k], max[k]) = (c - h, c + h);
            }
            if ray_enters(near, dir, min, max).is_none() {
                continue;
            }
            let clip = view_proj * Vec3::from_array(e.position).extend(1.0);
            if clip.w <= 0.0 {
                continue;
            }
            let (sx, sy) = ((clip.x / clip.w + 1.0) * 0.5 * screen[0], (1.0 - clip.y / clip.w) * 0.5 * screen[1]);
            let key = (prop, (sx - px).powi(2) + (sy - py).powi(2), e.id);
            if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
                best = Some(key);
            }
        }
        best.map(|b| b.2)
    }

    /// The ray through window pixel (px, py) of `view`'s camera `cam`:
    /// origin and direction.
    fn ray(view: &View, cam: &Camera, px: f32, py: f32) -> (Vec3, Vec3) {
        let inv = cam.view_proj().inverse();
        let ([x, y], [w, h]) = (view.local(px, py), view.size());
        let ndc = glam::Vec2::new(x / w * 2.0 - 1.0, 1.0 - y / h * 2.0);
        let near = inv.project_point3(ndc.extend(0.0));
        let far = inv.project_point3(ndc.extend(1.0));
        (near, (far - near).normalize())
    }

    /// Where the ray through window pixel (px, py) of the camera of the
    /// view there meets the terrain (clamped to the map).
    pub fn ground_at(&self, px: f32, py: f32) -> Option<Vec3> {
        let (view, cam) = self.view_at(px, py)?;
        self.ground_through(view, cam, px, py)
    }

    /// Where the ray through window pixel (px, py) of `view`'s camera `cam`
    /// meets the terrain (clamped to the map): march to the first sample
    /// below ground, then bisect.
    pub fn ground_through(&self, view: &View, cam: &Camera, px: f32, py: f32) -> Option<Vec3> {
        let (o, d) = Self::ray(view, cam, px, py);
        let (w, h) = self.map_size?;
        let step = (cam.eye_distance() / 300.0).max(0.25);
        // The ground under the cursor, or the water's surface over it
        // (clicks on water mean the surface, as moho's mouse world position).
        let below = |t: f32| {
            let p = o + d * t;
            p.y <= self.surface_at(p.x, p.z)
        };
        let max_t = cam.eye_distance() * 6.0 + 1000.0;
        let mut t = 0.0;
        while t < max_t && !below(t) {
            t += step;
        }
        if t >= max_t {
            // Ray misses the ground (looking at the sky): use the height of
            // the camera's look-at point.
            if d.y.abs() < 1e-6 {
                return None;
            }
            t = (cam.target.y - o.y) / d.y;
            if t < 0.0 {
                return None;
            }
        } else {
            let (mut lo, mut hi) = ((t - step).max(0.0), t);
            for _ in 0..12 {
                let mid = 0.5 * (lo + hi);
                if below(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            t = hi;
        }
        let p = o + d * t;
        let (x, z) = (p.x.clamp(0.0, w), p.z.clamp(0.0, h));
        Some(Vec3::new(x, self.surface_at(x, z), z))
    }

    /// The terrain's height at (x, z), or the water's surface where that's
    /// higher.
    pub fn surface_at(&self, x: f32, z: f32) -> f32 {
        let ground = self.height_at(x, z);
        match self.terrain_view.as_ref().and_then(|t| t.water) {
            Some(water) => ground.max(water),
            None => ground,
        }
    }

    /// Terrain height at (x, z) from the snapshot heightfield (nearest sample).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let Some(t) = &self.terrain_view else { return 0.0 };
        let xi = (x.max(0.0) as u32).min(t.width);
        let zi = (z.max(0.0) as u32).min(t.height);
        t.heights[(zi * (t.width + 1) + xi) as usize]
    }
}

/// `entities` (id order, the snapshot's) with `props` (id order, the latest
/// prop views) merged in; an id in both keeps the snapshot's view (a posed
/// prop, sent with its pose ops).
fn merge_props(entities: &mut Vec<EntityView>, props: &[EntityView]) {
    if props.is_empty() {
        return;
    }
    let dynamic = std::mem::take(entities);
    let mut out = Vec::with_capacity(dynamic.len() + props.len());
    let mut p = props.iter().peekable();
    for d in dynamic {
        while let Some(prop) = p.next_if(|prop| prop.id < d.id) {
            out.push(*prop);
        }
        p.next_if(|prop| prop.id == d.id);
        out.push(d);
    }
    out.extend(p.copied());
    *entities = out;
}

#[cfg(test)]
mod pick_timing {
    /// Times `pick_boxes` over 48k units (a zoomed-out 63-army battle):
    /// `cargo test --release --bin rts-core-engine pick_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn pick_over_48k_units() {
        use crate::engine::snapshot::{EntityView, Sighting, ViewKind};
        use glam::Vec3;
        let mut w = super::World::new(1920, 1080);
        let units: Vec<EntityView> = (0..48_000u32)
            .map(|i| EntityView { id: i + 1, kind: ViewKind::Unit, army: 1, position: [(i % 220) as f32 * 4.0, 20.0, (i / 220) as f32 * 4.0], rotation: [0.0, 0.0, 0.0, 1.0], center: [0.0, 0.5, 0.0], half: [0.5, 0.5, 0.7], health: 1.0, max_health: 100.0, build_rate: 0.0, build: 1.0, work: 0.0, shield: -1.0, blueprint: 0, mesh: i % 50, scale: [0.1; 3], pose_start: 0, pose_len: 0, sighting: Sighting::Seen, in_builder: false, attached: false, scroll: [0.0; 4], order_flags: 0 })
            .collect();
        w.frame.entities = std::sync::Arc::new(units);
        w.add_mesh_bounds(&(0..50).map(|m| (m, [-5.0, 0.0, -7.0, 5.0, 8.0, 7.0])).collect::<Vec<_>>());
        let view_proj = glam::camera::rh::proj::directx::perspective(0.8, 16.0 / 9.0, 1.0, 2000.0) * glam::camera::rh::view::look_at_mat4(Vec3::new(440.0, 600.0, 0.0), Vec3::new(440.0, 20.0, 400.0), Vec3::Y);
        let mut best = std::time::Duration::MAX;
        for k in 0..50 {
            let t = std::time::Instant::now();
            std::hint::black_box(w.pick_boxes(900.0 + k as f32, 500.0, [1920.0, 1080.0], view_proj));
            best = best.min(t.elapsed());
        }
        eprintln!("pick over 48k: {best:?}");
    }
}
