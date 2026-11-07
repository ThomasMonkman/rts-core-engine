//! Entity script objects and `moho.entity_methods` / `moho.prop_methods`.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::blueprints::{BlueprintKind, BlueprintRegistry};
use crate::engine::api::callbacks::EntityCallbacks;
use crate::engine::api::{moho, script_object};
use crate::lua_api::{MohoClass, Obj, Status, lua_fn, lua_method};
use crate::engine::sim::entity::{EntityId, EntityKind, EntityWorld};
use crate::engine::sim::math::{Fx, FxQuat, FxVec3};

/// `_c_object` of an entity script object.
#[derive(Clone, Copy, Debug)]
pub struct EntityHandle(pub EntityId);
impl LuaUserData for EntityHandle {}

const ENTITIES_KEY: &str = "engine.entities";

/// Registry table: entity id -> Lua object.
fn objects(lua: &Lua) -> LuaResult<LuaTable> {
    crate::engine::api::script_object::registry_table(lua, ENTITIES_KEY)
}

pub fn entity_id(obj: &LuaTable) -> LuaResult<EntityId> {
    Ok(script_object::handle::<EntityHandle>(obj)?.0)
}

pub(crate) fn world(lua: &Lua) -> mlua::AppDataRefMut<'_, EntityWorld> {
    lua.app_data_mut::<EntityWorld>().expect("EntityWorld not in app_data")
}

/// Runs `f` on the entity behind script object `this` (no-op if it is gone).
pub(crate) fn with_entity_mut<R: Default>(
    lua: &Lua,
    this: &LuaTable,
    f: impl FnOnce(&mut crate::engine::sim::entity::Entity) -> R,
) -> LuaResult<R> {
    let id = entity_id(this)?;
    Ok(world(lua).get_mut(id).map(f).unwrap_or_default())
}

/// Initial intel from a blueprint's `Intel` section: radii set, vision on,
/// and the intel without a radius it says the unit has (`Cloak = true`,
/// `JammerBlips`, ...), off. `EnableIntel` turns on only these.
pub(crate) fn intel_from_blueprint(bp: &LuaTable) -> LuaResult<crate::engine::sim::intel::IntelMap> {
    use crate::engine::sim::intel::{IntelState, IntelType};
    let mut map = crate::engine::sim::intel::IntelMap::new();
    let Some(intel) = bp.get::<Option<LuaTable>>("Intel")? else { return Ok(map) };
    for t in IntelType::ALL {
        let Some(field) = t.radius_field() else { continue };
        if let Some(r) = intel.get::<Option<f64>>(field)? {
            map.insert(t, IntelState { enabled: t == IntelType::Vision, radius: crate::engine::sim::math::from_lua(r) });
        }
    }
    let has = |field: &str| -> LuaResult<bool> {
        Ok(match intel.get::<LuaValue>(field)? {
            LuaValue::Boolean(b) => b,
            LuaValue::Integer(n) => n > 0,
            LuaValue::Number(n) => n > 0.0,
            LuaValue::Table(_) => true,
            _ => false,
        })
    };
    for (t, fields) in [
        (IntelType::Cloak, &["Cloak"][..]),
        (IntelType::RadarStealth, &["RadarStealth"]),
        (IntelType::SonarStealth, &["SonarStealth"]),
        (IntelType::Jammer, &["JammerBlips", "JamRadius"]),
        (IntelType::Spoof, &["SpoofRadius"]),
    ] {
        if fields.iter().try_fold(false, |any, f| Ok::<_, LuaError>(any || has(f)?))? {
            map.entry(t).or_default();
        }
    }
    Ok(map)
}

fn intel_type(s: &str) -> LuaResult<crate::engine::sim::intel::IntelType> {
    crate::engine::sim::intel::IntelType::parse(s).ok_or_else(|| LuaError::runtime(format!("unknown intel type '{s}'")))
}

/// `bone` of `e` (or its origin) as posed by its manipulators: world
/// position and orientation (`sim::pose`).
pub fn posed_bone(lua: &Lua, e: &crate::engine::sim::entity::Entity, bone: Option<u32>) -> (FxVec3, FxQuat) {
    let manips = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>();
    let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
    let clips = lua.app_data_ref::<crate::engine::sim::pose::Clips>();
    crate::engine::sim::pose::bone_world(e, bone, manips.as_deref(), weapons.as_deref(), clips.as_deref())
}

/// An aircraft let go on the ground (off a factory's pad) takes off: it's in
/// the air from here and climbs to its flying height (FAF leaves new
/// aircraft to "fly off the factory by themselves").
fn take_off(lua: &Lua, unit: EntityId, parent: Option<EntityId>) -> LuaResult<()> {
    use crate::engine::sim::layer::Layer;
    let grounded_aircraft = world(lua).get(unit).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_params.kind == crate::engine::sim::movement::MotionKind::Air && u.layer != Layer::Air);
    if grounded_aircraft {
        crate::engine::api::movement::set_layer(lua, unit, Layer::Air)?;
    }
    if let Some(factory) = parent {
        crate::engine::api::command::hover_off_factory(lua, unit, factory)?;
    }
    Ok(())
}

/// Attaches with the parent bone as posed by its manipulators (the rest pose
/// for a parent without any).
pub fn attach_posed(lua: &Lua, child: EntityId, attachment: crate::engine::sim::entity::Attachment) {
    let manips = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>();
    let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
    let clips = lua.app_data_ref::<crate::engine::sim::pose::Clips>();
    world(lua).attach_posed(child, attachment, |p, b| manips.as_deref().is_some_and(|m| m.of(p.id).next().is_some()).then(|| crate::engine::sim::pose::bone_world(p, Some(b), manips.as_deref(), weapons.as_deref(), clips.as_deref()).0));
}

/// `src`'s posed bones in mesh space (before its scale), for `TryCopyPose`:
/// only those away from `sk`'s rest pose, so a wreck of a unit that died
/// at rest holds nothing (and costs its snapshots nothing).
fn copied_pose(
    src: &crate::engine::sim::entity::Entity,
    sk: &crate::engine::assets::scm::Skeleton,
    manips: Option<&crate::engine::sim::manipulator::Manipulators>,
    weapons: Option<&crate::engine::sim::weapon::Weapons>,
    clips: Option<&crate::engine::sim::pose::Clips>,
) -> Vec<(u32, (FxVec3, FxQuat))> {
    use crate::engine::sim::math::Fx;
    // 1/1024 of a mesh unit; a quaternion component of 1/4096 (~0.03°).
    let (moved, turned) = (Fx::from_bits(1 << 22), Fx::from_bits(1 << 20));
    let inverse = src.orientation.conjugate();
    let scale = if src.cold.scale > Fx::ZERO { src.cold.scale } else { Fx::ONE };
    let mut out = Vec::new();
    for (b, &(rest_p, rest_r)) in sk.model.iter().enumerate() {
        let (p, r) = crate::engine::sim::pose::bone_world(src, Some(b as u32), manips, weapons, clips);
        let local = inverse.rotate(p - src.position());
        let local = FxVec3 { x: local.x / scale, y: local.y / scale, z: local.z / scale };
        let r = inverse.mul(r);
        let d = local - rest_p;
        let far = |a: Fx, b: Fx| (a - b).abs() > turned;
        if d.x.abs() > moved || d.y.abs() > moved || d.z.abs() > moved || far(r.x, rest_r.x) || far(r.y, rest_r.y) || far(r.z, rest_r.z) || far(r.w, rest_r.w) {
            out.push((b as u32, (local, r)));
        }
    }
    out
}

/// World position of `bone` (posed; or the entity origin) of script object `this`.
pub(crate) fn bone_position(lua: &Lua, this: &LuaTable, bone: Option<LuaValue>) -> LuaResult<FxVec3> {
    let id = entity_id(this)?;
    // A collision beam's bone 1 is its far end (where its last trace
    // stopped): FAF's beam damage lands at `GetPosition(1)`.
    if (matches!(bone, Some(LuaValue::Integer(1))) || matches!(bone, Some(LuaValue::Number(n)) if n == 1.0))
        && let Some(end) = crate::engine::api::beam::end_of(lua, id) {
        return Ok(end);
    }
    let index = match &bone {
        Some(b) if !b.is_nil() => crate::engine::api::manipulator::resolve_bone(lua, id, b)?,
        _ => None,
    };
    Ok(world(lua).get(id).map(|e| posed_bone(lua, e, index).0).unwrap_or_default())
}

/// Queues a world sound at entity `id` for the next snapshot (presentation
/// only: nothing in the sim reads it back).
pub(crate) fn queue_sound(lua: &Lua, id: EntityId, sound: &crate::engine::api::sound::SoundHandle) {
    let position = world(lua).get(id).map(|e| {
        let p = e.position();
        [p.x.to_num::<f32>(), p.y.to_num::<f32>(), p.z.to_num::<f32>()]
    });
    if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
        p.sounds.push(crate::engine::sim::presentation::SoundEvent {
            entity: Some(id),
            bank: sound.bank.clone(),
            cue: sound.cue.clone(),
            lod_cutoff: sound.lod_cutoff.clone(),
            position,
        });
    }
}

/// Mesh scale from a blueprint's `Display.UniformScale` (default 1).
pub(crate) fn blueprint_scale(bp: &LuaTable) -> LuaResult<Fx> {
    let s = match bp.get::<Option<LuaTable>>("Display")? {
        Some(d) => d.get::<Option<f64>>("UniformScale")?.unwrap_or(1.0),
        None => 1.0,
    };
    Ok(crate::engine::sim::math::from_lua(s))
}

/// The Lua object for entity `id`, if it exists.
pub fn object(lua: &Lua, id: EntityId) -> LuaResult<Option<LuaTable>> {
    objects(lua)?.raw_get(id)
}

/// [`object`], declared as class `C` (for a function returning it).
pub fn object_as<C>(lua: &Lua, id: EntityId) -> LuaResult<Option<Obj<C>>> {
    Ok(object(lua, id)?.map(Obj::new))
}

/// Platoons whose last unit died end (killing their threads). Not inside
/// `Destroy`, which may be running on one of those threads: before the Lua
/// threads run (units killed by weapons and projectiles this tick: their
/// platoons' threads would otherwise wake to an empty platoon) and again at
/// compaction (units killed by Lua).
pub fn end_emptied_platoons(lua: &Lua) -> LuaResult<()> {
    let doomed = lua.app_data_mut::<crate::engine::sim::platoon::Platoons>().map(|mut p| p.take_doomed()).unwrap_or_default();
    for id in doomed {
        crate::engine::api::platoon::destroy(lua, id)?;
    }
    Ok(())
}

/// End of tick: drops destroyed entities from the world and releases what
/// the engine holds for them (script object registry, weapons, manipulators).
pub fn compact(lua: &Lua) -> LuaResult<()> {
    end_emptied_platoons(lua)?;
    let removed = match lua.app_data_mut::<EntityWorld>() {
        Some(mut w) => w.compact(),
        None => return Ok(()),
    };
    if removed.is_empty() {
        return Ok(());
    }
    crate::engine::api::blip::units_removed(lua, &removed)?;
    let registry = objects(lua)?;
    for &id in &removed {
        registry.raw_set(id, LuaNil)?;
    }
    if let Some(mut w) = lua.app_data_mut::<crate::engine::sim::weapon::Weapons>() {
        w.remove_units(&removed);
    }
    let dropped = lua.app_data_mut::<crate::engine::sim::manipulator::Manipulators>().map(|mut m| m.remove_owners(&removed)).unwrap_or_default();
    if !dropped.is_empty() {
        crate::engine::api::manipulator::release_objects(lua, &dropped)?;
    }
    if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::projectile::Projectiles>() {
        p.remove_ids(&removed);
    }
    Ok(())
}

/// The engine's `Destroy` (moho's `Entity::Destroy`, MohoEngine.dll
/// 0x1025e420): `BeenDestroyed` is true from here on and systems skip the
/// entity, but its `OnDestroy` waits for the end of the beat
/// ([`run_destroy_queue`]); a second call is a no-op. Engine code that
/// destroys an entity itself (not through its script's `Destroy`) calls this.
pub fn destroy(lua: &Lua, this: &Obj<moho::Entity>) -> LuaResult<()> {
    let id = entity_id(this)?;
    if !world(lua).destroy(id) {
        return Ok(());
    }
    // A destroyed prop leaves the bounded list (moho's Prop
    // `RemoveBoundedProp` with its handle).
    let prop = world(lua).get(id).is_some_and(|e| e.kind == crate::engine::sim::entity::EntityKind::Prop);
    if let (true, Some(mut b)) = (prop, lua.app_data_mut::<crate::engine::sim::bounded_props::BoundedProps>())
        && !b.is_empty() {
        b.remove(id);
    }
    crate::engine::api::unit_stats::unit_destroyed(lua, id);
    if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
        p.ambient.remove(&id);
    }
    crate::engine::api::blip::unit_destroyed(lua, id)?;
    // Its orders end now (IsCommandDone shouldn't wait for compaction).
    crate::engine::api::command::clear_unit(lua, id);
    if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::platoon::Platoons>() {
        p.unit_destroyed(id);
    }
    Ok(())
}

/// End of the beat (after the Lua threads, before `Sync`): the `OnDestroy`
/// of every entity destroyed since the last drain, first destroyed first, as
/// `Sim::AdvanceBeat` drains its destroy queue (0x103194a1). An entity
/// destroyed meanwhile (by an `OnDestroy`) joins the back of this drain, so
/// the calls never nest. Each one (`Unit::OnDestroy` 0x10287f80, then
/// `Entity::OnDestroy` 0x1025e4b0): the native part first (adjacency, its
/// weapons, the ground it blocked), then the script's `OnDestroy` while it is
/// still attached, then `parent:OnAttachedDestroyed(self)` and the detach,
/// then `child:OnParentDestroyed(self)` for each child. Once the drain is
/// done the objects are freed (`IsDestroyed`), as moho's `Sim::Sync` deletes
/// them after the beat (`EntityDB::Purge` 0x10268270).
pub fn run_destroy_queue(lua: &Lua) -> LuaResult<()> {
    let mut done = Vec::new();
    loop {
        let Some(id) = world(lua).next_destroyed() else { break };
        on_destroy(lua, id)?;
        done.push(id);
    }
    let mut w = world(lua);
    for id in done {
        w.free(id);
    }
    Ok(())
}

/// One entity's turn in [`run_destroy_queue`].
fn on_destroy(lua: &Lua, id: EntityId) -> LuaResult<()> {
    crate::engine::api::adjacency::on_destroying(lua, id)?;
    crate::engine::api::weapon::destroy_weapons(lua, id)?;
    // A destroyed structure stops blocking the ground.
    let block = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()).and_then(|u| Some((u.nav_block.take()?, std::mem::take(&mut u.nav_occupy))));
    if let (Some((r, occupy)), Some(mut grid)) = (block, lua.app_data_mut::<crate::engine::sim::nav::NavGrid>()) {
        if occupy.is_empty() {
            grid.block_rect(r.0, r.1, r.2, r.3, false);
        }
        for r in occupy {
            grid.block_rect(r.0, r.1, r.2, r.3, false);
        }
    }
    let Some(this) = object(lua, id)? else {
        world(lua).detach_destroyed(id);
        return Ok(());
    };
    EntityCallbacks::on_destroy(lua, &this, "entity")?;
    let parent = world(lua).get(id).and_then(|e| e.attachment).map(|a| a.parent);
    if let Some(parent) = parent.map(|p| object(lua, p)).transpose()?.flatten() {
        EntityCallbacks::on_attached_destroyed(lua, &parent, "entity", Obj::new(this.clone()))?;
    }
    let children = world(lua).attached_to(id);
    world(lua).detach_destroyed(id);
    for child in children {
        if let Some(child) = object(lua, child)? {
            EntityCallbacks::on_parent_destroyed(lua, &child, "entity", Obj::new(this.clone()))?;
        }
    }
    Ok(())
}

/// A Lua array of the script objects of `ids`, in order (unbound ids skipped).
/// The script objects of `ids`, as a list of class `C` (the caller knows
/// which: `objects_table::<moho::Unit>`).
pub(crate) fn objects_table<C>(lua: &Lua, ids: &[EntityId]) -> LuaResult<shapes::List<Obj<C>>> {
    let t = lua.create_table_with_capacity(ids.len(), 0)?;
    let mut n = 0;
    for &id in ids {
        if let Some(obj) = object(lua, id)? {
            n += 1;
            t.raw_set(n, obj)?;
        }
    }
    Ok(shapes::List::new(t))
}

/// Instantiates `import(module)[class]` (FAF class factories create the
/// instance table), binds it to entity `id` and registers it.
pub(crate) fn create_object(lua: &Lua, id: EntityId, module: &str, class: &str) -> LuaResult<LuaTable> {
    let import: LuaFunction = lua.globals().get("import")?;
    let m: LuaTable = import.call(module)?;
    let class_table: LuaTable = match m.get::<LuaValue>(class)? {
        LuaValue::Table(t) => t,
        _ => return Err(LuaError::runtime(format!("script class '{class}' not found in '{module}'"))),
    };
    let instance: LuaTable = class_table.call(())?;
    instance.raw_set(script_object::C_OBJECT, EntityHandle(id))?;
    objects(lua)?.raw_set(id, instance.clone())?;
    Ok(instance)
}

/// Binds `obj` (a script instance still inside its `__init`) to a new,
/// blueprint-less engine entity. FAF's `Entity.__post_init` then calls
/// `OnCreate(spec)` itself.
pub(crate) fn bind_script_entity(
    lua: &Lua,
    obj: &LuaTable,
    kind: EntityKind,
    army: Option<u32>,
    position: FxVec3,
    orientation: FxQuat,
) -> LuaResult<EntityId> {
    let id = world(lua).spawn(kind, BlueprintKind::Unit, String::new(), army, position, orientation);
    obj.raw_set(script_object::C_OBJECT, EntityHandle(id))?;
    objects(lua)?.raw_set(id, obj.clone())?;
    Ok(id)
}

pub(crate) fn blueprint_table(lua: &Lua, kind: BlueprintKind, id: &str) -> LuaResult<LuaTable> {
    let reg = lua.app_data_ref::<BlueprintRegistry>().expect("BlueprintRegistry");
    reg.get(kind, id)
        .map(|b| b.table.clone())
        .ok_or_else(|| LuaError::runtime(format!("unknown {kind:?} blueprint '{id}'")))
}

pub(crate) fn vector(lua: &Lua, v: FxVec3) -> LuaResult<shapes::Vector> {
    let (x, y, z) = v.to_f64();
    let t = lua.create_table_with_capacity(3, 0)?;
    t.raw_set(1, x)?;
    t.raw_set(2, y)?;
    t.raw_set(3, z)?;
    t.set_metatable(Some(lua.named_registry_value::<LuaTable>("vector_metatable")?));
    Ok(shapes::Vector(t))
}

fn read_quat(o: &LuaTable) -> LuaResult<FxQuat> {
    Ok(FxQuat {
        x: crate::engine::sim::math::from_lua(o.raw_get::<f64>(1)?),
        y: crate::engine::sim::math::from_lua(o.raw_get::<f64>(2)?),
        z: crate::engine::sim::math::from_lua(o.raw_get::<f64>(3)?),
        w: crate::engine::sim::math::from_lua(o.raw_get::<f64>(4)?),
    })
}

/// Quaternion as FAF sees it: `{x, y, z, w}` (same layout as [`read_quat`]),
/// with the vector metatable (`.x` access, `*` operators).
pub(crate) fn quaternion(lua: &Lua, q: FxQuat) -> LuaResult<shapes::Quaternion> {
    let (x, y, z, w) = q.to_f64();
    let t = lua.create_table_with_capacity(4, 0)?;
    t.raw_set(1, x)?;
    t.raw_set(2, y)?;
    t.raw_set(3, z)?;
    t.raw_set(4, w)?;
    t.set_metatable(Some(lua.named_registry_value::<LuaTable>("vector_metatable")?));
    Ok(shapes::Quaternion(t))
}

fn read_vector(v: &LuaTable) -> LuaResult<FxVec3> {
    Ok(FxVec3::from_f64(v.raw_get(1)?, v.raw_get(2)?, v.raw_get(3)?))
}

/// Spawns a prop from blueprint `bp_path` and runs its `OnCreate`.
pub fn spawn_prop(lua: &Lua, bp_path: &str, position: FxVec3, orientation: FxQuat) -> LuaResult<Obj<moho::Prop>> {
    let bp_id = bp_path.to_lowercase();
    let bp = blueprint_table(lua, BlueprintKind::Prop, &bp_id)?;
    let module: String = bp.get::<Option<String>>("ScriptModule")?.unwrap_or_else(|| "/lua/sim/prop.lua".into());
    let class: String = bp.get::<Option<String>>("ScriptClass")?.unwrap_or_else(|| "Prop".into());
    let skeleton = crate::engine::api::mesh::skeleton_for(lua, &bp)?;
    let render_mesh = crate::engine::api::mesh::render_index_for(lua, &bp)?;
    let id = world(lua).spawn(EntityKind::Prop, BlueprintKind::Prop, bp_id, None, position, orientation);
    let scale = blueprint_scale(&bp)?;
    if let Some(e) = world(lua).get_mut(id) {
        e.cold.skeleton = skeleton;
        e.cold.scale = scale;
        e.cold.draw_scale = FxVec3 { x: scale, y: scale, z: scale };
        e.cold.render_mesh = render_mesh;
        e.cold.tree = match class.as_str() {
            "Tree" => Some(crate::engine::sim::entity::Tree::Single),
            "TreeGroup" => Some(crate::engine::sim::entity::Tree::Group),
            _ => None,
        };
    }
    let obj = create_object(lua, id, &module, &class)?;
    EntityCallbacks::on_create(lua, &obj, bp_path)?;
    Ok(Obj::new(obj))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(EntityWorld::default());
    lua.set_app_data(crate::engine::sim::bounded_props::BoundedProps::default());

    lua_fn!(lua, global,
        /// `GetUnitsInRect(rect)` or `(x0, z0, x1, z1)`: live units inside the XZ
        /// rectangle (inclusive), in entity id order; nil when there are none.
        GetUnitsInRect = |lua, args: LuaMultiValue| {
            let num = |v: Option<&LuaValue>| -> LuaResult<Fx> {
                match v {
                    Some(LuaValue::Integer(i)) => Ok(Fx::saturating_from_num(*i)),
                    Some(LuaValue::Number(n)) => Ok(crate::engine::sim::math::from_lua(*n)),
                    _ => Err(LuaError::runtime("GetUnitsInRect: expected a Rectangle or four numbers")),
                }
            };
            let (x0, z0, x1, z1) = match args.front() {
                Some(LuaValue::Table(r)) => (
                    num(Some(&r.get::<LuaValue>("x0")?))?,
                    num(Some(&r.get::<LuaValue>("y0")?))?,
                    num(Some(&r.get::<LuaValue>("x1")?))?,
                    num(Some(&r.get::<LuaValue>("y1")?))?,
                ),
                _ => (num(args.front())?, num(args.get(1))?, num(args.get(2))?, num(args.get(3))?),
            };
            let (minx, maxx, minz, maxz) = (x0.min(x1), x0.max(x1), z0.min(z1), z0.max(z1));
            let ids = world(lua).units_in_rect(minx, minz, maxx, maxz);
            if ids.is_empty() {
                return Ok(None);
            }
            Ok(Some(objects_table::<moho::Unit>(lua, &ids)?))
        })?;
    // Type predicates on script objects: false for non-entities and for
    // entities whose engine record is gone.
    fn kind_of(lua: &Lua, v: &LuaValue) -> Option<EntityKind> {
        let LuaValue::Table(t) = v else { return None };
        let id = script_object::handle::<EntityHandle>(t).ok()?.0;
        world(lua).get(id).map(|e| e.kind)
    }
    lua_fn!(lua, global,
        /// Always an empty table: the sim has no player selection.
        DebugGetSelection = |lua, ()| lua.create_table().map(shapes::List::<Obj<moho::Unit>>::new))?;
    lua_fn!(lua, global,
        /// True for a live entity script object (unit, prop, projectile, shield, ...).
        IsEntity = |lua, v: LuaValue| Ok(kind_of(lua, &v).is_some()))?;
    lua_fn!(lua, global,
        /// Ids reach Lua as strings (`GetEntityId`, a command's `targetId`) but
        /// numbers are accepted too. nil for unknown or destroyed entities.
        GetEntityById = |lua, id: shapes::EntityId| {
            let id = id.into_inner();
            let id = match id {
                LuaValue::Integer(i) => EntityId::try_from(i).ok(),
                LuaValue::Number(n) => (n >= 0.0 && n.fract() == 0.0).then_some(n as EntityId),
                LuaValue::String(s) => s.to_str().ok().and_then(|s| s.parse().ok()),
                _ => None,
            };
            let Some(id) = id.filter(|&id| world(lua).get(id).is_some_and(|e| !e.destroyed)) else { return Ok(None) };
            object_as::<moho::Entity>(lua, id)
        })?;
    lua_fn!(lua, global,
        IsUnit = |lua, v: LuaValue| Ok(kind_of(lua, &v) == Some(EntityKind::Unit)))?;
    lua_fn!(lua, global,
        IsProp = |lua, v: LuaValue| Ok(kind_of(lua, &v) == Some(EntityKind::Prop)))?;
    lua_fn!(lua, global,
        IsProjectile = |lua, v: LuaValue| Ok(kind_of(lua, &v) == Some(EntityKind::Projectile)))?;
    lua_fn!(lua, global,
        /// Props (incl. wrecks) and units in the rectangle, id order; nil if none.
        /// TODO: moho tests collision/visual boxes, we test origins.
        GetReclaimablesInRect = |lua, args: LuaMultiValue| {
            let num = |v: Option<&LuaValue>| -> LuaResult<Fx> {
                match v {
                    Some(LuaValue::Integer(i)) => Ok(Fx::saturating_from_num(*i)),
                    Some(LuaValue::Number(n)) => Ok(crate::engine::sim::math::from_lua(*n)),
                    _ => Err(LuaError::runtime("GetReclaimablesInRect: expected a Rectangle or four numbers")),
                }
            };
            let (x0, z0, x1, z1) = match args.front() {
                Some(LuaValue::Table(r)) => (
                    num(Some(&r.get::<LuaValue>("x0")?))?,
                    num(Some(&r.get::<LuaValue>("y0")?))?,
                    num(Some(&r.get::<LuaValue>("x1")?))?,
                    num(Some(&r.get::<LuaValue>("y1")?))?,
                ),
                _ => (num(args.front())?, num(args.get(1))?, num(args.get(2))?, num(args.get(3))?),
            };
            let (minx, maxx, minz, maxz) = (x0.min(x1), x0.max(x1), z0.min(z1), z0.max(z1));
            let mut ids = {
                let w = world(lua);
                let mut ids = w.props_in_rect(minx, minz, maxx, maxz);
                ids.extend(w.units_in_rect(minx, minz, maxx, maxz));
                ids
            };
            ids.sort_unstable();
            if ids.is_empty() {
                return Ok(None);
            }
            Ok(Some(objects_table::<moho::Entity>(lua, &ids)?))
        })?;
    lua_fn!(lua, global,
        /// Creates a prop from the blueprint `bp` at `location`.
        CreateProp = |lua, (location, bp): (shapes::Vector, String)| {
            spawn_prop(lua, &bp, read_vector(&location)?, FxQuat::IDENTITY)
        })?;
    lua_fn!(lua, global,
        /// Copies `from`'s bone pose (animations, aims, earlier copies) onto
        /// `to`, and its transform when `copy_transform`: FAF's wrecks keep
        /// their unit's death pose. Mismatched skeletons are a no-op.
        TryCopyPose = |lua, (from, to, copy_transform): (Obj<moho::Entity>, Obj<moho::Entity>, Option<bool>)| {
            let (from, to) = (entity_id(&from)?, entity_id(&to)?);
            let pose = {
                let w = world(lua);
                let Some(src) = w.get(from) else { return Ok(()) };
                let (Some(a), Some(b)) = (&src.cold.skeleton, w.get(to).and_then(|e| e.cold.skeleton.as_ref())) else { return Ok(()) };
                let same_skeleton = a.bones.len() == b.bones.len() && a.bones.iter().zip(&b.bones).all(|(x, y)| x.name.eq_ignore_ascii_case(&y.name));
                if !same_skeleton {
                    return Ok(());
                }
                let manips = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>();
                let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
                let clips = lua.app_data_ref::<crate::engine::sim::pose::Clips>();
                copied_pose(src, a, manips.as_deref(), weapons.as_deref(), clips.as_deref())
            };
            let mut w = world(lua);
            if copy_transform.unwrap_or(false) {
                let (position, orientation) = w.get(from).map(|e| (e.position(), e.orientation)).unwrap_or_default();
                w.set_transform(to, position, orientation);
            }
            let posed = !pose.is_empty();
            if let Some(e) = w.get_mut(to) {
                e.cold.copied_pose = pose;
            }
            drop(w);
            if posed {
                // Sent as `Place` ops with the hidden bones.
                crate::engine::snapshot::note_hidden_bones(lua, to);
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Creates a prop from the blueprint `bp` at (x, y, z), turned by heading, pitch and roll.
        CreatePropHPR = |lua, (bp, x, y, z, h, p, r): (String, f64, f64, f64, f64, f64, f64)| {
            let q = FxQuat::from_hpr(
                crate::engine::sim::math::from_lua(h),
                crate::engine::sim::math::from_lua(p),
                crate::engine::sim::math::from_lua(r),
            );
            spawn_prop(lua, &bp, FxVec3::from_f64(x, y, z), q)
        })?;

    lua_method!(lua, moho::Entity,
        /// `SetPosition(position, immediate)`: moves it there (FAF's missile
        /// redirect puts its new projectile where the old one was).
        SetPosition = |lua, (this, pos, _immediate): (Obj<moho::Entity>, shapes::Vector, Option<bool>)| {
            let id = entity_id(&this)?;
            let p = read_vector(&pos)?;
            world(lua).set_position(id, p);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// Detaches this entity from what it is attached to.
        DetachFrom = |lua, (this, _skip_ballistic): (Obj<moho::Entity>, Option<bool>)| {
            let parent = with_entity_mut(lua, &this, |e| e.attachment.take().map(|a| a.parent))?;
            take_off(lua, entity_id(&this)?, parent)
        })?;
    lua_method!(lua, moho::Entity,
        /// Detaches everything attached to this entity (to `bone`, if given).
        DetachAll = |lua, (this, bone, _skip): (Obj<moho::Entity>, Option<shapes::Bone>, Option<bool>)| {
            let id = entity_id(&this)?;
            let bone = match &bone {
                Some(b) if !b.is_nil() => crate::engine::api::manipulator::resolve_bone(lua, id, b)?,
                _ => None,
            };
            // (From the attach index: scanning every entity per call was a
            // big battle's costliest engine call from Lua.)
            let mut freed = Vec::new();
            {
                let mut w = world(lua);
                for child in w.attached_to(id) {
                    if let Some(e) = w.get_mut(child).filter(|e| e.attachment.is_some_and(|a| bone.is_none() || a.parent_bone == bone)) {
                        e.attachment = None;
                        freed.push(child);
                    }
                }
            }
            for unit in freed {
                take_off(lua, unit, Some(id))?;
            }
            Ok(())
        })?;
    // Texture scrollers (`sim::scroller`): tank treads, aircraft panels.
    fn set_scroller(lua: &Lua, this: &LuaTable, kind: Option<crate::engine::sim::scroller::ScrollKind>) -> LuaResult<()> {
        let id = entity_id(this)?;
        {
            let mut w = world(lua);
            let Some(e) = w.get_mut(id) else { return Ok(()) };
            let (p, q) = (e.position(), e.orientation);
            e.cold.scroller = kind.map(|k| crate::engine::sim::scroller::Scroller::new(k, p, q));
        }
        if kind.is_some() {
            if lua.app_data_ref::<crate::engine::sim::scroller::Scrolled>().is_none() {
                lua.set_app_data(crate::engine::sim::scroller::Scrolled::default());
            }
            if let Some(mut s) = lua.app_data_mut::<crate::engine::sim::scroller::Scrolled>() {
                s.0.insert(id);
            }
        }
        Ok(())
    }
    let fx = crate::engine::sim::math::from_lua;
    lua_method!(lua, moho::Entity,
        AddThreadScroller = move |lua, (this, side, mult): (Obj<moho::Entity>, f64, f64)| {
            set_scroller(lua, &this, Some(crate::engine::sim::scroller::ScrollKind::Thread { side: fx(side), mult: fx(mult) }))
        })?;
    lua_method!(lua, moho::Entity,
        AddManualScroller = move |lua, (this, a, b): (Obj<moho::Entity>, f64, f64)| {
            set_scroller(lua, &this, Some(crate::engine::sim::scroller::ScrollKind::Manual { speed: (fx(a), fx(b)) }))
        })?;
    lua_method!(lua, moho::Entity,
        /// Eight numbers: ping1, ping1 speed, pong1, pong1 speed, then the same
        /// for the second value (FAF's AirUnit.AddPingPong).
        AddPingPongScroller = move |lua, (this, a, b, c, d, e, f, g, h): (Obj<moho::Entity>, f64, f64, f64, f64, f64, f64, f64, f64)| {
            let values = [(fx(a), fx(b), fx(c), fx(d)), (fx(e), fx(f), fx(g), fx(h))];
            set_scroller(lua, &this, Some(crate::engine::sim::scroller::ScrollKind::PingPong { values, back: [false; 2] }))
        })?;
    lua_method!(lua, moho::Entity,
        RemoveScroller = |lua, this: Obj<moho::Entity>| set_scroller(lua, &this, None))?;
    lua_method!(lua, moho::Entity,
        /// The entity id as a string.
        GetEntityId = |_, this: Obj<moho::Entity>| {
            Ok(entity_id(&this)?.to_string())
        })?;
    /// Army and transform of a spec's `Owner` (`None` if no owner): an
    /// entity, or a 0-based army index (FAF's pings: `Owner = army - 1`),
    /// which gives only the army.
    fn owner_placement(lua: &Lua, spec: &LuaTable) -> LuaResult<Option<(Option<u32>, FxVec3, FxQuat)>> {
        let owner = match spec.get::<LuaValue>("Owner")? {
            LuaValue::Table(t) => t,
            LuaValue::Integer(i) if i >= 0 => return Ok(Some((Some(i as u32 + 1), FxVec3::default(), FxQuat::IDENTITY))),
            LuaValue::Number(n) if n >= 0.0 => return Ok(Some((Some(n as u32 + 1), FxVec3::default(), FxQuat::IDENTITY))),
            _ => return Ok(None),
        };
        let owner_id = entity_id(&owner)?;
        let w = world(lua);
        let o = w.get(owner_id).ok_or_else(|| LuaError::runtime("owner entity no longer exists"))?;
        Ok(Some((o.army, o.position(), o.orientation)))
    }
    /// The spec table `Entity.__init` / `Shield.__init` read.
    #[derive(crate::lua_api::LuaTable)]
    #[lua(name = "EntitySpec", write_only)]
    #[allow(dead_code)]
    struct EntitySpec {
        /// An entity, or a 0-based army index (pings).
        #[lua(rename = "Owner")]
        owner: Option<LuaValue>,
    }
    lua_fn!(lua, global,
        /// `Shield.__init`: the shield sits on its owner (it attaches itself via
        /// `AttachBoneTo` in `OnCreate`) and fights for the owner's army.
        _c_CreateShield = |lua, (shield, spec): (Obj<moho::Shield>, shapes::Shaped<EntitySpec>)| {
            let (army, position, orientation) = owner_placement(lua, &spec)?
                .ok_or_else(|| LuaError::runtime("_c_CreateShield: spec has no Owner"))?;
            bind_script_entity(lua, &shield, EntityKind::Shield, army, position, orientation)?;
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// `Entity.__init`: an optional `spec.Owner` gives army and start position.
        _c_CreateEntity = |lua, (entity, spec): (Obj<moho::Entity>, Option<shapes::Shaped<EntitySpec>>)| {
            let placement = match &spec {
                Some(spec) => owner_placement(lua, spec)?,
                None => None,
            };
            let (army, position, orientation) = placement.unwrap_or((None, FxVec3::default(), FxQuat::IDENTITY));
            bind_script_entity(lua, &entity, EntityKind::Script, army, position, orientation)?;
            Ok(())
        })?;
    /// Registers `entity:<name>(mode)` writing one `VizModes` field.
    fn viz_setter(
        lua: &Lua,
        name: &'static str,
        field: fn(&mut crate::engine::sim::entity::VizModes) -> &mut crate::engine::sim::entity::VisionMode,
    ) -> LuaResult<()> {
        crate::lua_api::add_class_fn(
            lua,
            moho::Entity::CLASS,
            name,
            Some(&[Some("this"), Some("mode")]),
            "Sets when this group of players sees the entity (`SetVizTo*`): one of `Never`, `Always`, `Intel`.",
            Status::Implemented,
            move |lua, (this, mode): (Obj<moho::Entity>, String)| {
                let mode = crate::engine::sim::entity::VisionMode::parse(&mode)
                    .ok_or_else(|| LuaError::runtime(format!("{name}: unknown vision mode '{mode}'")))?;
                with_entity_mut(lua, &this, |e| *field(&mut e.cold.viz) = mode)
            },
        )
    }
    viz_setter(lua, "SetVizToFocusPlayer", |v| &mut v.focus)?;
    viz_setter(lua, "SetVizToEnemies", |v| &mut v.enemies)?;
    viz_setter(lua, "SetVizToAllies", |v| &mut v.allies)?;
    viz_setter(lua, "SetVizToNeutrals", |v| &mut v.neutrals)?;
    lua_method!(lua, moho::Entity,
        /// The entity's blueprint table; nil for blueprint-less entities (shields, script entities).
        GetBlueprint = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            let (kind, bp) = {
                let w = world(lua);
                let e = w.get(id).ok_or_else(|| LuaError::runtime("entity no longer exists"))?;
                (e.cold.blueprint_kind, e.cold.blueprint_id.clone())
            };
            // Blueprint-less entities (shields, script entities) have none.
            if bp.is_empty() {
                return Ok(None);
            }
            blueprint_table(lua, kind, &bp).map(|t| Some(shapes::Blueprint(t)))
        })?;
    lua_method!(lua, moho::Entity,
        /// The owning army (1-based), or -1 for an entity without one.
        GetArmy = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            Ok(world(lua).get(id).and_then(|e| e.army).map_or(-1, |a| a as i64))
        })?;
    lua_method!(lua, moho::Entity,
        /// The owning army's brain; nil for an entity without an army.
        GetAIBrain = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            let army = world(lua).get(id).and_then(|e| e.army);
            match army {
                Some(a) => crate::engine::api::army::brain_obj(lua, a),
                None => Ok(None),
            }
        })?;
    lua_method!(lua, moho::Entity,
        /// True if `bone` (name or index) exists in the entity's skeleton. With
        /// `allow_nil`, index -1 (the entity itself) is valid too and nil is
        /// false; without it nil raises, as does any other type. Moho:
        /// MohoEngine.dll 0x1026ebd0 (FAF's annotation says nothing of -1).
        IsValidBone = |lua, (this, bone, allow_nil): (Obj<moho::Entity>, shapes::Bone, Option<bool>)| {
            let allow_nil = allow_nil.unwrap_or(false);
            let index = match bone.into_inner() {
                LuaValue::String(s) => {
                    let name = s.to_str()?.to_string();
                    let id = entity_id(&this)?;
                    return Ok(world(lua).get(id).and_then(|e| e.cold.skeleton.clone()).is_some_and(|skel| skel.find(&name).is_some()));
                }
                // Lua 5.0 truncates the number to an integer.
                LuaValue::Integer(i) => i as i64,
                LuaValue::Number(n) => n.trunc() as i64,
                LuaValue::Nil if allow_nil => return Ok(false),
                // Moho's messages, as it pairs them.
                _ => return Err(LuaError::runtime(if allow_nil { "Invalid bone identifier; must be a string or integer" } else { "Invalid bone identifier; must be a string, integer, or nil" })),
            };
            let id = entity_id(&this)?;
            let w = world(lua);
            let bones = w.get(id).and_then(|e| e.cold.skeleton.clone()).map_or(0, |skel| skel.bones.len() as i64);
            Ok(index >= if allow_nil { -1 } else { 0 } && index < bones)
        })?;
    lua_method!(lua, moho::Entity,
        /// Gives the entity intel of `type` (disabled until `EnableIntel`); an
        /// entity without an army (e.g. a VizMarker) takes `army` as its owner.
        InitIntel = |lua, (this, army, kind, radius): (Obj<moho::Entity>, shapes::Army, String, Option<f64>)| {
            let t = intel_type(&kind)?;
            let army = crate::engine::api::army::resolve_army(lua, &army).ok();
            with_entity_mut(lua, &this, |e| {
                let s = e.cold.intel.entry(t).or_default();
                s.radius = crate::engine::sim::math::from_lua(radius.unwrap_or(0.0));
                if e.army.is_none() {
                    e.army = army;
                }
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// Turns on the intel of `type` the entity has (from its blueprint or
        /// `InitIntel`); one it doesn't have stays off. LOUD tries every type
        /// on every unit and keeps those `IsIntelEnabled` then reports.
        EnableIntel = |lua, (this, kind): (Obj<moho::Entity>, String)| {
            let t = intel_type(&kind)?;
            with_entity_mut(lua, &this, |e| {
                if let Some(s) = e.cold.intel.get_mut(&t) {
                    s.enabled = true;
                }
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// True if the intel of `type` is enabled.
        IsIntelEnabled = |lua, (this, kind): (Obj<moho::Entity>, String)| {
            let t = intel_type(&kind)?;
            with_entity_mut(lua, &this, |e| e.cold.intel.get(&t).is_some_and(|s| s.enabled))
        })?;
    lua_method!(lua, moho::Entity,
        /// FAF's aircraft halve their vision when landed.
        SetIntelRadius = |lua, (this, kind, radius): (Obj<moho::Entity>, String, f64)| {
            let t = intel_type(&kind)?;
            with_entity_mut(lua, &this, |e| e.cold.intel.entry(t).or_default().radius = crate::engine::sim::math::from_lua(radius.max(0.0)))
        })?;
    lua_method!(lua, moho::Entity,
        /// The intel radius of `type` (0 if the entity has none).
        GetIntelRadius = |lua, (this, kind): (Obj<moho::Entity>, String)| {
            let t = intel_type(&kind)?;
            with_entity_mut(lua, &this, |e| e.cold.intel.get(&t).map_or(0.0, |s| s.radius.to_num::<f64>()))
        })?;
    lua_method!(lua, moho::Entity,
        /// Turns off the intel of `type`.
        DisableIntel = |lua, (this, kind): (Obj<moho::Entity>, String)| {
            let t = intel_type(&kind)?;
            with_entity_mut(lua, &this, |e| {
                if let Some(s) = e.cold.intel.get_mut(&t) {
                    s.enabled = false;
                }
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// Plays a sound at the entity (nothing when `sound` is nil).
        PlaySound = |lua, (this, sound): (Obj<moho::Entity>, Option<LuaUserDataRef<crate::engine::api::sound::SoundHandle>>)| {
            let Some(sound) = sound else { return Ok(()) };
            queue_sound(lua, entity_id(&this)?, &sound);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// Shakes the camera of players near the entity.
        ShakeCamera = |lua, (this, radius, max_shake, min_shake, duration): (Obj<moho::Entity>, f64, f64, f64, f64)| {
            let entity = entity_id(&this)?;
            if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
                p.camera_shakes.push(crate::engine::sim::presentation::CameraShake { entity, radius, max_shake, min_shake, duration });
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// The second (rumble) parameter is always nil in FAF; ignored.
        SetAmbientSound = |lua, (this, sound, _rumble): (Obj<moho::Entity>, Option<LuaUserDataRef<crate::engine::api::sound::SoundHandle>>, LuaValue)| {
            let id = entity_id(&this)?;
            if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
                match sound {
                    Some(sound) => {
                        p.ambient.insert(id, crate::engine::sim::presentation::AmbientSound {
                            bank: sound.bank.clone(),
                            cue: sound.cue.clone(),
                        });
                    }
                    None => {
                        p.ambient.remove(&id);
                    }
                }
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// Hides a bone (and its children if `children`) from rendering. Moho
        /// has it on units only (MohoEngine.dll); ours takes any entity.
        HideBone = |lua, (this, bone, children): (Obj<moho::Entity>, shapes::Bone, Option<bool>)| {
            let id = entity_id(&this)?;
            let Some(index) = crate::engine::api::manipulator::resolve_bone(lua, id, &bone)? else { return Ok(()) };
            with_entity_mut(lua, &this, |e| {
                for b in e.bone_set(index, children.unwrap_or(false)) {
                    e.cold.hidden_bones.insert(b);
                }
            })?;
            crate::engine::snapshot::note_hidden_bones(lua, id);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// `AttachBoneToEntityBone(selfBone, other, otherBone, flag)`: draws this
        /// entity's bone where `other`'s bone is (-1: its origin); moves neither
        /// entity. FAF tosses death debris with it: the part's bone follows a
        /// debris projectile. Drawn as long as both live; the flag (always false
        /// in FAF) is ignored.
        AttachBoneToEntityBone = |lua, (this, bone, other, other_bone, _flag): (Obj<moho::Entity>, shapes::Bone, Obj<moho::Entity>, shapes::Bone, Option<bool>)| {
            use crate::engine::api::manipulator::resolve_bone;
            let id = entity_id(&this)?;
            let other = entity_id(&other)?;
            let Some(bone) = resolve_bone(lua, id, &bone)? else { return Ok(()) };
            let other_bone = match other_bone.into_inner() {
                LuaValue::Integer(i) if i < 0 => None,
                LuaValue::Number(n) if n < 0.0 => None,
                b => resolve_bone(lua, other, &b)?,
            };
            with_entity_mut(lua, &this, |e| {
                e.cold.bone_links.retain(|&(b, _, _)| b != bone);
                e.cold.bone_links.push((bone, other, other_bone));
            })?;
            crate::engine::snapshot::note_hidden_bones(lua, id);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// Shows a bone hidden by `HideBone` (and its children if `children`).
        /// Moho has it on units only; ours takes any entity.
        ShowBone = |lua, (this, bone, children): (Obj<moho::Entity>, shapes::Bone, Option<bool>)| {
            let id = entity_id(&this)?;
            let Some(index) = crate::engine::api::manipulator::resolve_bone(lua, id, &bone)? else { return Ok(()) };
            with_entity_mut(lua, &this, |e| {
                for b in e.bone_set(index, children.unwrap_or(false)) {
                    e.cold.hidden_bones.remove(&b);
                }
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// Sets the orientation (a quaternion).
        SetOrientation = |lua, (this, q, _immediate): (Obj<moho::Entity>, shapes::Quaternion, Option<bool>)| {
            let q = read_quat(&q)?;
            with_entity_mut(lua, &this, |e| e.orientation = q)
        })?;
    lua_method!(lua, moho::Entity,
        /// The orientation as a quaternion.
        GetOrientation = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            let q = world(lua).get(id).map_or(FxQuat::IDENTITY, |e| e.orientation);
            quaternion(lua, q)
        })?;
    lua_method!(lua, moho::Entity,
        /// The entity heading (yaw).
        GetHeading = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            Ok(world(lua).get(id).map_or(0.0, |e| crate::engine::sim::movement::facing(e).to_num::<f64>()))
        })?;
    lua_method!(lua, moho::Entity,
        /// The position (of `bone`, if given) as a vector.
        GetPosition = |lua, (this, bone): (Obj<moho::Entity>, Option<shapes::Bone>)| {
            let pos = bone_position(lua, &this, bone.map(shapes::Bone::into_inner))?;
            vector(lua, pos)
        })?;
    lua_method!(lua, moho::Entity,
        /// Attaches this entity's `selfBone` to `parentBone` of `parent`.
        AttachBoneTo = |lua, (this, self_bone, parent, parent_bone): (Obj<moho::Entity>, shapes::Bone, Obj<moho::Entity>, shapes::Bone)| {
            use crate::engine::api::manipulator::resolve_bone;
            let child = entity_id(&this)?;
            let parent = entity_id(&parent)?;
            let attachment = crate::engine::sim::entity::Attachment {
                parent,
                parent_bone: resolve_bone(lua, parent, &parent_bone)?,
                self_bone: resolve_bone(lua, child, &self_bone)?,
            };
            attach_posed(lua, child, attachment);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// `AttachBoneTo` with this entity's origin as its attach point.
        AttachTo = |lua, (this, parent, parent_bone): (Obj<moho::Entity>, Obj<moho::Entity>, shapes::Bone)| {
            let child = entity_id(&this)?;
            let parent = entity_id(&parent)?;
            let attachment = crate::engine::sim::entity::Attachment {
                parent,
                parent_bone: crate::engine::api::manipulator::resolve_bone(lua, parent, &parent_bone)?,
                self_bone: None,
            };
            attach_posed(lua, child, attachment);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// `kind` is `None`, `Sphere` (centre, radius `sx`) or `Box` (centre, half extents `sx`, `sy`, `sz`).
        SetCollisionShape = |lua, (this, kind, cx, cy, cz, sx, sy, sz): (Obj<moho::Entity>, String, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>)| {
            use crate::engine::sim::entity::CollisionShape;
            let fx = |v: Option<f64>| crate::engine::sim::math::from_lua(v.unwrap_or(0.0));
            let center = FxVec3 { x: fx(cx), y: fx(cy), z: fx(cz) };
            // Moho reads the kind as a reflected enum, ignoring case
            // (`gpg::REnumType::GetEnumValue`: `_stricmp`): LOUD passes 'none'.
            let shape = match kind.to_ascii_lowercase().as_str() {
                "none" => CollisionShape::None,
                "sphere" => CollisionShape::Sphere { center, radius: fx(sx) },
                "box" => CollisionShape::Box { center, half: FxVec3 { x: fx(sx), y: fx(sy), z: fx(sz) } },
                _ => return Err(LuaError::runtime(format!("SetCollisionShape: unknown shape '{kind}'"))),
            };
            let id = entity_id(&this)?;
            world(lua).set_collision_override(id, Some(shape));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The rotation that turns +Z to face `dir` (`OrientFromDir`).
        OrientFromDir = |lua, v: shapes::Vector| {
            let num = |i: usize| -> LuaResult<f64> { Ok(v.raw_get::<Option<f64>>(i)?.unwrap_or(0.0)) };
            quaternion(lua, FxQuat::look_along(FxVec3::from_f64(num(1)?, num(2)?, num(3)?)))
        })?;
    lua_method!(lua, moho::Entity,
        /// Back to the blueprint's collision volume (FAF's upgrades turn the new
        /// structure's collision off while it's built, then call this). Moho
        /// has it on units only; ours takes any entity.
        RevertCollisionShape = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            world(lua).set_collision_override(id, None);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// Swaps the mesh and with it the skeleton bones resolve against.
        /// TODO: without `keepActor`, moho also drops the entity's manipulators.
        SetMesh = |lua, (this, mesh, _keep_actor): (Obj<moho::Entity>, String, Option<bool>)| {
            let skeleton = crate::engine::api::mesh::skeleton_for_mesh(lua, &mesh)?;
            let render_mesh = if mesh.is_empty() { None } else { Some(crate::engine::api::mesh::render_index(lua, &mesh)?) };
            with_entity_mut(lua, &this, |e| {
                e.cold.skeleton = skeleton;
                e.cold.mesh_override = Some(mesh.to_lowercase());
                e.cold.render_mesh = render_mesh;
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// Shifts an attached entity in its parent's frame (shield bubbles sit
        /// `ShieldVerticalOffset` above their generator this way).
        SetParentOffset = |lua, (this, v): (Obj<moho::Entity>, shapes::Vector)| {
            let num = |i: usize| -> LuaResult<f64> { Ok(v.raw_get::<Option<f64>>(i)?.unwrap_or(0.0)) };
            let offset = FxVec3::from_f64(num(1)?, num(2)?, num(3)?);
            world(lua).set_parent_offset(entity_id(&this)?, offset);
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// Uniform mesh draw scale (render only).
        SetDrawScale = |lua, (this, size): (Obj<moho::Entity>, f64)| {
            let scale = FxVec3::from_f64(size, size, size);
            with_entity_mut(lua, &this, |e| e.cold.draw_scale = scale)
        })?;
    lua_method!(lua, moho::Entity,
        /// `SetScale(s)` or `SetScale(sx, sy, sz)`.
        SetScale = |lua, (this, sx, sy, sz): (Obj<moho::Entity>, f64, Option<f64>, Option<f64>)| {
            let scale = FxVec3::from_f64(sx, sy.unwrap_or(sx), sz.unwrap_or(sx));
            with_entity_mut(lua, &this, |e| e.cold.draw_scale = scale)
        })?;
    lua_method!(lua, moho::Entity,
        /// Destroys the entity: `BeenDestroyed` is true from here on, and its
        /// `OnDestroy` runs at the end of the beat (moho's destroy queue); a
        /// second call is a no-op.
        Destroy = |lua, this: Obj<moho::Entity>| destroy(lua, &this))?;
    lua_method!(lua, moho::Entity,
        /// The position (of `bone`, if given) as three numbers.
        GetPositionXYZ = |lua, (this, bone): (Obj<moho::Entity>, Option<shapes::Bone>)| {
            Ok(bone_position(lua, &this, bone.map(shapes::Bone::into_inner))?.to_f64())
        })?;
    lua_method!(lua, moho::Entity,
        /// How built the entity is, 0..1.
        GetFractionComplete = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            Ok(world(lua).get(id).map_or(0.0, |e| e.fraction_complete.to_num::<f64>()))
        })?;
    lua_method!(lua, moho::Entity,
        /// Maximum health.
        GetMaxHealth = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            Ok(world(lua).get(id).map_or(0.0, |e| e.max_health.to_num::<f64>()))
        })?;
    lua_method!(lua, moho::Entity,
        /// The number of bones in the skeleton (0 without one).
        GetBoneCount = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            Ok(world(lua).get(id).and_then(|e| e.cold.skeleton.as_ref()).map_or(0, |s| s.bones.len()))
        })?;
    lua_method!(lua, moho::Entity,
        /// Falling trees: a `moho.MotorFallDown` to `Whack`. The fall itself is
        /// presentation only (not simulated); the tree stays where it is until
        /// FAF's script destroys it.
        FallDown = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            Ok(Obj::<moho::MotorFallDown>::new(script_object::create(lua, EntityHandle(id), moho::class_table(lua, "MotorFallDown")?)?))
        })?;
    lua_method!(lua, moho::MotorFallDown,
        /// Does nothing: the fall is presentation only.
        #[lua(partial = "no visual push yet")]
        Whack = |_, _args: LuaMultiValue| Ok(()))?;
    lua_method!(lua, moho::Entity,
        /// Sinking into the ground is presentation only too (FAF destroys the
        /// prop shortly after).
        #[lua(partial = "no sinking animation yet")]
        SinkAway = |_, _args: LuaMultiValue| Ok(()))?;
    lua_method!(lua, moho::Entity,
        /// The entity this one is attached to, else itself (FAF compares
        /// `unit:GetParent() ~= unit` to find attached units).
        GetParent = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            let parent = world(lua).get(id).and_then(|e| e.attachment.as_ref().map(|a| a.parent));
            match parent {
                Some(p) => Ok(object(lua, p)?.map_or(this, Obj::new)),
                None => Ok(this),
            }
        })?;
    lua_method!(lua, moho::Entity,
        /// Bone name by index (0 = root); nil when out of range.
        GetBoneName = |lua, (this, bone): (Obj<moho::Entity>, i64)| {
            let id = entity_id(&this)?;
            Ok(usize::try_from(bone)
                .ok()
                .and_then(|b| world(lua).get(id).and_then(|e| e.cold.skeleton.as_ref()).and_then(|s| s.bones.get(b).map(|b| b.name.clone()))))
        })?;
    lua_method!(lua, moho::Entity,
        /// A prop at `bone`, oriented as the entity times the bone's rest
        /// rotation (tree groups split into single trees this way).
        CreatePropAtBone = |lua, (this, bone, bp): (Obj<moho::Entity>, shapes::Bone, String)| {
            let id = entity_id(&this)?;
            let index = crate::engine::api::manipulator::resolve_bone(lua, id, &bone)?;
            let (pos, rot) = {
                let w = world(lua);
                let e = w.get(id).ok_or_else(|| LuaError::runtime("CreatePropAtBone: entity is gone"))?;
                posed_bone(lua, e, index)
            };
            spawn_prop(lua, &bp, pos, rot)
        })?;
    lua_fn!(lua, global,
        /// `SplitProp(original, blueprint)`: one `blueprint` prop at each of the
        /// original's bones but the root, in bone order; the original is
        /// destroyed. Returns the new props. Modelled on FAF's
        /// `Prop:SplitOnBonesByName`, its scripted sibling (inferred; tree groups
        /// with `SingleTreeBlueprint` call it).
        SplitProp = |lua, (original, bp): (Obj<moho::Prop>, String)| {
            let id = entity_id(&original)?;
            let posed: Vec<(FxVec3, FxQuat)> = {
                let w = world(lua);
                let Some(e) = w.get(id).filter(|e| !e.destroyed) else { return Ok(shapes::List::new(lua.create_table()?)) };
                let bones = e.cold.skeleton.as_ref().map_or(0, |s| s.bones.len());
                (1..bones).map(|i| posed_bone(lua, e, Some(i as u32))).collect()
            };
            let props = lua.create_table_with_capacity(posed.len(), 0)?;
            for (i, (pos, rot)) in posed.into_iter().enumerate() {
                props.raw_set(i + 1, spawn_prop(lua, &bp, pos, rot)?)?;
            }
            // Through the script, as FAF's `SplitOnBonesByName` does, so a
            // class's `Destroy` override runs.
            original.call_method::<()>("Destroy", ())?;
            Ok(shapes::List::<Obj<moho::Prop>>::new(props))
        })?;
    lua_method!(lua, moho::Prop,
        /// Puts the prop in moho's bounded list (`sim::bounded_props`): with
        /// 1000 props in it, the front one (lowest priority, then oldest) is
        /// destroyed first. The base game's and LOUD's wrecks use it.
        AddBoundedProp = |lua, (this, priority): (Obj<moho::Prop>, f64)| {
            use crate::engine::sim::bounded_props::{priority_of, BoundedProps};
            let id = entity_id(&this)?;
            if !world(lua).get(id).is_some_and(|e| !e.destroyed) {
                return Ok(());
            }
            loop {
                let front = {
                    let Some(mut b) = lua.app_data_mut::<BoundedProps>() else { return Ok(()) };
                    if !b.is_full() {
                        break;
                    }
                    b.pop_front()
                };
                let Some(front) = front else { break };
                // Moho destroys the dropped prop itself, not through its
                // script's `Destroy`.
                if let Some(obj) = object(lua, front)? {
                    destroy(lua, &Obj::new(obj))?;
                }
            }
            let tick = lua.app_data_ref::<crate::engine::sim::scheduler::Scheduler>().map_or(0, |s| s.tick());
            if let Some(mut b) = lua.app_data_mut::<BoundedProps>() {
                b.push(id, priority_of(priority), tick as i32);
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Entity,
        /// The posed bone's forward (+Z) axis in world space, as three numbers.
        GetBoneDirection = |lua, (this, bone): (Obj<moho::Entity>, shapes::Bone)| {
            let id = entity_id(&this)?;
            let index = crate::engine::api::manipulator::resolve_bone(lua, id, &bone)?;
            let dir = {
                let w = world(lua);
                let Some(e) = w.get(id) else { return Ok((0.0, 0.0, 1.0)) };
                posed_bone(lua, e, index).1.rotate(FxVec3 { z: Fx::ONE, ..FxVec3::default() })
            };
            Ok(dir.to_f64())
        })?;
    /// `GetCollisionExtents`' result.
    #[derive(crate::lua_api::LuaTable)]
    #[lua(name = "CollisionExtents", rename_all = "PascalCase", write_only)]
    #[allow(dead_code)]
    struct CollisionExtents {
        min: shapes::Vector,
        max: shapes::Vector,
    }
    lua_method!(lua, moho::Entity,
        /// World-space box around the collision volume: `{ Min = v, Max = v }`
        /// (a turned box's corners all inside). No volume (`SetCollisionShape
        /// 'None'`): both at the entity's position (inferred).
        GetCollisionExtents = |lua, this: Obj<moho::Entity>| -> LuaResult<Option<shapes::Shaped<CollisionExtents>>> {
            use crate::engine::sim::entity::CollisionShape;
            let id = entity_id(&this)?;
            let (min, max) = {
                let w = world(lua);
                let Some(e) = w.get(id) else { return Ok(None) };
                let pos = e.position();
                match e.collision_shape() {
                    CollisionShape::None => (pos, pos),
                    CollisionShape::Sphere { center, radius } => {
                        let c = pos + e.orientation.rotate(center);
                        let r = FxVec3 { x: radius, y: radius, z: radius };
                        (c - r, c + r)
                    }
                    CollisionShape::Box { center, half } => {
                        let c = pos + e.orientation.rotate(center);
                        let axis = |v: FxVec3| e.orientation.rotate(v);
                        let (ax, ay, az) = (axis(FxVec3 { x: half.x, ..FxVec3::default() }), axis(FxVec3 { y: half.y, ..FxVec3::default() }), axis(FxVec3 { z: half.z, ..FxVec3::default() }));
                        let reach = FxVec3 {
                            x: ax.x.abs() + ay.x.abs() + az.x.abs(),
                            y: ax.y.abs() + ay.y.abs() + az.y.abs(),
                            z: ax.z.abs() + ay.z.abs() + az.z.abs(),
                        };
                        (c - reach, c + reach)
                    }
                }
            };
            let t = lua.create_table_with_capacity(0, 2)?;
            t.raw_set("Min", vector(lua, min)?)?;
            t.raw_set("Max", vector(lua, max)?)?;
            Ok(Some(shapes::Shaped::new(t)))
        })?;
    lua_method!(lua, moho::Entity,
        /// Current health.
        GetHealth = |lua, this: Obj<moho::Entity>| {
            let id = entity_id(&this)?;
            Ok(world(lua).get(id).map_or(0.0, |e| e.health.to_num::<f64>()))
        })?;
    lua_method!(lua, moho::Entity,
        /// Sets the maximum health and nothing else: health is left as it is,
        /// even above the new maximum, as moho's (`Entity::SetMaxHealth` only
        /// stores it; the next `AdjustHealth` / `SetHealth` clamps). FAF's
        /// buffs lean on that: a veterancy level swaps its `DoNotFill` max
        /// health buff (down to the base, then up) before its heal, and a
        /// clamp there would lose the health above the base.
        SetMaxHealth = |lua, (this, max): (Obj<moho::Entity>, f64)| {
            with_entity_mut(lua, &this, |e| {
                e.max_health = crate::engine::sim::math::from_lua(max);
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// Sets health, clamped to [0, max]. The instigator is ignored.
        SetHealth = |lua, (this, _instigator, health): (Obj<moho::Entity>, shapes::Instigator, f64)| {
            with_entity_mut(lua, &this, |e| {
                e.health = crate::engine::sim::math::from_lua(health).clamp(Fx::ZERO, e.max_health);
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// Health += delta, clamped to [0, max]. Kill credit (`instigator`) is a
        /// TODO for veterancy.
        AdjustHealth = |lua, (this, _instigator, delta): (Obj<moho::Entity>, shapes::Instigator, f64)| {
            with_entity_mut(lua, &this, |e| {
                e.health = e.health.saturating_add(crate::engine::sim::math::from_lua(delta)).clamp(Fx::ZERO, e.max_health);
            })
        })?;
    lua_method!(lua, moho::Entity,
        /// Kills the entity once: health drops to 0 and `OnKilled(instigator,
        /// type, excessDamageRatio)` runs; FAF's script then plays the death and
        /// calls `Destroy`. Later calls are no-ops. A `Destroy` earlier in the
        /// beat doesn't stop it (moho's `Unit::Kill`, 0x102878b0, only tests
        /// the dead flag).
        Kill = |lua, (this, instigator, kind, excess): (Obj<moho::Entity>, shapes::Instigator, Option<shapes::DamageType>, Option<f64>)| {
            let first = with_entity_mut(lua, &this, |e| {
                let first = !e.cold.killed && !e.gone;
                if first {
                    e.cold.killed = true;
                    e.health = Fx::ZERO;
                }
                first
            })?;
            if !first {
                return Ok(());
            }
            // Lost / killed stats; credit goes to the instigator's army.
            let killer = match &*instigator {
                LuaValue::Table(t) => script_object::handle::<EntityHandle>(t).ok().map(|h| h.0),
                _ => None,
            };
            crate::engine::api::unit_stats::unit_killed(lua, entity_id(&this)?, killer)?;
            crate::engine::api::movement::start_falling(lua, entity_id(&this)?);
            let kind = kind.map(shapes::DamageType::into_inner).unwrap_or_else(|| LuaValue::String(lua.create_string("Normal").expect("string")));
            // Then the attachments hear of it (moho's `Entity::Kill`,
            // 0x1025e3b0, for every kind of entity): `parent:OnAttachedKilled(self)`,
            // then `child:OnParentKilled(self)` for each child.
            let id = entity_id(&this)?;
            let (parent, children) = {
                let w = world(lua);
                (w.get(id).and_then(|e| e.attachment.map(|a| a.parent)), w.attached_to(id))
            };
            if let Some(parent) = parent.map(|p| object(lua, p)).transpose()?.flatten() {
                EntityCallbacks::on_attached_killed(lua, &parent, "entity", Obj::new(this.table().clone()))?;
            }
            for child in children {
                if let Some(child) = object(lua, child)? {
                    EntityCallbacks::on_parent_killed(lua, &child, "entity", Obj::new(this.table().clone()))?;
                }
            }
            EntityCallbacks::on_killed(lua, &this, "entity", instigator, shapes::DamageType(kind), excess.unwrap_or(0.0))
        })?;

    lua_fn!(lua, global,
        /// Moves `obj` to `pos`, and turns it to `orient` (a quaternion) if given.
        Warp = |lua, (obj, pos, orient): (Obj<moho::Entity>, shapes::Vector, Option<shapes::Quaternion>)| {
            let id = entity_id(&obj)?;
            let p = read_vector(&pos)?;
            let q = orient.as_deref().map(read_quat).transpose()?;
            let mut w = world(lua);
            match q {
                Some(q) => w.set_transform(id, p, q),
                None => w.set_position(id, p),
            }
            Ok(())
        })?;
    Ok(())
}
