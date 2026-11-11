//! Compiled Luau bytecode cache for VFS scripts.
//!
//! Compiling is pure and deterministic, so every `.lua`/`.bp` file can be
//! translated + compiled on all cores ahead of time ([`ScriptCache::prewarm`],
//! run on a background thread) while the single-threaded Lua VM keeps going.
//! On a miss the caller compiles inline; if both race, they produce identical
//! bytes, so which one wins doesn't matter.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use mlua::{Compiler, Error as LuaError};

use crate::engine::vfs::Vfs;

/// Compiled bytecode, or the syntax error message for scripts that don't compile.
pub type Compiled = Result<Arc<[u8]>, Arc<str>>;

pub struct ScriptCache {
    vfs: Arc<RwLock<Vfs>>,
    compiler: Compiler,
    cache: RwLock<HashMap<String, Compiled>>,
    /// Every script that changed on disk (`--enable-disk-watch`), in the
    /// order seen: each VM tells its Lua the ones after its own cursor
    /// (`engine::disk_watch::deliver`).
    changed: RwLock<Vec<String>>,
}

impl std::fmt::Debug for ScriptCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptCache").field("scripts", &self.cache.read().map_or(0, |c| c.len())).finish()
    }
}

/// Prewarm order, Lua's demand order at startup: `/lua/system` (the
/// blueprint loader `RuleInit` imports), then every `.bp` by path, which is
/// FAF's blueprint load order (`/effects`, `/env`, `/meshes`,
/// `/projectiles`, `/props`, `/units`), then the rest (the sim and
/// interface scripts, unit scripts: needed later).
fn prewarm_group(path: &str) -> u8 {
    if path.starts_with("/lua/system/") {
        0
    } else if path.ends_with(".bp") {
        1
    } else {
        2
    }
}

/// The Luau compiler every game script is compiled with: the script cache
/// and every VM that loads source (`Lua::set_compiler`) use this one, so a
/// chunk behaves the same whether it came from the cache or not.
///
/// Optimization level 1, mlua's default, set explicitly. Level 2 (inlining,
/// loop unrolling) measured no faster on FAF's Lua (Lua stages within
/// 0.2% over a 6000-tick AI game) and would shift `debug.getinfo` stack
/// levels. Changing it makes existing replay keyframes unloadable (their
/// heap names protos by creation order).
///
/// Debug level 2 (local and upvalue names) with the Lua debugger on
/// (`--lua-debug`), else 1 (line numbers and function names). Names only:
/// the code is the same.
pub fn compiler() -> Compiler {
    let debug = if crate::engine::lua::debugger::enabled() { 2 } else { 1 };
    Compiler::new().set_optimization_level(1).set_debug_level(debug)
}

impl ScriptCache {
    pub fn new(vfs: Arc<RwLock<Vfs>>) -> Self {
        Self {
            vfs,
            compiler: compiler(),
            cache: RwLock::new(HashMap::new()),
            changed: RwLock::new(Vec::new()),
        }
    }

    /// `path` changed on disk: its translated source and bytecode are
    /// dropped (the next load reads the file again), and it joins the
    /// changes the VMs are told of.
    pub fn changed(&self, path: &str) {
        let key = Self::key(path);
        self.vfs.read().unwrap().invalidate(&key);
        self.cache.write().unwrap().remove(&key);
        self.changed.write().unwrap().push(key);
    }

    /// The changes after `cursor` (an index into every change so far),
    /// moving it past them.
    pub fn changes_since(&self, cursor: &mut usize) -> Vec<String> {
        let changed = self.changed.read().unwrap();
        let new = changed.get(*cursor..).unwrap_or_default().to_vec();
        *cursor = changed.len();
        new
    }

    /// How many changes there have been (a new VM's cursor: it loads
    /// everything fresh).
    pub fn change_count(&self) -> usize {
        self.changed.read().unwrap().len()
    }

    /// The VFS the scripts come from.
    pub fn vfs(&self) -> &Arc<RwLock<Vfs>> {
        &self.vfs
    }

    fn key(path: &str) -> String {
        let s = path.replace('\\', "/").to_lowercase();
        if s.starts_with('/') { s } else { format!("/{s}") }
    }

    /// Bytecode for `path`. `Err(NotFound)` if the file isn't in the VFS.
    pub fn get(&self, path: &str) -> io::Result<Compiled> {
        let key = Self::key(path);
        if let Some(hit) = self.cache.read().unwrap().get(&key) {
            return Ok(hit.clone());
        }
        // A miss: the prewarm hasn't reached it yet, so it compiles here, on
        // the caller's (Lua) thread.
        let _s = crate::prof_span!("script cache miss", path = %key);
        let compiled = self.compile(&key)?;
        self.cache.write().unwrap().insert(key, compiled.clone());
        Ok(compiled)
    }

    fn compile(&self, key: &str) -> io::Result<Compiled> {
        self.compile_with_hooks(key, &[])
    }

    /// Compiles `path` with the hook folders `extra` applied after the
    /// VFS's own (a session's mods, `engine::mods::ModHooks`), uncached:
    /// the bytecode depends on the VM's mods, so the caller keeps it.
    pub fn compile_with_hooks(&self, path: &str, extra: &[String]) -> io::Result<Compiled> {
        let source: Arc<[u8]> = {
            let vfs = self.vfs.read().unwrap();
            // Without extra hooks the VFS's own cache holds the translated
            // source, shared with every other session in the process.
            if extra.is_empty() {
                vfs.read_processed(path)?
            } else {
                Arc::from(vfs.read_processed_with_hooks(path, extra)?)
            }
        };
        let _s = crate::prof_span!("luau compile");
        Ok(match self.compiler.compile(&source) {
            Ok(bc) => Ok(Arc::from(bc)),
            Err(LuaError::SyntaxError { message, .. }) => Err(Arc::from(message)),
            Err(e) => Err(Arc::from(e.to_string())),
        })
    }

    /// Compiles every script in the VFS in parallel and fills the cache.
    pub fn prewarm(&self) {
        let _s = crate::prof_span!("script cache prewarm");
        let start = std::time::Instant::now();
        let mut paths: Vec<String> = {
            let vfs = self.vfs.read().unwrap();
            vfs.sorted_paths()
                .iter()
                .filter(|p| p.ends_with(".lua") || p.ends_with(".bp"))
                .cloned()
                .collect()
        };
        paths.sort_by(|a, b| (prewarm_group(a), a).cmp(&(prewarm_group(b), b)));
        // Lanes on the global pool, each taking the next file in demand
        // order, so the files Lua needs first are ready first. Two cores are
        // left to the Lua threads that are startup's critical path (the
        // sim's blueprint load and the interface's, in parallel); on a
        // machine with few cores whatever the prewarm hasn't reached is
        // compiled by Lua itself, the same work.
        let lanes = crate::threads::cores().saturating_sub(2).max(1);
        let next = AtomicUsize::new(0);
        let count = AtomicUsize::new(0);
        rayon::scope(|s| {
            for _ in 0..lanes {
                s.spawn(|_| {
                    while let Some(p) = paths.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if self.cache.read().unwrap().contains_key(p) {
                            continue;
                        }
                        let _s = crate::prof_span!("compile script (worker)", path = %p);
                        if let Ok(c) = self.compile(p) {
                            self.cache.write().unwrap().entry(p.clone()).or_insert(c);
                            count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        let count = count.into_inner();
        tracing::info!(
            count,
            ms = start.elapsed().as_millis() as u64,
            "script cache prewarmed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::vfs::MountPoint;

    fn vfs_with(files: &[(&str, &str)]) -> (Arc<RwLock<Vfs>>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "script_cache_{}_{}",
            std::process::id(),
            files.len()
        ));
        for (name, body) in files {
            let p = dir.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let vfs = Vfs::new(
            vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }],
            vec![],
        )
        .unwrap();
        (Arc::new(RwLock::new(vfs)), dir)
    }

    #[test]
    fn cached_bytecode_runs_and_reports_syntax_errors() {
        let (vfs, dir) = vfs_with(&[("ok.lua", "return 1 + 2"), ("bad.lua", "return +")]);
        let cache = ScriptCache::new(vfs);
        cache.prewarm();

        let lua = mlua::Lua::new();
        let bc = cache.get("/OK.lua").unwrap().unwrap();
        let v: i64 = lua.load(&*bc).eval().unwrap();
        assert_eq!(v, 3);
        assert!(cache.get("/bad.lua").unwrap().is_err());
        assert!(cache.get("/missing.lua").is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
