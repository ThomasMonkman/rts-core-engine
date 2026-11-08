//! Command-line options (`--help` lists them). See docs/guides/running.md.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};

use rts_core_engine::engine::session::{AutoTeams, SessionConfig};
use rts_core_engine::engine::snapshot::{DebugRequest, SimControl};

/// The repository's licence (GPL-3.0, `LICENSE`; also the packages'
/// LICENSE.txt), built into the binary so a copy always travels with it
/// (`--license`).
pub const LICENSE: &str = include_str!("../LICENSE");

#[derive(Parser, Debug)]
#[command(
    name = "rts-core-engine",
    version = concat!(env!("CARGO_PKG_VERSION"), "+", env!("RTS_BUILD_ID")),
    about = "Supreme Commander: Forged Alliance engine remake",
    after_help = "© Thomas Monkman. Free software under the GNU GPL v3 or later; no warranty. --license shows the terms."
)]
pub struct Cli {
    /// Print the licence (built into the program) and exit.
    #[arg(long)]
    pub license: bool,

    /// Run without a window or renderer (fast feedback; no snapshots built).
    #[arg(long)]
    pub headless: bool,

    /// Skip the main menu: start a skirmish straight away (the options below
    /// shape it). Implied by --headless and by any session option (--map,
    /// --script, --player, ...); without it a windowed run opens the main menu.
    #[arg(long)]
    pub session: bool,

    /// Ticks to run, then stop (10 ticks = 1 game second). Headless defaults
    /// to 0 (setup only); windowed runs forever without it.
    #[arg(long, value_name = "N")]
    pub ticks: Option<u64>,

    /// VFS path of the map's `_scenario.lua`.
    #[arg(long, value_name = "PATH", default_value = "/maps/scmp_009/scmp_009_scenario.lua")]
    pub map: String,

    /// AI armies filled from the start spots (0 = every start spot: a 4v4
    /// on Seton's Clutch).
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub armies: u32,

    /// Teams the armies split into by start spot (FAF's alternating
    /// layout: 2 = odd spots against even); 0 or 1 = everyone for themselves.
    #[arg(long, value_name = "N", default_value_t = 2)]
    pub teams: u32,

    /// Teams by where the armies start, as FAF's lobby AutoTeams option:
    /// top-bottom (north against south), left-right, or odd-even start spots.
    /// Replaces --teams.
    #[arg(long, value_name = "LAYOUT", value_parser = parse_auto_teams)]
    pub auto_teams: Option<AutoTeams>,

    /// The game's fog of war (FAF's FogOfWar option): none (everything
    /// visible), explored (the default: terrain known, units need intel) or
    /// unexplored (nothing shows until scouted; not in FAF's lobby).
    #[arg(long, value_name = "MODE", value_parser = ["none", "explored", "unexplored"])]
    pub fog: Option<String>,

    /// The game's victory condition (FAF's Victory option): sandbox (the
    /// default: no game end), demoralization (an army is out when its
    /// commander dies), domination, eradication, decapitation.
    #[arg(long, value_name = "CONDITION", value_parser = ["sandbox", "demoralization", "domination", "eradication", "decapitation"])]
    pub victory: Option<String>,

    /// Armies start with a prebuilt base (FAF's PrebuiltUnits lobby option:
    /// a factory, generators and extractors).
    #[arg(long)]
    pub prebuilt: bool,

    /// Armies are unplayed slots: no AI and no starting commanders (only a
    /// --script moves their units; stress tests).
    #[arg(long)]
    pub no_ai: bool,

    /// Runs a mod (its uid, from its `mod_info.lua`; repeatable), as if
    /// the lobby had it on: a sim mod in the sim, a UI-only one in the
    /// interface. E.g. Resource Rich: 74A9EAB2-E851-11DB-A1F1-F2C755D89593.
    #[arg(long = "mod", value_name = "UID")]
    pub mods: Vec<String>,

    /// FAF vault folder whose `maps/` and `mods/` are mounted (e.g. ../faf-vault).
    #[arg(long, value_name = "DIR")]
    pub vault: Option<PathBuf>,

    /// Sim RNG seed.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub seed: u64,

    /// Lua file (on disk) run in the sim after `BeginSession`, e.g. dev/scenes/fight.lua.
    #[arg(long, value_name = "FILE")]
    pub script: Option<String>,

    /// Game speed: a multiplier like 0.5, 2, 16, or `max`. A window
    /// defaults to 1. Headless runs go flat out unless this is given, then
    /// they keep it in real time (the network test harness wants `1`).
    #[arg(long, value_name = "X|max", value_parser = parse_speed)]
    pub speed: Option<u32>,

    /// Save one frame as a PNG once the sim reaches --screenshot-tick, then exit.
    #[arg(long, value_name = "PNG")]
    pub screenshot: Option<PathBuf>,

    /// Sim tick to take the --screenshot at (in the main menu: interface
    /// time, 10 = 1 s, at least 6 s for its opening animations).
    #[arg(long, value_name = "N", default_value_t = 50)]
    pub screenshot_tick: u64,

    /// Initial camera: look-at point and zoom (the ground across the
    /// window's longer side).
    #[arg(long, value_name = "X,Z,ZOOM", value_parser = parse_camera)]
    pub camera: Option<(f32, f32, f32)>,

    /// A FAF `Game.prefs` to play with (the interface's options and your
    /// key bindings). Defaults to `dev/Game.prefs` when that file exists.
    #[arg(long, value_name = "FILE")]
    pub prefs: Option<PathBuf>,

    /// Debug overlays to start with: paths, ranges, nav=land|hover|naval|amph, threat=<army>, fog=<army>.
    #[arg(long, value_name = "LIST", value_delimiter = ',', value_parser = parse_overlay)]
    pub overlay: Vec<Overlay>,

    /// Entities selected at start, comma separated (the first in the
    /// inspector, with its path).
    #[arg(long, value_name = "IDS", value_delimiter = ',')]
    pub select: Vec<u32>,

    /// Start with anti-aliasing (SMAA) off, whatever the profile says (the
    /// Overlays window toggles it).
    #[arg(long, alias = "no-fxaa")]
    pub no_aa: bool,

    /// Time every render pass and the debug groups inside them on the GPU
    /// and log the mean ms of each every 120 frames (target `gpu_passes`;
    /// exact on NVIDIA and AMD, only the frame total is sound on Apple GPUs).
    #[arg(long)]
    pub gpu_passes: bool,

    /// The main window's size in pixels, `WIDTHxHEIGHT` (e.g. `3024x1964`, a
    /// 14-inch MacBook Pro's screen), whatever the display: GPU profiling at
    /// another machine's resolution. Default 1024x768 points.
    #[arg(long, value_name = "WxH")]
    pub window_size: Option<String>,

    /// Shift counts as held throughout (screenshots of what Shift shows:
    /// every unit's queued orders, reclaim labels).
    #[arg(long)]
    pub hold_shift: bool,

    /// The cursor starts at this window point, `X,Y` in pixels (physical,
    /// from the top left): screenshots of what hovering shows, such as a
    /// build mode's placement ghost.
    #[arg(long, value_name = "X,Y")]
    pub cursor: Option<String>,

    /// FAF's second head (overrides the Secondary Adapter option):
    /// `windowed` (a second window on this display), `offscreen` (drawn
    /// without a window; `--screenshot` also saves it, as `<name>-head2.png`)
    /// or `width,height,refresh` (borderless fullscreen on another monitor).
    #[arg(long, value_name = "HEAD")]
    pub second_head: Option<String>,

    /// Lua run in the interface once it is up (dialogs for a screenshot,
    /// e.g. `import('/lua/ui/dialogs/options.lua').CreateDialog(GetFrame(0))`).
    #[arg(long, value_name = "LUA")]
    pub ui_lua: Option<String>,

    /// Seconds without a completed tick before the sim counts as stalled:
    /// logs an ERROR with the tick, stage and (if in Lua) the traceback.
    /// Headless runs then exit with code 3 if it is still stuck 10 s later.
    /// 0 disables.
    #[arg(long, value_name = "SECS", default_value_t = 30)]
    pub stall_timeout: u64,

    /// Log file (also shown in the window's Logs panel). Give separate runs
    /// separate files so they don't overwrite each other. Defaults to
    /// game.log: in the working directory in a source checkout, else beside
    /// the program (the settings folder if that's read-only).
    #[arg(long, value_name = "FILE")]
    pub log: Option<PathBuf>,

    /// FAF's Lua: the FAF client's data folder (C:\ProgramData\FAForever),
    /// a checkout of github.com/FAForever/fa, or an init file. Found by
    /// itself when left out (see docs/guides/packaging.md).
    #[arg(long, value_name = "DIR")]
    pub fa: Option<PathBuf>,

    /// The Supreme Commander: Forged Alliance install folder (the one with
    /// gamedata/). Found by itself when left out (Steam, GOG, the FAF client's
    /// setting, or beside the program).
    #[arg(long, value_name = "DIR")]
    pub game_dir: Option<PathBuf>,

    /// Army you play (windowed): a human slot whose units you command; the
    /// other armies keep the AI. Without it you watch the AIs, and can still
    /// order any army's units by hand.
    #[arg(long, value_name = "ARMY")]
    pub player: Option<u32>,

    /// Record the game to FILE (replay with --replay). Windowed games are
    /// recorded anyway, into the replays folder (see --no-record).
    #[arg(long, value_name = "FILE")]
    pub record: Option<PathBuf>,

    /// Don't record windowed games into the replays folder.
    #[arg(long)]
    pub no_record: bool,

    /// Replay a recorded game: its session (map, armies, seed, the lobby's
    /// options) and every player command, at their ticks.
    #[arg(long, value_name = "FILE")]
    pub replay: Option<PathBuf>,

    /// With `--replay`: write a keyframe every `--keyframe-every` ticks into
    /// this folder as the replay plays, carrying on from the latest one
    /// already there (the viewer's run-ahead process; `docs/architecture/replays.md`).
    #[arg(long, value_name = "DIR")]
    pub keyframes_out: Option<PathBuf>,

    /// Ticks between keyframes (`--keyframes-out`; rounded to the 100-tick
    /// collection interval).
    #[arg(long, value_name = "TICKS", default_value_t = 600)]
    pub keyframe_every: u64,

    /// Run at low priority, leaving the machine to a game being watched
    /// (the run-ahead process).
    #[arg(long)]
    pub background: bool,

    /// End when this process does (the run-ahead process: its viewer).
    #[arg(long, value_name = "PID")]
    pub parent_pid: Option<u32>,

    /// No longer needed: the tick stages are always timed. Accepted so older
    /// command lines still run.
    #[arg(long, hide = true)]
    pub profile_stages: bool,

    /// Headless network test: host a game and launch it once N players
    /// (this one included) have joined. The players take the first start
    /// spots; --armies fills the rest with AI. See docs/architecture/networking.md.
    #[arg(long, value_name = "N")]
    pub net_host: Option<u32>,

    /// Headless network test: join the game hosted at ADDR (`ip:port`).
    #[arg(long, value_name = "ADDR")]
    pub net_join: Option<String>,

    /// Port to listen on (0: 16010, or the next free port above it).
    #[arg(long, value_name = "PORT", default_value = "0")]
    pub net_port: u16,

    /// Player name in a network game.
    #[arg(long, value_name = "NAME", default_value = "Player")]
    pub net_name: String,

    /// In a network game, let a bot play the local army (random move
    /// orders every 3 s): test traffic.
    #[arg(long)]
    pub net_bot: bool,

    /// Write every path the virtual file system mounts (sorted, one per
    /// line) to FILE, default `vfs_dump.txt`. Off unless given.
    #[arg(long, value_name = "FILE", num_args = 0..=1, default_missing_value = "vfs_dump.txt")]
    pub vfs_dump: Option<PathBuf>,

    /// No sound output (the mixer still runs, so cues finish as usual).
    /// Headless runs never open an audio device.
    #[arg(long)]
    pub no_audio: bool,

    /// Write the first --audio-dump-seconds of the mixed sound to this WAV
    /// (works with --no-audio: checks sound without speakers).
    #[arg(long, value_name = "WAV")]
    pub audio_dump: Option<PathBuf>,

    /// Seconds of sound --audio-dump writes.
    #[arg(long, value_name = "SECS", default_value_t = 20.0)]
    pub audio_dump_seconds: f32,

    /// Write the engine's Lua API (Luau `.d.luau` and EmmyLua definitions per
    /// VM) into this folder, then exit (e.g. `docs/api`, ignored by git).
    #[arg(long, value_name = "DIR")]
    pub dump_api: Option<PathBuf>,

    /// With --dump-api: also write `faf-compare.md`, comparing the API with
    /// FAF's `/engine` annotations in this FAF checkout (default: `FAF_REPO`,
    /// else `../fa`). Fails if the checkout isn't there.
    #[arg(long, value_name = "FAF_DIR", num_args = 0..=1, requires = "dump_api")]
    pub faf_compare: Option<Option<PathBuf>>,

    /// Print the weapon audit's census (dev/audit's script on the flat audit map,
    /// every case in a session of its own: the golden test's runner) as a
    /// table to stdout: every case's id, attacker as bp[weapon], target,
    /// outcome, fired, damage. `WEAPON_AUDIT_IDS=a,b,c` runs a subset (the
    /// same rows; a bare id with its bearings, `n@b` one bearing);
    /// `WEAPON_AUDIT_BEARINGS=all` every 45 degrees (default `gate`: the
    /// golden's set). Headless; exit 0 when the census completes (the
    /// outcomes are data, not errors).
    #[arg(long, conflicts_with = "audit_test")]
    pub audit_table: bool,

    /// Watch weapon-audit case N run by itself on the flat audit map, centred on
    /// the map's centre; `N@B` runs it with the target at bearing B degrees off
    /// the attacker's facing (a turreted weapon's bearing sub-case, e.g. 531@180).
    /// Windowed you watch the engagement (the exact spot prints here seconds
    /// after launch); with --headless it runs to the verdict and prints the
    /// case's row.
    #[arg(long, value_name = "N[@B]", value_parser = parse_audit_key)]
    pub audit_test: Option<String>,

    /// The game's own `/switch` arguments that had no meaning here and
    /// were dropped (see [`moho_args`]), logged at startup.
    #[arg(skip)]
    pub ignored_moho_args: Vec<String>,
}

/// Translates the real game's command line (`SupremeCommander.exe /log
/// <file> /init <file>`, as LOUD's launcher and shortcuts run it) into
/// ours, so this program can stand in for `bin/SupremeCommander.exe`:
/// `/init` becomes `--fa`, `/log` becomes `--log`. Relative paths are read
/// from the program's folder: LOUD's launcher starts the game from the
/// Forged Alliance folder with `/init "..\LOUD\bin\LoudDataPath.lua"`,
/// which only names the file from `bin/`. Any other `/switch` (one word
/// after the slash, so `/maps/x/x_scenario.lua` stays a value) is dropped
/// with the values after it, as moho ignores switches it doesn't read.
/// Returns the new arguments and the dropped ones.
pub fn moho_args(args: Vec<OsString>, exe_dir: Option<&Path>) -> (Vec<OsString>, Vec<String>) {
    let is_switch = |a: &OsString| {
        let a = a.to_string_lossy();
        a.strip_prefix('/').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    };
    let is_value = |a: &OsString| !is_switch(a) && !a.to_string_lossy().starts_with('-');
    let resolve = |p: &OsString| -> OsString {
        let p = Path::new(p);
        match exe_dir {
            Some(dir) if p.is_relative() => dir.join(p).into_os_string(),
            _ => p.as_os_str().to_owned(),
        }
    };
    let mut out = Vec::with_capacity(args.len());
    let mut ignored = Vec::new();
    let mut it = args.into_iter().peekable();
    // The program's own path.
    out.extend(it.next());
    while let Some(arg) = it.next() {
        if !is_switch(&arg) {
            out.push(arg);
            continue;
        }
        let name = arg.to_string_lossy().to_ascii_lowercase();
        let ours = match name.as_str() {
            "/init" => Some("--fa"),
            "/log" => Some("--log"),
            _ => None,
        };
        match (ours, it.next_if(is_value)) {
            (Some(flag), Some(value)) => {
                out.push(flag.into());
                out.push(resolve(&value));
            }
            (_, value) => {
                ignored.push(arg.to_string_lossy().into_owned());
                ignored.extend(value.map(|v| v.to_string_lossy().into_owned()));
                while let Some(v) = it.next_if(is_value) {
                    ignored.push(v.to_string_lossy().into_owned());
                }
            }
        }
    }
    (out, ignored)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Overlay {
    Paths,
    Ranges,
    Nav(u8),
    Threat(u32),
    Fog(u32),
}

fn parse_auto_teams(s: &str) -> Result<AutoTeams, String> {
    match s {
        "top-bottom" | "north-south" | "tvsb" => Ok(AutoTeams::TopBottom),
        "left-right" | "west-east" | "lvsr" => Ok(AutoTeams::LeftRight),
        "odd-even" | "pvsi" => Ok(AutoTeams::OddEven),
        _ => Err(format!("expected top-bottom (north-south), left-right or odd-even, got `{s}`")),
    }
}

fn parse_speed(s: &str) -> Result<u32, String> {
    if s == "max" {
        return Ok(SimControl::MAX);
    }
    match s.parse::<f32>() {
        Ok(m) if m > 0.0 => Ok((m * 100.0).round().max(1.0) as u32),
        _ => Err(format!("expected a positive multiplier or `max`, got `{s}`")),
    }
}

fn parse_camera(s: &str) -> Result<(f32, f32, f32), String> {
    let n: Vec<f32> = s.split(',').map(|p| p.trim().parse::<f32>()).collect::<Result<_, _>>().map_err(|e| format!("{e}"))?;
    match n[..] {
        [x, z, d] => Ok((x, z, d)),
        _ => Err(format!("expected X,Z,DISTANCE, got `{s}`")),
    }
}

fn parse_overlay(s: &str) -> Result<Overlay, String> {
    match s.split_once('=') {
        None if s == "paths" => Ok(Overlay::Paths),
        None if s == "ranges" => Ok(Overlay::Ranges),
        Some(("nav", l)) => ["land", "hover", "naval", "amph"]
            .iter()
            .position(|n| *n == l)
            .map(|i| Overlay::Nav(i as u8))
            .ok_or_else(|| format!("nav layer must be land, hover, naval or amph, got `{l}`")),
        Some(("threat", a)) => a.parse().map(Overlay::Threat).map_err(|_| format!("threat expects an army number, got `{a}`")),
        Some(("fog", a)) => a.parse().map(Overlay::Fog).map_err(|_| format!("fog expects an army number, got `{a}`")),
        _ => Err(format!("unknown overlay `{s}` (paths, ranges, nav=<layer>, threat=<army>, fog=<army>)")),
    }
}

/// Options that only mean something in a game session: giving any of them
/// skips the main menu, as `--session` does.
const SESSION_OPTIONS: &[&str] = &[
    "ticks", "map", "armies", "teams", "auto_teams", "fog", "victory", "prebuilt", "no_ai", "seed", "script", "camera", "overlay", "select", "player", "record", "replay",
    "profile_stages", "net_host", "net_join", "net_bot", "mods",
];

impl Cli {
    /// The speed (percent) to keep in real time, or `None` to run flat out.
    /// A window always keeps one: `--speed`, else normal. A headless run
    /// keeps one only when given `--speed`, and `max` there is flat out.
    pub fn pace(&self, headless: bool) -> Option<u32> {
        if !headless {
            return Some(self.speed.unwrap_or(100));
        }
        self.speed.filter(|&s| s != SimControl::MAX)
    }

    /// Parses the process arguments (exits on `--help` or an error).
    pub fn parse_args() -> Self {
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
        let (args, ignored) = moho_args(std::env::args_os().collect(), exe_dir.as_deref());
        let mut cli = Self::from_matches(&Self::command().get_matches_from(args)).unwrap_or_else(|e| e.exit());
        cli.ignored_moho_args = ignored;
        cli
    }

    #[cfg(test)]
    fn try_parse_args<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<Self, clap::Error> {
        Self::from_matches(&Self::command().try_get_matches_from(args)?)
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let mut cli = Self::from_arg_matches(matches)?;
        cli.session |= cli.headless || SESSION_OPTIONS.iter().any(|id| matches.value_source(id) == Some(ValueSource::CommandLine));
        Ok(cli)
    }

    pub fn session(&self) -> SessionConfig {
        SessionConfig {
            scenario: self.map.clone(),
            ai_armies: self.armies,
            teams: self.teams,
            seed: self.seed,
            // Headless without --ticks: setup only.
            ticks: self.ticks.or(self.headless.then_some(0)),
            script: self.script.clone(),
            ai: !self.no_ai,
            create_initial_units: !self.no_ai,
            human_army: self.player.filter(|&a| a > 0),
            prebuilt: self.prebuilt,
            auto_teams: self.auto_teams,
            net_bot: self.net_bot,
            fog_of_war: self.fog.clone(),
            victory: self.victory.clone(),
            mods: self.mods.clone(),
            ..SessionConfig::default()
        }
    }

    /// Stall watchdog settings; `None` when disabled.
    pub fn watchdog(&self) -> Option<rts_core_engine::engine::watchdog::Config> {
        use std::time::Duration;
        (self.stall_timeout > 0).then(|| rts_core_engine::engine::watchdog::Config {
            stall: Duration::from_secs(self.stall_timeout),
            exit_after: self.headless.then_some(Duration::from_secs(10)),
        })
    }

    /// The audio thread's options (windowed runs).
    pub fn audio(&self) -> rts_core_engine::audio::AudioOptions {
        rts_core_engine::audio::AudioOptions { device: !self.no_audio, dump: self.audio_dump.clone().map(|p| (p, self.audio_dump_seconds)) }
    }

    /// Initial debug views for the renderer.
    pub fn debug_request(&self) -> DebugRequest {
        let mut r = DebugRequest { selected: self.select.first().copied(), ..Default::default() };
        for o in &self.overlay {
            match *o {
                Overlay::Paths => r.paths = true,
                Overlay::Ranges => r.weapon_ranges = true,
                Overlay::Nav(l) => r.nav_layer = Some(l),
                Overlay::Threat(a) => r.threat_army = Some(a),
                Overlay::Fog(a) => r.fog_army = Some(a),
            }
        }
        r
    }
}

/// `--audit-test`'s case key: `N` or `N@B` (B in degrees, may be negative).
fn parse_audit_key(s: &str) -> Result<String, String> {
    if rts_core_engine::weapon_audit::is_case_key(s) {
        Ok(s.to_owned())
    } else {
        Err(format!("'{s}' is not a case key: N or N@BEARING (e.g. 46, 531@180, 238@-90)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_and_render_options() {
        let c = Cli::try_parse_from([
            "rts", "--headless", "--map", "/maps/x/x_scenario.lua", "--seed", "7", "--armies", "4", "--ticks", "30",
            "--speed", "max", "--camera", "1,2,3", "--overlay", "paths,nav=naval,threat=2", "--select", "5",
        ])
        .unwrap();
        let s = c.session();
        assert_eq!(s.scenario, "/maps/x/x_scenario.lua");
        assert_eq!((s.seed, s.ai_armies, s.ticks), (7, 4, Some(30)));
        assert_eq!(c.speed, Some(SimControl::MAX));
        assert_eq!(c.pace(true), None, "headless max runs flat out");
        assert_eq!(c.camera, Some((1.0, 2.0, 3.0)));
        let r = c.debug_request();
        assert!(r.paths && !r.weapon_ranges);
        assert_eq!((r.nav_layer, r.threat_army, r.selected), (Some(2), Some(2), Some(5)));
    }

    #[test]
    fn speed_paces_a_window_always_and_a_headless_run_only_when_given() {
        let pace = |args: &[&str], headless| Cli::try_parse_from([&["rts"][..], args].concat()).unwrap().pace(headless);
        assert_eq!(pace(&[], false), Some(100), "a window defaults to normal speed");
        assert_eq!(pace(&["--speed", "2"], false), Some(200));
        assert_eq!(pace(&["--speed", "max"], false), Some(SimControl::MAX), "a window keeps max as a speed it can step down from");
        assert_eq!(pace(&["--headless"], true), None);
        assert_eq!(pace(&["--headless", "--speed", "1"], true), Some(100));
        assert_eq!(pace(&["--headless", "--speed", "0.5"], true), Some(50));
        assert!(Cli::try_parse_from(["rts", "--headless", "--realtime"]).is_err(), "--realtime is gone: use --speed");
    }

    #[test]
    fn headless_defaults_to_setup_only_and_rejects_typos() {
        let c = Cli::try_parse_from(["rts", "--headless"]).unwrap();
        assert_eq!(c.session().ticks, Some(0));
        assert_eq!(c.speed, None);
        assert_eq!(c.pace(true), None, "headless runs flat out by default");
        assert_eq!(c.log, None, "install::default_log places game.log");
        let w = c.watchdog().expect("watchdog on by default");
        assert_eq!((w.stall.as_secs(), w.exit_after.map(|d| d.as_secs())), (30, Some(10)));
        assert!(Cli::try_parse_from(["rts", "--stall-timeout", "0"]).unwrap().watchdog().is_none());
        assert_eq!(Cli::try_parse_from(["rts"]).unwrap().watchdog().unwrap().exit_after, None, "windowed never exits");
        assert!(Cli::try_parse_from(["rts", "--tick", "400"]).is_err(), "typo is an error, not ignored");
        assert!(Cli::try_parse_from(["rts", "--overlay", "nav=lava"]).is_err());
        assert!(Cli::try_parse_from(["rts", "--speed", "-2"]).is_err());
    }

    #[test]
    fn auto_teams_take_faf_layouts() {
        let layout = |s: &str| Cli::try_parse_from(["rts", "--auto-teams", s]).map(|c| c.session().auto_teams);
        assert_eq!(layout("north-south").unwrap(), Some(AutoTeams::TopBottom));
        assert_eq!(layout("tvsb").unwrap(), Some(AutoTeams::TopBottom));
        assert_eq!(layout("left-right").unwrap(), Some(AutoTeams::LeftRight));
        assert_eq!(layout("odd-even").unwrap(), Some(AutoTeams::OddEven));
        assert!(layout("diagonal").is_err());
        assert_eq!(Cli::try_parse_from(["rts"]).unwrap().session().auto_teams, None);
    }

    #[test]
    fn main_menu_unless_a_session_is_asked_for() {
        let menu = |args: &[&str]| !Cli::try_parse_args(["rts"].iter().chain(args).copied()).unwrap().session;
        assert!(menu(&[]));
        assert!(menu(&["--speed", "2", "--prefs", "x.prefs", "--ui-lua", "LOG(1)", "--screenshot", "a.png", "--vault", "../faf-vault"]));
        assert!(!menu(&["--session"]));
        assert!(!menu(&["--headless"]));
        assert!(!menu(&["--script", "dev/scenes/fight.lua"]));
        assert!(!menu(&["--map", "/maps/x/x_scenario.lua"]));
        assert!(!menu(&["--player", "1"]));
        assert!(!menu(&["--seed", "0"]), "a session option counts even at its default value");
        assert!(!menu(&["--auto-teams", "north-south"]));
    }

    /// LOUD's launcher command line: `/init` and `/log` map to ours, read
    /// from the program's folder; other switches drop with their values;
    /// our own flags and VFS paths pass through.
    #[test]
    fn translates_the_real_games_command_line() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        let bin = Path::new("C:/FA/bin");
        let (out, ignored) = moho_args(args(&["exe", "/log", "..\\LOUD\\bin\\LOUD.log", "/init", "..\\LOUD\\bin\\LoudDataPath.lua", "/nobugreport", "/gpgnet", "127.0.0.1:1", "--map", "/maps/x/x_scenario.lua"]), Some(bin));
        assert_eq!(
            out,
            args(&["exe", "--log"]).into_iter().chain([bin.join("..\\LOUD\\bin\\LOUD.log").into_os_string(), "--fa".into(), bin.join("..\\LOUD\\bin\\LoudDataPath.lua").into_os_string(), "--map".into(), "/maps/x/x_scenario.lua".into()]).collect::<Vec<_>>()
        );
        assert_eq!(ignored, ["/nobugreport", "/gpgnet", "127.0.0.1:1"]);
        let abs = if cfg!(windows) { "C:\\x\\init.lua" } else { "/x/init.lua" };
        assert_eq!(moho_args(args(&["exe", "/INIT", abs]), Some(bin)).0, args(&["exe", "--fa", abs]));
        let c = Cli::try_parse_from(moho_args(args(&["exe", "/init", "a.lua", "/log", "b.log"]), Some(bin)).0).unwrap();
        assert_eq!((c.fa, c.log), (Some(bin.join("a.lua")), Some(bin.join("b.log"))));
    }

    #[test]
    fn license_is_built_in() {
        assert!(LICENSE.contains("GNU GENERAL PUBLIC LICENSE") && LICENSE.contains("Version 3"));
        assert!(Cli::try_parse_args(["rts", "--license"]).unwrap().license);
    }
}
