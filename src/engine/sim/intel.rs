//! Intel (vision, radar, sonar, omni, stealth, jamming, cloaking).

use std::collections::BTreeMap;

use crate::engine::sim::math::Fx;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum IntelType {
    Vision,
    WaterVision,
    Radar,
    Sonar,
    Omni,
    RadarStealthField,
    SonarStealthField,
    CloakField,
    Jammer,
    Spoof,
    Cloak,
    RadarStealth,
    SonarStealth,
}

impl IntelType {
    /// The names scripts use.
    const NAMES: [(&'static str, Self); 13] = [
        ("Vision", Self::Vision),
        ("WaterVision", Self::WaterVision),
        ("Radar", Self::Radar),
        ("Sonar", Self::Sonar),
        ("Omni", Self::Omni),
        ("RadarStealthField", Self::RadarStealthField),
        ("SonarStealthField", Self::SonarStealthField),
        ("CloakField", Self::CloakField),
        ("Jammer", Self::Jammer),
        ("Spoof", Self::Spoof),
        ("Cloak", Self::Cloak),
        ("RadarStealth", Self::RadarStealth),
        ("SonarStealth", Self::SonarStealth),
    ];

    /// Any letter case: FAF's Soothsayer asks for its `'vision'` radius
    /// (`XRB3301_script.lua`).
    pub fn parse(s: &str) -> Option<Self> {
        let exact = Self::NAMES.iter().find(|(name, _)| *name == s);
        exact.or_else(|| Self::NAMES.iter().find(|(name, _)| name.eq_ignore_ascii_case(s))).map(|&(_, t)| t)
    }

    /// Blueprint `Intel` field holding this type's radius, if any.
    pub fn radius_field(self) -> Option<&'static str> {
        Some(match self {
            Self::Vision => "VisionRadius",
            Self::WaterVision => "WaterVisionRadius",
            Self::Radar => "RadarRadius",
            Self::Sonar => "SonarRadius",
            Self::Omni => "OmniRadius",
            Self::RadarStealthField => "RadarStealthFieldRadius",
            Self::SonarStealthField => "SonarStealthFieldRadius",
            Self::CloakField => "CloakFieldRadius",
            _ => return None,
        })
    }

    pub const ALL: [Self; 13] = [
        Self::Vision,
        Self::WaterVision,
        Self::Radar,
        Self::Sonar,
        Self::Omni,
        Self::RadarStealthField,
        Self::SonarStealthField,
        Self::CloakField,
        Self::Jammer,
        Self::Spoof,
        Self::Cloak,
        Self::RadarStealth,
        Self::SonarStealth,
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IntelState {
    pub enabled: bool,
    pub radius: Fx,
}

/// An entity's intel, type-ordered.
pub type IntelMap = BTreeMap<IntelType, IntelState>;

#[cfg(test)]
mod parse_tests {
    use super::IntelType;

    #[test]
    fn intel_names_parse_in_any_letter_case() {
        assert_eq!(IntelType::parse("Vision"), Some(IntelType::Vision));
        assert_eq!(IntelType::parse("vision"), Some(IntelType::Vision));
        assert_eq!(IntelType::parse("RADARSTEALTHFIELD"), Some(IntelType::RadarStealthField));
        assert_eq!(IntelType::parse("Visions"), None);
    }
}
