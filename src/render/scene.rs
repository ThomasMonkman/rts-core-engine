//! The 3D scene drawn from sim snapshots: terrain heightfield, water, and
//! every entity as its model (`models`), instanced per mesh, or as a box
//! (its collision volume) in army colour while it has none loaded.
//!
//! Entities are interpolated between the last two snapshots: snapshots
//! arrive every sim tick (100 ms) and the frame shows the state `alpha` of
//! the way from the previous one to the latest, so motion is smooth at any
//! frame rate (one tick of display latency).

use super::gpu_prof::Timed;
use std::sync::Arc;
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use wgpu::util::DeviceExt;

use crate::engine::snapshot::{EntityView, Snapshot, TerrainView, ViewKind};

use super::camera::{Camera, LodPlane};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The terrain's level of detail: the coarsest whose cells are at most this
/// many pixels across at the camera's target.
const TERRAIN_LOD_PIXELS: f32 = 3.0;

/// The terrain mesh: one vertex buffer, an index buffer per level of
/// detail (every 1st, 2nd, 4th, 8th vertex; the same triangulation), each
/// laid out tile by tile so a frame draws the tiles in view.
struct TerrainMesh {
    vertices: wgpu::Buffer,
    lods: Vec<TerrainLod>,
    /// Each tile's bounding sphere (centre, radius), tiles row-major.
    tile_bounds: Vec<(Vec3, f32)>,
}

struct TerrainLod {
    indices: wgpu::Buffer,
    tiles: Vec<std::ops::Range<u32>>,
    /// Its sample step in height samples (world units).
    step: u32,
}
const TICK_SECONDS: f32 = 0.1;

use super::globals::Globals;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TerrainVertex {
    position: [f32; 3],
    normal: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BoxVertex {
    position: [f32; 3],
    normal: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    model: [[f32; 4]; 4],
    color: [f32; 4],
}

const SHADER: &str = include_str!(concat!(env!("OUT_DIR"), "/scene.wgsl"));

/// An army's colour (`table` from [`Scene::army_rgb`], index = army - 1);
/// our palette for an army the sim sent none for, neutral grey for none.
pub(crate) fn army_color(table: &[[f32; 3]], army: u32) -> [f32; 3] {
    let i = army.checked_sub(1).map(|i| i as usize);
    i.and_then(|i| table.get(i).copied()).unwrap_or_else(|| crate::engine::api::colors::rgb(crate::engine::api::colors::fallback_army_color(army)))
}

/// A unit or structure short of finished (drawn translucent when its mesh
/// has no build technique). Not a prop: a partly reclaimed wreck or tree has
/// a fraction under 1 too, and moho draws it as it was.
fn under_construction(e: &EntityView) -> bool {
    e.build < 1.0 && matches!(e.kind, ViewKind::Unit | ViewKind::Structure)
}

fn entity_color(e: &EntityView, army_rgb: &[[f32; 3]]) -> [f32; 4] {
    let [r, g, b] = match e.kind {
        ViewKind::Projectile => [1.0, 0.9, 0.4],
        ViewKind::Prop => [0.35, 0.42, 0.28],
        _ => army_color(army_rgb, e.army),
    };
    // Structures a little darker; constructions translucent.
    let k = if e.kind == ViewKind::Structure { 0.8 } else { 1.0 };
    let a = if under_construction(e) { 0.45 } else { 1.0 };
    [r * k, g * k, b * k, a]
}

/// What the prepare worker reads of the camera.
#[derive(Clone, Copy, Debug)]
pub struct CamView {
    pub eye: Vec3,
    pub target: Vec3,
    pub view_proj: Mat4,
    /// The zoom, which strategic icons show by.
    pub zoom: f32,
    pub lod_scale: f32,
}

impl CamView {
    pub fn of(c: &Camera) -> Self {
        Self { eye: c.eye(), target: c.target, view_proj: c.view_proj(), zoom: c.zoom, lod_scale: c.lod_scale }
    }

    pub fn view_proj(&self) -> Mat4 {
        self.view_proj
    }

    pub fn lod_plane(&self) -> LodPlane {
        LodPlane::new(self.eye, self.target, self.lod_scale)
    }
}

/// A strategic icon this frame: entity, position, army, blueprint index,
/// what it draws as, alpha.
pub type Icon = (u32, [f32; 3], u32, u32, IconKind, f32);

/// How an icon draws: at the unit, in its army's colour (`Full`); an
/// unidentified radar / sonar contact (`Blip`: a generic shape, grey); or
/// a scouted structure now out of sight (`Remembered`: its own icon
/// greyed; the mesh keeps its last-seen look, darkened by the fog).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconKind {
    Full,
    Blip,
    Remembered,
}

/// World views a frame poses for: past this many, a view shows meshes but no
/// strategic icons (FAF makes at most three: split view's two, a second
/// head's).
pub const MAX_VIEWS: usize = 4;

/// A value lent to the prepare worker for part of the frame: taken out of
/// the scene before the worker starts and put back when it joins, so the
/// interface, which runs meanwhile on the main thread, keeps the scene.
/// Dereferencing it while it is out panics; no interface path does.
pub struct Lent<T>(Option<T>);

impl<T> Lent<T> {
    fn new(v: T) -> Self {
        Self(Some(v))
    }

    fn take(&mut self) -> T {
        self.0.take().expect("lent to the prepare worker")
    }

    fn put(&mut self, v: T) {
        debug_assert!(self.0.is_none(), "put back twice");
        self.0 = Some(v);
    }
}

impl<T> std::ops::Deref for Lent<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_ref().expect("lent to the prepare worker")
    }
}

impl<T> std::ops::DerefMut for Lent<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_mut().expect("lent to the prepare worker")
    }
}

/// The scene's per-frame work and the stores it needs (`Stage::run`),
/// lent to a worker thread while the interface runs (`Scene::stage`). Its
/// inputs are filled by `Scene::prepare_begin`; its entity and icon lists
/// are this frame's, swapped with the scene's published ones (which the
/// interface reads meanwhile) by `Scene::prepare_finish`.
pub struct Stage {
    pub models: super::models::Models,
    pub particles: Lent<super::particles::Particles>,
    latest: Option<Arc<Snapshot>>,
    previous: Option<Arc<Snapshot>>,
    alpha: f32,
    /// What each world view's camera sees, in view order (none before the
    /// world shows).
    views: Vec<CamView>,
    blueprint_icons: Option<Arc<Vec<String>>>,
    blueprint_icon_zoom: Option<Arc<Vec<f32>>>,
    army_rgb: Vec<[f32; 3]>,
    record_all_bones: bool,
    always_icons: bool,
    terrain_view: Option<Arc<TerrainView>>,
    /// This frame's interpolated entities (swapped into the scene).
    pub frame_entities: Vec<EntityView>,
    /// This frame's strategic icons per view, and each view's camera
    /// (swapped into the scene).
    pub icons: Vec<Vec<Icon>>,
    icon_cameras: Vec<String>,
    /// Indices into `frame_entities` of the on-screen units and structures
    /// that show a health or build bar (swapped into the scene: the world
    /// overlays draw those rather than test every entity each frame).
    pub bars: Vec<u32>,
    frame_prev_pose: Vec<(u32, u32)>,
    frame_lod: Vec<Option<u32>>,
    /// With more than one view: per frame entity, the views (bits) it is
    /// past its mesh's last LOD cutoff in.
    frame_past: Vec<u8>,
    model_list: Vec<(u32, super::models::MeshInstance)>,
    palette: Vec<[[f32; 4]; 4]>,
    /// The posed entities whose bones effects follow (all, when recording):
    /// (id, (model matrix, palette base or `NO_BONES`, mesh)), in id order.
    frame_bones: Vec<(u32, (Mat4, u32, u32))>,
    /// The pose pass's plan per entity, and its slot lists (reused).
    plans: Vec<Plan>,
    instance_entities: Vec<u32>,
    posed: Vec<u32>,
    bone_entities: Vec<u32>,
    pub boxed: Vec<u32>,
    /// Box instances for units whose model failed to load.
    boxes: Vec<Instance>,
    effect_entities: Vec<u32>,
    effect_ages: std::collections::BTreeMap<u32, (u32, f32, u32)>,
    age_generation: u32,
    pub culled: usize,
}

pub struct Scene {
    /// Group 0's layout; each view binds its own globals with the shared
    /// textures (`ViewGpu::bind_group`), remade when those change
    /// (`bind_generation`).
    layout: wgpu::BindGroupLayout,
    bind_generation: u64,
    sampler: wgpu::Sampler,
    env: EnvBindings,
    /// Grid overlay shown on the terrain: (version, mode, extent).
    overlay: Option<(u64, f32, [f32; 2])>,
    terrain_pipeline: wgpu::RenderPipeline,
    box_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    terrain: Option<TerrainMesh>,
    /// The terrain's heights, every sample ((width + 1) x (height + 1)
    /// f32, group 0 binding 10): decals and splats build their
    /// ground-hugging grids from it in the vertex shader.
    heights: wgpu::Buffer,
    pub stage: Lent<Stage>,
    vfs: Option<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>,
    /// The map's own terrain, decals and water, once its textures load.
    pub map: super::terrain::TerrainRenderer,
    /// Particle effects, beams and trails.
    /// The selected units' order lines and waypoints.
    pub command_graph: super::command_graph::CommandGraph,
    /// Scripts' debug lines: the sim canvas and the views' world shapes.
    pub debug_lines: super::debug_lines::DebugLines,
    /// Resource deposits' icons on the zoomed-out view.
    pub resource_icons: super::resource_icons::ResourceIcons,
    /// Surface size (pixels) and the cursor on it (set by the frame).
    screen: [f32; 2],
    /// Sun shadows (the cascaded shadow map the views share, drawing in
    /// turn).
    shadows: super::shadows::Shadows,
    /// The overlay texture bound in group 0 (rebinding keeps it).
    overlay_view: wgpu::TextureView,
    /// The view army's fog of war texture (group 0 binding 9): a level a
    /// cell, updated by the rows that change (`Snapshot::fog`).
    fog_view: wgpu::TextureView,
    fog_texture: Option<wgpu::Texture>,
    /// The fog grid's world extent while fog is on.
    fog_extent: Option<[f32; 2]>,
    /// Fog changes received, applied in order at the next prepare.
    pending_fog: Vec<crate::engine::snapshot::FogUpdate>,
    /// The view army's side's vision circles this frame (group 0 bindings
    /// 12 to 14): the clear ground while fog is on.
    vision: super::vision::VisionCircles,
    /// A lit 1x1 stand-in for the terrain horizon until the map loads.
    lit: wgpu::TextureView,
    horizon_bound: bool,
    /// Lowest and highest terrain height (cascade fitting).
    height_range: (f32, f32),
    started: Instant,
    /// Seconds the shaders' clock reads instead of [`Scene::started`]
    /// (golden images: pulsing glows hold still).
    pub frozen_time: Option<f32>,
    /// What each world view draws with this frame, by view (`prepare_view`).
    views_gpu: Vec<Option<ViewGpu>>,
    /// This frame's model instances (grouped by mesh) and draw ranges.
    model_instances: wgpu::Buffer,
    model_capacity: usize,
    model_draws: Vec<(u32, std::ops::Range<u32>)>,
    /// Timed splats (tread marks, footprints) since the last frame, for
    /// the terrain renderer.
    pending_splats: Vec<super::terrain::Splat>,
    frame_game_seconds: f32,
    /// Its mesh and instance slot (after the models') once prepared.
    ghost_draw: Option<(u32, u32)>,
    /// Queued builds' ghosts (mesh, instance slot): the selected units' build
    /// orders whose site has no structure yet.
    queued_ghost_draws: Vec<(u32, u32)>,
    template_ghost_draws: Vec<(u32, u32)>,
    /// Per frame entity: its pose slice in the previous snapshot.
    /// This frame's bone matrices.
    /// Per drawn model entity: its model matrix, first palette entry
    /// (`NO_BONES` = rest pose) and mesh, for effects on its bones.
    /// Render-state tests: a fixed interpolation fraction (else from the
    /// wall clock), and bone lookups for every drawn model, not only those
    /// carrying effects.
    pub fixed_alpha: Option<f32>,
    pub record_all_bones: bool,
    /// Entities with a mesh still drawn as boxes this frame (loading or
    /// failed).
    /// Entities outside the view this frame (not posed or drawn).
    /// Per frame entity: the mesh level of detail to draw (`None` = beyond
    /// its last cutoff, `Some(u32::MAX)` = no mesh).
    water: Option<(wgpu::Buffer, f32)>,
    cube: (wgpu::Buffer, wgpu::Buffer, u32),
    instances: wgpu::Buffer,
    instance_capacity: usize,
    instance_count: u32,
    depth: wgpu::TextureView,
    depth_texture: wgpu::Texture,
    ranges: super::ranges::Ranges,
    /// The frame and glow targets, and the bloom that finishes the frame.
    pub bloom: super::bloom::Bloom,
    smaa: super::smaa::Smaa,
    /// Latest and previous snapshot, and when the latest arrived.
    pub latest: Option<Arc<Snapshot>>,
    /// `latest` has been through `prepare` (its effects emitted). A
    /// snapshot replaced before that (two taken in one slow frame) hands
    /// the effects only it had to the particle system.
    latest_prepared: bool,
    previous: Option<Arc<Snapshot>>,
    arrived: Instant,
    pub map_size: Option<(f32, f32)>,
    /// Units drawn as strategic icons this frame, per view, and the camera
    /// each list is for (the views the frame posed for).
    pub icons: Vec<Vec<Icon>>,
    pub icon_cameras: Vec<String>,
    /// Interpolated entities this frame (for overlays / picking).
    pub frame_entities: Vec<EntityView>,
    /// See `Stage::bars`.
    pub bars: Vec<u32>,
    arrivals: std::collections::VecDeque<(Instant, u64)>,
    /// Terrain heights kept for camera height and overlays.
    pub terrain_view: Option<Arc<TerrainView>>,
    /// A terrain change being worked out off the render thread
    /// ([`terrain_patch`]), and the newest one waiting for it to finish.
    terrain_patch: Option<std::sync::mpsc::Receiver<TerrainPatch>>,
    terrain_next: Option<Arc<TerrainView>>,
    /// What the main thread decided for this frame (`set_input`): the
    /// cameras and views, the selection, ghosts, overlays, settings.
    input: super::world::FrameInput,
    /// The settings and interface decals last applied (`set_input`).
    applied: Option<super::world::RenderSettings>,
    user_decals: Option<Arc<Vec<crate::engine::assets::scmap::Decal>>>,
}


/// What a world view draws with that its camera decides, written by
/// `Scene::prepare_view` and read by `Scene::draw_view`. Each view has its
/// own, so every view is prepared before any draws and all their command
/// buffers go to the GPU together. The screen-sized targets and the shadow
/// maps are shared: the GPU runs the views' passes in turn.
struct ViewGpu {
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The `Scene::bind_generation` its bind group was made for.
    bound: u64,
    /// The terrain's level of detail (an index into `TerrainMesh::lods`),
    /// its sample step, and the visible tiles' index runs (adjacent ones
    /// merged).
    terrain_lod: usize,
    terrain_step: u32,
    terrain_draws: Vec<std::ops::Range<u32>>,
    decals: super::terrain::DecalDraws,
    graph: super::command_graph::GraphFrame,
    lines: super::debug_lines::LinesFrame,
    icons: super::resource_icons::IconsFrame,
    ranges: super::ranges::RangesFrame,
    cascades: Option<super::shadows::Cascades>,
    lights: super::shadows::Lights,
    /// Per cascade: the shadow casters in it (instances in
    /// `shadow_instances`, draw ranges by mesh).
    shadow_instances: wgpu::Buffer,
    shadow_capacity: usize,
    shadow_draws: Vec<Vec<(u32, std::ops::Range<u32>)>>,
}

/// A unit or structure that can carry a lifebar (whose bars show is
/// `frame::bar_visible`, moho's rule): not a blip or a remembered
/// structure (only its icon shows), not an upgrade's unit (no collision
/// while it's built inside the old structure: the structure's own bar
/// shows its progress), not a killed one (no health left: an aircraft
/// falling, a death animation).
pub fn shows_bar(e: &EntityView) -> bool {
    if !matches!(e.kind, ViewKind::Unit | ViewKind::Structure) || e.sighting != crate::engine::snapshot::Sighting::Seen {
        return false;
    }
    if e.build < 1.0 {
        return e.half != [0.0; 3];
    }
    e.health > 0.0
}

/// Entities from which the interpolation and pose passes split across
/// threads (rayon's wake-up is ~100 us).
const PARALLEL_MIN: usize = 4096;
/// Posed entities per bone-matrix group (each group writes one run of the
/// palette).
const POSE_GROUP: usize = 1024;

/// What the pose pass makes of an entity, decided and sized in a first,
/// parallel pass so the second writes straight into the slots.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum What {
    #[default]
    Skip,
    Blip,
    Icon,
    Culled,
    Instance,
    Box,
}

#[derive(Clone, Copy)]
struct Plan {
    what: What,
    mesh: u32,
    /// Bone matrices it needs, and where its run starts in the palette.
    bones: u32,
    offset: u32,
    /// Its bones are recorded for the effects it carries.
    effect: bool,
    /// A timed effect technique (its age goes in the instance).
    timed: bool,
    /// Drawn (`ShieldFill` is posed but not drawn).
    drawn: bool,
    bar: bool,
    /// A box: its model failed to load.
    failed: bool,
    /// Its strategic icon shows too, per view, at this alpha (`NO_ICON`:
    /// not in that view): that view's camera is zoomed out past the
    /// blueprint's `IconFadeInZoom`, or past the mesh's last LOD cutoff.
    icons: [f32; MAX_VIEWS],
}

impl Default for Plan {
    fn default() -> Self {
        Self { what: What::Skip, mesh: 0, bones: 0, offset: 0, effect: false, timed: false, drawn: false, bar: false, failed: false, icons: [NO_ICON; MAX_VIEWS] }
    }
}

/// `Plan::icons` for a view that shows none.
const NO_ICON: f32 = -1.0;

/// World units an entity may lie outside the view and still be drawn: its
/// shadow (cast into view by the low sun) and interpolation slack.
const CULL_MARGIN: f32 = 12.0;

/// The six planes of a view-projection's frustum (wgpu depth 0..1), each
/// `(n, d)` with `n . p + d >= 0` inside, normalised.
fn frustum_planes(m: Mat4) -> [glam::Vec4; 6] {
    let r = |i: usize| m.row(i);
    let planes = [r(3) + r(0), r(3) - r(0), r(3) + r(1), r(3) - r(1), r(2), r(3) - r(2)];
    planes.map(|p| p / p.truncate().length().max(1e-6))
}

/// Fog of war edges: a separable Gaussian (5 taps, sigma about one cell) over
/// the coverage cells, so vision fades out over a few cells instead of
/// ending in hard squares (the shader then interpolates between cells).
fn overlay_texture(device: &wgpu::Device, label: &str, w: u32, h: u32) -> (wgpu::TextureView, wgpu::Texture) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    (super::view(&tex, label), tex)
}

#[allow(clippy::too_many_arguments)]
fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    overlay: &wgpu::TextureView,
    fog: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    shadows: &super::shadows::Shadows,
    horizon: &wgpu::TextureView,
    env: &EnvBindings,
    heights: &wgpu::Buffer,
    vision: &super::vision::VisionCircles,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene: globals"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(overlay) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&shadows.array_view) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&shadows.compare_sampler) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(horizon) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(env.cube.as_ref().unwrap_or(&env.default_cube)) },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(env.brdf.as_ref().unwrap_or(&env.default_brdf)) },
            wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&env.sampler) },
            wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(fog) },
            wgpu::BindGroupEntry { binding: 10, resource: heights.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::TextureView(env.insect.as_ref().unwrap_or(&env.default_brdf)) },
            wgpu::BindGroupEntry { binding: 12, resource: vision.rings.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 13, resource: vision.cells.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 14, resource: vision.lists.as_entire_binding() },
        ],
    })
}

/// Group 0's lighting extras: the map's environment cube and FAF's BRDF
/// lookup (neutral stand-ins until the map loads), and their sampler.
pub struct EnvBindings {
    pub cube: Option<wgpu::TextureView>,
    pub brdf: Option<wgpu::TextureView>,
    /// `/textures/engine/insectlookup.dds` (mesh.fx's `insectTexture`).
    pub insect: Option<wgpu::TextureView>,
    pub default_cube: wgpu::TextureView,
    pub default_brdf: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl EnvBindings {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        // A mid-grey sky; a lookup giving half Fresnel scale, no bias.
        let cube = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("scene: default environment cube"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 6 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[96u8; 24],
        );
        Self {
            cube: None,
            brdf: None,
            insect: None,
            default_cube: cube.create_view(&wgpu::TextureViewDescriptor { label: Some("scene: default environment cube"), dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() }),
            default_brdf: super::models::solid(device, queue, "scene: default BRDF lookup", [128, 0, 0, 255], wgpu::TextureFormat::Rgba8Unorm),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene: environment sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
        }
    }
}

/// The build placement ghost: a structure's mesh where it would go.
/// (`Scene::ghost_drawn`: it was prepared this frame.)
#[derive(Clone, Copy, Debug)]
pub struct Ghost {
    /// Blueprint index (`Snapshot::blueprints`).
    pub blueprint: u32,
    pub position: Vec3,
    /// The ghost's colour (FAF's `UnitPlace` lights the instance colour).
    pub color: [f32; 3],
    /// Its build skirt (world XZ), outlined on the ground like a queued
    /// build's site.
    pub site: Option<[f32; 4]>,
}

/// Queued builds' ghost colour: moho's `SetColor(0xD800D800)` (ARGB: green
/// 216), the same as the placement ghost's (MohoEngine.dll 0x103efa29).
const QUEUED_GHOST: [f32; 3] = [0.0, 216.0 / 255.0, 0.0];

fn instance_buffer(device: &wgpu::Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

/// The scene's depth buffer (copied for the range overlays: the ground's
/// depth before units draw).
fn depth_target(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene: depth"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let v = super::view(&t, "scene: depth");
    (t, v)
}

fn cube(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer, u32) {
    // Unit cube centred on the origin, 4 vertices per face (flat normals).
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let mut verts = Vec::new();
    let mut idx: Vec<u16> = Vec::new();
    for (n, u, v) in faces {
        let base = verts.len() as u16;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = [
                0.5 * (n[0] + su * u[0] + sv * v[0]),
                0.5 * (n[1] + su * u[1] + sv * v[1]),
                0.5 * (n[2] + su * u[2] + sv * v[2]),
            ];
            verts.push(BoxVertex { position: p, normal: n });
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("scene: box vertices"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("scene: box indices"), contents: bytemuck::cast_slice(&idx), usage: wgpu::BufferUsages::INDEX });
    (vb, ib, idx.len() as u32)
}

impl Stage {
    fn new(models: super::models::Models, particles: super::particles::Particles) -> Self {
        Self {
            models,
            particles: Lent::new(particles),
            latest: None,
            previous: None,
            alpha: 0.0,
            views: Vec::new(),
            blueprint_icons: None,
            blueprint_icon_zoom: None,
            army_rgb: Vec::new(),
            always_icons: false,
            record_all_bones: false,
            terrain_view: None,
            frame_entities: Vec::new(),
            icons: Vec::new(),
            icon_cameras: Vec::new(),
            bars: Vec::new(),
            frame_prev_pose: Vec::new(),
            frame_lod: Vec::new(),
            frame_past: Vec::new(),
            model_list: Vec::new(),
            palette: Vec::new(),
            frame_bones: Default::default(),
            boxed: Vec::new(),
            boxes: Vec::new(),
            effect_entities: Vec::new(),
            effect_ages: std::collections::BTreeMap::new(),
            age_generation: 0,
            culled: 0,
            plans: Vec::new(),
            instance_entities: Vec::new(),
            posed: Vec::new(),
            bone_entities: Vec::new(),
        }
    }

    /// The pose passes (see `run`): plan, slots, instances, bones.
    fn pose_passes(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, latest: &Snapshot, prev_poses: &[crate::engine::snapshot::Pose], alpha: f32) {
        let poses: &[crate::engine::snapshot::Pose] = &latest.poses;
        let pose_span = rts_core_engine::prof_span!("pose bones + instances");
        self.model_list.clear();
        self.age_generation = self.age_generation.wrapping_add(1);
        self.palette.clear();
        self.frame_bones.clear();
        self.boxed.clear();
        for icons in &mut self.icons {
            icons.clear();
        }
        self.bars.clear();
        self.boxes.clear();
        // Entities that carry effects (their bones are recorded below).
        self.effect_entities.clear();
        self.effect_entities.extend(latest.effects.iter().map(|e| e.entity).filter(|&id| id != u32::MAX));
        self.effect_entities.sort_unstable();
        self.effect_entities.dedup();
        // Each view's frustum (planes facing in), for culling entities no
        // view can see before they're posed and instanced. Off in tests that
        // want every entity's bones.
        let frustums: Vec<[glam::Vec4; 6]> = if self.record_all_bones { Vec::new() } else { self.views.iter().map(|c| frustum_planes(c.view_proj())).collect() };
        let now = latest.sim_tick as f32 - 1.0 + alpha;
        let n = self.frame_entities.len();
        let models = self.models.view();
        let views = &self.views[..self.views.len().min(MAX_VIEWS)];
        // 1. Plan (parallel): what each entity becomes and how many bone
        // matrices it needs. The model store is read through a shared view.
        {
            let (effect_entities, blueprint_icons, icon_zoom, record_all_bones, always_icons) = (&self.effect_entities, self.blueprint_icons.as_deref(), self.blueprint_icon_zoom.as_deref(), self.record_all_bones, self.always_icons);
            let plan_of = |e: &EntityView, lod: Option<u32>, past: u8| -> Plan {
                let mut p = Plan::default();
                // A radar / sonar contact: an unidentified strategic icon
                // at any zoom, never the mesh.
                if e.sighting == crate::engine::snapshot::Sighting::Blip {
                    p.what = What::Blip;
                    return p;
                }
                let unit = matches!(e.kind, ViewKind::Unit | ViewKind::Structure);
                // An upgrade being built inside its structure (no box) shows
                // no icon of its own: the structure's stands for both.
                let iconic = unit && e.army != 0 && e.half != [0.0; 3];
                let has_icon = iconic && blueprint_icons.and_then(|i| i.get(e.blueprint as usize)).is_some_and(|n| !n.is_empty());
                // Past its last LOD cutoff in every view the mesh isn't
                // drawn: the icon stands alone, and a unit without one draws
                // nothing (moho shows nothing there); props aren't
                // drawn either.
                if lod.is_none() {
                    p.what = if has_icon { What::Icon } else { What::Skip };
                    return p;
                }
                // Per view: past the mesh's last cutoff there (another view
                // is near enough to draw it), the icon at full strength;
                // zoomed out past the blueprint's `IconFadeInZoom` (130 for
                // nearly every unit; the mesh's last LOD cutoff is further,
                // 215 for most), the icon over the mesh, fading in over the
                // next 15% of zoom (the width is inferred: moho fades, the
                // wiki gives only where it begins); with Always Show
                // Strategic Icons, over the mesh at any zoom.
                if has_icon {
                    let fade = icon_zoom.and_then(|z| z.get(e.blueprint as usize)).copied();
                    for (v, c) in views.iter().enumerate() {
                        p.icons[v] = if always_icons || past & (1 << v) != 0 {
                            1.0
                        } else {
                            match fade {
                                Some(fade) if c.zoom >= fade => ((c.zoom - fade) / (fade * 0.15).max(1.0)).clamp(0.0, 1.0),
                                _ => NO_ICON,
                            }
                        };
                    }
                }
                let mesh = lod.filter(|&l| l != u32::MAX && models.ready(l).is_some()).unwrap_or(e.mesh);
                let model_ready = (e.mesh != u32::MAX).then(|| models.ready(mesh)).flatten();
                if !frustums.is_empty() {
                    let scale = e.scale[0].abs().max(e.scale[1].abs()).max(e.scale[2].abs());
                    let radius = model_ready.map_or(0.0, |m| m.radius * scale).max(Vec3::from_array(e.half).length()) + CULL_MARGIN;
                    let centre = Vec3::from_array(e.position).extend(1.0);
                    if !frustums.iter().any(|planes| planes.iter().all(|p| p.dot(centre) >= -radius)) {
                        p.what = What::Culled;
                        return p;
                    }
                }
                p.bar = shows_bar(e);
                match model_ready {
                    Some(m) => {
                        p.what = What::Instance;
                        p.mesh = mesh;
                        p.bones = if e.pose_len == 0 || m.skeleton.is_empty() { 0 } else { m.skeleton.len() as u32 };
                        p.effect = record_all_bones || effect_entities.binary_search(&e.id).is_ok();
                        p.timed = m.shading.timed();
                        // (`ShieldFill` is a depth-only pass: culling does its job.)
                        p.drawn = m.shading != crate::engine::api::mesh::Shading::ShieldFill;
                    }
                    // Not drawn until its model loads (moho shows no
                    // stand-in); mesh-less entities (FAF's script helpers:
                    // beam ends, effect anchors) are invisible. Only a unit
                    // whose model failed to load shows as a box, so missing
                    // assets stay noticeable.
                    None if e.mesh != u32::MAX => {
                        p.what = What::Box;
                        p.failed = unit && models.failed(e.mesh);
                    }
                    None => {}
                }
                p
            };
            // (One view: no per-view LOD flags.)
            let past = |i: usize| self.frame_past.get(i).copied().unwrap_or(0);
            self.plans.clear();
            if n >= PARALLEL_MIN {
                use rayon::prelude::*;
                self.plans.par_extend(self.frame_entities.par_iter().zip(self.frame_lod.par_iter()).enumerate().with_min_len(1024).map(|(i, (e, &lod))| plan_of(e, lod, past(i))));
            } else {
                self.plans.extend(self.frame_entities.iter().zip(&self.frame_lod).enumerate().map(|(i, (e, &lod))| plan_of(e, lod, past(i))));
            }
        }
        // 2. Slots (serial, in entity order): each drawn instance's place,
        // each posed entity's run of the palette; the icons, boxes and bars
        // (cheap) as they come.
        self.culled = 0;
        self.instance_entities.clear();
        self.posed.clear();
        self.bone_entities.clear();
        let mut palette_len = 0u32;
        let n_views = views.len();
        self.icons.resize_with(n_views, Vec::new);
        for (i, (p, e)) in self.plans.iter_mut().zip(&self.frame_entities).enumerate() {
            match p.what {
                What::Skip => {}
                // A blip: its generic shape, grey, at any zoom. The icon
                // standing alone (past the mesh's last LOD cutoff, or a
                // remembered scouted structure): its own icon, greyed for
                // the remembered structure, full colour otherwise.
                What::Blip => {
                    for icons in &mut self.icons {
                        icons.push((e.id, e.position, e.army, e.blueprint, IconKind::Blip, 1.0));
                    }
                }
                What::Icon => {
                    let kind = if e.sighting == crate::engine::snapshot::Sighting::Remembered { IconKind::Remembered } else { IconKind::Full };
                    for icons in &mut self.icons {
                        icons.push((e.id, e.position, e.army, e.blueprint, kind, 1.0));
                    }
                }
                What::Culled => self.culled += 1,
                What::Instance => {
                    if p.bones > 0 {
                        p.offset = palette_len;
                        palette_len += p.bones;
                        self.posed.push(i as u32);
                    }
                    if p.drawn {
                        self.instance_entities.push(i as u32);
                    }
                    if p.effect {
                        self.bone_entities.push(i as u32);
                    }
                }
                What::Box => {
                    self.boxed.push(e.id);
                    if p.failed {
                        let half = Vec3::from_array(e.half);
                        let model = Mat4::from_translation(Vec3::from_array(e.position))
                            * Mat4::from_quat(Quat::from_array(e.rotation).normalize())
                            * Mat4::from_translation(Vec3::from_array(e.center))
                            * Mat4::from_scale((half * 2.0).max(Vec3::splat(0.15)));
                        self.boxes.push(Instance { model: model.to_cols_array_2d(), color: entity_color(e, &self.army_rgb) });
                    }
                }
            }
            if p.bar {
                self.bars.push(i as u32);
            }
            for (icons, &alpha) in self.icons.iter_mut().zip(&p.icons) {
                if alpha != NO_ICON {
                    // A remembered structure's icon stays greyed.
                    let kind = if e.sighting == crate::engine::snapshot::Sighting::Remembered { IconKind::Remembered } else { IconKind::Full };
                    icons.push((e.id, e.position, e.army, e.blueprint, kind, alpha));
                }
            }
        }
        let (plans, entities, prev_pose, army_rgb, effect_ages) = (&self.plans, &self.frame_entities, &self.frame_prev_pose, &self.army_rgb, &self.effect_ages);
        let model_of = |e: &EntityView| Mat4::from_translation(Vec3::from_array(e.position)) * Mat4::from_quat(Quat::from_array(e.rotation).normalize()) * Mat4::from_scale(Vec3::from_array(e.scale));
        // 3. Instances (parallel), each into its slot.
        {
            let build = |i: u32| -> (u32, super::models::MeshInstance) {
                let (e, p) = (&entities[i as usize], &plans[i as usize]);
                let m = models.ready(p.mesh).expect("planned this frame");
                let bones = if p.bones > 0 { p.offset } else { super::models::NO_BONES };
                let model = model_of(e);
                let [r, g, b] = army_color(army_rgb, e.army);
                let (wreck, cutout, effect) = (m.wreck, m.cutout, m.effect);
                // No army (props, effects): no team colour (rgb < 0 = off).
                let [r, g, b] = if e.army == 0 { [-1.0; 3] } else { [r, g, b] };
                // Effect techniques read their mesh's age in ticks (moho's
                // `time - material.x`): from the frame the entity first
                // drew with this mesh (a new or changed entry: 0).
                let age = if p.timed {
                    match effect_ages.get(&e.id) {
                        Some(&(seen, since, _)) if seen == p.mesh => (now - since).max(0.0),
                        _ => 0.0,
                    }
                } else {
                    0.0
                };
                let instance = match m.build {
                    // Build-in techniques read the fraction complete from
                    // `team.a` (projectiles, the UEF cube: none, 0) and the
                    // technique from `params.x`.
                    Some(style) => {
                        let fraction = if e.kind == ViewKind::Projectile { 0.0 } else { e.build };
                        super::models::MeshInstance { model: model.to_cols_array_2d(), team: [r, g, b, fraction], params: [style as u8 as f32, 0.0, 0.0, 0.0], bones: [bones, 0, 0, 0], shading: [m.shading as u8 as f32, 0.0, 0.0, 0.0] }
                    }
                    // Shields: `team.a` carries the health fraction.
                    None if m.shading.is_shield() => super::models::MeshInstance {
                        model: model.to_cols_array_2d(),
                        team: [r, g, b, e.health],
                        params: [0.0; 4],
                        bones: [bones, 0, 0, 0],
                        shading: [m.shading as u8 as f32, 0.0, 0.0, 0.0],
                    },
                    None => {
                        let team = [r, g, b, if under_construction(e) { 0.45 } else { 1.0 }];
                        // Tread scroll, blended between the last two ticks
                        // (kept to one turn: U wraps).
                        let (s1, s2) = if m.scrolling {
                            let [p1, p2, c1, c2] = e.scroll;
                            ((p1 + (c1 - p1) * alpha).rem_euclid(1.0), (p2 + (c2 - p2) * alpha).rem_euclid(1.0))
                        } else {
                            (0.0, 0.0)
                        };
                        super::models::MeshInstance { model: model.to_cols_array_2d(), team, params: [wreck as u32 as f32, cutout as u32 as f32, 0.0, effect as u32 as f32], bones: [bones, m.vertex_motion as u32, 0, 0], shading: [m.shading as u8 as f32, s1, s2, age] }
                    }
                };
                (p.mesh, instance)
            };
            self.model_list.resize(self.instance_entities.len(), (0, bytemuck::Zeroable::zeroed()));
            if self.instance_entities.len() >= PARALLEL_MIN {
                use rayon::prelude::*;
                self.model_list.par_iter_mut().zip(self.instance_entities.par_iter()).with_min_len(512).for_each(|(slot, &i)| *slot = build(i));
            } else {
                for (slot, &i) in self.model_list.iter_mut().zip(&self.instance_entities) {
                    *slot = build(i);
                }
            }
        }
        // 4. Bone matrices (parallel): the posed entities in groups, each
        // group writing its own run of the palette.
        self.palette.resize(palette_len as usize, [[0.0; 4]; 4]);
        if !self.posed.is_empty() {
            let posed = &self.posed;
            let groups = posed.len().div_ceil(POSE_GROUP).max(1);
            let per_group = posed.len().div_ceil(groups).max(1);
            let mut tasks: Vec<(&[u32], &mut [[[f32; 4]; 4]])> = Vec::with_capacity(groups);
            let mut rest: &mut [[[f32; 4]; 4]] = &mut self.palette;
            let mut consumed = 0usize;
            for ids in posed.chunks(per_group) {
                let last = &plans[*ids.last().expect("non-empty chunk") as usize];
                let end = (last.offset + last.bones) as usize;
                let (mine, after) = rest.split_at_mut(end - consumed);
                tasks.push((ids, mine));
                rest = after;
                consumed = end;
            }
            let pose_group = |(ids, out): (&[u32], &mut [[[f32; 4]; 4]])| {
                let mut scratch = rts_core_engine::render_pose::Scratch::default();
                let mut matrices = Vec::new();
                let base = plans[ids[0] as usize].offset as usize;
                for &i in ids {
                    let (e, p) = (&entities[i as usize], &plans[i as usize]);
                    let Some(m) = models.ready(p.mesh) else { continue };
                    let ops = &poses[e.pose_start as usize..(e.pose_start + e.pose_len) as usize];
                    let (ps, pl) = prev_pose[i as usize];
                    let prev = &prev_poses[(ps as usize).min(prev_poses.len())..((ps + pl) as usize).min(prev_poses.len())];
                    matrices.clear();
                    rts_core_engine::render_pose::evaluate(&m.skeleton, ops.iter().map(|op| rts_core_engine::render_pose::interpolate(op, prev, alpha)), |c| models.clip_for(p.mesh, c), &mut scratch, &mut matrices);
                    let at = p.offset as usize - base;
                    let len = matrices.len().min(p.bones as usize);
                    out[at..at + len].copy_from_slice(&matrices[..len]);
                }
            };
            if posed.len() >= PARALLEL_MIN {
                use rayon::prelude::*;
                tasks.into_par_iter().for_each(pose_group);
            } else {
                tasks.into_iter().for_each(pose_group);
            }
        }
        // 5. The bones effects follow, in id order (the entities are).
        {
            let bone_of = |i: u32| {
                let (e, p) = (&entities[i as usize], &plans[i as usize]);
                (e.id, (model_of(e), if p.bones > 0 { p.offset } else { super::models::NO_BONES }, p.mesh))
            };
            if self.bone_entities.len() >= PARALLEL_MIN {
                use rayon::prelude::*;
                self.frame_bones.par_extend(self.bone_entities.par_iter().with_min_len(1024).map(|&i| bone_of(i)));
            } else {
                self.frame_bones.extend(self.bone_entities.iter().map(|&i| bone_of(i)));
            }
        }
        // Timed effect techniques: their age entries, after the reads.
        let generation = self.age_generation;
        for (p, e) in self.plans.iter().zip(&self.frame_entities) {
            if p.what == What::Instance && p.timed {
                let entry = self.effect_ages.entry(e.id).or_insert((p.mesh, now, generation));
                if entry.0 != p.mesh {
                    *entry = (p.mesh, now, generation);
                }
                entry.2 = generation;
            }
        }
        let _ = pose_span;
        self.effect_ages.retain(|_, a| a.2 == generation);
        self.models.upload_palette(device, queue, &self.palette);
    }

    /// The heavy part of a frame's prepare, off the main thread while the
    /// interface runs: entities interpolated between the two snapshots,
    /// levels of detail and loads requested, bones posed and instances
    /// built (sorted by draw order), the particles emitted and placed.
    /// Only the queue is touched (buffer writes): no interface state.
    pub fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(latest) = self.latest.clone() else { return };
        let previous = self.previous.clone();
        let alpha = self.alpha;
        // The LOD planes: moho measures detail by depth along the view, as
        // for decals and resource icons, not by distance to the eye, which
        // made meshes pop in a circle round the camera.
        let lod_planes: Vec<LodPlane> = self.views.iter().map(|c| c.lod_plane()).collect();
        let frustums: Vec<[glam::Vec4; 6]> = self.views.iter().map(|c| frustum_planes(c.view_proj())).collect();
        // Finished loads in before anything reads the store.
        self.models.upload(device, queue);
        // Interpolation beside the LOD pass: the LOD only needs a depth
        // along the view, for which the snapshot's position (a tick stale at
        // most) serves, so it reads that while the entities are blended.
        {
            let (frame_entities, frame_prev_pose, frame_lod, frame_past, models) = (&mut self.frame_entities, &mut self.frame_prev_pose, &mut self.frame_lod, &mut self.frame_past, &mut self.models);
            let (lod_planes, frustums) = (&lod_planes, &frustums);
            let record_all_bones = self.record_all_bones;
            let (latest, previous) = (&latest, previous.as_deref());
            rayon::join(
                || {
                    let _s = rts_core_engine::prof_span!("interpolate entities");
                    // Each entity finds its last-tick self by binary search
                    // (both lists are in id order) and is blended in place,
                    // so the walk splits across threads with nothing to
                    // merge.
                    let prev = previous.map(|p| p.entities.as_slice()).unwrap_or(&[]);
                    frame_entities.clear();
                    frame_entities.extend_from_slice(&latest.entities);
                    frame_prev_pose.clear();
                    frame_prev_pose.resize(latest.entities.len(), (0, 0));
                    let interpolate = |v: &mut EntityView, pp: &mut (u32, u32)| {
                        if let Ok(j) = prev.binary_search_by_key(&v.id, |p| p.id) {
                            let p = &prev[j];
                            let lerp = |a: f32, b: f32| a + (b - a) * alpha;
                            v.position = [lerp(p.position[0], v.position[0]), lerp(p.position[1], v.position[1]), lerp(p.position[2], v.position[2])];
                            v.rotation = Quat::from_array(p.rotation).slerp(Quat::from_array(v.rotation), alpha).to_array();
                            *pp = (p.pose_start, p.pose_len);
                        } else if let Ok(j) = latest.launches.binary_search_by_key(&v.id, |l| l.0) {
                            // Launched this tick: it already flew a tick, so
                            // lerp it from its muzzle.
                            let from = latest.launches[j].1;
                            let lerp = |a: f32, b: f32| a + (b - a) * alpha;
                            v.position = [lerp(from[0], v.position[0]), lerp(from[1], v.position[1]), lerp(from[2], v.position[2])];
                        }
                    };
                    if latest.entities.len() >= PARALLEL_MIN {
                        use rayon::prelude::*;
                        frame_entities.par_iter_mut().zip(frame_prev_pose.par_iter_mut()).with_min_len(1024).for_each(|(v, pp)| interpolate(v, pp));
                    } else {
                        for (v, pp) in frame_entities.iter_mut().zip(frame_prev_pose.iter_mut()) {
                            interpolate(v, pp);
                        }
                    }
                },
                || {
                    let _s = rts_core_engine::prof_span!("LOD + asset requests");
                    let poses: &[crate::engine::snapshot::Pose] = &latest.poses;
                    // Level of detail per entity by view depth, the finest
                    // any view needs (tests that inspect bones keep LOD0):
                    // `None` = beyond the last cutoff in every view. With
                    // more views, which of them each is past it in.
                    frame_lod.clear();
                    frame_past.clear();
                    let split = lod_planes.len() > 1 && !record_all_bones;
                    for e in &latest.entities {
                        let at = Vec3::from_array(e.position);
                        let lod = match e.mesh {
                            u32::MAX => Some(u32::MAX),
                            m if record_all_bones || lod_planes.is_empty() => Some(m),
                            m if !split => models.lod(m, lod_planes[0].metric(at)),
                            // The nearest of the views that can see it (a view
                            // close to the ground elsewhere would otherwise give
                            // every unit of the map its mesh in a zoomed-out one).
                            m => {
                                let scale = e.scale[0].abs().max(e.scale[1].abs()).max(e.scale[2].abs());
                                let radius = models.ready(m).map_or(0.0, |r| r.radius * scale).max(Vec3::from_array(e.half).length()) + CULL_MARGIN;
                                let seen = |v: usize| frustums[v].iter().all(|p| p.dot(at.extend(1.0)) >= -radius);
                                let nearest = |all: bool| lod_planes.iter().enumerate().filter(|&(v, _)| all || seen(v)).map(|(_, p)| p.metric(at)).fold(f32::INFINITY, f32::min);
                                let metric = match nearest(false) {
                                    f32::INFINITY => nearest(true),
                                    d => d,
                                };
                                models.lod(m, metric)
                            }
                        };
                        frame_lod.push(lod);
                        if split {
                            let past = lod_planes.iter().take(MAX_VIEWS).enumerate().filter(|(_, p)| e.mesh != u32::MAX && models.lod(e.mesh, p.metric(at)).is_none()).fold(0u8, |bits, (v, _)| bits | 1 << v);
                            frame_past.push(past);
                        }
                    }
                    // Units and structures queue their loads ahead of props:
                    // the loader takes requests in order, and at game start
                    // a map's hundreds of trees and rocks would otherwise
                    // hold back the commanders.
                    for e in latest.entities.iter().filter(|e| e.kind != crate::engine::snapshot::ViewKind::Prop && e.mesh != u32::MAX) {
                        let _ = models.get(e.mesh);
                    }
                    for (e, lod) in latest.entities.iter().zip(frame_lod.iter()) {
                        let lod_mesh = lod.filter(|&l| l != u32::MAX && l != e.mesh);
                        if let Some(l) = lod_mesh {
                            // Load the level it should be drawn at (LOD0 stands in meanwhile).
                            let _ = models.get(l);
                        }
                        if e.mesh == u32::MAX || models.get(e.mesh).is_none() {
                            continue;
                        }
                        for p in &poses[e.pose_start as usize..(e.pose_start + e.pose_len) as usize] {
                            if let crate::engine::snapshot::PoseOp::Clip { clip, .. } = p.op {
                                models.request_clip(clip);
                                models.bind(e.mesh, clip);
                                if let Some(l) = lod_mesh.filter(|&l| models.ready(l).is_some()) {
                                    models.bind(l, clip);
                                }
                            }
                        }
                    }
                },
            );
        }
        // The pose passes, and beside them the particle step's bookkeeping
        // (its state merge and asset resolution need no bones).
        {
            let mut particles = self.particles.take();
            let (latest, previous) = (&latest, previous.as_deref());
            rayon::join(
                || self.pose_passes(device, queue, latest, previous.map_or(&[], |p| p.poses.as_slice()), alpha),
                || particles.begin(latest.sim_tick, &latest.effects, previous.map_or(&[], |p| p.effects.as_slice()), previous.map_or(&[], |p| p.effect_curves.as_slice())),
            );
            self.particles.put(particles);
        }
        // Opaque models, then units under construction (blended), then
        // effects, then order feedback (drawn over everything); by mesh
        // within each.
        let models = &self.models;
        self.model_list.sort_by_key(|(m, _)| (models.ready(*m).map_or(0, draw_order), *m));
        if let Some(c) = self.views.first() {
            let prev: &[crate::engine::snapshot::EffectView] = self.previous.as_ref().map_or(&[], |p| p.effects.as_slice());
            let now = latest.sim_tick as f32 - 1.0 + alpha;
            // Attached effects sit on the posed, interpolated bone the model
            // is drawn with (the sim only knows rest poses).
            let (bones, palette, entities) = (&self.frame_bones, &self.palette, &self.frame_entities);
            // The skeletons of the meshes drawn this frame, looked up once
            // (the model store isn't shareable across the threads placing
            // effects).
            let mut skeletons: std::collections::HashMap<u32, &rts_core_engine::render_pose::Skeleton> = std::collections::HashMap::new();
            for &(_, (_, _, mesh)) in bones.iter() {
                if let std::collections::hash_map::Entry::Vacant(v) = skeletons.entry(mesh)
                    && let Some(m) = self.models.ready(mesh) {
                    v.insert(&m.skeleton);
                }
            }
            let resolve = |entity: u32, bone: u32| -> Option<(Vec3, Quat)> {
                let base = || {
                    let e = &entities[entities.binary_search_by_key(&entity, |e| e.id).ok()?];
                    Some(Mat4::from_scale_rotation_translation(Vec3::from_array(e.scale), Quat::from_array(e.rotation).normalize(), Vec3::from_array(e.position)))
                };
                let found = bones.binary_search_by_key(&entity, |b| b.0).ok().map(|i| bones[i].1);
                let m = match found {
                    Some((model, base_index, mesh)) if bone != u32::MAX => {
                        let sk = *skeletons.get(&mesh)?;
                        let skin = (base_index != super::models::NO_BONES).then(|| palette.get(base_index as usize + bone as usize)).flatten();
                        model * sk.bone(bone as usize, skin)?
                    }
                    Some((model, _, _)) => model,
                    // No model to pose (collision beams, script entities): a
                    // bone is the sim's anchor (a beam's bone 1 is its far end).
                    None if bone != u32::MAX => return None,
                    None => base()?,
                };
                let (_, r, t) = m.to_scale_rotation_translation();
                Some((t, r))
            };
            // Waterline snapping and over-water emission read the map.
            let terrain = self.terrain_view.as_ref();
            let water = terrain.and_then(|t| t.water);
            let ground = |x: f32, z: f32| -> f32 {
                let Some(t) = terrain else { return 0.0 };
                let xi = (x.max(0.0) as u32).min(t.width);
                let zi = (z.max(0.0) as u32).min(t.height);
                t.heights[(zi * (t.width + 1) + xi) as usize]
            };
            // Emitters in any view emit; the first view's camera faces the
            // billboards until each view sets its own (`Scene::prepare_view`).
            let sight: Vec<([glam::Vec4; 6], LodPlane)> = self.views.iter().map(|v| (frustum_planes(v.view_proj()), v.lod_plane())).collect();
            self.particles.finish(device, queue, now, &latest.effects, prev, &latest.effect_curves, alpha, &sight, &resolve, &latest.launches, water, &ground, c.zoom);
        }
    }
}

impl Scene {
    /// The VFS the scene loads from, once a snapshot brought it.
    pub fn vfs(&self) -> Option<&Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>> {
        self.vfs.as_ref()
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, w: u32, h: u32) -> Self {
        // The scene renders into the gamma-space frame; the bloom writes
        // the surface (`format`).
        let surface_format = format;
        let format = super::globals::FRAME_FORMAT;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("scene: shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let vision_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene: globals layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                // Sun shadows: cascades, comparison sampler, terrain horizon.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                // Environment cube, BRDF lookup, their sampler.
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                // The terrain's heights (decal and tread grids).
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                // FA's insect lookup (Cybran units at Medium Fidelity).
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                // Fog of war's vision circles: circles, grid cells, lists.
                vision_entry(12),
                vision_entry(13),
                vision_entry(14),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("scene: overlay sampler"), ..Default::default() });
        let (overlay_view, _) = overlay_texture(device, "scene: grid overlay (none)", 1, 1);
        let (fog_view, _) = overlay_texture(device, "scene: fog of war (none)", 1, 1);
        let lit = super::models::solid(device, queue, "scene: lit (no shadows)", [255; 4], wgpu::TextureFormat::R8Unorm);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("scene: pipeline layout"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let depth_stencil = |write: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let pipeline = |label: &str, vs: &str, fs: &str, buffers: &[wgpu::VertexBufferLayout], blend: Option<wgpu::BlendState>, depth_write: bool, cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &super::globals::scene_targets(format, blend, super::globals::GlowWrite::Keep),
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
                depth_stencil: Some(depth_stencil(depth_write)),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pos_normal = wgpu::VertexBufferLayout {
            array_stride: 24,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4],
        };
        let terrain_pipeline = pipeline("scene: terrain (untextured)", "vs_terrain", "fs_terrain", std::slice::from_ref(&pos_normal), None, true, None);
        let box_pipeline = pipeline("scene: boxes", "vs_box", "fs_box", &[pos_normal, instance_layout], Some(wgpu::BlendState::ALPHA_BLENDING), true, Some(wgpu::Face::Back));
        let water_layout = wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3] };
        let water_pipeline = pipeline("scene: water (flat)", "vs_water", "fs_water", &[water_layout], Some(wgpu::BlendState::ALPHA_BLENDING), false, None);
        let instance_capacity = 1024;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene: box instances"),
            size: (instance_capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let models = super::models::Models::new(device, queue, &layout, format, DEPTH_FORMAT);
        let shadows = super::shadows::Shadows::new(device, &models.layout, &models.palette_layout, &super::models::VERTEX_LAYOUTS);
        let env = EnvBindings::new(device, queue);
        let heights = device.create_buffer(&wgpu::BufferDescriptor { label: Some("terrain: heights"), size: 4, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
        let map = super::terrain::TerrainRenderer::new(device, queue, &layout, format, DEPTH_FORMAT, w, h);
        let mut particles = super::particles::Particles::new(device, queue, &layout, format, DEPTH_FORMAT);
        let command_graph = super::command_graph::CommandGraph::new(device, queue, &layout, format, DEPTH_FORMAT);
        let debug_lines = super::debug_lines::DebugLines::new(device, &layout, format, DEPTH_FORMAT);
        let resource_icons = super::resource_icons::ResourceIcons::new(device, &layout, format, DEPTH_FORMAT);
        let bloom = super::bloom::Bloom::new(device, surface_format, w, h);
        let smaa = super::smaa::Smaa::new(device, queue, surface_format, w, h);
        particles.set_background(device, &bloom.background);
        let model_capacity = 1024;
        let model_instances = instance_buffer(device, "models: instances", model_capacity * std::mem::size_of::<super::models::MeshInstance>());
        let depth = depth_target(device, w, h);
        Self {
            views_gpu: Vec::new(),
            shadows,
            overlay_view,
            fog_view,
            fog_texture: None,
            fog_extent: None,
            pending_fog: Vec::new(),
            vision: super::vision::VisionCircles::new(device),
            lit,
            horizon_bound: false,
            height_range: (0.0, 0.0),
            map,
            command_graph,
            debug_lines,
            resource_icons,
            screen: [w as f32, h as f32],
            started: Instant::now(),
            frozen_time: None,
            stage: Lent::new(Stage::new(models, particles)),
            vfs: None,
            model_instances,
            model_capacity,
            model_draws: Vec::new(),
            pending_splats: Vec::new(),
            frame_game_seconds: 0.0,
            ghost_draw: None,
            queued_ghost_draws: Vec::new(),
            template_ghost_draws: Vec::new(),
            fixed_alpha: None,
            latest_prepared: false,
            record_all_bones: false,
            bind_generation: 0,
            layout,
            sampler,
            env,
            overlay: None,
            terrain_pipeline,
            box_pipeline,
            water_pipeline,
            terrain: None,
            heights,
            water: None,
            cube: cube(device),
            instances,
            instance_capacity,
            instance_count: 0,
            depth: depth.1,
            depth_texture: depth.0,
            ranges: super::ranges::Ranges::new(device, DEPTH_FORMAT, w, h),
            bloom,
            smaa,
            latest: None,
            previous: None,
            arrived: Instant::now(),
            map_size: None,
            icons: Vec::new(),
            icon_cameras: Vec::new(),
            frame_entities: Vec::new(),
            bars: Vec::new(),
            arrivals: Default::default(),
            terrain_view: None,
            terrain_patch: None,
            terrain_next: None,
            input: Default::default(),
            applied: None,
            user_decals: None,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        (self.depth_texture, self.depth) = depth_target(device, w, h);
        self.ranges.resize(device, DEPTH_FORMAT, w, h);
        self.screen = [w as f32, h as f32];
        self.bloom.resize(device, w, h);
        self.smaa.resize(device, w, h);
        self.stage.particles.set_background(device, &self.bloom.background);
        self.map.resize(device, w, h);
    }

    /// The main thread's decisions for the next frame (cameras, views, the
    /// selection, ghosts, overlays, settings); what changed among the
    /// settings that need the device (shadow map size, texture detail) and
    /// the interface's decals is applied now.
    pub fn set_input(&mut self, device: &wgpu::Device, input: super::world::FrameInput) {
        let s = input.settings;
        if self.applied.is_none_or(|a| a.shadow_size != s.shadow_size) {
            self.set_shadow_size(device, s.shadow_size);
        }
        if self.applied.is_some_and(|a| a.mip_skip != s.mip_skip) || (self.applied.is_none() && s.mip_skip != 0) {
            self.set_mip_skip(device, s.mip_skip);
        }
        (self.bloom.enabled, self.bloom.blur_scale) = (s.bloom, s.bloom_blur);
        self.applied = Some(s);
        if let Some(d) = input.user_decals.as_ref().filter(|d| !self.user_decals.as_ref().is_some_and(|u| Arc::ptr_eq(u, d))) {
            self.map.set_user_decals(d.clone());
            self.user_decals = Some(d.clone());
        }
        self.input = input;
    }

    /// What one snapshot brings (`World::receive`), in order: new meshes,
    /// clips and effect assets, a terrain change, fog, splats, the sim's
    /// decals, and the snapshot itself, which becomes the latest.
    pub fn apply(&mut self, device: &wgpu::Device, delta: super::world::SceneDelta) {
        {
            let _s = rts_core_engine::prof_span!("snapshot: new meshes");
            self.stage.models.receive(delta.vfs, delta.clips, delta.meshes);
            self.vfs = self.stage.models.vfs().cloned();
        }
        {
            let _s = rts_core_engine::prof_span!("snapshot: new effect assets");
            let mut assets = delta.effect_assets;
            self.stage.particles.receive(&mut assets, self.vfs.as_ref());
        }
        self.command_graph.set_vfs(self.stage.models.vfs());
        self.resource_icons.set_vfs(self.stage.models.vfs());
        if let Some(t) = delta.terrain {
            let _s = rts_core_engine::prof_span!("snapshot: terrain");
            if self.patchable(&t) {
                // Applied when worked out (`poll_terrain`).
                self.terrain_next = Some(t);
            } else {
                (self.terrain_patch, self.terrain_next) = (None, None);
                self.upload_terrain(device, &t);
                self.map.receive(&t, self.stage.models.vfs());
                self.map_size = Some((t.width as f32, t.height as f32));
                self.terrain_view = Some(t);
            }
        }
        if let Some(f) = delta.fog {
            self.pending_fog.push(f);
        }
        self.pending_splats.extend(delta.splats);
        if let Some(d) = delta.decals {
            self.map.set_script_decals(d);
        }
        let snap = delta.snapshot;
        // A snapshot never prepared (more than one taken this frame): the
        // effects only it had (born and gone within it, such as a one-tick
        // impact flash) would never emit, so the particle system emits them
        // backdated.
        if !self.latest_prepared
            && let Some(old) = &self.latest {
            let mut i = 0;
            let mut only = Vec::new();
            for e in &old.effects {
                while i < snap.effects.len() && snap.effects[i].id < e.id {
                    i += 1;
                }
                if i >= snap.effects.len() || snap.effects[i].id != e.id {
                    only.push(*e);
                }
            }
            if !only.is_empty() {
                tracing::debug!(tick = old.tick, effects = only.len(), "snapshot never drawn: its effects carried");
                self.stage.particles.carry_missed(&only, &old.effect_curves);
            }
        }
        self.latest_prepared = false;
        self.arrivals.push_back((delta.arrived, snap.tick));
        while self.arrivals.len() > 2 && delta.arrived.duration_since(self.arrivals[0].0).as_secs_f32() > 1.0 {
            self.arrivals.pop_front();
        }
        // Replacing drops the snapshot before the previous one: its entity,
        // effect and projectile vectors.
        let _s = rts_core_engine::prof_span!("snapshot: replace (drop old)");
        self.previous = self.latest.replace(snap);
        let _ = _s;
        self.arrived = delta.arrived;
    }

    /// The input the frame is being drawn with.
    pub fn input(&self) -> &super::world::FrameInput {
        &self.input
    }

    /// What this frame worked out that the main thread reads.
    /// The sim's meshes' bounds loaded since the last call (none while the
    /// stage is out with the prepare worker: they wait for the next call).
    pub fn take_new_mesh_bounds(&mut self) -> Vec<(u32, [f32; 6])> {
        match self.stage.0.as_mut() {
            Some(stage) => stage.models.take_new_bounds(),
            None => Vec::new(),
        }
    }

    pub fn results(&self) -> super::world::FrameResults {
        let _s = rts_core_engine::prof_span!("frame results");
        super::world::FrameResults {
            entities: Arc::new(self.frame_entities.clone()),
            model_draws: self.stage.models.draws.get() as usize,
            particle_draws: self.stage.particles.draws.get() as usize,
            particles: self.stage.particles.live(),
        }
    }

    /// View `index`'s order lines, waypoints and brackets this frame.
    pub fn graph(&self, index: usize) -> Option<&super::command_graph::GraphFrame> {
        Some(&self.views_gpu.get(index)?.as_ref()?.graph)
    }

    /// View `index`'s deposit icons this frame (tests).
    #[allow(dead_code)]
    pub fn deposit_icons(&self, index: usize) -> usize {
        self.views_gpu.get(index).and_then(Option::as_ref).map_or(0, |v| v.icons.count())
    }

    /// Starts a frame's prepare: the fog and overlay updates, the
    /// interpolation phase, splats, then the stage for the worker
    /// (`Stage::run`), loaded with everything it reads: the two snapshots,
    /// the camera's view, the map. `None` before the first snapshot.
    pub fn prepare_begin(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Stage> {
        self.poll_terrain(device, queue);
        self.update_overlay(device, queue);
        self.update_fog(device, queue);
        let Some(latest) = &self.latest else { return None };
        self.latest_prepared = true;
        // Blend over the measured snapshot interval (shorter at higher game
        // speeds; 100 ms at 1x).
        let interval = match (self.arrivals.front(), self.arrivals.back()) {
            (Some(a), Some(b)) if b.1 > a.1 => b.0.duration_since(a.0).as_secs_f32() / (b.1 - a.1) as f32,
            _ => TICK_SECONDS,
        };
        let alpha = self.fixed_alpha.unwrap_or_else(|| (self.arrived.elapsed().as_secs_f32() / interval.max(1e-3)).clamp(0.0, 1.0));
        // Game seconds the frame shows: splats are born and fade by it.
        self.frame_game_seconds = (latest.sim_tick as f32 - 1.0 + alpha) * TICK_SECONDS;
        if !self.pending_splats.is_empty() || self.map.splats_waiting() {
            let splats = std::mem::take(&mut self.pending_splats);
            self.map.lay_splats(device, queue, &splats, self.frame_game_seconds);
        }
        {
            let stage = &mut *self.stage;
            stage.latest = self.latest.clone();
            stage.previous = self.previous.clone();
            stage.alpha = alpha;
            // Last frame's views (the interface lays this frame's out while
            // the stage runs).
            stage.views.clear();
            stage.icon_cameras.clear();
            for v in &self.input.views {
                if let Some(c) = self.input.cameras.get(&v.camera) {
                    stage.views.push(CamView::of(c));
                    stage.icon_cameras.push(v.camera.clone());
                }
            }
            stage.blueprint_icons = self.input.blueprint_icons.clone();
            stage.blueprint_icon_zoom = self.input.blueprint_icon_zoom.clone();
            stage.army_rgb.clone_from(&*self.input.army_rgb);
            stage.record_all_bones = self.record_all_bones;
            stage.always_icons = self.input.settings.always_icons;
            stage.terrain_view = self.terrain_view.clone();
        }
        Some(self.stage.take())
    }

    /// The whole prepare in one go (tests, headless): begin, run, finish.
    #[allow(dead_code)]
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(mut stage) = self.prepare_begin(device, queue) {
            stage.run(device, queue);
            self.prepare_finish(stage, device, queue);
        }
    }

    /// The worker's stage back, last frame's entity and icon lists
    /// replaced by this frame's, then what needs the interface's output
    /// (its meshes, the placement ghosts) and the cheap passes: instance
    /// uploads, map, command graph, overlays, shadows, globals.
    pub fn prepare_finish(&mut self, stage: Stage, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.stage.put(stage);
        std::mem::swap(&mut self.frame_entities, &mut self.stage.frame_entities);
        std::mem::swap(&mut self.icons, &mut self.stage.icons);
        std::mem::swap(&mut self.icon_cameras, &mut self.stage.icon_cameras);
        std::mem::swap(&mut self.bars, &mut self.stage.bars);
        // The interface's meshes (order feedback, rally flags), by path.
        for m in &self.input.interface_meshes {
            use crate::engine::api::mesh::{technique_name, Shading};
            let shading = Shading::from_shader(technique_name(&m.shader));
            let mesh = self.stage.models.interface_mesh(&m.mesh, &m.texture, shading);
            if self.stage.models.get(mesh).is_none() {
                continue;
            }
            let model = Mat4::from_scale_rotation_translation(Vec3::from_array(m.scale), Quat::from_array(m.orientation).normalize(), Vec3::from_array(m.position));
            self.stage.model_list.push((
                mesh,
                super::models::MeshInstance {
                    model: model.to_cols_array_2d(),
                    team: [-1.0, -1.0, -1.0, 1.0],
                    params: [0.0, 0.0, m.lifetime, 0.0],
                    bones: [super::models::NO_BONES, 0, 0, 0],
                    shading: [shading as u8 as f32, 0.0, 0.0, m.age],
                },
            ));
        }
        self.model_draws.clear();
        for (i, (m, _)) in self.stage.model_list.iter().enumerate() {
            match self.model_draws.last_mut() {
                Some((last, r)) if last == m => r.end = i as u32 + 1,
                _ => self.model_draws.push((*m, i as u32..i as u32 + 1)),
            }
        }
        // Queued builds' ghosts: each selected unit's build order whose site
        // has no structure on it yet (the base game shows them in green).
        self.queued_ghost_draws.clear();
        let mut queued: Vec<(String, Vec3)> = Vec::new();
        if let Some(latest) = &self.latest {
            // (A site being dragged shows as the placement ghost; let go,
            // it's where it went.)
            let dragged = self.input.command_drag;
            for (_, marks) in &latest.debug.orders {
                for m in marks {
                    let Some(bp) = &m.blueprint else { continue };
                    let at = match dragged {
                        Some((c, _, true)) if c == m.command => continue,
                        Some((c, node, false)) if c == m.command => Vec3::from_array(node),
                        _ => Vec3::from_array(m.node),
                    };
                    let built = self.frame_entities.iter().any(|e| e.kind == ViewKind::Structure && (e.position[0] - at.x).abs() < 0.5 && (e.position[2] - at.z).abs() < 0.5);
                    if !built && !queued.iter().any(|(b, p)| b == bp && p.distance_squared(at) < 0.01) {
                        queued.push((bp.clone(), at));
                    }
                }
            }
        }
        for (bp, at) in queued {
            let Some(index) = self.input.blueprints.as_ref().and_then(|b| b.iter().position(|b| *b == bp)) else { continue };
            let Some((mesh, scale)) = self.input.blueprint_meshes.as_ref().and_then(|m| m.get(index).copied()).filter(|&(m, _)| m != u32::MAX) else { continue };
            if self.stage.models.get(mesh).is_none() {
                continue;
            }
            let position = Vec3::new(at.x, self.height_at(at.x, at.z), at.z);
            let model = Mat4::from_translation(position) * Mat4::from_scale(Vec3::splat(scale));
            let c = QUEUED_GHOST;
            self.queued_ghost_draws.push((mesh, self.stage.model_list.len() as u32));
            self.stage.model_list.push((mesh, super::models::MeshInstance { model: model.to_cols_array_2d(), team: [c[0], c[1], c[2], 1.0], params: [0.0; 4], bones: [super::models::NO_BONES, 0, 0, 0], shading: [0.0; 4] }));
        }
        // The placement ghost last, in its own pass (with a template's
        // other structures).
        self.template_ghost_draws.clear();
        for g in &self.input.template_ghosts {
            let Some((mesh, scale)) = self.input.blueprint_meshes.as_ref().and_then(|m| m.get(g.blueprint as usize).copied()).filter(|&(m, _)| m != u32::MAX) else { continue };
            if self.stage.models.get(mesh).is_none() {
                continue;
            }
            let model = Mat4::from_translation(g.position) * Mat4::from_scale(Vec3::splat(scale));
            self.template_ghost_draws.push((mesh, self.stage.model_list.len() as u32));
            self.stage.model_list.push((mesh, super::models::MeshInstance { model: model.to_cols_array_2d(), team: [g.color[0], g.color[1], g.color[2], 1.0], params: [0.0; 4], bones: [super::models::NO_BONES, 0, 0, 0], shading: [0.0; 4] }));
        }
        self.ghost_draw = None;
        if let Some(g) = &self.input.ghost {
            let mesh = self.input.blueprint_meshes.as_ref().and_then(|m| m.get(g.blueprint as usize).copied()).filter(|&(m, _)| m != u32::MAX);
            if let Some((mesh, scale)) = mesh
                && self.stage.models.get(mesh).is_some() {
                let model = Mat4::from_translation(g.position) * Mat4::from_scale(Vec3::splat(scale));
                self.ghost_draw = Some((mesh, self.stage.model_list.len() as u32));
                self.stage.model_list.push((mesh, super::models::MeshInstance { model: model.to_cols_array_2d(), team: [g.color[0], g.color[1], g.color[2], 1.0], params: [0.0; 4], bones: [super::models::NO_BONES, 0, 0, 0], shading: [0.0; 4] }));
            }
        }
        let upload_span = rts_core_engine::prof_span!("upload instances");
        if self.stage.model_list.len() > self.model_capacity {
            self.model_capacity = self.stage.model_list.len().next_power_of_two();
            self.model_instances = instance_buffer(device, "models: instances", self.model_capacity * std::mem::size_of::<super::models::MeshInstance>());
        }
        let model_data: Vec<super::models::MeshInstance> = self.stage.model_list.iter().map(|(_, i)| *i).collect();
        queue.write_buffer(&self.model_instances, 0, bytemuck::cast_slice(&model_data));
        let instances: Vec<Instance> = std::mem::take(&mut self.stage.boxes);
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instances = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene: box instances"),
                size: (self.instance_capacity * std::mem::size_of::<Instance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        self.instance_count = instances.len() as u32;
        let _ = upload_span;
        {
            let _s = rts_core_engine::prof_span!("map prepare");
            self.map.prepare(device, queue);
            // Shoreline waves lay particles (handed over once).
            if let Some(waves) = self.map.wave_generators() {
                self.stage.particles.set_waves(waves.clone(), self.terrain_view.as_ref().and_then(|t| t.water));
            }
        }
        if self.map.horizon().is_some() && !self.horizon_bound {
            // The map's lighting textures come with it.
            self.env.cube = self.map.environment().map(|(v, _)| v.clone());
            self.env.brdf = self.map.brdf().cloned();
            self.env.insect = self.map.insect().cloned();
            self.rebind();
            self.horizon_bound = true;
        }
        self.bloom.set_map_bloom(self.map.lighting().map_or(0.0, |l| l.bloom));
        // A HiDPI window's scale (2 on Retina) remakes the frame's
        // buffers: the refracting particles then read the new background,
        // not the old one, which nothing draws into any more (black).
        if self.bloom.set_scale(device, self.input.ui_scale) {
            self.stage.particles.set_background(device, &self.bloom.background);
        }
        self.bloom.prepare(queue);
        self.smaa.prepare(queue);
        self.prepare_vision(device, queue);
        for i in 0..self.input.views.len() {
            self.prepare_view(device, queue, i);
        }
        self.views_gpu.truncate(self.input.views.len());
    }

    /// This frame's vision circles while fog of war is on: the latest
    /// snapshot's, blended from the one before as the units are, those that
    /// can reach a view (rebinding group 0 when their buffers grow).
    fn prepare_vision(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let _s = rts_core_engine::prof_span!("vision circles");
        let planes: Vec<[glam::Vec4; 6]> = self.input.views.iter().filter_map(|v| self.input.cameras.get(&v.camera)).map(|c| frustum_planes(c.view_proj())).collect();
        match self.latest.as_deref().filter(|_| self.fog_extent.is_some()) {
            Some(latest) => {
                let previous = self.previous.as_deref().map_or(&[][..], |p| p.vision.as_slice());
                let keep = |c: [f32; 3]| planes.iter().any(|p| super::vision::circle_in_view(p, c));
                crate::engine::snapshot::blend_vision(&latest.vision, previous, self.stage.alpha, keep, &mut self.vision.circles);
            }
            None => self.vision.circles.clear(),
        }
        if self.vision.prepare(device, queue) {
            self.rebind();
        }
    }

    /// Prepares world view `index` for drawing: all its camera decides (the
    /// terrain's detail and tiles, the decals in view, order lines, deposit
    /// icons, range overlays, sun shadow cascades and their casters, the
    /// shaders' globals), into its own [`ViewGpu`].
    fn prepare_view(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, index: usize) {
        let _s = rts_core_engine::prof_span!("prepare view");
        let Some(shown) = self.input.views.get(index).cloned() else { return };
        let Some(c) = self.input.cameras.get(&shown.camera).copied() else { return };
        let size = shown.size();
        let c = &c;
        if self.views_gpu.len() <= index {
            self.views_gpu.resize_with(index + 1, || None);
        }
        let mut v = self.views_gpu[index].take().unwrap_or_else(|| self.view_gpu(device));
        if v.bound != self.bind_generation {
            v.bind_group = self.view_bind_group(device, &v.globals);
            v.bound = self.bind_generation;
        }
        if let Some(mesh) = &self.terrain {
            // The level of detail: the coarsest step whose cells are at most
            // `TERRAIN_LOD_PIXELS` across at the camera's target (moho's
            // terrain picks tiles' detail by screen error, `ren_ViewError`).
            let pixels_per_unit = size[1] / (2.0 * c.eye_distance().max(1.0) * c.half_tangents().1);
            v.terrain_lod = mesh.lods.iter().rposition(|l| l.step as f32 * pixels_per_unit <= TERRAIN_LOD_PIXELS).unwrap_or(0);
            v.terrain_step = mesh.lods[v.terrain_lod].step;
            // The tiles in view, adjacent runs merged into one draw.
            let planes = frustum_planes(c.view_proj());
            let lod = &mesh.lods[v.terrain_lod];
            v.terrain_draws.clear();
            for (r, &(centre, radius)) in lod.tiles.iter().zip(&mesh.tile_bounds) {
                if r.is_empty() || !planes.iter().all(|p| p.dot(centre.extend(1.0)) >= -radius) {
                    continue;
                }
                match v.terrain_draws.last_mut() {
                    Some(last) if last.end == r.start => last.end = r.end,
                    _ => v.terrain_draws.push(r.clone()),
                }
            }
        }
        self.map.cull_decals(&frustum_planes(c.view_proj()), c.lod_plane(), v.terrain_step, &mut v.decals);
        {
            let _s = rts_core_engine::prof_span!("command graph");
            // In the view's own pixels.
            let screen = size;
            let terrain = self.terrain_view.as_ref();
            // Bilinear between height samples, so splats follow the ground;
            // on the water's surface over water (orders to ships go there).
            let water = terrain.and_then(|t| t.water);
            let ground = |x: f32, z: f32| {
                let Some(t) = terrain else { return 0.0 };
                let (x, z) = (x.clamp(0.0, t.width as f32), z.clamp(0.0, t.height as f32));
                let (x0, z0) = ((x as u32).min(t.width.saturating_sub(1)), (z as u32).min(t.height.saturating_sub(1)));
                let (fx, fz) = (x - x0 as f32, z - z0 as f32);
                let h = |xi: u32, zi: u32| t.heights[(zi.min(t.height) * (t.width + 1) + xi.min(t.width)) as usize];
                let top = h(x0, z0) + (h(x0 + 1, z0) - h(x0, z0)) * fx;
                let bottom = h(x0, z0 + 1) + (h(x0 + 1, z0 + 1) - h(x0, z0 + 1)) * fx;
                let h = top + (bottom - top) * fz;
                water.map_or(h, |w| h.max(w))
            };
            let project = |p: Vec3| c.project(p, screen);
            let view = super::command_graph::View {
                eye: c.eye(),
                px_per_distance: 2.0 * c.half_tangents().1 / screen[1].max(1.0),
                screen,
                ground: &ground,
                project: &project,
                cursor: self.input.cursor.filter(|&[x, y]| shown.contains(x, y)).map(|[x, y]| shown.local(x, y)),
                time: self.time(),
            };
            let orders = self.latest.as_ref().map(|l| l.debug.orders.as_slice()).unwrap_or(&[]);
            // A waypoint being dragged draws where it's going.
            let dragged: Option<Vec<(u32, Vec<rts_core_engine::engine::snapshot::OrderMarker>)>> = self.input.command_drag.map(|(command, node, _)| {
                orders.iter().map(|(u, marks)| (*u, marks.iter().map(|m| if m.command == command { super::player::moved_marker(m, node) } else { m.clone() }).collect())).collect()
            });
            let orders = dragged.as_deref().unwrap_or(orders);
            // Selection brackets: each selected unit's selection box, round
            // its mesh's box (moho, 0x103c7726: the mesh box's centre across,
            // the unit's height; the collision box until the mesh loads).
            let boxes = self.input.blueprint_selection.as_deref();
            let models = &self.stage.models;
            let brackets: Vec<super::command_graph::Bracket> = self
                .input
                .selection
                .iter()
                .filter_map(|id| {
                    let e = &self.frame_entities[self.frame_entities.binary_search_by_key(id, |e| e.id).ok()?];
                    let [sx, sz, thickness, ox, oy, oz] = boxes.and_then(|b| b.get(e.blueprint as usize).copied()).unwrap_or([0.0; 6]);
                    let mesh = super::selection_box::unit_box(e.position, e.rotation, e.scale, e.center, e.half, models.bounds(e.mesh));
                    Some(super::command_graph::Bracket {
                        position: Vec3::new(mesh.centre.x, e.position[1], mesh.centre.z),
                        rotation: Quat::from_array(e.rotation).normalize(),
                        half: super::command_graph::bracket_half([sx, sz], [mesh.half.x, mesh.half.z]),
                        thickness,
                        offset: [ox, oy, oz],
                    })
                })
                .collect();
            // Every placement ghost's skirt (a drag's or a template's too).
            let ghost_sites: Vec<[f32; 4]> = self.input.ghost.iter().chain(&self.input.template_ghosts).filter_map(|g| g.site).collect();
            let nearby = self.nearby_skirts();
            self.command_graph.update(device, queue, orders, &brackets, &ghost_sites, &nearby, &self.frame_entities, &view, &mut v.graph);
            // Deposits stand on the ground (moho's `GetElevation`), under
            // water too.
            let height = |x: f32, z: f32| {
                let Some(t) = terrain else { return 0.0 };
                let (x, z) = (x.clamp(0.0, t.width as f32), z.clamp(0.0, t.height as f32));
                t.heights[((z as u32).min(t.height) * (t.width + 1) + (x as u32).min(t.width)) as usize]
            };
            let deposits = self.input.deposits.as_deref().map_or(&[][..], |d| d.as_slice());
            self.resource_icons.update(device, queue, deposits, c.lod_plane(), &height, &project, screen, self.input.ui_scale, &mut v.icons);
        }
        {
            let _s = rts_core_engine::prof_span!("range overlays");
            let groups = self.range_groups(c);
            self.ranges.prepare(device, queue, c.view_proj(), c.eye().to_array(), self.water.as_ref().map(|w| w.1), shown.rect, &groups, &mut v.ranges);
        }
        {
            let water = self.water.as_ref().map_or(-1e9, |w| w.1);
            // Units are lit from the map's sun once it is known.
            let light = self.map.sun_direction().map_or(Vec3::new(-0.4, -1.0, -0.3), |s| -Vec3::from_array(s)).normalize();
            // Sun shadow cascades around what the camera sees.
            let cascades = super::shadows::fit(c, -light, self.height_range, self.shadows.size, self.input.settings.shadow_lod / c.lod_scale.max(1e-3), self.shadow_cascades());
            v.lights.write(queue, &cascades, self.frame_game_seconds / TICK_SECONDS);
            v.cascades = Some(cascades);
            // Casters per cascade: only those in its column along the sun.
            let _s = rts_core_engine::prof_span!("cull shadow casters");
            let mut list: Vec<super::models::MeshInstance> = Vec::new();
            let on = self.shadows_on();
            for (c, draws) in v.shadow_draws.iter_mut().enumerate() {
                draws.clear();
                if !on || c >= cascades.count {
                    continue;
                }
                for (mesh, inst) in &self.stage.model_list {
                    let Some(m) = self.stage.models.ready(*mesh) else { continue };
                    // Shield bubbles are see-through: no shadow (moho's
                    // shield techniques have no shadow pass).
                    use crate::engine::api::mesh::Shading;
                    // (Nor does the border round the map.)
                    if m.effect || matches!(m.shading, Shading::NormalMappedGlow | Shading::MapImager) || m.shading.effect_only() || m.shading.is_shield() {
                        continue;
                    }
                    let model = Mat4::from_cols_array_2d(&inst.model);
                    let pos = model.w_axis.truncate();
                    // Reach: the model's scale times a generous mesh radius.
                    let reach = model.x_axis.truncate().length() * m.radius + 2.0;
                    if !cascades.casts_into(c, pos, reach) {
                        continue;
                    }
                    let i = list.len() as u32;
                    list.push(*inst);
                    match draws.last_mut() {
                        Some((last, r)) if last == mesh && r.end == i => r.end = i + 1,
                        _ => draws.push((*mesh, i..i + 1)),
                    }
                }
            }
            if list.len() > v.shadow_capacity {
                v.shadow_capacity = list.len().next_power_of_two();
                v.shadow_instances = instance_buffer(device, "shadows: instances", v.shadow_capacity * std::mem::size_of::<super::models::MeshInstance>());
            }
            if !list.is_empty() {
                queue.write_buffer(&v.shadow_instances, 0, bytemuck::cast_slice(&list));
            }
            let (mw, mh) = self.map_size.unwrap_or((1.0, 1.0));
            let lighting = self.map.lighting();
            let globals = Globals {
                view_proj: c.view_proj().to_cols_array_2d(),
                light_dir: light.extend(0.0).to_array(),
                params: [water, self.time(), self.frame_game_seconds, 0.0],
                camera: c.eye().extend(1.0).to_array(),
                overlay: match self.overlay {
                    Some((_, mode, ext)) => [1.0, mode, ext[0], ext[1]],
                    None => [0.0, 0.0, 1.0, 1.0],
                },
                // w: the LOD scale, for the shaders' cutoffs.
                forward: (c.target - c.eye()).normalize().extend(c.lod_scale).to_array(),
                light_vp: cascades.view_proj.map(|m| m.to_cols_array_2d()),
                shadow: [cascades.ends[0], cascades.ends[1], if self.shadows_on() { cascades.count as f32 } else { 0.0 }, 1.0 / self.shadows.size as f32],
                horizon: [1.0 / mw, 1.0 / mh, self.horizon_bound as u32 as f32, 0.0],
                terrain: [v.terrain_step as f32, self.terrain_view.as_ref().map_or(0.0, |t| t.width as f32), self.terrain_view.as_ref().map_or(0.0, |t| t.height as f32), 0.0],
                sun_color: lighting.map_or([1.0, 1.0, 1.0, 1.0], |l| [l.sun_color[0], l.sun_color[1], l.sun_color[2], l.multiplier]),
                ambience: {
                    let a = lighting.map_or([0.3; 3], |l| l.ambience);
                    let mip_bias = self.map.environment().filter(|_| self.horizon_bound).map_or(0.0, |(_, size)| (size.max(1) as f32 / 256.0).log2());
                    [a[0], a[1], a[2], mip_bias]
                },
                shadow_fill: lighting.map_or([0.0; 4], |l| [l.shadow_fill[0], l.shadow_fill[1], l.shadow_fill[2], 0.0]),
                fog: match self.fog_extent {
                    Some([w, h]) => [1.0, w, h, 0.0],
                    None => [0.0, 1.0, 1.0, 0.0],
                },
                shadow_reach: [cascades.ends[cascades.count - 1], 0.0, 0.0, 0.0],
                graphics: [self.input.settings.fidelity as f32, 0.0, 0.0, 0.0],
                vision: self.vision.globals().0,
                vision_dims: self.vision.globals().1,
            };
            queue.write_buffer(&v.globals, 0, bytemuck::bytes_of(&globals));
        }
        // Scripts' debug lines: the sim canvas (listed once per snapshot)
        // and this view's world shapes.
        self.debug_lines.set_canvas(self.latest.as_ref().map(|s| &s.debug.draw), self.latest.as_ref().map_or(0, |s| s.tick));
        self.debug_lines.prepare(device, queue, &mut v.lines, None, size, 2.0 * c.half_tangents().1 / size[1].max(1.0));
        self.views_gpu[index] = Some(v);
    }

    /// A view's own GPU state, empty (`prepare_view` fills it).
    fn view_gpu(&self, device: &wgpu::Device) -> ViewGpu {
        let globals = device.create_buffer(&wgpu::BufferDescriptor { label: Some("scene: globals"), size: std::mem::size_of::<Globals>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        ViewGpu {
            bind_group: self.view_bind_group(device, &globals),
            globals,
            bound: self.bind_generation,
            terrain_lod: 0,
            terrain_step: 1,
            terrain_draws: Vec::new(),
            decals: Default::default(),
            graph: Default::default(),
            lines: Default::default(),
            icons: Default::default(),
            ranges: Default::default(),
            cascades: None,
            lights: self.shadows.lights(device),
            shadow_instances: instance_buffer(device, "shadows: instances", 1024 * std::mem::size_of::<super::models::MeshInstance>()),
            shadow_capacity: 1024,
            shadow_draws: vec![Vec::new(); super::globals::CASCADES],
        }
    }

    /// Group 0 for a view: its `globals` with the shared textures.
    fn view_bind_group(&self, device: &wgpu::Device, globals: &wgpu::Buffer) -> wgpu::BindGroup {
        let horizon = self.map.horizon().unwrap_or(&self.lit);
        make_bind_group(device, &self.layout, globals, &self.overlay_view, &self.fog_view, &self.sampler, &self.shadows, horizon, &self.env, &self.heights, &self.vision)
    }

    /// Shadow casters drawn into each cascade this frame.
    #[allow(dead_code)]
    pub fn shadow_caster_counts(&self) -> Vec<u32> {
        self.views_gpu.first().and_then(Option::as_ref).map_or_else(Vec::new, |v| v.shadow_draws.iter().map(|d| d.iter().map(|(_, r)| r.end - r.start).sum()).collect())
    }

    /// Any model, map texture or effect asset still loading.
    #[allow(dead_code)]
    pub fn loading(&self) -> bool {
        self.stage.models.loading() || self.stage.particles.loading() || self.map.loading() || self.command_graph.loading() || self.resource_icons.loading()
    }

    /// Starts loading every mesh's lower levels of detail (tests).
    #[allow(dead_code)]
    pub fn request_all_lods(&mut self) {
        self.stage.models.request_all_lods();
    }

    #[allow(dead_code)]
    pub fn asset_failures(&self) -> Vec<String> {
        let mut v: Vec<String> = self.stage.models.failures().iter().chain(self.map.failures()).chain(self.stage.particles.failures()).chain(self.command_graph.failures()).chain(self.resource_icons.failures()).cloned().collect();
        v.sort();
        v.dedup();
        v
    }

    /// World transform of `bone` of `entity` as drawn this frame (posed and
    /// interpolated), for entities whose bones were recorded.
    #[allow(dead_code)]
    pub fn bone_world(&self, entity: u32, bone: u32) -> Option<Mat4> {
        let (model, base, mesh) = self.stage.frame_bones[self.stage.frame_bones.binary_search_by_key(&entity, |b| b.0).ok()?].1;
        let sk = &self.stage.models.ready(mesh)?.skeleton;
        let skin = (base != super::models::NO_BONES).then(|| self.stage.palette.get(base as usize + bone as usize)).flatten();
        Some(model * sk.bone(bone as usize, skin)?)
    }

    /// `ren_MipSkipLevels` (FAF's Texture Detail: 0 high, 1 medium, 2 low):
    /// models and the terrain's strata sampled from that many mips down.
    pub fn set_mip_skip(&mut self, device: &wgpu::Device, skip: u32) {
        tracing::info!(skip, "texture mip skip");
        self.stage.models.set_mip_skip(device, skip);
        self.map.set_mip_skip(device, skip);
    }

    /// Sun shadows drawn: on (the debug switch) and Shadow Fidelity not Off.
    pub fn shadows_on(&self) -> bool {
        self.input.settings.shadows && self.input.settings.shadow_fidelity > 0
    }

    /// Cascades for the Shadow Fidelity: Low 2, else all.
    fn shadow_cascades(&self) -> usize {
        if self.input.settings.shadow_fidelity == 1 { 2 } else { super::globals::CASCADES }
    }

    /// `ren_ShadowSize` (FAF's Shadow Resolution option): texels per sun
    /// shadow cascade side.
    pub fn set_shadow_size(&mut self, device: &wgpu::Device, size: u32) {
        if self.shadows.set_size(device, size) {
            tracing::info!(size = self.shadows.size, "shadow map size");
            self.rebind();
        }
    }

    /// Group 0 changed (overlay, horizon or shadow maps): each view rebinds
    /// its own as it prepares.
    fn rebind(&mut self) {
        self.bind_generation += 1;
    }

    /// Applies the fog of war changes received since the last frame: a new
    /// grid reallocates the texture, changed rows are written in place.
    fn update_fog(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        use crate::engine::snapshot::FogUpdate;
        for update in std::mem::take(&mut self.pending_fog) {
            match update {
                FogUpdate::Off => self.fog_extent = None,
                FogUpdate::Grid { cols, rows, cell, full, bands } => {
                    let fits = self.fog_texture.as_ref().is_some_and(|t| t.width() == cols && t.height() == rows);
                    if full && !fits {
                        let (view, tex) = overlay_texture(device, "scene: fog of war", cols, rows);
                        self.fog_view = view;
                        self.fog_texture = Some(tex);
                        self.rebind();
                    } else if !fits {
                        // Rows of a grid we never had in full.
                        continue;
                    }
                    let Some(tex) = &self.fog_texture else { continue };
                    for (first, levels) in bands {
                        let bytes: Vec<u8> = levels.iter().map(|&l| l.saturating_mul(85)).collect();
                        let n = (bytes.len() as u32 / cols.max(1)).min(rows.saturating_sub(first));
                        if n == 0 {
                            continue;
                        }
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: first, z: 0 }, aspect: wgpu::TextureAspect::All },
                            &bytes[..(n * cols) as usize],
                            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(cols), rows_per_image: Some(n) },
                            wgpu::Extent3d { width: cols, height: n, depth_or_array_layers: 1 },
                        );
                    }
                    self.fog_extent = Some([cols as f32 * cell, rows as f32 * cell]);
                }
            }
        }
    }

    /// Uploads the latest grid overlay if it changed (`mode` 0 nav, 1 threat).
    fn update_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let grid = self.latest.as_ref().and_then(|s| s.debug.grid.clone());
        let Some(grid) = grid else {
            self.overlay = None;
            return;
        };
        if self.overlay.is_some_and(|(v, ..)| v == grid.version) {
            return;
        }
        // Mode from the version's top bits: 0 nav, 1 threat.
        let mode = (grid.version >> 40) as u8;
        let max = grid.values.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
        let bytes: Vec<u8> = grid
            .values
            .iter()
            .map(|v| match mode {
                1 => ((v / max).clamp(0.0, 1.0) * 255.0) as u8,
                _ => (*v * 255.0) as u8,
            })
            .collect();
        let (view, tex) = overlay_texture(device, "scene: grid overlay", grid.cols, grid.rows);
        queue.write_texture(
            tex.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(grid.cols), rows_per_image: Some(grid.rows) },
            wgpu::Extent3d { width: grid.cols, height: grid.rows, depth_or_array_layers: 1 },
        );
        self.overlay_view = view;
        self.rebind();
        self.overlay = Some((grid.version, mode as f32, [grid.cols as f32 * grid.cell, grid.rows as f32 * grid.cell]));
    }

    /// The terrain's height at (x, z), or the water's surface where that's
    /// higher.
    pub fn surface_at(&self, x: f32, z: f32) -> f32 {
        let ground = self.height_at(x, z);
        match self.terrain_view.as_ref().and_then(|t| t.water) {
            Some(water) => ground.max(water),
            None => ground,
        }
    }

    /// Terrain height at (x, z) (nearest sample).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let Some(t) = &self.terrain_view else { return 0.0 };
        let xi = (x.max(0.0) as u32).min(t.width);
        let zi = (z.max(0.0) as u32).min(t.height);
        t.heights[(zi * (t.width + 1) + xi) as usize]
    }

    fn upload_terrain(&mut self, device: &wgpu::Device, t: &TerrainView) {
        self.height_range = height_range(&t.heights);
        // Every sample up to 20 km maps; bigger ones every n-th (the mesh
        // at most ~1024^2). Coarser levels index the same vertices.
        let step = super::terrain::mesh_step(t);
        let (cols, rows) = (t.width / step + 1, t.height / step + 1);
        let verts = terrain_vertices(t, 0..rows, 0..cols);
        let lods = super::terrain::lod_indices(cols, rows, step)
            .into_iter()
            .map(|l| TerrainLod {
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(&format!("terrain: indices (every {} samples)", l.step)), contents: bytemuck::cast_slice(&l.indices), usage: wgpu::BufferUsages::INDEX }),
                tiles: l.tiles,
                step: l.step,
            })
            .collect();
        let tile = super::terrain::TERRAIN_TILE * step;
        let (tiles_x, tiles_z) = (t.width.div_ceil(tile), t.height.div_ceil(tile));
        let tile_bounds = (0..tiles_z).flat_map(|tz| (0..tiles_x).map(move |tx| (tx, tz))).map(|(tx, tz)| tile_bound(t, tx, tz)).collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("terrain: vertices"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST | TEST_READBACK });
        self.terrain = Some(TerrainMesh { vertices, lods, tile_bounds });
        self.heights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("terrain: heights"), contents: bytemuck::cast_slice(&t.heights), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | TEST_READBACK });
        self.rebind();
        self.upload_water(device, t);
    }

    /// Can `t` be applied as a change to the terrain uploaded (same size and
    /// water)? Else it's uploaded whole.
    fn patchable(&self, t: &TerrainView) -> bool {
        let (Some(old), Some(_)) = (&self.terrain_view, &self.terrain) else { return false };
        (old.width, old.height, old.water) == (t.width, t.height, t.water) && old.heights.len() == t.heights.len()
    }

    /// Applies a worked-out terrain change, if one is ready, and starts on
    /// the newest waiting one. A change (flattened under a structure, ...)
    /// arrives as the whole heightfield; a pool worker diffs it against the
    /// one uploaded and builds what changed ([`terrain_patch`]), so the
    /// render thread only has the GPU copy them into the existing buffers.
    fn poll_terrain(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(rx) = &self.terrain_patch {
            match rx.try_recv() {
                Ok(p) => {
                    self.terrain_patch = None;
                    self.apply_patch(queue, p);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.terrain_patch = None,
            }
        }
        let (Some(next), Some(old)) = (self.terrain_next.take(), self.terrain_view.clone()) else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let device = device.clone();
        rayon::spawn(move || {
            let _ = tx.send(terrain_patch(&device, &old, next));
        });
        self.terrain_patch = Some(rx);
    }

    fn apply_patch(&mut self, queue: &wgpu::Queue, p: TerrainPatch) {
        let _s = rts_core_engine::prof_span!("terrain: apply change");
        let Some(mesh) = &mut self.terrain else { return };
        if let Some(staging) = &p.staging {
            let mut encoder = p.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upload: terrain change") });
            for &(to_vertices, from, to, size) in &p.copies {
                encoder.copy_buffer_to_buffer(staging, from, if to_vertices { &mesh.vertices } else { &self.heights }, to, size);
            }
            queue.submit([encoder.finish()]);
        }
        for &(i, bound) in &p.tiles {
            mesh.tile_bounds[i] = bound;
        }
        self.height_range = p.range;
        self.map.receive(&p.view, self.stage.models.vfs());
        self.terrain_view = Some(p.view);
    }

    /// The terrain as uploaded: vertex and height buffers read back, tile
    /// bounds, height range. Applies `t` first, in place if it can (whether
    /// it did is returned too), else in full.
    #[cfg(test)]
    pub fn apply_terrain(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, t: Arc<TerrainView>, in_place: bool) -> (bool, Vec<u8>, Vec<u8>, Vec<(Vec3, f32)>, (f32, f32)) {
        let updated = in_place && self.patchable(&t);
        if updated {
            self.terrain_next = Some(t);
            self.settle_terrain(device, queue);
        } else {
            self.upload_terrain(device, &t);
            self.terrain_view = Some(t);
        }
        let read = |b: &wgpu::Buffer| {
            let copy = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: b.size(), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            let mut enc = device.create_command_encoder(&Default::default());
            enc.copy_buffer_to_buffer(b, 0, &copy, 0, b.size());
            queue.submit([enc.finish()]);
            copy.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            copy.slice(..).get_mapped_range().to_vec()
        };
        let mesh = self.terrain.as_ref().expect("a terrain mesh");
        (updated, read(&mesh.vertices), read(&self.heights), mesh.tile_bounds.clone(), self.height_range)
    }

    /// Waits for every terrain change queued to be applied.
    #[cfg(test)]
    pub fn settle_terrain(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        while self.terrain_patch.is_some() || self.terrain_next.is_some() {
            self.poll_terrain(device, queue);
            std::thread::yield_now();
        }
    }

    /// Queues a terrain change as a snapshot would.
    #[cfg(test)]
    pub fn queue_terrain(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, t: Arc<TerrainView>) {
        assert!(self.patchable(&t));
        self.terrain_next = Some(t);
        self.poll_terrain(device, queue);
    }

    fn upload_water(&mut self, device: &wgpu::Device, t: &TerrainView) {
        self.water = t.water.map(|y| {
            let (w, hh) = (t.width as f32, t.height as f32);
            let quad: [[f32; 3]; 6] = [[0.0, y, 0.0], [0.0, y, hh], [w, y, 0.0], [w, y, 0.0], [0.0, y, hh], [w, y, hh]];
            (device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("scene: water quad"), contents: bytemuck::cast_slice(&quad), usage: wgpu::BufferUsages::VERTEX }), y)
        });
    }

    /// Interpolates the entities to now and fills the instance buffer.
    /// Was the placement ghost prepared for drawing this frame (its mesh
    /// known and loaded)?
    /// The build-in technique and fraction complete of each model drawn
    /// with one this frame (render-state tests).
    /// The shaders' clock (seconds).
    fn time(&self) -> f32 {
        self.frozen_time.unwrap_or_else(|| self.started.elapsed().as_secs_f32())
    }

    /// The vertex motion (`VertexMotion` as u32) of each model drawn.
    #[allow(dead_code)]
    pub fn drawn_vertex_motions(&self) -> Vec<u32> {
        self.stage.model_list.iter().map(|(_, i)| i.bones[1]).collect()
    }

    #[allow(dead_code)]
    pub fn drawn_build_models(&self) -> Vec<(crate::engine::api::mesh::BuildShader, f32)> {
        self.stage.model_list.iter().filter_map(|(m, i)| self.stage.models.ready(*m)?.build.map(|b| (b, i.team[3]))).collect()
    }

    /// Whether entity `id`'s mesh draws this frame (render tests: its LOD).
    #[allow(dead_code)]
    pub fn mesh_drawn(&self, id: u32) -> bool {
        self.stage.instance_entities.iter().any(|&i| self.frame_entities.get(i as usize).is_some_and(|e| e.id == id))
    }

    /// This frame's vision circles (`[x, z, radius]`; fog of war).
    #[allow(dead_code)]
    pub fn vision_circles(&self) -> &[[f32; 3]] {
        &self.vision.circles
    }

    /// How many instances the box pass draws this frame (render tests:
    /// failed-mesh boxes; the LOD markers used to reach it too).
    #[allow(dead_code)]
    pub fn drawn_box_instances(&self) -> u32 {
        self.instance_count
    }

    #[allow(dead_code)]
    pub fn ghost_drawn(&self) -> bool {
        self.ghost_draw.is_some()
    }

    /// Queued builds' ghosts drawn this frame (render-state tests).
    #[allow(dead_code)]
    pub fn queued_ghosts(&self) -> usize {
        self.queued_ghost_draws.len()
    }


    /// Draws the world views (prepared: `prepare_finish`) into `target`,
    /// each inside its own rectangle (the first clears the frame), through
    /// the bloom. Returns the encoders unfinished, in order: finishing is
    /// where wgpu replays a recorded pass (most of a frame's command cost),
    /// so the frame finishes them in parallel (`frame::render_frame`).
    pub fn draw(&self, device: &wgpu::Device, target: &wgpu::TextureView, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>, normal_timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) -> Vec<wgpu::CommandEncoder> {
        let _span = rts_core_engine::prof_span!("encode scene");
        self.stage.models.draws.set(0);
        self.stage.particles.draws.set(0);
        let (mut timestamps, mut normal_timestamps) = (timestamps, normal_timestamps);
        let mut encoders = Vec::new();
        // Fog of war's vision circles, binned before anything fogs.
        let mut vision = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame: vision circles") });
        self.vision.bin(&mut vision);
        encoders.push(vision);
        for i in 0..self.input.views.len() {
            encoders.extend(self.draw_view(device, i, timestamps.take(), normal_timestamps.take()));
        }
        let mut post = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame: post (bloom, SMAA)") });
        if self.input.settings.smaa {
            self.bloom.apply(&mut post, &self.smaa.input, true);
            self.smaa.apply(&mut post, target);
        } else {
            self.bloom.apply(&mut post, target, false);
        }
        encoders.push(post);
        encoders
    }

    /// World view `index`'s passes, prepared (`prepare_view`): the shadows
    /// and terrain normals, then the world. The first view clears the
    /// frame's targets; later ones keep what the views before drew.
    fn draw_view(&self, device: &wgpu::Device, index: usize, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>, normal_timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) -> Vec<wgpu::CommandEncoder> {
        let shown = &self.input.views[index];
        // (Unprepared: no camera yet. It still clears the frame.)
        let v = self.views_gpu.get(index).and_then(Option::as_ref).filter(|_| self.input.cameras.get(&shown.camera).is_some());
        let mut first = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame: shadows + terrain normals") });
        let encoder = &mut first;
        // With the map's textures in: the normals pass first (it clears the
        // depth buffer the scene pass then loads).
        let map_terrain = match (&self.terrain, v.filter(|_| self.map.is_ready())) {
            (Some(m), Some(v)) => Some((&m.vertices, &m.lods[v.terrain_lod.min(m.lods.len() - 1)].indices, v.terrain_draws.as_slice())),
            _ => None,
        };
        // Sun shadows first: every model casts (effects and glows don't).
        if let (true, Some(v), Some(cascades)) = (self.shadows_on(), v, v.and_then(|v| v.cascades)) {
            let _s = rts_core_engine::prof_span!("shadow casters");
            for (c, draws) in v.shadow_draws.iter().enumerate().take(cascades.count) {
                let mut pass = self.shadows.begin(encoder, c, &v.lights);
                let mut bound = self.stage.models.begin(&mut pass, &v.shadow_instances);
                let mut current = None;
                for (mesh, range) in draws {
                    let Some(m) = self.stage.models.ready(*mesh) else { continue };
                    if current != Some(m.cutout) {
                        pass.set_pipeline(if m.cutout { &self.shadows.cutout } else { &self.shadows.opaque });
                        current = Some(m.cutout);
                    }
                    self.stage.models.draw(&mut pass, &mut bound, *mesh, range.clone());
                }
            }
        }
        if let (Some(t), Some(v)) = (map_terrain, v) {
            let _s = rts_core_engine::prof_span!("terrain normals");
            self.map.draw_normals(encoder, &self.depth, &v.bind_group, t, normal_timestamps, shown, &v.decals);
        }
        let ranges = v.map(|v| &v.ranges).filter(|r| r.active() && map_terrain.is_some());
        // The ground's depth for the range overlays (before any unit).
        if ranges.is_some() {
            self.ranges.copy_ground_depth(encoder, &self.depth_texture);
        }
        let mut second = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame: world") });
        let encoder = &mut second;
        // The first view clears the frame; later ones draw over the cleared
        // frame inside their own rectangles.
        let clear = |color: wgpu::Color| if index == 0 { wgpu::LoadOp::Clear(color) } else { wgpu::LoadOp::Load };
        'scene: {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.bloom.scene,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    }),
                    // Glow starts at FA's terrain value (`glowMinimum`), which
                    // terrain and water keep.
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.bloom.glow,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: clear(wgpu::Color { r: 0.01, g: 0.0, b: 0.0, a: 0.0 }), store: wgpu::StoreOp::Store },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: if map_terrain.is_some() { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(1.0) }, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: timestamps.or_else(|| super::gpu_prof::pass_writes("world")),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let Some(v) = v else { break 'scene };
            shown.bind(&mut pass);
            pass.set_bind_group(0, &v.bind_group, &[]);
            // The sky first, behind everything (where the camera sees past
            // the terrain).
            if self.input.settings.sky && map_terrain.is_some() {
                pass.timed_push("sky");
                self.map.draw_sky(&mut pass);
                pass.timed_pop();
            }
            pass.timed_push("terrain");
            if let Some(t) = map_terrain {
                self.map.draw_albedo(&mut pass, t, &v.decals);
            } else if let Some(m) = &self.terrain {
                let l = &m.lods[v.terrain_lod.min(m.lods.len() - 1)];
                pass.set_pipeline(&self.terrain_pipeline);
                pass.set_vertex_buffer(0, m.vertices.slice(..));
                pass.set_index_buffer(l.indices.slice(..), wgpu::IndexFormat::Uint32);
                for r in &v.terrain_draws {
                    pass.draw_indexed(r.clone(), 0, 0..1);
                }
            }
            pass.timed_pop();
            if self.instance_count > 0 {
                pass.timed_push("boxes (models still loading)");
                pass.set_pipeline(&self.box_pipeline);
                pass.set_vertex_buffer(0, self.cube.0.slice(..));
                pass.set_vertex_buffer(1, self.instances.slice(..));
                pass.set_index_buffer(self.cube.1.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..self.cube.2, 0, 0..self.instance_count);
                pass.timed_pop();
            }
            if !self.model_draws.is_empty() {
                let _s = rts_core_engine::prof_span!("draw models");
                pass.timed_push("models");
                let mut bound = self.stage.models.begin(&mut pass, &self.model_instances);
                let mut current = None;
                for (mesh, range) in &self.model_draws {
                    use crate::engine::api::mesh::BuildShader;
                    let Some(m) = self.stage.models.ready(*mesh) else { continue };
                    if let Some(style) = m.build {
                        // Moho's build techniques: pass 0, then the overlay
                        // (UEF, Cybran, Aeon); the cube writes no depth.
                        current = None;
                        pass.set_pipeline(if style == BuildShader::UefBuildCube { &self.stage.models.build_cube_pipeline } else { &self.stage.models.build_pipeline });
                        self.stage.models.draw(&mut pass, &mut bound, *mesh, range.clone());
                        if !matches!(style, BuildShader::Seraphim | BuildShader::UefBuildCube) {
                            pass.set_pipeline(&self.stage.models.build_overlay_pipeline);
                            self.stage.models.draw(&mut pass, &mut bound, *mesh, range.clone());
                        }
                        continue;
                    }
                    if m.shading.is_shield() || m.shading.effect_only() {
                        continue; // after the water
                    }
                    let kind = if m.effect { 2 } else { m.cutout as u8 };
                    if current != Some(kind) {
                        pass.set_pipeline(match kind {
                            2 => &self.stage.models.effect_pipeline,
                            1 => &self.stage.models.cutout_pipeline,
                            _ => &self.stage.models.pipeline,
                        });
                        current = Some(kind);
                    }
                    self.stage.models.draw(&mut pass, &mut bound, *mesh, range.clone());
                }
                pass.timed_pop();
            }
            // Particles sorted under the water surface (torpedo wakes,
            // bubbles) go before it.
            pass.timed_push("particles: underwater");
            self.stage.particles.draw_underwater(&mut pass);
            pass.timed_pop();
            if let Some((vb, _)) = &self.water {
                let _s = rts_core_engine::prof_span!("draw water");
                pass.timed_push("water");
                if !self.map.draw_water(&mut pass, vb) {
                    pass.set_pipeline(&self.water_pipeline);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.draw(0..6, 0..1);
                }
                pass.timed_pop();
            }
            // Shield bubbles and effect techniques, after the water (FA's
            // `STAGE_POSTWATER`).
            if !self.model_draws.is_empty() {
                use crate::engine::api::mesh::Shading;
                // (The water pass binds its own group 0.)
                pass.timed_push("models: shields + effect techniques");
                pass.set_bind_group(0, &v.bind_group, &[]);
                let mut bound = self.stage.models.begin(&mut pass, &self.model_instances);
                for (mesh, range) in &self.model_draws {
                    let Some(m) = self.stage.models.ready(*mesh) else { continue };
                    // Effect techniques (and the two-pass ones' overlays).
                    if let Some(effect) = m.shading.effect_pass() {
                        pass.set_pipeline(self.stage.models.effect_pipeline(effect));
                        self.stage.models.draw(&mut pass, &mut bound, *mesh, range.clone());
                        continue;
                    }
                    if !m.shading.is_shield() {
                        continue;
                    }
                    pass.set_pipeline(if m.shading == Shading::ShieldSeraphim { &self.stage.models.shield_add_pipeline } else { &self.stage.models.shield_pipeline });
                    self.stage.models.draw(&mut pass, &mut bound, *mesh, range.clone());
                    if m.shading == Shading::ShieldCybran {
                        pass.set_pipeline(&self.stage.models.shield_offset_pipeline);
                        self.stage.models.draw(&mut pass, &mut bound, *mesh, range.clone());
                    }
                }
                pass.timed_pop();
            }
            // The range overlays, over the ground and water and under the
            // particles and order lines.
            if let Some(rings) = ranges {
                drop(pass);
                self.ranges.draw(encoder, &self.bloom.scene, &self.bloom.glow, &self.depth, shown, rings);
                pass = self.resume_pass(encoder, "world (after range overlays)");
                shown.bind(&mut pass);
                pass.set_bind_group(0, &v.bind_group, &[]);
            }
            // Refracting particles read the frame so far: copy it, then go
            // on in a second pass.
            if self.stage.particles.refracts() {
                drop(pass);
                self.bloom.copy_background(encoder);
                pass = self.resume_pass(encoder, "world (after the refraction copy)");
                shown.bind(&mut pass);
                pass.set_bind_group(0, &v.bind_group, &[]);
            }
            {
                let _s = rts_core_engine::prof_span!("draw particles");
                pass.timed_push("particles");
                self.stage.particles.draw(&mut pass);
                pass.timed_pop();
                pass.timed_push("particles: lights");
                self.stage.particles.draw_lights(&mut pass);
                pass.timed_pop();
            }
            // Queued builds' ghosts (FAF's `UnitPlace`), under their lines.
            if !self.queued_ghost_draws.is_empty() {
                pass.timed_push("ghosts: queued builds");
                pass.set_pipeline(&self.stage.models.place_pipeline);
                let mut bound = self.stage.models.begin(&mut pass, &self.model_instances);
                for &(mesh, slot) in &self.queued_ghost_draws {
                    self.stage.models.draw(&mut pass, &mut bound, mesh, slot..slot + 1);
                }
                pass.timed_pop();
            }
            // Deposits' icons zoomed out, under the order lines.
            pass.timed_push("resource icons");
            self.resource_icons.draw(&mut pass, &v.icons);
            pass.timed_pop();
            // Order lines and waypoints, on the ground and over the models
            // (as the base game draws them).
            pass.timed_push("command graph (order lines)");
            self.command_graph.draw(&mut pass, &v.graph);
            pass.timed_pop();
            // Scripts' debug lines, over everything in the world.
            pass.timed_push("debug lines");
            self.debug_lines.draw(&mut pass, &v.lines);
            pass.timed_pop();
            // FAF's `UnitPlace` renders after water and effects.
            if self.ghost_draw.is_some() || !self.template_ghost_draws.is_empty() {
                pass.timed_push("ghosts: placement");
                pass.set_pipeline(&self.stage.models.place_pipeline);
                let mut bound = self.stage.models.begin(&mut pass, &self.model_instances);
                for &(mesh, slot) in self.ghost_draw.iter().chain(&self.template_ghost_draws) {
                    self.stage.models.draw(&mut pass, &mut bound, mesh, slot..slot + 1);
                }
                pass.timed_pop();
            }
        }
        vec![first, second]
    }

    /// While a structure is being placed: the build skirts of the
    /// structures already standing (or going up) near the ghosts, so new
    /// ones can be packed against them.
    fn nearby_skirts(&self) -> Vec<[f32; 4]> {
        /// How far from a ghost (world units) other structures' skirts show.
        const NEAR: f32 = 64.0;
        let ghosts: Vec<Vec3> = self.input.ghost.iter().chain(&self.input.template_ghosts).map(|g| g.position).collect();
        let Some(skirts) = self.input.blueprint_skirts.as_deref().filter(|_| !ghosts.is_empty()) else { return Vec::new() };
        self.frame_entities
            .iter()
            .filter(|e| e.kind == ViewKind::Structure)
            .filter(|e| ghosts.iter().any(|g| (g.x - e.position[0]).abs() < NEAR && (g.z - e.position[2]).abs() < NEAR))
            .filter_map(|e| {
                let [x0, z0, x1, z1] = (*skirts.get(e.blueprint as usize)?)?;
                Some([e.position[0] + x0, e.position[2] + z0, e.position[0] + x1, e.position[2] + z1])
            })
            .collect()
    }

    /// This frame's range overlay rings: per active filter, its units'
    /// ranges of the kinds it takes in, in its normal colour, with selected
    /// units' in its selected colour and the hovered unit's in its rollover
    /// colour (each drawn once, as FAF's patch has it); selected and hovered
    /// units' rings no active filter draws, each kind in its own overlay's
    /// selected / rollover colour (`range_RenderSelected` /
    /// `range_RenderHighlighted`; kinds in [`SELECTION_RANGE_KINDS`]); and
    /// the placement ghost's every range, each kind in its own overlay's
    /// colour, whatever is toggled (FAF's `range_RenderBuild`). Line widths
    /// go from the
    /// zoomed-in to zoomed-out values with the zoom of the view's camera
    /// `c`; the rings'
    /// volumes span the map's height band (lowest ground or water less 5 to
    /// the highest ground, FAF's clamp).
    /// The range overlays' cell lists at most, `None` for the usual limit
    /// (tests: 0 draws every pixel through the overflow path).
    #[cfg(test)]
    pub(super) fn set_range_list_limit(&mut self, n: Option<usize>) {
        self.ranges.max_list = n.unwrap_or(super::ranges::MAX_LIST);
    }

    pub(super) fn range_groups(&self, c: &Camera) -> Vec<super::ranges::Group> {
        use super::ranges::{Group, Ring};
        let Some(ranges) = &self.input.blueprint_ranges else { return Vec::new() };
        // The placement ghost's ranges, and its assist ring (moho's ghost
        // rings, MohoEngine.dll 0x103b7e50: `GuardScanRadius` when
        // `ShowAssistRangeOnSelect`), drawn in the misc overlay's colour (ours).
        let assist = self.input.ghost.as_ref().and_then(|g| self.input.blueprint_assist_range.as_ref()?.get(g.blueprint as usize).copied()).filter(|&r| r > 0.0);
        let ghost = self.input.ghost.as_ref().and_then(|g| Some((g.position, ranges.get(g.blueprint as usize).filter(|r| !r.is_empty() || assist.is_some())?)));
        let hovered = self.input.hovered;
        let own = (self.input.settings.range_selected && !self.input.selection.is_empty()) || (self.input.settings.range_highlighted && hovered.is_some());
        if self.input.range_filters.is_empty() && ghost.is_none() && !own {
            return Vec::new();
        }
        let zoom = ((c.zoom - c.min_zoom) / (c.max_zoom - c.min_zoom).max(1e-3)).clamp(0.0, 1.0);
        let lerp = |t: [f32; 2]| t[0] + (t[1] - t[0]) * zoom;
        let water = self.water.as_ref().map(|w| w.1);
        let low = water.map_or(self.height_range.0, |w| w.min(self.height_range.0)) - 5.0;
        let high = self.height_range.1.max(water.unwrap_or(f32::MIN)) + 1.0;
        let mut normal = Vec::new();
        let mut selected = Vec::new();
        let mut rollover = Vec::new();
        for f in self.input.range_filters.iter() {
            let (outer, inner) = (lerp(f.outer), lerp(f.inner));
            let mut groups = [Vec::new(), Vec::new(), Vec::new()];
            for e in &self.frame_entities {
                // A blip's ranges would identify it, a remembered one's
                // would describe a structure that may be gone; a unit still
                // being built (an upgrade inside its structure, a factory's
                // unit) has no weapons or intel working yet.
                if !matches!(e.kind, ViewKind::Unit | ViewKind::Structure) || e.sighting != crate::engine::snapshot::Sighting::Seen || e.build < 1.0 || !f.members.get(e.blueprint as usize).copied().unwrap_or(false) {
                    continue;
                }
                let Some(list) = ranges.get(e.blueprint as usize) else { continue };
                let state = if hovered == Some(e.id) {
                    2
                } else if self.input.selection.contains(&e.id) {
                    1
                } else {
                    0
                };
                for &(kind, min, max) in list {
                    if f.kinds & (1 << kind) != 0 {
                        groups[state].push(Ring { center_band: [e.position[0], e.position[2], low, high], radii: [max, outer, min, inner] });
                    }
                }
            }
            let [n, s, r] = groups;
            normal.push(Group { color: f.colors[0], rings: n });
            selected.push(Group { color: f.colors[1], rings: s });
            rollover.push(Group { color: f.colors[2], rings: r });
        }
        // Selected (then hovered) units' own rings: the kinds no active
        // filter takes them in for, in each kind's selected / rollover
        // colour. Kinds sharing an overlay (countermeasure and shields) share
        // a group.
        let mut own_groups: Vec<Group> = Vec::new();
        if own {
            let mut add = |ids: &mut dyn Iterator<Item = u32>, state: usize| {
                for id in ids {
                    let Some(e) = self.frame_entities.binary_search_by_key(&id, |e| e.id).ok().map(|i| &self.frame_entities[i]) else { continue };
                    if !matches!(e.kind, ViewKind::Unit | ViewKind::Structure) || e.sighting != crate::engine::snapshot::Sighting::Seen || e.build < 1.0 {
                        continue;
                    }
                    let Some(list) = ranges.get(e.blueprint as usize) else { continue };
                    let drawn = self.input.range_filters.iter().filter(|f| f.members.get(e.blueprint as usize).copied().unwrap_or(false)).fold(0u16, |k, f| k | f.kinds);
                    for &(kind, min, max) in list {
                        if SELECTION_RANGE_KINDS & (1 << kind) == 0 || drawn & (1 << kind) != 0 {
                            continue;
                        }
                        let Some(style) = self.input.range_styles.get(kind as usize).copied().flatten() else { continue };
                        let ring = Ring { center_band: [e.position[0], e.position[2], low, high], radii: [max, lerp(style.outer), min, lerp(style.inner)] };
                        let color = style.colors[state];
                        match own_groups.iter_mut().find(|g| g.color == color) {
                            Some(g) => g.rings.push(ring),
                            None => own_groups.push(Group { color, rings: vec![ring] }),
                        }
                    }
                }
            };
            let hover = hovered.filter(|_| self.input.settings.range_highlighted);
            if self.input.settings.range_selected {
                add(&mut self.input.selection.iter().copied().filter(|&id| Some(id) != hover), 1);
            }
            add(&mut hover.into_iter(), 2);
        }
        let mut ghost_groups = Vec::new();
        if let Some((at, list)) = ghost {
            let misc = crate::engine::snapshot::RANGE_KINDS.iter().position(|k| *k == "MISC").unwrap_or(0) as u8;
            for &(kind, min, max) in list.iter().chain(assist.map(|r| (misc, 0.0, r)).as_ref()) {
                let Some(style) = self.input.range_styles.get(kind as usize).copied().flatten() else { continue };
                let ring = Ring { center_band: [at.x, at.z, low, high], radii: [max, lerp(style.outer), min, lerp(style.inner)] };
                ghost_groups.push(Group { color: style.colors[0], rings: vec![ring] });
            }
        }
        normal.into_iter().chain(selected).chain(rollover).chain(own_groups).chain(ghost_groups).collect()
    }

    /// The scene pass again, keeping what it drew (frame, glow, depth).
    fn resume_pass<'e>(&'e self, encoder: &'e mut wgpu::CommandEncoder, label: &str) -> wgpu::RenderPass<'e> {
        let keep = wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store };
        let target = |view| Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: keep });
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[target(&self.bloom.scene), target(&self.bloom.glow)],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }),
            timestamp_writes: super::gpu_prof::pass_writes(label),
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }
}

/// Range kinds (bits by `snapshot::RANGE_KINDS`) selected and hovered units
/// show with no overlay on: all of them (weapons with their minimum range,
/// shields, build range, radar / sonar / omni, stealth and cloak fields).
const SELECTION_RANGE_KINDS: u16 = (1 << crate::engine::snapshot::RANGE_KINDS.len()) - 1;

/// Draw order of a model: opaque 0, build-in techniques 1 (blended over the
/// opaque), effects 2.
fn draw_order(m: &super::models::GpuModel) -> u8 {
    if m.shading.effect_pass() == Some(crate::engine::api::mesh::EffectPass::Feedback) {
        3
    } else if m.build.is_some() {
        1
    } else if m.effect {
        2
    } else {
        0
    }
}


/// Tests read the terrain's buffers back.
const TEST_READBACK: wgpu::BufferUsages = if cfg!(test) { wgpu::BufferUsages::COPY_SRC } else { wgpu::BufferUsages::empty() };

/// Lowest and highest of the terrain's heights.
fn height_range(heights: &[f32]) -> (f32, f32) {
    heights.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)))
}

/// The terrain mesh's vertices in vertex rows `rows` and columns `cols`,
/// row by row, at its sample step: position and a normal from the samples
/// a step either side.
fn terrain_vertices(t: &TerrainView, rows: std::ops::Range<u32>, cols: std::ops::Range<u32>) -> Vec<TerrainVertex> {
    let step = super::terrain::mesh_step(t);
    let h = |x: u32, z: u32| t.heights[(z.min(t.height) * (t.width + 1) + x.min(t.width)) as usize];
    let mut verts = Vec::with_capacity(cols.len() * rows.len());
    for zi in rows {
        for xi in cols.clone() {
            let (x, z) = (xi * step, zi * step);
            let dx = h(x + step, z) - h(x.saturating_sub(step), z);
            let dz = h(x, z + step) - h(x, z.saturating_sub(step));
            let n = Vec3::new(-dx, 2.0 * step as f32, -dz).normalize();
            verts.push(TerrainVertex { position: super::terrain::mesh_vertex(t, x, z), normal: n.to_array() });
        }
    }
    verts
}

/// Tile (`tx`, `tz`)'s bounding sphere (centre, radius) over the heights it
/// covers.
fn tile_bound(t: &TerrainView, tx: u32, tz: u32) -> (Vec3, f32) {
    let step = super::terrain::mesh_step(t);
    let tile = super::terrain::TERRAIN_TILE * step;
    let h = |x: u32, z: u32| t.heights[(z.min(t.height) * (t.width + 1) + x.min(t.width)) as usize];
    let (x0, z0, x1, z1) = (tx * tile, tz * tile, ((tx + 1) * tile).min(t.width), ((tz + 1) * tile).min(t.height));
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for z in (z0..=z1).step_by(step as usize) {
        for x in (x0..=x1).step_by(step as usize) {
            let y = h(x, z);
            (y0, y1) = (y0.min(y), y1.max(y));
        }
    }
    let (lo, hi) = (Vec3::new(x0 as f32, y0, z0 as f32), Vec3::new(x1 as f32, y1, z1 as f32));
    ((lo + hi) * 0.5, (hi - lo).length() * 0.5)
}

/// What changed between the uploaded heightfield and a new one of the same
/// size, ready to write: [`terrain_patch`].
struct TerrainPatch {
    view: Arc<TerrainView>,
    device: wgpu::Device,
    range: (f32, f32),
    /// The changed height rows and rebuilt vertex rows, written by the
    /// worker; `None` if nothing changed.
    staging: Option<wgpu::Buffer>,
    /// Copies out of `staging`: (into the vertices, else the heights; from,
    /// to, bytes).
    copies: Vec<(bool, u64, u64, u64)>,
    /// Rebuilt tile bounds by tile index.
    tiles: Vec<(usize, (Vec3, f32))>,
}

/// Diffs `new` against `old` (a pool worker: on an 81 km map that's 16M
/// samples) and builds what changed: each changed row's changed spans (a
/// flattened footprint is a few samples a side), only those copied. The
/// changed heights and rebuilt vertices go into a mapped staging buffer
/// here, so the render thread copies nothing itself.
fn terrain_patch(device: &wgpu::Device, old: &TerrainView, new: Arc<TerrainView>) -> TerrainPatch {
    let _s = rts_core_engine::prof_span!("terrain: work out change");
    let t = &*new;
    let stride = (t.width + 1) as usize;
    let mut range = (f32::MAX, f32::MIN);
    // Changed spans: (row, first column, last column).
    let mut spans: Vec<(u32, u32, u32)> = Vec::new();
    for (z, (a, b)) in old.heights.chunks_exact(stride).zip(t.heights.chunks_exact(stride)).enumerate() {
        range = b.iter().fold(range, |(lo, hi), &v| (lo.min(v), hi.max(v)));
        // (Compared as bits: a byte compare, and NaN-safe.)
        let (a, b): (&[u32], &[u32]) = (bytemuck::cast_slice(a), bytemuck::cast_slice(b));
        if a == b {
            continue;
        }
        let first = spans.len();
        for x in (0..stride).filter(|&x| a[x] != b[x]) {
            let x = x as u32;
            match spans[first..].last_mut() {
                // Spans this close share one copy.
                Some(s) if x <= s.2 + SPAN_GAP => s.2 = x,
                _ => spans.push((z as u32, x, x)),
            }
        }
    }
    let step = super::terrain::mesh_step(t);
    let (cols, vrows) = (t.width / step + 1, t.height / step + 1);
    let tile = super::terrain::TERRAIN_TILE * step;
    let tiles_x = t.width.div_ceil(tile);
    let last = |v: u32, max: u32| (v / tile).min(max.div_ceil(tile).saturating_sub(1));
    // Vertex spans: the vertices whose position or normal (from the samples
    // a step either side) read a changed sample, per vertex row, overlapping
    // ones merged.
    let mut vertex_spans: std::collections::BTreeMap<u32, Vec<(u32, u32)>> = Default::default();
    let mut tiles = std::collections::BTreeSet::new();
    for &(z, x0, x1) in &spans {
        let (c0, c1) = (x0.saturating_sub(step) / step, ((x1 + step) / step).min(cols - 1));
        for r in z.saturating_sub(step) / step..=((z + step) / step).min(vrows - 1) {
            vertex_spans.entry(r).or_default().push((c0, c1));
        }
        for tz in z.saturating_sub(1) / tile..=last(z, t.height) {
            for tx in x0.saturating_sub(1) / tile..=last(x1, t.width) {
                tiles.insert((tz * tiles_x + tx) as usize);
            }
        }
    }
    let mut verts: Vec<(u32, u32, Vec<TerrainVertex>)> = Vec::new();
    for (r, mut row) in vertex_spans {
        row.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (c0, c1) in row {
            match merged.last_mut() {
                Some(m) if c0 <= m.1 + 1 => m.1 = m.1.max(c1),
                _ => merged.push((c0, c1)),
            }
        }
        verts.extend(merged.into_iter().map(|(c0, c1)| (r, c0, terrain_vertices(t, r..r + 1, c0..c1 + 1))));
    }
    // (Into the vertices, else the heights; the bytes; their offset there.)
    let vertex_size = std::mem::size_of::<TerrainVertex>() as u64;
    let pieces: Vec<(bool, &[u8], u64)> = spans
        .iter()
        .map(|&(z, x0, x1)| {
            let at = z as usize * stride;
            (false, bytemuck::cast_slice(&t.heights[at + x0 as usize..=at + x1 as usize]), (at as u64 + u64::from(x0)) * 4)
        })
        .chain(verts.iter().map(|(r, c0, v)| (true, bytemuck::cast_slice(v), u64::from(r * cols + c0) * vertex_size)))
        .collect();
    let size: u64 = pieces.iter().map(|p| p.1.len() as u64).sum();
    let mut copies = Vec::with_capacity(pieces.len());
    let staging = (size > 0).then(|| {
        let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("staging: terrain change"), size, usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: true });
        {
            let mut mapped = staging.slice(..).get_mapped_range_mut();
            let mut at = 0u64;
            for &(to_vertices, bytes, to) in &pieces {
                mapped.slice(at as usize..at as usize + bytes.len()).copy_from_slice(bytes);
                copies.push((to_vertices, at, to, bytes.len() as u64));
                at += bytes.len() as u64;
            }
        }
        staging.unmap();
        staging
    });
    TerrainPatch {
        range,
        staging,
        copies,
        tiles: tiles.into_iter().map(|i| (i, tile_bound(t, i as u32 % tiles_x, i as u32 / tiles_x))).collect(),
        device: device.clone(),
        view: new,
    }
}

/// Changed samples in a row this close (in samples) are copied as one span:
/// a copy each costs more than the few bytes between.
const SPAN_GAP: u32 = 16;
