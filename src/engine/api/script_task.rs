//! Script tasks: `IssueScript(units, { TaskName = 'EnhanceTask', ... })`.
//!
//! When a unit's current order is a script order, the engine instantiates
//! `import('/lua/sim/tasks/<TaskName>.lua')[TaskName]` (FAF classes derived
//! from `moho.ScriptTask_Methods`), calls `OnCreate(commandData)` with the
//! order's table (shared by every unit of the order), then `TaskTick()` as
//! the returned status asks: `Done` (-1) / `Abort` (-3) end the task
//! (`OnDestroy`) and its order; `Repeat` (0) runs it again this tick (up to
//! a few times, so a state change takes effect at once); a positive `n`
//! waits `n` ticks; `Suspend` (-2) / `Delay` (-4) wait a tick (inferred). A
//! cleared or replaced order destroys the task. Tasks run in unit id order.
//!
//! Also here: the unit's navigator object (`GetNavigator`), `IsMobile`,
//! `IsMoving`.

use crate::lua_api::shapes;
use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::entity::{entity_id, world, EntityHandle};
use crate::engine::api::callbacks::ScriptTaskCallbacks;
use crate::engine::api::{moho, script_object};
use crate::lua_api::{Obj, lua_method};
use crate::engine::sim::command::{CommandId, CommandKind, Commands};
use crate::engine::sim::entity::EntityId;

#[derive(Clone, Copy, Debug)]
pub struct ScriptTaskHandle(pub EntityId);
impl LuaUserData for ScriptTaskHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct Task {
    command: CommandId,
    /// Next tick `TaskTick` runs.
    wake: u64,
}

/// Live tasks by unit (one per unit: its current order).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct ScriptTasks {
    by_unit: BTreeMap<EntityId, Task>,
    /// Command ids whose order table is kept in `ORDERS`.
    stored: Vec<CommandId>,
}

/// Registry tables: order tables by command id, task objects by unit id.
const ORDERS: &str = "script_orders";
const OBJECTS: &str = "script_task_objects";

const DONE: i64 = -1;
const ABORT: i64 = -3;
const REPEAT: i64 = 0;
/// `Repeat` re-runs within one tick at most this often.
const MAX_REPEATS: u32 = 4;

fn registry(lua: &Lua, name: &str) -> LuaResult<LuaTable> {
    lua.named_registry_value(name)
}

fn tick_now(lua: &Lua) -> u64 {
    lua.app_data_ref::<crate::engine::sim::scheduler::Scheduler>().map_or(0, |s| s.tick())
}

/// Keeps `order` for the script command `command` (`IssueScript`).
pub fn store_order(lua: &Lua, command: CommandId, order: LuaTable) -> LuaResult<()> {
    registry(lua, ORDERS)?.raw_set(command, order)?;
    lua.app_data_mut::<ScriptTasks>().expect("ScriptTasks").stored.push(command);
    Ok(())
}

fn current_script(lua: &Lua, unit: EntityId) -> Option<CommandId> {
    let c = lua.app_data_ref::<Commands>()?;
    c.current(unit).filter(|c| c.kind == CommandKind::Script).map(|c| c.id)
}

/// Ends `unit`'s task: `OnDestroy`, forget it.
fn destroy(lua: &Lua, unit: EntityId) -> LuaResult<()> {
    lua.app_data_mut::<ScriptTasks>().expect("ScriptTasks").by_unit.remove(&unit);
    let objects = registry(lua, OBJECTS)?;
    if let Some(obj) = objects.raw_get::<Option<LuaTable>>(unit)? {
        objects.raw_set(unit, LuaNil)?;
        ScriptTaskCallbacks::on_destroy(lua, &obj, "script task")?;
    }
    Ok(())
}

/// Pops `command` if it's still `unit`'s current order.
fn finish_order(lua: &Lua, unit: EntityId, command: CommandId) {
    if let Some(mut c) = lua.app_data_mut::<Commands>()
        && c.current(unit).is_some_and(|cmd| cmd.id == command) {
        c.pop_front(unit);
    }
}

/// Instantiates the task for `unit`'s script order `command`.
fn create(lua: &Lua, unit: EntityId, command: CommandId) -> LuaResult<()> {
    let order: Option<LuaTable> = registry(lua, ORDERS)?.raw_get(command)?;
    let name: Option<String> = match &order {
        Some(o) => o.get("TaskName")?,
        None => None,
    };
    let class = match &name {
        Some(n) => {
            let import: LuaFunction = lua.globals().get("import")?;
            match import.call::<LuaTable>(format!("/lua/sim/tasks/{n}.lua")) {
                Ok(m) => m.get::<Option<LuaTable>>(n.as_str())?,
                Err(e) => {
                    tracing::warn!(target: "lua", "script task '{n}' failed to load: {e}");
                    None
                }
            }
        }
        None => None,
    };
    let Some(class) = class else {
        tracing::warn!(target: "lua", task = ?name, "script order without a usable TaskName; dropped");
        finish_order(lua, unit, command);
        return Ok(());
    };
    let obj: LuaTable = class.call(())?;
    obj.raw_set(script_object::C_OBJECT, ScriptTaskHandle(unit))?;
    registry(lua, OBJECTS)?.raw_set(unit, obj.clone())?;
    lua.app_data_mut::<ScriptTasks>().expect("ScriptTasks").by_unit.insert(unit, Task { command, wake: tick_now(lua) });
    ScriptTaskCallbacks::on_create(lua, &obj, "script task", order)?;
    Ok(())
}

/// Runs one unit's due `TaskTick`s.
fn run(lua: &Lua, unit: EntityId, task: Task, now: u64) -> LuaResult<()> {
    if task.wake > now {
        return Ok(());
    }
    let Some(obj) = registry(lua, OBJECTS)?.raw_get::<Option<LuaTable>>(unit)? else { return Ok(()) };
    for _ in 0..MAX_REPEATS {
        let status = match obj.get::<Option<LuaFunction>>("TaskTick")? {
            Some(f) => match f.call::<Option<i64>>(obj.clone()) {
                Ok(s) => s.unwrap_or(DONE),
                Err(e) => {
                    tracing::warn!(target: "lua", "Error running TaskTick script in script task: {e}");
                    ABORT
                }
            },
            None => DONE,
        };
        match status {
            DONE | ABORT => {
                destroy(lua, unit)?;
                finish_order(lua, unit, task.command);
                return Ok(());
            }
            REPEAT => continue,
            n => {
                let wait = if n > 0 { n as u64 } else { 1 };
                if let Some(t) = lua.app_data_mut::<ScriptTasks>().expect("ScriptTasks").by_unit.get_mut(&unit) {
                    t.wake = now + wait;
                }
                return Ok(());
            }
        }
    }
    // Still repeating: carry on next tick.
    if let Some(t) = lua.app_data_mut::<ScriptTasks>().expect("ScriptTasks").by_unit.get_mut(&unit) {
        t.wake = now + 1;
    }
    Ok(())
}

pub fn tick(lua: &Lua) -> LuaResult<()> {
    let now = tick_now(lua);
    let mut units: Vec<EntityId> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(()) };
        c.units_doing(&[CommandKind::Script])
    };
    units.extend(lua.app_data_ref::<ScriptTasks>().map(|t| t.by_unit.keys().copied().collect::<Vec<_>>()).unwrap_or_default());
    units.sort_unstable();
    units.dedup();
    for unit in units {
        let alive = world(lua).get(unit).is_some_and(|e| !e.destroyed);
        let current = if alive { current_script(lua, unit) } else { None };
        let task = lua.app_data_ref::<ScriptTasks>().and_then(|t| t.by_unit.get(&unit).copied());
        match (task, current) {
            (Some(t), Some(c)) if t.command == c => run(lua, unit, t, now)?,
            (Some(_), _) => {
                destroy(lua, unit)?;
                if let Some(c) = current.filter(|_| !crate::engine::api::orders::held(&world(lua), unit)) {
                    create(lua, unit, c)?;
                }
            }
            // Moho's dispatch hold: no new task while held.
            (None, Some(c)) if !crate::engine::api::orders::held(&world(lua), unit) => {
                create(lua, unit, c)?;
                if let Some(t) = lua.app_data_ref::<ScriptTasks>().and_then(|t| t.by_unit.get(&unit).copied()) {
                    run(lua, unit, t, now)?;
                }
            }
            _ => {}
        }
    }
    // Drop order tables of finished script commands.
    let finished: Vec<CommandId> = {
        let (Some(c), Some(mut t)) = (lua.app_data_ref::<Commands>(), lua.app_data_mut::<ScriptTasks>()) else { return Ok(()) };
        let (done, keep): (Vec<_>, Vec<_>) = t.stored.iter().partition(|&&id| c.is_done(id));
        t.stored = keep;
        done
    };
    if !finished.is_empty() {
        let orders = registry(lua, ORDERS)?;
        for id in finished {
            orders.raw_set(id, LuaNil)?;
        }
    }
    Ok(())
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(ScriptTasks::default());
    lua.set_named_registry_value(ORDERS, lua.create_table()?)?;
    lua.set_named_registry_value(OBJECTS, lua.create_table()?)?;
    lua_method!(lua, moho::ScriptTask,
        /// The unit the task runs on.
        GetUnit = |lua, this: Obj<moho::ScriptTask>| {
        let ScriptTaskHandle(unit) = script_object::handle(&this)?;
        crate::engine::api::entity::object_as::<moho::Unit>(lua, unit)
    })?;
    lua_method!(lua, moho::ScriptTask,
        /// The AI reads results from FAF's own Lua; nothing native consumes it yet.
        SetAIResult = |_, (_this, _r): (Obj<moho::ScriptTask>, LuaValue)| Ok(()))?;

    lua_method!(lua, moho::Entity,
        /// Asks the UI to redraw the entity's panel (after enhancements, a
        /// remote viewer's cost): the panel reads the sim each frame anyway.
        RequestRefreshUI = |_, _this: Obj<moho::Entity>| Ok(()))?;
    lua_method!(lua, moho::Unit,
        /// Whether the unit has a motion type.
        IsMobile = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        Ok(world(lua).get(id).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_params.kind != crate::engine::sim::movement::MotionKind::None))
    })?;
    lua_method!(lua, moho::Unit,
        /// Whether the unit has speed.
        IsMoving = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        Ok(world(lua).get(id).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_state.speed != crate::engine::sim::math::Fx::ZERO))
    })?;
    // The navigator is the unit itself seen through `navigator_methods`.
    lua_method!(lua, moho::Unit,
        /// The unit's navigator: the unit itself seen through the navigator class.
        GetNavigator = |lua, this: Obj<moho::Unit>| {
        let id = entity_id(&this)?;
        script_object::create(lua, EntityHandle(id), moho::class_table(lua, "navigator_methods")?).map(Obj::<moho::Navigator>::new)
    })?;
    lua_method!(lua, moho::Navigator,
        /// Stops a move in progress: a move-type current order is dropped.
        AbortMove = |lua, this: Obj<moho::Navigator>| {
        let id = entity_id(&this)?;
        if let Some(mut c) = lua.app_data_mut::<Commands>() {
            use CommandKind as K;
            if c.current(id).is_some_and(|cmd| matches!(cmd.kind, K::Move | K::FormMove | K::AggressiveMove | K::FormAggressiveMove)) {
                c.pop_front(id);
            }
        }
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.navigator_goal = None;
            u.navigator_dest = None;
            u.path = None;
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Navigator,
        /// Sends the unit to `position`; followed while it has no order.
        /// Takes the position (moho's binding requires 2 arguments and reads a
        /// vector; FAF's annotation `SetGoal()` drops it): see
        /// `docs/research/verifying-against-the-binary.md`.
        SetGoal = |lua, (this, position): (Obj<moho::Navigator>, shapes::Vector)| {
        let id = entity_id(&this)?;
        let p = crate::engine::sim::math::FxVec3::from_f64(position.get::<f64>(1)?, position.get::<Option<f64>>(2)?.unwrap_or(0.0), position.get::<f64>(3)?);
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.navigator_goal = Some(p);
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Navigator,
        /// Points the navigator at another unit to chase (MohoEngine.dll
        /// 0x10195560). An attack order on it with `SetSpeedThroughGoal`
        /// closes to contact; otherwise kept only.
        #[lua(partial = "only changes an attack order on the same unit")]
        SetDestUnit = |lua, (this, target): (Obj<moho::Navigator>, Obj<moho::Unit>)| {
        let (id, target) = (entity_id(&this)?, entity_id(&target)?);
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.navigator_dest = Some(target);
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Navigator,
        /// Whether to keep going through the goal rather than stop at it (set
        /// for patrols and waypoint chains; MohoEngine.dll 0x10195930).
        #[lua(partial = "only changes an attack order on the dest unit (SetDestUnit)")]
        SetSpeedThroughGoal = |lua, (this, on): (Obj<moho::Navigator>, bool)| {
        let id = entity_id(&this)?;
        if let Some(u) = world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.speed_through_goal = on;
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Navigator,
        /// Where the unit is headed in the end: its order's goal, else its
        /// `SetGoal` point, else where it stands (MohoEngine.dll 0x10195be0
        /// always gives a vector; Uveso's ACU behaviour compares it with the
        /// unit's position).
        GetGoalPos = |lua, this: Obj<moho::Navigator>| {
        let id = entity_id(&this)?;
        let goal = {
            let commands = lua.app_data_ref::<Commands>();
            let w = world(lua);
            commands.and_then(|c| crate::engine::sim::movement::final_goal(&c, &w, id))
        };
        crate::engine::api::entity::vector(lua, goal.unwrap_or_default())
    })?;
    lua_method!(lua, moho::Navigator,
        /// Where the unit is steering now, or nil when it isn't going anywhere.
        GetCurrentTargetPos = |lua, this: Obj<moho::Navigator>| {
        let id = entity_id(&this)?;
        let target = {
            let commands = lua.app_data_ref::<Commands>();
            let w = world(lua);
            commands.and_then(|c| crate::engine::sim::movement::current_target(&c, &w, id))
        };
        match target {
            Some(p) => crate::engine::api::entity::vector(lua, p).map(Some),
            None => Ok(None),
        }
    })?;
    Ok(())
}
