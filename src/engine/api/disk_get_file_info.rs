use std::sync::{Arc, RwLock};

use mlua::prelude::*;

use crate::engine::vfs::Vfs;
use crate::lua_api::lua_fn;
use crate::lua_api::shapes::Shaped;

/// The table `DiskGetFileInfo` returns for an existing file.
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "FileInfo", write_only, rename_all = "PascalCase")]
#[allow(dead_code)]
struct FileInfo {
    /// True for a loose folder on disk (a folder inside an archive isn't
    /// found).
    is_folder: bool,
    /// True inside an archive.
    read_only: bool,
    size_bytes: u64,
    /// The write time as 16 hex digits. Fixed (the epoch) in ours: a real
    /// time differs per machine and both VMs see this table.
    time_stamp: String,
    /// The write time, broken down (`year`, `month`, `mday`, `wday`,
    /// `hour`, `minute`, `second`). Fixed, as `TimeStamp`.
    write_time: LuaTable,
}

/// What moho's `DiskGetFileInfo` / `exists` find: absolute, forward-slash
/// paths only (a relative or a backslash path is "not there"), and no folder
/// inside an archive (`/lua/sim`, `/lua/ui/game` and `/lua/sim/` are not
/// there in either VM: dev/probes probe_api_spot `core.DiskGetFileInfo4` /
/// `5`, probe_ui_globals `exists folder`). A loose folder on disk is found
/// (inferred, not probed): FAF's lobby lists an AI mod's AIs only if
/// `exists(mod.location .. '/lua/AI/CustomAIs_v2')` is true for the mod's
/// mounted folder (`ui/lobby/aitypes.lua`, `system/Localization.lua`), and
/// the `IsFolder` field would otherwise always be false; probe_re3_ui
/// confirmed it with and without the trailing slash. A mount root named
/// without its trailing slash (`/maps`, `/schook`) is not found, while
/// `/maps/` is (probe_re3_ui): moho matches a mount by its `/maps/` prefix.
pub(crate) fn lookup(vfs: &Vfs, path: &str) -> Option<crate::engine::vfs::FileInfo> {
    if !path.starts_with('/') || path.contains('\\') {
        return None;
    }
    vfs.file_info(path).filter(|i| !(i.is_folder && i.read_only)).filter(|i| !(i.is_folder && !path.ends_with('/') && vfs.is_mount_point(path)))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// `{ IsFolder, ReadOnly, SizeBytes, TimeStamp, WriteTime }` of a VFS
        /// file or loose folder, or false if it doesn't exist (see
        /// [`lookup`]).
        DiskGetFileInfo = |lua, path: String| {
            let vfs = lua
                .app_data_ref::<Arc<RwLock<Vfs>>>()
                .expect("VFS not in app_data")
                .clone();
            let info = lookup(&vfs.read().expect("VFS poisoned"), &path);
            let Some(info) = info else {
                return Ok(mlua::Either::Left(false));
            };
            // The times are fixed: real ones differ per machine and would
            // break lockstep determinism if the sim read them.
            let at = crate::engine::replay::utc_time(0);
            let write = lua.create_table_with_capacity(0, 7)?;
            write.raw_set("year", at.year)?;
            write.raw_set("month", at.month)?;
            write.raw_set("mday", at.day)?;
            write.raw_set("wday", at.weekday)?;
            write.raw_set("hour", at.hour)?;
            write.raw_set("minute", at.minute)?;
            write.raw_set("second", at.second)?;
            let t = lua.create_table_with_capacity(0, 5)?;
            t.raw_set("IsFolder", info.is_folder)?;
            t.raw_set("ReadOnly", info.read_only)?;
            t.raw_set("SizeBytes", info.size)?;
            t.raw_set("TimeStamp", format!("{:016x}", 0))?;
            t.raw_set("WriteTime", write)?;
            Ok(mlua::Either::Right(Shaped::<FileInfo>::new(t)))
        })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::lookup;
    use crate::engine::vfs::{MountPoint, Vfs};

    #[test]
    fn folders_inside_archives_and_odd_paths_are_not_found() {
        let dir = std::env::temp_dir().join(format!("disk_info_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("loose/mods/m/lua/ai/customais_v2")).unwrap();
        std::fs::write(dir.join("loose/mods/m/lua/ai/customais_v2/m.lua"), b"").unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("lua/sim/unit.lua", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"x").unwrap();
        std::fs::write(dir.join("lua.scd"), zip.finish().unwrap().into_inner()).unwrap();
        let vfs = Vfs::new(
            vec![
                MountPoint { source: dir.join("lua.scd"), mountpoint: "/".into() },
                MountPoint { source: dir.join("loose"), mountpoint: "/".into() },
                MountPoint { source: dir.join("loose/mods"), mountpoint: "/schook".into() },
            ],
            vec![],
        )
        .unwrap();
        let found = |p: &str| lookup(&vfs, p).map(|i| (i.is_folder, i.read_only));
        assert_eq!(found("/lua/sim/unit.lua"), Some((false, true)));
        assert_eq!(found("/LUA/SIM/UNIT.LUA"), Some((false, true)));
        assert_eq!(found("/lua/sim"), None, "a folder inside an archive");
        assert_eq!(found("/lua/sim/"), None);
        assert_eq!(found("lua/sim/unit.lua"), None, "relative");
        assert_eq!(found("\\lua\\sim\\unit.lua"), None, "backslashes");
        assert_eq!(found("/mods/m/lua/AI/CustomAIs_v2"), Some((true, false)), "a loose folder");
        assert_eq!(found("/mods/m/lua/AI/CustomAIs_v2/"), Some((true, false)), "with the slash");
        assert_eq!(found("/schook"), None, "a mount root without its slash (probe_re3_ui)");
        assert_eq!(found("/SCHOOK"), None);
        assert_eq!(found("/schook/"), Some((true, false)), "with it");
        assert_eq!(found("/schook/m"), Some((true, false)), "a folder under the mount root");
        std::fs::remove_dir_all(&dir).ok();
    }
}
