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
}

impl Stat {
    const ALL: [Stat; 6] = [Stat::Fps, Stat::EntityUnit, Stat::EntityProjectile, Stat::EntityProp, Stat::EntityShield, Stat::EntityOther];

    /// The node's names from the root's children down.
    fn path(self) -> &'static [&'static str] {
        match self {
            Stat::Fps => &["Frame", "FPS"],
            Stat::EntityUnit => &["EntityCount", "Unit"],
            Stat::EntityProjectile => &["EntityCount", "Projectile"],
            Stat::EntityProp => &["EntityCount", "Prop"],
            Stat::EntityShield => &["EntityCount", "Shield"],
            Stat::EntityOther => &["EntityCount", "Other"],
        }
    }
}

/// The value nodes' tables (by [`Stat`]) and the values last written.
struct Nodes {
    tables: Vec<Option<LuaTable>>,
    last: Vec<Option<f64>>,
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
    Ok(Nodes { tables, last: vec![None; Stat::ALL.len()] })
}

/// Sets `stat`'s value to `v` if that changes it.
pub(super) fn set(lua: &Lua, stat: Stat, v: f64) -> LuaResult<()> {
    if lua.app_data_ref::<Nodes>().is_none() {
        let nodes = find(lua)?;
        lua.set_app_data(nodes);
    }
    let mut nodes = lua.app_data_mut::<Nodes>().expect("just set");
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
