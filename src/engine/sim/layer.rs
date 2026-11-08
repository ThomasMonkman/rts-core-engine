//! Movement/targeting layers (`Land|Water|Seabed|Sub|Air|Orbit`).

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    pub struct LayerMask: u8 {
        const LAND = 1 << 0;
        const SEABED = 1 << 1;
        const SUB = 1 << 2;
        const WATER = 1 << 3;
        const AIR = 1 << 4;
        const ORBIT = 1 << 5;
    }
}

/// Layer names in [`LayerMask`] bit order.
pub const LAYER_NAMES: [&str; 6] = ["Land", "Seabed", "Sub", "Water", "Air", "Orbit"];

impl LayerMask {
    /// A blueprint's `Physics.BuildOnLayerCaps`: the registered text form
    /// (`'Land'`, `'9'`) or the blueprint's own `{ LAYER_Land = true }`
    /// table; moho's default when absent is `Land` (real-game probe
    /// `probe_bp`).
    pub fn build_caps(bp: &mlua::Table) -> mlua::Result<Self> {
        let caps = match bp.get::<Option<mlua::Table>>("Physics")? {
            Some(physics) => physics.get::<mlua::Value>("BuildOnLayerCaps")?,
            None => mlua::Value::Nil,
        };
        if caps.is_nil() {
            return Ok(Self::LAND);
        }
        Ok(Self::from_bits_truncate(crate::engine::sim::unit_state::bitfield_bits(&LAYER_NAMES, "LAYER_", &caps)? as u8))
    }

    /// Parses moho mask strings: `"Land|Water"`, `"None"`, `""`. Unknown names
    /// are ignored.
    pub fn parse(s: &str) -> Self {
        s.split('|').fold(Self::empty(), |m, part| {
            m | match part.trim() {
                "Land" => Self::LAND,
                "Seabed" => Self::SEABED,
                "Sub" => Self::SUB,
                "Water" => Self::WATER,
                "Air" => Self::AIR,
                "Orbit" => Self::ORBIT,
                _ => Self::empty(),
            }
        })
    }

    /// Renders the mask in `parse`'s canonical order; empty yields `""`.
    pub fn as_mask_str(self) -> String {
        const LAYERS: &[(LayerMask, &str)] = &[
            (LayerMask::LAND, "Land"),
            (LayerMask::SEABED, "Seabed"),
            (LayerMask::SUB, "Sub"),
            (LayerMask::WATER, "Water"),
            (LayerMask::AIR, "Air"),
            (LayerMask::ORBIT, "Orbit"),
        ];
        let mut out = String::new();
        for &(bit, name) in LAYERS {
            if self.contains(bit) {
                if !out.is_empty() {
                    out.push('|');
                }
                out.push_str(name);
            }
        }
        out
    }
}

/// The single layer a unit is on (moho `Layer`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, crate::lua_api::LuaEnum, serde::Serialize, serde::Deserialize)]
pub enum Layer {
    #[default]
    Land,
    Seabed,
    Sub,
    Water,
    Air,
    Orbit,
}

impl Layer {
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Land => "Land",
            Layer::Seabed => "Seabed",
            Layer::Sub => "Sub",
            Layer::Water => "Water",
            Layer::Air => "Air",
            Layer::Orbit => "Orbit",
        }
    }

    pub fn mask(self) -> LayerMask {
        match self {
            Layer::Land => LayerMask::LAND,
            Layer::Seabed => LayerMask::SEABED,
            Layer::Sub => LayerMask::SUB,
            Layer::Water => LayerMask::WATER,
            Layer::Air => LayerMask::AIR,
            Layer::Orbit => LayerMask::ORBIT,
        }
    }

    /// Layer of a unit spawned with blueprint `Physics.MotionType`, standing
    /// over water (`wet`) or dry land. Moho's spawn layer (MohoEngine.dll
    /// 0x10214aa0, in the Confirmed table) takes Sub first over water when
    /// the footprint's caps include it (not for experimentals): this returns
    /// Water for a `SurfacingSub` and `api::unit` moves it under.
    /// `build_caps`: the blueprint's `Physics.BuildOnLayerCaps`
    /// ([`LayerMask::build_caps`]), which places a structure on water.
    pub fn initial(motion_type: &str, wet: bool, build_caps: LayerMask) -> Self {
        match motion_type {
            "RULEUMT_Air" => Layer::Air,
            "RULEUMT_Water" | "RULEUMT_SurfacingSub" | "RULEUMT_AmphibiousFloating" | "RULEUMT_Hover" if wet => Layer::Water,
            // Structures (`None`) on water float if built on the surface
            // (naval factories, sonar); one built only in the Sub layer
            // (the HARMS, xrb2309) is in Sub, as the real game's is when
            // made at the surface (dev/probes probe_auditgaps); the rest sit
            // on the seabed (underwater extractors, storage).
            "RULEUMT_None" if wet && build_caps.contains(LayerMask::WATER) => Layer::Water,
            "RULEUMT_None" if wet && build_caps.contains(LayerMask::SUB) && !build_caps.contains(LayerMask::SEABED) => Layer::Sub,
            _ if wet => Layer::Seabed,
            _ => Layer::Land,
        }
    }

    /// The layer a moving unit of `kind` is in, over water (`wet`) or not
    /// (same rules as [`Layer::initial`]).
    pub fn for_motion(kind: crate::engine::sim::movement::MotionKind, wet: bool) -> Self {
        use crate::engine::sim::movement::MotionKind as M;
        match kind {
            M::Air => Layer::Air,
            M::Water | M::Hover if wet => Layer::Water,
            _ if wet => Layer::Seabed,
            M::Water => Layer::Water,
            _ => Layer::Land,
        }
    }
}

/// Whether a unit of `kind` over ground at `height` is in the water (Seabed,
/// or Water for floating units) rather than on Land: the ground is below
/// the water surface, plus its `LayerChangeOffsetHeight` (`offset`) for
/// units that can be on land. MohoEngine.dll
/// `CUnitMotion::UpdateCurrentLayer` (0x102a1c00): with `LAND` in the
/// footprint's occupancy caps the switch height is water + offset, else
/// the water surface; the same height both ways.
pub fn wet(kind: crate::engine::sim::movement::MotionKind, height: crate::engine::sim::math::Fx, water: crate::engine::sim::math::Fx, offset: crate::engine::sim::math::Fx) -> bool {
    height < if kind.on_land() { water.saturating_add(offset) } else { water }
}

/// A structure blueprint built on the water surface rather than the seabed
/// (`Physics.BuildOnLayerCaps.LAYER_Water`: naval factories, sonar,
/// torpedo launchers; underwater extractors and storage list only
/// `LAYER_Seabed`).
pub fn floats_on_water(bp: &mlua::Table) -> mlua::Result<bool> {
    Ok(LayerMask::build_caps(bp)?.contains(LayerMask::WATER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_layers() {
        let (land, water, seabed, sub) = (LayerMask::LAND, LayerMask::LAND | LayerMask::WATER, LayerMask::LAND | LayerMask::SEABED, LayerMask::SUB);
        assert_eq!(Layer::initial("RULEUMT_Air", true, land), Layer::Air);
        assert_eq!(Layer::initial("RULEUMT_Land", false, land), Layer::Land);
        assert_eq!(Layer::initial("RULEUMT_Amphibious", true, land), Layer::Seabed);
        assert_eq!(Layer::initial("RULEUMT_Hover", true, land), Layer::Water);
        assert_eq!(Layer::initial("RULEUMT_None", true, water), Layer::Water, "naval factory");
        assert_eq!(Layer::initial("RULEUMT_None", true, seabed), Layer::Seabed, "underwater extractor");
        assert_eq!(Layer::initial("RULEUMT_None", true, sub), Layer::Sub, "HARMS (xrb2309)");
        assert_eq!(Layer::initial("RULEUMT_None", true, sub | LayerMask::SEABED), Layer::Seabed);
        assert_eq!(Layer::initial("RULEUMT_None", false, water), Layer::Land);
        assert_eq!(Layer::Water.mask(), LayerMask::WATER);
    }

    #[test]
    fn shallows_count_as_land_by_the_layer_change_offset() {
        use crate::engine::sim::math::Fx;
        use crate::engine::sim::movement::MotionKind as M;
        let (water, acu) = (Fx::from_num(10), Fx::from_num(-0.95));
        // Half a unit under: land for the ACU, in the water without the offset.
        let ground = Fx::from_num(9.5);
        assert!(!wet(M::Amphibious, ground, water, acu));
        assert!(wet(M::Amphibious, ground, water, Fx::ZERO));
        // A unit under: the seabed.
        assert!(wet(M::Amphibious, Fx::from_num(9), water, acu));
        // Ships don't walk on land: the water surface alone.
        assert!(wet(M::Water, ground, water, acu));
    }

    #[test]
    fn parses_masks() {
        assert_eq!(LayerMask::parse("Land|Water"), LayerMask::LAND | LayerMask::WATER);
        assert_eq!(LayerMask::parse("None"), LayerMask::empty());
    }
}
