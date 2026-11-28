//! Guard for tests and benches that need data outside the repo: the FAF
//! checkout (`../fa`), the game install (`../Supreme Commander Forged
//! Alliance`), the unpacked original game, the FAF vault maps.
//!
//! Missing data fails the test, naming the path it looked for: a test that
//! quietly returns reports `ok` without having run (paths are relative to
//! the crate root, so from a git worktree under `.claude/worktrees/<name>/`
//! they resolve beside the worktree, not beside the main checkout). Set
//! `RTS_SKIP_DATA_TESTS=1` to skip such tests instead (CI without the data).

/// Environment variable that turns a missing-data failure into a skip.
pub const SKIP_ENV: &str = "RTS_SKIP_DATA_TESTS";

/// The FAF checkout's init file and the game install, relative to the crate
/// root (the development layout, `docs/guides/running.md`).
pub const INIT_LUA_PATH: &str = "../fa/setup/bin/init_local_development.lua";
pub const FA_PATH: &str = "../Supreme Commander Forged Alliance";

/// Whether skipping data tests was asked for (`RTS_SKIP_DATA_TESTS` set to
/// anything but empty or `0`).
pub fn skip_requested() -> bool {
    std::env::var(SKIP_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// `true` when every path in `paths` exists. Otherwise panics naming the
/// missing ones (as given and as resolved from the working directory), or,
/// with `RTS_SKIP_DATA_TESTS` set, prints that `what` is skipped and
/// returns `false`.
pub fn require<P: AsRef<std::path::Path>>(what: &str, paths: &[P]) -> bool {
    // A sim stage error ends a test's run rather than being passed over.
    crate::engine::sim::tick::set_strict_stage_errors(true);
    let missing: Vec<&std::path::Path> = paths.iter().map(AsRef::as_ref).filter(|p| !p.exists()).collect();
    if missing.is_empty() {
        return true;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let list = missing.iter().map(|p| format!("{} (looked at {})", p.display(), cwd.join(p).display())).collect::<Vec<_>>().join(", ");
    if skip_requested() {
        eprintln!("skipping {what}: not found: {list}");
        return false;
    }
    panic!("{what} needs data that isn't there: {list}. Put it there (in a git worktree, symlink it beside the worktree) or set {SKIP_ENV}=1 to skip");
}

/// [`require`] for the FAF checkout and the game install, what every engine
/// session needs.
pub fn require_game(what: &str) -> bool {
    require(what, &[INIT_LUA_PATH, FA_PATH])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_paths_pass() {
        assert!(require("this test", &["Cargo.toml", "src"]));
    }

    #[test]
    #[should_panic(expected = "definitely/not/here")]
    fn missing_path_fails_naming_it() {
        // Only meaningful without the opt-out; with it, fail the same way.
        if skip_requested() {
            panic!("definitely/not/here");
        }
        require("this test", &["Cargo.toml", "definitely/not/here"]);
    }
}
