use std::sync::{Arc, RwLock};

use mlua::prelude::*;

use crate::engine::vfs::Vfs;
use crate::lua_api::lua_fn;

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// True if a file or loose folder exists in the VFS; false for a
        /// folder inside an archive, as moho (see
        /// [`super::disk_get_file_info::lookup`]).
        exists = |lua, name: String| {
            let vfs = lua
                .app_data_ref::<Arc<RwLock<Vfs>>>()
                .expect("VFS not in app_data")
                .clone();
            let found = super::disk_get_file_info::lookup(&vfs.read().expect("VFS poisoned"), &name).is_some();
            Ok(found)
        })
}
