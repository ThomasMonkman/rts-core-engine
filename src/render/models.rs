//! Unit and prop models: each entity's LOD0 `.scm` mesh in its rest pose,
//! textured with its albedo and team-coloured through the specteam alpha
//! (FA's `mesh.fx`: `albedo.rgb = lerp(teamColor, albedo.rgb, 1 - specular.a)`).
//!
//! The sim names each mesh once (`Snapshot::new_meshes`: VFS paths of the
//! mesh and its textures). The first time an entity with that mesh is drawn,
//! a renderer worker reads and decodes the files off the render thread; the
//! render thread uploads finished loads, a few per frame. Until a model is
//! ready (or if it fails to load) its entities stay boxes.
//!
//! Textures upload block-compressed (DXT1/3/5 as BC1/2/3) when the GPU
//! samples BC, else decoded to RGBA8 on the worker.
//!
//! Animated entities (snapshot pose ops: turrets, rotators, walk clips)
//! skin rigidly, one bone per vertex as FA does: their bone matrices
//! (`render_pose`) go in a per-frame palette storage buffer and the
//! instance names its first entry; static instances skip skinning.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::engine::api::mesh::{BuildShader, RenderAsset};
use crate::engine::assets::dds::{self, Format};
use crate::engine::assets::sca::{self, Clip};
use crate::engine::assets::scm::{self, MeshData};
use crate::engine::vfs::Vfs;

/// Most models uploaded in one frame (keeps first sight of a new army's
/// units from hitching a frame).
const UPLOADS_PER_FRAME: usize = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MeshVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    bone: u32,
    /// Tangent frame and second texture coordinates (normal maps).
    tangent: [f32; 3],
    binormal: [f32; 3],
    uv1: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MeshInstance {
    pub model: [[f32; 4]; 4],
    /// rgb = team colour (negative: none, e.g. props), a = opacity.
    pub team: [f32; 4],
    /// x = wreck (0/1), y = alpha cutout (0/1), z unused, w = effect
    /// overlay (0/1).
    pub params: [f32; 4],
    /// x = first palette entry ([`NO_BONES`]: unskinned), y =
    /// [`crate::engine::api::mesh::VertexMotion`] (the vertex shader's sway).
    pub bones: [u32; 4],
    /// x = [`crate::engine::api::mesh::Shading`] (FAF's pixel shader family).
    pub shading: [f32; 4],
}

/// `MeshInstance::bones` of an instance drawn in its rest pose.
pub const NO_BONES: u32 = u32::MAX;

const MESH: (&str, &str) = ("vs_mesh", "fs_mesh");

pub const SHADER: &str = include_str!(concat!(env!("OUT_DIR"), "/models.wgsl"));

const VERTEX_ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Uint32, 11 => Float32x3, 12 => Float32x3, 13 => Float32x2];
const INSTANCE_ATTRS: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Uint32x4, 14 => Float32x4];
/// Mesh vertices and per-instance data (also the shadow casters' input).
pub const VERTEX_LAYOUTS: [wgpu::VertexBufferLayout<'static>; 2] = [
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<MeshVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &VERTEX_ATTRS },
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<MeshInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &INSTANCE_ATTRS },
];

pub struct GpuModel {
    /// Its mesh's path (GPU labels).
    name: String,
    /// Where its vertices and indices sit in the store's geometry
    /// ([`Geometry`]): the chunk, the first vertex (the draw's base vertex)
    /// and the first index.
    chunk: u32,
    base_vertex: i32,
    first_index: u32,
    index_count: u32,
    textures: wgpu::BindGroup,
    /// Its albedo, spec-team, normal and lookup textures (fallbacks in
    /// place) and build secondary, kept to rebind them with a new sampler
    /// (Texture Detail).
    views: [wgpu::TextureView; 4],
    secondary_view: Option<wgpu::TextureView>,
    pub wreck: bool,
    pub cutout: bool,
    pub effect: bool,
    /// Moho build-in technique, with its secondary texture (group 3).
    pub build: Option<BuildShader>,
    /// Tread bands scroll (`RenderAsset::scrolling`).
    pub scrolling: bool,
    /// Its vertices sway or swell (`RenderAsset::vertex_motion`).
    pub vertex_motion: crate::engine::api::mesh::VertexMotion,
    secondary: Option<wgpu::BindGroup>,
    pub shading: crate::engine::api::mesh::Shading,
    /// Farthest vertex from the origin (mesh units).
    pub radius: f32,
    pub skeleton: rts_core_engine::render_pose::Skeleton,
}

/// Every mesh's vertices and indices in a few big buffers, appended as the
/// meshes load (the store never unloads one), so a pass binds a chunk once
/// and a draw is its texture bind and a draw call: wgpu replays each
/// recorded command when the encoder is finished, which was most of the
/// frame's submit cost at five commands a draw.
struct Geometry {
    chunks: Vec<GeometryChunk>,
}

struct GeometryChunk {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Vertices and indices used so far (the index count is kept even, so
    /// the next write is 4-byte aligned).
    vertex_used: u32,
    index_used: u32,
}

/// A chunk's capacity: 24 MB of vertices (68 bytes each), 4 MB of 16-bit
/// indices. A mesh is at most 65536 vertices (16-bit indices), so it always
/// fits a fresh chunk.
const CHUNK_VERTICES: u32 = 24 * 1024 * 1024 / std::mem::size_of::<MeshVertex>() as u32;
const CHUNK_INDICES: u32 = 2 * 1024 * 1024;

impl Geometry {
    /// Appends a mesh: (chunk, base vertex, first index).
    fn push(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[MeshVertex], indices: &[u16]) -> (u32, i32, u32) {
        debug_assert!(indices.len() % 2 == 0, "indices padded to an even count");
        let (nv, ni) = (vertices.len() as u32, indices.len() as u32);
        let fits = |c: &GeometryChunk| c.vertex_used + nv <= CHUNK_VERTICES && c.index_used + ni <= CHUNK_INDICES;
        let chunk = match self.chunks.iter().rposition(fits) {
            Some(i) => i,
            None => {
                let n = self.chunks.len();
                self.chunks.push(GeometryChunk {
                    vertices: device.create_buffer(&wgpu::BufferDescriptor { label: Some(&format!("models: vertices {n}")), size: CHUNK_VERTICES as u64 * std::mem::size_of::<MeshVertex>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
                    indices: device.create_buffer(&wgpu::BufferDescriptor { label: Some(&format!("models: indices {n}")), size: CHUNK_INDICES as u64 * 2, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
                    vertex_used: 0,
                    index_used: 0,
                });
                self.chunks.len() - 1
            }
        };
        let c = &mut self.chunks[chunk];
        let (base_vertex, first_index) = (c.vertex_used, c.index_used);
        queue.write_buffer(&c.vertices, base_vertex as u64 * std::mem::size_of::<MeshVertex>() as u64, bytemuck::cast_slice(vertices));
        queue.write_buffer(&c.indices, first_index as u64 * 2, bytemuck::cast_slice(indices));
        c.vertex_used += nv;
        c.index_used += ni;
        (chunk as u32, base_vertex as i32, first_index)
    }
}

/// What a render pass has bound of the model store, so consecutive draws
/// skip the binds they share ([`Models::begin`] starts one).
#[derive(Default)]
pub struct Bound {
    chunk: Option<u32>,
    mesh: Option<u32>,
}

/// See [`Models::view`].
pub struct ModelsView<'a> {
    slots: &'a [Slot],
    interface_slots: &'a [Slot],
    clips: &'a [ClipSlot],
    bindings: &'a std::collections::BTreeMap<(u32, u32), Vec<Option<u32>>>,
}

impl<'a> ModelsView<'a> {
    fn slot(&self, index: u32) -> Option<&'a Slot> {
        match index.checked_sub(INTERFACE_BASE) {
            Some(k) => self.interface_slots.get(k as usize),
            None => self.slots.get(index as usize),
        }
    }

    pub fn ready(&self, index: u32) -> Option<&'a GpuModel> {
        match self.slot(index) {
            Some(Slot::Ready(m)) => Some(m),
            _ => None,
        }
    }

    pub fn failed(&self, index: u32) -> bool {
        matches!(self.slot(index), Some(Slot::Failed))
    }

    pub fn clip_for(&self, mesh: u32, clip: u32) -> Option<(&'a Clip, &'a [Option<u32>])> {
        let Some(ClipSlot::Ready(c)) = self.clips.get(clip as usize) else { return None };
        Some((c.as_ref(), self.bindings.get(&(mesh, clip))?.as_slice()))
    }
}

enum ClipSlot {
    Idle(String),
    Loading,
    Ready(Arc<Clip>),
    Failed,
}

enum Slot {
    /// Named by the sim, not drawn yet.
    Idle,
    Loading,
    Ready(GpuModel),
    Failed,
}

/// A worker's result.
enum Done {
    Model(Loaded),
    Clip(u32, std::io::Result<Clip>),
}

/// A worker's decoded files.
struct Loaded {
    index: u32,
    mesh: std::io::Result<MeshData>,
    /// Uploaded by the loader ([`super::texture_load`]).
    albedo: Option<wgpu::TextureView>,
    specteam: Option<wgpu::TextureView>,
    normals: Option<wgpu::TextureView>,
    secondary: Option<wgpu::TextureView>,
    lookup: Option<wgpu::TextureView>,
}

pub struct Models {
    assets: Vec<Option<RenderAsset>>,
    slots: Vec<Slot>,
    /// Meshes the interface draws by path (`AddCommandFeedbackBlip`,
    /// `WorldMesh`), indexed from [`INTERFACE_BASE`] so the sim's indices
    /// never meet them.
    interface_assets: Vec<RenderAsset>,
    interface_slots: Vec<Slot>,
    interface_index: std::collections::HashMap<(String, String, u8), u32>,
    vfs: Option<Arc<RwLock<Vfs>>>,
    /// Bounding boxes (mesh space, rest pose: min xyz, max xyz) of the sim's
    /// meshes loaded since last taken ([`Models::take_new_bounds`]): the
    /// main thread's picking and drag selection test them.
    new_bounds: Vec<(u32, [f32; 6])>,
    /// Every sim mesh's bounds loaded so far (same form), by mesh index:
    /// selection brackets are centred on them.
    bounds: Vec<Option<[f32; 6]>>,
    /// For the loaders, which upload their textures themselves.
    gpu: (wgpu::Device, wgpu::Queue),
    /// The textures many models share (lookup, build), loaded once.
    shared: super::texture_load::Cache,
    /// Every texture goes through `shared`, which another head's models
    /// load through too (each head's scene has its own models; their
    /// textures are loaded once between them).
    share_all: bool,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    clips: Vec<ClipSlot>,
    /// Each clip's file, by clip index (a clip changed on disk loads again).
    clip_paths: Vec<String>,
    /// Per (mesh, clip): each mesh bone's clip bone.
    bindings: std::collections::BTreeMap<(u32, u32), Vec<Option<u32>>>,
    /// Draw calls this frame (the status bar; `Scene::draw` resets it).
    pub draws: std::cell::Cell<u32>,
    geometry: Geometry,
    pub(super) palette_layout: wgpu::BindGroupLayout,
    palette: wgpu::Buffer,
    palette_capacity: usize,
    palette_group: wgpu::BindGroup,
    /// Meshes / albedo textures that failed to load (render-state tests).
    failures: Vec<String>,
    /// The GPU samples BC1-3 directly.
    bc: bool,
    pub(super) layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Stand-ins for a texture that failed to load: white albedo, no team
    /// colour / specular.
    white: wgpu::TextureView,
    plain_spec: wgpu::TextureView,
    /// A flat normal map (FA reads x from green, y from alpha).
    flat_normals: wgpu::TextureView,
    /// Closed meshes (back faces culled) and alpha-tested two-sided ones.
    pub pipeline: wgpu::RenderPipeline,
    pub cutout_pipeline: wgpu::RenderPipeline,
    /// Effect overlays: two-sided, blended, no depth writes.
    pub effect_pipeline: wgpu::RenderPipeline,
    /// The build placement ghost (FAF's `UnitPlace` technique).
    pub place_pipeline: wgpu::RenderPipeline,
    /// Build-in techniques: pass 0 (depth written), pass 1 overlays (equal
    /// depth passes, none written), and the UEF build cube (no depth writes).
    secondary_layout: wgpu::BindGroupLayout,
    pub build_pipeline: wgpu::RenderPipeline,
    pub build_overlay_pipeline: wgpu::RenderPipeline,
    pub build_cube_pipeline: wgpu::RenderPipeline,
    /// Shield bubbles (`fs_shield`): blended, the Cybran second pass, and
    /// the additive Seraphim one.
    pub shield_pipeline: wgpu::RenderPipeline,
    pub shield_offset_pipeline: wgpu::RenderPipeline,
    pub shield_add_pipeline: wgpu::RenderPipeline,
    /// Effect techniques after the water (see [`Models::effect_pipeline`]).
    effect_pipelines: [wgpu::RenderPipeline; 7],
}

/// A 1x1 texture of one colour (a stand-in where a texture is missing).
pub(super) fn solid(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, rgba: [u8; 4], format: wgpu::TextureFormat) -> wgpu::TextureView {
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
    );
    tex.create_view(&wgpu::TextureViewDescriptor { label: Some(label), ..Default::default() })
}

/// The GPU format a `.dds` of `format` is sampled as.
pub(super) fn texture_format(format: Format, srgb: bool) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as F;
    match (format, srgb) {
        (Format::Bc1, true) => F::Bc1RgbaUnormSrgb,
        (Format::Bc1, false) => F::Bc1RgbaUnorm,
        (Format::Bc2, true) => F::Bc2RgbaUnormSrgb,
        (Format::Bc2, false) => F::Bc2RgbaUnorm,
        (Format::Bc3, true) => F::Bc3RgbaUnormSrgb,
        (Format::Bc3, false) => F::Bc3RgbaUnorm,
        (Format::Rgba8, true) => F::Rgba8UnormSrgb,
        (Format::Rgba8, false) => F::Rgba8Unorm,
        (Format::Bgra8, true) => F::Bgra8UnormSrgb,
        (Format::Bgra8, false) => F::Bgra8Unorm,
    }
}

/// Uploads a decoded `.dds` (all its levels).
pub(super) fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, t: &dds::Texture, srgb: bool, label: &str) -> Option<wgpu::TextureView> {
    let format = texture_format(t.format, srgb);
    // BC textures must be whole blocks.
    if t.format.is_compressed() && (t.width % 4 != 0 || t.height % 4 != 0) {
        return None;
    }
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        mip_level_count: t.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: super::texture_load::USAGE,
        view_formats: &[],
    });
    for (l, data) in t.levels.iter().enumerate() {
        let (w, h) = ((t.width >> l).max(1), (t.height >> l).max(1));
        let (row, rows, extent) = if t.format.is_compressed() {
            let block = if t.format == Format::Bc1 { 8 } else { 16 };
            (w.div_ceil(4) * block, h.div_ceil(4), (w.next_multiple_of(4), h.next_multiple_of(4)))
        } else {
            (w * 4, h, (w, h))
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: l as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(rows) },
            wgpu::Extent3d { width: extent.0, height: extent.1, depth_or_array_layers: 1 },
        );
    }
    Some(tex.create_view(&wgpu::TextureViewDescriptor { label: Some(label), ..Default::default() }))
}

/// The models' texture sampler: `mip_skip` is `ren_MipSkipLevels` (FAF's
/// Texture Detail: 0 high, 1 medium, 2 low), the finest mip levels never
/// sampled.
fn model_sampler(device: &wgpu::Device, mip_skip: u32) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("models: sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        lod_min_clamp: mip_skip as f32,
        anisotropy_clamp: 4,
        ..Default::default()
    })
}

/// A model's textures (albedo, spec-team, normals, lookup) with `sampler`.
fn texture_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, views: &[wgpu::TextureView; 4], name: &str) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("model textures: {name}")),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[0]) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[1]) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&views[2]) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&views[3]) },
        ],
    })
}

/// A build-in technique's secondary texture with `sampler`.
fn secondary_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, view: &wgpu::TextureView, name: &str) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("model build texture: {name}")),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) }],
    })
}

fn palette_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some("models: bone palette"), size: (capacity * 64) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

fn palette_bind_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, buffer: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("models: bone palette"), layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }] })
}

/// First index of the interface's meshes ([`Models::interface_mesh`]).
pub const INTERFACE_BASE: u32 = 1 << 30;

/// Reads one model's files and uploads its textures (worker thread).
fn load(gpu: &(wgpu::Device, wgpu::Queue), shared: &super::texture_load::Cache, share_all: bool, vfs: &RwLock<Vfs>, index: u32, asset: &RenderAsset, bc: bool) -> Loaded {
    let _s = rts_core_engine::prof_span!("load model");
    let read = |path: &str| vfs.read().ok().and_then(|v| v.read(path).ok());
    let mesh = match read(&asset.mesh) {
        Some(b) => scm::read_mesh(&b),
        None => Err(std::io::Error::new(std::io::ErrorKind::NotFound, asset.mesh.clone())),
    };
    // The model's own textures' GPU copies go in one submit (shared ones
    // are submitted as they load, before another model can use them).
    let mut encoder = gpu.0.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(&format!("upload: model textures {}", asset.mesh)) });
    let loaded = {
        let Ok(vfs) = vfs.read() else { return Loaded { index, mesh, albedo: None, specteam: None, normals: None, secondary: None, lookup: None } };
        let shared = |path: &str, srgb: bool, label: &str| shared.load(&gpu.0, &gpu.1, &vfs, path, srgb, bc, label);
        let mut texture = |path: &str, srgb: bool, label: &str| if share_all { shared(path, srgb, label) } else { super::texture_load::load_into(&gpu.0, &gpu.1, &mut encoder, &vfs, path, srgb, bc, label) };
        Loaded {
            index,
            albedo: texture(&asset.albedo, true, "albedo"),
            specteam: texture(&asset.specteam, false, "specteam"),
            normals: texture(&asset.normals, false, "normals"),
            secondary: asset.secondary.as_deref().and_then(|p| shared(p, true, "build secondary")),
            lookup: asset.lookup.as_deref().and_then(|p| shared(p, false, "lookup")),
            mesh,
        }
    };
    gpu.1.submit([encoder.finish()]);
    loaded
}

impl Models {
    /// The sim's meshes' bounds loaded since the last call (see `new_bounds`).
    pub fn take_new_bounds(&mut self) -> Vec<(u32, [f32; 6])> {
        std::mem::take(&mut self.new_bounds)
    }

    /// A sim mesh's bounds (mesh space, rest pose: min xyz, max xyz), once
    /// it has loaded.
    pub fn bounds(&self, index: u32) -> Option<[f32; 6]> {
        self.bounds.get(index as usize).copied().flatten()
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, globals: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> Self {
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("models: textures layout"),
            entries: &[tex_entry(0), tex_entry(1), wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }, tex_entry(3), tex_entry(4)],
        });
        let sampler = model_sampler(device, 0);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("models: shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let palette_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("models: bone palette layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("models: pipeline layout"), bind_group_layouts: &[Some(globals), Some(&layout), Some(&palette_layout)], immediate_size: 0 });
        let secondary_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("models: build texture layout"),
            entries: &[tex_entry(0), wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }],
        });
        let build_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("models: build pipeline layout"),
            bind_group_layouts: &[Some(globals), Some(&layout), Some(&palette_layout), Some(&secondary_layout)],
            immediate_size: 0,
        });
        use super::globals::GlowWrite;
        let blended_pipeline = |label: &str, layout: &wgpu::PipelineLayout, cull: Option<wgpu::Face>, depth_write: bool, compare: wgpu::CompareFunction, blend: wgpu::BlendState, glow: GlowWrite, (vs, fs): (&str, &str)| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(vs),
                buffers: &VERTEX_LAYOUTS,
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs),
                targets: &super::globals::scene_targets(format, Some(blend), glow),
                compilation_options: Default::default(),
            }),
            // Triangles wind counter-clockwise about their stored normals in
            // this (right-handed) view.
            primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode: cull, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let pipeline_with = |label: &str, layout: &wgpu::PipelineLayout, cull: Option<wgpu::Face>, depth_write: bool, compare: wgpu::CompareFunction, glow: GlowWrite, entry: (&str, &str)| blended_pipeline(label, layout, cull, depth_write, compare, wgpu::BlendState::ALPHA_BLENDING, glow, entry);
        let pipeline = |label, cull, depth_write, glow, entry| pipeline_with(label, &pipeline_layout, cull, depth_write, wgpu::CompareFunction::Less, glow, entry);
        let build_pipeline = pipeline_with("models: build", &build_layout, Some(wgpu::Face::Back), true, wgpu::CompareFunction::Less, GlowWrite::Keep, ("vs_build", "fs_build"));
        let build_overlay_pipeline = pipeline_with("models: build overlay", &build_layout, Some(wgpu::Face::Back), false, wgpu::CompareFunction::LessEqual, GlowWrite::Keep, ("vs_build", "fs_build_overlay"));
        let build_cube_pipeline = pipeline_with("models: UEF build cube", &build_layout, Some(wgpu::Face::Back), false, wgpu::CompareFunction::LessEqual, GlowWrite::Keep, ("vs_build", "fs_build"));
        // Shields: blended over everything, no depth writes; front faces
        // only (FAF's `ShieldFill` depth pass does that); the Seraphim
        // bubble adds (`AlphaBlend_SrcAlpha_One`).
        // (Their alpha blends into the glow too: `Write_RGBA`.)
        let shield_pipeline = pipeline_with("models: shield", &pipeline_layout, Some(wgpu::Face::Back), false, wgpu::CompareFunction::LessEqual, GlowWrite::Blend(wgpu::BlendState::ALPHA_BLENDING), ("vs_mesh", "fs_shield"));
        let shield_offset_pipeline = pipeline_with("models: shield offset", &pipeline_layout, Some(wgpu::Face::Back), false, wgpu::CompareFunction::LessEqual, GlowWrite::Blend(wgpu::BlendState::ALPHA_BLENDING), ("vs_shield_offset", "fs_shield"));
        let shield_add_pipeline = {
            let add = wgpu::BlendState {
                color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
                alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("models: shield (additive)"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_mesh"), buffers: &VERTEX_LAYOUTS, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_shield"),
                    // `AlphaBlend_SrcAlpha_One_Write_RGB`: no alpha write,
                    // so nothing goes into the glow (bloom).
                    targets: &super::globals::scene_targets(format, Some(add), GlowWrite::Keep),
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Effect techniques after the water, by `EffectPass` (`fs_effect`).
        let effect_pipelines = {
            use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O, CompareFunction as Z};
            let alpha = wgpu::BlendState::ALPHA_BLENDING;
            let add = wgpu::BlendState { color: C { src_factor: F::SrcAlpha, dst_factor: F::One, operation: O::Add }, alpha: C { src_factor: F::Zero, dst_factor: F::One, operation: O::Add } };
            let premultiplied = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
            let fx = ("vs_mesh", "fs_effect");
            let back = Some(wgpu::Face::Back);
            [
                blended_pipeline("models: effect alpha", &pipeline_layout, back, false, Z::LessEqual, alpha, GlowWrite::Keep, fx),
                blended_pipeline("models: effect alpha + depth", &pipeline_layout, back, true, Z::Less, alpha, GlowWrite::Keep, fx),
                blended_pipeline("models: effect alpha two-sided", &pipeline_layout, None, false, Z::LessEqual, alpha, GlowWrite::Blend(alpha), fx),
                blended_pipeline("models: effect additive", &pipeline_layout, back, false, Z::LessEqual, add, GlowWrite::Blend(add), fx),
                blended_pipeline("models: effect premultiplied", &pipeline_layout, back, false, Z::LessEqual, premultiplied, GlowWrite::Blend(premultiplied), fx),
                blended_pipeline("models: effect overlay", &pipeline_layout, back, false, Z::LessEqual, alpha, GlowWrite::Blend(alpha), ("vs_overlay", "fs_effect")),
                // `Depth_Disable`, `Write_RGB`: order feedback over everything.
                blended_pipeline("models: order feedback", &pipeline_layout, back, false, Z::Always, alpha, GlowWrite::Keep, ("vs_feedback", "fs_effect")),
            ]
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let palette_capacity = 1024;
        let palette = palette_buffer(device, palette_capacity);
        let palette_group = palette_bind_group(device, &palette_layout, &palette);
        Self {
            clips: Vec::new(),
            clip_paths: Vec::new(),
            bindings: Default::default(),
            draws: Default::default(),
            geometry: Geometry { chunks: Vec::new() },
            palette_layout,
            palette,
            palette_capacity,
            palette_group,
            failures: Vec::new(),
            assets: Vec::new(),
            slots: Vec::new(),
            interface_assets: Vec::new(),
            interface_slots: Vec::new(),
            interface_index: Default::default(),
            vfs: None,
            new_bounds: Vec::new(),
            bounds: Vec::new(),
            gpu: (device.clone(), queue.clone()),
            shared: Default::default(),
            share_all: false,
            done_tx,
            done_rx,
            bc: device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            white: solid(device, queue, "models: white (missing albedo)", [255; 4], wgpu::TextureFormat::Rgba8UnormSrgb),
            plain_spec: solid(device, queue, "models: black (missing spec / lookup)", [0; 4], wgpu::TextureFormat::Rgba8Unorm),
            flat_normals: solid(device, queue, "models: flat normals (missing)", [128, 128, 255, 128], wgpu::TextureFormat::Rgba8Unorm),
            layout,
            sampler,
            // Opaque meshes write their glow; alpha-tested and effect meshes
            // keep what's behind (`Write_RGB`).
            pipeline: pipeline("models: opaque", Some(wgpu::Face::Back), true, GlowWrite::Replace, MESH),
            cutout_pipeline: pipeline("models: cutout", None, true, GlowWrite::Keep, MESH),
            effect_pipeline: pipeline("models: effect (no technique)", None, false, GlowWrite::Keep, MESH),
            // `UnitPlace_LowFidelity`: blended, back faces culled, depth
            // tested and written (`Depth_Enable_Less`), so the ghost shows
            // its outer surface only.
            place_pipeline: pipeline("models: placement ghost", Some(wgpu::Face::Back), true, GlowWrite::Keep, ("vs_place", "fs_place")),
            secondary_layout,
            build_pipeline,
            build_overlay_pipeline,
            build_cube_pipeline,
            shield_pipeline,
            shield_offset_pipeline,
            shield_add_pipeline,
            effect_pipelines,
        }
    }

    /// The pipeline an effect technique's after-water pass draws with.
    pub fn effect_pipeline(&self, pass: crate::engine::api::mesh::EffectPass) -> &wgpu::RenderPipeline {
        use crate::engine::api::mesh::EffectPass as P;
        &self.effect_pipelines[match pass {
            P::Alpha => 0,
            P::AlphaWrite => 1,
            P::AlphaTwoSided => 2,
            P::Add => 3,
            P::Premultiplied => 4,
            P::Overlay => 5,
            P::Feedback => 6,
        }]
    }

    /// Takes the sim's newly named meshes (and the VFS, once).
    pub fn receive(&mut self, vfs: Option<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>, clips: Vec<(u32, String)>, meshes: Vec<(u32, crate::engine::api::mesh::RenderAsset)>) {
        if let Some(v) = vfs {
            self.vfs = Some(v);
        }
        for (i, path) in clips {
            let i = i as usize;
            if self.clips.len() <= i {
                self.clips.resize_with(i + 1, || ClipSlot::Failed);
            }
            if self.clip_paths.len() <= i {
                self.clip_paths.resize_with(i + 1, String::new);
            }
            self.clip_paths[i] = path.clone();
            self.clips[i] = ClipSlot::Idle(path);
        }
        for (i, asset) in meshes {
            let i = i as usize;
            if self.assets.len() <= i {
                self.assets.resize_with(i + 1, || None);
                self.slots.resize_with(i + 1, || Slot::Idle);
            }
            self.assets[i] = Some(asset);
        }
    }

    /// Files changed on disk (`--enable-disk-watch`; lowercase VFS paths):
    /// every model using one as its mesh or a texture, and every clip from
    /// one, loads again when next drawn (a model is a box until then).
    pub fn reload(&mut self, changed: &[String]) {
        let hit = |p: &str| !p.is_empty() && changed.iter().any(|c| *c == crate::engine::vfs::normalise(p));
        for p in changed {
            self.shared.forget(p);
        }
        let uses = |a: &RenderAsset| {
            [a.mesh.as_str(), &a.albedo, &a.specteam, &a.normals].into_iter().chain(a.secondary.as_deref()).chain(a.lookup.as_deref()).any(hit)
        };
        let stale: Vec<u32> = (0..self.assets.len() as u32)
            .chain((0..self.interface_assets.len() as u32).map(|k| INTERFACE_BASE + k))
            .filter(|&i| self.asset(i).is_some_and(uses))
            .collect();
        for &i in &stale {
            if let Some(slot) = self.slot_mut(i)
                && matches!(slot, Slot::Ready(_) | Slot::Failed) {
                *slot = Slot::Idle;
            }
        }
        let stale_clips: Vec<u32> = (0..self.clip_paths.len() as u32).filter(|&c| hit(&self.clip_paths[c as usize])).collect();
        for &c in &stale_clips {
            if matches!(self.clips[c as usize], ClipSlot::Ready(_) | ClipSlot::Failed) {
                self.clips[c as usize] = ClipSlot::Idle(self.clip_paths[c as usize].clone());
            }
        }
        if !stale.is_empty() || !stale_clips.is_empty() {
            self.bindings.retain(|(m, c), _| !stale.contains(m) && !stale_clips.contains(c));
            tracing::info!(models = stale.len(), clips = stale_clips.len(), "disk watch: reloading models");
        }
    }

    /// The level of detail of mesh `index` to draw at `distance` along the
    /// view from the camera (the mesh blueprint's `LODCutoff`s): `Some(index)` of that
    /// level's mesh, `None` beyond the last cutoff (not drawn).
    pub fn lod(&self, index: u32, distance: f32) -> Option<u32> {
        let Some(a) = self.asset(index) else { return Some(index) };
        if a.lods.is_empty() {
            return Some(index);
        }
        a.lods.iter().find(|(cutoff, _)| distance <= *cutoff).map(|&(_, m)| m)
    }

    /// Starts loading every level of detail of every known mesh (tests:
    /// LOD meshes otherwise load only when the camera needs them).
    #[allow(dead_code)]
    pub fn request_all_lods(&mut self) {
        let levels: Vec<u32> = self.assets.iter().flatten().flat_map(|a| a.lods.iter().map(|l| l.1)).collect();
        for l in levels {
            let _ = self.get(l);
        }
    }

    /// `ren_MipSkipLevels`: samples every model from `skip` mips down (the
    /// textures stay loaded whole, so a change rebinds them, no reload).
    pub fn set_mip_skip(&mut self, device: &wgpu::Device, skip: u32) {
        self.sampler = model_sampler(device, skip);
        let (layout, secondary_layout, sampler) = (&self.layout, &self.secondary_layout, &self.sampler);
        for slot in self.slots.iter_mut().chain(&mut self.interface_slots) {
            if let Slot::Ready(m) = slot {
                m.textures = texture_group(device, layout, sampler, &m.views, &m.name);
                m.secondary = m.secondary_view.as_ref().map(|v| secondary_group(device, secondary_layout, sampler, v, &m.name));
            }
        }
    }

    /// Loads every texture through `cache` from now on: a second head's
    /// models share it, so each texture is in GPU memory once.
    pub fn share_textures(&mut self, cache: super::texture_load::Cache) {
        self.shared = cache;
        self.share_all = true;
    }

    /// The texture cache shared with another head, if any.
    pub fn texture_share(&self) -> Option<super::texture_load::Cache> {
        self.share_all.then(|| self.shared.clone())
    }

    /// The model for mesh `index`, starting its load on first request.
    pub fn get(&mut self, index: u32) -> Option<&GpuModel> {
        match self.slot(index)? {
            Slot::Ready(_) => {}
            Slot::Idle => {
                let (Some(vfs), Some(asset)) = (self.vfs.clone(), self.asset(index).cloned()) else { return None };
                *self.slot_mut(index)? = Slot::Loading;
                let (tx, bc, gpu, shared, share_all) = (self.done_tx.clone(), self.bc, self.gpu.clone(), self.shared.clone(), self.share_all);
                rayon::spawn(move || {
                    let _ = tx.send(Done::Model(load(&gpu, &shared, share_all, &vfs, index, &asset, bc)));
                });
                return None;
            }
            Slot::Loading | Slot::Failed => return None,
        }
        match self.slot(index)? {
            Slot::Ready(m) => Some(m),
            _ => None,
        }
    }

    /// The model index of an interface mesh by its files and technique,
    /// registering it on first use (loaded like the sim's, when first
    /// drawn).
    pub fn interface_mesh(&mut self, mesh: &str, albedo: &str, shading: crate::engine::api::mesh::Shading) -> u32 {
        let key = (mesh.to_lowercase(), albedo.to_lowercase(), shading as u8);
        if let Some(&i) = self.interface_index.get(&key) {
            return i;
        }
        let i = INTERFACE_BASE + self.interface_assets.len() as u32;
        // The border round the map shades with its spec and normal maps,
        // named after its mesh (`map-border_squ_uef_lod0.scm` ->
        // `map-border_squ_uef_specteam.dds`).
        let named = |suffix: &str| match (shading, key.0.strip_suffix("_lod0.scm")) {
            (crate::engine::api::mesh::Shading::MapImager, Some(stem)) => format!("{stem}{suffix}"),
            _ => String::new(),
        };
        let (specteam, normals) = (named("_specteam.dds"), named("_normalsts.dds"));
        self.interface_assets.push(RenderAsset {
            mesh: key.0.clone(),
            albedo: key.1.clone(),
            specteam,
            wreck: false,
            cutout: false,
            effect: false,
            build: None,
            secondary: None,
            lookup: None,
            normals,
            shading,
            scrolling: false,
            vertex_motion: Default::default(),
            lods: Vec::new(),
        });
        self.interface_slots.push(Slot::Idle);
        self.interface_index.insert(key, i);
        i
    }

    fn asset(&self, index: u32) -> Option<&RenderAsset> {
        match index.checked_sub(INTERFACE_BASE) {
            Some(k) => self.interface_assets.get(k as usize),
            None => self.assets.get(index as usize)?.as_ref(),
        }
    }

    fn slot(&self, index: u32) -> Option<&Slot> {
        match index.checked_sub(INTERFACE_BASE) {
            Some(k) => self.interface_slots.get(k as usize),
            None => self.slots.get(index as usize),
        }
    }

    fn slot_mut(&mut self, index: u32) -> Option<&mut Slot> {
        match index.checked_sub(INTERFACE_BASE) {
            Some(k) => self.interface_slots.get_mut(k as usize),
            None => self.slots.get_mut(index as usize),
        }
    }

    /// The game's file system (from the first snapshot).
    pub fn vfs(&self) -> Option<&Arc<RwLock<Vfs>>> {
        self.vfs.as_ref()
    }

    /// Something is still loading (models or clips).
    #[allow(dead_code)]
    pub fn loading(&self) -> bool {
        self.slots.iter().chain(&self.interface_slots).any(|s| matches!(s, Slot::Loading)) || self.clips.iter().any(|c| matches!(c, ClipSlot::Loading))
    }

    /// Assets that failed to load, so far.
    #[allow(dead_code)]
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// Starts loading clip `index` if it isn't yet.
    pub fn request_clip(&mut self, index: u32) {
        let Some(slot) = self.clips.get_mut(index as usize) else { return };
        let ClipSlot::Idle(path) = slot else { return };
        let Some(vfs) = self.vfs.clone() else { return };
        let path = std::mem::take(path);
        *slot = ClipSlot::Loading;
        let tx = self.done_tx.clone();
        rayon::spawn(move || {
            let _s = rts_core_engine::prof_span!("load clip");
            let clip = vfs
                .read()
                .ok()
                .and_then(|v| v.read(&path).ok())
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, path.clone()))
                .and_then(|b| sca::read_clip(&b));
            let _ = tx.send(Done::Clip(index, clip));
        });
    }

    /// Makes sure mesh `mesh`'s binding to clip `clip` is cached (once both
    /// are loaded).
    pub fn bind(&mut self, mesh: u32, clip: u32) {
        if self.bindings.contains_key(&(mesh, clip)) {
            return;
        }
        let (Some(Slot::Ready(m)), Some(ClipSlot::Ready(c))) = (self.slot(mesh), self.clips.get(clip as usize)) else { return };
        let b = m.skeleton.bind(c);
        self.bindings.insert((mesh, clip), b);
    }

    /// Uploads this frame's bone palette.
    pub fn upload_palette(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, palette: &[[[f32; 4]; 4]]) {
        if palette.len() > self.palette_capacity {
            self.palette_capacity = palette.len().next_power_of_two();
            self.palette = palette_buffer(device, self.palette_capacity);
            self.palette_group = palette_bind_group(device, &self.palette_layout, &self.palette);
        }
        if !palette.is_empty() {
            queue.write_buffer(&self.palette, 0, bytemuck::cast_slice(palette));
        }
    }

    /// A read-only view of the store to share across threads (the store
    /// itself can't be: its loader channel), for the pose pass.
    pub fn view(&self) -> ModelsView<'_> {
        ModelsView { slots: &self.slots, interface_slots: &self.interface_slots, clips: &self.clips, bindings: &self.bindings }
    }

    /// The model for mesh `index` if it is loaded (no load request).
    pub fn ready(&self, index: u32) -> Option<&GpuModel> {
        match self.slot(index) {
            Some(Slot::Ready(m)) => Some(m),
            _ => None,
        }
    }

    /// Uploads finished loads (a few per frame).
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let _s = rts_core_engine::prof_span!("upload models");
        for _ in 0..UPLOADS_PER_FRAME {
            let l = match self.done_rx.try_recv() {
                Ok(Done::Model(l)) => l,
                Ok(Done::Clip(i, clip)) => {
                    self.clips[i as usize] = match clip {
                        Ok(c) => ClipSlot::Ready(Arc::new(c)),
                        Err(e) => {
                            tracing::debug!("clip {i}: {e}");
                            self.failures.push(format!("clip {i}: {e}"));
                            ClipSlot::Failed
                        }
                    };
                    continue;
                }
                Err(_) => break,
            };
            let index = l.index;
            let asset = self.asset(index).cloned();
            let asset = asset.as_ref();
            let (wreck, cutout, effect) = asset.map_or((false, false, false), |a| (a.wreck, a.cutout, a.effect));
            let build = asset.and_then(|a| a.build);
            let loaded = match l.mesh {
                Ok(mesh) if !mesh.indices.is_empty() => {
                    if index < INTERFACE_BASE {
                        let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
                        for p in &mesh.positions {
                            for k in 0..3 {
                                b[k] = b[k].min(p[k]);
                                b[k + 3] = b[k + 3].max(p[k]);
                            }
                        }
                        self.new_bounds.push((index, b));
                        if self.bounds.len() <= index as usize {
                            self.bounds.resize(index as usize + 1, None);
                        }
                        self.bounds[index as usize] = Some(b);
                    }
                    let verts: Vec<MeshVertex> = (0..mesh.positions.len())
                        .map(|v| MeshVertex {
                            position: mesh.positions[v],
                            normal: mesh.normals[v],
                            uv: mesh.uvs[v],
                            bone: mesh.bone[v] as u32,
                            tangent: mesh.tangents[v],
                            binormal: mesh.binormals[v],
                            uv1: mesh.uvs1[v],
                        })
                        .collect();
                    // Index buffers are copied in 4-byte units.
                    let mut indices = mesh.indices.clone();
                    if indices.len() % 2 == 1 {
                        indices.push(0);
                    }
                    let albedo = l.albedo;
                    if albedo.is_none() {
                        let path = asset.map_or("?", |a| a.albedo.as_str());
                        let name = asset.map_or("?", |a| a.mesh.as_str());
                        tracing::warn!("model {name}: albedo {path} not found (white stand-in)");
                        self.failures.push(format!("albedo {path}"));
                    }
                    let (spec, normals, lookup) = (l.specteam, l.normals, l.lookup);
                    let views = [
                        albedo.unwrap_or_else(|| self.white.clone()),
                        spec.unwrap_or_else(|| self.plain_spec.clone()),
                        normals.unwrap_or_else(|| self.flat_normals.clone()),
                        lookup.unwrap_or_else(|| self.plain_spec.clone()),
                    ];
                    let name = asset.map_or("?", |a| a.mesh.as_str()).to_owned();
                    let textures = texture_group(device, &self.layout, &self.sampler, &views, &name);
                    let secondary_view = build.map(|_| {
                        let view = l.secondary.clone();
                        if view.is_none() {
                            self.failures.push(format!("secondary {}", asset.and_then(|a| a.secondary.as_deref()).unwrap_or("?")));
                        }
                        view.unwrap_or_else(|| self.white.clone())
                    });
                    let secondary = secondary_view.as_ref().map(|v| secondary_group(device, &self.secondary_layout, &self.sampler, v, &name));
                    let (chunk, base_vertex, first_index) = self.geometry.push(device, queue, &verts, &indices);
                    Slot::Ready(GpuModel {
                        name,
                        chunk,
                        base_vertex,
                        first_index,
                        index_count: mesh.indices.len() as u32,
                        textures,
                        views,
                        secondary_view,
                        wreck,
                        cutout,
                        effect,
                        build,
                        secondary,
                        shading: asset.map_or(Default::default(), |a| a.shading),
                        scrolling: asset.is_some_and(|a| a.scrolling),
                        vertex_motion: asset.map_or(Default::default(), |a| a.vertex_motion),
                        radius: mesh.positions.iter().map(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt()).fold(0.0, f32::max),
                        skeleton: rts_core_engine::render_pose::Skeleton::new(&mesh.bones),
                    })
                }
                Ok(_) => {
                    self.failures.push(format!("mesh {} (empty)", asset.map_or("?", |a| a.mesh.as_str())));
                    Slot::Failed
                }
                Err(e) => {
                    tracing::debug!("model {}: {e}", asset.map_or("?", |a| a.mesh.as_str()));
                    self.failures.push(format!("mesh {}: {e}", asset.map_or("?", |a| a.mesh.as_str())));
                    Slot::Failed
                }
            };
            if let Some(slot) = self.slot_mut(index) {
                *slot = loaded;
            }
        }
    }

    /// Starts drawing models in `pass` from `instances`: binds what every
    /// draw shares (the bone palette, the instance buffer). Call again after
    /// a pipeline of another layout (water, particles, the order lines) has
    /// been used in the pass, with group 0; the returned state tracks the
    /// per-mesh binds.
    pub fn begin<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, instances: &'a wgpu::Buffer) -> Bound {
        pass.set_bind_group(2, &self.palette_group, &[]);
        pass.set_vertex_buffer(1, instances.slice(..));
        Bound::default()
    }

    /// Draws `instances[range]` of model `index` (pipeline already set),
    /// binding only what the last draw in `bound` did not.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, bound: &mut Bound, index: u32, range: std::ops::Range<u32>) {
        let Some(m) = self.ready(index) else { return };
        if bound.chunk != Some(m.chunk) {
            let c = &self.geometry.chunks[m.chunk as usize];
            pass.set_vertex_buffer(0, c.vertices.slice(..));
            pass.set_index_buffer(c.indices.slice(..), wgpu::IndexFormat::Uint16);
            bound.chunk = Some(m.chunk);
        }
        if bound.mesh != Some(index) {
            pass.set_bind_group(1, &m.textures, &[]);
            if let Some(s) = &m.secondary {
                pass.set_bind_group(3, s, &[]);
            }
            bound.mesh = Some(index);
        }
        pass.draw_indexed(m.first_index..m.first_index + m.index_count, m.base_vertex, range);
        self.draws.set(self.draws.get() + 1);
    }
}
