//! Mods: the `mod_info.lua` files under `/mods`, the session's active ones
//! and their hooks.
//!
//! A game's sim mods come from the lobby's launch (`gameConfig.GameMods`)
//! or `--mod`. The interface starts with them, and the game's own
//! `/lua/SessionInit.lua` adds the player's UI mods (its `mods.lua`
//! `GetUiMods`, so FAF's and LOUD's rules each apply). A VM's mods are its
//! `__active_mods`: as moho's script loader (FA exe 0x4ce2c0), each script
//! is hooked, when it loads, by the hook folders (`<location><hookdir>`,
//! default `/hook`) of the mods `__active_mods` lists then ([`sync_hooks`]),
//! after the init file's hooks and the engine's own.
//!
//! Determinism: every peer runs the same sim mods, ordered the same way
//! (by uid, then each mod's `before` / `after`; see [`resolve`]), so the
//! hooked sim scripts are identical. UI-only mods never reach the sim VM.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::sync::{Arc, Mutex};

use mlua::prelude::*;
use rayon::prelude::*;
use tracing::{info, warn};

use crate::engine::lua::script_cache::{Compiled, ScriptCache};
use crate::engine::sim::lua_data::LuaData;
use crate::engine::vfs::{Vfs, normalise};

/// A mod's `mod_info.lua`, with FAF's defaults (`/lua/mods.lua`'s
/// `LoadModInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct ModInfo {
    pub uid: String,
    pub name: String,
    /// `version` as written (FAF's are integers).
    pub version: String,
    /// The mod's folder (`Dirname` of its `mod_info.lua`), normalised.
    pub location: String,
    /// `location .. hookdir` (default `/hook`), normalised, no trailing slash.
    pub hook_dir: String,
    pub ui_only: bool,
    /// Mods this one is applied before / after (`after` defaults to
    /// `requires`, as FAF's sort).
    pub before: Vec<String>,
    pub after: Vec<String>,
    /// The whole table, as `__active_mods` hands it to the scripts.
    pub info: LuaData,
}

/// Every mod in the VFS, by uid.
pub type Mods = BTreeMap<String, Arc<ModInfo>>;

/// Finds and reads every `mod_info.lua` under `/mods` (as `mods.lua`'s
/// `AllMods`, disabled ones left out; of two with one uid, the later path
/// wins), for what the engine resolves itself: launch uids, `--mod`, the
/// lobby's hashes. Which UI mods an interface runs is its game code's
/// choice (`SessionInit.lua`; FAF's leaves out `Hotstats`). Each file
/// runs in its own throwaway VM, in parallel.
pub fn discover(vfs: &Vfs) -> Mods {
    let files: Vec<&str> = match vfs.find_files("/mods", "*mod_info.lua") {
        Ok(f) => f.collect(),
        Err(_) => return Mods::new(),
    };
    let read: Vec<Option<ModInfo>> = files
        .par_iter()
        .map(|path| match read_mod_info(vfs, path) {
            Ok(m) => m,
            Err(e) => {
                warn!(path, "mod info: {e}");
                None
            }
        })
        .collect();
    let mut mods = Mods::new();
    for m in read.into_iter().flatten() {
        mods.insert(m.uid.clone(), Arc::new(m));
    }
    mods
}

fn read_mod_info(vfs: &Vfs, path: &str) -> LuaResult<Option<ModInfo>> {
    let source = vfs.read_processed(path).map_err(LuaError::external)?;
    let location = path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    let lua = Lua::new();
    lua.set_compiler(crate::engine::lua::script_cache::compiler());
    let env = lua.create_table()?;
    for (k, v) in [
        ("location", location.as_str()),
        ("name", path),
        ("description", "<LOC uimod_0006>(No description)"),
        ("author", ""),
        ("copyright", ""),
        ("icon", "/textures/ui/common/dialogs/mod-manager/generic-icon_bmp.dds"),
        ("hookdir", "/hook"),
        ("shadowdir", "/shadow"),
        ("uid", path),
    ] {
        env.raw_set(k, v)?;
    }
    env.raw_set("exclusive", false)?;
    env.raw_set("selectable", true)?;
    lua.load(&*source).set_name(path).set_environment(env.clone()).exec()?;
    // The mod can't move itself.
    env.raw_set("location", location.as_str())?;
    if env.raw_get::<LuaValue>("enabled")? == LuaValue::Boolean(false) {
        return Ok(None);
    }
    let text = |k: &str| -> LuaResult<String> {
        Ok(match env.raw_get::<LuaValue>(k)? {
            LuaValue::String(s) => s.to_string_lossy().to_string(),
            LuaValue::Integer(i) => i.to_string(),
            LuaValue::Number(n) => n.to_string(),
            _ => String::new(),
        })
    };
    let list = |k: &str| -> LuaResult<Option<Vec<String>>> {
        Ok(env.raw_get::<Option<LuaTable>>(k)?.map(|t| t.sequence_values::<String>().filter_map(Result::ok).collect()))
    };
    let name = text("name")?;
    let hookdir = text("hookdir")?;
    let after = match list("after")? {
        Some(a) => a,
        None => list("requires")?.unwrap_or_default(),
    };
    Ok(Some(ModInfo {
        uid: text("uid")?,
        name,
        version: text("version")?,
        hook_dir: hook_dir(&location, &hookdir),
        location,
        ui_only: env.raw_get::<Option<bool>>("ui_only").ok().flatten().unwrap_or(false),
        before: list("before")?.unwrap_or_default(),
        after,
        info: LuaData::from_lua(&LuaValue::Table(env)),
    }))
}

/// A mod's hook folder: `location .. hookdir`, normalised, no trailing slash.
fn hook_dir(location: &str, hookdir: &str) -> String {
    normalise(&format!("{location}/{}", hookdir.trim_start_matches('/'))).trim_end_matches('/').to_string()
}

/// The mods `uids` names that are in `all`, in the order they apply: by
/// uid, each after the mods it says it comes `after` (and before those it
/// comes `before`). The same set gives the same order on every machine
/// (FAF's lobby sorts with `pairs`, whose order a VM's history decides).
/// A cycle is broken by uid, with a warning, as FAF falls back to its own
/// order. Unknown uids are left out, with a warning.
pub fn resolve(all: &Mods, uids: &[String]) -> Vec<Arc<ModInfo>> {
    let mut chosen: BTreeMap<&str, &Arc<ModInfo>> = BTreeMap::new();
    for uid in uids {
        // FAF's uids are case-sensitive; a typed `--mod` may not be.
        match all.get(uid).or_else(|| all.values().find(|m| m.uid.eq_ignore_ascii_case(uid))) {
            Some(m) => {
                chosen.insert(m.uid.as_str(), m);
            }
            None => warn!(uid, "mods: not installed, left out"),
        }
    }
    // `first -> then` edges between the chosen mods.
    let mut then: BTreeMap<&str, BTreeSet<&str>> = chosen.keys().map(|&u| (u, BTreeSet::new())).collect();
    for (&uid, m) in &chosen {
        for b in m.before.iter().filter_map(|b| chosen.get_key_value(b.as_str()).map(|(k, _)| *k)) {
            then.get_mut(uid).unwrap().insert(b);
        }
        for a in m.after.iter().filter_map(|a| chosen.get_key_value(a.as_str()).map(|(k, _)| *k)) {
            then.get_mut(a).unwrap().insert(uid);
        }
    }
    let mut waiting: BTreeMap<&str, usize> = chosen.keys().map(|&u| (u, 0)).collect();
    for nexts in then.values() {
        for n in nexts {
            *waiting.get_mut(n).unwrap() += 1;
        }
    }
    let mut order = Vec::with_capacity(chosen.len());
    while !waiting.is_empty() {
        let next = match waiting.iter().find(|(_, n)| **n == 0) {
            Some((&u, _)) => u,
            None => {
                let (&u, _) = waiting.iter().next().unwrap();
                warn!(uid = u, "mods: their before / after lists form a cycle; broken by uid");
                u
            }
        };
        waiting.remove(next);
        for n in &then[next] {
            if let Some(w) = waiting.get_mut(n) {
                *w = w.saturating_sub(1);
            }
        }
        order.push(chosen[next].clone());
    }
    order
}

/// The sim mods a lobby's launch selected: `{Config = gameConfig, ...}`'s
/// `GameMods`, the lobby's `ModInfo` list (`Mods.GetGameMods`), or a set
/// of uids (`{[uid] = true}`).
pub fn uids_from_launch(launch: &LuaData) -> Vec<String> {
    let field = |t: &LuaData, k: &str| -> Option<LuaData> {
        let LuaData::Table(fields) = t else { return None };
        fields.iter().find(|(key, _)| *key == LuaData::string(k)).map(|(_, v)| v.clone())
    };
    let text = |v: &LuaData| match v {
        LuaData::String(s) => Some(String::from_utf8_lossy(s).into_owned()),
        _ => None,
    };
    let Some(LuaData::Table(mods)) = field(launch, "Config").and_then(|c| field(&c, "GameMods")) else { return Vec::new() };
    mods.iter()
        .filter_map(|(k, v)| match v {
            LuaData::Table(_) => field(v, "uid").as_ref().and_then(text),
            LuaData::Bool(true) => text(k),
            _ => None,
        })
        .collect()
}

/// A hash of the sim mods: uid, version and every file in each mod's
/// folder (paths and contents, in mod then path order). Peers whose
/// hashes differ run different sim code: it goes into the sim checksum
/// ([`ModsHash`]), so lockstep reports the desync at the first check.
pub fn hash(vfs: &Vfs, mods: &[Arc<ModInfo>]) -> u64 {
    let mut all = xxhash_rust::xxh3::Xxh3::new();
    for m in mods {
        all.update(m.uid.as_bytes());
        all.update(m.version.as_bytes());
        let files: Vec<&str> = vfs.find_files(&m.location, "*").map(|f| f.collect()).unwrap_or_default();
        let hashes: Vec<u64> = files.par_iter().map(|p| vfs.read(p).map_or(0, |b| xxhash_rust::xxh3::xxh3_64(&b))).collect();
        for (p, h) in files.iter().zip(hashes) {
            all.update(p.as_bytes());
            all.update(&h.to_le_bytes());
        }
    }
    all.digest()
}

/// An installed sim mod as peers compare it before a launch.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub name: String,
    pub version: String,
    /// [`hash`] of this mod alone.
    pub hash: u64,
}

/// Every installed sim mod (UI-only ones are each player's own), by uid,
/// each hashed ([`hash`]) in parallel.
pub fn hash_installed(vfs: &Vfs) -> BTreeMap<String, Installed> {
    let sim: Vec<Arc<ModInfo>> = discover(vfs).into_values().filter(|m| !m.ui_only).collect();
    let hashes: Vec<u64> = sim.par_iter().map(|m| hash(vfs, std::slice::from_ref(m))).collect();
    sim.into_iter().zip(hashes).map(|(m, hash)| (m.uid.clone(), Installed { name: m.name.clone(), version: m.version.clone(), hash })).collect()
}

static INSTALLED: std::sync::OnceLock<BTreeMap<String, Installed>> = std::sync::OnceLock::new();
static HASHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hashes the installed sim mods on their own thread at startup (as the
/// game code is, `net::core::hash_code_in_background`), for the lobby's
/// mod check; [`installed`] waits for it. Once per process: the hash is of
/// the installed mods, which every session sees.
pub fn hash_installed_in_background(vfs: Arc<std::sync::RwLock<Vfs>>) {
    static SPAWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SPAWNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    HASHING.store(true, std::sync::atomic::Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("mods-hash".into()).spawn(move || {
        let start = std::time::Instant::now();
        let installed = vfs.read().map(|v| hash_installed(&v)).unwrap_or_default();
        info!(mods = installed.len(), ms = start.elapsed().as_millis() as u64, "installed sim mods hashed");
        let _ = INSTALLED.set(installed);
    });
    if spawned.is_err() {
        let _ = INSTALLED.set(BTreeMap::new());
    }
}

/// The installed sim mods ([`hash_installed_in_background`]), waiting
/// for the hashing; empty if it never started.
pub fn installed() -> &'static BTreeMap<String, Installed> {
    static NONE: BTreeMap<String, Installed> = BTreeMap::new();
    if HASHING.load(std::sync::atomic::Ordering::SeqCst) { INSTALLED.wait() } else { INSTALLED.get().unwrap_or(&NONE) }
}

/// [`installed`] without waiting: `None` while the hashing runs.
pub fn installed_now() -> Option<&'static BTreeMap<String, Installed>> {
    static NONE: BTreeMap<String, Installed> = BTreeMap::new();
    if HASHING.load(std::sync::atomic::Ordering::SeqCst) { INSTALLED.get() } else { Some(INSTALLED.get().unwrap_or(&NONE)) }
}

/// Sim app data: [`hash`] of the session's sim mods (absent without any).
#[derive(Clone, Copy, Debug)]
pub struct ModsHash(pub u64);

/// A VM's mod hooks (app data): the active mods' hook folders, in order,
/// and the scripts they hook, compiled with them. Scripts no mod hooks
/// come from the shared [`ScriptCache`] as before; one lookup decides.
#[derive(Debug)]
pub struct ModHooks {
    dirs: Vec<String>,
    /// VFS keys of the scripts at least one mod hooks.
    hooked: HashSet<String>,
    compiled: Mutex<HashMap<String, Compiled>>,
}

impl ModHooks {
    pub fn new(vfs: &Vfs, mods: &[Arc<ModInfo>]) -> Self {
        Self::from_dirs(vfs, mods.iter().map(|m| m.hook_dir.clone()).collect())
    }

    fn from_dirs(vfs: &Vfs, dirs: Vec<String>) -> Self {
        let mut hooked = HashSet::new();
        for dir in &dirs {
            for f in vfs.find_files(dir, "*").into_iter().flatten() {
                if f.ends_with(".lua") || f.ends_with(".bp") {
                    hooked.insert(f[dir.len()..].to_string());
                }
            }
        }
        Self { dirs, hooked, compiled: Mutex::new(HashMap::new()) }
    }

    /// The hook folders, in the order they apply.
    pub fn dirs(&self) -> &[String] {
        &self.dirs
    }

    /// `path`'s bytecode with the mods' hooks when a mod hooks it, else
    /// `None` (the shared cache has it).
    pub fn get(&self, cache: &ScriptCache, path: &str) -> Option<io::Result<Compiled>> {
        if self.hooked.is_empty() {
            return None;
        }
        let key = normalise(path);
        if !self.hooked.contains(&key) {
            return None;
        }
        if let Some(c) = self.compiled.lock().unwrap().get(&key) {
            return Some(Ok(c.clone()));
        }
        let compiled = cache.compile_with_hooks(&key, &self.dirs);
        if let Ok(c) = &compiled {
            self.compiled.lock().unwrap().insert(key, c.clone());
        }
        Some(compiled)
    }

    /// `path`'s source with the VFS's and the mods' hooks (scripts run
    /// from source: `RuleInit.lua`).
    pub fn source(&self, vfs: &Vfs, path: &str) -> io::Result<Vec<u8>> {
        vfs.read_processed_with_hooks(path, &self.dirs)
    }
}

/// Makes `mods` the VM's active mods: `__active_mods` lists them (in
/// order) and their hooks apply to every script it loads from now on.
pub fn activate(lua: &Lua, vfs: &Vfs, mods: &[Arc<ModInfo>]) -> LuaResult<()> {
    let list = lua.create_table_with_capacity(mods.len(), 0)?;
    for m in mods {
        list.raw_push(m.info.to_lua(lua)?)?;
    }
    lua.globals().raw_set("__active_mods", list)?;
    if mods.is_empty() {
        lua.remove_app_data::<ModHooks>();
    } else {
        lua.set_app_data(ModHooks::new(vfs, mods));
    }
    Ok(())
}

/// The hook folders of the mods the VM's `__active_mods` lists now, in
/// order (a repeat left out): each entry's `location .. hookdir`, `hookdir`
/// defaulting to `/hook`, as moho's loader reads them.
fn active_hook_dirs(lua: &Lua) -> Vec<String> {
    let Ok(LuaValue::Table(list)) = lua.globals().raw_get::<LuaValue>("__active_mods") else { return Vec::new() };
    let mut dirs: Vec<String> = Vec::new();
    for entry in list.sequence_values::<LuaValue>().flatten() {
        let LuaValue::Table(m) = entry else { continue };
        let Ok(LuaValue::String(location)) = m.raw_get::<LuaValue>("location") else { continue };
        let hookdir = match m.raw_get::<LuaValue>("hookdir") {
            Ok(LuaValue::String(h)) => h.to_string_lossy().to_string(),
            _ => "/hook".to_string(),
        };
        let dir = hook_dir(&location.to_string_lossy(), &hookdir);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Brings the VM's [`ModHooks`] in line with its `__active_mods`, before a
/// script loads: moho's loader reads `__active_mods` at every load, so a
/// mod the game's Lua adds (FAF's and LOUD's `SessionInit.lua` add the UI
/// mods after `userInit.lua`) hooks the scripts loaded after that. `vfs`
/// is only asked for when the list changed.
pub fn sync_hooks(lua: &Lua, vfs: impl FnOnce() -> Option<Arc<std::sync::RwLock<Vfs>>>) {
    let dirs = active_hook_dirs(lua);
    if lua.app_data_ref::<ModHooks>().map(|h| h.dirs == dirs).unwrap_or(dirs.is_empty()) {
        return;
    }
    if dirs.is_empty() {
        lua.remove_app_data::<ModHooks>();
        return;
    }
    let Some(vfs) = vfs() else { return };
    let Ok(vfs) = vfs.read() else { return };
    lua.set_app_data(ModHooks::from_dirs(&vfs, dirs));
}

/// The VFS source of a script run from source, with the VM's mod hooks.
pub fn script_source(lua: &Lua, vfs: &Vfs, path: &str) -> io::Result<Arc<[u8]>> {
    let dirs = active_hook_dirs(lua);
    if !lua.app_data_ref::<ModHooks>().map(|h| h.dirs == dirs).unwrap_or(dirs.is_empty()) {
        if dirs.is_empty() {
            lua.remove_app_data::<ModHooks>();
        } else {
            lua.set_app_data(ModHooks::from_dirs(vfs, dirs));
        }
    }
    match lua.app_data_ref::<ModHooks>() {
        Some(h) => h.source(vfs, path).map(Arc::from),
        None => vfs.read_processed(path),
    }
}

/// What the sim's session runs, for the interface (its VM boots on the
/// render thread while the sim loads): the sim mods, UI-only mods asked
/// for with `--mod`, and every installed mod (the player's UI mods are
/// picked from those).
#[derive(Clone, Debug, Default)]
pub struct SessionMods {
    pub sim: Vec<Arc<ModInfo>>,
    pub ui: Vec<String>,
    pub all: Arc<Mods>,
}

static SESSION: Mutex<Option<SessionMods>> = Mutex::new(None);

/// The sim publishes its session's mods before loading blueprints.
pub fn publish(mods: SessionMods) {
    *SESSION.lock().unwrap() = Some(mods);
}

/// The current session's mods, once the sim has published them.
pub fn session() -> Option<SessionMods> {
    SESSION.lock().unwrap().clone()
}

/// Forgets the last session's mods (a new game is launching).
pub fn clear_session() {
    *SESSION.lock().unwrap() = None;
}

/// Logs the mods a VM runs (FAF's "Active mods" lines).
pub fn log_active(what: &str, mods: &[Arc<ModInfo>]) {
    for m in mods {
        info!(vm = what, uid = %m.uid, name = %m.name, version = %m.version, ui_only = m.ui_only, "active mod");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(uid: &str, before: &[&str], after: &[&str]) -> (String, Arc<ModInfo>) {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        (uid.to_string(), Arc::new(ModInfo { uid: uid.into(), name: uid.into(), version: "1".into(), location: format!("/mods/{uid}"), hook_dir: format!("/mods/{uid}/hook"), ui_only: false, before: s(before), after: s(after), info: LuaData::Nil }))
    }

    fn order(all: &Mods, uids: &[&str]) -> Vec<String> {
        resolve(all, &uids.iter().map(|s| s.to_string()).collect::<Vec<_>>()).iter().map(|m| m.uid.clone()).collect()
    }

    /// By uid, whatever order they were asked for in, then moved by
    /// `before` / `after`; unknown uids are left out.
    #[test]
    fn resolve_orders_by_uid_then_before_after() {
        let all: Mods = [m("a", &[], &["c"]), m("b", &[], &[]), m("c", &[], &[]), m("d", &["b"], &[])].into_iter().collect();
        assert_eq!(order(&all, &["b", "c", "x"]), ["b", "c"]);
        assert_eq!(order(&all, &["d", "c", "b", "a"]), ["c", "a", "d", "b"]);
        assert_eq!(order(&all, &["C", "a"]), ["c", "a"], "a typed uid's case doesn't matter");
        let cycle: Mods = [m("a", &[], &["b"]), m("b", &[], &["a"])].into_iter().collect();
        assert_eq!(order(&cycle, &["b", "a"]), ["a", "b"]);
    }

    #[test]
    fn launch_mods_are_a_list_or_a_set() {
        let s = LuaData::string;
        let launch = |mods| LuaData::Table(vec![(s("Config"), LuaData::Table(vec![(s("GameMods"), mods)]))]);
        let list = LuaData::Table(vec![(LuaData::number(1.0), LuaData::Table(vec![(s("name"), s("Resource Rich")), (s("uid"), s("74A9"))]))]);
        assert_eq!(uids_from_launch(&launch(list)), ["74A9"]);
        let set = LuaData::Table(vec![(s("74A9"), LuaData::Bool(true))]);
        assert_eq!(uids_from_launch(&launch(set)), ["74A9"]);
        assert!(uids_from_launch(&LuaData::Nil).is_empty());
    }

    /// A mod's `mod_info.lua` with FAF's defaults, and its hooks appended
    /// to the scripts they hook (after the VFS's own), in mod order.
    #[test]
    fn discovers_mods_and_hooks_scripts() {
        let dir = std::env::temp_dir().join(format!("mods_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, body) in [
            ("lua/a.lua", "base"),
            ("lua/b.lua", "untouched"),
            ("mods/one/mod_info.lua", "name = 'One'\nuid = 'one'\nversion = 2\n"),
            ("mods/one/hook/lua/a.lua", "one"),
            ("mods/two/mod_info.lua", "name = 'Two'\nuid = 'two'\nui_only = true\nhookdir = '/h'\n"),
            ("mods/two/h/lua/a.lua", "two"),
            ("mods/off/mod_info.lua", "name = 'Off'\nuid = 'off'\nenabled = false\n"),
        ] {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let vfs = Vfs::new(vec![crate::engine::vfs::MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        let all = discover(&vfs);
        assert_eq!(all.keys().collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!((all["one"].version.as_str(), all["one"].location.as_str(), all["one"].hook_dir.as_str(), all["one"].ui_only), ("2", "/mods/one", "/mods/one/hook", false));
        assert_eq!((all["two"].hook_dir.as_str(), all["two"].ui_only), ("/mods/two/h", true));
        let mods = resolve(&all, &["two".into(), "one".into()]);
        let hooks = ModHooks::new(&vfs, &mods);
        assert_eq!(String::from_utf8(hooks.source(&vfs, "/lua/a.lua").unwrap()).unwrap(), "base\none\ntwo");
        let cache = ScriptCache::new(Arc::new(std::sync::RwLock::new(Vfs::new(vec![], vec![]).unwrap())));
        assert!(hooks.get(&cache, "/lua/b.lua").is_none(), "no mod hooks it: the shared cache's");

        let lua = Lua::new();
        activate(&lua, &vfs, &mods).unwrap();
        let names: Vec<String> = lua.load("local t = {} for _, m in __active_mods do table.insert(t, m.name .. '@' .. m.location) end return t").eval().unwrap();
        assert_eq!(names, ["One@/mods/one", "Two@/mods/two"]);
        assert_ne!(hash(&vfs, &mods[..1]), hash(&vfs, &mods));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every mod is found, a `Hotstats` too: which UI mods run is the game
    /// code's choice (FAF's `mods.lua` leaves that one out). The hooks
    /// follow `__active_mods` as the game's Lua changes it, as moho's loader
    /// reads it at each load: a mod added there hooks the scripts loaded
    /// after, one taken out stops hooking.
    #[test]
    fn hooks_follow_active_mods() {
        let dir = std::env::temp_dir().join(format!("mods_sync_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, body) in [
            ("lua/a.lua", "base"),
            ("mods/hotstats/mod_info.lua", "name = 'Hotstats'\nuid = 'hs'\nui_only = true\n"),
            ("mods/hotstats/hook/lua/a.lua", "hs"),
            ("mods/sim/mod_info.lua", "name = 'Sim'\nuid = 'sim'\n"),
            ("mods/sim/hook/lua/a.lua", "sim"),
        ] {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let vfs = Vfs::new(vec![crate::engine::vfs::MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        let all = discover(&vfs);
        assert_eq!(all.keys().collect::<Vec<_>>(), ["hs", "sim"]);
        let lua = Lua::new();
        let src = |lua: &Lua| String::from_utf8(script_source(lua, &vfs, "/lua/a.lua").unwrap().to_vec()).unwrap();
        activate(&lua, &vfs, &resolve(&all, &["sim".into()])).unwrap();
        assert_eq!(src(&lua), "base\nsim");
        // As SessionInit.lua adds a UI mod (an info table of its own).
        lua.load("table.insert(__active_mods, {name = 'Hotstats', uid = 'hs', location = '/mods/hotstats'})").exec().unwrap();
        assert_eq!(src(&lua), "base\nsim\nhs");
        lua.load("__active_mods = {}").exec().unwrap();
        assert_eq!(src(&lua), "base");
        assert!(lua.app_data_ref::<ModHooks>().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
