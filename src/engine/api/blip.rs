//! Recon blips (`Unit:GetBlip(army)`, `moho.blip_methods`), answered from
//! the per-army intel grids (`sim::vision`).
//!
//! An army has a blip of a unit while it detects it (seen, or on radar /
//! sonar / omni), and keeps one for a structure it has ever seen (structures
//! don't move, so the memory stays true). Its own and allied units are always
//! known. The blip object carries the source's entity handle, so the
//! inherited entity methods (`GetPosition`, `GetBlueprint`, ...) read the
//! source; the `Is*` methods take the asking army. Fake blips (jammers) come
//! with counter-intel.
//!
//! An army's blip of a unit is one object, made from FAF's `Blip` class
//! (`/lua/sim/Blip.lua`: moho's `ReconBlip` constructor, MohoEngine.dll
//! 0x101ac100, makes its Lua object from it). Moho keeps a blip from the
//! army's first intel on the unit while the unit lives (losing and regaining
//! intel gives the same object back); `ReconTick` (0x101ae220) deletes
//! blips only once their unit is gone, and destroying one runs `OnDestroy`
//! (`DestroyIfUnused` 0x101acf30 -> `Entity::Destroy`), which runs the hooks
//! FAF's campaign objectives add (`AddDestroyHook`). When the unit dies
//! ([`AtDeath`]): a blip the army has intel on is destroyed; a structure's
//! out of sight is "maybe dead" until the army sees the spot; a mobile
//! unit's out of sight stays, with no hook. `FlushIntelInRect` hides blips
//! without destroying them. All from FAF probe `dev/probes/maps/probe_blip`.
//!
//! Here a blip is cached weakly until a script stores a field on it (the
//! class's `__newindex`), then kept until its unit dies, when it goes as
//! above. A blip no script has written to looks the same as a new one,
//! so letting the next (fixed-tick, `sim::gc`) collection take it changes
//! nothing scripts can see, and a big battle's ~40k `OnIntelChange` calls a
//! tick reuse their blips rather than building one per call (9.7 MB of
//! garbage a tick) without keeping one per army and unit alive.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::army::resolve_army;
use crate::engine::api::entity::{entity_id, world, EntityHandle};
use crate::engine::api::callbacks::UnitCallbacks;
use crate::engine::api::{moho, script_object};
use crate::lua_api::{lua_fn, lua_method, MohoClass, Obj, Status};
use crate::engine::sim::army::{Alliance, Armies};
use crate::engine::sim::entity::EntityId;
use crate::engine::sim::vision::{Detection, IntelGrids};

/// Registry table: `(army << 32) | unit` -> that army's blip of the unit,
/// weak values.
const BLIPS_KEY: &str = "engine.blips";

/// Registry table: unit -> {army -> blip} of the blips scripts have stored
/// a field on, kept until the unit dies.
const KEPT_KEY: &str = "engine.blips_kept";

/// Registry table: kept blips to destroy at the next intel pass, in order.
const DYING_KEY: &str = "engine.blips_dying";

/// Registry table: kept blips of structures that died out of their army's
/// sight, flat (blip, army, x bits, z bits, ...), in order: destroyed once
/// the army sees the spot.
const ORPHANS_KEY: &str = "engine.blips_orphans";

/// Registry value: the blips' metatable ([`blip_class`]).
const CLASS_KEY: &str = "engine.blip_class";

fn blips(lua: &Lua) -> LuaResult<LuaTable> {
    script_object::registry_table(lua, BLIPS_KEY)
}

fn cache_key(army: u32, unit: EntityId) -> i64 {
    (i64::from(army) << 32) | i64::from(unit)
}

/// [`blip_class`], fetched once per VM.
struct BlipClass(LuaTable);

/// The blips' metatable: FAF's `Blip` class, else `moho.blip_methods` (no
/// FAF scripts: tests, benches), with a `__newindex` that keeps a blip once
/// a script stores a field on it (fields the engine sets are raw).
fn blip_class(lua: &Lua) -> LuaResult<LuaTable> {
    if let Some(c) = lua.app_data_ref::<BlipClass>() {
        return Ok(c.0.clone());
    }
    let class = match lua.named_registry_value::<Option<LuaTable>>(CLASS_KEY)? {
        Some(class) => class,
        None => {
            let faf = lua
                .globals()
                .get::<Option<LuaFunction>>("import")?
                .and_then(|import| import.call::<LuaTable>("/lua/sim/Blip.lua").ok())
                .and_then(|module| module.get::<Option<LuaTable>>("Blip").ok().flatten());
            let class = match faf {
                Some(class) => class,
                None => moho::class_table(lua, "blip_methods")?,
            };
            let keep: LuaFunction = lua
                .load(
                    "local kept = ...
                     return function(blip, key, value)
                         local unit = rawget(blip, '_unit')
                         local held = kept[unit]
                         if not held then held = {} kept[unit] = held end
                         held[rawget(blip, '_army')] = blip
                         rawset(blip, key, value)
                     end",
                )
                .set_name("=engine blip __newindex")
                .call(script_object::registry_table(lua, KEPT_KEY)?)?;
            class.raw_set("__newindex", keep)?;
            lua.set_named_registry_value(CLASS_KEY, &class)?;
            class
        }
    };
    lua.set_app_data(BlipClass(class.clone()));
    Ok(class)
}

/// `army`'s blip of `unit`: the one scripts still hold, else a new one.
fn blip_object(lua: &Lua, cache: &LuaTable, class: &LuaTable, army: u32, unit: EntityId) -> LuaResult<LuaTable> {
    let key = cache_key(army, unit);
    if let Some(blip) = cache.raw_get::<Option<LuaTable>>(key)? {
        return Ok(blip);
    }
    let blip = script_object::create(lua, EntityHandle(unit), class.clone())?;
    blip.raw_set("_army", army)?;
    blip.raw_set("_unit", unit)?;
    cache.raw_set(key, &blip)?;
    Ok(blip)
}

/// What becomes of an army's blip of a unit as the unit is destroyed
/// (moho's `ReconTick`, 0x101ae220, and FAF probe `probe_blip`).
enum AtDeath {
    /// The army has intel on it (or it's friendly): destroyed at the next
    /// intel pass.
    Destroyed,
    /// A structure out of sight: "maybe dead" until the army sees the spot,
    /// then destroyed.
    MaybeDead,
    /// A mobile unit out of sight: the blip stays as it is (no destroy hook).
    Stays,
}

/// `unit` was destroyed (`Entity:Destroy`): each army's blip of it, if one
/// exists, goes as [`AtDeath`] says. Only blips scripts stored a field on
/// are destroyed (`OnDestroy`); a blip without fields has no hooks to run.
pub fn unit_destroyed(lua: &Lua, unit: EntityId) -> LuaResult<()> {
    let kept = script_object::registry_table(lua, KEPT_KEY)?;
    let held = kept.raw_get::<Option<LuaTable>>(unit)?;
    if held.is_some() {
        kept.raw_set(unit, LuaNil)?;
    }
    let structure = is_structure(lua, unit);
    let (fates, at) = {
        let w = world(lua);
        let Some(e) = w.get(unit) else { return Ok(()) };
        let (Some(armies), grids) = (lua.app_data_ref::<Armies>(), lua.app_data_ref::<IntelGrids>()) else { return Ok(()) };
        let fates: Vec<(u32, AtDeath)> = armies
            .list
            .iter()
            .map(|a| {
                let friendly = e.army.is_some_and(|owner| owner == a.index || armies.alliance(a.index, owner) == Alliance::Ally);
                let known = friendly || grids.as_ref().is_none_or(|g| g.detect(a.index, e).any());
                (a.index, if known { AtDeath::Destroyed } else if structure { AtDeath::MaybeDead } else { AtDeath::Stays })
            })
            .collect();
        (fates, e.position())
    };
    let cache = blips(lua)?;
    for (army, fate) in fates {
        let kept_blip = match &held {
            Some(h) => h.raw_get::<Option<LuaTable>>(army)?,
            None => None,
        };
        let Some(blip) = (match &kept_blip {
            Some(b) => Some(b.clone()),
            None => cache.raw_get::<Option<LuaTable>>(cache_key(army, unit))?,
        }) else {
            continue;
        };
        match fate {
            AtDeath::Destroyed if kept_blip.is_some() => {
                let dying = script_object::registry_table(lua, DYING_KEY)?;
                dying.raw_set(dying.raw_len() + 1, blip)?;
            }
            AtDeath::MaybeDead => {
                blip.raw_set("_maybe_dead", true)?;
                if kept_blip.is_some() {
                    let orphans = script_object::registry_table(lua, ORPHANS_KEY)?;
                    let n = orphans.raw_len();
                    orphans.raw_set(n + 1, blip)?;
                    orphans.raw_set(n + 2, army)?;
                    orphans.raw_set(n + 3, at.x.to_bits())?;
                    orphans.raw_set(n + 4, at.z.to_bits())?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Units removed from the world: kept blips still listed for them (stored
/// on after `Destroy`) are dropped.
pub fn units_removed(lua: &Lua, removed: &[EntityId]) -> LuaResult<()> {
    let kept = script_object::registry_table(lua, KEPT_KEY)?;
    for &unit in removed {
        if kept.raw_get::<LuaValue>(unit)? != LuaNil {
            kept.raw_set(unit, LuaNil)?;
        }
    }
    Ok(())
}

/// Blips due to go are destroyed: those of units destroyed in their army's
/// intel since the last pass, then orphans whose spot the army now sees.
/// `OnDestroy` (FAF's `Blip` runs its destroy hooks), in the order they
/// were queued.
fn destroy_dying(lua: &Lua) -> LuaResult<()> {
    let mut gone = Vec::new();
    let dying = script_object::registry_table(lua, DYING_KEY)?;
    let n = dying.raw_len();
    for i in 1..=n {
        gone.push(dying.raw_get::<LuaTable>(i)?);
        dying.raw_set(i, LuaNil)?;
    }
    let orphans = script_object::registry_table(lua, ORPHANS_KEY)?;
    let n = orphans.raw_len();
    if n > 0 {
        let mut entries = Vec::with_capacity(n / 4);
        for i in (1..=n).step_by(4) {
            let at = |k: usize| -> LuaResult<crate::engine::sim::math::Fx> { Ok(crate::engine::sim::math::Fx::from_bits(orphans.raw_get::<i64>(i + k)?)) };
            entries.push((orphans.raw_get::<LuaTable>(i)?, orphans.raw_get::<u32>(i + 1)?, at(2)?, at(3)?));
        }
        let seen: Vec<bool> = match lua.app_data_ref::<IntelGrids>() {
            Some(g) => entries
                .iter()
                .map(|&(_, army, x, z)| g.detection(army, crate::engine::sim::math::FxVec3 { x, y: crate::engine::sim::math::Fx::ZERO, z }, crate::engine::sim::layer::Layer::Land).seen)
                .collect(),
            None => vec![true; entries.len()],
        };
        for i in 1..=n {
            orphans.raw_set(i, LuaNil)?;
        }
        let mut k = 0;
        for ((blip, army, x, z), seen) in entries.into_iter().zip(seen) {
            if seen {
                gone.push(blip);
            } else {
                orphans.raw_set(k + 1, blip)?;
                orphans.raw_set(k + 2, army)?;
                orphans.raw_set(k + 3, x.to_bits())?;
                orphans.raw_set(k + 4, z.to_bits())?;
                k += 4;
            }
        }
    }
    if gone.is_empty() {
        return Ok(());
    }
    let cache = blips(lua)?;
    for blip in gone {
        if let (Some(army), Some(unit)) = (blip.raw_get::<Option<u32>>("_army")?, blip.raw_get::<Option<EntityId>>("_unit")?) {
            cache.raw_set(cache_key(army, unit), LuaNil)?;
        }
        script_object::run_script(lua, &blip, "OnDestroy", "blip", ())?;
    }
    Ok(())
}

/// What `army` knows of `unit` now, and whether it has ever seen it.
fn knowledge(lua: &Lua, army: u32, unit: EntityId) -> (Detection, bool) {
    let w = world(lua);
    let Some(e) = w.get(unit).filter(|e| !e.destroyed) else { return (Detection::default(), false) };
    let friendly = match (e.army, lua.app_data_ref::<Armies>()) {
        (Some(a), Some(armies)) => a == army || armies.alliance(army, a) == Alliance::Ally,
        _ => false,
    };
    if friendly {
        return (Detection { seen: true, radar: true, sonar: true, omni: true }, true);
    }
    let Some(grids) = lua.app_data_ref::<IntelGrids>() else {
        // No map loaded (tests): everything visible.
        return (Detection { seen: true, ..Default::default() }, true);
    };
    let now = grids.detect(army, e);
    (now, now.seen || grids.ever_seen(army, unit))
}

fn is_structure(lua: &Lua, unit: EntityId) -> bool {
    let index = world(lua).get(unit).and_then(|e| e.category_index);
    match (crate::engine::api::categories::domain(lua).ok(), index) {
        (Some(d), Some(i)) => d.named.get("STRUCTURE").is_some_and(|s| s.contains(i)),
        _ => false,
    }
}

/// `blip:Is*(army)`: the army argument, else the blip's own (`GetBlip`'s).
fn asking_army(lua: &Lua, this: &LuaTable, army: &LuaValue) -> LuaResult<u32> {
    match army {
        LuaValue::Nil => Ok(this.raw_get::<Option<u32>>("_army")?.unwrap_or(0)),
        a => resolve_army(lua, a),
    }
}

/// After the intel update: blips of units destroyed since the last pass are
/// destroyed ([`destroy_dying`]), each enemy army's brain hears its intel on a unit
/// change (`OnIntelChange(blip, reconType, val)`; FAF's jammer reset and
/// campaign intel triggers), and a unit hears the armies that saw it for the
/// first time (`OnDetectedBy(army)`).
pub fn intel_callbacks(lua: &Lua) -> LuaResult<()> {
    {
        let _s = crate::prof_span!("intel dying blips");
        destroy_dying(lua)?;
    }
    let (changes, first_seen) = match lua.app_data_mut::<IntelGrids>() {
        Some(mut g) if !g.changes.is_empty() || !g.first_seen.is_empty() => (std::mem::take(&mut g.changes), std::mem::take(&mut g.first_seen)),
        _ => return Ok(()),
    };
    let _lua = crate::prof_span!("lua intel callbacks");
    let class = blip_class(lua)?;
    let cache = blips(lua)?;
    // Each army's brain and its OnIntelChange, looked up once per tick (a
    // big battle makes thousands of changes a tick; the brain fetch and the
    // method's metatable lookup were half this pass).
    let mut handlers: std::collections::BTreeMap<u32, Option<(LuaTable, LuaFunction)>> = std::collections::BTreeMap::new();
    crate::prof_plot!("intel changes", changes.len());
    crate::prof_plot!("intel first seen", first_seen.len());
    let on_change = crate::prof_span!("lua OnIntelChange");
    for c in &changes {
        let handler = match handlers.get(&c.army) {
            Some(h) => h.clone(),
            None => {
                let h = match crate::engine::api::army::brain(lua, c.army)? {
                    LuaValue::Table(brain) => brain.get::<Option<LuaFunction>>("OnIntelChange")?.map(|f| (brain, f)),
                    _ => None,
                };
                handlers.insert(c.army, h.clone());
                h
            }
        };
        let Some((brain, f)) = handler else { continue };
        let blip = blip_object(lua, &cache, &class, c.army, c.unit)?;
        if let Err(e) = f.call::<()>((brain, blip, c.recon.name(), c.on)) {
            tracing::warn!(target: "lua", "Error running OnIntelChange script in brain: {e}");
        }
    }
    drop(on_change);
    let _detected = crate::prof_span!("lua OnDetectedBy");
    for &(unit, army) in &first_seen {
        if let Some(obj) = crate::engine::api::entity::object(lua, unit)? {
            UnitCallbacks::on_detected_by(lua, &obj, "unit", army)?;
        }
    }
    // Hand the buffers back for reuse.
    if let Some(mut g) = lua.app_data_mut::<IntelGrids>() {
        let (mut changes, mut first_seen) = (changes, first_seen);
        changes.clear();
        first_seen.clear();
        g.changes = changes;
        g.first_seen = first_seen;
    }
    Ok(())
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    let cache = lua.create_table()?;
    let weak = lua.create_table_with_capacity(0, 1)?;
    weak.raw_set("__mode", "v")?;
    cache.set_metatable(Some(weak));
    lua.set_named_registry_value(BLIPS_KEY, cache)?;
    lua_fn!(lua, global,
        /// Removes remembered recon blips in the area (moho: radar-covered parts
        /// regenerate unseen blips, which current detection does here).
        FlushIntelInRect = |lua, (x0, z0, x1, z1): (f64, f64, f64, f64)| {
            let fx = crate::engine::sim::math::from_lua;
            if let (Some(mut g), Some(w)) = (lua.app_data_mut::<IntelGrids>(), lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>()) {
                g.flush_in_rect(&w, fx(x0.min(x1)), fx(z0.min(z1)), fx(x0.max(x1)), fx(z0.max(z1)));
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Unit,
        /// The recon blip `army` has of this unit, or nil when it knows nothing
        /// of it (a structure it has ever seen keeps its blip).
        GetBlip = |lua, (this, army): (Obj<moho::Unit>, shapes::Army)| {
            let id = entity_id(&this)?;
            let army = resolve_army(lua, &army)?;
            let (now, ever) = knowledge(lua, army, id);
            if !now.any() && !(ever && is_structure(lua, id)) {
                return Ok(None);
            }
            let blip = blip_object(lua, &blips(lua)?, &blip_class(lua)?, army, id)?;
            Ok(Some(Obj::<moho::Blip>::new(blip)))
        })?;
    lua_fn!(lua, global,
        /// Is `object` a recon blip (FAF's `Blip` class over
        /// `moho.blip_methods`)? FAF checks orders' targets with it
        /// (`commands/shared.lua`).
        IsBlip = |lua, object: LuaValue| {
            let LuaValue::Table(t) = object else { return Ok(false) };
            let Some(m) = t.metatable() else { return Ok(false) };
            Ok(m == blip_class(lua)? || m == moho::class_table(lua, "blip_methods")?)
        })?;
    lua_method!(lua, moho::Blip,
        /// The unit or prop this blip stands for.
        GetSource = |lua, this: Obj<moho::Blip>| {
            crate::engine::api::entity::object_as::<moho::Unit>(lua, entity_id(&this)?)
        })?;
    fn query(lua: &Lua, name: &'static str, f: fn((Detection, bool)) -> bool) -> LuaResult<()> {
        crate::lua_api::add_class_fn(lua, moho::Blip::CLASS, name, Some(&[Some("this"), Some("army")]), "The army defaults to the blip's own (`GetBlip`'s).", Status::Implemented, move |lua, (this, army): (Obj<moho::Blip>, shapes::Army)| {
            let army = asking_army(lua, &this, &army)?;
            Ok(f(knowledge(lua, army, entity_id(&this)?)))
        })
    }
    query(lua, "IsSeenNow", |(d, _)| d.seen)?;
    query(lua, "IsSeenEver", |(_, ever)| ever)?;
    query(lua, "IsOnRadar", |(d, _)| d.radar)?;
    query(lua, "IsOnSonar", |(d, _)| d.sonar)?;
    query(lua, "IsOnOmni", |(d, _)| d.omni)?;
    lua_method!(lua, moho::Blip,
        /// Always false: fake blips (jammers) are not modelled yet.
        #[lua(stub)]
        IsKnownFake = |_, (_this, _army): (Obj<moho::Blip>, shapes::Army)| Ok(false))?;
    lua_method!(lua, moho::Blip,
        /// The source structure died out of the army's sight (until the army
        /// sees the spot and the blip goes).
        IsMaybeDead = |_lua, (this, _army): (Obj<moho::Blip>, shapes::Army)| {
            Ok(this.raw_get::<Option<bool>>("_maybe_dead")?.unwrap_or(false))
        })?;
    Ok(())
}
