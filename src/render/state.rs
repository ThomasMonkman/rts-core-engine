use crate::debug_ui::DebugUiState;
use std::sync::{Arc, Mutex};
use winit::window::Window;
use rts_core_engine::engine::sim::command::CommandTarget;

pub struct RenderState {
    pub window: Arc<Window>,
    /// The device (its memory use for the status bar); the GPU work is the
    /// presenter's.
    pub device: wgpu::Device,
    /// The surface's configuration as the main thread last set it (its
    /// size, present mode); the presenter holds the surface.
    pub surface_config: wgpu::SurfaceConfiguration,
    pub egui_ctx: egui::Context,
    pub egui_state: egui_winit::State,
    pub collector: egui_tracing::EventCollector,
    pub debug_ui: Arc<Mutex<DebugUiState>>,
    /// The window's GPU side (the scene, the interface's and egui's passes,
    /// the surface), on its own thread.
    pub presenter: super::present::PresentThread,
    /// What the last frame handed back (timings, the interface's draw count).
    pub present_out: super::present::PresentOut,
    /// Images the interface draws by name that the presenter has.
    pub ui_images: std::collections::BTreeSet<String>,
    /// The main thread's world: snapshot data, cameras and views, the last
    /// frame's entities for picking.
    pub world: super::world::World,
    /// The render settings (console, options, debug windows) and the
    /// frame's input for the GPU side, filled each frame.
    pub settings: super::world::RenderSettings,
    pub frame_input: super::world::FrameInput,
    /// CPU time to build each frame (ms, without waiting on the swapchain:
    /// vsync pacing isn't work), eased, and its recent window.
    pub cpu_frame_ms: f32,
    pub cpu_times: Percentiles,
    /// GPU time per frame (ms), eased (0: not measured).
    pub gpu_frame_ms: f32,
    /// Process memory and GPU memory (bytes), sampled twice a second.
    pub memory: super::memory::MemorySample,
    pub snapshots: std::sync::mpsc::Receiver<crate::engine::snapshot::Snapshot>,
    pub input: Input,
    pub last_frame: std::time::Instant,
    /// Smoothed real time between frames (seconds), for the FPS readout.
    pub frame_interval: f32,
    /// Recent frame intervals and sim tick times (ms), for the status
    /// bar's p50 / p99.
    pub frame_times: Percentiles,
    pub tick_times: Percentiles,
    /// Frames over 100 ms (stutter), for the log.
    pub slow_frames: rts_core_engine::diagnostics::Hitches,
    /// The sim tick whose time `tick_times` last took.
    pub timed_tick: u64,
    /// `--screenshot <png> [--screenshot-tick <n>]`: save a frame once the
    /// sim reaches tick n, then exit (visual checks without screen access).
    pub screenshot: Option<(std::path::PathBuf, u64)>,
    /// See `Options::exit_when_sim_ends`.
    pub exit_when_sim_ends: bool,
    /// `--camera x,z,distance`: initial camera (applied once the map loads).
    pub camera_override: Option<(f32, f32, f32)>,
    /// Overlays / selection the renderer asks the sim to include.
    pub request: crate::engine::snapshot::SharedDebugRequest,
    /// Game speed / pause shared with the sim loop.
    pub control: std::sync::Arc<crate::engine::snapshot::SimControl>,
    /// Selection and orders.
    pub player: super::player::PlayerControl,
    /// The user layer (FAF's UI scripts), booted once the VFS arrives.
    pub ui: Option<rts_core_engine::ui::UserVm>,
    /// The interface VM once its blueprints and `userInit.lua` have run,
    /// waiting for the session (armies, scenario) to build the panels.
    pub ui_booting: Option<rts_core_engine::ui::UserVm>,
    /// The user layer failed to start (don't retry every frame).
    pub ui_failed: bool,
    /// Snapshot tick the UI last had its beat for.
    pub ui_tick: u64,
    /// Mouse buttons held, as bits (1 << button), for the interface.
    pub ui_buttons: u8,
    /// The selection the interface was last told about.
    pub ui_selection: Vec<u32>,
    /// The `Game.prefs` the interface loads and saves to.
    pub prefs: rts_core_engine::install::Prefs,
    /// Ground under the cursor before this frame's zoom.
    pub zoom_anchor: Option<glam::Vec3>,
    /// Cameras following entities (`UI_TrackUnit` the main one,
    /// `TrackEntities` any), each until moved by hand or every entity it
    /// follows is gone.
    pub tracks: Vec<(String, Vec<u32>)>,
    /// Followed cameras switched to a projectile their entity launched
    /// (`frame::ProjectileFollow`).
    pub projectile_follows: Vec<super::frame::ProjectileFollow>,
    /// Timed camera moves the interface asked for, under way, at most one
    /// per camera.
    pub camera_moves: Vec<(String, super::camera::CameraMove)>,
    /// Camera requests for a camera not made yet, and the frames each has
    /// waited.
    pub camera_waiting: Vec<(u32, rts_core_engine::ui::CameraRequest)>,
    /// Shift held as last told to the interface (FAF's command graph).
    pub ui_shift: bool,
    /// Drag building: where the drag began (snapped), while the button is
    /// held in build mode on a `DRAGBUILD` structure.
    pub build_drag: Option<glam::Vec3>,
    /// Hovering a waypoint that converts the selection's moves into a
    /// patrol: each group of units with the same route, and that route.
    pub patrol_convert: Option<Vec<(Vec<u32>, Vec<glam::Vec3>)>>,
    /// The right button went down to convert (the order goes on release).
    pub patrol_converting: bool,
    /// The waypoint under the cursor (a unit on its order, and the order),
    /// and the one being dragged.
    pub hover_command: Option<(u32, rts_core_engine::engine::snapshot::OrderMarker)>,
    pub command_drag: Option<crate::render::player::CommandDrag>,
    /// A dragged order let go and sent: (command, where it went, its node
    /// there, the snapshot tick then). It draws there until a snapshot shows
    /// the move (the sim takes it a tick or more later).
    pub command_moved: Option<(u32, CommandTarget, [f32; 3], u64)>,
    /// The window resized since the interface was last told.
    pub ui_resized: bool,
    /// Physical pixels per logical pixel (the window's scale factor, the
    /// web's devicePixelRatio): FAF's interface is laid out in logical
    /// pixels and drawn at this scale.
    pub scale_factor: f32,
    /// The alliances last given to the user layer.
    pub ui_alliances: Option<std::sync::Arc<Vec<(u32, u32, crate::engine::sim::army::Alliance)>>>,
    /// The army display the user layer has (`Scene::army_display`).
    pub ui_army_display: Option<std::sync::Arc<Vec<crate::engine::snapshot::ArmyDisplay>>>,
    /// The custom names the user layer has (`Scene::custom_names`).
    pub ui_custom_names: Option<std::sync::Arc<Vec<(u32, String)>>>,
    /// The `TeamColorMode` version the scene has.
    pub ui_team_colors: u64,
    /// The interface's draw list this frame (physical pixels), drawn over
    /// the strategic icons.
    pub ui_items: Vec<rts_core_engine::ui::controls::DrawItem>,
    /// Where the interface's minimap is this frame (screen pixels).
    pub minimap_rect: Option<[f32; 4]>,
    /// The minimap's map preview, looked for once a session.
    pub minimap: super::frame::MinimapPreview,
    /// The minimap's unit dots, an image the size of the minimap redrawn
    /// each sim tick (`frame::minimap_items`): pixels, the tick drawn, size.
    pub minimap_dots: Vec<u8>,
    pub minimap_dots_tick: u64,
    pub minimap_dots_size: (u32, u32),
    /// `--select`: re-applied once the interface is up (FAF's startup
    /// clears the selection).
    pub pending_select: Vec<u32>,
    /// `--ui-lua`: run in the interface with the start-up selection.
    pub pending_ui_lua: Option<String>,
    /// Watching a recording (`--replay`).
    pub replay: bool,
    /// The recording's timeline (`Replay.GetEvents`), while watching one.
    pub timeline: Option<std::sync::Arc<rts_core_engine::engine::replay::Timeline>>,
    /// A replay seek under way (`Replay.Seek`).
    pub seek: Option<Seek>,
    /// The watched replay's keyframes (`docs/architecture/replays.md`): where a seek
    /// can start from.
    pub keyframes: Option<Keyframes>,
    /// No session: the interface is the main menu (moho's front end).
    pub front_end: bool,
    /// Where a lobby's launch goes (`Options::launch`).
    pub launch: Option<std::sync::mpsc::Sender<crate::engine::session::Launch>>,
    /// The front end's preferences, for the game's fresh interface VM.
    pub carried_prefs: Option<crate::engine::sim::lua_data::LuaData>,
    /// The egui debug windows (Ctrl+` toggles; hidden at start, the game's
    /// own interface is the HUD).
    pub show_debug: bool,
    /// The egui stats bar along the bottom (its own button toggles it).
    pub show_status: bool,
    /// `--no-aa`: anti-aliasing off whatever the interface's settings say.
    pub no_aa: bool,
    /// `--window-size`: the window keeps that size (the profile's display
    /// mode isn't applied).
    pub fixed_size: bool,
    /// `--hold-shift`: Shift counts as held (screenshots).
    pub hold_shift: bool,
    /// Interface options read from the profile ([`crate::render::console::read_profile_options`]).
    pub profile: crate::render::console::ProfileOptions,
    /// The unit under the cursor this frame (not over a panel).
    pub rollover: Option<u32>,
    /// The cursor FAF wants, and the one the window has.
    pub ui_cursor: rts_core_engine::ui::cursor::Wanted,
    pub os_cursor: rts_core_engine::ui::cursor::Wanted,
    /// Hardware cursors made from FAF's textures, by path (`None`: failed).
    pub os_cursors: std::collections::BTreeMap<String, Option<winit::window::CustomCursor>>,
    /// The present modes the surface offers (FAF's Vertical Sync option
    /// picks among them).
    pub present_modes: Vec<wgpu::PresentMode>,
    /// FAF's Frametime option (`sc_FrameTimeClamp`): the shortest time a
    /// frame takes (a frame rate cap; zero: none).
    pub min_frame: std::time::Duration,
    /// FAF's Lock Fullscreen Cursor To Window option (`SC_ToggleCursorClip`).
    pub cursor_clip: bool,
    /// Space and the mouse are spinning this camera; the cursor (hidden
    /// meanwhile) goes back to where it was when it ends.
    pub spinning: Option<(String, (f64, f64))>,
    /// `cam_Free` ("Allow the camera to remain rotated", FAF's Ctrl-V): a
    /// spin isn't undone when Space is let go, and works at any zoom.
    pub cam_free: bool,
    /// FAF's Primary Adapter value in effect (`windowed`, `w,h,hz`).
    pub display_mode: String,
    /// The Primary Display option: the monitor the main window goes on
    /// ([`monitors`] key; empty: the system's main display).
    pub primary_display: String,
    /// The Secondary Display option: the second head's monitor (empty: the
    /// first other than the main window's).
    pub secondary_display: String,
    /// The interface saved its preferences this frame: the display options
    /// are read again after it.
    pub displays_saved: bool,
    /// What a second head is made with (its surface from the same instance,
    /// on the same device).
    pub gpu: Gpu,
    /// FAF's second head, once made (`make_second_head`).
    pub second: Option<SecondHead>,
    /// The second head to make (the window needs the event loop: `app.rs`
    /// makes it after the frame that asked) and its size, decided once
    /// (moho reads Secondary Adapter as it makes the device: a change takes
    /// a restart).
    pub second_wanted: Option<(SecondHeadSpec, u32, u32, f32)>,
    pub second_decided: bool,
    /// Snapshots taken before the second head was decided on, for its
    /// scene if it is made (the file system, the map).
    pub second_stash: Vec<super::world::SceneDelta>,
    /// `--second-head`: overrides the Secondary Adapter option.
    pub second_head_flag: Option<SecondHeadSpec>,
    /// The cursor is on head 1's window.
    pub cursor_head: usize,
    /// FAF's Vertical Sync option is on.
    pub vsync: bool,
    /// The windows are Wayland's: vsync presents by Mailbox
    /// ([`present_mode`]).
    pub wayland: bool,
    /// Whether the main window can be seen ([`RenderState::apply_visibility`]).
    pub visibility: Visibility,
    /// Present threads ask for frames through it.
    pub proxy: winit::event_loop::EventLoopProxy<super::app::UserEvent>,
}

/// What the main window's GPU setup leaves for a second head.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub queue: wgpu::Queue,
    /// The textures both heads' models load through (each once).
    pub textures: super::texture_load::Cache,
}

/// How FAF's second head shows (`secondary_adapter`, `--second-head`).
#[derive(Clone, Debug, PartialEq)]
pub enum SecondHeadSpec {
    /// Borderless fullscreen on another monitor (the option's
    /// `width,height,refresh`: the monitor's desktop mode is kept).
    Mode(u32, u32, u32),
    /// A window on the same display (dev, testing on one monitor).
    Windowed,
    /// No window: drawn offscreen (tests, `--screenshot` of both heads).
    Offscreen,
}

impl SecondHeadSpec {
    /// FAF's Secondary Adapter value: `width,height,refresh`, the engine's
    /// `windowed`, or none (`disabled`; `overridden`, which FAF reads as
    /// disabled).
    pub fn from_option(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "windowed" => Some(Self::Windowed),
            v => parse_mode(v).map(|(w, h, hz)| Self::Mode(w, h, hz)),
        }
    }

    /// `windowed`, `offscreen`, or `width,height,refresh`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "windowed" => Some(Self::Windowed),
            "offscreen" => Some(Self::Offscreen),
            v => parse_mode(v).map(|(w, h, hz)| Self::Mode(w, h, hz)),
        }
    }
}

/// FAF's second head: root frame 1 (`GetFrame(1)`, where FAF puts its
/// `CameraHead2` world view) in its own window, drawn by its own present
/// thread at its own display's vsync.
pub struct SecondHead {
    /// `None`: drawn offscreen.
    pub window: Option<Arc<Window>>,
    pub presenter: super::present::PresentThread,
    /// Its size (physical pixels) and scale (physical per logical pixel).
    pub size: (u32, u32),
    pub scale_factor: f32,
    /// Snapshots taken since its last frame: its scene takes every one.
    pub deltas: Vec<super::world::SceneDelta>,
    /// Root frame 1's draw list (physical pixels), made in the main
    /// window's frame (the interface runs once per main frame).
    pub ui_items: Vec<rts_core_engine::ui::controls::DrawItem>,
    /// `--screenshot`: saved here, before the main window's: asked for by
    /// the main frame, sent with this head's next frame, then saved.
    pub capture: Option<std::path::PathBuf>,
    pub capture_requested: bool,
    pub capture_sent: bool,
    pub captured: bool,
    /// It has a window (its frames wait on vsync): the main window paces
    /// its frames too ([`RenderState::pace_main`]).
    pub paced: bool,
    /// Its last frame's CPU time (ms): the render thread's, and the main
    /// thread's (`second_frame`).
    pub cpu_ms: f32,
    pub main_ms: f32,
    /// The time between its frames (s), eased, for its FPS readout.
    pub frame_interval: f32,
    /// Its FPS readout's egui (no input: nothing on it is clicked).
    pub egui_ctx: egui::Context,
    /// Whether its window can be seen.
    pub visibility: Visibility,
    /// When its last frame started (its cameras step by the time since).
    pub last_frame: std::time::Instant,
}

/// Camera input gathered from window events between frames.
#[derive(Default)]
pub struct Input {
    pub keys: std::collections::BTreeSet<winit::keyboard::KeyCode>,
    pub wheel: f32,
    /// Precise-scroll gesture / momentum state (issue #3).
    pub scroll: super::app::ScrollState,
    pub middle_down: bool,
    pub cursor: Option<(f64, f64)>,
    /// Middle-drag motion since the last frame, in pixels.
    pub drag: (f64, f64),
    /// Raw mouse motion since the last frame (the device's, so it goes on
    /// with the cursor held still): what a Space spin turns by.
    pub motion: (f64, f64),
    /// Selection / order input since the last frame, with the modifiers
    /// held at the time.
    pub events: Vec<(super::player::Event, super::player::Mods)>,
    pub mods: super::player::Mods,
}

impl Input {
    /// A cursor move (physical pixels). A high-polling mouse sends many per
    /// frame, and only the latest position is used: one queued move with
    /// nothing after it is replaced instead of piling up, which keeps the
    /// interface's per-event hit tests to one a frame.
    pub fn cursor_moved(&mut self, x: f64, y: f64, mods: super::player::Mods) {
        match self.events.last_mut() {
            Some((super::player::Event::Move(px, py), last)) => {
                *px = x;
                *py = y;
                *last = mods;
            }
            _ => self.events.push((super::player::Event::Move(x, y), mods)),
        }
    }
}

impl RenderState {
    pub async fn new(
        window: Arc<Window>,
        collector: egui_tracing::EventCollector,
        debug_ui: Arc<Mutex<DebugUiState>>,
        snapshots: std::sync::mpsc::Receiver<crate::engine::snapshot::Snapshot>,
        request: crate::engine::snapshot::SharedDebugRequest,
        control: std::sync::Arc<crate::engine::snapshot::SimControl>,
        options: super::Options,
        proxy: winit::event_loop::EventLoopProxy<super::app::UserEvent>,
    ) -> Self {
        let scale_factor = window.scale_factor() as f32;
        let size = window.inner_size();

        // wgpu's environment variables apply (`WGPU_BACKEND`, `WGPU_DEBUG=1`:
        // pass labels and debug groups go to the driver as markers, so a GPU
        // profiler like Nsight times each one); unset, its defaults.
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());

        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter found");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rts-core-engine"),
                // BC sampling (when offered) lets unit textures upload as stored.
                required_features: (adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC) | super::gpu_prof::features(&adapter, options.gpu_passes),
                // The terrain shader samples 33 textures (FA's 10 albedo + 9
                // normal strata, masks, water, ramp, waves, sky, the normal
                // buffer, sun shadows, horizon): above the default 16.
                required_limits: wgpu::Limits {
                    max_sampled_textures_per_shader_stage: adapter.limits().max_sampled_textures_per_shader_stage.min(64),
                    ..wgpu::Limits::default()
                },
                ..Default::default()
            })
            .await
            .expect("failed to create device");
        // For bug reports: the GPU and driver, and every graphics error
        // (logged, not a crash: wgpu's default would abort the game).
        let info = adapter.get_info();
        tracing::info!(
            gpu = %info.name,
            kind = ?info.device_type,
            backend = ?info.backend,
            driver = %info.driver,
            driver_info = %info.driver_info,
            vendor = format!("{:04x}", info.vendor),
            device = format!("{:04x}", info.device),
            "graphics"
        );
        if let Some(m) = window.current_monitor() {
            let s = m.size();
            tracing::info!(monitor = m.name().unwrap_or_default(), width = s.width, height = s.height, refresh_hz = m.refresh_rate_millihertz().map_or(0, |r| r / 1000), scale = window.scale_factor(), "screen");
        }
        let errors = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
            // The first few in full, then one in a hundred (one a frame
            // would drown the log).
            let n = errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 10 || n % 100 == 0 {
                tracing::error!(target: "graphics", count = n + 1, "graphics error: {e}");
            }
        }));
        device.set_device_lost_callback(|reason, message| tracing::error!(target: "graphics", ?reason, "graphics device lost: {message}"));

        let surface_caps = surface.get_capabilities(&adapter);
        let wayland = is_wayland(&window);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|&&f| f == wgpu::TextureFormat::Bgra8UnormSrgb)
            .copied()
            .unwrap_or_else(|| {
                surface_caps
                    .formats
                    .iter()
                    .find(|f| f.is_srgb())
                    .copied()
                    .unwrap_or(surface_caps.formats[0])
            });

        // COPY_SRC (when offered) lets `--screenshot` read the frame back.
        let copy = surface_caps.usages.contains(wgpu::TextureUsages::COPY_SRC);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | if copy { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: present_mode(true, &surface_caps.present_modes, wayland),
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::viewport::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            Some(2048),
        );
        let scene = super::scene::Scene::new(&device, &queue, surface_format, surface_config.width, surface_config.height);
        let world = super::world::World::new(surface_config.width, surface_config.height);
        let settings = super::world::RenderSettings { smaa: options.aa, ..Default::default() };
        let ui_renderer = super::ui::UiRenderer::new(&device, surface_format);
        let presenter = super::present::Presenter::new(device.clone(), queue.clone(), Some(surface), surface_config.clone(), scene, ui_renderer, options.screenshot.is_some(), options.gpu_passes);
        // The window draws on its own thread, paced by its swapchain; each
        // frame it starts asks this (the main) thread for the next.
        let ready = proxy.clone();
        let presenter = super::present::PresentThread::spawn(presenter, "present", move || {
            let _ = ready.send_event(super::app::UserEvent::PresentReady(0));
        });
        Self {
            window,
            device,
            surface_config,
            egui_ctx,
            egui_state,
            collector,
            debug_ui,
            presenter,
            present_out: Default::default(),
            ui_images: Default::default(),
            world,
            settings,
            frame_input: Default::default(),
            snapshots,
            input: Input { cursor: options.cursor, ..Input::default() },
            last_frame: std::time::Instant::now(),
            frame_interval: 1.0 / 60.0,
            frame_times: Percentiles::new(600),
            cpu_frame_ms: 0.0,
            cpu_times: Percentiles::new(600),
            gpu_frame_ms: 0.0,
            memory: Default::default(),
            slow_frames: rts_core_engine::diagnostics::Hitches::new("frame"),
            tick_times: Percentiles::new(300),
            timed_tick: 0,
            screenshot: options.screenshot,
            exit_when_sim_ends: options.exit_when_sim_ends,
            camera_override: options.camera,
            request,
            control,
            player: super::player::PlayerControl::new(options.player, options.select.clone(), options.orders),
            ui: None,
            ui_booting: None,
            ui_failed: false,
            ui_tick: 0,
            ui_buttons: 0,
            ui_selection: Vec::new(),
            ui_alliances: None,
            ui_army_display: None,
            ui_custom_names: None,
            ui_team_colors: 0,
            ui_shift: false,
            build_drag: None,
            patrol_convert: None,
            patrol_converting: false,
            hover_command: None,
            command_drag: None,
            command_moved: None,
            tracks: Vec::new(),
            projectile_follows: Vec::new(),
            camera_moves: Vec::new(),
            camera_waiting: Vec::new(),
            zoom_anchor: None,
            ui_resized: false,
            scale_factor,
            ui_items: Vec::new(),
            minimap_rect: None,
            minimap: Default::default(),
            minimap_dots: Vec::new(),
            minimap_dots_tick: u64::MAX,
            minimap_dots_size: (0, 0),
            pending_select: options.select,
            pending_ui_lua: options.ui_lua,
            replay: options.replay.is_some(),
            timeline: options.replay.as_deref().and_then(read_timeline),
            seek: None,
            keyframes: options.replay.as_deref().and_then(Keyframes::for_replay),
            front_end: options.front_end,
            launch: options.launch.clone(),
            carried_prefs: None,
            no_aa: !options.aa,
            fixed_size: options.window_size.is_some(),
            hold_shift: options.hold_shift,
            profile: Default::default(),
            rollover: None,
            show_debug: false,
            show_status: true,
            ui_cursor: rts_core_engine::ui::cursor::Wanted::System,
            os_cursor: rts_core_engine::ui::cursor::Wanted::System,
            os_cursors: Default::default(),
            prefs: options.prefs.clone(),
            present_modes: surface_caps.present_modes.clone(),
            min_frame: std::time::Duration::ZERO,
            cursor_clip: false,
            spinning: None,
            cam_free: false,
            display_mode: "windowed".into(),
            primary_display: String::new(),
            secondary_display: String::new(),
            displays_saved: false,
            gpu: Gpu { instance, adapter, queue, textures: Default::default() },
            second: None,
            second_wanted: None,
            second_decided: false,
            second_stash: Vec::new(),
            second_head_flag: options.second_head.clone(),
            cursor_head: 0,
            vsync: true,
            wayland,
            visibility: Visibility::default(),
            proxy,
        }
    }

    /// Physical pixels per logical (interface) pixel.
    pub fn ui_scale(&self) -> f32 {
        self.scale_factor.max(0.25)
    }

    /// A point on the cursor's head's window (physical pixels) where the
    /// interface sees it (logical pixels): every position handed to the
    /// user VM goes through this.
    pub fn ui_point(&self, x: f64, y: f64) -> (f32, f32) {
        let scale = self.head_scale(self.cursor_head);
        (x as f32 / scale, y as f32 / scale)
    }

    /// FAF's Vertical Sync option (`SC_VerticalSync`): the surface
    /// presents in step with the display, or as soon as a frame is ready.
    pub fn set_vsync(&mut self, on: bool) {
        let mode = present_mode(on, &self.present_modes, self.wayland);
        if mode != self.surface_config.present_mode || on != self.vsync {
            self.vsync = on;
            self.surface_config.present_mode = mode;
            self.command_heads(|| super::present::Command::PresentMode { mode, vsync: on });
            self.pace_main();
        }
    }

    /// Keeps the cursor inside the window while it's fullscreen with the
    /// Lock Fullscreen Cursor To Window option on (re-applied when the
    /// window regains focus or changes mode: the OS drops it). macOS can't
    /// confine the cursor (winit: only lock it in place), so there it stays
    /// free.
    pub fn apply_cursor_clip(&self) {
        use winit::window::CursorGrabMode;
        let mode = if self.cursor_clip && self.window.fullscreen().is_some() { CursorGrabMode::Confined } else { CursorGrabMode::None };
        if let Err(e) = self.window.set_cursor_grab(mode) {
            tracing::debug!(?mode, "cursor clip: {e}");
        }
    }

    /// FAF's Primary Adapter (`SC_PrimaryAdapter`): `windowed`, or
    /// fullscreen at `width,height,refresh`: always borderless over the
    /// window's monitor, at the desktop's mode. Never exclusive: on Windows
    /// winit's exclusive fullscreen changes the display mode and keeps the
    /// window always-on-top, so Alt-Tab can't bring anything in front of it.
    pub fn set_display_mode(&mut self, value: &str) {
        let value = value.trim().to_ascii_lowercase();
        if value == self.display_mode || self.fixed_size {
            return;
        }
        let fullscreen = parse_mode(&value).map(|_| winit::window::Fullscreen::Borderless(self.primary_monitor()));
        tracing::info!(mode = value, "display mode");
        self.window.set_fullscreen(fullscreen);
        self.display_mode = value;
        self.apply_cursor_clip();
    }

    /// The Primary Display's fullscreen modes as (width, height, refresh
    /// Hz), largest first, one per size and rate (FAF's Primary Adapter
    /// list).
    pub fn display_modes(&self) -> Vec<(u32, u32, u32)> {
        let Some(monitor) = self.primary_monitor() else { return Vec::new() };
        let mut modes: Vec<(u32, u32, u32)> = monitor.video_modes().map(|v| (v.size().width, v.size().height, (v.refresh_rate_millihertz() + 500) / 1000)).collect();
        modes.sort_unstable_by(|a, b| b.cmp(a));
        modes.dedup();
        modes
    }

    /// Sends a command to every head's presenter.
    pub fn command_heads(&self, make: impl Fn() -> super::present::Command) {
        self.presenter.command(make());
        if let Some(h) = &self.second {
            h.presenter.command(make());
        }
    }

    /// Physical pixels per logical pixel on head `head`.
    pub fn head_scale(&self, head: usize) -> f32 {
        match (head, &self.second) {
            (1, Some(h)) => h.scale_factor.max(0.25),
            _ => self.ui_scale(),
        }
    }

    /// Decides on FAF's second head, once (moho reads Secondary Adapter as
    /// it makes the device): `--second-head`, else `option` (the profile's
    /// Secondary Adapter). A fullscreen head goes on a monitor other than
    /// the main window's (none: no second head). Its size is known now (the
    /// interface lays out root frame 1 before the window is made).
    pub fn decide_second_head(&mut self, option: Option<&str>) {
        if std::mem::replace(&mut self.second_decided, true) {
            return;
        }
        let stash = std::mem::take(&mut self.second_stash);
        tracing::info!(option, flag = ?self.second_head_flag, "secondary adapter");
        let Some(spec) = self.second_head_flag.clone().or_else(|| option.and_then(SecondHeadSpec::from_option)) else { return };
        let scale = self.ui_scale();
        let (w, h, scale) = match &spec {
            SecondHeadSpec::Offscreen => (1280, 720, scale),
            SecondHeadSpec::Windowed => ((1024.0 * scale) as u32, (768.0 * scale) as u32, scale),
            SecondHeadSpec::Mode(..) => match self.other_monitor() {
                Some(m) => (m.size().width, m.size().height, m.scale_factor() as f32),
                None => {
                    tracing::warn!(?spec, "second head: no second monitor (it stays off)");
                    return;
                }
            },
        };
        tracing::info!(?spec, width = w, height = h, scale, "second head");
        self.second_stash = stash;
        self.second_wanted = Some((spec, w, h, scale));
    }

    /// The fullscreen modes of a monitor other than the main window's
    /// (FAF's Secondary Adapter list; none with one monitor).
    pub fn second_display_modes(&self) -> Vec<(u32, u32, u32)> {
        let Some(monitor) = self.other_monitor() else { return Vec::new() };
        let mut modes: Vec<(u32, u32, u32)> = monitor.video_modes().map(|v| (v.size().width, v.size().height, (v.refresh_rate_millihertz() + 500) / 1000)).collect();
        modes.sort_unstable_by(|a, b| b.cmp(a));
        modes.dedup();
        modes
    }

    /// The second head's monitor: the Secondary Display, else the first
    /// other than the Primary Display (none: one monitor).
    fn other_monitor(&self) -> Option<winit::monitor::MonitorHandle> {
        let primary = self.primary_monitor();
        let others: Vec<(String, winit::monitor::MonitorHandle)> = monitors(&self.window).into_iter().filter(|(_, m)| Some(m) != primary.as_ref()).collect();
        others.iter().find(|(k, _)| *k == self.secondary_display).or(others.first()).map(|(_, m)| m.clone())
    }

    /// The Primary Display's monitor (unset or gone: the system's main
    /// display, else the window's).
    pub fn primary_monitor(&self) -> Option<winit::monitor::MonitorHandle> {
        monitors(&self.window)
            .into_iter()
            .find(|(k, _)| !self.primary_display.is_empty() && *k == self.primary_display)
            .map(|(_, m)| m)
            .or_else(|| self.window.primary_monitor())
            .or_else(|| self.window.current_monitor())
    }

    /// The Primary Display option is `key`: the main window moves there
    /// (centred; fullscreen again on it).
    pub fn set_primary_display(&mut self, key: &str) {
        let key = if key == DEFAULT_DISPLAY { "" } else { key };
        if key == self.primary_display {
            return;
        }
        self.primary_display = key.to_owned();
        let Some(monitor) = self.primary_monitor() else { return };
        tracing::info!(monitor = monitor.name().unwrap_or_default(), "primary display");
        if self.window.fullscreen().is_some() {
            let mode = std::mem::take(&mut self.display_mode);
            self.set_display_mode(&mode);
        } else {
            self.window.set_outer_position(centred(&monitor, self.window.outer_size()));
        }
    }

    /// Makes the second head decided on (`decide_second_head`): its window
    /// (borderless fullscreen on its monitor, a plain window, or none), its
    /// surface on the same device, its scene and present thread. Both
    /// heads' models load their textures through one cache from then on.
    pub fn make_second_head(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let Some((spec, w, h, scale)) = self.second_wanted.take() else { return };
        let attributes = Window::default_attributes().with_title("RTS Core Engine (second head)").with_active(false);
        let window = match &spec {
            SecondHeadSpec::Offscreen => None,
            // On its monitor when there is another (else beside the main
            // window, as the system places it).
            SecondHeadSpec::Windowed => Some(match self.other_monitor() {
                Some(m) => attributes.with_inner_size(winit::dpi::PhysicalSize::new(w, h)).with_position(centred(&m, winit::dpi::PhysicalSize::new(w, h))),
                None => attributes.with_inner_size(winit::dpi::PhysicalSize::new(w, h)),
            }),
            SecondHeadSpec::Mode(mw, mh, hz) => {
                let Some(monitor) = self.other_monitor() else { return };
                if (monitor.size().width, monitor.size().height) != (*mw, *mh) {
                    tracing::info!(monitor = monitor.name().unwrap_or_default(), width = monitor.size().width, height = monitor.size().height, asked = format!("{mw}x{mh}@{hz}"), "second head: the monitor's own mode kept (borderless)");
                }
                // Borderless over the monitor; not native fullscreen (on macOS
                // that is a Space of its own, and depends on "Displays have
                // separate Spaces").
                let a = attributes.with_decorations(false).with_position(monitor.position()).with_inner_size(monitor.size());
                #[cfg(not(target_os = "macos"))]
                let a = a.with_fullscreen(Some(winit::window::Fullscreen::Borderless(Some(monitor))));
                Some(a)
            }
        };
        let window = match window.map(|a| event_loop.create_window(a)).transpose() {
            Ok(w) => w.map(Arc::new),
            Err(e) => {
                tracing::error!("second head: no window: {e}");
                return;
            }
        };
        #[cfg(target_os = "macos")]
        if let (Some(win), SecondHeadSpec::Mode(..)) = (&window, &spec) {
            use winit::platform::macos::WindowExtMacOS;
            win.set_simple_fullscreen(true);
        }
        let (w, h, scale) = match &window {
            Some(win) => (win.inner_size().width.max(1), win.inner_size().height.max(1), win.scale_factor() as f32),
            None => (w, h, scale),
        };
        let surface = match window.as_ref().map(|win| self.gpu.instance.create_surface(win.clone())).transpose() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("second head: no surface: {e}");
                return;
            }
        };
        let mut config = wgpu::SurfaceConfiguration { width: w, height: h, ..self.surface_config.clone() };
        if let Some(surface) = &surface {
            let caps = surface.get_capabilities(&self.gpu.adapter);
            if !caps.formats.contains(&config.format) {
                config.format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
            }
            if !caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
                config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
            }
            config.alpha_mode = caps.alpha_modes[0];
            if !caps.present_modes.contains(&config.present_mode) {
                config.present_mode = present_mode(self.vsync, &caps.present_modes, self.wayland);
            }
            surface.configure(&self.device, &config);
        }
        let queue = &self.gpu.queue;
        let mut scene = super::scene::Scene::new(&self.device, queue, config.format, w, h);
        scene.stage.models.share_textures(self.gpu.textures.clone());
        let ui_renderer = super::ui::UiRenderer::new(&self.device, config.format);
        // (Pass timing is the main head's: the heads would share its queries.)
        let mode = config.present_mode;
        let presenter = super::present::Presenter::new(self.device.clone(), queue.clone(), surface, config, scene, ui_renderer, self.screenshot.is_some(), false);
        let proxy = self.proxy.clone();
        let presenter = super::present::PresentThread::spawn(presenter, "present (head 2)", move || {
            let _ = proxy.send_event(super::app::UserEvent::PresentReady(1));
        });
        presenter.command(super::present::Command::MinFrame(self.min_frame));
        presenter.command(super::present::Command::PresentMode { mode, vsync: self.vsync });
        presenter.command(super::present::Command::VsyncPeriod(window.as_ref().and_then(|w| vsync_period(w))));
        let paced = window.is_some();
        if let Some(vfs) = self.world.models_vfs() {
            presenter.command(super::present::Command::UiVfs(vfs));
        }
        self.presenter.command(super::present::Command::ShareTextures(self.gpu.textures.clone()));
        self.world.resize_head(1, w, h);
        // `--screenshot x.png`: head 2 saved first, as `x-head2.png`.
        let capture = self.screenshot.as_ref().map(|(p, _)| {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            p.with_file_name(format!("{stem}-head2.png"))
        });
        tracing::info!(?spec, width = w, height = h, windowed = window.is_some(), "second head made");
        self.second = Some(SecondHead {
            window,
            presenter,
            size: (w, h),
            scale_factor: scale,
            deltas: std::mem::take(&mut self.second_stash),
            ui_items: Vec::new(),
            capture,
            capture_requested: false,
            capture_sent: false,
            captured: false,
            paced,
            cpu_ms: 0.0,
            main_ms: 0.0,
            frame_interval: 1.0 / 60.0,
            egui_ctx: egui::Context::default(),
            last_frame: std::time::Instant::now(),
            visibility: Visibility::default(),
        });
        self.pace_main();
    }

    /// The main window paces its frames by its monitor's refresh while a
    /// second head with a window shares the device (`Presenter::pace`: each
    /// head's wait for a drawable otherwise holds up the other's submits);
    /// alone, it leaves vsync to wgpu.
    pub fn pace_main(&self) {
        let second_paced = self.second.as_ref().is_some_and(|h| h.paced);
        let period = pace_period(self.vsync, self.surface_config.present_mode, second_paced, vsync_period(&self.window), fallback_period(&self.window));
        self.presenter.command(super::present::Command::VsyncPeriod(period));
    }

    /// Tells each head's present thread whether its window is hidden
    /// (`Command::Hidden`). A hidden main window still runs the interface,
    /// which a shown second head's picture and input need: at that head's
    /// refresh then, else at [`super::present::HIDDEN_FRAME`].
    pub fn apply_visibility(&self) {
        use super::present::{Command, HIDDEN_FRAME};
        let second_shown = self.second.as_ref().filter(|h| h.window.is_some() && !h.visibility.hidden());
        let main_interval = match second_shown {
            Some(h) => h.window.as_deref().and_then(vsync_period).unwrap_or(super::present::OFFSCREEN_FRAME),
            None => HIDDEN_FRAME,
        };
        self.presenter.command(Command::Hidden(self.visibility.hidden().then_some(main_interval)));
        if let Some(h) = &self.second {
            h.presenter.command(Command::Hidden(h.visibility.hidden().then_some(HIDDEN_FRAME)));
        }
    }

    /// The second head's window resized.
    pub fn resize_second(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        let Some(h) = self.second.as_mut().filter(|_| size.width > 0 && size.height > 0) else { return };
        h.size = (size.width, size.height);
        h.presenter.command(super::present::Command::Resize(size.width, size.height));
        self.world.resize_head(1, size.width, size.height);
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.presenter.command(super::present::Command::Resize(size.width, size.height));
        self.world.resize(size.width, size.height);
        self.ui_resized = true;
    }
}

/// One refresh of the monitor `window` is on (a second head paces its
/// frames by it: `Presenter::pace`).
/// A replay seek under way (`Replay.Seek`): fast-forwarding to `target`.
#[derive(Clone, Debug)]
pub struct Seek {
    pub target: u64,
    /// The world session (`World::session`) whose snapshots count: the
    /// next one when the seek restarted the replay.
    pub session: u64,
    /// The speed to carry on at once there.
    pub speed: u32,
    /// Where the main camera then looks.
    pub focus: Option<[f32; 3]>,
}

/// A watched replay's keyframe folder, listed now and then.
pub struct Keyframes {
    pub store: rts_core_engine::engine::keyframe::store::Store,
    /// The ticks with a keyframe, ascending (as last listed).
    pub ticks: std::sync::Arc<Vec<u64>>,
    pub listed: Option<std::time::Instant>,
}

impl Keyframes {
    /// The folder of the replay at `path` (`None`: no keyframes folder).
    pub fn for_replay(path: &std::path::Path) -> Option<Self> {
        use rts_core_engine::engine::keyframe::{recorder, store::Store};
        let dir = recorder::dir_for_replay(&recorder::root()?, path).ok()?;
        Some(Self { store: Store::new(dir, recorder::FULL_EVERY), ticks: Default::default(), listed: None })
    }

    /// Lists the folder again if a second has passed.
    pub fn refresh(&mut self) -> bool {
        if self.listed.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1)) {
            return false;
        }
        self.listed = Some(std::time::Instant::now());
        let mut ticks: Vec<u64> = self.store.entries().iter().map(|e| e.tick).collect();
        ticks.dedup();
        if ticks != *self.ticks {
            self.ticks = std::sync::Arc::new(ticks);
        }
        true
    }

    /// The latest keyframe at or before `tick`.
    pub fn at_or_before(&self, tick: u64) -> Option<u64> {
        self.ticks.iter().rev().find(|&&k| k <= tick).copied()
    }
}

/// A recording's timeline (`None` if it can't be read).
pub fn read_timeline(path: &std::path::Path) -> Option<std::sync::Arc<rts_core_engine::engine::replay::Timeline>> {
    let path = rts_core_engine::engine::replay::find_file(path);
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(std::sync::Arc::new(rts_core_engine::engine::replay::read_timeline(&text))),
        Err(e) => {
            tracing::warn!(path = %path.display(), "replay timeline: {e}");
            None
        }
    }
}

pub fn vsync_period(window: &Window) -> Option<std::time::Duration> {
    window.current_monitor().and_then(refresh_period)
}

/// One refresh of `monitor` (winit gives the rate in millihertz).
fn refresh_period(monitor: winit::monitor::MonitorHandle) -> Option<std::time::Duration> {
    let millihertz = monitor.refresh_rate_millihertz().filter(|&r| r > 0)?;
    Some(std::time::Duration::from_secs_f64(1000.0 / millihertz as f64))
}

/// A refresh period to pace by when the window's monitor isn't known yet
/// (Wayland has none until the surface has entered an output, a few frames
/// in): the primary monitor's, else the shortest of the monitors'.
fn fallback_period(window: &Window) -> Option<std::time::Duration> {
    window.primary_monitor().and_then(refresh_period).or_else(|| window.available_monitors().filter_map(refresh_period).min())
}

/// Pacing at 60 Hz when no monitor gives its refresh.
const DEFAULT_PERIOD: std::time::Duration = std::time::Duration::from_nanos(1_000_000_000 / 60);

/// The period the present thread paces the main window's frames by. Mailbox
/// (Wayland vsync) has no wait in the acquire, so it is never left unpaced:
/// the window's monitor's period, else `fallback`, else 60 Hz. With a paced
/// second head the pace is the monitor's alone (`None` if unknown). Fifo
/// alone leaves vsync to wgpu.
pub fn pace_period(vsync: bool, mode: wgpu::PresentMode, second_paced: bool, current: Option<std::time::Duration>, fallback: Option<std::time::Duration>) -> Option<std::time::Duration> {
    if vsync && mode == wgpu::PresentMode::Mailbox {
        return Some(current.or(fallback).unwrap_or(DEFAULT_PERIOD));
    }
    second_paced.then_some(current).flatten()
}

/// `window` is a Wayland surface.
fn is_wayland(window: &Window) -> bool {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window.window_handle().is_ok_and(|h| matches!(h.as_raw(), RawWindowHandle::Wayland(_)))
}

/// Whether a window can be seen: winit says when one is fully covered
/// (macOS, X11; `Occluded`) and a minimised one is resized to nothing
/// (Windows). Wayland says neither: there Mailbox keeps a hidden window's
/// present from waiting ([`present_mode`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct Visibility {
    pub occluded: bool,
    pub minimised: bool,
}

impl Visibility {
    pub fn hidden(&self) -> bool {
        self.occluded || self.minimised
    }
}

/// The Primary / Secondary Display options' key for the default monitor.
pub const DEFAULT_DISPLAY: &str = "default";

/// The monitors as the display options name them, in the system's order:
/// each by its name (a second of the same name gets ` #2`, and so on).
pub fn monitors(window: &Window) -> Vec<(String, winit::monitor::MonitorHandle)> {
    let mut out: Vec<(String, winit::monitor::MonitorHandle)> = Vec::new();
    for m in window.available_monitors() {
        let name = m.name().unwrap_or_else(|| "Display".into());
        let same = out.iter().filter(|(k, _)| *k == name || k.starts_with(&format!("{name} #"))).count();
        out.push((if same == 0 { name } else { format!("{name} #{}", same + 1) }, m));
    }
    out
}

/// Where a window of `size` (physical) goes to sit in the middle of
/// `monitor`.
pub fn centred(monitor: &winit::monitor::MonitorHandle, size: winit::dpi::PhysicalSize<u32>) -> winit::dpi::PhysicalPosition<i32> {
    let (p, m) = (monitor.position(), monitor.size());
    let half = |a: u32, b: u32| (a as i32 - b as i32).max(0) / 2;
    winit::dpi::PhysicalPosition::new(p.x + half(m.width, size.width), p.y + half(m.height, size.height))
}

/// A Primary Adapter value `width,height,refresh` (`None`: windowed or
/// unreadable).
pub fn parse_mode(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.split(',').map(|p| p.trim().parse::<u32>().ok());
    let (w, h, hz) = (parts.next()??, parts.next()??, parts.next()??);
    (w > 0 && h > 0).then_some((w, h, hz))
}

/// The present mode for vsync `on` (Fifo, which every surface offers), or
/// off: the first the surface offers of Immediate (no wait, may tear) and
/// Mailbox (no wait, the newest frame shown at the next refresh).
///
/// On Wayland vsync is Mailbox when offered (the present thread paces it,
/// `Presenter::pace`): a compositor sends no frame callbacks to a covered
/// window, and Mesa's Fifo present waits for one, so a hidden window's
/// present waits until it is shown, holding the device both heads share,
/// and winit can't say a Wayland window is covered.
pub fn present_mode(on: bool, offered: &[wgpu::PresentMode], wayland: bool) -> wgpu::PresentMode {
    use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
    if on {
        return if wayland && offered.contains(&Mailbox) { Mailbox } else { Fifo };
    }
    [Immediate, Mailbox].into_iter().find(|m| offered.contains(m)).unwrap_or(Fifo)
}

/// A rolling window of samples (the last `cap`) and its percentiles.
pub struct Percentiles {
    samples: std::collections::VecDeque<f32>,
    cap: usize,
}

impl Percentiles {
    pub fn new(cap: usize) -> Self {
        Self { samples: std::collections::VecDeque::with_capacity(cap), cap }
    }

    pub fn push(&mut self, v: f32) {
        if self.samples.len() == self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(v);
    }

    /// The `q` (0..1) percentile of the window, 0 when empty.
    pub fn get(&self, q: f32) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut v: Vec<f32> = self.samples.iter().copied().collect();
        v.sort_by(f32::total_cmp);
        v[((v.len() - 1) as f32 * q).round() as usize]
    }
}


#[cfg(test)]
mod tests {
    use super::{pace_period, present_mode};
    use wgpu::PresentMode::{AutoVsync, Fifo, Immediate, Mailbox};

    /// Vertical Sync on is Fifo; off, Immediate where offered, else Mailbox,
    /// else (a surface with neither) Fifo.
    /// Primary Adapter values: `w,h,hz` is a fullscreen mode, anything
    /// else (`windowed`, FAF's `overridden`) none.
    #[test]
    fn display_mode_values() {
        use super::parse_mode;
        assert_eq!(parse_mode("1920,1080,60"), Some((1920, 1080, 60)));
        assert_eq!(parse_mode("windowed"), None);
        assert_eq!(parse_mode("overridden"), None);
        assert_eq!(parse_mode("0,768,60"), None);
    }

    #[test]
    fn secondary_adapter_values() {
        use super::SecondHeadSpec;
        assert_eq!(SecondHeadSpec::from_option("windowed"), Some(SecondHeadSpec::Windowed));
        assert_eq!(SecondHeadSpec::from_option("1920,1080,60"), Some(SecondHeadSpec::Mode(1920, 1080, 60)));
        assert_eq!(SecondHeadSpec::from_option("disabled"), None);
        assert_eq!(SecondHeadSpec::from_option("overridden"), None);
        // Only `--second-head` draws offscreen.
        assert_eq!(SecondHeadSpec::from_option("offscreen"), None);
    }

    #[test]
    fn vsync_picks_a_present_mode() {
        assert_eq!(present_mode(true, &[Fifo, Immediate, Mailbox], false), Fifo);
        assert_eq!(present_mode(false, &[Fifo, Mailbox, Immediate], false), Immediate);
        assert_eq!(present_mode(false, &[Fifo, Mailbox], false), Mailbox);
        assert_eq!(present_mode(false, &[Fifo, AutoVsync], false), Fifo);
        // Wayland: vsync by Mailbox when offered.
        assert_eq!(present_mode(true, &[Fifo, Immediate, Mailbox], true), Mailbox);
        assert_eq!(present_mode(true, &[Fifo, Immediate], true), Fifo);
        assert_eq!(present_mode(false, &[Fifo, Mailbox, Immediate], true), Immediate);
    }

    /// A frame's worth of mouse motion is one event at the latest position
    /// (a high-polling mouse sends many), but events in between still order
    /// the ones around them.
    #[test]
    fn cursor_moves_coalesce_until_another_event() {
        use super::Input;
        use crate::render::player::{Event, Mods};
        let mut input = Input::default();
        input.cursor_moved(1.0, 2.0, Mods::default());
        input.cursor_moved(3.0, 4.0, Mods { shift: true, ..Default::default() });
        assert_eq!(input.events.len(), 1);
        let Some((Event::Move(x, y), m)) = input.events.first().copied() else { panic!("not a move") };
        assert_eq!((x, y), (3.0, 4.0));
        assert!(m.shift);
        input.events.push((Event::LeftDown(3.0, 4.0), Mods::default()));
        input.cursor_moved(5.0, 6.0, Mods::default());
        assert_eq!(input.events.len(), 3);
        let Some((Event::Move(x, y), _)) = input.events.last().copied() else { panic!("not a move") };
        assert_eq!((x, y), (5.0, 6.0));
    }

    /// Wayland's Mailbox has no wait in the acquire, so it is always paced,
    /// even before the window's monitor is known.
    #[test]
    fn mailbox_vsync_is_paced_without_a_current_monitor() {
        use std::time::Duration;
        let hz = |r: f64| Some(Duration::from_secs_f64(1.0 / r));
        let ms = |d: Option<Duration>| d.map(|d| d.as_secs_f64() * 1000.0);
        assert!((ms(pace_period(true, Mailbox, false, None, hz(144.0))).unwrap() - 1000.0 / 144.0).abs() < 1e-6);
        assert!((ms(pace_period(true, Mailbox, false, None, None)).unwrap() - 1000.0 / 60.0).abs() < 1e-6);
        assert!((ms(pace_period(true, Mailbox, false, hz(75.0), hz(144.0))).unwrap() - 1000.0 / 75.0).abs() < 1e-6);
        // Off, or Fifo alone: unpaced.
        assert_eq!(pace_period(false, Mailbox, false, hz(60.0), hz(60.0)), None);
        assert_eq!(pace_period(true, Fifo, false, hz(60.0), hz(60.0)), None);
        // A paced second head still paces by the monitor.
        assert!(pace_period(true, Fifo, true, hz(60.0), None).is_some());
    }
}
