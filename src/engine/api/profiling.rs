use mlua::prelude::*;

use crate::lua_api::lua_fn;

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// Sim seconds, not wall-clock: FAF only times code with it for log
        /// lines (`string.format("%.2f ms", ...)`), but a wall-clock value
        /// makes those strings differ between runs, which shifts Lua
        /// allocation and so GC timing, which in turn changes weak-table
        /// contents and table order. Profile the sim from Rust (`StageTimes`,
        /// tracing) instead.
        GetSystemTimeSecondsOnlyForProfileUse = |lua, ()| Ok(crate::engine::api::sim_time::current_tick(lua) as f64 / 10.0))?;
    lua_fn!(lua, global,
        /// Seconds since `origin` (FAF's binary patch, for timings that
        /// would lose precision as one big float). Sim seconds, for the
        /// reason `GetSystemTimeSecondsOnlyForProfileUse` gives.
        GetTimeForProfile = |lua, origin: Option<f64>| Ok(crate::engine::api::sim_time::current_tick(lua) as f64 / 10.0 - origin.unwrap_or(0.0)))
}
