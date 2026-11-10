//! Particle effects (`moho.IEffect`): `CreateEmitter*` and friends.
//!
//! Presentation only: effects live in [`Presentation`] for the renderer and
//! never feed back into the sim. An effect ends when Lua destroys it, when
//! its lifetime runs out (the blueprint's `Lifetime` ticks, or a `LIFETIME`
//! parameter), or with the entity it is attached to (or a beam's far end).

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::entity::{entity_id, world};
use crate::engine::api::manipulator::resolve_bone;
use crate::engine::api::{moho, script_object};
use crate::engine::sim::presentation::{Effect, EffectKind, Presentation};
use crate::lua_api::{Obj, lua_fn, lua_method};

/// `_c_object` of an effect script object.
#[derive(Clone, Copy, Debug)]
pub struct EffectHandle(pub u32);
impl LuaUserData for EffectHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

/// Army argument as moho takes it: an index, negative = none.
fn army_of(v: &LuaValue) -> Option<u32> {
    match v {
        LuaValue::Integer(i) if *i >= 0 => Some(*i as u32),
        LuaValue::Number(n) if *n >= 0.0 => Some(*n as u32),
        _ => None,
    }
}

/// Creates an effect at `object`'s `bone` (`None` = origin), attached to it
/// if `attach`, and returns its `moho.IEffect` object.
fn spawn_effect(
    lua: &Lua,
    kind: EffectKind,
    object: &LuaTable,
    bone: Option<&LuaValue>,
    attach: bool,
    army: &LuaValue,
    blueprint: String,
) -> LuaResult<Obj<moho::IEffect>> {
    let id = entity_id(object)?;
    let bone = match bone {
        Some(b) if !b.is_nil() => resolve_bone(lua, id, b)?,
        _ => None,
    };
    let position = crate::engine::api::beam::bone_position(lua, &world(lua), id, bone).unwrap_or_default().to_f64();
    let lifetime = blueprint_lifetime(lua, kind, &blueprint);
    let effect = Effect {
        kind,
        blueprint,
        army: army_of(army),
        position: [position.0, position.1, position.2],
        attached: attach.then_some((id, bone)),
        source: Some((id, bone)),
        scale: 1.0,
        offset: [0.0; 3],
        params: Default::default(),
        curves: Default::default(),
        curve_lengths: Default::default(),
        ramp: None,
        asset: u32::MAX,
        ramp_asset: u32::MAX,
        beam_to: None,
        created: crate::engine::api::sim_time::snapshot_tick(lua),
        lifetime,
    };
    let handle = lua.app_data_mut::<Presentation>().expect("Presentation").add_effect(effect);
    effect_object(lua, handle)
}

/// The engine's strong references to live effects' Lua objects, by id.
/// Moho keeps its effects' script objects alive; without this, FAF's
/// weak-valued TrashBags (an engineer's `BuildEffectsBag`) lose entries
/// whenever the garbage collector runs, so `OnStopBuild` never destroys
/// those effects: build beams stay, stretched after their engineer.
fn objects(lua: &Lua) -> LuaResult<LuaTable> {
    crate::engine::api::script_object::registry_table(lua, "sim.effect_objects")
}

/// A new effect's `moho.IEffect` object, kept alive while the effect is.
fn effect_object(lua: &Lua, handle: u32) -> LuaResult<Obj<moho::IEffect>> {
    let obj = script_object::create(lua, EffectHandle(handle), moho::class_table(lua, "IEffect")?)?;
    objects(lua)?.raw_set(handle, obj.clone())?;
    Ok(Obj::new(obj))
}

/// Effect blueprint lifetimes by path, read once each.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Lifetimes(std::collections::BTreeMap<String, Option<f64>>);

/// The `Lifetime` of an emitter / trail blueprint (lights get theirs from
/// their arguments). Beams last until destroyed or their ends go: FAF keeps
/// build beams (`Lifetime = 2`) in a bag for the whole build, and projectile
/// and ambient building beams are never destroyed by script.
fn blueprint_lifetime(lua: &Lua, kind: EffectKind, blueprint: &str) -> Option<f64> {
    if matches!(kind, EffectKind::Light { .. } | EffectKind::Beam) || !blueprint.len().checked_sub(3).is_some_and(|i| blueprint.get(i..).is_some_and(|e| e.eq_ignore_ascii_case(".bp"))) {
        return None;
    }
    // Cached under the path as given (no allocation on the hot path).
    if let Some(l) = lua.app_data_ref::<Lifetimes>().and_then(|c| c.0.get(blueprint).copied()) {
        return l;
    }
    let key = blueprint.to_lowercase();
    let src = lua
        .app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>()
        .and_then(|v| v.read().ok().and_then(|v| v.read(&key).ok()));
    let lifetime = src.and_then(|b| crate::engine::assets::effect_bp::lifetime_ticks(&String::from_utf8_lossy(&b)));
    if lua.app_data_ref::<Lifetimes>().is_none() {
        lua.set_app_data(Lifetimes::default());
    }
    if let Some(mut c) = lua.app_data_mut::<Lifetimes>() {
        c.0.insert(blueprint.to_owned(), lifetime);
    }
    lifetime
}

/// End of tick: drops effects whose lifetime ran out and those attached to
/// (or beaming to) an entity that is gone (id order). Runs before the
/// destroyed entities are compacted away, so a trail whose entity died
/// this tick is kept for the snapshot with the entity's final transform
/// (`Presentation::ended_trails`): its last segment reaches the impact.
pub fn expire(lua: &Lua) {
    // The clock effects were created on (the current tick number).
    let tick = crate::engine::api::sim_time::snapshot_tick(lua);
    let destroyed = match lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>() {
        Some(mut w) => w.take_destroyed_log(),
        None => return,
    };
    let Some(world) = lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>() else { return };
    let Some(mut p) = lua.app_data_mut::<Presentation>() else { return };
    let w: &crate::engine::sim::entity::EntityWorld = &world;
    p.ended_trails.clear();
    // Lifetime 0 disables an emitter; positive ones run that many ticks.
    // Which end comes from the index (no scan of every effect), in id order.
    let ended = p.ending_effects(tick, &destroyed, |id| w.get(id).is_some_and(|e| !e.destroyed));
    for id in &ended {
        let Some(e) = p.remove_effect(*id) else { continue };
        if !matches!(e.kind, crate::engine::sim::presentation::EffectKind::Trail) {
            continue;
        }
        let Some((entity, bone)) = e.attached else { continue };
        if let Some((position, rotation)) = crate::engine::snapshot::last_anchor(w, entity, bone) {
            p.ended_trails.push(crate::engine::sim::presentation::EndedTrail { id: *id, effect: e, position, rotation });
        }
    }
    drop((p, world));
    // Their script objects may go now.
    if !ended.is_empty()
        && let Ok(t) = objects(lua) {
        for id in ended {
            let _ = t.raw_set(id, LuaNil);
        }
    }
}

/// Runs `f` on the effect behind `this` (no-op once destroyed) and returns
/// `this`, so calls chain as in moho.
fn with_effect(lua: &Lua, this: Obj<moho::IEffect>, f: impl FnOnce(&mut Effect)) -> LuaResult<Obj<moho::IEffect>> {
    let id = script_object::handle::<EffectHandle>(&this)?.0;
    lua.app_data_mut::<Presentation>().expect("Presentation").effect_mut(id, f);
    Ok(this)
}

/// `CreateLightParticle*(object, bone, army, size, lifetime, texture, ramp)`.
fn spawn_light(lua: &Lua, intel: bool, args: (Obj<moho::Entity>, shapes::Bone, shapes::Army, f64, f64, String, String)) -> LuaResult<Obj<moho::IEffect>> {
    let (object, bone, army, size, lifetime, texture, ramp) = args;
    let obj = spawn_effect(lua, EffectKind::Light { intel }, &object, Some(&bone), false, &army, texture)?;
    let ramp_asset = lua.app_data_mut::<Presentation>().expect("Presentation").intern_asset(&ramp);
    with_effect(lua, obj, |e| {
        e.params.insert("SIZE".into(), size);
        e.params.insert("LIFETIME".into(), lifetime);
        e.ramp = Some(ramp);
        e.ramp_asset = ramp_asset;
    })
}

/// A beam from `(object, bone)` to `(other, other_bone)`.
fn spawn_beam(lua: &Lua, object: &LuaTable, bone: &LuaValue, other: &LuaTable, other_bone: &LuaValue, army: &LuaValue, blueprint: String) -> LuaResult<Obj<moho::IEffect>> {
    let obj = spawn_effect(lua, EffectKind::Beam, object, Some(bone), true, army, blueprint)?;
    let to = entity_id(other)?;
    let to_bone = match other_bone {
        LuaValue::Nil => None,
        b => resolve_bone(lua, to, b)?,
    };
    with_effect(lua, obj, |e| e.beam_to = Some((to, to_bone)))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global, CreateBeamEntityToEntity = |lua, (a, ab, b, bb, army, bp): (Obj<moho::Entity>, shapes::Bone, Obj<moho::Entity>, shapes::Bone, shapes::Army, String)| spawn_beam(lua, &a, &ab, &b, &bb, &army, bp))?;
    lua_fn!(lua, global,
        /// Textured beam (no blueprint) of `thickness` from `bone` to `other_bone`;
        /// the texture is stored as the blueprint. Moho's binding takes exactly
        /// these 7 (MohoEngine.dll 0x10256030; FAF's scripts don't call it).
        CreateBeamToEntityBone = |lua, (a, ab, b, bb, army, thickness, texture): (Obj<moho::Entity>, shapes::Bone, Obj<moho::Entity>, shapes::Bone, shapes::Army, f64, String)| {
            let obj = spawn_beam(lua, &a, &ab, &b, &bb, &army, texture)?;
            with_effect(lua, obj, |e| {
                e.params.insert("THICKNESS".into(), thickness);
            })
        })?;
    lua_fn!(lua, global,
        /// Textured beam (no blueprint); the texture is stored as the blueprint.
        AttachBeamEntityToEntity = |lua, (a, ab, b, bb, army, texture): (Obj<moho::Entity>, shapes::Bone, Obj<moho::Entity>, shapes::Bone, shapes::Army, String)| spawn_beam(lua, &a, &ab, &b, &bb, &army, texture))?;
    lua_fn!(lua, global, CreateAttachedBeam = |lua, (object, bone, army, length, thickness, bp): (Obj<moho::Entity>, shapes::Bone, shapes::Army, f64, f64, String)| {
            let obj = spawn_effect(lua, EffectKind::Beam, &object, Some(&bone), true, &army, bp)?;
            with_effect(lua, obj, |e| {
                e.params.insert("LENGTH".into(), length);
                e.params.insert("THICKNESS".into(), thickness);
            })
        })?;
    lua_fn!(lua, global, CreateBeamEmitterOnEntity = |lua, (object, bone, army, bp): (Obj<moho::Entity>, shapes::Bone, shapes::Army, String)| {
            spawn_effect(lua, EffectKind::Beam, &object, Some(&bone), true, &army, bp)
        })?;
    lua_fn!(lua, global,
        /// A free beam; `AttachBeamToEntity` anchors it later.
        CreateBeamEmitter = |lua, (bp, army): (String, shapes::Army)| {
            let lifetime = blueprint_lifetime(lua, EffectKind::Beam, &bp);
            let effect = Effect {
                kind: EffectKind::Beam,
                blueprint: bp,
                army: army_of(&army),
                position: [0.0; 3],
                attached: None,
                source: None,
                scale: 1.0,
                offset: [0.0; 3],
                params: Default::default(),
                curves: Default::default(),
                curve_lengths: Default::default(),
                ramp: None,
                asset: u32::MAX,
                ramp_asset: u32::MAX,
                beam_to: None,
                created: crate::engine::api::sim_time::snapshot_tick(lua),
                lifetime,
            };
            let handle = lua.app_data_mut::<Presentation>().expect("Presentation").add_effect(effect);
            effect_object(lua, handle)
        })?;
    lua_fn!(lua, global, AttachBeamToEntity = |lua, (beam, object, bone, _army): (Obj<moho::IEffect>, Obj<moho::Entity>, shapes::Bone, shapes::Army)| {
            let id = entity_id(&object)?;
            let b = match &*bone {
                LuaValue::Nil => None,
                b => resolve_bone(lua, id, b)?,
            };
            with_effect(lua, beam, |e| {
                e.attached = Some((id, b));
                e.source = Some((id, b));
            })
        })?;
    lua_fn!(lua, global,
        /// TODO: grant the army vision for the light's lifetime once the intel
        /// system exists (sim-visible, so it must be fixed-point then).
        CreateLightParticle = |lua, args: (Obj<moho::Entity>, shapes::Bone, shapes::Army, f64, f64, String, String)| spawn_light(lua, false, args))?;
    lua_fn!(lua, global, CreateLightParticleIntel = |lua, args: (Obj<moho::Entity>, shapes::Bone, shapes::Army, f64, f64, String, String)| spawn_light(lua, true, args))?;
    lua_fn!(lua, global, CreateEmitterAtBone = |lua, (object, bone, army, bp): (Obj<moho::Entity>, shapes::Bone, shapes::Army, String)| {
            spawn_effect(lua, EffectKind::Emitter, &object, Some(&bone), false, &army, bp)
        })?;
    lua_fn!(lua, global, CreateTrail = |lua, (object, bone, army, bp): (Obj<moho::Entity>, shapes::Bone, shapes::Army, String)| {
            spawn_effect(lua, EffectKind::Trail, &object, Some(&bone), true, &army, bp)
        })?;
    lua_fn!(lua, global, CreateAttachedEmitter = |lua, (object, bone, army, bp): (Obj<moho::Entity>, shapes::Bone, shapes::Army, String)| {
            spawn_effect(lua, EffectKind::Emitter, &object, Some(&bone), true, &army, bp)
        })?;
    lua_fn!(lua, global, CreateEmitterAtEntity = |lua, (object, army, bp): (Obj<moho::Entity>, shapes::Army, String)| {
            spawn_effect(lua, EffectKind::Emitter, &object, None, false, &army, bp)
        })?;
    lua_fn!(lua, global, CreateEmitterOnEntity = |lua, (object, army, bp): (Obj<moho::Entity>, shapes::Army, String)| {
            spawn_effect(lua, EffectKind::Emitter, &object, None, true, &army, bp)
        })?;
    lua_method!(lua, moho::IEffect, SetEmitterParam = |lua, (this, param, value): (Obj<moho::IEffect>, String, f64)| {
        with_effect(lua, this, |e| {
            e.params.insert(param.to_uppercase(), value);
        })
    })?;
    lua_method!(lua, moho::IEffect, ScaleEmitter = |lua, (this, scale): (Obj<moho::IEffect>, f64)| {
        with_effect(lua, this, |e| e.scale = scale)
    })?;
    lua_method!(lua, moho::IEffect, OffsetEmitter = |lua, (this, x, y, z): (Obj<moho::IEffect>, f64, f64, f64)| {
        with_effect(lua, this, |e| e.offset = [x, y, z])
    })?;
    lua_method!(lua, moho::IEffect, SetEmitterCurveParam = |lua, (this, param, height, size): (Obj<moho::IEffect>, String, f64, f64)| {
        with_effect(lua, this, |e| {
            e.curves.insert(param.to_uppercase(), (height, size));
        })
    })?;
    lua_method!(lua, moho::IEffect, ResizeEmitterCurve = |lua, (this, param, ticks): (Obj<moho::IEffect>, String, f64)| {
        with_effect(lua, this, |e| {
            e.curve_lengths.insert(param.to_uppercase(), ticks);
        })
    })?;
    lua_method!(lua, moho::IEffect,
        /// Beam parameters share the effect's parameter table.
        SetBeamParam = |lua, (this, name, value): (Obj<moho::IEffect>, String, f64)| {
        with_effect(lua, this, |e| {
            e.params.insert(name.to_uppercase(), value);
        })
    })?;
    lua_method!(lua, moho::IEffect, Destroy = |lua, this: Obj<moho::IEffect>| {
        let id = script_object::handle::<EffectHandle>(&this)?.0;
        lua.app_data_mut::<Presentation>().expect("Presentation").remove_effect(id);
        objects(lua)?.raw_set(id, LuaNil)?;
        Ok(())
    })?;
    Ok(())
}
