//! The user layer's sound globals (FAF's `/engine/User.lua`): `PlaySound`,
//! `PlayVoice`, `StartSound`, `StopSound`, `SoundIsPrepared`, `PauseSound`,
//! `PauseVoice`, `SetVolume` / `GetVolume`, `StopAllSounds`,
//! `Enable/DisableWorldSounds`, `Get/SetMovieVolume`, and `WaitFor` on a
//! playing cue (FAF's `UserMusic.lua` waits for the old music to fade).
//!
//! They only send commands to the audio thread ([`Audio`]); cue states come
//! back through its status board. The user layer isn't the sim, so nothing
//! here can reach sim state.

use mlua::Either;
use mlua::prelude::*;

use crate::audio::{Audio, Command, CueStatus};
use crate::engine::api::sound::SoundHandle;
use crate::lua_api::{Status, lua_fn};
use crate::ui::shapes::Loose;

/// A playing (or prepared) cue, as scripts hold it. Collecting it lets the
/// cue play on untracked.
pub struct CueHandle {
    id: u64,
    audio: Audio,
}

impl Drop for CueHandle {
    fn drop(&mut self) {
        self.audio.forget(self.id);
    }
}

impl crate::lua_api::LuaType for CueHandle {
    fn ty() -> crate::lua_api::LuaTy {
        crate::opaque_lua_type!("CueHandle", "A playing or prepared sound cue (`PlaySound`, `PlayVoice`); pass it to `StartSound`, `StopSound`, `SoundIsPrepared` or `WaitFor`.")
    }
}

impl LuaUserData for CueHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
        methods.add_meta_method(LuaMetaMethod::ToString, |_, h, ()| Ok(format!("CueHandle({})", h.id)));
    }
}

fn audio(lua: &Lua) -> Audio {
    lua.app_data_ref::<Audio>().map(|a| a.clone()).unwrap_or_else(Audio::disabled)
}

/// Bank and cue of a `Sound{...}` (or a plain `{Bank=, Cue=}` table).
fn bank_cue(value: &LuaValue) -> LuaResult<Option<(String, String)>> {
    Ok(match value {
        LuaValue::UserData(ud) => match ud.borrow::<SoundHandle>() {
            Ok(s) => Some((s.bank.to_string(), s.cue.to_string())),
            Err(_) => None,
        },
        LuaValue::Table(t) => match (t.get::<Option<String>>("Bank")?, t.get::<Option<String>>("Cue")?) {
            (Some(b), Some(c)) => Some((b, c)),
            _ => None,
        },
        _ => None,
    })
}

fn handle_id(value: &LuaValue) -> Option<u64> {
    match value {
        LuaValue::UserData(ud) => ud.borrow::<CueHandle>().ok().map(|h| h.id),
        _ => None,
    }
}

fn play(lua: &Lua, sound: &LuaValue, start: bool, voice: bool, duck: bool) -> LuaResult<Loose<Option<CueHandle>>> {
    let Some((bank, cue)) = bank_cue(sound)? else { return Ok(Loose::new(LuaNil)) };
    if bank.is_empty() || cue.is_empty() {
        return Ok(Loose::new(LuaNil));
    }
    let audio = audio(lua);
    let id = audio.play(&bank, &cue, start, voice, duck);
    Ok(Loose::new(LuaValue::UserData(lua.create_userdata(CueHandle { id, audio })?)))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    let g = lua.globals();
    lua_fn!(lua, global,
        /// Plays `sound` (a `Sound{Bank, Cue}`) and returns its cue handle (nil
        /// for an empty bank or cue). With `prepareOnly` the cue loads but waits
        /// for `StartSound`.
        PlaySound = |lua, (sound, prepare_only): (Loose<Either<SoundHandle, LuaTable>>, Option<bool>)| play(lua, &sound, !prepare_only.unwrap_or(false), false, false))?;
    lua_fn!(lua, global,
        /// Like `PlaySound` for a voice: voices queue one after another. With
        /// `duck` other audio is turned down while it plays. Moho takes these
        /// two only (MohoEngine.dll 0x1046e9f0).
        PlayVoice = |lua, (sound, duck): (Loose<Either<SoundHandle, LuaTable>>, Option<bool>)| {
            play(lua, &sound, true, true, duck.unwrap_or(false))
        })?;
    lua_fn!(lua, global,
        /// Starts a prepared cue.
        StartSound = |lua, handle: Loose<Option<CueHandle>>| {
            if let Some(id) = handle_id(&handle) {
                audio(lua).send(Command::Start(id));
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Stops a cue, at once with `immediate`.
        StopSound = |lua, (handle, immediate): (Loose<Option<CueHandle>>, Option<bool>)| {
            if let Some(id) = handle_id(&handle) {
                audio(lua).send(Command::Stop { id, immediate: immediate.unwrap_or(false) });
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Whether the cue is loaded and ready to start (or already playing or
        /// finished). True for anything that isn't a cue handle.
        SoundIsPrepared = |lua, handle: Loose<Option<CueHandle>>| {
            Ok(match handle_id(&handle) {
                Some(id) => audio(lua).status(id) != Some(CueStatus::Pending),
                None => true,
            })
        })?;
    lua_fn!(lua, global,
        /// `WaitFor`'s check for a cue: still going (pending, prepared or
        /// playing).
        #[lua(internal)]
        __engine_cue_busy = |lua, handle: Loose<Option<CueHandle>>| {
            Ok(match handle_id(&handle) {
                Some(id) => matches!(audio(lua).status(id), Some(CueStatus::Pending | CueStatus::Prepared | CueStatus::Playing)),
                None => false,
            })
        })?;
    for name in ["PauseSound", "PauseVoice"] {
        crate::lua_api::register_fn(
            lua,
            &g,
            "",
            name,
            Some(&[Some("category"), Some("paused")]),
            "Pauses (or with `paused` false resumes) a category of audio; paused defaults to true.",
            Status::Implemented,
            false,
            |lua, (category, paused): (String, Option<bool>)| {
                audio(lua).send(Command::Pause { category, paused: paused.unwrap_or(true) });
                Ok(())
            },
        )?;
    }
    lua_fn!(lua, global,
        /// Sets a category's volume (clamped to 0..2).
        SetVolume = |lua, (category, volume): (String, f32)| {
            audio(lua).set_volume(&category, volume.clamp(0.0, 2.0));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// A category's volume.
        GetVolume = |lua, category: String| Ok(audio(lua).volume(&category)))?;
    lua_fn!(lua, global,
        /// The movies' own soundtracks' volume (the video player reads it back);
        /// clamped to 0..2.
        SetMovieVolume = |lua, volume: f32| {
            audio(lua).set_volume("Movie", volume.clamp(0.0, 2.0));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The movies' soundtrack volume.
        GetMovieVolume = |lua, ()| Ok(audio(lua).volume("Movie")))?;
    lua_fn!(lua, global,
        /// Stops every sound.
        StopAllSounds = |lua, ()| {
            audio(lua).send(Command::StopAll);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Turns on the sounds the world makes.
        EnableWorldSounds = |lua, ()| {
            audio(lua).send(Command::WorldEnabled(true));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Turns off the sounds the world makes.
        DisableWorldSounds = |lua, ()| {
            audio(lua).send(Command::WorldEnabled(false));
            Ok(())
        })?;
    // `AudioSetLanguage` also picks the voice banks (`/sounds/voice/<lang>`).
    let set_language: LuaFunction = g.get("AudioSetLanguage")?;
    lua_fn!(lua, global,
        /// Sets the audio language; also picks the voice banks
        /// (`/sounds/voice/<lang>`).
        AudioSetLanguage = move |lua, language: String| {
            audio(lua).send(Command::SetLanguage(language.clone()));
            set_language.call::<()>(language)
        })?;
    // `WaitFor` on a cue yields a frame at a time until it finishes; on
    // anything else it returns at once (the user layer has no manipulators).
    let wait_for: LuaFunction = lua
        .load("local busy, yield = ... return function(obj) while busy(obj) do yield(1) end end")
        .set_name("=[engine user WaitFor]")
        .call((g.get::<LuaFunction>("__engine_cue_busy")?, g.get::<LuaTable>("coroutine")?.get::<LuaFunction>("yield")?))?;
    g.set("WaitFor", wait_for)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(audio: Audio) -> Lua {
        let lua = Lua::new();
        lua.set_app_data(audio);
        crate::engine::api::sound::register(&lua).unwrap();
        crate::engine::api::audio::register(&lua).unwrap();
        register(&lua).unwrap();
        lua
    }

    #[test]
    fn without_audio_cues_are_done_at_once() {
        let lua = vm(Audio::disabled());
        let (prepared, busy, none): (bool, bool, LuaValue) = lua
            .load(
                "local h = PlaySound(Sound{Bank='Music', Cue='Main_Menu'}, true) \
                 StartSound(h) StopSound(h, true) \
                 return SoundIsPrepared(h), __engine_cue_busy(h), PlaySound(Sound{Bank='', Cue=''})",
            )
            .eval()
            .unwrap();
        assert!(prepared && !busy && none.is_nil());
        lua.load("SetVolume('Music', 0.5) SetMovieVolume(3)").exec().unwrap();
        let (music, movie, other): (f32, f32, f32) = lua.load("return GetVolume('Music'), GetMovieVolume(), GetVolume('VO')").eval().unwrap();
        assert_eq!((music, movie, other), (0.5, 2.0, 1.0));
    }

    /// The movie flow: a prepared soundtrack and voice report prepared, wait
    /// for `StartSound`, then play; dropping the handles stops tracking them.
    #[test]
    fn movie_cues_prepare_then_start() {
        let Some(dir) = crate::audio::xact::tests::sounds_dir() else { return };
        let audio = Audio::start(crate::audio::AudioOptions::default());
        audio.send(Command::SetSource(crate::audio::Source::Dir(dir)));
        let lua = vm(audio.clone());
        lua.load(
            "sound = PlaySound(Sound{Bank='FMV_BG', Cue='X_FMV_Intro'}, true) \
             voice = PlayVoice(Sound{Bank='XGG', Cue='XGG_Computer__04716'}, false, true)",
        )
        .exec()
        .unwrap();
        let ready = |lua: &Lua| lua.load("return SoundIsPrepared(sound) and SoundIsPrepared(voice)").eval::<bool>().unwrap();
        let t = std::time::Instant::now();
        while !ready(&lua) && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ready(&lua));
        assert!(lua.load("return __engine_cue_busy(sound)").eval::<bool>().unwrap(), "prepared, not started");
        lua.load("StartSound(sound) StartSound(voice)").exec().unwrap();
        let status = |lua: &Lua, name: &str| {
            let ud: LuaAnyUserData = lua.globals().get(name).unwrap();
            let id = ud.borrow::<CueHandle>().unwrap().id;
            audio.status(id)
        };
        let t = std::time::Instant::now();
        while status(&lua, "voice") != Some(CueStatus::Playing) && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(status(&lua, "voice"), Some(CueStatus::Playing));
        let tracked = audio.tracked();
        lua.load("StopSound(sound, true) StopSound(voice, true) sound = nil voice = nil").exec().unwrap();
        lua.gc_collect().unwrap();
        lua.gc_collect().unwrap();
        assert_eq!(audio.tracked(), tracked - 2);
    }
}
