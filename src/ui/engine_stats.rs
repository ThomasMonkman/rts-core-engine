//! `__EngineStats`, moho's engine statistics tree in the interface: nodes
//! `{Name, Type, Value, Children}` under `{Name = 'Root', Children}`, which
//! FAF and mods look up by name. The tree is made in `user_api.rs` with only
//! the nodes Lua reads (listed in `docs/architecture/ui.md`); this
//! module keeps their values up. Each node's table is found once and kept
//! (FAF code holds on to nodes), and a value is written only when it
//! changed, so a frame costs a few comparisons and at most a few table
//! writes.

use mlua::prelude::*;

/// A value node of the tree.
#[derive(Clone, Copy, Debug)]
pub(super) enum Stat {
    Fps,
    EntityUnit,
    EntityProjectile,
    EntityProp,
    EntityShield,
    EntityOther,
    CursorElevation,
    CursorPosition,
    HeapCommitted,
    HeapTotal,
    HeapInUse,
    InstanceEntity,
    InstanceProp,
    InstanceUnit,
    InstancePlatoon,
    InstanceDecal,
    InstanceLuaTask,
}

impl Stat {
    const ALL: [Stat; 17] = [
        Stat::Fps,
        Stat::EntityUnit,
        Stat::EntityProjectile,
        Stat::EntityProp,
        Stat::EntityShield,
        Stat::EntityOther,
        Stat::CursorElevation,
        Stat::CursorPosition,
        Stat::HeapCommitted,
        Stat::HeapTotal,
        Stat::HeapInUse,
        Stat::InstanceEntity,
        Stat::InstanceProp,
        Stat::InstanceUnit,
        Stat::InstancePlatoon,
        Stat::InstanceDecal,
        Stat::InstanceLuaTask,
    ];

    /// The node's names from the root's children down.
    fn path(self) -> &'static [&'static str] {
        match self {
            Stat::Fps => &["Frame", "FPS"],
            Stat::EntityUnit => &["EntityCount", "Unit"],
            Stat::EntityProjectile => &["EntityCount", "Projectile"],
            Stat::EntityProp => &["EntityCount", "Prop"],
            Stat::EntityShield => &["EntityCount", "Shield"],
            Stat::EntityOther => &["EntityCount", "Other"],
            Stat::CursorElevation => &["Camera", "Cursor", "Elevation"],
            Stat::CursorPosition => &["Camera", "Cursor", "Position"],
            Stat::HeapCommitted => &["Heap", "Committed"],
            Stat::HeapTotal => &["Heap", "Total"],
            Stat::HeapInUse => &["Heap", "InUse"],
            Stat::InstanceEntity => &["Instance Counts", "class Moho::Entity"],
            Stat::InstanceProp => &["Instance Counts", "class Moho::Prop"],
            Stat::InstanceUnit => &["Instance Counts", "class Moho::Unit"],
            Stat::InstancePlatoon => &["Instance Counts", "class Moho::CPlatoon"],
            Stat::InstanceDecal => &["Instance Counts", "class Moho::CDecalHandle"],
            Stat::InstanceLuaTask => &["Instance Counts", "class Moho::CLuaTask"],
        }
    }
}

/// The value nodes' tables (by [`Stat`]) and the values last written.
struct Nodes {
    tables: Vec<Option<LuaTable>>,
    last: Vec<Option<f64>>,
    /// What the cursor's point was last found from, and that point.
    cursor: Option<(CursorKey, glam::Vec3)>,
    /// The sim's live Lua threads, from the latest snapshot.
    sim_threads: u32,
}

/// The mouse (interface coordinates), the camera and the terrain a cursor
/// point was found from: while they stay put, so does the point.
#[derive(PartialEq)]
struct CursorKey {
    mouse: (f32, f32),
    view_proj: glam::Mat4,
    terrain: usize,
}

/// The child of `node` named `name`.
fn child(node: &LuaTable, name: &str) -> LuaResult<Option<LuaTable>> {
    let Some(children) = node.raw_get::<Option<LuaTable>>("Children")? else { return Ok(None) };
    for c in children.sequence_values::<LuaTable>() {
        let c = c?;
        if c.raw_get::<Option<LuaString>>("Name")?.is_some_and(|n| n.as_bytes() == name.as_bytes()) {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

fn find(lua: &Lua) -> LuaResult<Nodes> {
    let root = lua.globals().raw_get::<Option<LuaTable>>("__EngineStats")?;
    let mut tables = Vec::with_capacity(Stat::ALL.len());
    for stat in Stat::ALL {
        let mut node = root.clone();
        for name in stat.path() {
            node = match node {
                Some(n) => child(&n, name)?,
                None => None,
            };
        }
        tables.push(node);
    }
    Ok(Nodes { tables, last: vec![None; Stat::ALL.len()], cursor: None, sim_threads: 0 })
}

/// Makes the stats tree and `LaunchDir` (the globals moho fills in,
/// `/engine/Library.lua`), unless something already has.
pub(super) fn register(lua: &Lua) -> LuaResult<()> {
    crate::lua_api::script::exec(
        lua,
        r#"
        -- Engine-populated globals (/engine/Library.lua). `__EngineStats` is
        -- moho's stats tree (`{Name, Type, Value, Children}` nodes), with
        -- only the nodes FAF and mods read, kept up by the engine
        -- (`ui::engine_stats`): `Frame` / `FPS` (AI-Uveso's interface hook,
        -- FAF's debug FPS panel), `EntityCount` (FAF's performance
        -- tracking samples `Unit`), `Camera` / `Cursor` (FAF's
        -- `GetMouseTerrainElevation`: the water depth indicator, the
        -- context-based build templates), `Heap` (FAF's session logger, AI-Uveso),
        -- `Instance Counts` (AI-Uveso's hook; only the classes ours has).
        -- The cursor's are 0 until the mouse is first over a world view.
        local function stat(name, type) return { Name = name, Type = type, Value = 0 } end
        rawset(_G, '__EngineStats', rawget(_G, '__EngineStats') or {
            Name = 'Root', Children = {
                { Name = 'Frame', Children = { stat('FPS', 'Float') } },
                { Name = 'EntityCount', Children = { stat('Prop', 'Integer'), stat('Unit', 'Integer'), stat('Projectile', 'Integer'), stat('Shield', 'Integer'), stat('Other', 'Integer') } },
                { Name = 'Instance Counts', Children = {
                    stat('class Moho::Entity', 'Integer'), stat('class Moho::Prop', 'Integer'), stat('class Moho::Unit', 'Integer'),
                    stat('class Moho::CPlatoon', 'Integer'), stat('class Moho::CDecalHandle', 'Integer'), stat('class Moho::CLuaTask', 'Integer'),
                } },
                { Name = 'Heap', Children = { { Name = 'Committed', Type = 'String', Value = '0.000M' }, { Name = 'Total', Type = 'String', Value = '0.000M' }, { Name = 'InUse', Type = 'String', Value = '0.000M' } } },
                { Name = 'Camera', Children = { { Name = 'Cursor', Children = { { Name = 'Position', Type = 'String', Value = 'x=0.00,y=0.00,z=0.00' }, stat('Elevation', 'Float') } } } },
            },
        })
        rawset(_G, 'LaunchDir', rawget(_G, 'LaunchDir') or '')
        "#,
        "=[engine stats]",
    )
}

fn nodes(lua: &Lua) -> LuaResult<mlua::AppDataRefMut<'_, Nodes>> {
    if lua.app_data_ref::<Nodes>().is_none() {
        let nodes = find(lua)?;
        lua.set_app_data(nodes);
    }
    Ok(lua.app_data_mut::<Nodes>().expect("just set"))
}

/// Sets `stat`'s value to `v` if that changes it.
pub(super) fn set(lua: &Lua, stat: Stat, v: f64) -> LuaResult<()> {
    let mut nodes = nodes(lua)?;
    let i = stat as usize;
    if nodes.last[i] == Some(v) {
        return Ok(());
    }
    nodes.last[i] = Some(v);
    match &nodes.tables[i] {
        Some(t) => t.raw_set("Value", v),
        None => Ok(()),
    }
}

/// Sets `stat`'s value to the string `value()` makes if `key` (what the
/// string is made from) changed.
fn set_string(lua: &Lua, stat: Stat, key: f64, value: impl FnOnce() -> String) -> LuaResult<()> {
    let mut nodes = nodes(lua)?;
    let i = stat as usize;
    if nodes.last[i] == Some(key) {
        return Ok(());
    }
    nodes.last[i] = Some(key);
    match &nodes.tables[i] {
        Some(t) => t.raw_set("Value", value()),
        None => Ok(()),
    }
}

/// Bytes as moho writes its heap figures: megabytes to four significant
/// figures, `M` after (`512.0M`, `315.3M`, `4.503M`: FAF probe
/// `probe_engine_stats`).
fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    match mb {
        m if m < 10.0 => format!("{m:.3}M"),
        m if m < 100.0 => format!("{m:.2}M"),
        m if m < 1000.0 => format!("{m:.1}M"),
        m => format!("{m:.0}M"),
    }
}

/// `Heap` / `Committed`, `Total` (FAF's session logger) and `InUse`
/// (AI-Uveso's hook: its report's `%s` raises without it or `Reserved`):
/// moho's own heap figures; ours are all the process's resident memory
/// (the status bar's RAM figure), the nearest true number we have.
/// `Reserved` (address space) is left out.
/// Nothing is written while the OS doesn't report it.
pub(super) fn update_heap(lua: &Lua) -> LuaResult<()> {
    let Some(bytes) = memory_stats::memory_stats().map(|m| m.physical_mem as u64) else { return Ok(()) };
    set_string(lua, Stat::HeapCommitted, bytes as f64, || megabytes(bytes))?;
    set_string(lua, Stat::HeapTotal, bytes as f64, || megabytes(bytes))?;
    set_string(lua, Stat::HeapInUse, bytes as f64, || megabytes(bytes))
}

/// The sim's counts (`EntityCount`, `Instance Counts`), from the latest
/// snapshot. `Instance Counts` has only classes ours has the same objects
/// for; moho's `Entity` also counts its recon blips, which aren't entities
/// here.
pub(super) fn set_counts(lua: &Lua, c: &crate::engine::snapshot::EngineCounts) -> LuaResult<()> {
    set(lua, Stat::EntityUnit, c.units.into())?;
    set(lua, Stat::EntityProjectile, c.projectiles.into())?;
    set(lua, Stat::EntityProp, c.props.into())?;
    set(lua, Stat::EntityShield, c.shields.into())?;
    set(lua, Stat::EntityOther, c.other.into())?;
    let entities = c.units + c.projectiles + c.props + c.shields + c.other;
    set(lua, Stat::InstanceEntity, entities.into())?;
    set(lua, Stat::InstanceProp, c.props.into())?;
    set(lua, Stat::InstanceUnit, c.units.into())?;
    set(lua, Stat::InstancePlatoon, c.platoons.into())?;
    set(lua, Stat::InstanceDecal, c.decals.into())?;
    nodes(lua)?.sim_threads = c.lua_threads;
    update_lua_tasks(lua)
}

/// `Instance Counts` / `CLuaTask`: moho counts the instances of a class
/// across the process, so both VMs' live Lua threads, the sim's (from the
/// snapshot) and the interface's.
pub(super) fn update_lua_tasks(lua: &Lua) -> LuaResult<()> {
    let user = lua.app_data_ref::<crate::engine::sim::scheduler::Scheduler>().map_or(0, |s| s.task_count() as u32);
    let sim = nodes(lua)?.sim_threads;
    set(lua, Stat::InstanceLuaTask, (sim + user).into())
}

/// `Camera` / `Cursor`: where the ray through the mouse meets the terrain
/// or the water above it (`Position`, `x=..,y=..,z=..`) and the terrain
/// height there (`Elevation`: the sea floor under water), through the
/// camera of the world view under the mouse; kept from the last time the
/// mouse was over one. FAF's `GetMouseTerrainElevation` reads `Elevation`
/// for its water depth indicator (`/lua/ui/game/cursor/depth.lua`) and
/// context-based build templates (land or water under the mouse).
pub(super) fn update_cursor(lua: &Lua) -> LuaResult<()> {
    let g = lua.globals();
    let mouse = (g.raw_get::<Option<f32>>("__engine_mouse_x")?.unwrap_or(0.0), g.raw_get::<Option<f32>>("__engine_mouse_y")?.unwrap_or(0.0));
    // (Found first: no other app data may be borrowed while it's set.)
    drop(nodes(lua)?);
    let (key, hit) = {
        let Some(views) = lua.app_data_ref::<super::world_pick::PickViews>() else { return Ok(()) };
        let Some(view) = views.under(mouse.0, mouse.1) else { return Ok(()) };
        let key = CursorKey { mouse, view_proj: view.view_proj, terrain: view.terrain.as_ref().map_or(0, |t| std::sync::Arc::as_ptr(t) as usize) };
        if nodes(lua)?.cursor.as_ref().is_some_and(|(k, _)| *k == key) {
            return Ok(());
        }
        (key, view.cursor(mouse.0, mouse.1))
    };
    let Some((p, ground)) = hit else { return Ok(()) };
    set(lua, Stat::CursorElevation, ground.into())?;
    let mut nodes = nodes(lua)?;
    let moved = nodes.cursor.as_ref().is_none_or(|(_, q)| *q != p);
    nodes.cursor = Some((key, p));
    match &nodes.tables[Stat::CursorPosition as usize] {
        Some(t) if moved => t.raw_set("Value", format!("x={:.2},y={:.2},z={:.2}", p.x, p.y, p.z)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn megabytes_read_as_moho_writes_them() {
        let mb = |m: f64| super::megabytes((m * 1024.0 * 1024.0) as u64);
        assert_eq!(mb(512.0), "512.0M");
        assert_eq!(mb(315.3), "315.3M");
        assert_eq!(mb(18.21), "18.21M");
        assert_eq!(mb(4.503), "4.503M");
        assert_eq!(mb(2048.0), "2048M");
    }
}
