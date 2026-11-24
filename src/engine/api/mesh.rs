//! Mesh/skeleton resolution for entities (shared, cached per mesh path).

#[allow(clippy::disallowed_types)]
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use mlua::prelude::*;

use crate::engine::api::blueprints::{BlueprintKind, BlueprintRegistry};
use crate::engine::assets::scm::{self, Skeleton};
use crate::engine::vfs::Vfs;

/// Parsed skeletons by `.scm` path, `None` for one that failed to load (so
/// it warns once and isn't re-read). Lookups only; never iterated.
#[allow(clippy::disallowed_types)]
#[derive(Default)]
pub struct SkeletonCache(HashMap<String, Option<Arc<Skeleton>>>);

/// Creates the skeleton cache up front: the first skeleton can load while
/// the snapshot holds `SnapshotSink` (`blueprint_transport`), and creating
/// app data then would panic on the borrowed container.
pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(SkeletonCache::default());
    Ok(())
}

/// LOD0 `.scm` path for an entity blueprint: `Display.MeshBlueprint` ->
/// mesh blueprint `LODs[1].MeshName`, defaulting to `<meshid minus _mesh>_lod0.scm`;
/// relative names resolve against the mesh blueprint's directory.
fn mesh_path(lua: &Lua, bp: &LuaTable) -> LuaResult<Option<String>> {
    let Some(display) = bp.get::<Option<LuaTable>>("Display")? else { return Ok(None) };
    let Some(mesh_id) = display.get::<Option<String>>("MeshBlueprint")? else { return Ok(None) };
    mesh_path_for(lua, &mesh_id).map(Some)
}

/// LOD0 `.scm` path for mesh blueprint `mesh_id`.
fn mesh_path_for(lua: &Lua, mesh_id: &str) -> LuaResult<String> {
    let mesh_id = mesh_id.to_lowercase();
    let mesh_bp = lua
        .app_data_ref::<BlueprintRegistry>()
        .and_then(|r| r.get(BlueprintKind::Mesh, &mesh_id).map(|b| b.table.clone()));
    let name = match &mesh_bp {
        Some(m) => m
            .get::<Option<LuaTable>>("LODs")?
            .map(|lods| lods.get::<Option<LuaTable>>(1))
            .transpose()?
            .flatten()
            .map(|lod| lod.get::<Option<String>>("MeshName"))
            .transpose()?
            .flatten(),
        None => None,
    };
    Ok(match name {
        Some(n) if n.starts_with('/') => n.to_lowercase(),
        Some(n) => {
            let dir = mesh_id.rsplit_once('/').map_or("", |(d, _)| d);
            format!("{dir}/{}", n.to_lowercase())
        }
        // Moho fills missing LOD names from the mesh blueprint's `Source`
        // (`/units/x/x_unit.bp` -> `/units/x/x_lod0.scm`), which also covers
        // FAF's generated `<mesh>_wreck` copies; else from the id.
        None => match source_stem(&mesh_bp)? {
            Some(stem) => format!("{stem}_lod0.scm"),
            None => format!("{}_lod0.scm", mesh_id.strip_suffix("_mesh").unwrap_or(&mesh_id)),
        },
    })
}

/// `Source` minus its `_<kind>.bp` suffix, lowercased.
fn source_stem(mesh_bp: &Option<LuaTable>) -> LuaResult<Option<String>> {
    let Some(source) = mesh_bp.as_ref().map(|m| m.get::<Option<String>>("Source")).transpose()?.flatten() else {
        return Ok(None);
    };
    let source = source.to_lowercase();
    let Some(base) = source.strip_suffix(".bp") else { return Ok(None) };
    Ok(match base.rsplit_once('_') {
        Some((stem, kind)) if !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase()) => Some(stem.to_owned()),
        _ => None,
    })
}

/// What the renderer needs to draw a mesh blueprint: VFS paths of its LOD0
/// `.scm` and textures.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RenderAsset {
    pub mesh: String,
    pub albedo: String,
    /// Specular (rgb) + team colour mask (alpha).
    pub specteam: String,
    /// A wreck (FAF's `<mesh>_wreck` copies: `Wreckage` / `Blackened*`
    /// shaders): drawn charred, without team colour.
    pub wreck: bool,
    /// Alpha-tested (`*Alpha` and `VertexNormal` shaders: foliage, tree
    /// groups' far LOD cards, wrecks of them): low albedo alpha is cut out,
    /// faces are two-sided.
    pub cutout: bool,
    /// A visual effect mesh without a technique (an `/effects/` blueprint
    /// with no `ShaderName`): a faint unlit overlay that writes no depth.
    pub effect: bool,
    /// Moho's build-in shader (FAF's generated `<mesh>_build` blueprints
    /// and the UEF build cube): drawn with the faction's construction look
    /// by fraction complete.
    pub build: Option<BuildShader>,
    /// The LOD's `SecondaryName` texture (the build shaders' pattern).
    pub secondary: Option<String>,
    /// The LOD's `LookupName` texture (moho's `lookupTexture`: the phase
    /// shields' electricity), else its `SecondaryName` (the Seraphim
    /// personal shield samples the secondary texture).
    pub lookup: Option<String>,
    /// Tangent-space normal map (`NormalsName`, else `<stem>_normalsTS.dds`).
    pub normals: String,
    /// FAF's `mesh.fx` shading family, by `ShaderName`.
    pub shading: Shading,
    /// The LOD scrolls its tread bands (`Scrolling`: "requires texture
    /// scrolling in the shader").
    pub scrolling: bool,
    /// How its vertices move in the vertex shader (trees swaying in the
    /// wind, pulsing pods), by `ShaderName`.
    pub vertex_motion: VertexMotion,
    /// Level of detail by camera distance (LOD0 assets only): `(cutoff,
    /// asset)` per mesh blueprint `LODs` entry, nearest first. The first
    /// level whose `LODCutoff` is beyond the distance is drawn; beyond the
    /// last none is (SupCom shows strategic icons there). Empty: always
    /// this mesh.
    pub lods: Vec<(f32, u32)>,
}

/// How a mesh is lit: FAF's `mesh.fx` pixel shader families by LOD
/// `ShaderName` (naval / stunned / phase-shield variants share their
/// faction's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum Shading {
    /// Anything not ported: plain lambert.
    #[default]
    Basic = 0,
    /// `Unit`, `UEFNavy`: `PBR_UEF_PS`.
    Uef = 1,
    /// `Insect`, `CybranNavy`: `PBR_CybranPS`.
    Cybran = 2,
    /// `Aeon`, `AeonNavy`: `PBR_AeonPS`.
    Aeon = 3,
    /// `Seraphim`, `SeraphimNavy`: `PBR_SeraphimPS`.
    Seraphim = 4,
    /// `AeonCZAR`: `PBR_AeonCZARPS`.
    AeonCzar = 5,
    /// `VertexNormal` (props): `VertexNormalPS_HighFidelity`.
    VertexNormal = 6,
    /// `NormalMappedAlpha` and its undulating / bloating forms (trees,
    /// props): `NormalMappedPS`.
    NormalMapped = 7,
    /// `NormalMappedTerrain` (rocks): `NormalMappedTerrainPS`.
    NormalMappedTerrain = 8,
    /// Shield bubbles: `ShieldUEF` / `ShieldCybran` / `ShieldAeon` /
    /// `ShieldSeraphim` (`ShieldPS` and kin), drawn after the water,
    /// blended, writing no depth.
    ShieldUef = 9,
    ShieldCybran = 10,
    ShieldAeon = 11,
    ShieldSeraphim = 12,
    /// `ShieldFill`: a depth-only fill so shields show their front surface;
    /// back-face culling does that here, so it draws nothing.
    ShieldFill = 13,
    /// `NormalMappedGlow` (most projectiles, as `TMeshGlow`):
    /// `NormalMappedPS` whose specular blue glows (the bloom picks it up).
    NormalMappedGlow = 14,
    /// `Wreckage`: `WreckagePS`, the noise texture scrolled per wreck.
    Wreckage = 15,
    /// `BlackenedNormalMappedAlpha` (burnt trees, alpha-tested wrecks):
    /// `BlackenedNormalMappedPS`.
    Blackened = 16,
    /// `AeonBuildPuddle` (the mercury pool under Aeon builds):
    /// `PBR_AeonBuildPuddlePS`, fully metallic, its texture drifting.
    AeonBuildPuddle = 17,
    /// `Flat` (`TMeshNoLighting`): `FlatPS`, unlit albedo, alpha-tested.
    Flat = 18,
    /// `Metal`: `NormalMappedMetalPS` (anisotropic highlight).
    Metal = 19,
    // ---- Blended effect techniques, drawn after the water (moho's
    // `STAGE_POSTWATER`) and timed by the mesh's age (`time - material.x`).
    /// `Effect` (`TMeshAdd`): `EffectPS`.
    Effect = 20,
    /// `Explosion` (`TMeshExplosion`): `ExplosionPS` (fade in, dissolve).
    Explosion = 21,
    /// `Cloud` (`TMeshCloud`): `EffectFadePS(180, 0.025)`.
    Cloud = 22,
    /// `OuterCloud`: `AlphaFadeTexShiftScalePS(1, -0.008, 180, 0.025)`.
    OuterCloud = 23,
    /// `NukeUEF`: `NukeHeadPS`.
    NukeUef = 24,
    /// `NukeEMP`: `AlphaFadeTexShiftScalePS(1, -0.008, 180, 0.025)`.
    NukeEmp = 25,
    /// `NukeQuantum`: `AlphaFadeTexShiftScalePS(0.5, -0.006, 150, 0.025)`.
    NukeQuantum = 26,
    /// `PhalanxEffect`: `TwoTexShiftScalePS`, premultiplied.
    PhalanxEffect = 27,
    /// `UEFQuantumGate`: `TexSubAlphaMaskStaticLoFiPS`, two-sided.
    UefQuantumGate = 28,
    /// `ShieldImpact`: `ShieldImpactPS`, additive.
    ShieldImpact = 29,
    /// `CybranShieldImpact`: `CybranShieldImpactPS`, two-sided.
    CybranShieldImpact = 30,
    /// `GlassAlpha`: `GlassAlphaPS`.
    GlassAlpha = 31,
    /// `AlphaFade`: `AlphaFadePS(2, 0.145)`.
    AlphaFade = 32,
    /// `TACloak`: `TACloakPS`, half transparent.
    Cloak = 33,
    // ---- Two-pass techniques: the faction's unit shader, then an overlay.
    /// `PhaseShield` / `AeonPhaseShield` / `CybranPhaseShield` /
    /// `SeraphimPersonalShield`: overlay `*PhaseShieldPS` pushed 0.02 out.
    PhaseShieldUef = 34,
    PhaseShieldAeon = 35,
    PhaseShieldCybran = 36,
    PhaseShieldSeraphim = 37,
    /// `UnitStunned` / `InsectStunned` / `AeonStunned` / `SeraphimStunned`:
    /// overlay `StunnedUnit` (black at 66%).
    StunnedUef = 38,
    StunnedCybran = 39,
    StunnedAeon = 40,
    StunnedSeraphim = 41,
    // ---- Order feedback (interface meshes: `AddCommandFeedbackBlip`,
    // `WorldMesh`), drawn last without depth: `CommandFeedbackVS(scaleTo)`
    // eases the scale from 1 to `scaleTo` over the lifetime parameter.
    /// `CommandFeedback`: scale to 0.7, fading out (`CommandFeedbackPS0(true)`).
    CommandFeedback = 42,
    /// `CommandFeedback2`: scale to 1.1, fading out.
    CommandFeedback2 = 43,
    /// `RallyPoint`: scale to 0.7, not fading.
    RallyPoint = 44,
    /// `MapImager` (the skin's border round the map, `MapBorderAdd`):
    /// `MapImagerPS0` / `PS1`, sun-shaded unless the spec map's blue masks
    /// it, its blue glowing.
    MapImager = 45,
    /// `Clutter` / `UnderwaterClutter` / `UndulatingClutter` /
    /// `UnderwaterUndulatingClutter` (ground clutter, kelp): `ClutterPS`,
    /// the albedo lit by the normal map and the sun (no specular, no
    /// shadow attenuation), alpha-tested at `0x70`.
    Clutter = 46,
    /// `CommandFeedback3` (FAF's ping markers, `ping_nuke_marker`): order
    /// feedback at the mesh's own size (`VertexNormalVS`), not fading
    /// (`CommandFeedbackPS0(false)`), not alpha-tested.
    CommandFeedback3 = 47,
}

/// How a technique's vertex shader moves the mesh's vertices over moho's
/// shader `time` (game ticks; FA `mesh.fx`). Render-only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum VertexMotion {
    #[default]
    Still = 0,
    /// `UndulatingNormalMappedAlpha` (trees, `UndulatingNormalMappedVS`):
    /// each vertex leans along the wind by `0.003 * y` (mesh units) times
    /// `sin^2(0.05 * time - wind . translation)`.
    Undulating = 1,
    /// `UndulatingClutter` / `UnderwaterUndulatingClutter` (FAF's
    /// `Clutter02_prop`, `UndulatingClutterVS`; kelp is `UnderwaterClutter`
    /// and stands still): `0.1 * y`, `sin^2(0.0625 * time - ...)`.
    UndulatingClutter = 2,
    /// `BloatingNormalMappedAlpha` (Red Rocks pods,
    /// `BloatingNormalMappedVS`): swells and shrinks along the direction
    /// from the mesh origin.
    Bloating = 3,
}

impl VertexMotion {
    pub fn from_shader(name: &str) -> Self {
        match name {
            "UndulatingNormalMappedAlpha" => Self::Undulating,
            "UndulatingClutter" | "UnderwaterUndulatingClutter" => Self::UndulatingClutter,
            "BloatingNormalMappedAlpha" => Self::Bloating,
            _ => Self::Still,
        }
    }
}

/// How a mesh's technique is drawn after the water (blend and depth state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectPass {
    /// `SrcAlpha, InvSrcAlpha`, no depth writes.
    Alpha,
    /// Alpha blended and writing depth (alpha-tested in the shader).
    AlphaWrite,
    /// Alpha blended, two-sided, no depth writes.
    AlphaTwoSided,
    /// `SrcAlpha, One`.
    Add,
    /// `One, InvSrcAlpha`.
    Premultiplied,
    /// Alpha blended, pushed 0.02 out along the normal (phase shields).
    Overlay,
    /// Alpha blended without depth test or writes, scaled by age and camera
    /// distance (`CommandFeedbackVS`: order feedback, rally points).
    Feedback,
}

/// Moho's table of old shader names (the exe's "Use of 'old' shader"
/// remap, beside its `mesh.fx` technique lookup): the technique each draws
/// with. Anything else is a technique name already.
pub fn technique_name(name: &str) -> &str {
    match name {
        "TMeshNoLighting" => "Flat",
        "TMeshNoNormals" => "VertexNormal",
        "TMeshAlpha" => "NormalMappedAlpha",
        "TMeshGlow" => "NormalMappedGlow",
        "TMeshTerrain" => "NormalMappedTerrain",
        "Simple" | "Team" => "Unit",
        "TMeshAlphaGlowFade" => "UnitBuild",
        "TMeshMetalBuild" => "AeonBuild",
        "TMeshShield" => "Shield",
        "TMeshZFill" => "ShieldFill",
        "TMeshAdd" => "Effect",
        "TMeshExplosion" => "Explosion",
        "TMeshCloud" => "Cloud",
        "TMeshOuterCloud" => "OuterCloud",
        "TMeshEMPNuke" => "NukeEMP",
        "TMeshQuantumNuke" => "NukeQuantum",
        "TMeshTemporalBubble" => "TemporalBubble",
        other => other,
    }
}

impl Shading {
    pub fn from_shader(name: &str) -> Self {
        match name {
            "Unit" | "UEFNavy" => Self::Uef,
            "Insect" | "CybranNavy" => Self::Cybran,
            "Aeon" | "AeonNavy" => Self::Aeon,
            "Seraphim" | "SeraphimNavy" => Self::Seraphim,
            "PhaseShield" => Self::PhaseShieldUef,
            "AeonPhaseShield" => Self::PhaseShieldAeon,
            "CybranPhaseShield" => Self::PhaseShieldCybran,
            "SeraphimPersonalShield" => Self::PhaseShieldSeraphim,
            "UnitStunned" => Self::StunnedUef,
            "InsectStunned" => Self::StunnedCybran,
            "AeonStunned" => Self::StunnedAeon,
            "SeraphimStunned" => Self::StunnedSeraphim,
            "Flat" => Self::Flat,
            "Metal" => Self::Metal,
            "Effect" => Self::Effect,
            "Explosion" => Self::Explosion,
            "Cloud" => Self::Cloud,
            "OuterCloud" => Self::OuterCloud,
            "NukeUEF" => Self::NukeUef,
            "NukeEMP" => Self::NukeEmp,
            "NukeQuantum" => Self::NukeQuantum,
            "PhalanxEffect" => Self::PhalanxEffect,
            "UEFQuantumGate" => Self::UefQuantumGate,
            "ShieldImpact" => Self::ShieldImpact,
            "CybranShieldImpact" => Self::CybranShieldImpact,
            "GlassAlpha" => Self::GlassAlpha,
            "AlphaFade" => Self::AlphaFade,
            "TACloak" => Self::Cloak,
            "CommandFeedback" => Self::CommandFeedback,
            "CommandFeedback2" => Self::CommandFeedback2,
            "CommandFeedback3" => Self::CommandFeedback3,
            "RallyPoint" => Self::RallyPoint,
            "MapImager" => Self::MapImager,
            "AeonCZAR" => Self::AeonCzar,
            "VertexNormal" => Self::VertexNormal,
            "NormalMappedAlpha" | "UndulatingNormalMappedAlpha" | "BloatingNormalMappedAlpha" | "NormalMappedAlphaNoShadow" => Self::NormalMapped,
            "NormalMappedTerrain" => Self::NormalMappedTerrain,
            "ShieldUEF" => Self::ShieldUef,
            "ShieldCybran" => Self::ShieldCybran,
            "ShieldAeon" => Self::ShieldAeon,
            "ShieldSeraphim" => Self::ShieldSeraphim,
            "ShieldFill" => Self::ShieldFill,
            "NormalMappedGlow" => Self::NormalMappedGlow,
            "Wreckage" => Self::Wreckage,
            "BlackenedNormalMappedAlpha" => Self::Blackened,
            "AeonBuildPuddle" => Self::AeonBuildPuddle,
            "Clutter" | "UnderwaterClutter" | "UndulatingClutter" | "UnderwaterUndulatingClutter" => Self::Clutter,
            // The build-in techniques (drawn by `BuildShader`).
            "UEFBuild" | "CybranBuild" | "AeonBuild" | "AeonBuildNoAnimation" | "SeraphimBuild" | "UEFBuildCube" => Self::Basic,
            // Moho falls back to `Unit` for a technique it doesn't have.
            _ => Self::Uef,
        }
    }

    pub fn is_shield(self) -> bool {
        matches!(self, Self::ShieldUef | Self::ShieldCybran | Self::ShieldAeon | Self::ShieldSeraphim)
    }

    /// The technique's after-water pass, if it has one (the two-pass ones
    /// also draw their faction's shader with the opaque models).
    pub fn effect_pass(self) -> Option<EffectPass> {
        use EffectPass as P;
        Some(match self {
            Self::Effect | Self::OuterCloud | Self::NukeEmp | Self::NukeQuantum | Self::GlassAlpha | Self::StunnedUef | Self::StunnedCybran | Self::StunnedAeon | Self::StunnedSeraphim => P::Alpha,
            Self::Explosion | Self::Cloud | Self::NukeUef | Self::AlphaFade | Self::Cloak => P::AlphaWrite,
            Self::UefQuantumGate | Self::CybranShieldImpact => P::AlphaTwoSided,
            Self::ShieldImpact => P::Add,
            Self::PhalanxEffect => P::Premultiplied,
            Self::PhaseShieldUef | Self::PhaseShieldAeon | Self::PhaseShieldCybran | Self::PhaseShieldSeraphim => P::Overlay,
            Self::CommandFeedback | Self::CommandFeedback2 | Self::CommandFeedback3 | Self::RallyPoint => P::Feedback,
            _ => return None,
        })
    }

    /// Drawn only after the water (no opaque base pass).
    pub fn effect_only(self) -> bool {
        self.effect_pass().is_some() && !self.has_base_pass()
    }

    /// Two-pass techniques: the faction's unit shader first.
    pub fn has_base_pass(self) -> bool {
        matches!(
            self,
            Self::PhaseShieldUef | Self::PhaseShieldAeon | Self::PhaseShieldCybran | Self::PhaseShieldSeraphim | Self::StunnedUef | Self::StunnedCybran | Self::StunnedAeon | Self::StunnedSeraphim
        )
    }

    /// Its shader reads the mesh's age (`time - material.x`).
    pub fn timed(self) -> bool {
        self.effect_pass().is_some()
    }
}

/// The build-in techniques of FA's `mesh.fx`, by LOD `ShaderName`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum BuildShader {
    Uef = 1,
    Cybran = 2,
    Aeon = 3,
    AeonNoAnimation = 4,
    Seraphim = 5,
    UefBuildCube = 6,
}

impl BuildShader {
    pub fn from_shader(name: &str) -> Option<Self> {
        Some(match name {
            "UEFBuild" => Self::Uef,
            "CybranBuild" => Self::Cybran,
            "AeonBuild" => Self::Aeon,
            "AeonBuildNoAnimation" => Self::AeonNoAnimation,
            "SeraphimBuild" => Self::Seraphim,
            "UEFBuildCube" => Self::UefBuildCube,
            _ => return None,
        })
    }
}

/// `path` with `.` and `..` segments resolved (`/a/b/../c` -> `/a/c`).
fn collapse_dots(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Mesh blueprints the renderer may be asked to draw, numbered in first-use
/// order (render-side only: never read by the sim).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct MeshRegistry {
    ids: std::collections::BTreeMap<String, u32>,
    pub assets: Vec<RenderAsset>,
}

/// A texture of the LOD's entry (`AlbedoName` / `SpecularName`), else
/// defaulted from the mesh file's stem (`/units/x/x_lod0.scm` ->
/// `/units/x/x_albedo.dds`; some blueprints share a mesh's textures that way,
/// `spherez_prop`: "share other rock's albedo and specteam"), or — when the
/// mesh file ships elsewhere — from the mesh blueprint's `Source` stem (the
/// Megalith: mesh in `/units/xrl0404/`, textures in `/units/xrl0403/`).
fn texture_path(
    lod: Option<&LuaTable>,
    key: &str,
    mesh_id: &str,
    mesh_stem: &str,
    source_stem: Option<&str>,
    suffix: &str,
    exists: &dyn Fn(&str) -> bool,
) -> LuaResult<String> {
    let name = match lod {
        Some(l) => l.get::<Option<String>>(key)?,
        None => None,
    };
    Ok(match name {
        Some(n) if n.starts_with('/') => n.to_lowercase(),
        Some(n) => collapse_dots(&format!("{}/{}", mesh_id.rsplit_once('/').map_or("", |(d, _)| d), n.to_lowercase())),
        None => {
            let own = format!("{mesh_stem}{suffix}");
            match source_stem.filter(|_| !exists(&own)).map(|s| format!("{s}{suffix}")).filter(|p| exists(p)) {
                Some(alt) => alt,
                None => own,
            }
        }
    })
}

/// Whether technique `name` alpha-tests in FAF's `mesh.fx` (the mesh
/// techniques with `AlphaTestEnable = true`; `Flat` tests in its shader).
fn alpha_tested(name: &str) -> bool {
    name.ends_with("Alpha") || name.ends_with("Clutter") || name == "NormalMappedAlphaNoShadow" || name == "VertexNormal"
}

/// The material of mesh blueprint `LODs` entry `lod` drawing `mesh`: its
/// technique and textures, each named texture or default resolved by
/// `texture(key, default suffix)`.
fn lod_material(lod: Option<&LuaTable>, mesh_id: &str, mesh: String, texture: &dyn Fn(&str, &str) -> LuaResult<String>) -> LuaResult<RenderAsset> {
    let named = |key: &str| -> LuaResult<Option<String>> {
        match lod {
            Some(l) => l.get::<Option<String>>(key),
            None => Ok(None),
        }
    };
    let shader = named("ShaderName")?.map(|n| technique_name(&n).to_owned());
    let secondary = named("SecondaryName")?;
    Ok(RenderAsset {
        wreck: shader.as_deref().is_some_and(|s| s == "Wreckage" || s.starts_with("Blackened")),
        cutout: shader.as_deref().is_some_and(alpha_tested),
        effect: mesh_id.starts_with("/effects/") && shader.is_none(),
        build: shader.as_deref().and_then(BuildShader::from_shader),
        lookup: match (named("LookupName")?, &secondary) {
            (Some(_), _) => Some(texture("LookupName", "")?),
            (None, Some(_)) => Some(texture("SecondaryName", "")?),
            (None, None) => None,
        },
        albedo: texture("AlbedoName", "_albedo.dds")?,
        specteam: texture("SpecularName", "_specteam.dds")?,
        // (The UEF shield reads its normals from the secondary texture.)
        normals: match shader.as_deref() {
            Some("ShieldUEF") if secondary.is_some() => texture("SecondaryName", "")?.to_lowercase(),
            _ => texture("NormalsName", "_normalsTS.dds")?.to_lowercase(),
        },
        secondary: secondary.map(|n| n.to_lowercase()),
        shading: shader.as_deref().map_or(Shading::Basic, Shading::from_shader),
        scrolling: match lod {
            Some(l) => l.get::<Option<bool>>("Scrolling")?.unwrap_or(false),
            None => false,
        },
        vertex_motion: shader.as_deref().map_or(VertexMotion::Still, VertexMotion::from_shader),
        lods: Vec::new(),
        mesh,
    })
}

/// The render index of mesh blueprint `mesh_id`, registering it on first use.
pub fn render_index(lua: &Lua, mesh_id: &str) -> LuaResult<u32> {
    let mesh_id = mesh_id.to_lowercase();
    if let Some(i) = lua.app_data_ref::<MeshRegistry>().and_then(|r| r.ids.get(&mesh_id).copied()) {
        return Ok(i);
    }
    let mesh = mesh_path_for(lua, &mesh_id)?;
    let mesh_bp = lua
        .app_data_ref::<BlueprintRegistry>()
        .and_then(|r| r.get(BlueprintKind::Mesh, &mesh_id).map(|b| b.table.clone()));
    let lod = mesh_bp
        .as_ref()
        .and_then(|m| m.get::<Option<LuaTable>>("LODs").ok().flatten().and_then(|l| l.get::<Option<LuaTable>>(1).ok().flatten()));
    let exists = |p: &str| lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().is_some_and(|v| v.read().is_ok_and(|v| v.file_info(p).is_some()));
    // LOD0's mesh-file stem (`/units/xrl0404/xrl0404`), and the mesh
    // blueprint's `Source` stem for names the file's own folder can't
    // provide (the Megalith's textures ship with its source folder).
    let mesh_stem = mesh.strip_suffix("_lod0.scm").or_else(|| mesh.strip_suffix(".scm")).unwrap_or(&mesh).to_owned();
    let src_stem = source_stem(&mesh_bp)?;
    let asset = lod_material(lod.as_ref(), &mesh_id, mesh, &|key, suffix| {
        texture_path(lod.as_ref(), key, &mesh_id, &mesh_stem, src_stem.as_deref(), suffix, &exists)
    })?;
    let lod_table = lua
        .app_data_ref::<BlueprintRegistry>()
        .and_then(|r| r.get(BlueprintKind::Mesh, &mesh_id).map(|b| b.table.clone()))
        .and_then(|m| m.get::<Option<LuaTable>>("LODs").ok().flatten());
    let mut levels = Vec::new();
    if let Some(lods) = &lod_table {
        for (k, entry) in lods.clone().sequence_values::<LuaTable>().enumerate() {
            let entry = entry?;
            let cutoff = entry.get::<Option<f64>>("LODCutoff")?.map_or(f32::INFINITY, |c| c as f32);
            if k == 0 {
                levels.push((cutoff, None));
                continue;
            }
            // `<Source stem>_lod<k>.scm` (or the entry's `MeshName`); a level
            // whose mesh isn't there keeps drawing the previous one.
            let mesh = match entry.get::<Option<String>>("MeshName")? {
                Some(n) if n.starts_with('/') => Some(n.to_lowercase()),
                Some(n) => Some(collapse_dots(&format!("{}/{}", mesh_id.rsplit_once('/').map_or("", |(d, _)| d), n.to_lowercase()))),
                None => Some(match &src_stem {
                    Some(s) => format!("{s}_lod{k}.scm"),
                    None => format!("{mesh_stem}_lod{k}.scm"),
                }),
            }
            .filter(|m| exists(m));
            let level = mesh.map(|mesh| {
                // The level's own technique and textures (moho's
                // `RMeshBlueprintLOD::Init` on every entry): a name it gives,
                // else the Source stem's default — LOD0's art, never a
                // `_lod<k>` file (XSL0105's LOD1 names its albedo but not its
                // normals: retail draws it with `xsl0105_normalsTS.dds`).
                // Where that file is missing, LOD0's (ours: retail binds
                // `b_fails_to_load.dds`).
                let lod0 = |key: &str| match key {
                    "AlbedoName" => asset.albedo.clone(),
                    "SpecularName" => asset.specteam.clone(),
                    "NormalsName" => asset.normals.clone(),
                    _ => String::new(),
                };
                let mut level = lod_material(Some(&entry), &mesh_id, mesh, &|key, suffix| {
                    if entry.get::<Option<String>>(key)?.is_some() {
                        return texture_path(Some(&entry), key, &mesh_id, "", None, suffix, &exists);
                    }
                    Ok(src_stem.as_deref().map(|s| format!("{s}{suffix}").to_lowercase()).filter(|p| exists(p)).unwrap_or_else(|| lod0(key)))
                })?;
                // An entry without a `LookupName` would take `<Source
                // stem>_lookup.dds`, which no FAF unit ships (1.8k Seraphim
                // LOD1 entries leave it out): LOD0's.
                if level.lookup.is_none() && entry.get::<Option<String>>("LookupName")?.is_none() {
                    level.lookup = src_stem.as_deref().map(|s| format!("{s}_lookup.dds")).filter(|p| exists(p)).or_else(|| asset.lookup.clone());
                }
                if entry.get::<Option<bool>>("Scrolling")?.is_none() {
                    level.scrolling = asset.scrolling;
                }
                Ok::<_, LuaError>(level)
            });
            levels.push((cutoff, level.transpose()?));
        }
    }
    if lua.app_data_ref::<MeshRegistry>().is_none() {
        lua.set_app_data(MeshRegistry::default());
    }
    let mut r = lua.app_data_mut::<MeshRegistry>().expect("MeshRegistry");
    let i = r.assets.len() as u32;
    r.assets.push(asset);
    r.ids.insert(mesh_id, i);
    let mut lods = Vec::with_capacity(levels.len());
    let mut last = i;
    for (cutoff, level) in levels {
        if let Some(a) = level {
            last = r.assets.len() as u32;
            r.assets.push(a);
        }
        lods.push((cutoff, last));
    }
    // Only cutoffs matter if every level is LOD0 itself.
    r.assets[i as usize].lods = lods;
    Ok(i)
}

/// The render index for an entity blueprint's `Display.MeshBlueprint`.
/// FAF names a mesh blueprint for every blueprint (`<source>_mesh`) but only
/// registers one when the blueprint has a `Display.Mesh`: without it there
/// is no model (e.g. the UEF build effect, the warp-in sphere).
pub fn render_index_for(lua: &Lua, bp: &LuaTable) -> LuaResult<Option<u32>> {
    let Some(display) = bp.get::<Option<LuaTable>>("Display")? else { return Ok(None) };
    let Some(mesh_id) = display.get::<Option<String>>("MeshBlueprint")? else { return Ok(None) };
    let registered = lua.app_data_ref::<BlueprintRegistry>().is_none_or(|r| r.get(BlueprintKind::Mesh, &mesh_id.to_lowercase()).is_some());
    if !registered {
        return Ok(None);
    }
    render_index(lua, &mesh_id).map(Some)
}

/// The skeleton for mesh blueprint `mesh_id` (`Entity:SetMesh`).
pub fn skeleton_for_mesh(lua: &Lua, mesh_id: &str) -> LuaResult<Option<Arc<Skeleton>>> {
    // `SetMesh('')` clears the mesh.
    if mesh_id.is_empty() {
        return Ok(None);
    }
    let path = mesh_path_for(lua, mesh_id)?;
    load_skeleton(lua, path)
}

/// The skeleton for an entity blueprint, if it has a mesh that loads.
pub fn skeleton_for(lua: &Lua, bp: &LuaTable) -> LuaResult<Option<Arc<Skeleton>>> {
    let Some(path) = mesh_path(lua, bp)? else { return Ok(None) };
    load_skeleton(lua, path)
}

/// Parses (or fetches from the cache) the skeleton of `.scm` file `path`.
fn load_skeleton(lua: &Lua, path: String) -> LuaResult<Option<Arc<Skeleton>>> {
    if let Some(s) = lua.app_data_ref::<SkeletonCache>().and_then(|c| c.0.get(&path).cloned()) {
        return Ok(s);
    }
    let bytes = {
        let vfs = lua.app_data_ref::<Arc<RwLock<Vfs>>>().expect("VFS not in app_data").clone();
        let vfs = vfs.read().expect("VFS poisoned");
        vfs.read(&path)
    };
    let skeleton = match bytes.and_then(|b| scm::read_skeleton(&b)) {
        Ok(s) => Some(Arc::new(s)),
        Err(e) => {
            tracing::warn!(path, "failed to load mesh skeleton: {e}");
            None
        }
    };
    if lua.app_data_ref::<SkeletonCache>().is_none() {
        lua.set_app_data(SkeletonCache::default());
    }
    lua.app_data_mut::<SkeletonCache>().unwrap().0.insert(path, skeleton.clone());
    Ok(skeleton)
}

#[cfg(test)]
mod tests {
    // FA's clutter techniques (ground clutter, kelp) draw with `ClutterPS`,
    // cut out; before they fell to `Basic` as solid cards.
    #[test]
    fn clutter_techniques_shade_as_clutter_and_cut_out() {
        for name in ["Clutter", "UnderwaterClutter", "UndulatingClutter", "UnderwaterUndulatingClutter"] {
            assert_eq!(super::Shading::from_shader(name), super::Shading::Clutter, "{name}");
            assert!(super::alpha_tested(name), "{name}");
        }
    }

    #[test]
    #[ignore = "requires the FA install"]
    fn parses_acu_skeleton() {
        let f = std::fs::File::open("../Supreme Commander Forged Alliance/gamedata/units.scd").unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let name = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().to_owned())
            .find(|n| n.eq_ignore_ascii_case("units/uel0001/uel0001_lod0.scm"))
            .unwrap();
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut z.by_name(&name).unwrap(), &mut b).unwrap();
        let s = crate::engine::assets::scm::read_skeleton(&b).unwrap();
        assert_eq!(s.bones.len(), 29);
        assert_eq!(s.bones[1].name, "Torso");
        assert_eq!(s.find("right_upgrade_muzzle"), Some(28));
    }

    // The Megalith's LOD0 mesh ships in `/units/xrl0404/` while its
    // textures ship with `/units/xrl0403/`; a defaulted texture named after
    // the mesh file resolved into the wrong folder and drew the model white.
    #[test]
    fn defaulted_lod_textures_prefer_the_mesh_file_then_the_source() {
        use super::{BlueprintKind, MeshRegistry, render_index, source_stem};

        let lua = mlua::Lua::new();
        super::super::blueprints::register(&lua).unwrap();
        let vfs = crate::engine::vfs::Vfs::new_with_embedded(
            vec![],
            vec![],
            &[
                ("/units/xrl0403/xrl0403_albedo.dds", &[]),
                ("/units/xrl0403/xrl0403_specteam.dds", &[]),
                ("/units/xrl0403/xrl0403_normalsTS.dds", &[]),
                ("/units/xrl0404/xrl0404_lod1.scm", &[]),
                ("/units/xrl0403/xrl0403_lod1_normalsTS.dds", &[]),
                ("/env/common/props/sphere_lod0.scm", &[]),
                ("/env/common/props/sphere_albedo.dds", &[]),
                ("/env/common/props/spherez_albedo.dds", &[]),
            ],
        )
        .unwrap();
        lua.set_app_data(std::sync::Arc::new(std::sync::RwLock::new(vfs)));

        let meg = mlua::Lua::new();
        let meg = &meg;
        super::super::blueprints::register(meg).unwrap();
        let vfs_handle = lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().unwrap().clone();
        meg.set_app_data(vfs_handle);
        meg.load(
            "RegisterMeshBlueprint \
             { BlueprintId = '/units/xrl0403/xrl0403_mesh' \
             , Source = '/units/XRL0403/XRL0403_unit.bp' \
             , LODs = \
               { { MeshName = '/units/xrl0404/xrl0404_LOD0.scm', ShaderName = 'Insect' } \
               , { LODCutoff = 400, MeshName = '/units/xrl0404/xrl0404_LOD1.scm', ShaderName = 'Insect' } \
               } }",
        )
        .exec()
        .unwrap();
        let i = render_index(meg, "/units/xrl0403/xrl0403_mesh").unwrap();
        let r = meg.app_data_ref::<MeshRegistry>().unwrap();
        let a = &r.assets[i as usize];
        assert_eq!(a.mesh, "/units/xrl0404/xrl0404_lod0.scm");
        assert_eq!(a.albedo, "/units/xrl0403/xrl0403_albedo.dds");
        assert_eq!(a.specteam, "/units/xrl0403/xrl0403_specteam.dds");
        assert_eq!(a.normals, "/units/xrl0403/xrl0403_normalsts.dds");
        let (_, level) = a.lods.iter().find(|(_, idx)| *idx != i).unwrap();
        let l = &r.assets[*level as usize];
        assert_eq!(l.mesh, "/units/xrl0404/xrl0404_lod1.scm");
        assert_eq!(l.albedo, "/units/xrl0403/xrl0403_albedo.dds", "the Source stem's, as for LOD0");
        assert_eq!(l.normals, "/units/xrl0403/xrl0403_normalsts.dds", "never the `_lod1` file");
        drop(r);

        let bp = meg
            .app_data_ref::<super::BlueprintRegistry>()
            .unwrap()
            .get(BlueprintKind::Mesh, "/units/xrl0403/xrl0403_mesh")
            .unwrap()
            .table
            .clone();
        assert_eq!(source_stem(&Some(bp)).unwrap().as_deref(), Some("/units/xrl0403/xrl0403"));

        // `spherez_prop` shares another mesh's albedo ("share other rock's
        // albedo and specteam"): the mesh file's stem wins over the Source's.
        lua.load(
            "RegisterMeshBlueprint \
             { BlueprintId = '/env/common/props/spherez_prop_mesh' \
             , Source = '/env/common/props/spherez_prop.bp' \
             , LODs = { { MeshName = 'sphere_LOD0.scm', ShaderName = 'TMeshZFill' } } }",
        )
        .exec()
        .unwrap();
        let i = render_index(&lua, "/env/common/props/spherez_prop_mesh").unwrap();
        let r = lua.app_data_ref::<MeshRegistry>().unwrap();
        assert_eq!(r.assets[i as usize].albedo, "/env/common/props/sphere_albedo.dds");
        drop(r);

        // A standalone mesh blueprint without a `Source` still names its
        // textures after the mesh file's stem (`/meshes/game/arrow_mesh`).
        lua.load(
            "RegisterMeshBlueprint \
             { BlueprintId = '/meshes/game/arrow_mesh', LODs = { { MeshName = '/meshes/game/arrow_LOD0.scm' } } }",
        )
        .exec()
        .unwrap();
        let i = render_index(&lua, "/meshes/game/arrow_mesh").unwrap();
        let r = lua.app_data_ref::<MeshRegistry>().unwrap();
        let a = &r.assets[i as usize];
        assert_eq!(a.mesh, "/meshes/game/arrow_lod0.scm");
        assert_eq!(a.albedo, "/meshes/game/arrow_albedo.dds");
        assert_eq!(a.normals, "/meshes/game/arrow_normalsts.dds");
    }

    // Moho's `RMeshBlueprintLOD::Init` gives every level its own
    // technique and names, defaulting an empty texture to the Source stem's
    // (LOD0's art); XSL0105's LOD1 names its albedo and specteam only.
    #[test]
    fn lod_levels_take_their_own_material() {
        use super::{MeshRegistry, render_index};

        let lua = mlua::Lua::new();
        super::super::blueprints::register(&lua).unwrap();
        let vfs = crate::engine::vfs::Vfs::new_with_embedded(
            vec![],
            vec![],
            &[
                ("/units/xsl0105/xsl0105_lod0.scm", &[]),
                ("/units/xsl0105/xsl0105_lod1.scm", &[]),
                ("/units/xsl0105/xsl0105_albedo.dds", &[]),
                ("/units/xsl0105/xsl0105_specteam.dds", &[]),
                ("/units/xsl0105/xsl0105_normalsTS.dds", &[]),
                ("/units/xsl0105/xsl0105_lod1_albedo.dds", &[]),
                ("/units/xsl0105/xsl0105_lod1_specteam.dds", &[]),
                ("/units/xsl0105/xsl0105_lod1_normalsTS.dds", &[]),
            ],
        )
        .unwrap();
        lua.set_app_data(std::sync::Arc::new(std::sync::RwLock::new(vfs)));
        lua.load(
            "RegisterMeshBlueprint \
             { BlueprintId = '/units/xsl0105/xsl0105_mesh' \
             , Source = '/units/XSL0105/XSL0105_unit.bp' \
             , LODs = \
               { { LODCutoff = 80, ShaderName = 'Seraphim', LookupName = '/textures/environment/Falloff_seraphim_lookup.dds' } \
               , { LODCutoff = 215, ShaderName = 'Unit', AlbedoName = 'xsl0105_lod1_albedo.dds', SpecularName = 'xsl0105_lod1_specteam.dds' } \
               } }",
        )
        .exec()
        .unwrap();
        let i = render_index(&lua, "/units/xsl0105/xsl0105_mesh").unwrap();
        let r = lua.app_data_ref::<MeshRegistry>().unwrap();
        let a = &r.assets[i as usize];
        let (_, level) = a.lods.iter().find(|(_, idx)| *idx != i).unwrap();
        let l = &r.assets[*level as usize];
        assert_eq!(l.mesh, "/units/xsl0105/xsl0105_lod1.scm");
        assert_eq!(l.albedo, "/units/xsl0105/xsl0105_lod1_albedo.dds");
        assert_eq!(l.specteam, "/units/xsl0105/xsl0105_lod1_specteam.dds");
        assert_eq!(l.normals, "/units/xsl0105/xsl0105_normalsts.dds", "LOD0's map, not the `_lod1` one beside it");
        assert_eq!(l.shading, super::Shading::from_shader("Unit"), "its own technique");
        assert_ne!(l.shading, a.shading);
        assert_eq!(l.lookup, a.lookup, "no `_lookup.dds` ships: LOD0's");
    }

    // A tree group's far LOD (`VertexNormal`, alpha-tested `0x23` in
    // `mesh.fx`) is a card cut out of its albedo, not an opaque square.
    #[test]
    fn vertex_normal_lods_are_cut_out() {
        use super::{MeshRegistry, render_index};

        let lua = mlua::Lua::new();
        super::super::blueprints::register(&lua).unwrap();
        let vfs = crate::engine::vfs::Vfs::new_with_embedded(
            vec![],
            vec![],
            &[
                ("/env/trees/group_lod0.scm", &[]),
                ("/env/trees/group_lod1.scm", &[]),
                ("/env/trees/group_albedo.dds", &[]),
                ("/env/trees/group_lod1_albedo.dds", &[]),
            ],
        )
        .unwrap();
        lua.set_app_data(std::sync::Arc::new(std::sync::RwLock::new(vfs)));
        lua.load(
            "RegisterMeshBlueprint \
             { BlueprintId = '/env/trees/group_mesh' \
             , LODs = \
               { { LODCutoff = 165, ShaderName = 'TMeshAlpha' } \
               , { LODCutoff = 640, ShaderName = 'VertexNormal', AlbedoName = 'group_lod1_albedo.dds' } \
               } }",
        )
        .exec()
        .unwrap();
        let i = render_index(&lua, "/env/trees/group_mesh").unwrap();
        let r = lua.app_data_ref::<MeshRegistry>().unwrap();
        let a = &r.assets[i as usize];
        let (_, level) = a.lods.iter().find(|(_, idx)| *idx != i).unwrap();
        let l = &r.assets[*level as usize];
        assert_eq!(l.mesh, "/env/trees/group_lod1.scm");
        assert!(a.cutout && l.cutout);
    }
}
