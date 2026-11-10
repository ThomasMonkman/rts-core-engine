//! Replays: a game's applied player commands (one line each, see
//! `sim::input::encode`) after a header naming the build that played it and
//! the session it was (map, armies, teams, seed, AI, the lobby's options).
//! The sim is deterministic, so the same build replaying the same commands
//! into the same session plays the same game; `--replay <file>` restores
//! the session from the header.
//!
//! ```text
//! # rts-core-engine replay 1
//! # build 0.1.0+1a2b3c4d, FAF 3839, Lua 5e6f7a8b
//! # session <hex: the session config as LuaData>
//! 12 0 0 1 2 0 ...
//! ```
//!
//! Windowed games record by default into the player's profile folder of
//! [`crate::install::replays_dir`] (`<replays>/<profile>/<name>.rtsreplay`, the
//! layout of moho's `GetSpecialFiles`, which the main menu's Replay dialog
//! lists), named by when they started (UTC) and the map; the newest
//! [`KEEP_REPLAYS`] there are kept.
//!
//! Between the commands, comment lines mark the game's major moments for
//! the replay timeline ([`Timeline`]): `# event <tick> <kind> <army>
//! <blueprint> <x> <y> <z>` (the sim's `Replay.AddEvent`: an ACU dying, an
//! experimental finished, a nuke launched, or a mod's own), `# tick <n>`
//! every [`TICK_MARK_EVERY`] ticks and `# end <n>` when the game ends, so
//! the length is known even of a game closed abruptly. Older readers skip
//! them as comments. The format is ours, not moho's
//! `.SCFAReplay`: the extension and the first line (`# rts-core-engine replay`)
//! say so, and the build line which engine recorded it.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::engine::session::{AiChoice, AutoTeams, SessionConfig};
use crate::engine::sim::lua_data::LuaData;

/// Default recordings kept in the replays folder (oldest removed first).
pub const KEEP_REPLAYS: usize = 100;

const MAGIC: &str = "# rts-core-engine replay 1";

/// How often (ticks) a recording notes how far the game got (`# tick`).
pub const TICK_MARK_EVERY: u64 = 100;

/// One of a game's major moments, as the timeline shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub tick: u64,
    /// `acu_death`, `experimental_built`, `nuke_launch`, or a mod's own (one
    /// word: spaces are written as `_`).
    pub kind: String,
    /// The army it happened to (1-based).
    pub army: i64,
    /// The unit's blueprint id (`-` when none).
    pub blueprint: String,
    /// Where (world units), for the camera to go to.
    pub position: [f32; 3],
}

impl Event {
    /// Its replay line (no newline).
    pub fn line(&self) -> String {
        let word = |s: &str| if s.is_empty() { "-".to_string() } else { s.split_whitespace().collect::<Vec<_>>().join("_") };
        let [x, y, z] = self.position;
        format!("# event {} {} {} {} {x} {y} {z}", self.tick, word(&self.kind), self.army, word(&self.blueprint))
    }

    fn parse(rest: &str) -> Option<Self> {
        let f: Vec<&str> = rest.split_whitespace().collect();
        let [tick, kind, army, blueprint, x, y, z] = f.as_slice() else { return None };
        Some(Self {
            tick: tick.parse().ok()?,
            kind: kind.to_string(),
            army: army.parse().ok()?,
            blueprint: blueprint.to_string(),
            position: [x.parse().ok()?, y.parse().ok()?, z.parse().ok()?],
        })
    }
}

/// What a replay's timeline shows: its events in tick order and how long
/// the game ran (ticks).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub events: Vec<Event>,
    pub length: u64,
    /// The recording marks how far the game got (`# end` / `# tick`): a
    /// replay stops there. Older recordings only have their commands.
    pub marked: bool,
}

/// Reads the timeline from a replay's text: the events, and the length as
/// the last tick the recording reached (its `# end`, else the latest
/// `# tick` mark, event or command).
pub fn read_timeline(text: &str) -> Timeline {
    let mut t = Timeline::default();
    for line in text.lines() {
        let tick = if let Some(rest) = line.strip_prefix("# event ") {
            Event::parse(rest).map(|e| {
                let tick = e.tick;
                t.events.push(e);
                tick
            })
        } else if let Some(n) = line.strip_prefix("# end ").or_else(|| line.strip_prefix("# tick ")) {
            t.marked = true;
            n.trim().parse().ok()
        } else if line.starts_with('#') {
            None
        } else {
            line.split_whitespace().next().and_then(|n| n.parse().ok())
        };
        t.length = t.length.max(tick.unwrap_or(0));
    }
    // Recorded in tick order already; stable for any that aren't.
    t.events.sort_by_key(|e| e.tick);
    t
}

/// Our recordings' extension (moho's are `.SCFAReplay`, FAF's `.fafreplay`).
pub const EXTENSION: &str = "rtsreplay";

/// The profile games are recorded for when none is known (a `--session`
/// game): the one a fresh install's preferences have (`ui::prefs`).
pub const DEFAULT_PROFILE: &str = "Player";

/// FAF's file picker refuses longer names (`filepicker.lua`
/// `maxFilenameSize`): the dialog couldn't open the file.
pub const MAX_NAME: usize = 30;

/// Whether `text` starts as one of our recordings (with a header).
pub fn is_recording(text: &str) -> bool {
    text.lines().next().is_some_and(|l| l.trim_end() == MAGIC)
}

/// A profile's folder in `root` (characters a folder name can't hold
/// become `_`).
pub fn profile_dir(root: &Path, profile: &str) -> PathBuf {
    let name: String = profile.chars().map(|c| if c.is_control() || r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let name = name.trim_matches(['.', ' ']);
    root.join(if name.is_empty() { DEFAULT_PROFILE } else { name })
}

/// `path` as it is on disk when only its profile folder (the file's parent)
/// differs in case: FAF's file picker lowercases the profile name, which a
/// case-sensitive file system (Linux) won't find. `path` itself otherwise.
pub fn find_file(path: &Path) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    let (Some(file), Some(profile), Some(root)) = (path.file_name(), path.parent().and_then(|p| p.file_name()), path.parent().and_then(|p| p.parent())) else {
        return path.to_path_buf();
    };
    let wanted = profile.to_string_lossy();
    let found = std::fs::read_dir(root).ok().and_then(|entries| {
        let mut matches: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&wanted))).collect();
        matches.sort();
        matches.into_iter().map(|dir| dir.join(file)).find(|p| p.exists())
    });
    found.unwrap_or_else(|| path.to_path_buf())
}

/// The recordings in `root` by profile folder (`GetSpecialFiles`): each
/// profile's base names, sorted (a stable order for the interface).
pub fn list(root: &Path) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(profiles) = std::fs::read_dir(root) else { return out };
    for profile in profiles.flatten().filter(|e| e.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(profile.path()) else { continue };
        let mut names: Vec<String> = files
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .collect();
        names.sort();
        out.insert(profile.file_name().to_string_lossy().into_owned(), names);
    }
    out
}
/// The header lines for a game of `session` played by `build`.
pub fn header(build: &str, session: &SessionConfig) -> String {
    let mut bytes = Vec::new();
    encode_session(session).encode(&mut bytes);
    format!("{MAGIC}\n# build {build}\n# session {}\n", data_encoding::HEXLOWER.encode(&bytes))
}

/// Writes the header to `out`, flushed (commands flush as they're written,
/// so a game closed abruptly keeps all it recorded).
pub fn write_header(out: &mut impl Write, build: &str, session: &SessionConfig) -> std::io::Result<()> {
    out.write_all(header(build, session).as_bytes())?;
    out.flush()
}

/// What a replay's header says: the build that recorded it and the session
/// (both absent in recordings from before headers).
#[derive(Debug, Default, PartialEq)]
pub struct Header {
    pub build: Option<String>,
    pub session: Option<SessionConfig>,
}

/// Reads the header lines of a replay's text.
pub fn read_header(text: &str) -> Result<Header, String> {
    let mut h = Header::default();
    for line in text.lines().take_while(|l| l.starts_with('#')) {
        if let Some(b) = line.strip_prefix("# build ") {
            h.build = Some(b.trim().to_string());
        } else if let Some(hex) = line.strip_prefix("# session ") {
            let bytes = data_encoding::HEXLOWER.decode(hex.trim().as_bytes()).map_err(|e| format!("session header: {e}"))?;
            let data = LuaData::decode(&mut bytes.as_slice())?;
            h.session = Some(decode_session(&data)?);
        }
    }
    Ok(h)
}

/// A default recording's path in `dir`: `YYYY-MM-DD_HH-MM-SS_<map>.rtsreplay`
/// (UTC; the map is its folder name, e.g. `scmp_009`, cut so the name fits
/// [`MAX_NAME`]).
pub fn default_path(dir: &Path, now: std::time::SystemTime, session: &SessionConfig) -> PathBuf {
    let secs = now.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let stamp = utc_stamp(secs);
    let map = session_map(session);
    let map: String = map.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).take(MAX_NAME - stamp.len() - 1).collect();
    dir.join(format!("{stamp}_{map}.{EXTENSION}"))
}

/// Removes the oldest default recordings in `dir` past the newest `keep`
/// (their names sort by time; files named otherwise are left alone).
pub fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == EXTENSION) && p.file_stem().is_some_and(|n| is_default_name(&n.to_string_lossy())))
        .collect();
    files.sort();
    let excess = files.len().saturating_sub(keep);
    for old in &files[..excess] {
        let _ = std::fs::remove_file(old);
    }
}

/// Starts with a [`utc_stamp`] (`YYYY-MM-DD_HH-MM-SS`).
fn is_default_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 19 && b[..19].iter().enumerate().all(|(i, c)| match i {
        4 | 7 | 13 | 16 => *c == b'-',
        10 => *c == b'_',
        _ => c.is_ascii_digit(),
    })
}

/// The map a session plays: the lobby's `ScenarioFile`, else the scenario
/// path; its folder name.
fn session_map(session: &SessionConfig) -> String {
    let scenario = session
        .launch
        .as_ref()
        .and_then(|l| field(l, "Config"))
        .and_then(|c| field(c, "ScenarioFile"))
        .and_then(|s| match s {
            LuaData::String(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        })
        .unwrap_or_else(|| session.scenario.clone());
    let parts: Vec<&str> = scenario.split('/').filter(|p| !p.is_empty()).collect();
    parts.iter().rev().nth(1).or(parts.last()).map_or("game".to_string(), |s| s.to_lowercase())
}

/// `YYYY-MM-DD_HH-MM-SS` for seconds since 1970 (UTC).
fn utc_stamp(secs: u64) -> String {
    let t = utc_time(secs);
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// A UTC date and time (moho's `WriteTime` fields).
#[derive(Debug, PartialEq)]
pub struct UtcTime {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    /// Days since Sunday.
    pub weekday: i64,
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
}

/// Seconds since 1970 as a UTC date and time (civil date from days, Howard
/// Hinnant's algorithm).
pub fn utc_time(secs: u64) -> UtcTime {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // 1970-01-01 was a Thursday.
    UtcTime { year: y, month: m, day: d, weekday: (days + 4).rem_euclid(7), hour: rem / 3600, minute: rem / 60 % 60, second: rem % 60 }
}

fn field<'a>(t: &'a LuaData, key: &str) -> Option<&'a LuaData> {
    match t {
        LuaData::Table(fields) => fields.iter().find(|(k, _)| matches!(k, LuaData::String(s) if s == key.as_bytes())).map(|(_, v)| v),
        _ => None,
    }
}

fn s(v: &str) -> LuaData {
    LuaData::string(v)
}

fn opt_s(v: &Option<String>) -> LuaData {
    v.as_deref().map_or(LuaData::Nil, s)
}

/// The session as a table. Integers go as strings (`LuaData` numbers are
/// floats; a seed needs all 64 bits).
fn encode_session(c: &SessionConfig) -> LuaData {
    let int = |v: u64| s(&v.to_string());
    let auto = c.auto_teams.map(|a| a.key().to_string());
    LuaData::Table(vec![
        (s("scenario"), s(&c.scenario)),
        (s("ai_armies"), int(c.ai_armies.into())),
        (s("teams"), int(c.teams.into())),
        (s("ai"), LuaData::Bool(c.ai)),
        (s("seed"), int(c.seed)),
        (s("create_initial_units"), LuaData::Bool(c.create_initial_units)),
        (s("script"), opt_s(&c.script)),
        (s("human_army"), c.human_army.map_or(LuaData::Nil, |a| int(a.into()))),
        (s("prebuilt"), LuaData::Bool(c.prebuilt)),
        (s("launch"), c.launch.clone().unwrap_or(LuaData::Nil)),
        (s("auto_teams"), opt_s(&auto)),
        (s("fog_of_war"), opt_s(&c.fog_of_war)),
        (s("victory"), opt_s(&c.victory)),
        (s("mods"), string_list(&c.mods)),
        (s("ai_choice"), string_list(&c.ai_choice.values())),
    ])
}

fn string_list(v: &[String]) -> LuaData {
    LuaData::Table(v.iter().enumerate().map(|(i, m)| (LuaData::number(i as f64 + 1.0), s(m))).collect())
}

fn decode_list(v: Option<&LuaData>) -> Vec<String> {
    match v {
        Some(LuaData::Table(m)) => m.iter().filter_map(|(_, v)| match v {
            LuaData::String(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }).collect(),
        _ => Vec::new(),
    }
}

fn decode_session(t: &LuaData) -> Result<SessionConfig, String> {
    let get = |k: &str| field(t, k).filter(|v| **v != LuaData::Nil);
    let text = |k: &str| -> Option<String> {
        match get(k)? {
            LuaData::String(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    };
    let int = |k: &str| -> Result<Option<u64>, String> { text(k).map(|v| v.parse::<u64>().map_err(|e| format!("session {k}: {e}"))).transpose() };
    let flag = |k: &str| matches!(get(k), Some(LuaData::Bool(true)));
    let auto_teams = match text("auto_teams") {
        Some(k) => Some(AutoTeams::from_key(&k).ok_or_else(|| format!("session auto_teams: {k}"))?),
        None => None,
    };
    Ok(SessionConfig {
        scenario: text("scenario").ok_or("session has no scenario")?,
        ai_armies: int("ai_armies")?.unwrap_or(0) as u32,
        teams: int("teams")?.unwrap_or(0) as u32,
        ai: flag("ai"),
        seed: int("seed")?.unwrap_or(0),
        create_initial_units: flag("create_initial_units"),
        ticks: None,
        script: text("script"),
        human_army: int("human_army")?.map(|a| a as u32),
        prebuilt: flag("prebuilt"),
        launch: get("launch").cloned(),
        auto_teams,
        net_bot: false,
        fog_of_war: text("fog_of_war"),
        victory: text("victory"),
        mods: decode_list(get("mods")),
        ai_choice: AiChoice::parse(&decode_list(get("ai_choice"))).map_err(|e| format!("session ai_choice: {e}"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_reads_events_and_length() {
        let e = Event { tick: 812, kind: "acu death".into(), army: 3, blueprint: "uel0001".into(), position: [412.5, 20.25, 380.0] };
        assert_eq!(e.line(), "# event 812 acu_death 3 uel0001 412.5 20.25 380");
        let text = format!("# rts-core-engine replay 1\n# build x\n12 0 0 1 2 0 - - - 5 - 0 - - -\n# tick 100\n{}\n# event 900 nuke_launch 1 - 1 2 3\n# event bad\n950 0 0 1 2 0 - - - 5 - 0 - - -\n", e.line());
        let t = read_timeline(&text);
        assert_eq!(t.events.len(), 2);
        assert_eq!(t.events[0], Event { kind: "acu_death".into(), ..e });
        assert_eq!(t.events[1].blueprint, "-");
        assert_eq!(t.length, 950, "the last command");
        assert!(t.marked);
        assert!(!read_timeline("12 0 0 1 2 0 - - - 5 - 0 - - -\n").marked, "an older recording");
        assert_eq!(read_timeline(&(text + "# tick 1000\n# end 1043\n")).length, 1043);
    }

    #[test]
    fn session_round_trips_through_the_header() {
        let launch = LuaData::Table(vec![(s("Config"), LuaData::Table(vec![(s("ScenarioFile"), s("/maps/SCMP_009/SCMP_009_scenario.lua"))])), (s("LocalID"), s("1"))]);
        let session = SessionConfig {
            scenario: "/maps/scmp_009/scmp_009_scenario.lua".into(),
            ai_armies: 3,
            teams: 2,
            ai: true,
            seed: u64::MAX - 7,
            create_initial_units: true,
            ticks: None,
            script: Some("dev/scenes/fight.lua".into()),
            human_army: Some(1),
            prebuilt: false,
            launch: Some(launch),
            auto_teams: Some(AutoTeams::LeftRight),
            net_bot: false,
            fog_of_war: Some("explored".into()),
            victory: None,
            mods: vec!["74A9EAB2-E851-11DB-A1F1-F2C755D89593".into()],
            ai_choice: AiChoice { default: Some("RNGStandard".into()), armies: vec![(2, "adaptive".into())] },
        };
        let text = header("0.1.0+abcd1234, FAF 3839, Lua 0123abcd", &session) + "12 0 0 1 2 0 - - - 5 - 0 - - -\n";
        let h = read_header(&text).unwrap();
        assert_eq!(h.build.as_deref(), Some("0.1.0+abcd1234, FAF 3839, Lua 0123abcd"));
        assert_eq!(h.session, Some(session));
        assert_eq!(read_header("12 0 0 1 2 0 - - - 5 - 0 - - -\n").unwrap(), Header::default(), "an old recording: no header");
    }

    #[test]
    fn default_names_sort_by_time_and_name_the_map() {
        let session = SessionConfig { scenario: "/maps/SCMP_009/SCMP_009_scenario.lua".into(), ..Default::default() };
        let at = |secs| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        // 2026-10-11 08:30:05 UTC.
        assert_eq!(default_path(Path::new("r"), at(1_791_707_405), &session), Path::new("r/2026-10-11_08-30-05_scmp_009.rtsreplay"));
        let long = SessionConfig { scenario: "/maps/Setons Clutch.v0004/x_scenario.lua".into(), ..Default::default() };
        assert_eq!(default_path(Path::new("r"), at(1_791_707_405), &long), Path::new("r/2026-10-11_08-30-05_setons_clu.rtsreplay"), "cut to the file picker's 30");
        assert_eq!(utc_stamp(0), "1970-01-01_00-00-00");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29_00-00-00", "leap day");
        assert_eq!(utc_time(1_791_707_405).weekday, 0, "2026-10-11 is a Sunday");
    }

    #[test]
    fn prune_keeps_the_newest() {
        let dir = std::env::temp_dir().join(format!("rts-replays-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for d in 1..=5 {
            std::fs::write(dir.join(format!("2026-10-0{d}_00-00-00_x.rtsreplay")), "").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        std::fs::write(dir.join("0 my best game.rtsreplay"), "").unwrap();
        prune(&dir, 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["0 my best game.rtsreplay", "2026-10-04_00-00-00_x.rtsreplay", "2026-10-05_00-00-00_x.rtsreplay", "notes.txt"], "a renamed one is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recordings_list_by_profile_in_name_order() {
        let root = std::env::temp_dir().join(format!("rts-replay-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (profile, name) in [("Player", "b"), ("Player", "a"), ("Zed", "c")] {
            let dir = profile_dir(&root, profile);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.rtsreplay")), MAGIC).unwrap();
        }
        std::fs::write(profile_dir(&root, "Zed").join("old.SCFAReplay"), "").unwrap();
        std::fs::write(root.join("loose.rtsreplay"), "").unwrap();
        let listed: Vec<(String, Vec<String>)> = list(&root).into_iter().collect();
        assert_eq!(listed, [("Player".to_string(), vec!["a".to_string(), "b".to_string()]), ("Zed".to_string(), vec!["c".to_string()])]);
        assert_eq!(profile_dir(&root, "a/b:c"), root.join("a_b_c"));
        assert_eq!(profile_dir(&root, " "), root.join(DEFAULT_PROFILE));
        assert!(is_recording(&std::fs::read_to_string(profile_dir(&root, "Zed").join("c.rtsreplay")).unwrap()));
        assert_eq!(find_file(&root.join("zed/c.rtsreplay")).canonicalize().unwrap(), root.join("Zed/c.rtsreplay").canonicalize().unwrap(), "the picker's lowercased profile");
        assert_eq!(find_file(&root.join("nobody/c.rtsreplay")), root.join("nobody/c.rtsreplay"));
        assert!(!is_recording("12 0 0 1 2 0 - - - 5 - 0 - - -\n"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
