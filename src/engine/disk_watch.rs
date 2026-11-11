//! `--enable-disk-watch` (the game's `/EnableDiskWatch`): game files edited
//! on disk while the game runs are reloaded, as moho's `ScrDiskWatcherTask`
//! does, for modding without restarting.
//!
//! - **Watched**: the VFS's folder mounts (a file in an archive can't
//!   change), one watcher per set of game files ([`watch`]). Editors write a
//!   save as several events, so a change is taken once the folder has been
//!   quiet for [`QUIET`]. Only files already in the VFS count.
//! - **Forgotten**: a changed file's translated source and bytecode are
//!   dropped ([`ScriptCache::forget`]); a changed hook file counts as a
//!   change to the script it hooks too ([`Vfs::changed_scripts`]). Each
//!   change joins one log ([`changes_since`]) that every consumer reads
//!   from its own cursor: each VM ([`deliver`]) and the renderer (models,
//!   textures, emitters: `render::scene`, `render::ui`).
//! - **Scripts**: each VM calls every function in its `__diskwatch` table
//!   with the path (lowercase, as `import` keys its modules) and a reason.
//!   FAF's `import.lua` puts `dirty_module` there, which runs the module's
//!   `__moduleinfo.OnDirty`, takes it and every module that imported it out
//!   of `__modules`, and the next `import` runs the file again (`OnReload`
//!   then gets the new module).
//! - **Blueprints**: a changed `.bp` is passed to the VM's
//!   `ReloadBlueprint` (FAF's `Blueprints.lua`), which runs the file and the
//!   blueprint pipeline (`ModBlueprints`, ...) on it alone and registers the
//!   result again. A blueprint registered again is merged into the one
//!   registered before, in place (`blueprints::reloading`): its table stays
//!   the same, so units already out see the new values in Lua, and a field
//!   the new file leaves out keeps its old value, as FAF's notes on moho's
//!   reload describe. The caller then refreshes what it derived from the
//!   blueprints (the `after_blueprints` of [`deliver`]: defaults,
//!   categories, the sim's per-blueprint caches). What the sim copied into a
//!   unit when it was made (health, speed, weapons) stays until a new one
//!   is made, as in moho ("some changes will be picked up by existing
//!   units, some not until a new unit of that type is created").
//! - **When**: the interface is told every frame; the sim before each
//!   tick, except in a networked game, where reloading on one machine
//!   would desync it.
//!
//! Moho's exact arguments to the `__diskwatch` functions aren't confirmed
//! from the binary: FAF's `dirty_module(name, why)` only needs `why` to be
//! truthy (it then logs "changed on disk").

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, mpsc};
use std::time::Duration;

use mlua::prelude::*;
use notify::{EventKind, RecursiveMode, Watcher as _};
use tracing::{info, warn};

use crate::engine::lua::script_cache::ScriptCache;
use crate::engine::vfs::{Vfs, normalise};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Every file that changed on disk (lowercase VFS paths), in the order seen.
static CHANGES: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// How long a folder must be quiet before its changes are taken.
const QUIET: Duration = Duration::from_millis(100);

/// Turns disk watching on (at startup, before the game files are built).
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The changes after `cursor` (an index into every change so far), moving
/// it past them; each path once. Nothing (one atomic load) with disk
/// watching off.
pub fn changes_since(cursor: &mut usize) -> Vec<String> {
    if !enabled() {
        return Vec::new();
    }
    changes_after(cursor)
}

fn changes_after(cursor: &mut usize) -> Vec<String> {
    let changes = CHANGES.read().unwrap();
    let mut seen = BTreeSet::new();
    let new = changes.get(*cursor..).unwrap_or_default().iter().filter(|p| seen.insert(p.as_str())).cloned().collect();
    *cursor = changes.len();
    new
}

/// How many changes there have been: a new consumer's cursor (it loads
/// everything fresh).
pub fn change_count() -> usize {
    CHANGES.read().unwrap().len()
}

/// Records that `path` changed on disk, dropping its compiled script from
/// `cache`.
fn changed(cache: &ScriptCache, path: &str) {
    let path = normalise(path);
    cache.forget(&path);
    CHANGES.write().unwrap().push(path);
}

/// A running watcher; dropping it stops it.
pub struct Watcher {
    _inner: notify::RecommendedWatcher,
}

/// Watches the folders `vfs` mounts, recording each changed VFS file.
/// `None` when disk watching is off or the OS watcher fails.
pub fn watch(vfs: &Arc<RwLock<Vfs>>, cache: &Arc<ScriptCache>) -> Option<Watcher> {
    if enabled() { watch_folders(vfs, cache) } else { None }
}

fn watch_folders(vfs: &Arc<RwLock<Vfs>>, cache: &Arc<ScriptCache>) -> Option<Watcher> {
    let (tx, rx) = mpsc::channel::<PathBuf>();
    let mut inner = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    })
    .map_err(|e| warn!("disk watch: can't watch for changes: {e}"))
    .ok()?;
    let folders = vfs.read().unwrap().folder_sources();
    for folder in &folders {
        if let Err(e) = inner.watch(folder, RecursiveMode::Recursive) {
            warn!(folder = %folder.display(), "disk watch: can't watch {e}");
        }
    }
    info!(folders = folders.len(), "disk watch: watching the mounted folders");
    let (vfs, cache) = (vfs.clone(), cache.clone());
    std::thread::Builder::new()
        .name("disk-watch".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut paths = BTreeSet::from([first]);
                while let Ok(p) = rx.recv_timeout(QUIET) {
                    paths.insert(p);
                }
                let changes: BTreeSet<String> = {
                    let vfs = vfs.read().unwrap();
                    paths
                        .iter()
                        .map(|p| normalise(&vfs.disk_to_local(&p.to_string_lossy())))
                        .filter(|p| vfs.contains(p))
                        .flat_map(|p| vfs.changed_scripts(&p))
                        .collect()
                };
                for path in changes {
                    info!(path, "disk watch: changed on disk");
                    changed(&cache, &path);
                }
            }
        })
        .map_err(|e| warn!("disk watch: can't start its thread: {e}"))
        .ok()?;
    Some(Watcher { _inner: inner })
}

/// How far into the changes a VM has been told (VM app data).
struct Cursor(usize);

/// Tells `lua` of the files changed since it was last told: each function
/// in its `__diskwatch` table is called with `(path, "changed on disk")`,
/// then each changed blueprint goes to its `ReloadBlueprint`, and if any
/// did, `after_blueprints` refreshes what the VM derived from them. A Lua
/// function that errors is warned about and the rest still run. Does
/// nothing (one atomic load) with disk watching off.
pub fn deliver(lua: &Lua, after_blueprints: impl FnOnce(&Lua) -> LuaResult<()>) -> LuaResult<()> {
    if enabled() { deliver_changes(lua, after_blueprints) } else { Ok(()) }
}

fn deliver_changes(lua: &Lua, after_blueprints: impl FnOnce(&Lua) -> LuaResult<()>) -> LuaResult<()> {
    // A VM's first call starts from now: it loaded everything fresh.
    let mut cursor = lua.app_data_ref::<Cursor>().map_or_else(change_count, |c| c.0);
    let changes = changes_after(&mut cursor);
    lua.set_app_data(Cursor(cursor));
    if changes.is_empty() {
        return Ok(());
    }
    if let Some(hooks) = lua.app_data_ref::<crate::engine::mods::ModHooks>() {
        for path in &changes {
            hooks.forget(path);
        }
    }
    let globals = lua.globals();
    if let Some(watchers) = globals.raw_get::<Option<LuaTable>>("__diskwatch")? {
        let watchers: Vec<LuaFunction> = watchers.sequence_values().collect::<LuaResult<_>>()?;
        for path in &changes {
            for f in &watchers {
                if let Err(e) = f.call::<()>((path.as_str(), "changed on disk")) {
                    warn!(target: "lua", path, "__diskwatch: {e}");
                }
            }
        }
    }
    let blueprints: Vec<&String> = changes.iter().filter(|p| p.ends_with(".bp")).collect();
    let Some(reload) = globals.raw_get::<Option<LuaFunction>>("ReloadBlueprint")?.filter(|_| !blueprints.is_empty()) else { return Ok(()) };
    for path in blueprints {
        info!(path, "disk watch: reloading the blueprint");
        let result = crate::engine::api::blueprints::reloading(lua, || reload.call::<()>(path.as_str()));
        if let Err(e) = result {
            warn!(target: "lua", path, "ReloadBlueprint: {e}");
        }
    }
    after_blueprints(lua)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::vfs::MountPoint;

    /// A fresh folder with `/lua/<name>.lua` holding `src`. Tests share the
    /// change log, so each uses its own file name.
    fn folder(name: &str, src: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("disk_watch_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(dir.join(format!("lua/{name}.lua")), src).unwrap();
        dir
    }

    fn files(dir: &std::path::Path) -> (Arc<RwLock<Vfs>>, Arc<ScriptCache>) {
        let vfs = Vfs::new(vec![MountPoint { source: dir.to_path_buf(), mountpoint: "/".into() }], vec!["/schook".into()]).unwrap();
        vfs.register_transform("lua", Arc::new(|b: &[u8]| crate::engine::lua_translation::translate(&String::from_utf8_lossy(b)).into_bytes()));
        let vfs = Arc::new(RwLock::new(vfs));
        let cache = Arc::new(ScriptCache::new(vfs.clone()));
        (vfs, cache)
    }

    #[test]
    fn changed_script_is_read_again_and_told_to_lua() {
        let dir = folder("reload", "x = 1");
        let (_vfs, cache) = files(&dir);
        let lua = Lua::new();
        lua.set_app_data(cache.clone());
        crate::engine::api::dofile_or_script::register(&lua).unwrap();
        lua.load("__diskwatch = {} told = {} table.insert(__diskwatch, function(p, why) if p == '/lua/reload.lua' then table.insert(told, p) end end)").exec().unwrap();
        lua.load("doscript('/lua/reload.lua')").exec().unwrap();
        deliver_changes(&lua, |_| Ok(())).unwrap();
        assert_eq!(lua.globals().get::<i64>("x").unwrap(), 1);

        std::fs::write(dir.join("lua/reload.lua"), "x = 2").unwrap();
        changed(&cache, "/LUA/reload.lua");
        changed(&cache, "/lua/reload.lua");
        deliver_changes(&lua, |_| Ok(())).unwrap();
        assert_eq!(lua.load("return #told").eval::<i64>().unwrap(), 1, "told once");
        lua.load("doscript('/lua/reload.lua')").exec().unwrap();
        assert_eq!(lua.globals().get::<i64>("x").unwrap(), 2, "the new file runs");
        deliver_changes(&lua, |_| Ok(())).unwrap();
        assert_eq!(lua.load("return #told").eval::<i64>().unwrap(), 1, "each change is told once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_blueprint_goes_to_reload_blueprint() {
        let dir = folder("bp_reload", "");
        let (_vfs, cache) = files(&dir);
        let lua = Lua::new();
        lua.set_app_data(cache.clone());
        lua.load("reloaded = {} function ReloadBlueprint(f) if f == '/units/bp_reload/bp_reload_unit.bp' then table.insert(reloaded, f) end end").exec().unwrap();
        deliver_changes(&lua, |_| Ok(())).unwrap();
        changed(&cache, "/units/bp_reload/bp_reload_unit.bp");
        let mut after = false;
        deliver_changes(&lua, |_| {
            after = true;
            Ok(())
        })
        .unwrap();
        assert_eq!(lua.load("return #reloaded").eval::<i64>().unwrap(), 1);
        assert!(after, "the VM refreshes what it derived from blueprints");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hook_change_changes_the_script_it_hooks() {
        let dir = folder("hooks", "");
        let (vfs, _) = files(&dir);
        let vfs = vfs.read().unwrap();
        assert_eq!(vfs.changed_scripts("/schook/lua/a.lua"), ["/schook/lua/a.lua", "/lua/a.lua"]);
        assert_eq!(vfs.changed_scripts("/mods/m/hook/lua/a.lua"), ["/mods/m/hook/lua/a.lua", "/lua/a.lua"]);
        assert_eq!(vfs.changed_scripts("/lua/a.lua"), ["/lua/a.lua"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_on_disk_reach_the_log() {
        let dir = folder("watch", "x = 1");
        let (vfs, cache) = files(&dir);
        let _watcher = watch_folders(&vfs, &cache).expect("watcher");
        // FSEvents can report writes from just before the watch began.
        std::thread::sleep(Duration::from_millis(300));
        let mut cursor = change_count();
        std::fs::write(dir.join("lua/watch.lua"), "x = 2").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while !seen.iter().any(|p| p == "/lua/watch.lua") && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            seen.extend(changes_after(&mut cursor));
        }
        assert!(seen.iter().any(|p| p == "/lua/watch.lua"), "{seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
