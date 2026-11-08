use mlua::prelude::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::debug;

use crate::lua_api::lua_fn;

pub fn register_unsafe(lua: &Lua) -> LuaResult<()> {
    let io = lua.create_table()?;

    lua_fn!(lua, table(io, "io"),
        /// Opens a Lua file for reading (`:lines()`, `:read()`, `:close()`).
        /// Read-only and Lua files only: anything else is nil, as for a
        /// missing file.
        open = |lua, (path, mode): (String, Option<String>)| io_open(lua, (path, mode)))?;
    lua_fn!(lua, table(io, "io"),
        /// The file names matching a glob like `/dir/*.ext`, sorted, after
        /// `"."` and `".."` when the pattern matches them.
        dir = |lua, pattern: String| io_dir(lua, pattern))?;

    lua.globals().set("io", io)
}

/// io.open(path [, mode]) — returns a file handle table or nil.
/// Supports :lines() iterator and :close(). Read-only, and only Lua files
/// (FAF's init reads mods' `mod_info.lua`): anything else is nil, as for a
/// missing file.
fn io_open(lua: &Lua, (path, mode): (String, Option<String>)) -> LuaResult<LuaValue> {
    let mode = mode.unwrap_or_default();
    if mode.contains(['w', 'a', '+']) || !is_lua_file(&path) {
        debug!(path = %path, mode = %mode, "io.open: refused (read-only, Lua files only)");
        return Ok(LuaNil);
    }
    let Some(bytes) = std::fs::read(&path).ok() else {
        debug!(path = %path, "io.open: not found");
        return Ok(LuaNil);
    };

    debug!(path = %path, bytes = bytes.len(), "io.open: opened");
    let content = String::from_utf8_lossy(&bytes).into_owned();
    let lines: Arc<Vec<String>> = Arc::new(content.lines().map(str::to_owned).collect());

    let handle = lua.create_table()?;

    let lines_for_iter = lines.clone();
    handle.set(
        "lines",
        lua.create_function(move |lua, _self: LuaValue| {
            let lines = lines_for_iter.clone();
            let idx = Arc::new(AtomicUsize::new(0));
            lua.create_function(move |_, ()| {
                let i = idx.fetch_add(1, Ordering::Relaxed);
                Ok(lines.get(i).cloned())
            })
        })?,
    )?;

    handle.set("close", lua.create_function(|_, _self: LuaValue| Ok(()))?)?;

    handle.set(
        "read",
        lua.create_function({
            let lines = lines.clone();
            let idx = Arc::new(AtomicUsize::new(0));
            move |_, (_self, _fmt): (LuaValue, Option<String>)| {
                let i = idx.fetch_add(1, Ordering::Relaxed);
                Ok(lines.get(i).cloned())
            }
        })?,
    )?;

    Ok(LuaValue::Table(handle))
}

/// io.dir(pattern) — lists filenames matching a glob under a directory.
/// Pattern: "/abs/or/real/path/*.ext"
/// `"."` and `".."` come first, but only when the folder exists and the
/// pattern matches them (`*`, `*.*`), as `FindFirstFile` lists them: FAF's
/// init tests `IoDir(moddir .. "/mod_info.lua")[1]` for nil to skip a folder
/// without a mod info file (or a stray file, like a `.7z`, in `mods/`).
fn io_dir(lua: &Lua, pattern: String) -> LuaResult<LuaTable> {
    let table = lua.create_table()?;

    let (dir_str, glob) = split_disk_glob(&pattern);

    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut dots: Vec<&str> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir_str) {
        dots = [".", ".."].into_iter().filter(|d| glob_match(d, glob)).collect();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if glob_match(&name, glob) {
                names.insert(name);
            }
        }
    }

    debug!(dir = %dir_str, glob = glob, count = names.len(), "io.dir");

    let mut i = 1i64;
    for d in dots {
        table.set(i, d)?;
        i += 1;
    }
    for name in names {
        table.set(i, name)?;
        i += 1;
    }

    Ok(table)
}

/// A `.lua` file (any case): what the init VM may read.
pub fn is_lua_file(path: &str) -> bool {
    std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("lua"))
}

/// A real-disk path from an init file, written with Windows separators
/// (`InitFileDir .. '\\..\\gamedata\\*.scd'`), with `/` instead: every
/// platform's file system takes it.
pub fn disk_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Splits a disk glob (`C:\dir\*.scd`, either separator) into its folder
/// (separators normalised, see [`disk_path`]) and its file-name pattern.
pub fn split_disk_glob(pattern: &str) -> (String, &str) {
    match pattern.rfind(['/', '\\']) {
        Some(pos) => (disk_path(&pattern[..pos]), &pattern[pos + 1..]),
        None => (".".to_string(), pattern),
    }
}

/// Whether a file name matches a Windows-style pattern: `*` any run, `?` one
/// character, ASCII case ignored (`*.*` matches names without a dot too).
pub fn glob_match(name: &str, glob: &str) -> bool {
    if glob == "*" || glob == "*.*" {
        return true;
    }
    let lower = |s: &str| s.chars().map(|c| c.to_ascii_lowercase()).collect::<Vec<char>>();
    let (n, g) = (lower(name), lower(glob));
    // Iterative wildcard match, backtracking to the last `*`.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while i < n.len() {
        if j < g.len() && (g[j] == '?' || g[j] == n[i]) {
            i += 1;
            j += 1;
        } else if j < g.len() && g[j] == '*' {
            star = Some(j);
            mark = i;
            j += 1;
        } else if let Some(s) = star {
            j = s + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    g[j..].iter().all(|&c| c == '*')
}

/// The files and folders a disk glob in an init file's `mount_dir` names
/// (moho takes a wildcard in the last part: `gamedata\*.scd`), sorted by
/// name ignoring case, then by name, so the mount order is the same on every
/// machine. A pattern without a wildcard is itself, whether it exists or not.
pub fn expand_disk_glob(pattern: &str) -> Vec<std::path::PathBuf> {
    let (dir, glob) = split_disk_glob(pattern);
    if !glob.contains(['*', '?']) {
        return vec![std::path::PathBuf::from(disk_path(pattern))];
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| glob_match(n, glob)).collect())
        .unwrap_or_default();
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    names.into_iter().map(|n| std::path::Path::new(&dir).join(n)).collect()
}

#[cfg(test)]
mod tests {
    use mlua::prelude::*;

    /// The init VM reads Lua files and nothing else, never writes, and
    /// `os.remove` deletes nothing; no VM has Luau's disk `require`.
    #[test]
    fn init_vm_reads_lua_files_only() {
        let dir = std::env::temp_dir().join(format!("io_sandbox_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (lua_file, other) = (dir.join("mod_info.lua"), dir.join("secret.txt"));
        std::fs::write(&lua_file, "version = 3\n").unwrap();
        std::fs::write(&other, "secret").unwrap();
        let lua = Lua::new();
        crate::engine::api::register_unsafe(&lua).unwrap();
        lua.globals().set("lua_file", lua_file.to_string_lossy().as_ref()).unwrap();
        lua.globals().set("other", other.to_string_lossy().as_ref()).unwrap();
        let (read, txt, write, req): (Option<String>, bool, bool, bool) = lua
            .load(
                r#"
                local h = io.open(lua_file, 'rb')
                local line = h and h:lines()()
                os.remove(lua_file)
                os.remove(other)
                return line, io.open(other) ~= nil, io.open(lua_file, 'w') ~= nil, pcall(require, 'mod_info')
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(read.as_deref(), Some("version = 3"));
        assert!(!txt && !write && !req);
        assert!(lua_file.exists() && other.exists(), "os.remove must not delete");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Init files name disk paths with Windows separators and wildcards
    /// anywhere in the file name (LOUD's `BrewLAN-StrategicIcons*.scd`).
    #[test]
    fn disk_globs_take_windows_paths() {
        use super::*;
        assert!(glob_match("BrewLAN-StrategicIcons-LARGE.scd", "brewlan-strategicicons*.scd"));
        assert!(glob_match("units.scd", "*.scd") && !glob_match("units.scd.bak", "*.scd"));
        assert!(glob_match("a1", "a?") && !glob_match("a12", "a?") && glob_match("x", "*.*"));
        assert_eq!(split_disk_glob("C:\\fa\\bin\\..\\gamedata\\*.scd"), ("C:/fa/bin/../gamedata".to_string(), "*.scd"));
        let dir = std::env::temp_dir().join(format!("disk_glob_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["b.scd", "A.scd", "c.txt"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        assert_eq!(expand_disk_glob(&format!("{}\\*.scd", dir.display())), vec![dir.join("A.scd"), dir.join("b.scd")]);
        assert_eq!(expand_disk_glob(&format!("{}\\c.txt", dir.display())), vec![dir.join("c.txt")]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `io.dir` lists `.` and `..` only for a folder that exists and a
    /// pattern they match, so FAF's `IoDir(moddir .. "/mod_info.lua")[1]`
    /// is nil for a stray archive in `mods/` and init skips it.
    #[test]
    fn io_dir_lists_dots_only_when_they_match() {
        let dir = std::env::temp_dir().join(format!("io_dir_dots_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("mod")).unwrap();
        std::fs::write(dir.join("mod/mod_info.lua"), "").unwrap();
        std::fs::write(dir.join("stray.7z"), "not a zip").unwrap();
        let lua = Lua::new();
        crate::engine::api::register_unsafe(&lua).unwrap();
        lua.globals().set("d", dir.to_string_lossy().replace('\\', "/")).unwrap();
        let (stray, info, all, missing): (Option<String>, Option<String>, String, Option<String>) = lua
            .load(
                r#"
                local all = table.concat(io.dir(d .. '/mod/*.*'), ',')
                return io.dir(d .. '/stray.7z/mod_info.lua')[1], io.dir(d .. '/mod/mod_info.lua')[1], all, io.dir(d .. '/nope/*')[1]
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(stray, None);
        assert_eq!(info.as_deref(), Some("mod_info.lua"));
        assert_eq!(all, ".,..,mod_info.lua");
        assert_eq!(missing, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sim_vm_has_no_disk_require() {
        let lua = Lua::new();
        assert!(lua.globals().get::<LuaValue>("require").unwrap().is_function(), "Luau's default VM has one");
        crate::engine::api::remove_disk_access(&lua).unwrap();
        // Still defined (FAF's strict globals), but refuses.
        let (ok, err): (bool, String) = lua.load("local ok, e = pcall(require, 'x') return ok, tostring(e)").eval().unwrap();
        assert!(!ok && err.contains("disabled"), "{err}");
        assert!(lua.globals().get::<LuaValue>("package").unwrap().is_nil());
    }
}
