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
}

impl Stat {
    const ALL: [Stat; 8] = [
        Stat::Fps,
        Stat::EntityUnit,
        Stat::EntityProjectile,
        Stat::EntityProp,
        Stat::EntityShield,
        Stat::EntityOther,
        Stat::CursorElevation,
        Stat::CursorPosition,
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
        }
    }
}

/// The value nodes' tables (by [`Stat`]) and the values last written.
struct Nodes {
    tables: Vec<Option<LuaTable>>,
    last: Vec<Option<f64>>,
    /// What the cursor's point was last found from, and that point.
    cursor: Option<(CursorKey, glam::Vec3)>,
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
    Ok(Nodes { tables, last: vec![None; Stat::ALL.len()], cursor: None })
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

/// The sim's entity counts (`EntityCount`), from the latest snapshot.
pub(super) fn set_counts(lua: &Lua, c: &crate::engine::snapshot::EngineCounts) -> LuaResult<()> {
    set(lua, Stat::EntityUnit, c.units.into())?;
    set(lua, Stat::EntityProjectile, c.projectiles.into())?;
    set(lua, Stat::EntityProp, c.props.into())?;
    set(lua, Stat::EntityShield, c.shields.into())?;
    set(lua, Stat::EntityOther, c.other.into())
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
