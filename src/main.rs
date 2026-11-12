// wgpu's types nest deep enough that proving `wgpu::QuerySet: Sync` (the
// `OnceLock` statics in render/gpu_prof.rs) passes the default limit of 128 on
// rustc 1.101, which warns and will make it an error.
#![recursion_limit = "256"]

mod audit;
mod cli;
mod render;

use rts_core_engine::{audio, debug_ui, engine, install};

use debug_ui::DebugUiState;
use std::sync::{Arc, Mutex};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

// The development layout the render tests run against (the program itself
// finds its files with `install::locate`).
#[cfg(test)]
const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
#[cfg(test)]
const FA_PATH: &str = "../Supreme Commander Forged Alliance";

/// The Tracy layer with `--features tracy`, else nothing.
#[cfg(feature = "tracy")]
fn tracy_layer() -> Option<tracing_tracy::TracyLayer> {
    Some(tracing_tracy::TracyLayer::default())
}
#[cfg(not(feature = "tracy"))]
fn tracy_layer() -> Option<tracing_subscriber::layer::Identity> {
    None
}

/// Exit code when the engine fails to initialise or start (3 is the stall
/// watchdog's).
const EXIT_START_FAILED: u8 = 2;

/// Asks with folder pickers for what `install::locate` couldn't find,
/// saving the answer to the settings file for next time. `None`: cancelled.
fn ask_for_install(overrides: &install::Overrides, mut missing: install::Missing) -> Option<install::Install> {
    let mut chosen = overrides.clone();
    let mut retry = false;
    loop {
        let (title, text) = if missing.game {
            (
                "Find Supreme Commander: Forged Alliance",
                "This engine plays on the files of your own copy of Supreme Commander: Forged Alliance (Steam or GOG), and none was found.\n\nChoose its install folder: the one with the gamedata folder inside.",
            )
        } else {
            (
                "Find FAF's game code",
                "This engine runs Forged Alliance Forever's game code, and it wasn't found.\n\nChoose the FAF client's data folder (C:\\ProgramData\\FAForever), or a download of github.com/FAForever/fa.",
            )
        };
        let text = if retry { format!("That folder isn't the one.\n\n{text}") } else { text.to_string() };
        tracing::warn!("{missing}");
        let go = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Info).set_title(title).set_description(text).set_buttons(rfd::MessageButtons::OkCancel).show();
        if go != rfd::MessageDialogResult::Ok {
            return None;
        }
        let dir = rfd::FileDialog::new().set_title(title).pick_folder()?;
        let asked_for_game = missing.game;
        if asked_for_game { chosen.game = Some(dir) } else { chosen.fa = Some(dir) }
        missing = match install::locate(&chosen) {
            Ok(found) => {
                match install::save(&found) {
                    Ok(path) => tracing::info!("saved the game's folders to {}", path.display()),
                    Err(e) => tracing::warn!("couldn't save the game's folders: {e}"),
                }
                return Some(found);
            }
            Err(m) => m,
        };
        retry = if asked_for_game { missing.game } else { missing.fa };
    }
}

/// Our own log targets: the library crate, this binary and the custom
/// targets the code logs under. They keep TRACE.
const OWN_TARGETS: &[&str] = &["rts_core_engine", "engine", "lua", "graphics", "gpu_passes", "panic"];

/// The log filter: TRACE for our own targets, WARN for every other crate.
/// Dependencies log per event-loop wake or per packet at TRACE (calloop and
/// the Wayland clipboard, the DNS resolver, iroh): hundreds of thousands of
/// lines. A new custom log target of ours must be added to `OWN_TARGETS`.
fn log_filter() -> Targets {
    OWN_TARGETS.iter().fold(Targets::new().with_default(LevelFilter::WARN), |t, own| t.with_target(*own, LevelFilter::TRACE))
}

/// Whether the console log gets ANSI colours: only when its stream is a
/// terminal (a pipe, such as the VS Code extension's output panel, or a
/// redirect gets plain text), and not if `NO_COLOR` is set or `TERM=dumb`.
fn console_colour(stderr: bool) -> bool {
    use std::io::IsTerminal;
    let tty = if stderr { std::io::stderr().is_terminal() } else { std::io::stdout().is_terminal() };
    tty && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) && std::env::var("TERM").as_deref() != Ok("dumb")
}

fn main() -> std::process::ExitCode {
    // Parse first: `--help` and argument errors exit before any setup.
    let cli = cli::Cli::parse_args();
    if cli.license {
        print!("{}", cli::LICENSE);
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(path) = &cli.vfs_dump {
        rts_core_engine::engine::set_vfs_dump(path.clone());
    }
    if let Some(dir) = &cli.dump_api {
        let faf = cli.faf_compare.clone().map(|dir| dir.unwrap_or_else(|| std::env::var("FAF_REPO").unwrap_or_else(|_| "../fa".into()).into()));
        if let Some(faf) = &faf
            && !faf.join("engine").is_dir()
        {
            eprintln!("--faf-compare: no FAF checkout at {} (expected its `engine/` folder); pass the path or set FAF_REPO", faf.display());
            return std::process::ExitCode::FAILURE;
        }
        return match rts_core_engine::lua_api::dump::write(dir, faf.as_deref()) {
            Ok(names) => {
                println!("wrote {} to {}", names.join(", "), dir.display());
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("--dump-api failed: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    // The in-game Logs window's events: INFO and up (the file log keeps
    // DEBUG), the last 20k. Unbounded at DEBUG it kept every doscript line
    // of the run, formatted on the sim thread as it logged.
    let collector = egui_tracing::EventCollector::default().with_max_level(tracing::Level::INFO).with_max_events(Some(20_000));
    let ui_filter = log_filter();

    // The audit-table run's table owns stdout: anything else the sim logs
    // goes to stderr (the file log keeps everything).
    let audit_quiet_stdout = cli.audit_table || (cli.audit_test.is_some() && cli.headless);

    // A given log that can't be created (a launcher's path to a folder that
    // isn't there) falls back to the default one rather than ending the run.
    let mut log_path = cli.log.clone().unwrap_or_else(install::default_log);
    let mut log_fallback = None;
    let log_file = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) if cli.log.is_some() => {
            let default = install::default_log();
            let msg = format!("couldn't create the log {}: {e}; logging to {} instead", log_path.display(), default.display());
            eprintln!("{msg}");
            log_fallback = Some(msg);
            log_path = default;
            std::fs::File::create(&log_path).unwrap_or_else(|e| panic!("failed to create {}: {e}", log_path.display()))
        }
        Err(e) => panic!("failed to create {}: {e}", log_path.display()),
    };
    let (non_blocking_writer, _guard) = tracing_appender::non_blocking(log_file);

    tracing_subscriber::registry()
        .with(collector.clone().with_filter(ui_filter.clone()))
        .with(
            fmt::layer()
                .with_writer(non_blocking_writer)
                .with_ansi(false)
                // The run-ahead process keeps INFO and up: at DEBUG it logs
                // ~300 MB for an hour of a 16-army game.
                .with_filter(if cli.background { ui_filter.clone().with_default(LevelFilter::INFO) } else { ui_filter.clone() }),
        )
        .with(
            fmt::layer()
                .with_ansi(console_colour(audit_quiet_stdout))
                .with_writer(if audit_quiet_stdout {
                    tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stderr)
                } else {
                    tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stdout)
                })
                .with_filter(ui_filter),
        )
        .with(tracy_layer())
        .init();
    rts_core_engine::threads::init_global_pool();
    // Before any VM or script cache: it changes how scripts compile.
    if cli.lua_debug.is_some() || cli.lua_debug_wait {
        let port = cli.lua_debug.unwrap_or(4711);
        if let Err(e) = rts_core_engine::engine::lua::debugger::start(port, cli.lua_debug_wait) {
            tracing::error!("--lua-debug: can't listen on 127.0.0.1:{port}: {e}");
        }
    }
    rts_core_engine::diagnostics::log_panics();
    rts_core_engine::diagnostics::log_system();
    if let Some(msg) = &log_fallback {
        tracing::warn!("{msg}");
    }
    if !cli.ignored_moho_args.is_empty() {
        tracing::info!("ignored the game's command-line switches: {}", cli.ignored_moho_args.join(" "));
    }

    // FAF's Lua and the game's data: found, or (windowed) asked for.
    let overrides = install::Overrides { fa: cli.fa.clone(), game: cli.game_dir.clone() };
    let found = match install::locate(&overrides) {
        Ok(found) => found,
        // The audit-table run is a headless machine command even without
        // the flag's pair (it prints a table; it never asks for installs).
        Err(missing) if cli.headless || cli.audit_table => {
            tracing::error!("{missing}");
            return std::process::ExitCode::from(EXIT_START_FAILED);
        }
        Err(missing) => match ask_for_install(&overrides, missing) {
            Some(found) => found,
            None => return std::process::ExitCode::from(EXIT_START_FAILED),
        },
    };
    tracing::info!(init = %found.init.display(), game = %found.game.display(), log = %log_path.display(), "game files");
    let (init_path, game_path) = (found.init.to_string_lossy().into_owned(), found.game.to_string_lossy().into_owned());

    // The weapon-audit flags run their own audit sessions, not the CLI's:
    // the census headless to its table, one case (headless) to its row.
    if cli.audit_table {
        return audit::run_table(&init_path, &game_path);
    }
    if let Some(key) = cli.audit_test.as_deref()
        && cli.headless
    {
        return audit::run_case(key, &init_path, &game_path);
    }

    let debug_ui: Arc<Mutex<DebugUiState>> = Arc::new(Mutex::new(DebugUiState::new()));
    let debug_ui_engine = debug_ui.clone();

    let headless = cli.headless;
    // `--audit-test` (windowed) plays one weapon-audit case instead of a
    // front-end session: no main menu, the audit script's run to watch.
    let audit_session = cli.audit_test.as_deref().filter(|_| !headless).map(|n| audit::case_session(n, cli.ticks));
    let front_end = !cli.session && audit_session.is_none();
    let session = match audit_session {
        Some(s) => match s {
            Ok(s) => s,
            Err(e) => {
                eprintln!("--audit-test: {e}");
                return std::process::ExitCode::FAILURE;
            }
        },
        None => cli.session(),
    };
    // Windowed runs attach a renderer through a snapshot channel; headless
    // runs don't create one, so the sim builds no snapshots.
    let (sink, snapshots) = if headless {
        (None, None)
    } else {
        let (sink, rx, request) = engine::snapshot::channel();
        if let Ok(mut r) = request.lock() {
            *r = cli.debug_request();
        }
        (Some(sink), Some((rx, request, engine::snapshot::SimControl::new(cli.speed.unwrap_or(100)))))
    };
    let engine_control = snapshots.as_ref().map(|s| s.2.clone());
    // Hold the sim at the screenshot tick until the frame is taken.
    if let (Some(control), Some(_)) = (&engine_control, &cli.screenshot) {
        control.set_hold_at(cli.screenshot_tick);
    }
    let watchdog = cli.watchdog();
    // `--audit-test` plays on the audit map, in the repo's test vault.
    let vault = if cli.audit_test.is_some() { Some(std::path::PathBuf::from(audit::VAULT)) } else { cli.vault.clone() };
    let (record, replay) = (cli.record.clone(), cli.replay.clone());
    let (keyframes_out, keyframe_every) = (cli.keyframes_out.clone(), cli.keyframe_every);
    if cli.background {
        lower_priority();
    }
    if let Some(pid) = cli.parent_pid {
        exit_with_parent(pid);
    }
    // Windowed games are recorded by default (not replays, not --record'ed
    // ones, which go where asked), into the player's profile folder.
    let record_root = (!headless && !cli.no_record && record.is_none() && replay.is_none()).then(rts_core_engine::install::replays_dir).flatten();
    let (net_host, net_join, net_port, net_name) = (cli.net_host, cli.net_join.clone(), cli.net_port, cli.net_name.clone());
    // A window runs in real time at --speed (normal by default), --ticks or
    // not. A headless run goes flat out unless given a --speed.
    let pace = cli.pace(headless);
    // The renderer's order channel (windowed only).
    let (orders, input_link) = if headless {
        (None, None)
    } else {
        let (tx, link) = engine::sim::input::channel();
        (Some(tx), Some(link))
    };
    // A lobby's launch (the front end's `LaunchGame`), from the renderer.
    let (launch, launches) = std::sync::mpsc::channel::<engine::session::Launch>();
    // The engine thread runs one session after another: the command line's
    // (or the front end's first launch), then whatever the renderer launches
    // next (the main menu after `ExitGame`, `RestartSession`). Each session
    // gets a fresh engine; the renderer's channels pass from one to the next.
    let run_engine = move || {
        let (mut sink, mut input_link) = (sink, input_link);
        let mut first = true;
        // What the last session was (`Launch::Restart` starts it again).
        let mut last: Option<engine::session::Launch> = None;
        // The run-ahead process writing the watched replay's keyframes.
        let mut run_ahead: Option<RunAhead> = None;
        loop {
            let mut engine = match engine::Engine::new_with_vault(&init_path, &game_path, debug_ui_engine.clone(), vault.as_deref()) {
                Ok(engine) => engine,
                Err(e) => {
                    tracing::error!("engine init failed: {e}");
                    return false;
                }
            };
            engine.profile_stages();
            if let Some(config) = watchdog
                && let Err(e) = engine.start_watchdog(config) {
                tracing::warn!("stall watchdog not started: {e}");
            }
            if let Some(sink) = sink.take() {
                engine.set_snapshot_sink(sink);
            }
            if let Some(link) = input_link.take() {
                engine.set_input_link(link);
            }
            if let Some(path) = record.as_ref().filter(|_| first)
                && let Err(e) = engine.record_commands(path) {
                tracing::error!("--record {}: {e}", path.display());
                return false;
            }
            // A replay's own session (its header) replaces the options' (but
            // `--ticks` still says how long to run).
            let play_replay = |engine: &mut engine::Engine, path: &std::path::Path, session: engine::session::SessionConfig| match engine.replay_commands(path) {
                Ok((n, header)) => {
                    tracing::info!(commands = n, build = header.build.as_deref().unwrap_or("?"), "replay loaded");
                    Ok(match header.session {
                        Some(recorded) => engine::session::SessionConfig { ticks: session.ticks, ..recorded },
                        None => session,
                    })
                }
                Err(e) => Err(e),
            };
            if let Some(control) = &engine_control {
                control.begin_session();
                engine.set_sim_control(control.clone());
            }
            // The front end: the renderer has the file system (sent with the
            // sink) and runs the main menu; the session waits for its lobby,
            // or for a replay picked in the Replay dialog. After the first
            // session the renderer always launches the next. `None`: the
            // command line's session (and its `--replay`).
            let mut resume_at = None;
            let launched = if front_end || !first {
                tracing::info!("front end: waiting for a lobby to launch");
                match launches.recv() {
                    Ok(engine::session::Launch::Restart) => last.clone(),
                    Ok(engine::session::Launch::Seek(tick)) => {
                        resume_at = Some(tick);
                        last.clone()
                    }
                    Ok(l) => Some(l),
                    // The window closed first.
                    Err(_) => return true,
                }
            } else {
                None
            };
            let mut profile = None;
            let replay_file = match &launched {
                Some(engine::session::Launch::Replay(path)) => Some(path.as_path()),
                Some(_) => None,
                None => replay.as_deref(),
            };
            let session = match (&launched, replay_file) {
                (Some(engine::session::Launch::Game { config, profile: p }), _) => {
                    profile = p.clone();
                    engine::session::SessionConfig { launch: Some(config.clone()), ..session.clone() }
                }
                (_, Some(path)) => match play_replay(&mut engine, path, session.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("replay {}: {e}", path.display());
                        return false;
                    }
                },
                _ => session.clone(),
            };
            // Keyframes for a replay: the run-ahead process's own (its
            // folder given), or the watched replay's, with a process playing
            // ahead to write them when the options ask.
            match (replay_file, &keyframes_out) {
                (Some(_), Some(dir)) => engine.set_keyframes(dir.clone(), keyframe_every, Some(u64::MAX)),
                (Some(path), None) if !headless => match engine::keyframe::recorder::root().map(|root| (engine::keyframe::recorder::dir_for_replay(&root, path), root)) {
                    Some((Ok(dir), root)) => {
                        let settings = engine::keyframe::recorder::settings();
                        engine::keyframe::recorder::evict(&root, &dir, settings.budget_mb * 1_048_576);
                        if settings.precompute && run_ahead.as_ref().is_none_or(|r| r.replay != path) {
                            run_ahead = RunAhead::spawn(path, &dir, settings.every, vault.as_deref());
                        }
                        engine.set_keyframes(dir, settings.every, resume_at);
                    }
                    Some((Err(e), _)) => tracing::warn!("replay keyframes: {e}"),
                    None => {}
                },
                _ => {}
            }
            // Another session: the last replay's run-ahead process stops.
            if replay_file.is_none_or(|p| run_ahead.as_ref().is_some_and(|r| r.replay != p)) {
                run_ahead = None;
            }
            last = launched;
            if let Some(root) = record_root.as_deref().filter(|_| !engine.is_replay()) {
                engine.record_into(&engine::replay::profile_dir(root, profile.as_deref().unwrap_or(engine::replay::DEFAULT_PROFILE)));
            }
            // Headless network test: the lobby is the harness.
            let session = match (&net_host, &net_join) {
                (Some(players), _) => match rts_core_engine::net::harness::host(*players, net_port, &net_name, &session) {
                    Ok(launch) => engine::session::SessionConfig { launch: Some(launch), ..session },
                    Err(e) => {
                        tracing::error!("--net-host: {e}");
                        return false;
                    }
                },
                (None, Some(addr)) => match rts_core_engine::net::harness::join(addr, net_port, &net_name) {
                    Ok(launch) => engine::session::SessionConfig { launch: Some(launch), ..session },
                    Err(e) => {
                        tracing::error!("--net-join: {e}");
                        return false;
                    }
                },
                _ => session,
            };
            if let Some(speed) = pace {
                engine.set_pace(speed);
            }
            let ticks = session.ticks.is_some();
            if let Err(e) = engine.start(&session) {
                tracing::error!("engine start failed: {e}");
                return false;
            }
            // A run of `--ticks` ends the process; a real-time session ends
            // when the renderer closes it (`SimControl::close`): the next
            // engine takes over its channels.
            if headless || ticks {
                return true;
            }
            (sink, input_link) = engine.release_renderer();
            first = false;
        }
    };

    // `--headless`: run the engine to completion without opening a window
    // (used for the scripted run → fix missing API loop).
    if headless {
        engine::tracy::name_thread("sim");
        return if run_engine() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::from(EXIT_START_FAILED) };
    }

    // Windowed, the engine runs beside the window: a failed start ends the
    // process from its thread (after a moment for the log writer to flush;
    // the window would otherwise stay open on nothing).
    std::thread::Builder::new().name("sim".into()).spawn(move || {
        engine::tracy::name_thread("sim");
        if !run_engine() {
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::process::exit(EXIT_START_FAILED as i32);
        }
    }).expect("sim thread");
    engine::tracy::name_thread("render");
    render::prioritise_render_thread();
    render::keep_awake();

    // Sound (presentation only: the interface and the renderer feed it).
    audio::Audio::start(cli.audio()).install();

    let (snapshots, request, control) = snapshots.expect("windowed runs have a snapshot channel");
    let options = render::Options {
        screenshot: cli.screenshot.clone().map(|p| (p, cli.screenshot_tick)),
        exit_when_sim_ends: cli.ticks.is_some(),
        camera: cli.camera,
        prefs: rts_core_engine::install::choose_prefs(cli.prefs.clone(), cli.screenshot.is_some()),
        player: cli.player.filter(|&a| a > 0),
        select: cli.select,
        ui_lua: cli.ui_lua,
        replay: cli.replay.clone(),
        front_end,
        launch: Some(launch),
        aa: !cli.no_aa,
        gpu_passes: cli.gpu_passes,
        window_size: cli.window_size.as_deref().and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        }),
        hold_shift: cli.hold_shift,
        cursor: cli.cursor.as_deref().and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        }),
        second_head: cli.second_head.as_deref().and_then(|v| {
            let spec = render::state::SecondHeadSpec::parse(v);
            if spec.is_none() {
                tracing::warn!(value = v, "--second-head: not windowed, offscreen or width,height,refresh");
            }
            spec
        }),
        orders,
    };
    render::run(collector, debug_ui, snapshots, request, control, options);
    std::process::ExitCode::SUCCESS
}

/// The process playing a watched replay ahead, writing its keyframes
/// (`--keyframes-out`); stopped when the replay is left.
struct RunAhead {
    replay: std::path::PathBuf,
    child: std::process::Child,
}

impl RunAhead {
    fn spawn(replay: &std::path::Path, dir: &std::path::Path, every: u64, vault: Option<&std::path::Path>) -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("replay keyframes: {e}");
            return None;
        }
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--replay").arg(replay)
            .arg("--headless")
            .arg("--ticks").arg(u32::MAX.to_string())
            .arg("--keyframes-out").arg(dir)
            .arg("--keyframe-every").arg(every.to_string())
            .arg("--background")
            .arg("--parent-pid").arg(std::process::id().to_string())
            .arg("--stall-timeout").arg("0")
            .arg("--log").arg(dir.join("run-ahead.log"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(v) = vault {
            cmd.arg("--vault").arg(v);
        }
        // Half the cores: the watched game keeps the rest.
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).max(1);
        cmd.env("RAYON_NUM_THREADS", threads.to_string());
        match cmd.spawn() {
            Ok(child) => {
                tracing::info!(pid = child.id(), dir = %dir.display(), "replay keyframes: playing ahead");
                Some(Self { replay: replay.to_path_buf(), child })
            }
            Err(e) => {
                tracing::warn!("replay keyframes: the run-ahead process didn't start: {e}");
                None
            }
        }
    }
}

impl Drop for RunAhead {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `--background`: leave the machine to the game being watched.
fn lower_priority() {
    #[cfg(unix)]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS};
        SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}

/// `--parent-pid`: the process ends once `pid` has (a viewer killed rather
/// than closed leaves no run-ahead process behind).
fn exit_with_parent(pid: u32) {
    let _ = std::thread::Builder::new().name("parent watch".into()).spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if !process_alive(pid) {
            tracing::info!(pid, "the parent process ended: exiting");
            std::process::exit(0);
        }
    });
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // Signal 0 checks the process exists without signalling it.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        alive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    #[test]
    fn external_crates_are_quiet_and_ours_are_not() {
        let f = log_filter();
        for noisy in ["calloop::sources", "smithay_clipboard::state", "n0_dns_resolver::x", "iroh::magicsock", "wayland_client::event_queue"] {
            assert!(!f.would_enable(noisy, &Level::TRACE), "{noisy} at TRACE should be off");
            assert!(!f.would_enable(noisy, &Level::INFO), "{noisy} at INFO should be off");
            assert!(f.would_enable(noisy, &Level::WARN), "{noisy} at WARN should stay on");
        }
        for own in ["rts_core_engine::sim::tick", "rts_core_engine", "engine", "lua", "graphics", "gpu_passes", "panic"] {
            assert!(f.would_enable(own, &Level::TRACE), "{own} at TRACE should be on");
        }
    }
}
