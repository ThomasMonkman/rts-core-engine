//! Where the files the engine runs on live: FAF's Lua (an init file that
//! mounts it) and the Forged Alliance install (the game's data). Neither
//! ships with the engine, so each is found at startup, first match wins:
//!
//! 1. `--fa` / `--game-dir`, then `RTS_FA` / `RTS_GAME_DIR`;
//! 2. the development layout: `../fa` and `../Supreme Commander Forged
//!    Alliance` beside the working directory;
//! 3. the settings file (`settings.ini` in [`config_dir`]), which a folder
//!    picked in the "not found" dialog is saved to;
//!    (Between 2 and 3: the folder above the executable's, when it stands
//!    in for the game's `bin/SupremeCommander.exe`.)
//! 4. beside the executable (or beside the `.app` on macOS): `fa/`,
//!    `FAForever/`, `Supreme Commander Forged Alliance/`;
//! 5. the FAF client's data folder (`%ProgramData%\FAForever` on Windows,
//!    `~/.faforever` elsewhere): its `bin/init_faf.lua` and the game path
//!    in its `fa_path.lua`;
//! 6. Steam libraries (native, Flatpak and Snap Steam on Linux, as
//!    faf-linux's `setup.sh` looks), then GOG's default folders.
//!
//! `--fa` takes a FAF repository checkout (its `init_local_development.lua`),
//! a FAF client data folder (its `bin/init_faf.lua`) or an init file.
//! See docs/guides/packaging.md.

use std::path::{Path, PathBuf};

/// The game's folder name under Steam (and the dev layout's sibling).
const GAME_FOLDER: &str = "Supreme Commander Forged Alliance";

/// Resolved locations: the init file run to build the VFS mounts, and the
/// Forged Alliance install folder (`fa_path` in FAF's init files).
#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    pub init: PathBuf,
    pub game: PathBuf,
}

/// Locations given explicitly (command line), ahead of the search.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub fa: Option<PathBuf>,
    pub game: Option<PathBuf>,
}

/// What [`locate`] couldn't find (either may be missing).
#[derive(Debug, Default)]
pub struct Missing {
    pub fa: bool,
    pub game: bool,
}

impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.game {
            writeln!(f, "Supreme Commander: Forged Alliance wasn't found (a folder with gamedata/units.scd).")?;
            writeln!(f, "  Give its install folder with --game-dir <DIR> (or RTS_GAME_DIR).")?;
        }
        if self.fa {
            writeln!(f, "FAF's Lua wasn't found (the FAF client's data folder, or a checkout of github.com/FAForever/fa).")?;
            writeln!(f, "  Give it with --fa <DIR> (or RTS_FA).")?;
        }
        if let Some(p) = settings_path() {
            write!(f, "Both can also be set in {}", p.display())?;
        }
        Ok(())
    }
}

/// Finds both locations; see the module docs for the order.
pub fn locate(overrides: &Overrides) -> Result<Install, Missing> {
    let settings = read_settings();
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let explicit_fa = overrides.fa.clone().or_else(|| env("RTS_FA"));
    let explicit_game = overrides.game.clone().or_else(|| env("RTS_GAME_DIR"));

    let init = match &explicit_fa {
        // Given but wrong: say so rather than quietly using something else.
        Some(p) => init_file(p),
        None => fa_candidates(settings.fa).iter().find_map(|p| init_file(p)),
    };
    let game = match &explicit_game {
        Some(p) => is_game_dir(p).then(|| p.clone()),
        None => game_candidates(settings.game, init.as_deref()).into_iter().find(|p| is_game_dir(p)),
    };
    match (init, game) {
        (Some(init), Some(game)) => Ok(Install { init, game }),
        (init, game) => Err(Missing { fa: init.is_none(), game: game.is_none() }),
    }
}

/// The init file for `path`: the file itself, a FAF repository checkout's
/// `init_local_development.lua` (at the root before FAF 3839, in
/// `setup/bin/` since), or a FAF client data folder's `bin/init_faf.lua`.
pub fn init_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    ["init_local_development.lua", "setup/bin/init_local_development.lua", "bin/init_faf.lua"].iter().map(|f| path.join(f)).find(|f| f.is_file())
}

/// A Forged Alliance install: has the game's unit archive.
pub fn is_game_dir(path: &Path) -> bool {
    path.join("gamedata/units.scd").is_file()
}

fn fa_candidates(saved: Option<PathBuf>) -> Vec<PathBuf> {
    let mut c = vec![PathBuf::from("../fa")];
    c.extend(saved);
    for dir in exe_dirs() {
        c.push(dir.join("fa"));
        c.push(dir.join("FAForever"));
    }
    c.extend(faf_client_dir());
    c
}

fn game_candidates(saved: Option<PathBuf>, init: Option<&Path>) -> Vec<PathBuf> {
    let mut c = vec![Path::new("..").join(GAME_FOLDER)];
    // Standing in for the game's own `bin/SupremeCommander.exe` (as LOUD
    // runs it): the install the program is in, ahead of a saved pick.
    c.extend(exe_dirs().first().and_then(|d| d.parent()).map(Path::to_path_buf));
    c.extend(saved);
    c.extend(exe_dirs().into_iter().map(|d| d.join(GAME_FOLDER)));
    // The FAF client records the install it plays with in `fa_path.lua`,
    // beside its `bin/` (the chosen FAF data folder first).
    let faf_dirs = init.and_then(|i| i.parent()?.parent().map(Path::to_path_buf)).into_iter().chain(faf_client_dir());
    c.extend(faf_dirs.filter_map(|d| std::fs::read_to_string(d.join("fa_path.lua")).ok()).filter_map(|t| lua_string(&t, "fa_path")).map(PathBuf::from));
    for lib in steam_libraries() {
        c.push(lib.join("steamapps/common").join(GAME_FOLDER));
    }
    c.extend(gog_dirs());
    c
}

/// The folders the executable sits in: its own, and on macOS the folder
/// holding its `.app` bundle.
fn exe_dirs() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return vec![] };
    let app = dir.ancestors().nth(2).filter(|a| a.extension().is_some_and(|e| e == "app")).and_then(|a| a.parent()).map(Path::to_path_buf);
    std::iter::once(dir).chain(app).collect()
}

/// The FAF client's data folder: `%ProgramData%\FAForever` on Windows,
/// `~/.faforever` elsewhere (the Linux client's, faf-linux's
/// `~/.faforever/bin/ForgedAlliance.exe`).
fn faf_client_dir() -> Option<PathBuf> {
    let dir = if cfg!(windows) {
        std::env::var_os("ProgramData").map(PathBuf::from)?.join("FAForever")
    } else {
        home_dir()?.join(".faforever")
    };
    Some(dir).filter(|d| d.is_dir())
}

/// Every Steam library folder (each listed in the Steam install's
/// `steamapps/libraryfolders.vdf`), the Steam install's own first.
fn steam_libraries() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        roots.extend(["ProgramFiles(x86)", "ProgramFiles"].iter().filter_map(std::env::var_os).map(|p| PathBuf::from(p).join("Steam")));
    } else if let Some(home) = home_dir() {
        let xdg_data = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
        roots.extend(unix_steam_roots(&home, xdg_data));
    }
    let mut libs = Vec::new();
    for root in roots.into_iter().filter(|r| r.is_dir()) {
        let listed = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).map(|t| vdf_paths(&t)).unwrap_or_default();
        for lib in std::iter::once(root).chain(listed) {
            if !libs.contains(&lib) {
                libs.push(lib);
            }
        }
    }
    libs
}

/// Where Steam installs itself outside Windows: macOS's, then Linux's
/// native (`$XDG_DATA_HOME`, `~/.local/share`; `~/.steam/steam` links
/// there), Flatpak and Snap ones.
fn unix_steam_roots(home: &Path, xdg_data: Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = vec![home.join("Library/Application Support/Steam")];
    roots.extend(xdg_data.map(|d| d.join("Steam")));
    roots.extend([
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
    ]);
    roots
}

/// The `"path"` values in a Steam `libraryfolders.vdf`.
fn vdf_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("\"path\"")?.trim();
            Some(PathBuf::from(rest.strip_prefix('"')?.strip_suffix('"')?.replace("\\\\", "\\")))
        })
        .collect()
}

/// GOG installs: any "... Forged Alliance" folder in GOG's default places.
fn gog_dirs() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return vec![];
    }
    let mut parents = vec![PathBuf::from(r"C:\GOG Games")];
    parents.extend(std::env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join(r"GOG Galaxy\Games")));
    let mut found: Vec<PathBuf> = parents
        .iter()
        .filter_map(|p| std::fs::read_dir(p).ok())
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("forged alliance")))
        .collect();
    found.sort();
    found
}

/// `name = "value"` from a Lua file (FAF's `fa_path.lua`).
fn lua_string(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == name).then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// The engine's folder name in the OS's settings and cache folders. Every
/// local folder the engine writes goes through [`config_dir`] or
/// [`cache_dir`], which add it: don't spell it out elsewhere.
pub const APP_DIR: &str = "rts-core-engine";

/// The settings folder's earlier name, moved to [`APP_DIR`] on first use.
const OLD_APP_DIR: &str = "rts-engine";

/// `base`'s [`APP_DIR`], moving an [`OLD_APP_DIR`] there first if only
/// that exists (players' settings, prefs and replays from before the
/// rename). A failed move leaves the old folder, logged.
fn app_dir_in(base: &Path) -> PathBuf {
    let dir = base.join(APP_DIR);
    let old = base.join(OLD_APP_DIR);
    if !dir.exists() && old.is_dir() {
        match std::fs::rename(&old, &dir) {
            Ok(()) => tracing::info!("moved {} to {}", old.display(), dir.display()),
            Err(e) => tracing::warn!("couldn't move {} to {}: {e}", old.display(), dir.display()),
        }
    }
    dir
}

/// The engine's own folder for settings and logs: `%APPDATA%\rts-core-engine`,
/// `~/Library/Application Support/rts-core-engine`, or
/// `~/.config/rts-core-engine` (`$XDG_CONFIG_HOME`).
pub fn config_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            home_dir().map(|h| h.join("Library/Application Support"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home_dir().map(|h| h.join(".config")))
        };
        base.map(|b| app_dir_in(&b))
    })
    .clone()
}

/// The engine's folder for caches it can rebuild (generated mip levels):
/// `%LOCALAPPDATA%\rts-core-engine\cache`, `~/Library/Caches/rts-core-engine`,
/// or `~/.cache/rts-core-engine` (`$XDG_CACHE_HOME`). Safe to delete.
pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join(APP_DIR).join("cache"))
    } else if cfg!(target_os = "macos") {
        home_dir().map(|h| h.join("Library/Caches").join(APP_DIR))
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| home_dir().map(|h| h.join(".cache"))).map(|b| b.join(APP_DIR))
    }
}

/// Where games are recorded by default: `replays/` in a source checkout's
/// working directory (beside `game.log`), else in [`config_dir`].
pub fn replays_dir() -> Option<PathBuf> {
    if is_dev_checkout() {
        return Some(PathBuf::from("replays"));
    }
    config_dir().map(|d| d.join("replays"))
}

/// Where replays' keyframes are kept (a folder per replay; see
/// `engine::keyframe::recorder`).
pub fn keyframes_dir() -> Option<PathBuf> {
    if is_dev_checkout() {
        return Some(PathBuf::from("replays/.keyframes"));
    }
    config_dir().map(|d| d.join("keyframes"))
}

/// The engine's own `Game.prefs` (the interface's options and key
/// bindings), in [`config_dir`].
pub fn prefs_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("Game.prefs"))
}

/// Forged Alliance's `Game.prefs`, which the FAF client and the retail game
/// share (`%LOCALAPPDATA%\Gas Powered Games\Supreme Commander Forged
/// Alliance`). Windows only: neither runs natively elsewhere.
pub fn faf_prefs_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(PathBuf::from)?;
    Some(local.join("Gas Powered Games").join(GAME_FOLDER).join("Game.prefs"))
}

/// Which `Game.prefs` the interface loads and which it saves to.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Prefs {
    pub load: Option<PathBuf>,
    pub save: Option<PathBuf>,
}

/// Picks the preference files: an explicit `--prefs` file is both read and
/// written. Otherwise the first that exists of `dev/Game.prefs` (a source
/// checkout's), the engine's own ([`prefs_path`]) and Forged Alliance's
/// ([`faf_prefs_path`]: imported, never written) is read, and the engine's
/// own is written. `fixed` (a `--screenshot` run) reads and writes no saved
/// file, so it looks the same on every machine.
pub fn choose_prefs(explicit: Option<PathBuf>, fixed: bool) -> Prefs {
    if let Some(p) = explicit {
        return Prefs { load: Some(p.clone()).filter(|p| p.is_file()), save: Some(p) };
    }
    if fixed {
        return Prefs::default();
    }
    let dev = is_dev_checkout().then(|| PathBuf::from("dev/Game.prefs"));
    if let Some(dev) = dev.filter(|p| p.is_file()) {
        return Prefs { load: Some(dev.clone()), save: Some(dev) };
    }
    let own = prefs_path();
    let load = own.iter().cloned().chain(faf_prefs_path()).find(|p| p.is_file());
    Prefs { load, save: own }
}

/// The settings file the found locations can be saved to.
pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.ini"))
}

/// Running from a source checkout (`cargo run` in the repo): logs and
/// debugging dumps go in the working directory, as they always have.
pub fn is_dev_checkout() -> bool {
    Path::new("Cargo.toml").is_file() && Path::new("src/main.rs").is_file()
}

/// Where the log goes unless `--log` says: `game.log` in a source checkout's
/// working directory, else beside the program (beside the `.app` on macOS:
/// files written inside a signed bundle break its signature), else in
/// [`config_dir`] when that can't be written (Program Files, or a macOS app
/// run from its download location, which is read-only).
pub fn default_log() -> PathBuf {
    if is_dev_checkout() {
        return PathBuf::from("game.log");
    }
    let beside = exe_dirs().pop();
    beside
        .into_iter()
        .chain(config_dir())
        .map(|dir| dir.join("game.log"))
        .find(|log| log.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::OpenOptions::new().append(true).create(true).open(log).is_ok())
        .unwrap_or_else(|| PathBuf::from("game.log"))
}

#[derive(Debug, Default, PartialEq)]
struct Settings {
    fa: Option<PathBuf>,
    game: Option<PathBuf>,
}

fn read_settings() -> Settings {
    settings_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_settings(&t)).unwrap_or_default()
}

/// `key = value` lines; `#` starts a comment line.
fn parse_settings(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().trim_matches('"');
        if v.is_empty() {
            continue;
        }
        match k.trim() {
            "fa" => s.fa = Some(PathBuf::from(v)),
            "game" => s.game = Some(PathBuf::from(v)),
            _ => {}
        }
    }
    s
}

/// Saves `install` to the settings file, so the next start finds it.
pub fn save(install: &Install) -> std::io::Result<PathBuf> {
    let path = settings_path().ok_or_else(|| std::io::Error::other("no settings folder"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    // The FAF folder rather than its init file when it's the usual one.
    let fa = match install.init.file_name().and_then(|n| n.to_str()) {
        Some("init_local_development.lua") => crate::engine::repository_root(&install.init).unwrap_or_else(|| install.init.parent().unwrap().to_path_buf()),
        Some("init_faf.lua") => install.init.parent().and_then(Path::parent).unwrap().to_path_buf(),
        _ => install.init.clone(),
    };
    let text = format!(
        "# Where rts-core-engine finds FAF's Lua (fa) and Supreme Commander: Forged Alliance (game).\n\
         # Delete a line to search for it again at the next start.\n\
         fa = {}\ngame = {}\n",
        fa.display(),
        install.game.display()
    );
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_settings_folder_moves_to_the_new_name() {
        let base = std::env::temp_dir().join(format!("rts-app-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(OLD_APP_DIR)).unwrap();
        std::fs::write(base.join(OLD_APP_DIR).join("settings.ini"), "fa = x\n").unwrap();
        let dir = app_dir_in(&base);
        assert_eq!(dir, base.join(APP_DIR));
        assert_eq!(std::fs::read_to_string(dir.join("settings.ini")).unwrap(), "fa = x\n");
        assert!(!base.join(OLD_APP_DIR).exists());
        // Both there: the new one is kept as it is, the old left alone.
        std::fs::create_dir_all(base.join(OLD_APP_DIR)).unwrap();
        assert_eq!(app_dir_in(&base), base.join(APP_DIR));
        assert!(base.join(OLD_APP_DIR).exists() && dir.join("settings.ini").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `--prefs` is read (when it exists) and written; a screenshot run
    /// touches no saved file; FAF's own file is never written.
    #[test]
    fn chooses_preference_files() {
        let given = PathBuf::from("no/such/Game.prefs");
        assert_eq!(choose_prefs(Some(given.clone()), true), Prefs { load: None, save: Some(given) });
        assert_eq!(choose_prefs(None, true), Prefs::default());
        let chosen = choose_prefs(None, false);
        assert!(chosen.save.is_some());
        assert_ne!(chosen.save, faf_prefs_path());
    }

    #[test]
    fn looks_for_flatpak_and_snap_steam() {
        let roots = unix_steam_roots(Path::new("/home/p"), Some(PathBuf::from("/data")));
        for want in ["/data/Steam", "/home/p/.local/share/Steam", "/home/p/.var/app/com.valvesoftware.Steam/.local/share/Steam", "/home/p/snap/steam/common/.local/share/Steam"] {
            assert!(roots.contains(&PathBuf::from(want)), "{want} not in {roots:?}");
        }
    }

    #[test]
    fn reads_steam_library_paths() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(vdf_paths(vdf), [PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\SteamLibrary")]);
    }

    #[test]
    fn reads_faf_clients_game_path() {
        let t = "fa_path = \"C:/Program Files (x86)/Steam/steamapps/common/Supreme Commander Forged Alliance\"\ncustom_vault_path = \"C:/x\"\nGameType = \"faf\"\n";
        assert_eq!(lua_string(t, "fa_path").as_deref(), Some("C:/Program Files (x86)/Steam/steamapps/common/Supreme Commander Forged Alliance"));
        assert_eq!(lua_string(t, "fa"), None);
    }

    #[test]
    fn settings_round_trip_lines() {
        let s = parse_settings("# comment\nfa = C:/x/fa\ngame = \"D:/Games/FA\"\nother = 1\n");
        assert_eq!(s, Settings { fa: Some("C:/x/fa".into()), game: Some("D:/Games/FA".into()) });
        assert_eq!(parse_settings("fa =\n"), Settings::default());
    }

    /// A repository checkout and a FAF client data folder each lead to
    /// their init file; a game folder needs its unit archive.
    #[test]
    fn recognises_fa_and_game_folders() {
        let dir = std::env::temp_dir().join(format!("install_test_{}", std::process::id()));
        let (repo, client, game) = (dir.join("fa"), dir.join("FAForever"), dir.join("game"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(client.join("bin")).unwrap();
        std::fs::create_dir_all(game.join("gamedata")).unwrap();
        std::fs::write(client.join("bin/init_faf.lua"), "").unwrap();
        // FAF 3839 on: the development init file is in setup/bin.
        std::fs::create_dir_all(repo.join("setup/bin")).unwrap();
        std::fs::write(repo.join("setup/bin/init_local_development.lua"), "").unwrap();
        assert_eq!(init_file(&repo), Some(repo.join("setup/bin/init_local_development.lua")));
        std::fs::write(repo.join("init_local_development.lua"), "").unwrap();
        assert_eq!(init_file(&repo), Some(repo.join("init_local_development.lua")));
        assert_eq!(init_file(&client), Some(client.join("bin/init_faf.lua")));
        assert_eq!(init_file(&dir), None);
        assert!(!is_game_dir(&game));
        std::fs::write(game.join("gamedata/units.scd"), "").unwrap();
        assert!(is_game_dir(&game));
        std::fs::remove_dir_all(&dir).ok();
    }
}
