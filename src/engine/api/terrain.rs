use crate::lua_api::shapes;
use std::sync::{Arc, RwLock};

use mlua::prelude::*;

use crate::engine::api::moho;
use crate::engine::assets::scmap;
use crate::engine::sim::terrain::Terrain;
use crate::engine::vfs::Vfs;
use crate::lua_api::{Obj, lua_fn, lua_method};

/// Loads the `.scmap` named by `ScenarioInfo.map` into the sim [`Terrain`].
pub fn load_map(lua: &Lua) -> LuaResult<()> {
    let scenario: LuaTable = lua.globals().get("ScenarioInfo")?;
    let path: String = scenario.get("map")?;
    let bytes = {
        let vfs = lua.app_data_ref::<Arc<RwLock<Vfs>>>().expect("VFS not in app_data").clone();
        let vfs = vfs.read().expect("VFS poisoned");
        vfs.read(&path).map_err(LuaError::external)?
    };
    let data = scmap::read_terrain(&bytes).map_err(LuaError::external)?;
    lua.set_app_data(crate::engine::snapshot::MapPath(path.clone()));
    let hm = &data.heightmap;
    tracing::info!(
        path,
        version = data.header.version,
        width = hm.width,
        height = hm.height,
        water = ?data.water,
        "map loaded"
    );
    lua.set_app_data(
        Terrain::from_heightmap(hm)
            .with_water(data.water.map(|w| w.surface))
            .with_types(data.terrain_types),
    );
    if let Some(mut w) = lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>() {
        w.set_bounds(hm.width, hm.height);
    }
    lua.set_app_data(crate::engine::sim::threat::ThreatMap::new(hm.width, hm.height));
    lua.set_app_data(crate::engine::sim::vision::IntelGrids::new(hm.width, hm.height));
    let blocking = blocking_types(lua)?;
    let grid = {
        let t = lua.app_data_ref::<Terrain>().expect("terrain just set");
        // FA's `SpecFootprints` (`footprints.lua`), else the same table
        // built in.
        let classes = lua.app_data_ref::<crate::engine::api::spec_footprints::RegisteredFootprints>().and_then(|r| r.nav_classes());
        tracing::info!(registered = classes.as_ref().map_or(0, |c| c.len()), "nav footprint classes");
        crate::engine::sim::nav::NavGrid::build_with(&t, &blocking, classes.unwrap_or_else(crate::engine::sim::nav::default_classes))
    };
    lua.set_app_data(grid);
    lua.set_app_data(BlockingTypes(blocking));
    lua.set_app_data(PendingMapProps(data.props));
    Ok(())
}

/// The loaded map's props, until `spawn_map_props` creates them.
struct PendingMapProps(Vec<scmap::MapProp>);

/// Creates the map's own props (trees, rocks, wrecks) from the `.scmap`, in
/// file order, each running its script's `OnCreate` (FAF's `Prop` sets the
/// reclaim values there). Props whose blueprint isn't loaded are skipped and
/// counted. Runs after `SetupSession`: FAF's `prop.lua` captures
/// `GridReclaimInstance` when first imported, and `SetupSession` creates it,
/// so props made earlier would never reach the AI's reclaim grid. (FAF's
/// `CreateProps` only handles the scenario file's `Props` table.)
pub fn spawn_map_props(lua: &Lua) -> LuaResult<()> {
    use crate::engine::sim::math::{FxQuat, FxVec3};
    let Some(PendingMapProps(props)) = lua.remove_app_data::<PendingMapProps>() else { return Ok(()) };
    // Wall time for the log line only.
    #[allow(clippy::disallowed_methods)]
    let started = std::time::Instant::now();
    let v = |a: [f32; 3]| FxVec3::from_f64(a[0] as f64, a[1] as f64, a[2] as f64);
    let (mut spawned, mut missing) = (0usize, std::collections::BTreeMap::<&str, usize>::new());
    for p in &props {
        let bp = p.blueprint.to_lowercase();
        let known = lua
            .app_data_ref::<crate::engine::api::blueprints::BlueprintRegistry>()
            .is_some_and(|b| b.get(crate::engine::api::blueprints::BlueprintKind::Prop, &bp).is_some());
        if !known {
            *missing.entry(p.blueprint.as_str()).or_default() += 1;
            continue;
        }
        let [x, y, z] = p.rotation;
        let q = FxQuat::from_axes(v(x), v(y), v(z));
        crate::engine::api::entity::spawn_prop(lua, &bp, v(p.position), q)?;
        spawned += 1;
    }
    tracing::info!(spawned, skipped = props.len() - spawned, ms = started.elapsed().as_millis() as u64, "map props created");
    for (bp, n) in missing {
        tracing::warn!(blueprint = bp, count = n, "map prop blueprint not loaded; skipped");
    }
    Ok(())
}

fn terrain(lua: &Lua) -> LuaResult<mlua::AppDataRef<'_, Terrain>> {
    lua.app_data_ref::<Terrain>().ok_or_else(|| LuaError::runtime("no map loaded"))
}

fn register_blocking(lua: &Lua) -> LuaResult<()> {
    lua_method!(lua, moho::AIBrain,
        /// Whether terrain blocks a shot from `from` to `to` (positions);
        /// `arc` is the fire arc (`"none"` by default).
        CheckBlockingTerrain = |lua, (_brain, from, to, arc): (Obj<moho::AIBrain>, shapes::Vector, shapes::Vector, Option<String>)| {
            let v = |t: &LuaTable| -> LuaResult<crate::engine::sim::math::FxVec3> {
                Ok(crate::engine::sim::math::FxVec3::from_f64(t.raw_get(1)?, t.raw_get(2)?, t.raw_get(3)?))
            };
            let arc = crate::engine::sim::terrain::FireArc::parse(arc.as_deref().unwrap_or("none"));
            Ok(terrain(lua)?.blocks(v(&from)?, v(&to)?, arc))
        })
}

const TERRAIN_TYPES_KEY: &str = "engine.terrain_types";

/// The playable area set by `SetPlayableRect` (world units).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlayableRect {
    pub min_x: crate::engine::sim::math::Fx,
    pub min_z: crate::engine::sim::math::Fx,
    pub max_x: crate::engine::sim::math::Fx,
    pub max_z: crate::engine::sim::math::Fx,
}

/// Armies exempt from the playable area (`SetIgnorePlayableRect`).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct IgnorePlayableRect(pub std::collections::BTreeSet<u32>);

/// Terrain type codes that block movement (`TerrainTypes` `Blocking`), kept
/// for nav grid updates.
pub struct BlockingTypes(pub [bool; 256]);

fn blocking_types(lua: &Lua) -> LuaResult<[bool; 256]> {
    let mut out = [false; 256];
    for pair in terrain_types(lua)?.pairs::<i64, LuaTable>() {
        let (code, entry) = pair?;
        if (0..256).contains(&code) && entry.get::<Option<bool>>("Blocking")?.unwrap_or(false) {
            out[code as usize] = true;
        }
    }
    Ok(out)
}

/// `TypeCode -> TerrainTypes entry`, loaded once from /lua/TerrainTypes.lua
/// (moho reads this file itself).
fn terrain_types(lua: &Lua) -> LuaResult<LuaTable> {
    if let Some(t) = lua.named_registry_value::<Option<LuaTable>>(TERRAIN_TYPES_KEY)? {
        return Ok(t);
    }
    let env = lua.create_table()?;
    let doscript: LuaFunction = lua.globals().get("doscript")?;
    doscript.call::<()>(("/lua/TerrainTypes.lua", env.clone()))?;
    let by_code = lua.create_table()?;
    if let LuaValue::Table(list) = env.raw_get::<LuaValue>("TerrainTypes")? {
        for entry in list.sequence_values::<LuaTable>() {
            let entry = entry?;
            let code: i64 = entry.get("TypeCode")?;
            by_code.raw_set(code, entry)?;
        }
    }
    lua.set_named_registry_value(TERRAIN_TYPES_KEY, by_code.clone())?;
    Ok(by_code)
}

/// The `TerrainTypes` entry for a type code (unknown codes: the default,
/// code 1), as `GetTerrainType` returns it.
pub(crate) fn terrain_type_entry(lua: &Lua, code: u8) -> LuaResult<LuaValue> {
    let types = terrain_types(lua)?;
    match types.raw_get::<LuaValue>(code as i64)? {
        LuaValue::Nil => types.raw_get::<LuaValue>(1),
        entry => Ok(entry),
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    register_blocking(lua)?;
    lua_fn!(lua, global,
        /// Flattens the map rect (`x`, `z`, size `sx`, `sz`) to `elevation`.
        /// Sim-visible: unit heights, projectile impacts and line-of-fire all
        /// read the changed heights. TODO: push the change to the renderer.
        FlattenMapRect = |lua, (x, z, sx, sz, elevation): (f64, f64, f64, f64, f64)| {
            let fx = crate::engine::sim::math::from_lua;
            if let Some(mut t) = lua.app_data_mut::<Terrain>() {
                t.flatten(fx(x), fx(z), fx(x + sx), fx(z + sz), fx(elevation));
            }
            // Ground units standing on it stand on the new ground next tick
            // (the real game's do: dev/probes/maps/probe_fatboy). A unit's
            // corners reach at most half the biggest footprint out.
            {
                let reach = crate::engine::sim::math::Fx::from_num(16);
                let mut world = crate::engine::api::entity::world(lua);
                let mut ids = Vec::new();
                world.unit_candidates(fx(x) - reach, fx(z) - reach, fx(x + sx) + reach, fx(z + sz) + reach, &mut ids);
                for id in ids {
                    if let Some(u) = world.get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
                        u.on_ground = false;
                    }
                }
            }
            // Re-derive passability around the change (cells touching it).
            let (Some(t), Some(mut grid), Some(b)) =
                (lua.app_data_ref::<Terrain>(), lua.app_data_mut::<crate::engine::sim::nav::NavGrid>(), lua.app_data_ref::<BlockingTypes>())
            else {
                return Ok(());
            };
            let cell = |v: f64| v.max(0.0).floor() as u32;
            grid.recompute(&t, &b.0, cell(x - 1.0), cell(z - 1.0), cell(x + sx + 1.0) + 1, cell(z + sz + 1.0) + 1);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Sets the playable area (campaign maps grow it; FAF's
        /// ScenarioFramework also records it in `ScenarioInfo.PlayableArea`).
        /// Units are held inside it after each move (`api::movement`, unless
        /// their army is let out); paths and flow fields don't know it.
        SetPlayableRect = |lua, (x0, z0, x1, z1): (f64, f64, f64, f64)| {
            let fx = crate::engine::sim::math::from_lua;
            lua.set_app_data(PlayableRect { min_x: fx(x0.min(x1)), min_z: fx(z0.min(z1)), max_x: fx(x0.max(x1)), max_z: fx(z0.max(z1)) });
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Lets an army (index or name) outside the playable area (campaign
        /// off-map attacks), or ends that.
        SetIgnorePlayableRect = |lua, (army, ignore): (shapes::Army, bool)| {
            let army = crate::engine::api::army::resolve_army(lua, &army)?;
            let mut set = lua.remove_app_data::<IgnorePlayableRect>().unwrap_or_default();
            if ignore {
                set.0.insert(army);
            } else {
                set.0.remove(&army);
            }
            lua.set_app_data(set);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The map's width and height in world units.
        GetMapSize = |lua, ()| {
            let t = terrain(lua)?;
            Ok((t.width, t.height))
        })?;
    lua_fn!(lua, global,
        /// The `TerrainTypes` entry under the point.
        GetTerrainType = |lua, (x, z): (f64, f64)| {
            let code = terrain(lua)?.type_at(crate::engine::sim::math::from_lua(x), crate::engine::sim::math::from_lua(z));
            Ok(match terrain_type_entry(lua, code)? {
                LuaValue::Table(t) => Some(shapes::TerrainType(t)),
                _ => None,
            })
        })?;
    lua_fn!(lua, global,
        /// The terrain type's `HeightOffset` at the point (only a few types
        /// set one), else 0.
        GetTerrainTypeOffset = |lua, (x, z): (f64, f64)| {
            let code = terrain(lua)?.type_at(crate::engine::sim::math::from_lua(x), crate::engine::sim::math::from_lua(z));
            let types = terrain_types(lua)?;
            let entry = match types.raw_get::<Option<LuaTable>>(code as i64)? {
                Some(e) => Some(e),
                None => types.raw_get::<Option<LuaTable>>(1)?,
            };
            Ok(match entry {
                Some(e) => e.get::<Option<f64>>("HeightOffset")?.unwrap_or(0.0),
                None => 0.0,
            })
        })?;
    lua_fn!(lua, global,
        /// The surface height (terrain or water) at the point.
        GetSurfaceHeight = |lua, (x, z): (f64, f64)| {
            let t = terrain(lua)?;
            Ok(t.surface_at(crate::engine::sim::math::from_lua(x), crate::engine::sim::math::from_lua(z)).to_num::<f64>())
        })?;
    lua_fn!(lua, global,
        /// The terrain height at the point.
        GetTerrainHeight = |lua, (x, z): (f64, f64)| {
            let t = terrain(lua)?;
            Ok(t.height_at(crate::engine::sim::math::from_lua(x), crate::engine::sim::math::from_lua(z)).to_num::<f64>())
        })
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires the FA install in ../Supreme Commander Forged Alliance"]
    fn parses_stock_map() {
        let b = std::fs::read("../Supreme Commander Forged Alliance/maps/SCMP_009/SCMP_009.scmap").unwrap();
        let d = crate::engine::assets::scmap::read_terrain(&b).unwrap();
        assert_eq!((d.header.version, d.heightmap.width, d.heightmap.height), (60, 1024, 1024));
        assert_eq!(d.heightmap.samples.len(), 1025 * 1025);
        assert_eq!(d.water.unwrap().surface, 17.5);
        assert_eq!(d.terrain_types.len(), 1024 * 1024);
        assert_eq!(d.props.len(), 5182);
    }
}
