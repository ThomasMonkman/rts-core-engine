mod app;
/// The render thread's own rayon pool: the prepare worker runs on it and
/// its parallel passes (interpolation, posing, particle placement and
/// emission) split over it. The global pool is the sim's (and background
/// loading's: scripts, models, effects, textures): in a big battle the sim
/// keeps it full for most of every tick, and rayon tasks run to completion,
/// so frame work queued there would wait behind the sim's stages.
pub fn pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::LazyLock<rayon::ThreadPool> = std::sync::LazyLock::new(|| {
        let threads = (rts_core_engine::threads::cores() / 3).clamp(2, 6);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("render worker {i}"))
            .start_handler(|_| {
                rts_core_engine::engine::tracy::name_current();
                prioritise_render_thread();
            })
            .build()
            .expect("render pool")
    });
    &POOL
}

/// Opts the process out of the OS's background throttling for its lifetime.
/// Without it a window that isn't frontmost (alt-tabbed, covered, or
/// started from a terminal) gets the whole process throttled, which in a
/// lockstep game slows every player:
/// - macOS (App Nap): the process is demoted to the background tier, every
///   sim worker drops to priority 0 on throttled efficiency cores and ticks
///   take about twice as long;
/// - Windows 11 (power throttling / EcoQoS): the process can be moved to
///   efficiency cores at low clocks, and its timer resolution requests are
///   ignored (sleeps round up to ~15 ms, uneven real-time pacing).
pub fn keep_awake() {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical,
            &NSString::from_str("running the game"),
        );
        // Ended only by the process ending.
        std::mem::forget(activity);
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
        };
        // A bit set in the control mask and clear in the state mask turns
        // that throttling off. Windows 10 rejects the timer flag (it came in
        // Windows 11), so fall back to execution speed alone.
        let opt_out = |mask: u32| {
            let state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: mask, StateMask: 0 };
            // SAFETY: the current process's pseudo-handle and a live struct of the size passed.
            unsafe {
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&raw const state).cast(),
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                ) != 0
            }
        };
        if !opt_out(PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION)
            && !opt_out(PROCESS_POWER_THROTTLING_EXECUTION_SPEED)
        {
            tracing::warn!("couldn't opt out of power throttling: the game may slow down when not in front");
        }
    }
}

/// Puts the calling thread ahead of the sim's workers for the CPU (macOS:
/// user-interactive QoS; Windows: above-normal priority; Linux: nothing
/// without privileges). The sim keeps every core busy in a big battle and
/// rayon tasks run to completion, so a render worker that loses its core
/// mid-pass costs a whole frame (60-90 ms stalls in the pose pass and the
/// beam walk were seen with the sim at 250 ms a tick).
pub fn prioritise_render_thread() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets the calling thread's own QoS class; no pointers.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
    #[cfg(windows)]
    // SAFETY: the calling thread's pseudo-handle.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

/// A view of all of `texture`, labelled (wgpu doesn't carry a texture's
/// label to its views; captures list them separately).
pub(crate) fn view(texture: &wgpu::Texture, label: &str) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor { label: Some(label), ..Default::default() })
}

pub mod bloom;
pub mod camera;
pub mod command_graph;
pub mod console;
pub mod debug_lines;
pub mod frame;
pub mod globals;
pub mod gpu_prof;
pub mod memory;
pub mod models;
pub mod overlays;
pub mod selection_box;
pub mod particles;
pub mod player;
pub mod present;
pub mod ranges;
pub mod resource_icons;
#[cfg(test)]
mod render_tests;
pub mod scene;
pub mod shadows;
pub mod sky;
pub mod smaa;
pub mod state;
pub mod status_bar;
pub mod terrain;
pub mod texture_load;
pub mod ui;
pub mod vision;
pub mod world;

use crate::debug_ui::DebugUiState;
use std::sync::{Arc, Mutex};
use winit::event_loop::{ControlFlow, EventLoop};

/// Renderer start-up options (from the command line).
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Save a frame here once the sim reaches the tick, then exit.
    pub screenshot: Option<(std::path::PathBuf, u64)>,
    /// `--ticks` in a window: close once the sim has run them and gone
    /// (a screenshot still to come is taken first).
    pub exit_when_sim_ends: bool,
    /// Initial camera (x, z, distance).
    pub camera: Option<(f32, f32, f32)>,
    /// Army the player commands (`--player`); `None` = observer who may
    /// order any army's units.
    pub player: Option<u32>,
    /// Entities selected at start (`--select`).
    pub select: Vec<u32>,
    /// Anti-aliasing (SMAA) on the finished frame (`--no-aa` turns it off).
    pub aa: bool,
    /// `--gpu-passes`: GPU time of every pass and debug group, logged.
    pub gpu_passes: bool,
    /// `--window-size`: the main window's size in pixels.
    pub window_size: Option<(u32, u32)>,
    /// `--hold-shift`: Shift counts as held.
    pub hold_shift: bool,
    /// `--cursor`: where the cursor starts, window pixels.
    pub cursor: Option<(f64, f64)>,
    /// Interface Lua to run once it is up (`--ui-lua`).
    pub ui_lua: Option<String>,
    /// Watching a recording (`--replay`): the interface's `SessionIsReplay`
    /// and the replay timeline's events.
    pub replay: Option<std::path::PathBuf>,
    /// No session: the interface is the main menu (moho's front end).
    pub front_end: bool,
    /// Where the front end's lobby sends its launch (the engine thread
    /// starts that session).
    pub launch: Option<std::sync::mpsc::Sender<crate::engine::session::Launch>>,
    /// The `Game.prefs` the interface loads and saves to
    /// ([`rts_core_engine::install::choose_prefs`]).
    pub prefs: rts_core_engine::install::Prefs,
    /// `--second-head`: FAF's second head, whatever the profile says.
    pub second_head: Option<state::SecondHeadSpec>,
    /// Where player orders go (the sim's command stream).
    pub orders: Option<std::sync::mpsc::Sender<crate::engine::sim::input::PlayerCommand>>,
}

pub fn run(
    collector: egui_tracing::EventCollector,
    debug_ui: Arc<Mutex<DebugUiState>>,
    snapshots: std::sync::mpsc::Receiver<crate::engine::snapshot::Snapshot>,
    request: crate::engine::snapshot::SharedDebugRequest,
    control: std::sync::Arc<crate::engine::snapshot::SimControl>,
    options: Options,
) {
    // `--screenshot` runs are unattended: don't steal focus from whatever
    // the user is typing in.
    let unattended = options.screenshot.is_some();
    let mut builder = EventLoop::<app::UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_activate_ignoring_other_apps(!unattended);
    }
    let _ = unattended;
    let event_loop = builder.build().expect("failed to create event loop");
    // Frames come from the present thread (vsync paces it): the main thread
    // waits for them and for input.
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = app::App::new(collector, debug_ui, snapshots, request, control, options, proxy);
    event_loop.run_app(&mut app).expect("event loop error");
}
