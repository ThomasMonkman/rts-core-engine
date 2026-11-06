//! Which game's Lua the mounts hold. The engine runs FAF's, LOUD's (a
//! standalone replacement for the base game's data) and anything else
//! built on the base game's. A few engine behaviours depend on it, each
//! named here, so the choice is made in one place.

use super::vfs::Vfs;

/// The game code the VFS holds, from files only it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameCode {
    /// FAF's: `/lua/version.lua`.
    Faf,
    /// LOUD's: `/lua/loudutilities.lua`.
    Loud,
    /// Anything else: the base game's, or a total conversion of it.
    Other,
}

impl GameCode {
    /// The game code mounted in `vfs` (two lookups).
    pub fn detect(vfs: &Vfs) -> Self {
        if vfs.contains("/lua/version.lua") {
            GameCode::Faf
        } else if vfs.contains("/lua/loudutilities.lua") {
            GameCode::Loud
        } else {
            GameCode::Other
        }
    }

    /// The name the engine's Lua sees (`session_launch.lua`).
    pub fn name(self) -> &'static str {
        match self {
            GameCode::Faf => "faf",
            GameCode::Loud => "loud",
            GameCode::Other => "other",
        }
    }

    /// Whether the engine's own hooks (`/engine_hook`: picture-in-picture,
    /// the replay timeline, the 63-slot lobby) apply: they extend FAF's
    /// scripts and assume FAF's tables (LOUD's `GameColors` has no
    /// `LobbyColorOrder`).
    pub fn engine_hooks(self) -> bool {
        self == GameCode::Faf
    }

    /// Whether every option's setter runs when an interface starts, as
    /// moho's: MohoEngine.dll `OPTIONS_Apply(bool)` (0x10487da0) calls
    /// `optionslogic.Apply`, which runs the setters of every section. On
    /// FAF's code only the sound and video sections run: FAF's other setters
    /// reach engine APIs we don't have yet (`share_mouse` imports
    /// `casting/mouse.lua`, which needs `AddOnSyncCallback` in the front end;
    /// `SetInvertMidMouseButton`). LOUD's main menu needs its
    /// `main_menu_animation_duration` setter to have run.
    pub fn runs_every_option_setter(self) -> bool {
        self != GameCode::Faf
    }

    /// Whether a mod named `Hotstats` is left out of the installed mods:
    /// FAF's `/lua/mods.lua` (`AllMods`) skips it, as FAF has the score
    /// screen built in. The base game's (`mohodata.scd`) and LOUD's only
    /// skip `enabled = false`, and LOUD ships Hotstats as a UI mod.
    pub fn skips_hotstats_mod(self) -> bool {
        self == GameCode::Faf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::vfs::MountPoint;

    fn vfs_with(files: &[&str]) -> Vfs {
        let dir = std::env::temp_dir().join(format!("game_code_{}_{}", std::process::id(), files.join("_").replace('/', "-")));
        for f in files {
            let p = dir.join(f.trim_start_matches('/'));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        vfs
    }

    #[test]
    fn detects_the_game_code() {
        assert_eq!(GameCode::detect(&vfs_with(&["/lua/version.lua", "/lua/simInit.lua"])), GameCode::Faf);
        assert_eq!(GameCode::detect(&vfs_with(&["/lua/LoudUtilities.lua", "/lua/simInit.lua"])), GameCode::Loud);
        assert_eq!(GameCode::detect(&vfs_with(&["/lua/simInit.lua"])), GameCode::Other);
        assert!(GameCode::Faf.engine_hooks() && !GameCode::Loud.engine_hooks());
        assert!(!GameCode::Faf.runs_every_option_setter() && GameCode::Other.runs_every_option_setter());
    }
}
