//! `Sound{Bank=, Cue=, LodCutoff=}` — moho sound parameter objects.
//!
//! Blueprints create thousands of these at load time, so the handle is a small
//! native userdata rather than a Lua table. `GetCueBank` reads it back.

use std::sync::Arc;

use mlua::prelude::*;

use crate::lua_api::lua_fn;
use crate::lua_api::shapes::Shaped;

/// The table `Sound` takes.
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "SoundSpec", write_only, rename_all = "PascalCase")]
#[allow(dead_code)]
struct SoundSpec {
    bank: Option<String>,
    cue: Option<String>,
    lod_cutoff: Option<String>,
}

/// Names are shared (`Arc<str>`): a unit's `PlaySound` hands them to the
/// presentation queue without copying strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundHandle {
    pub bank: Arc<str>,
    pub cue: Arc<str>,
    pub lod_cutoff: Option<Arc<str>>,
}

impl crate::lua_api::LuaType for SoundHandle {
    fn ty() -> crate::lua_api::LuaTy {
        crate::lua_api::LuaTy::Class("sound_methods")
    }
}

impl LuaUserData for SoundHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
        methods.add_meta_method(LuaMetaMethod::ToString, |_, s, ()| {
            Ok(format!("Sound({}/{})", s.bank, s.cue))
        });
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// A sound parameter object from `{ Bank, Cue, LodCutoff }`.
        Sound = |_, spec: Shaped<SoundSpec>| {
            Ok(SoundHandle {
                bank: spec.get::<Option<String>>("Bank")?.unwrap_or_default().into(),
                cue: spec.get::<Option<String>>("Cue")?.unwrap_or_default().into(),
                lod_cutoff: spec.get::<Option<String>>("LodCutoff")?.map(Into::into),
            })
        })?;
    lua_fn!(lua, global,
        /// `cue, bank = GetCueBank(sound)`.
        GetCueBank = |_, sound: LuaUserDataRef<SoundHandle>| {
            let s = sound;
            Ok((s.cue.to_string(), s.bank.to_string()))
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_builds_handle() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let h: LuaAnyUserData = lua
            .load("return Sound { Bank = 'Explosions', Cue = 'Nuke', LodCutoff = 'Weapon_LodCutoff' }")
            .eval()
            .unwrap();
        let h = h.borrow::<SoundHandle>().unwrap();
        assert_eq!(&*h.bank, "Explosions");
        assert_eq!(&*h.cue, "Nuke");
        assert_eq!(h.lod_cutoff.as_deref(), Some("Weapon_LodCutoff"));
        let (cue, bank): (String, String) = lua.load("return GetCueBank(Sound { Bank = 'Explosions', Cue = 'Nuke' })").eval().unwrap();
        assert_eq!((cue.as_str(), bank.as_str()), ("Nuke", "Explosions"));
    }
}
