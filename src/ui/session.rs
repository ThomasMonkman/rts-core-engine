//! The session as the interface sees it: the clock, the game speed, pause
//! and whether a game or the main menu is up.
//!
//! The engine keeps [`Session`] (app data of the user VM) current through
//! `UserVm` (`beat`, `set_game_speed`, `set_paused`, `start_front_end`) and
//! takes what the interface asks for from it (`take_speed_request`,
//! `take_pause_request`, `exit_requested`). Render-thread only.

use mlua::prelude::*;

use crate::lua_api::{LuaEnum, lua_fn};

/// Moho's interface states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, LuaEnum)]
#[lua(name = "UIState")]
pub enum UiState {
    /// The main menu.
    #[lua(rename = "frontend")]
    Frontend,
    #[default]
    #[lua(rename = "game")]
    Game,
}

/// What the interface knows of the session and asks of it.
#[derive(Default)]
pub struct Session {
    /// The sim tick of the last beat.
    pub tick: u64,
    /// The game speed as moho's step (-10..10, 0 normal; rate
    /// 10^(step/10)); `None` until the engine first says.
    pub game_speed: Option<i32>,
    /// The rate the sim actually runs at (1 = normal).
    pub sim_rate: f32,
    /// A step `SetGameSpeed` asked for, not yet taken.
    pub speed_request: Option<i32>,
    pub paused: bool,
    /// A pause (true) or resume `SessionRequestPause` / `SessionResume`
    /// asked for, not yet taken.
    pub pause_request: Option<bool>,
    pub ui_state: UiState,
    /// `ExitApplication` was called.
    pub exit_request: bool,
}

/// The VM's session state (made on first use, so a bare VM in a test has one).
pub fn state(lua: &Lua) -> mlua::AppDataRefMut<'_, Session> {
    if lua.app_data_ref::<Session>().is_none() {
        lua.set_app_data(Session::default());
    }
    lua.app_data_mut::<Session>().expect("just set")
}

/// Moho's `%02d:%02d:%02d` of whole seconds (hours aren't wrapped).
fn format_time(seconds: f64) -> String {
    let s = seconds.floor() as i64;
    format!("{:02}:{:02}:{:02}", s.div_euclid(3600), s.div_euclid(60).rem_euclid(60), s.rem_euclid(60))
}

/// Seconds of game time: moho's sim runs 10 ticks a second.
fn seconds(lua: &Lua) -> f64 {
    state(lua).tick as f64 / 10.0
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Session::default());
    lua_fn!(lua, global,
        /// The sim tick the interface has reached (the last beat's).
        GameTick = |lua, ()| Ok(state(lua).tick as f64))?;
    lua_fn!(lua, global,
        /// Game time in seconds.
        GameTime = |lua, ()| Ok(seconds(lua)))?;
    lua_fn!(lua, global,
        /// Game time in seconds.
        GetGameTimeSeconds = |lua, ()| Ok(seconds(lua)))?;
    lua_fn!(lua, global,
        /// Game time as `hh:mm:ss`.
        GetGameTime = |lua, ()| Ok(format_time(seconds(lua))))?;
    lua_fn!(lua, global,
        /// Whole seconds as `hh:mm:ss` (nil: 0).
        FormatTime = |_, seconds: Option<f64>| Ok(format_time(seconds.unwrap_or(0.0))))?;
    lua_fn!(lua, global,
        /// Game speed as moho's step (-10..10, 0 normal; rate 10^(step/10)).
        GetGameSpeed = |lua, ()| Ok(f64::from(state(lua).game_speed.unwrap_or(0))))?;
    lua_fn!(lua, global,
        /// Asks for a game speed step (rounded down, kept to -10..10; nil: 0).
        /// The engine takes it at the end of the frame.
        SetGameSpeed = |lua, speed: Option<f64>| {
            state(lua).speed_request = Some(speed.unwrap_or(0.0).floor().clamp(-10.0, 10.0) as i32);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The rate the sim actually runs at (1 = normal).
        GetSimRate = |lua, ()| Ok(f64::from(state(lua).sim_rate)))?;
    lua_fn!(lua, global,
        /// A pause or resume asked for and not yet taken shows at once, as
        /// the real game's does in the same frame (dev/probes
        /// probe_ui_globals, probe_re2_ui).
        SessionIsPaused = |lua, ()| {
            let s = state(lua);
            Ok(s.pause_request.unwrap_or(s.paused))
        })?;
    lua_fn!(lua, global,
        SessionRequestPause = |lua, ()| {
            state(lua).pause_request = Some(true);
            Ok(())
        })?;
    lua_fn!(lua, global,
        SessionResume = |lua, ()| {
            state(lua).pause_request = Some(false);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// A session's world is loaded and running: moho's world state is
        /// "playing" (6 of 0..8; the binding in SupremeCommander.exe compares
        /// it), which pausing, the game ending and `SessionEndGame` leave
        /// alone. False at the main menu, where there is no world.
        WorldIsPlaying = |lua, ()| Ok(state(lua).ui_state == UiState::Game))?;
    lua_fn!(lua, global,
        /// 'frontend' (the main menu) or 'game'.
        GetCurrentUIState = |lua, ()| Ok(state(lua).ui_state))?;
    lua_fn!(lua, global,
        /// The renderer closes the window (`UserVm::exit_requested`).
        ExitApplication = |lua, ()| {
            state(lua).exit_request = true;
            Ok(())
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn time_is_whole_seconds_with_unwrapped_hours() {
        assert_eq!(format_time(0.0), "00:00:00");
        assert_eq!(format_time(3725.9), "01:02:05");
        assert_eq!(format_time(360_000.0), "100:00:00");
    }
}
