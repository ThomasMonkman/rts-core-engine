use mlua::prelude::*;
use tracing::debug;

use crate::lua_api::lua_fn;

bitflags::bitflags! {
    /// Layer occupancy caps for a footprint (`Caps` in the spec table, `OccupancyCaps` in the
    /// underlying footprint struct). Mirrors the globals defined in FAF's `lua/footprints.lua`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OccupancyCaps: u8 {
        const LAND = 0x01;
        const SEABED = 0x02;
        const SUB = 0x04;
        const WATER = 0x08;
        const AIR = 0x10;
        const ORBIT = 0x20;
    }
}

bitflags::bitflags! {
    /// Footprint behaviour flags (`Flags` in the spec table). Mirrors the globals in
    /// FAF's `lua/footprints.lua`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct FootprintFlags: u8 {
        const IGNORE_STRUCTURES = 0x01;
    }
}

/// A single named footprint type registered via `SpecFootprints{}` in `blueprints.lua`.
///
/// Mirrors FAF's `FootprintSpec` (`lua/footprints.lua`), which extends `FootprintBlueprint`
/// (the per-entity footprint shape) with the registration fields `Name`, `Caps`, `Flags` and
/// `MaxWaterDepth`. Each spec causes a map-wide pathfinding structure to be built.
///
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintSpec {
    pub name: String,
    pub size_x: i8,
    pub size_z: i8,
    /// Layer caps (`LAND`/`SEABED`/`SUB`/`WATER`/...). Stored as `OccupancyCaps` in the
    /// underlying footprint struct; the spec table spells it `Caps`.
    pub caps: OccupancyCaps,
    pub flags: FootprintFlags,
    pub max_slope: Option<f32>,
    pub min_water_depth: Option<f32>,
    pub max_water_depth: Option<f32>,
}

impl crate::lua_api::LuaType for FootprintSpec {
    fn ty() -> crate::lua_api::LuaTy {
        use crate::lua_api::LuaTy;
        static INFO: crate::lua_api::AliasInfo = crate::lua_api::AliasInfo {
            name: "FootprintSpec",
            doc: "A footprint type for `SpecFootprints`; `Caps` and `Flags` are the `OCCUPY_*` / footprint flag bits.",
            ty: || {
                LuaTy::Record(vec![
                    ("Name", LuaTy::String),
                    ("SizeX", LuaTy::Number),
                    ("SizeZ", LuaTy::Number),
                    ("Caps", LuaTy::Number),
                    ("Flags", LuaTy::Number.optional()),
                    ("MaxSlope", LuaTy::Number.optional()),
                    ("MinWaterDepth", LuaTy::Number.optional()),
                    ("MaxWaterDepth", LuaTy::Number.optional()),
                ])
            },
            src: crate::lua_api::Src { file: file!(), line: line!() },
        };
        LuaTy::Alias(&INFO)
    }
}

impl FromLua for FootprintSpec {
    fn from_lua(value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        let t = value
            .as_table()
            .ok_or_else(|| LuaError::FromLuaConversionError {
                from: value.type_name(),
                to: "FootprintSpec".into(),
                message: Some("expected a table".into()),
            })?;

        Ok(FootprintSpec {
            name: t.get("Name")?,
            size_x: t.get("SizeX")?,
            size_z: t.get("SizeZ")?,
            caps: OccupancyCaps::from_bits_retain(t.get::<u8>("Caps")?),
            flags: t
                .get::<Option<u8>>("Flags")?
                .map(FootprintFlags::from_bits_retain)
                .unwrap_or_default(),
            max_slope: t.get("MaxSlope")?,
            min_water_depth: t.get("MinWaterDepth")?,
            max_water_depth: t.get("MaxWaterDepth")?,
        })
    }
}

/// The registered specs in registration order (the nav grid's class list
/// is indexed in this order).
#[derive(Default)]
pub struct RegisteredFootprints(pub Vec<FootprintSpec>);

impl RegisteredFootprints {
    /// As nav grid classes (`None`: nothing registered).
    pub fn nav_classes(&self) -> Option<Vec<crate::engine::sim::nav::ClassSpec>> {
        use crate::engine::sim::math::Fx;
        let fx = |v: Option<f32>| v.map(|v| Fx::from_num(v as f64));
        (!self.0.is_empty()).then(|| {
            self.0
                .iter()
                .map(|s| crate::engine::sim::nav::ClassSpec {
                    name: s.name.clone(),
                    size: s.size_x.max(s.size_z).max(1) as u32,
                    caps: s.caps.bits(),
                    ignore_structures: s.flags.contains(FootprintFlags::IGNORE_STRUCTURES),
                    max_slope: fx(s.max_slope),
                    min_water_depth: fx(s.min_water_depth),
                    max_water_depth: fx(s.max_water_depth),
                })
                .collect()
        })
    }
}

pub fn register_safe(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<RegisteredFootprints>().is_none() {
        lua.set_app_data(RegisteredFootprints::default());
    }
    lua_fn!(lua, global,
        /// Registers the footprint types (`{ Name, SizeX, SizeZ, Caps, Flags,
        /// MaxSlope, MinWaterDepth, MaxWaterDepth }`), each building a map-wide
        /// pathfinding structure.
        SpecFootprints = |lua, specs: Vec<FootprintSpec>| {
            let mut ordered = lua.app_data_mut::<RegisteredFootprints>();
            for spec in specs {
                debug!(name = %spec.name, "SpecFootprints: registering footprint");
                if let Some(o) = ordered.as_mut() {
                    o.0.push(spec);
                }
            }
            Ok(())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::lua_translation::translate;

    #[test]
    fn parses_and_spawns_specs() {
        let lua = Lua::new();
        register_safe(&lua).unwrap();

        // Mirrors FAF's `lua/footprints.lua`: layer-cap globals combined with the
        // bitwise `|` (e.g. `Caps = LAND | SEABED`). Run through `translate()` so
        // the `|` is rewritten to `bit32.bor` before the Luau VM sees it — the
        // same path real blueprint scripts take.
        let src = translate(
            r#"
            LAND = 0x01
            SEABED = 0x02
            SUB = 0x04
            WATER = 0x08
            AIR = 0x10
            ORBIT = 0x20
            IgnoreStructures = 0x01

            SpecFootprints {
                { Name = 'Vehicle1x1', SizeX = 1, SizeZ = 1, Caps = LAND, MaxWaterDepth = 0.05, MaxSlope = 0.75, Flags = 0 },
                { Name = 'Water3x3', SizeX = 3, SizeZ = 3, Caps = WATER, MinWaterDepth = 0.25, Flags = 0 },
                { Name = 'Amphibious3x3', SizeX = 3, SizeZ = 3, Caps = LAND | SEABED, MaxWaterDepth = 25, MaxSlope = 0.75, Flags = IgnoreStructures },
            }
            "#,
        );
        lua.load(&src).exec().unwrap();

        let mut specs: Vec<FootprintSpec> =
            lua.app_data_ref::<RegisteredFootprints>().unwrap().0.clone();
        specs.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(specs.len(), 3);

        // sorted by name: Amphibious3x3, Vehicle1x1, Water3x3
        let amphibious = &specs[0];
        assert_eq!(amphibious.name, "Amphibious3x3");
        assert_eq!(amphibious.caps, OccupancyCaps::LAND | OccupancyCaps::SEABED);
        assert!(amphibious.caps.contains(OccupancyCaps::SEABED));
        assert_eq!(amphibious.flags, FootprintFlags::IGNORE_STRUCTURES);

        let vehicle = &specs[1];
        assert_eq!(vehicle.name, "Vehicle1x1");
        assert_eq!(vehicle.size_x, 1);
        assert_eq!(vehicle.size_z, 1);
        assert_eq!(vehicle.caps, OccupancyCaps::LAND);
        assert_eq!(vehicle.flags, FootprintFlags::empty());
        assert_eq!(vehicle.max_slope, Some(0.75));
        assert_eq!(vehicle.max_water_depth, Some(0.05));
        assert_eq!(vehicle.min_water_depth, None);

        let water = &specs[2];
        assert_eq!(water.name, "Water3x3");
        assert_eq!(water.caps, OccupancyCaps::WATER);
        assert_eq!(water.min_water_depth, Some(0.25));
        assert_eq!(water.max_slope, None);
        assert_eq!(water.max_water_depth, None);
    }
}
