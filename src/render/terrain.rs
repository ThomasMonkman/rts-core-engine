//! Map terrain as FA/FAF draws it: the `.scmap`'s texture strata blended by
//! its masks, lit by the map's sun, tinted under water by the water ramp,
//! with albedo and normal decals, and a water surface.
//!
//! Like FA (`effects/terrain.fx`) it takes two passes. The normals pass
//! blends the strata normal maps (UDN, onto the heightfield or FAF's terrain
//! normal texture) into a screen-sized normal buffer, and normal decals blend
//! into that buffer. The albedo pass blends the strata albedos, reads the
//! normal back at its pixel and lights it; albedo decals then draw lit the
//! same way. Units, water and overlays follow in the same pass.
//!
//! The map's terrain shader picks the features ([`ShaderKind::parse`]):
//! - `TTerrain`: 4 strata, albedo masks use the upper half of the range
//!   (`saturate(mask * 2 - 1)`), white specular from `1 - albedo.a`;
//! - `TTerrainXP`: 8 strata, half-range albedo masks, specular
//!   `albedo.a * SpecularColor`;
//! - FAF `TTerrainXPExt` / `TerrainABC[B]`: stratum 7's slots hold the
//!   terrain info texture (shadow a, ambient occlusion g) and the terrain
//!   normal texture (`.ag`); `A` = 0 legacy lighting, 1 PBR, 2 PBR with
//!   height-based splatting (heights and roughness from the 2x2 atlas in the
//!   upper slot, even strata rotated); `B` = 0 full mask range, 5 half;
//!   `C` = lower albedo blended on top as a macro texture (1 lerp, 2
//!   multiply).
//!
//! FA's lighting maths runs in gamma space (D3D9, no sRGB), so textures
//! sample as UNORM and the result is converted for the sRGB target.
//! Textures load on a background thread; until they arrive the scene keeps
//! its plain height-coloured terrain.

use super::gpu_prof::Timed;
use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

use crate::engine::assets::dds;
use crate::engine::assets::scmap::{self, decal_kind, MapRender};
use crate::engine::snapshot::TerrainView;
use crate::engine::vfs::Vfs;
use crate::render::camera::LodPlane;

pub const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The light buffer: shadow, ambient occlusion and water depth a pixel
/// (`light_at` in the shader), written by the normals pass.
pub const LIGHT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Terrain shader features, from the map's shader name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderKind {
    /// Strata blended (excluding lower and upper).
    pub strata: u32,
    pub albedo_half: bool,
    pub normal_half: bool,
    /// Stratum 7 slots hold the terrain info and terrain normal textures.
    pub info: bool,
    pub pbr: bool,
    pub height_splat: bool,
    /// TTerrain's white specular from `1 - albedo.a`.
    pub tterrain_spec: bool,
    /// Upper albedo is a macro texture (not the PBR atlas).
    pub upper_macro: bool,
    /// Lower albedo blended on top: 0 none, 1 lerp, 2 multiply.
    pub lower_macro: u32,
}

impl ShaderKind {
    pub fn parse(name: &str) -> Self {
        let xp = Self { strata: 8, albedo_half: true, normal_half: false, info: false, pbr: false, height_splat: false, tterrain_spec: false, upper_macro: true, lower_macro: 0 };
        match name {
            "TTerrain" => Self { strata: 4, tterrain_spec: true, ..xp },
            "TTerrainXP" => xp,
            "TTerrainXPExt" => Self { strata: 7, info: true, ..xp },
            n => {
                let digits: Vec<u32> = n.strip_prefix("Terrain").unwrap_or("").chars().take(3).filter_map(|c| c.to_digit(10)).collect();
                match digits.as_slice() {
                    [a, b, c] => Self {
                        strata: 7,
                        albedo_half: *b == 5,
                        normal_half: *b == 5,
                        info: true,
                        pbr: *a >= 1,
                        height_splat: *a >= 2,
                        tterrain_spec: false,
                        upper_macro: *a == 0,
                        lower_macro: *c,
                    },
                    _ => xp,
                }
            }
        }
    }

    fn bits(&self) -> u32 {
        (self.albedo_half as u32)
            | (self.normal_half as u32) << 1
            | (self.info as u32) << 2
            | (self.pbr as u32) << 3
            | (self.height_splat as u32) << 4
            | (self.tterrain_spec as u32) << 5
            | (self.upper_macro as u32) << 6
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TerrainParams {
    /// Repeats across the map per stratum: lower, 0-7, upper.
    albedo_tile: [[f32; 4]; 3],
    /// Lower, 0-7.
    normal_tile: [[f32; 4]; 3],
    /// 1/w, 1/h, w, h.
    size: [f32; 4],
    /// Toward the sun; w = lighting multiplier.
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    ambience: [f32; 4],
    shadow_fill: [f32; 4],
    specular: [f32; 4],
    /// Surface, deep, abyss, has water.
    water: [f32; 4],
    /// Strata, feature bits, lower macro variant, advanced water.
    flags: [u32; 4],
    /// Per wave layer: repeat, movement x, movement y.
    waves: [[f32; 4]; 4],
    /// Water colour lerp (min, max).
    water_lerp: [f32; 4],
    /// Toward the water's sun.
    water_sun_dir: [f32; 4],
    /// Water surface colour; w = sun shininess.
    water_color: [f32; 4],
    /// Water sun colour; w = sun strength.
    water_sun: [f32; 4],
    /// Fresnel bias, power, sky reflection, 0.
    water_fresnel: [f32; 4],
    sky: [f32; 4],
}

/// Decoded map assets (read on the global pool).
struct Loaded {
    render: MapRender,
    albedo: Vec<Option<dds::Texture>>,
    normals: Vec<Option<dds::Texture>>,
    masks: [Option<dds::Texture>; 2],
    water_map: Option<dds::Texture>,
    ramp: Option<dds::Texture>,
    waves: [Option<dds::Texture>; 4],
    /// Sky reflection cube map (water).
    cube: Option<[dds::Texture; 6]>,
    /// Units' environment cube (reflections, ambient) and FAF's BRDF lookup
    /// (`/textures/engine/anisotropiclookup.dds`, mesh.fx's
    /// `anisotropicTexture`).
    environment: Option<[dds::Texture; 6]>,
    brdf: Option<dds::Texture>,
    /// mesh.fx's `insectTexture` (`/textures/engine/insectlookup.dds`).
    insect: Option<dds::Texture>,
    decals: BTreeMap<String, Option<dds::Texture>>,
    failures: Vec<String>,
    horizon: Horizon,
    /// The map's sky (moho's defaults without one) and its textures.
    sky: (scmap::SkyBox, super::sky::SkyTextures),
}

/// The terrain's own sun shadow: `size` texels over the map, 255 = lit.
pub struct Horizon {
    pub size: (u32, u32),
    pub texels: Vec<u8>,
}

/// Bakes the terrain's shadow on itself: from each texel, march toward the
/// sun over the heightfield and compare the steepest rise met with the
/// sun's elevation (a small slope band gives soft edges). The sun never
/// moves in FA, so this is done once per map.
pub fn horizon_map(hm: &scmap::Heightmap, sun: Vec3) -> Horizon {
    use rayon::prelude::*;
    let (w, h) = (hm.width, hm.height);
    // A texel per cell (per 2 on maps over 20 km: 81 km maps are 4096).
    let step = if w.max(h) > 1024 { 2.0 } else { 1.0 };
    let (tw, th) = (((w as f32) / step) as u32, ((h as f32) / step) as u32);
    let at = |x: f32, z: f32| -> f32 {
        let (x, z) = (x.clamp(0.0, w as f32), z.clamp(0.0, h as f32));
        let (x0, z0) = (x.floor().min(w as f32 - 1.0) as u32, z.floor().min(h as f32 - 1.0) as u32);
        let (fx, fz) = (x - x0 as f32, z - z0 as f32);
        let s = |xi: u32, zi: u32| hm.samples[(zi * (w + 1) + xi) as usize] as f32 * hm.scale;
        let a = s(x0, z0) + (s(x0 + 1, z0) - s(x0, z0)) * fx;
        let b = s(x0, z0 + 1) + (s(x0 + 1, z0 + 1) - s(x0, z0 + 1)) * fx;
        a + (b - a) * fz
    };
    let top = hm.samples.iter().copied().max().unwrap_or(0) as f32 * hm.scale;
    let flat = Vec3::new(sun.x, 0.0, sun.z);
    let len = flat.length();
    if len < 1e-3 || sun.y <= 0.0 {
        // Sun overhead (or below the horizon): nothing / everything shades.
        return Horizon { size: (tw, th), texels: vec![if sun.y > 0.0 { 255 } else { 0 }; (tw * th) as usize] };
    }
    let dir = flat / len;
    let sun_slope = sun.y / len;
    // Penumbra: slopes this close to the sun's read as partly lit.
    let band = 0.12;
    let texels = (0..th)
        .into_par_iter()
        .flat_map_iter(|j| {
            (0..tw).map(move |i| {
                let (x, z) = ((i as f32 + 0.5) * step, (j as f32 + 0.5) * step);
                let h0 = at(x, z);
                let mut steepest = f32::MIN;
                let mut t = step;
                // Past where the sun ray clears the highest point nothing can shade.
                while h0 + sun_slope * t <= top + 0.5 {
                    let (qx, qz) = (x + dir.x * t, z + dir.z * t);
                    if qx < 0.0 || qz < 0.0 || qx > w as f32 || qz > h as f32 {
                        break;
                    }
                    steepest = steepest.max((at(qx, qz) - h0) / t);
                    t += step;
                }
                (((sun_slope - steepest) / band + 0.5).clamp(0.0, 1.0) * 255.0) as u8
            })
        })
        .collect();
    Horizon { size: (tw, th), texels }
}

fn load(vfs: &RwLock<Vfs>, path: &str, bc: bool) -> std::io::Result<Loaded> {
    let _s = rts_core_engine::prof_span!("load map textures");
    let bytes = vfs.read().ok().and_then(|v| v.read(path).ok()).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, path.to_owned()))?;
    let (terrain, render) = scmap::read_render(&bytes)?;
    let horizon = {
        let _s = rts_core_engine::prof_span!("horizon map");
        let started = std::time::Instant::now();
        let h = horizon_map(&terrain.heightmap, Vec3::from_array(render.lighting.sun_direction));
        tracing::info!(ms = started.elapsed().as_millis() as u64, size = ?h.size, "terrain horizon map baked");
        h
    };
    let failures = std::sync::Mutex::new(Vec::new());
    let file = |p: &str| -> Option<dds::Texture> {
        if p.is_empty() {
            return None;
        }
        let fail = |e: String| {
            tracing::warn!("map texture {p}: {e}");
            failures.lock().unwrap().push(format!("map texture {p}: {e}"));
        };
        let (b, fingerprint) = {
            let vfs = vfs.read().ok()?;
            match vfs.read(p) {
                Ok(b) => (b, vfs.fingerprint(p)),
                Err(e) => {
                    fail(e.to_string());
                    return None;
                }
            }
        };
        let mut t = dds::read(&b, bc).map_err(|e| fail(e.to_string())).ok()?;
        let slot = super::texture_load::mip_cache().and_then(|c| super::texture_load::MipSlot::new(c, fingerprint, p, false, bc));
        super::texture_load::fill_mips(&mut t, false, bc, &format!("map texture {p}"), slot);
        Some(t)
    };
    // The map's own textures: cached by their bytes.
    let blob = |b: &[u8], what: &str| {
        let mut t = if b.is_empty() { None } else { dds::read(b, bc).ok() }?;
        let slot = super::texture_load::mip_cache().and_then(|c| super::texture_load::MipSlot::new(c, Some(xxhash_rust::xxh3::xxh3_64(b)), what, false, bc));
        super::texture_load::fill_mips(&mut t, false, bc, what, slot);
        Some(t)
    };
    // Every texture is independent: decode in parallel (order kept by index).
    use rayon::prelude::*;
    let albedo: Vec<_> = render.albedo.par_iter().map(|s| file(&s.path)).collect();
    let normals: Vec<_> = render.normals.par_iter().map(|s| file(&s.path)).collect();
    let mut decal_paths: Vec<&str> = render
        .decals
        .iter()
        .filter(|d| matches!(d.kind, decal_kind::ALBEDO | decal_kind::NORMALS | decal_kind::NORMALS_ALPHA))
        .flat_map(|d| [d.textures.first(), spec_path(d)].into_iter().flatten().map(String::as_str))
        .filter(|p| !p.is_empty())
        .collect();
    decal_paths.sort_unstable();
    decal_paths.dedup();
    let decals: BTreeMap<String, Option<dds::Texture>> = decal_paths.par_iter().map(|p| (p.to_string(), file(p))).collect::<Vec<_>>().into_iter().collect();
    let water = render.water.clone().unwrap_or_default();
    let sky = render.sky.clone().unwrap_or_else(|| super::sky::default_sky((terrain.header.width, terrain.header.height)));
    let sky_textures = super::sky::SkyTextures::read(&sky, file);
    Ok(Loaded {
        sky: (sky, sky_textures),
        masks: [blob(&render.mask_low, "map: strata mask (low)"), blob(&render.mask_high, "map: strata mask (high)")],
        water_map: blob(&render.water_map, "map: water map"),
        ramp: file(&water.ramp),
        waves: [file(&water.waves[0].2), file(&water.waves[1].2), file(&water.waves[2].2), file(&water.waves[3].2)],
        cube: [water.cube_map.as_str(), render.sky_cube.as_str()].iter().find_map(|p| {
            let b = vfs.read().ok()?.read(p).ok()?;
            dds::read_cube(&b, bc).ok().flatten()
        }),
        environment: {
            let named = render.environment_cubes.iter().find(|(n, _)| n == "<default>").or(render.environment_cubes.first()).map(|(_, p)| p.as_str());
            [named.unwrap_or(""), "/textures/environment/defaultenvcube.dds"].iter().filter(|p| !p.is_empty()).find_map(|p| {
                let b = vfs.read().ok()?.read(&p.to_lowercase()).ok()?;
                dds::read_cube(&b, bc).ok().flatten()
            })
        },
        // (Uncompressed: the lookup must stay exact.)
        brdf: vfs.read().ok().and_then(|v| v.read("/textures/engine/anisotropiclookup.dds").ok()).and_then(|b| dds::read(&b, false).ok()),
        insect: vfs.read().ok().and_then(|v| v.read("/textures/engine/insectlookup.dds").ok()).and_then(|b| dds::read(&b, false).ok()),
        albedo,
        normals,
        decals,
        render,
        failures: failures.into_inner().unwrap_or_default(),
        horizon,
    })
}

/// One decal as the vertex shader reads it (an instance): it builds the
/// decal's grid of terrain cells from the height buffer at this frame's
/// level of detail and projects the texture onto it (`vs_decal`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq)]
pub struct DecalInstance {
    /// Its position (the texture's origin, uv 0,0); its `LODCutoff`: view
    /// depth it fades out by (0: never).
    pub origin_cutoff: [f32; 4],
    /// Rows x and z of its matrix ([`decal_matrix`]); 1 / scale x and z.
    pub row_u: [f32; 4],
    pub row_v: [f32; 4],
    /// The footprint's samples (x0, z0, x1, z1), on the mesh's grid.
    pub bounds: [u32; 4],
    /// Heading (the normal decals' tangent frame turns by it); unused.
    pub heading: [f32; 4],
}

impl DecalInstance {
    /// Its texture coordinates at world `p` (as `vs_decal`).
    #[allow(dead_code)]
    pub fn uv(&self, p: Vec3) -> [f32; 2] {
        let d = p - Vec3::new(self.origin_cutoff[0], self.origin_cutoff[1], self.origin_cutoff[2]);
        let (u, v) = (Vec3::new(self.row_u[0], self.row_u[1], self.row_u[2]), Vec3::new(self.row_v[0], self.row_v[1], self.row_v[2]));
        [u.dot(d) * self.row_u[3], v.dot(d) * self.row_v[3]]
    }

    /// Its cells at a level of detail of sample step `step` on a map
    /// `width` x `height` cells (as `vs_decal`): the footprint widened to
    /// that step's grid, (first column, first row, columns, rows).
    pub fn cells(&self, step: u32, width: u32, height: u32) -> [u32; 4] {
        let [x0, z0, x1, z1] = self.bounds;
        let (i0, j0) = (x0 / step, z0 / step);
        let (i1, j1) = (x1.div_ceil(step).min(width / step), z1.div_ceil(step).min(height / step));
        [i0, j0, i1.saturating_sub(i0), j1.saturating_sub(j0)]
    }
}

/// A tread mark as the vertex shader reads it (an instance): it builds the
/// mark's grid of terrain cells from the height buffer (`vs_tread`), born
/// and faded by game time (`g.params.z`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TreadInstance {
    /// Centre (x, z); half extents across and along.
    centre_half: [f32; 4],
    /// Heading; game seconds it appears at, and lives; the camera depth
    /// it's drawn to (`lodParam`; 0: any).
    heading_time: [f32; 4],
}

/// A timed splat to lay (FAF's tread marks and footprints).
#[derive(Clone, Debug, PartialEq)]
pub struct TreadMark {
    pub texture: String,
    /// Centre on the ground (x, z).
    pub centre: [f32; 2],
    /// Radians: the mark's z axis points along (sin, cos).
    pub heading: f32,
    /// Full extent across (x) and along (z).
    pub size: [f32; 2],
    /// Game seconds.
    pub born: f32,
    pub lifetime: f32,
    /// Camera depth beyond which it isn't drawn (the splat's `lodParam`;
    /// 0: always drawn).
    pub lod: f32,
}

/// Marks each tread texture keeps; the oldest is overwritten first.
const TREAD_RING: u32 = 16384;

/// One texture's tread marks: a ring of instances, all drawn every frame
/// (the shader hides those not born yet or faded).
struct TreadRing {
    group: usize,
    instances: wgpu::Buffer,
    head: u32,
    /// Marks laid so far, up to the ring's size.
    count: u32,
    /// The largest half diagonal laid: sizes the grid every instance draws.
    radius: f32,
}

#[derive(Default)]
struct Treads {
    rings: BTreeMap<String, TreadRing>,
    /// Marks whose texture was still loading when laid: retried each
    /// `lay_treads` until it's in (dropped if it fails, or once their
    /// lifetime has run out). Without this a one-off splat (`CreateSplat`)
    /// with a texture not seen before never showed: its own request is what
    /// starts the load.
    waiting: Vec<TreadMark>,
}

/// A run of decals drawn in one call: same texture and pass, adjacent
/// instances, each drawing `vertices` (the run's largest grid; smaller ones
/// collapse their extra vertices).
#[derive(Clone)]
struct DecalDraw {
    texture: usize,
    normals: bool,
    instances: std::ops::Range<u32>,
    vertices: u32,
    /// The run's smallest and largest grids (cells).
    cells: (u32, u32),
}

/// One decal: its instance, with its bounding sphere for the per-frame cull.
struct DecalPiece {
    texture: usize,
    normals: bool,
    instance: u32,
    centre: Vec3,
    radius: f32,
    /// Its `LODCutoff` (0: drawn at any distance).
    cutoff: f32,
}

/// Decals on the terrain, built once in file order as instances
/// ([`DecalInstance`]); the vertex shader lays each one's grid of terrain
/// cells at this frame's level of detail, so a decal costs 80 bytes rather
/// than a mesh. Each frame [`DecalLayer::cull`] picks those in view and
/// within their `LODCutoff` and merges consecutive ones with the same
/// texture and pass into one instanced draw when their grids are within 2x
/// of each other (a big map carries thousands of decals, and recording a
/// draw per decal per pass was once most of a frame's command cost). The
/// cutoff fades over its last quarter in the shader, as FAF sets moho's
/// `ren_DecalFadeFraction`.
struct DecalLayer {
    instances: wgpu::Buffer,
    /// The instances (the per-frame vertex counts come from their grids).
    records: Vec<DecalInstance>,
    pieces: Vec<DecalPiece>,
    /// The map's size in cells (the grids' clamp).
    size: (u32, u32),
}

impl DecalLayer {
    /// Builds `decals` (albedo and normal kinds `texture` gives a bind group
    /// for: its texture and an albedo decal's spec map; `None` skips it for
    /// now), in order.
    fn build(device: &wgpu::Device, view: &TerrainView, decals: &[scmap::Decal], mut texture: impl FnMut(&scmap::Decal) -> Option<usize>) -> Self {
        let (mut records, mut pieces) = (Vec::new(), Vec::new());
        for d in decals {
            let normals = matches!(d.kind, decal_kind::NORMALS | decal_kind::NORMALS_ALPHA);
            if !(normals || d.kind == decal_kind::ALBEDO) {
                continue;
            }
            let Some(tex) = texture(d) else { continue };
            let Some((record, _, _)) = decal_instance(view, d) else { continue };
            // Big decals in tiles, each culled on its own (a 400-unit mud
            // decal is 160k cells; most of it is off screen).
            for (record, centre, radius) in decal_tiles(view, record) {
                pieces.push(DecalPiece { texture: tex, normals, instance: records.len() as u32, centre, radius, cutoff: d.cutoff.max(0.0) });
                records.push(record);
            }
        }
        let contents: Vec<DecalInstance> = if records.is_empty() { vec![DecalInstance::zeroed()] } else { records.clone() };
        Self {
            instances: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("decals: instances"), contents: bytemuck::cast_slice(&contents), usage: wgpu::BufferUsages::VERTEX }),
            records,
            pieces,
            size: (view.width, view.height),
        }
    }

    /// `draws` from `pieces` in order at sample step `step`: a piece with
    /// the same texture and pass right after the previous run extends it
    /// when the run's grids stay within 2x (each pass draws its runs in
    /// order, so blending is unchanged).
    fn merge<'a>(&self, draws: &mut Vec<DecalDraw>, pieces: impl Iterator<Item = &'a DecalPiece>, step: u32) {
        draws.clear();
        for p in pieces {
            let [_, _, cols, rows] = self.records[p.instance as usize].cells(step, self.size.0, self.size.1);
            let cells = cols * rows;
            if cells == 0 {
                continue;
            }
            match draws.iter_mut().rev().find(|r| r.normals == p.normals) {
                Some(r) if r.texture == p.texture && r.instances.end == p.instance && cells.max(r.cells.1) <= 2 * cells.min(r.cells.0) => {
                    r.instances.end += 1;
                    r.cells = (r.cells.0.min(cells), r.cells.1.max(cells));
                    r.vertices = r.cells.1 * 6;
                }
                _ => draws.push(DecalDraw { texture: p.texture, normals: p.normals, instances: p.instance..p.instance + 1, vertices: cells * 6, cells: (cells, cells) }),
            }
        }
    }

    /// This frame's draws at sample step `step`: the pieces inside the
    /// frustum `planes` (each `(n, d)`, `n . p + d >= 0` inside) and not
    /// wholly beyond their cutoff on the LOD plane (`lod`).
    fn cull(&self, planes: &[glam::Vec4; 6], lod: LodPlane, step: u32, draws: &mut Vec<DecalDraw>) {
        let visible = |p: &DecalPiece| {
            if p.cutoff > 0.0 && lod.metric(p.centre) - p.radius * lod.scale > p.cutoff {
                return false;
            }
            planes.iter().all(|pl| pl.dot(p.centre.extend(1.0)) >= -p.radius)
        };
        self.merge(draws, self.pieces.iter().filter(|p| visible(p)), step);
    }
}

/// A world view's decal draws this frame ([`TerrainRenderer::cull_decals`]):
/// per layer (the map's, the sim's, the interface's), and the terrain's
/// sample step their grids (and the tread marks') are built at.
#[derive(Default)]
pub struct DecalDraws {
    step: u32,
    layers: [Vec<DecalDraw>; 3],
}

/// The sim's ground decals (tarmacs, deposit markers, scorch marks): its
/// latest list, built into a layer over the map's once the map is ready.
#[derive(Default)]
struct ScriptDecals {
    source: Option<Arc<Vec<scmap::Decal>>>,
    dirty: bool,
    layer: Option<DecalLayer>,
    /// The interface's decals (`UserDecal`: range rings under the cursor),
    /// a layer of their own so a moving cursor rebuilds only them.
    user_source: Option<Arc<Vec<scmap::Decal>>>,
    user_dirty: bool,
    user_layer: Option<DecalLayer>,
    textures: BTreeMap<String, DecalTexture>,
    /// Uploaded textures (`DecalTexture::Ready` indexes these).
    views: Vec<wgpu::TextureView>,
    /// Bind groups by (texture, spec map; "" for none).
    group_of: BTreeMap<(String, String), usize>,
    groups: Vec<wgpu::BindGroup>,
    /// Decoded textures from the global pool: (path, texture).
    /// Uploaded by the loader.
    loaded: Option<(std::sync::mpsc::Sender<(String, Option<wgpu::TextureView>)>, Receiver<(String, Option<wgpu::TextureView>)>)>,
}

/// A script decal texture: read and decoded off the render thread (a first
/// sight on the render thread hitched frames by 15-30 ms); decals wait,
/// undrawn, until theirs is in.
enum DecalTexture {
    Loading,
    Ready(usize),
    Failed,
}

/// An albedo decal's spec map (its second texture), if it names one:
/// TTerrain lights it from `.r`, XP from `.a`, PBR's roughness is
/// `0.9 (1 - r)` (FA's `DecalsPS` / `DecalAlbedoXP`).
fn spec_path(d: &scmap::Decal) -> Option<&String> {
    (d.kind == decal_kind::ALBEDO).then(|| d.textures.get(1)).flatten().filter(|p| !p.is_empty())
}

/// A decal bind group: its texture and spec map.
fn decal_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, texture: &wgpu::TextureView, spec: &wgpu::TextureView, name: &str) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("decal: {name}")),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(texture) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(spec) }],
    })
}

/// GPU state once the map's textures are in.
struct Ready {
    params: wgpu::Buffer,
    textures: wgpu::BindGroup,
    /// The textures' views in binding order, kept to rebind them with a
    /// new sampler (Texture Detail).
    views: Vec<wgpu::TextureView>,
    decals: DecalLayer,
    /// Per (texture, spec map) pair: its bind group.
    decal_groups: Vec<wgpu::BindGroup>,
    water: Option<f32>,
    sun: [f32; 3],
    /// The terrain's own shadow (R8, over the whole map).
    horizon: wgpu::TextureView,
    sky: super::sky::Sky,
    /// Units' environment cube and BRDF lookup (see `Loaded`).
    environment: Option<(wgpu::TextureView, u32)>,
    brdf: Option<wgpu::TextureView>,
    insect: Option<wgpu::TextureView>,
    lighting: scmap::Lighting,
    /// Shoreline wave generators (drawn as particles, `particles.rs`).
    waves: Arc<Vec<scmap::WaveGenerator>>,
}

/// The strata's sampler (tiled): `mip_skip` finest levels never sampled
/// (`ren_MipSkipLevels`, FAF's Texture Detail).
fn wrap_sampler(device: &wgpu::Device, mip_skip: u32) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("terrain: wrap sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        lod_min_clamp: mip_skip as f32,
        anisotropy_clamp: 8,
        ..Default::default()
    })
}

pub struct TerrainRenderer {
    pending: Option<Receiver<std::io::Result<Loaded>>>,
    ready: Option<Ready>,
    requested: Option<String>,
    texture_layout: wgpu::BindGroupLayout,
    decal_layout: wgpu::BindGroupLayout,
    /// A decal's spec map when it has none: black (no specular; PBR rough).
    no_spec: wgpu::TextureView,
    normal_layout: wgpu::BindGroupLayout,
    normals_pipeline: wgpu::RenderPipeline,
    albedo_pipeline: wgpu::RenderPipeline,
    decal_normals_pipeline: wgpu::RenderPipeline,
    decal_albedo_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    wrap: wgpu::Sampler,
    clamp: wgpu::Sampler,
    /// The sky's pipelines (each map's sky is in `Ready`).
    sky: super::sky::SkyRenderer,
    normal_target: NormalTarget,
    bc: bool,
    /// For the decal loaders, which upload their textures themselves.
    gpu: (wgpu::Device, wgpu::Queue),
    /// Heights kept for decal meshes.
    view: Option<Arc<TerrainView>>,
    vfs: Option<Arc<RwLock<Vfs>>>,
    script: ScriptDecals,
    treads: Treads,
    tread_pipeline: wgpu::RenderPipeline,
    failures: Vec<String>,
}

const ALBEDO_SLOTS: usize = 10;
/// Bindings: albedo 0-9, normals 10-18, masks 19-20, water map 21, ramp 22,
/// waves 23-26, sky cube 27, wrap sampler 28, clamp sampler 29, params 30.
const BINDINGS: u32 = 31;

fn texture_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}

/// The normals pass's targets, and the bind group the lit passes read them
/// through (binding 0 the normals, 1 the light).
struct NormalTarget {
    normals: wgpu::TextureView,
    light: wgpu::TextureView,
    group: wgpu::BindGroup,
}

fn normal_target(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, w: u32, h: u32) -> NormalTarget {
    let target = |label: &str, format: wgpu::TextureFormat| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        super::view(&tex, label)
    };
    let normals = target("terrain: normals target", NORMAL_FORMAT);
    let light = target("terrain: light target", LIGHT_FORMAT);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain: normals + light targets"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&normals) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&light) },
        ],
    });
    NormalTarget { normals, light, group }
}

impl TerrainRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, globals: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, depth: wgpu::TextureFormat, w: u32, h: u32) -> Self {
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..27).map(|b| texture_entry(b, vf)).collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 27,
            visibility: vf,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry { binding: 28, visibility: vf, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None });
        entries.push(wgpu::BindGroupLayoutEntry { binding: 29, visibility: vf, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None });
        entries.push(wgpu::BindGroupLayoutEntry { binding: 30, visibility: vf, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None });
        debug_assert_eq!(entries.len() as u32, BINDINGS);
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("terrain: map textures layout"), entries: &entries });
        let decal_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("decals: textures layout"), entries: &[texture_entry(0, wgpu::ShaderStages::FRAGMENT), texture_entry(1, wgpu::ShaderStages::FRAGMENT)] });
        let target_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let normal_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("terrain: normals + light targets layout"), entries: &[target_entry(0), target_entry(1)] });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("terrain: shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let terrain_vertex = wgpu::VertexBufferLayout { array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3] };
        // Decals and tread marks are instances; their vertices come from the
        // vertex index and the height buffer.
        let decal_vertex = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<DecalInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Uint32x4, 4 => Float32x4] };
        let tread_vertex = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<TreadInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4] };
        let water_vertex = wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3] };
        let layout = |groups: &[Option<&wgpu::BindGroupLayout>]| device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("terrain: pipeline layout"), bind_group_layouts: groups, immediate_size: 0 });
        let normals_layout = layout(&[Some(globals), Some(&texture_layout)]);
        let albedo_layout = layout(&[Some(globals), Some(&texture_layout), Some(&normal_layout)]);
        let decal_normals_layout = layout(&[Some(globals), Some(&texture_layout), Some(&decal_layout)]);
        let decal_albedo_layout = layout(&[Some(globals), Some(&texture_layout), Some(&decal_layout), Some(&normal_layout)]);
        let depth_state = |write: bool, compare: wgpu::CompareFunction, bias: i32| wgpu::DepthStencilState {
            format: depth,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState { constant: bias, slope_scale: if bias != 0 { -1.0 } else { 0.0 }, clamp: 0.0 },
        };
        // The normals pass has the normal and light targets: the terrain
        // writes both, decals (blended) only blend into the normals. The
        // scene pass's pipelines also carry the glow target, which terrain
        // and water leave alone.
        let pipeline = |label: &str, layout: &wgpu::PipelineLayout, vs: &str, fs: &str, buffers: &[wgpu::VertexBufferLayout], target: wgpu::TextureFormat, blend: Option<wgpu::BlendState>, depth: wgpu::DepthStencilState| {
            let targets = if target == NORMAL_FORMAT {
                let light = if blend.is_none() { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::empty() };
                vec![Some(wgpu::ColorTargetState { format: target, blend, write_mask: wgpu::ColorWrites::ALL }), Some(wgpu::ColorTargetState { format: LIGHT_FORMAT, blend: None, write_mask: light })]
            } else {
                super::globals::scene_targets(target, blend, super::globals::GlowWrite::Keep).to_vec()
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &targets,
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(depth),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction as C;
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        Self {
            normals_pipeline: pipeline("terrain: normals + depth", &normals_layout, "vs_terrain", "fs_normals", std::slice::from_ref(&terrain_vertex), NORMAL_FORMAT, None, depth_state(true, C::Less, 0)),
            albedo_pipeline: pipeline("terrain: lit albedo", &albedo_layout, "vs_terrain", "fs_albedo", &[terrain_vertex], format, None, depth_state(false, C::LessEqual, 0)),
            // Decals lie on the terrain's own vertices: equal depth passes.
            decal_normals_pipeline: pipeline("decals: normals", &decal_normals_layout, "vs_decal", "fs_decal_normals", std::slice::from_ref(&decal_vertex), NORMAL_FORMAT, blend, depth_state(false, C::LessEqual, 0)),
            decal_albedo_pipeline: pipeline("decals: albedo", &decal_albedo_layout, "vs_decal", "fs_decal_albedo", &[decal_vertex], format, blend, depth_state(false, C::LessEqual, 0)),
            tread_pipeline: pipeline("tread marks + footprints", &decal_albedo_layout, "vs_tread", "fs_tread", &[tread_vertex], format, blend, depth_state(false, C::LessEqual, 0)),
            water_pipeline: pipeline("water: map", &normals_layout, "vs_water", "fs_water", &[water_vertex], format, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), depth_state(false, C::Less, 0)),
            wrap: wrap_sampler(device, 0),
            sky: super::sky::SkyRenderer::new(device, globals, format, depth),
            clamp: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("terrain: clamp sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                // Decals at a glancing view, as the strata.
                anisotropy_clamp: 8,
                ..Default::default()
            }),
            normal_target: normal_target(device, &normal_layout, w, h),
            texture_layout,
            decal_layout,
            // (wgpu zero-fills new textures: black, alpha 0.)
            no_spec: device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("decals: no spec map"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor { label: Some("decals: no spec map"), ..Default::default() }),
            normal_layout,
            pending: None,
            ready: None,
            requested: None,
            bc: device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            view: None,
            vfs: None,
            script: ScriptDecals::default(),
            gpu: (device.clone(), queue.clone()),
            treads: Treads::default(),
            failures: Vec::new(),
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        self.normal_target = normal_target(device, &self.normal_layout, w, h);
    }

    /// A new terrain arrived: start loading its map's textures (once per map).
    pub fn receive(&mut self, view: &Arc<TerrainView>, vfs: Option<&Arc<RwLock<Vfs>>>) {
        self.view = Some(view.clone());
        self.script.dirty = true;
        if let Some(v) = vfs {
            self.vfs = Some(v.clone());
        }
        let (Some(map), Some(vfs)) = (view.map.clone(), vfs) else { return };
        if self.requested.as_deref() == Some(map.as_str()) {
            return;
        }
        self.requested = Some(map.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let (vfs, bc) = (vfs.clone(), self.bc);
        rayon::spawn(move || {
            let _ = tx.send(load(&vfs, &map, bc));
        });
        self.pending = Some(rx);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.is_some()
    }

    /// Textures tread marks or footprints have been laid with (render-state
    /// tests).
    #[allow(dead_code)]
    pub fn tread_textures(&self) -> Vec<String> {
        self.treads.rings.keys().cloned().collect()
    }

    /// Marks laid into the tread ring of each texture whose path contains
    /// `name` (render-state tests).
    #[allow(dead_code)]
    pub fn tread_count(&self, name: &str) -> u32 {
        self.treads.rings.iter().filter(|(t, _)| t.contains(name)).map(|(_, r)| r.count).sum()
    }

    /// Decals built into the map's layer and into the sim's (render-state
    /// tests).
    #[allow(dead_code)]
    pub fn decal_counts(&self) -> (usize, usize) {
        (self.ready.as_ref().map_or(0, |r| r.decals.pieces.len()), self.script.layer.as_ref().map_or(0, |l| l.pieces.len()))
    }

    /// A view's decal draws this frame, into `out`: those in its frustum
    /// (`planes`) and within their cutoff on its LOD plane (`lod`), their
    /// grids at the terrain's sample step `step` there.
    pub fn cull_decals(&self, planes: &[glam::Vec4; 6], lod: LodPlane, step: u32, out: &mut DecalDraws) {
        let _s = rts_core_engine::prof_span!("cull decals");
        out.step = step;
        let layers = [self.ready.as_ref().map(|r| &r.decals), self.script.layer.as_ref(), self.script.user_layer.as_ref()];
        for (layer, draws) in layers.into_iter().zip(&mut out.layers) {
            match layer {
                Some(layer) => layer.cull(planes, lod, step, draws),
                None => draws.clear(),
            }
        }
    }

    /// The map's textures (or script decal textures) are still loading.
    #[allow(dead_code)]
    pub fn loading(&self) -> bool {
        self.pending.is_some() || self.script.textures.values().any(|t| matches!(t, DecalTexture::Loading))
    }

    #[allow(dead_code)]
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// Uploads the map textures once loaded; writes this frame's params.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(Ok(mut l)) => {
                    let _s = rts_core_engine::prof_span!("upload map textures");
                    self.failures.append(&mut l.failures);
                    self.ready = self.upload(device, queue, l);
                    self.pending = None;
                }
                Ok(Err(e)) => {
                    tracing::warn!("map textures: {e}");
                    self.failures.push(format!("map: {e}"));
                    self.pending = None;
                }
                Err(_) => {}
            }
        }
        // Script decal textures that finished loading: upload, then the
        // layer is rebuilt with the decals that waited for them.
        if let Some((_, rx)) = &self.script.loaded {
            while let Ok((path, view)) = rx.try_recv() {
                let state = match view {
                    Some(v) => {
                        self.script.views.push(v);
                        DecalTexture::Ready(self.script.views.len() - 1)
                    }
                    None => {
                        self.failures.push(format!("decal {path}"));
                        DecalTexture::Failed
                    }
                };
                self.script.textures.insert(path, state);
                self.script.dirty = true;
                self.script.user_dirty = true;
            }
        }
        if self.script.dirty && self.ready.is_some() {
            self.build_script_decals(device);
        }
        if self.script.user_dirty && self.ready.is_some() {
            self.script.user_dirty = false;
            if let (Some(view), Some(source)) = (self.view.clone(), self.script.user_source.clone()) {
                let (vfs, bc) = (self.vfs.clone(), self.bc);
                let (script, gpu, layout, no_spec) = (&mut self.script, &self.gpu, &self.decal_layout, &self.no_spec);
                script.user_layer = (!source.is_empty()).then(|| DecalLayer::build(device, &view, &source, |d| request_group(script, gpu, device, layout, no_spec, &vfs, bc, d.textures.first()?, spec_path(d).map_or("", String::as_str))));
            }
        }
    }

    /// The interface's decals changed (`UserDecal`).
    pub fn set_user_decals(&mut self, decals: Arc<Vec<scmap::Decal>>) {
        self.script.user_source = Some(decals);
        self.script.user_dirty = true;
    }

    /// The sim's ground decals changed (snapshot `decals`).
    pub fn set_script_decals(&mut self, decals: Arc<Vec<scmap::Decal>>) {
        self.script.source = Some(decals);
        self.script.dirty = true;
    }

    /// Rebuilds the script decal layer; textures load on first use (a few
    /// small `.dds`: tarmacs, deposit markers, scorches).
    fn build_script_decals(&mut self, device: &wgpu::Device) {
        let _s = rts_core_engine::prof_span!("script decals");
        self.script.dirty = false;
        let (Some(view), Some(source)) = (self.view.clone(), self.script.source.clone()) else { return };
        let (vfs, bc) = (self.vfs.clone(), self.bc);
        let (script, gpu, layout, no_spec) = (&mut self.script, &self.gpu, &self.decal_layout, &self.no_spec);
        script.layer = Some(DecalLayer::build(device, &view, &source, |d| request_group(script, gpu, device, layout, no_spec, &vfs, bc, d.textures.first()?, spec_path(d).map_or("", String::as_str))));
    }

    /// Marks wait for their texture (`lay_treads` should run even with no
    /// new ones).
    pub fn treads_waiting(&self) -> bool {
        !self.treads.waiting.is_empty()
    }

    /// Lays tread marks and footprints at game seconds `now`, with those
    /// still waiting for their texture. A mark whose texture is loading
    /// waits; one whose texture failed, or whose lifetime is over, is
    /// dropped. Each texture's ring is written where its head is, in at
    /// most two runs.
    pub fn lay_treads(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, marks: &[TreadMark], now: f32) {
        if marks.is_empty() && self.treads.waiting.is_empty() {
            return;
        }
        let Some(view) = self.view.clone() else {
            self.treads.waiting.extend_from_slice(marks);
            return;
        };
        let _s = rts_core_engine::prof_span!("lay tread marks");
        let (vfs, bc) = (self.vfs.clone(), self.bc);
        let waiting = std::mem::take(&mut self.treads.waiting);
        let mut by_texture: BTreeMap<String, Vec<TreadInstance>> = BTreeMap::new();
        for m in waiting.iter().chain(marks) {
            if m.born + m.lifetime <= now {
                continue;
            }
            let Some(group) = request_group(&mut self.script, &self.gpu, device, &self.decal_layout, &self.no_spec, &vfs, bc, &m.texture, "") else {
                if matches!(self.script.textures.get(&m.texture), Some(DecalTexture::Loading)) {
                    self.treads.waiting.push(m.clone());
                }
                continue;
            };
            // No marks under water.
            if view.water.is_some_and(|w| height(&view, m.centre[0], m.centre[1]) < w) {
                continue;
            }
            let (hx, hz) = (m.size[0] / 2.0, m.size[1] / 2.0);
            let ring = self.treads.rings.entry(m.texture.clone()).or_insert_with(|| TreadRing {
                group,
                instances: device.create_buffer(&wgpu::BufferDescriptor { label: Some(&format!("tread marks: {}", m.texture)), size: (TREAD_RING as usize * std::mem::size_of::<TreadInstance>()) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
                head: 0,
                count: 0,
                radius: 0.0,
            });
            ring.radius = ring.radius.max(hx.hypot(hz));
            by_texture.entry(m.texture.clone()).or_default().push(TreadInstance { centre_half: [m.centre[0], m.centre[1], hx, hz], heading_time: [m.heading, m.born, m.lifetime, m.lod] });
        }
        let stride = std::mem::size_of::<TreadInstance>() as u64;
        for (texture, marks) in by_texture {
            let Some(ring) = self.treads.rings.get_mut(&texture) else { continue };
            // More than a ring's worth at once: keep the newest.
            let marks = &marks[marks.len().saturating_sub(TREAD_RING as usize)..];
            let n = marks.len() as u32;
            let first = n.min(TREAD_RING - ring.head);
            queue.write_buffer(&ring.instances, ring.head as u64 * stride, bytemuck::cast_slice(&marks[..first as usize]));
            if first < n {
                queue.write_buffer(&ring.instances, 0, bytemuck::cast_slice(&marks[first as usize..]));
            }
            ring.head = (ring.head + n) % TREAD_RING;
            ring.count = (ring.count + n).min(TREAD_RING);
        }
    }

    /// The map's textures (`views`, in binding order), samplers and
    /// parameters: the terrain's group.
    fn texture_group(&self, device: &wgpu::Device, views: &[wgpu::TextureView], params: &wgpu::Buffer) -> wgpu::BindGroup {
        let mut entries: Vec<wgpu::BindGroupEntry> = views.iter().enumerate().map(|(i, v)| wgpu::BindGroupEntry { binding: i as u32, resource: wgpu::BindingResource::TextureView(v) }).collect();
        entries.push(wgpu::BindGroupEntry { binding: 28, resource: wgpu::BindingResource::Sampler(&self.wrap) });
        entries.push(wgpu::BindGroupEntry { binding: 29, resource: wgpu::BindingResource::Sampler(&self.clamp) });
        entries.push(wgpu::BindGroupEntry { binding: 30, resource: params.as_entire_binding() });
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("terrain: map textures"), layout: &self.texture_layout, entries: &entries })
    }

    /// `ren_MipSkipLevels`: the tiled strata are sampled from `skip` mips
    /// down (rebound, not reloaded).
    pub fn set_mip_skip(&mut self, device: &wgpu::Device, skip: u32) {
        self.wrap = wrap_sampler(device, skip);
        if let Some(r) = &self.ready {
            let textures = self.texture_group(device, &r.views, &r.params);
            if let Some(r) = &mut self.ready {
                r.textures = textures;
            }
        }
    }

    fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, l: Loaded) -> Option<Ready> {
        // The map's textures are loaded (and their cache entries marked used).
        super::texture_load::mip_cache_loaded();
        let view = self.view.as_ref()?;
        let r = &l.render;
        let kind = ShaderKind::parse(&r.shader);
        let up = |t: &Option<dds::Texture>, label: &str| {
            t.as_ref().and_then(|t| {
                let v = super::models::upload_texture(device, queue, t, false, label);
                if v.is_none() {
                    tracing::warn!("{label}: {}x{} {:?} can't be uploaded", t.width, t.height, t.format);
                }
                v
            })
        };
        let grey = super::models::solid(device, queue, "terrain: grey (missing)", [128, 128, 128, 255], wgpu::TextureFormat::Rgba8Unorm);
        let flat = super::models::solid(device, queue, "terrain: flat normals (missing)", [128, 128, 255, 255], wgpu::TextureFormat::Rgba8Unorm);
        let black = super::models::solid(device, queue, "terrain: black (missing)", [0, 0, 0, 0], wgpu::TextureFormat::Rgba8Unorm);
        let white = super::models::solid(device, queue, "terrain: white (missing)", [255, 255, 255, 255], wgpu::TextureFormat::Rgba8Unorm);
        fn path(s: Option<&scmap::Stratum>) -> &str {
            s.map_or("", |s| s.path.as_str())
        }
        let albedo: Vec<Option<wgpu::TextureView>> = l.albedo.iter().enumerate().map(|(i, t)| up(t, &format!("map: stratum {i} albedo {}", path(r.albedo.get(i))))).collect();
        let normals: Vec<Option<wgpu::TextureView>> = l.normals.iter().enumerate().map(|(i, t)| up(t, &format!("map: stratum {i} normals {}", path(r.normals.get(i))))).collect();
        let masks = [up(&l.masks[0], "map: stratum mask (low)"), up(&l.masks[1], "map: stratum mask (high)")];
        let water_map = up(&l.water_map, "map: water map");
        let ramp = up(&l.ramp, "map: water ramp");
        let waves: Vec<Option<wgpu::TextureView>> = l.waves.iter().enumerate().map(|(i, t)| up(t, &format!("map: water waves {i}"))).collect();
        let cube = l.cube.as_ref().and_then(|faces| upload_cube(device, queue, "map: water reflection cube", faces));
        let grey_cube = upload_cube(device, queue, "map: grey cube (missing)", &std::array::from_fn(|_| dds::Texture { width: 1, height: 1, format: dds::Format::Rgba8, levels: vec![vec![110, 125, 140, 255]] })).expect("1x1 cube");
        let mut views: Vec<&wgpu::TextureView> = Vec::with_capacity(25);
        for (i, a) in albedo.iter().enumerate() {
            // Info texture slot defaults to "lit, unoccluded".
            let fallback = if kind.info && i == 8 { &white } else if i == ALBEDO_SLOTS - 1 { &black } else { &grey };
            views.push(a.as_ref().unwrap_or(fallback));
        }
        for n in &normals {
            views.push(n.as_ref().unwrap_or(&flat));
        }
        views.extend([masks[0].as_ref().unwrap_or(&black), masks[1].as_ref().unwrap_or(&black), water_map.as_ref().unwrap_or(&black), ramp.as_ref().unwrap_or(&black)]);
        views.extend(waves.iter().map(|w| w.as_ref().unwrap_or(&flat)));
        views.push(cube.as_ref().unwrap_or(&grey_cube));
        let (w, h) = (view.width as f32, view.height as f32);
        let tiles = |strata: &[scmap::Stratum]| {
            let mut out = [[0.0f32; 4]; 3];
            for (i, s) in strata.iter().enumerate() {
                out[i / 4][i % 4] = if s.scale > 0.0 { w / s.scale } else { 1.0 };
            }
            out
        };
        let lg = &r.lighting;
        let water = r.water.clone();
        let wr = water.clone().unwrap_or_default();
        let sky = lg.fog_color;
        let params = TerrainParams {
            albedo_tile: tiles(&r.albedo),
            normal_tile: tiles(&r.normals),
            size: [1.0 / w, 1.0 / h, w, h],
            sun_dir: [lg.sun_direction[0], lg.sun_direction[1], lg.sun_direction[2], lg.multiplier],
            sun_color: [lg.sun_color[0], lg.sun_color[1], lg.sun_color[2], 0.0],
            ambience: [lg.ambience[0], lg.ambience[1], lg.ambience[2], 0.0],
            shadow_fill: [lg.shadow_fill[0], lg.shadow_fill[1], lg.shadow_fill[2], 0.0],
            specular: lg.specular.map(|v| if v.abs() < 1e-30 { 0.0 } else { v }),
            water: [wr.levels.surface, wr.levels.deep, wr.levels.abyss, water.is_some() as u32 as f32],
            flags: [kind.strata, kind.bits(), kind.lower_macro, (lg.multiplier >= 2.2) as u32],
            waves: std::array::from_fn(|i| [wr.waves[i].0, wr.waves[i].1[0], wr.waves[i].1[1], 0.0]),
            water_lerp: [wr.color_lerp[0], wr.color_lerp[1], 0.0, 0.0],
            water_sun_dir: [wr.sun_direction[0], wr.sun_direction[1], wr.sun_direction[2], 0.0],
            water_color: [wr.surface_color[0], wr.surface_color[1], wr.surface_color[2], wr.sun_shininess],
            water_sun: [wr.sun_color[0], wr.sun_color[1], wr.sun_color[2], wr.sun_strength],
            water_fresnel: [wr.fresnel_bias, wr.fresnel_power, wr.sky_reflection, 0.0],
            sky: [sky[0], sky[1], sky[2], 1.0],
        };
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("terrain: map parameters"), contents: bytemuck::bytes_of(&params), usage: wgpu::BufferUsages::UNIFORM });
        let views: Vec<wgpu::TextureView> = views.into_iter().cloned().collect();
        let textures = self.texture_group(device, &views, &params);

        // Decals: the terrain's cells under each, in file order; a bind
        // group per (texture, spec map) pair. A decal whose texture failed
        // is skipped; a spec map that failed is left out.
        let decal_views: BTreeMap<&str, wgpu::TextureView> = l.decals.iter().filter_map(|(path, t)| Some((path.as_str(), up(t, &format!("map decal {path}"))?))).collect();
        let mut group_of: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        let mut decal_groups = Vec::new();
        let built = std::time::Instant::now();
        let decals = DecalLayer::build(device, view, &r.decals, |d| {
            let texture = decal_views.get_key_value(d.textures.first()?.as_str())?;
            let spec = spec_path(d).and_then(|p| decal_views.get_key_value(p.as_str()));
            let key = (*texture.0, spec.map_or("", |s| *s.0));
            Some(*group_of.entry(key).or_insert_with(|| {
                decal_groups.push(decal_group(device, &self.decal_layout, texture.1, spec.map_or(&self.no_spec, |s| s.1), texture.0));
                decal_groups.len() - 1
            }))
        });
        tracing::info!(shader = r.shader, decals = r.decals.len(), decal_pieces = decals.pieces.len(), decal_groups = decal_groups.len(), decal_spec_maps = r.decals.iter().filter(|d| spec_path(d).is_some()).count(), decal_instances = decals.records.len(), decal_build_ms = built.elapsed().as_secs_f32() * 1e3, "map textures uploaded");
        Some(Ready {
            params,
            textures,
            views,
            decals,
            decal_groups,
            water: water.map(|w| w.levels.surface),
            sun: lg.sun_direction,
            environment: l.environment.as_ref().and_then(|f| upload_cube(device, queue, "map: environment cube", f).map(|v| (v, f[0].width))),
            brdf: l.brdf.as_ref().and_then(|t| super::models::upload_texture(device, queue, t, false, "anisotropic lookup /textures/engine/anisotropiclookup.dds")),
            insect: l.insect.as_ref().and_then(|t| super::models::upload_texture(device, queue, t, false, "insect lookup /textures/engine/insectlookup.dds")),
            lighting: *lg,
            waves: Arc::new(r.wave_generators.clone()),
            sky: self.sky.sky(device, queue, &l.sky.0, &l.sky.1),
            horizon: {
                let (hw, hh) = l.horizon.size;
                device
                    .create_texture_with_data(
                        queue,
                        &wgpu::TextureDescriptor {
                            label: Some("terrain: horizon shadow"),
                            size: wgpu::Extent3d { width: hw.max(1), height: hh.max(1), depth_or_array_layers: 1 },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::R8Unorm,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        wgpu::util::TextureDataOrder::LayerMajor,
                        &l.horizon.texels,
                    )
                    .create_view(&wgpu::TextureViewDescriptor { label: Some("terrain: horizon shadow"), ..Default::default() })
            },
        })
    }

    /// The map's shoreline wave generators, once its textures are in.
    pub fn wave_generators(&self) -> Option<&Arc<Vec<scmap::WaveGenerator>>> {
        self.ready.as_ref().map(|r| &r.waves)
    }

    /// The terrain's baked self-shadow, once loaded.
    pub fn horizon(&self) -> Option<&wgpu::TextureView> {
        self.ready.as_ref().map(|r| &r.horizon)
    }

    /// Units' environment cube (and its face size), once loaded.
    pub fn environment(&self) -> Option<(&wgpu::TextureView, u32)> {
        self.ready.as_ref().and_then(|r| r.environment.as_ref().map(|(v, s)| (v, *s)))
    }

    /// FAF's BRDF lookup texture, once loaded.
    pub fn brdf(&self) -> Option<&wgpu::TextureView> {
        self.ready.as_ref().and_then(|r| r.brdf.as_ref())
    }

    pub fn insect(&self) -> Option<&wgpu::TextureView> {
        self.ready.as_ref().and_then(|r| r.insect.as_ref())
    }

    /// The map's lighting, once loaded.
    pub fn lighting(&self) -> Option<&scmap::Lighting> {
        self.ready.as_ref().map(|r| &r.lighting)
    }

    /// Toward the map's sun, once loaded.
    pub fn sun_direction(&self) -> Option<[f32; 3]> {
        self.ready.as_ref().map(|r| r.sun)
    }

    /// Pass 1: terrain and normal decals into the normal buffer, and the
    /// terrain's shadow, ambient occlusion and water depth into the light
    /// buffer (clears them and `depth`), inside `view`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_normals(&self, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, globals: &wgpu::BindGroup, terrain: (&wgpu::Buffer, &wgpu::Buffer, &[std::ops::Range<u32>]), timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>, view: &super::camera::View, decals: &DecalDraws) {
        let Some(r) = &self.ready else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terrain: normals + depth"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: &self.normal_target.normals,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.5, g: 1.0, b: 0.5, a: 1.0 }), store: wgpu::StoreOp::Store },
                }),
                // (Read only where the terrain drew, which writes it.)
                Some(wgpu::RenderPassColorAttachment {
                    view: &self.normal_target.light,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: timestamps.or_else(|| super::gpu_prof::pass_writes("terrain: normals + depth")),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        view.bind(&mut pass);
        pass.timed_push("terrain");
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(1, &r.textures, &[]);
        pass.set_pipeline(&self.normals_pipeline);
        pass.set_vertex_buffer(0, terrain.0.slice(..));
        pass.set_index_buffer(terrain.1.slice(..), wgpu::IndexFormat::Uint32);
        for r in terrain.2 {
            pass.draw_indexed(r.clone(), 0, 0..1);
        }
        pass.timed_pop();
        self.draw_decals(&mut pass, r, true, decals);
    }

    /// The map's decals, then the sim's on top.
    fn draw_decals<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, r: &'a Ready, normals: bool, decals: &DecalDraws) {
        let _s = rts_core_engine::prof_span!("decals");
        pass.set_pipeline(if normals { &self.decal_normals_pipeline } else { &self.decal_albedo_pipeline });
        let layers = [
            Some(("decals: map", &r.decals, &r.decal_groups)),
            self.script.layer.as_ref().map(|l| ("decals: sim (scorch marks, ...)", l, &self.script.groups)),
            self.script.user_layer.as_ref().map(|l| ("decals: interface", l, &self.script.groups)),
        ];
        for ((name, layer, groups), draws) in layers.into_iter().zip(&decals.layers).filter_map(|(l, d)| Some((l?, d))) {
            pass.timed_push(name);
            pass.set_vertex_buffer(0, layer.instances.slice(..));
            let mut bound = usize::MAX;
            for d in draws.iter().filter(|d| d.normals == normals) {
                if bound != d.texture {
                    pass.set_bind_group(2, &groups[d.texture], &[]);
                    bound = d.texture;
                }
                pass.draw(0..d.vertices, d.instances.clone());
            }
            pass.timed_pop();
        }
    }

    /// Pass 2 (inside the scene pass): lit terrain, then albedo decals.
    /// The map's sky (group 0 bound), behind everything.
    pub fn draw_sky<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if let Some(r) = &self.ready {
            self.sky.draw(pass, &r.sky);
        }
    }

    pub fn draw_albedo<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, terrain: (&'a wgpu::Buffer, &'a wgpu::Buffer, &[std::ops::Range<u32>]), decals: &DecalDraws) {
        let Some(r) = &self.ready else { return };
        pass.set_bind_group(1, &r.textures, &[]);
        pass.set_pipeline(&self.albedo_pipeline);
        pass.set_bind_group(2, &self.normal_target.group, &[]);
        pass.set_vertex_buffer(0, terrain.0.slice(..));
        pass.set_index_buffer(terrain.1.slice(..), wgpu::IndexFormat::Uint32);
        for r in terrain.2 {
            pass.draw_indexed(r.clone(), 0, 0..1);
        }
        pass.set_bind_group(3, &self.normal_target.group, &[]);
        self.draw_decals(pass, r, false, decals);
        // Tread marks and footprints over the decals.
        if !self.treads.rings.is_empty() {
            pass.timed_push("tread marks + footprints");
            pass.set_pipeline(&self.tread_pipeline);
            for ring in self.treads.rings.values() {
                // Every instance draws the grid the ring's largest mark
                // needs at this step (`vs_tread` collapses the rest).
                let n = (2.0 * ring.radius / decals.step.max(1) as f32).ceil() as u32 + 2;
                pass.set_bind_group(2, &self.script.groups[ring.group], &[]);
                pass.set_vertex_buffer(0, ring.instances.slice(..));
                pass.draw(0..n * n * 6, 0..ring.count);
            }
            pass.timed_pop();
        }
    }

    /// The water surface (after opaque geometry).
    pub fn draw_water<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, quad: &'a wgpu::Buffer) -> bool {
        let Some(r) = &self.ready else { return false };
        if r.water.is_none() {
            return true;
        }
        pass.set_bind_group(1, &r.textures, &[]);
        pass.set_pipeline(&self.water_pipeline);
        pass.set_vertex_buffer(0, quad.slice(..));
        pass.draw(0..6, 0..1);
        true
    }

}

/// Uploads a cube map (all faces must share size and format).
fn upload_cube(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, faces: &[dds::Texture; 6]) -> Option<wgpu::TextureView> {
    use wgpu::TextureFormat as F;
    let f0 = &faces[0];
    let format = match f0.format {
        dds::Format::Bc1 => F::Bc1RgbaUnorm,
        dds::Format::Bc2 => F::Bc2RgbaUnorm,
        dds::Format::Bc3 => F::Bc3RgbaUnorm,
        dds::Format::Rgba8 => F::Rgba8Unorm,
        dds::Format::Bgra8 => F::Bgra8Unorm,
    };
    if f0.format.is_compressed() && (f0.width % 4 != 0 || f0.height % 4 != 0) {
        return None;
    }
    let levels = faces.iter().map(|f| f.levels.len()).min()? as u32;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: f0.width, height: f0.height, depth_or_array_layers: 6 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, face) in faces.iter().enumerate() {
        for l in 0..levels as usize {
            let (w, h) = ((f0.width >> l).max(1), (f0.height >> l).max(1));
            let (row, rows, extent) = if f0.format.is_compressed() {
                let block = if f0.format == dds::Format::Bc1 { 8 } else { 16 };
                (w.div_ceil(4) * block, h.div_ceil(4), (w.next_multiple_of(4), h.next_multiple_of(4)))
            } else {
                (w * 4, h, (w, h))
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: l as u32, origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 }, aspect: wgpu::TextureAspect::All },
                &face.levels[l],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(rows) },
                wgpu::Extent3d { width: extent.0, height: extent.1, depth_or_array_layers: 1 },
            );
        }
    }
    Some(tex.create_view(&wgpu::TextureViewDescriptor { label: Some(label), dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() }))
}

/// Height of the terrain at (x, z), bilinear.
/// A script decal / splat texture's bind group, or `None` while it loads:
/// on first sight it's read and decoded on the global pool.
/// A script decal / splat's bind group (its texture and spec map, "" for
/// none), or `None` while either loads (a spec map that fails to load is
/// left out).
#[allow(clippy::too_many_arguments)]
fn request_group(script: &mut ScriptDecals, gpu: &(wgpu::Device, wgpu::Queue), device: &wgpu::Device, layout: &wgpu::BindGroupLayout, no_spec: &wgpu::TextureView, vfs: &Option<Arc<RwLock<Vfs>>>, bc: bool, texture: &str, spec: &str) -> Option<usize> {
    let key = (texture.to_string(), spec.to_string());
    if let Some(&g) = script.group_of.get(&key) {
        return Some(g);
    }
    let t = request_texture(script, gpu, vfs, bc, texture)?;
    let s = if spec.is_empty() {
        None
    } else {
        match request_texture(script, gpu, vfs, bc, spec) {
            Some(i) => Some(i),
            None if matches!(script.textures.get(spec), Some(DecalTexture::Failed)) => None,
            None => return None,
        }
    };
    let group = decal_group(device, layout, &script.views[t], s.map_or(no_spec, |i| &script.views[i]), texture);
    script.groups.push(group);
    script.group_of.insert(key, script.groups.len() - 1);
    Some(script.groups.len() - 1)
}

/// A script decal texture's index in `views`, or `None` while it loads (or
/// failed to).
fn request_texture(script: &mut ScriptDecals, gpu: &(wgpu::Device, wgpu::Queue), vfs: &Option<Arc<RwLock<Vfs>>>, bc: bool, path: &str) -> Option<usize> {
    match script.textures.get(path) {
        Some(DecalTexture::Ready(i)) => return Some(*i),
        Some(_) => return None,
        None => {}
    }
    let tx = script.loaded.get_or_insert_with(std::sync::mpsc::channel).0.clone();
    script.textures.insert(path.to_string(), DecalTexture::Loading);
    let (path, vfs, gpu) = (path.to_string(), vfs.clone(), gpu.clone());
    rayon::spawn(move || {
        let _s = rts_core_engine::prof_span!("load decal texture");
        let view = vfs.as_ref().and_then(|v| super::texture_load::load(&gpu.0, &gpu.1, v, &path, false, bc, "decal"));
        let _ = tx.send((path, view));
    });
    None
}

fn height(t: &TerrainView, x: f32, z: f32) -> f32 {
    let (x, z) = (x.clamp(0.0, t.width as f32), z.clamp(0.0, t.height as f32));
    let (x0, z0) = (x.floor().min(t.width as f32 - 1.0), z.floor().min(t.height as f32 - 1.0));
    let (fx, fz) = (x - x0, z - z0);
    let at = |xi: f32, zi: f32| t.heights[(zi as u32 * (t.width + 1) + xi as u32) as usize];
    let a = at(x0, z0) + (at(x0 + 1.0, z0) - at(x0, z0)) * fx;
    let b = at(x0, z0 + 1.0) + (at(x0 + 1.0, z0 + 1.0) - at(x0, z0 + 1.0)) * fx;
    a + (b - a) * fz
}

/// The terrain mesh's sample spacing: every sample on maps up to 20 km
/// (1024 cells), every n-th on bigger ones to keep the mesh at most ~1024^2
/// (`Scene::upload_terrain`; decals and tread marks share its grid).
pub fn mesh_step(t: &TerrainView) -> u32 {
    (t.width.max(t.height) / 1024).max(1)
}

/// Terrain tiles' side in cells at the finest level: each level's indices
/// are laid out tile by tile so the frame draws only the tiles in view.
pub const TERRAIN_TILE: u32 = 64;

/// One level of detail of the terrain mesh's index buffer.
pub struct TerrainLodIndices {
    /// Its sample step (world units).
    pub step: u32,
    pub indices: Vec<u32>,
    /// Each tile's run of `indices` (tiles row-major over the map, the
    /// same tiles at every level: [`TERRAIN_TILE`] cells of the finest).
    pub tiles: Vec<std::ops::Range<u32>>,
}

/// The terrain mesh's index buffers, finest first: for each level of detail
/// (every 1st, 2nd, 4th, 8th vertex of a `cols` x `rows` vertex grid of
/// sample step `step`, while it has at least 2 cells a side), its sample
/// step and indices tile by tile, every cell `(x, z), (x, z+1), (x+1, z)`
/// then `(x+1, z), (x, z+1), (x+1, z+1)` (`terrain_cell_corner` in the
/// shaders).
pub fn lod_indices(cols: u32, rows: u32, step: u32) -> Vec<TerrainLodIndices> {
    let mut out = Vec::new();
    let (tiles_x, tiles_z) = ((cols - 1).div_ceil(TERRAIN_TILE), (rows - 1).div_ceil(TERRAIN_TILE));
    for k in [1u32, 2, 4, 8] {
        if !(cols - 1).is_multiple_of(k) || !(rows - 1).is_multiple_of(k) || (cols - 1) / k < 2 || (rows - 1) / k < 2 || !TERRAIN_TILE.is_multiple_of(k) {
            break;
        }
        let (c, r, t) = ((cols - 1) / k, (rows - 1) / k, TERRAIN_TILE / k);
        let at = |x: u32, z: u32| z * k * cols + x * k;
        let mut indices = Vec::with_capacity((c * r * 6) as usize);
        let mut tiles = Vec::with_capacity((tiles_x * tiles_z) as usize);
        for tz in 0..tiles_z {
            for tx in 0..tiles_x {
                let start = indices.len() as u32;
                for z in tz * t..((tz + 1) * t).min(r) {
                    for x in tx * t..((tx + 1) * t).min(c) {
                        indices.extend_from_slice(&[at(x, z), at(x, z + 1), at(x + 1, z), at(x + 1, z), at(x, z + 1), at(x + 1, z + 1)]);
                    }
                }
                tiles.push(start..indices.len() as u32);
            }
        }
        out.push(TerrainLodIndices { step: step * k, indices, tiles });
    }
    out
}

/// The terrain mesh's vertex at grid point (x, z): decals and tread marks
/// rebuild these exact positions from the height buffer (`terrain_vertex`
/// in the shaders), so they rasterise to the terrain's own depth.
pub fn mesh_vertex(t: &TerrainView, x: u32, z: u32) -> [f32; 3] {
    [x as f32, t.heights[(z.min(t.height) * (t.width + 1) + x.min(t.width)) as usize], z as f32]
}

/// A decal instance cut into pieces of at most [`TERRAIN_TILE`] samples a
/// side (on the tiles' grid), each with its bounding sphere over the
/// heights it covers.
fn decal_tiles(t: &TerrainView, d: DecalInstance) -> Vec<(DecalInstance, Vec3, f32)> {
    let step = mesh_step(t);
    let tile = TERRAIN_TILE * step;
    let [x0, z0, x1, z1] = d.bounds;
    let mut out = Vec::new();
    for tz in (z0 / tile)..z1.div_ceil(tile) {
        for tx in (x0 / tile)..x1.div_ceil(tile) {
            let b = [x0.max(tx * tile), z0.max(tz * tile), x1.min((tx + 1) * tile), z1.min((tz + 1) * tile)];
            if b[0] >= b[2] || b[1] >= b[3] {
                continue;
            }
            let (mut y0, mut y1) = (f32::MAX, f32::MIN);
            for z in (b[1]..=b[3]).step_by(step as usize) {
                for x in (b[0]..=b[2]).step_by(step as usize) {
                    let y = mesh_vertex(t, x, z)[1];
                    (y0, y1) = (y0.min(y), y1.max(y));
                }
            }
            let (lo, hi) = (Vec3::new(b[0] as f32, y0, b[1] as f32), Vec3::new(b[2] as f32, y1, b[3] as f32));
            let centre = (lo + hi) * 0.5;
            out.push((DecalInstance { bounds: b, ..d }, centre, (hi - centre).length()));
        }
    }
    out
}

/// A decal's texture matrix (moho's `CWldTerrainDecal::Update`,
/// MohoEngine.dll 0x1045fb50): world to local is the offset from its
/// position (the corner, uv 0,0) turned by `rotationAxisY(rotation.y)`,
/// then `X(rotation.x)`, then `Z(rotation.z)` (gpggal's `D3DXMatrixRotation*`,
/// row vectors), and uv = local (x, z) / scale (x, z). As a column-vector
/// matrix here: `local = m * (p - position)`. The tilt (x, z) projects the
/// texture along the decal's turned up axis; the footprint
/// (`ComputeCorner`) and the normal decals' tangent frame turn by the
/// heading alone.
pub fn decal_matrix(rotation: [f32; 3]) -> glam::Mat3 {
    let [rx, ry, rz] = rotation;
    let (sy, cy) = ry.sin_cos();
    let (sx, cx) = rx.sin_cos();
    let (sz, cz) = rz.sin_cos();
    // Transposes of D3DX's row-vector rotations, applied Y, X, Z.
    let y = glam::Mat3::from_cols(Vec3::new(cy, 0.0, -sy), Vec3::Y, Vec3::new(sy, 0.0, cy));
    let x = glam::Mat3::from_cols(Vec3::X, Vec3::new(0.0, cx, sx), Vec3::new(0.0, -sx, cx));
    let z = glam::Mat3::from_cols(Vec3::new(cz, sz, 0.0), Vec3::new(-sz, cz, 0.0), Vec3::Z);
    z * x * y
}

/// Where a decal's (u, v) corner lies on the ground plane, by its heading
/// alone (moho's `CWldTerrainDecal::ComputeCorner`, MohoEngine.dll
/// 0x1045f8b0): `x + u sx cos - v sz sin`, `z + u sx sin + v sz cos`.
pub fn decal_corner(d: &scmap::Decal, u: f32, v: f32) -> (f32, f32) {
    let (s, c) = d.rotation[1].sin_cos();
    let (lx, lz) = (u * d.scale[0], v * d.scale[2]);
    (d.position[0] + lx * c - lz * s, d.position[2] + lx * s + lz * c)
}

/// A decal as moho draws it (`terrain.fx` `DecalsVS`): the terrain mesh's
/// own cells under its footprint (built in the vertex shader from the
/// height buffer, at the frame's level of detail), the texture projected
/// through its matrix ([`decal_matrix`]); the shader clamps the texture,
/// hides the cells' part outside 0..1 and collapses cells wholly off it.
/// The same positions and triangulation as the terrain give the same
/// depth, so the decal passes `LessEqual` exactly where the ground is: no
/// lift, no z-fighting, no sinking into rises. Returns its instance and
/// bounding sphere, or `None` off the map.
fn decal_instance(t: &TerrainView, d: &scmap::Decal) -> Option<(DecalInstance, Vec3, f32)> {
    let (sx, sz) = (d.scale[0].max(0.01), d.scale[2].max(0.01));
    let m = decal_matrix(d.rotation);
    let origin = Vec3::from_array(d.position);
    let d = &scmap::Decal { scale: [sx, d.scale[1], sz], ..d.clone() };
    // The footprint's bounds, out to the mesh grid and in to the map: the
    // heading's corners, and where a tilted decal's corners meet the plane
    // at its height (local y solved for world y = position y), plus slack
    // for the ground's rise and fall under a tilt.
    let step = mesh_step(t);
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    let inv = m.transpose();
    let tilted = d.rotation[0] != 0.0 || d.rotation[2] != 0.0;
    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let mut corners = vec![decal_corner(d, u, v)];
        if tilted && inv.y_axis.y.abs() > 0.1 {
            let (lx, lz) = (u * sx, v * sz);
            let ly = -(inv.x_axis.y * lx + inv.z_axis.y * lz) / inv.y_axis.y;
            let w = inv * Vec3::new(lx, ly, lz) + origin;
            corners.push((w.x, w.z));
        }
        for (x, z) in corners {
            lo = (lo.0.min(x), lo.1.min(z));
            hi = (hi.0.max(x), hi.1.max(z));
        }
    }
    let slack = if tilted { 0.25 * sx.max(sz) } else { 0.0 };
    let f = step as f32;
    let snap = |v: f32, up: bool, last: u32| ((if up { (v / f).ceil() } else { (v / f).floor() }).clamp(0.0, (last / step) as f32) as u32) * step;
    let bounds = [snap(lo.0 - slack, false, t.width), snap(lo.1 - slack, false, t.height), snap(hi.0 + slack, true, t.width), snap(hi.1 + slack, true, t.height)];
    if bounds[0] == bounds[2] || bounds[1] == bounds[3] {
        return None;
    }
    // Its bounding sphere: the bounds' box over the heights it covers.
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for z in (bounds[1]..=bounds[3]).step_by(step as usize) {
        for x in (bounds[0]..=bounds[2]).step_by(step as usize) {
            let y = mesh_vertex(t, x, z)[1];
            (y0, y1) = (y0.min(y), y1.max(y));
        }
    }
    let lo3 = Vec3::new(bounds[0] as f32, y0, bounds[1] as f32);
    let hi3 = Vec3::new(bounds[2] as f32, y1, bounds[3] as f32);
    let centre = (lo3 + hi3) * 0.5;
    let (r0, r2) = (m.row(0), m.row(2));
    let record = DecalInstance {
        origin_cutoff: [origin.x, origin.y, origin.z, d.cutoff.max(0.0)],
        row_u: [r0.x, r0.y, r0.z, 1.0 / sx],
        row_v: [r2.x, r2.y, r2.z, 1.0 / sz],
        bounds,
        heading: [d.rotation[1], 0.0, 0.0, 0.0],
    };
    Some((record, centre, (hi3 - centre).length()))
}

const SHADER: &str = include_str!(concat!(env!("OUT_DIR"), "/terrain.wgsl"));

#[cfg(test)]
mod tests {
    use super::*;

    /// A decal's footprint snaps out to the mesh's grid from its position
    /// (the corner), runs `scale` along x and z unturned, and its texture
    /// covers 0..1 across it; turned a quarter, +u runs toward +z and +v
    /// toward -x (moho's `ComputeCorner`); at a coarser level of detail its
    /// grid widens to that step; off the map it has none.
    #[test]
    fn decals_cover_their_footprint_on_the_mesh_grid() {
        let flat = TerrainView { width: 64, height: 64, heights: (0..65 * 65).map(|i| (i % 7) as f32).collect(), water: None, map: None };
        let decal = |heading: f32| scmap::Decal { kind: decal_kind::ALBEDO, scale: [8.0, 8.0, 8.0], position: [10.0, 0.0, 20.0], rotation: [0.0, heading, 0.0], ..Default::default() };
        let (d, centre, _) = decal_instance(&flat, &decal(0.0)).unwrap();
        assert_eq!(d.bounds, [10, 20, 18, 28]);
        let at = |d: &DecalInstance, x: u32, z: u32| d.uv(Vec3::from_array(mesh_vertex(&flat, x, z)));
        assert_eq!(at(&d, 10, 20), [0.0, 0.0]);
        assert_eq!(at(&d, 18, 28), [1.0, 1.0]);
        assert!((centre.x - 14.0).abs() < 1e-4 && (centre.z - 24.0).abs() < 1e-4);
        assert_eq!(d.cells(1, 64, 64), [10, 20, 8, 8]);
        assert_eq!(d.cells(4, 64, 64), [2, 5, 3, 2], "x 8..20, z 20..28 at step 4");
        let (d, _, _) = decal_instance(&flat, &decal(std::f32::consts::FRAC_PI_2)).unwrap();
        let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
        assert!(close(at(&d, 10, 28), [1.0, 0.0]), "{:?}", at(&d, 10, 28));
        assert!(close(at(&d, 2, 20), [0.0, 1.0]), "{:?}", at(&d, 2, 20));
        assert_eq!(d.bounds, [2, 20, 10, 28]);
        assert!(decal_instance(&flat, &scmap::Decal { position: [100.0, 0.0, 100.0], ..decal(0.0) }).is_none());
    }

    /// The terrain's levels of detail index the same vertices with the
    /// same triangulation, every 1st, 2nd, 4th, 8th vertex.
    #[test]
    fn terrain_levels_of_detail_share_the_vertices() {
        let lods = lod_indices(17, 17, 1);
        assert_eq!(lods.iter().map(|l| (l.step, l.indices.len())).collect::<Vec<_>>(), vec![(1, 16 * 16 * 6), (2, 8 * 8 * 6), (4, 4 * 4 * 6), (8, 2 * 2 * 6)]);
        assert_eq!(&lods[0].indices[..6], &[0, 17, 1, 1, 17, 18]);
        assert_eq!(&lods[1].indices[..6], &[0, 34, 2, 2, 34, 36]);
        // Tiled: 128 cells a side are 2 x 2 tiles at every level, their
        // runs covering every cell once, in tile order.
        for l in lod_indices(129, 129, 1) {
            assert_eq!(l.tiles.len(), 4);
            assert_eq!(l.tiles.iter().map(|r| r.len()).sum::<usize>(), l.indices.len());
            assert!(l.tiles.windows(2).all(|w| w[0].end == w[1].start));
            let cells = 128 / (l.step as usize);
            assert_eq!(l.indices.len(), cells * cells * 6);
            assert_eq!(l.tiles[1].start as usize, (cells / 2) * (cells / 2) * 6, "tile 1 starts after tile 0's cells");
        }
    }

    /// A tilted decal (rotation x) projects along its turned up axis: on
    /// ground at its own height, v stretches by 1 / cos(tilt) and the
    /// footprint grows to cover it; u is unchanged.
    #[test]
    fn tilted_decals_project_along_their_turned_axis() {
        let flat = TerrainView { width: 64, height: 64, heights: vec![1.0; 65 * 65], water: None, map: None };
        let tilt = 0.5f32;
        let d = scmap::Decal { kind: decal_kind::ALBEDO, scale: [8.0, 8.0, 8.0], position: [10.0, 1.0, 20.0], rotation: [tilt, 0.0, 0.0], ..Default::default() };
        let (d, _, _) = decal_instance(&flat, &d).unwrap();
        let uv = d.uv(Vec3::new(18.0, 1.0, 28.0));
        assert!((uv[0] - 1.0).abs() < 1e-5 && (uv[1] - tilt.cos()).abs() < 1e-5, "{uv:?}");
        assert!(d.bounds[3] >= 30, "the stretched footprint is covered: {:?}", d.bounds);
        assert_eq!(decal_matrix([0.0; 3]), glam::Mat3::IDENTITY);
    }

    #[test]
    fn shader_names_pick_features() {
        let t = ShaderKind::parse("TTerrain");
        assert_eq!((t.strata, t.albedo_half, t.tterrain_spec, t.info), (4, true, true, false));
        let x = ShaderKind::parse("TTerrainXP");
        assert_eq!((x.strata, x.albedo_half, x.normal_half, x.pbr), (8, true, false, false));
        let a = ShaderKind::parse("Terrain200");
        assert_eq!((a.strata, a.albedo_half, a.pbr, a.height_splat, a.info, a.upper_macro), (7, false, true, true, true, false));
        let b = ShaderKind::parse("Terrain152B");
        assert_eq!((b.albedo_half, b.normal_half, b.pbr, b.height_splat, b.lower_macro), (true, true, true, false, 2));
        let e = ShaderKind::parse("TTerrainXPExt");
        assert_eq!((e.strata, e.info, e.upper_macro, e.albedo_half), (7, true, true, true));
    }
}

