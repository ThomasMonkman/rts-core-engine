//! Particle effects as FA draws them (`effects/particle.fx`, `CEfxEmitter`
//! in MohoEngine.dll): emitters, light flashes, poly trails and beams, from
//! the snapshots' effect views.
//!
//! Moho's model, which this follows: the CPU only *emits*. Each sim tick an
//! emitter samples its blueprint curves at that point of its repeat cycle
//! and writes one record per new particle (birth position, velocity,
//! acceleration, size, rotation, lifetime, texture frame / strip, ramp row,
//! drag). The record is uploaded once and never touched again: the vertex
//! shader (`particles.wesl`) evaluates position, size, rotation, billboard,
//! frame and ramp coordinate from the record and the clock, so the per-frame
//! CPU cost is emission plus placing the emitters, not the live particles.
//! Times are sim ticks: the clock is the latest snapshot's tick less one
//! plus the interpolation fraction (where the units are drawn), so effects
//! follow game speed and pause. Particles outlive their emitter.
//!
//! Records live in a pooled storage buffer in fixed segments owned by a
//! batch (texture, ramp, blend, sort order); a segment is freed once every
//! particle in it has died (the shader clips the ones that die earlier).
//! Batches draw in `SortOrder`: those below moho's `efx_ParticleWaterSurface`
//! (-101) before the water surface, the rest after it; lights (FA's `TLight`:
//! flat, no depth test, written to the glow) last. Beams and trails are CPU
//! ribbons. Blueprints and textures load on background threads; effects
//! whose assets aren't in yet simply don't draw.

use super::gpu_prof::Timed;
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock};

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3};

use crate::render::camera::LodPlane;

use crate::engine::assets::effect_bp::{CurveId, EffectBp, EmitterBp};
use crate::engine::assets::scmap::WaveGenerator;
use crate::engine::snapshot::{EffectCurve, EffectView, EffectViewKind};
use crate::engine::vfs::Vfs;

/// Emitter `Gravity`: ogrids per tick per tick (`CEfxEmitter::Emit`,
/// `MohoEngine.dll` 0x1076c768).
const GRAVITY: f32 = -0.02;
/// Records the pool may hold at most (moho logs "Particle cap exceeded").
const MAX_PARTICLES: u32 = 262_144;
/// Records per pool segment.
const SEGMENT: u32 = 256;
/// Catch-up particles emitted per frame at most (the rest wait a frame).
const CATCHUP_BUDGET: u32 = 50_000;
/// `efx_ParticleWaterSurface`: batches sorted below it draw under the
/// water surface, and `SnapToWaterline` snaps them below it.
const WATER_SURFACE_SORT: f32 = -101.0;
/// `efx_WaterOffset`: how far snapped particles sit from the surface.
const WATER_OFFSET: f32 = 0.01;
/// Blend modes: FA's 0-4 (0 alpha, 1 modulate inverse, 2 modulate 2x
/// inverse, 3 add, 4 premultiplied), [`BEAM_ADD`], [`REFRACT`].
const BLENDS: usize = 7;
/// Beams' additive mode: `One, One` (FA's `TBeam_*_ADD`; particles' add is
/// `SrcAlpha, One`).
const BEAM_ADD: u32 = 5;
/// Blueprint blend mode 5 (`TRamp_REFRACT`): `WorldRefractPS`, alpha blended.
const REFRACT: u32 = 6;
/// Slack around an emitter's anchor for the view test (its particles drift
/// and grow before they show).
const EMITTER_MARGIN: f32 = 30.0;
/// Slack for the cheap off-screen test (rest-pose position, before posing):
/// generous, as trails and big emitters reach well past their anchor.
const OFFSCREEN_MARGIN: f32 = 80.0;
const PLACE_PARALLEL_MIN: usize = 4096;

// Record flags (`particles.wesl`).
const FLAG_FLAT: u32 = 1;
const FLAG_ALIGN: u32 = 2;
const FLAG_NO_MOVE: u32 = 4;
const FLAG_DRAG: u32 = 8;
const FLAG_ANIMATED: u32 = 16;

/// Shoreline waves (moho's `WaveSystem`): no new ones while the camera is
/// zoomed out past this (MohoEngine.dll 0x1076c700, compared in
/// `WaveSystem::Update`; read here as the camera's zoom distance, inferred).
const WAVE_ZOOM: f32 = 200.0;
/// `efx_WaveCutoff`: generators further than this from the camera don't lay
/// waves (their spatial index's cutoff, MohoEngine.dll 0x10898c94).
const WAVE_CUTOFF: f32 = 400.0;
/// Generators updated at most (`WaveSystem::Update`'s query limit, 0x800).
const WAVE_GENERATORS_IN_VIEW: usize = 2048;
/// Waves' blend mode: `TRampAnimateFlat` with the particle record's default
/// (3, additive: `CEffectManagerImpl`'s record constructor, 0x102410d0).
const WAVE_BLEND: u32 = 3;

/// The map's shoreline wave generators and their timers.
struct Waves {
    generators: Arc<Vec<WaveGenerator>>,
    water: Option<f32>,
    /// Each generator's batch (its texture and ramp).
    batches: Vec<u32>,
    /// Each generator's last wave and current period (seconds on the
    /// particle clock); set on the first frame.
    timers: Vec<(f64, f64)>,
    /// The generators in view (refreshed every 5th frame).
    visible: Vec<u32>,
    frame: u32,
    rng: u64,
}

/// Graphics fidelity an emitter blueprint is shown at (`LowFidelity` /
/// `MedFidelity` / `HighFidelity`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    High = 2,
}

/// One particle as the shader reads it (FA's instanced vertex).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Record {
    /// Birth position; initial rotation (radians).
    pos_rot: [f32; 4],
    /// Velocity (ogrids/tick); rotation rate (radians/tick).
    vel_rate: [f32; 4],
    /// Acceleration; half-size at birth.
    accel_size: [f32; 4],
    /// Birth tick; lifetime; frame rate (frames/tick); half-size growth per tick.
    time: [f32; 4],
    /// Frame width; strip v offset; strip height; ramp row.
    tex: [f32; 4],
    /// Drag k, 1/k, 1/k^2; flags (bit cast).
    drag_flags: [f32; 4],
}

const RECORD_SIZE: u64 = std::mem::size_of::<Record>() as u64;

/// Ribbon vertex (beams).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
    ramp: [f32; 2],
    color: [f32; 4],
}

/// Camera axes and the clock (`ParticleUniforms`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    /// x: the clock (sim ticks). The billboards face the drawing view's
    /// camera from the scene's globals.
    time: [f32; 4],
}

/// What a batch draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// Emitter particles (`TRamp*`).
    Particle,
    /// Light particles (`TLight`): flat, no depth test, glow.
    Light,
    /// Trail ribbons (`TPolyTrail`).
    Trail,
    /// Beam ribbons (`TBeam`): glow.
    Beam,
}

/// (sort order, kind, blend, texture, ramp) a draw call needs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BatchKey {
    /// `SortOrder` x 100, rounded.
    sort: i32,
    kind: Kind,
    blend: u32,
    texture: String,
    ramp: String,
}

impl BatchKey {
    /// The bind group it draws with: its textures, and whether the texture
    /// wraps along v too. FA's `BeamPS` and `TrailPS` sample
    /// `ParticleSampler0Wrap` (wrap both ways), so a beam's `VShift` /
    /// `RepeatRate` scroll repeats the texture along the beam; emitter
    /// particles' `ParticleSampler0` clamps v (clamped, the Seraphim
    /// build beams scrolled off their texture onto its black edge row).
    fn group(&self) -> GroupKey {
        (self.texture.clone(), self.ramp.clone(), matches!(self.kind, Kind::Beam | Kind::Trail))
    }
}

/// A bind group's (texture, ramp, wraps along v).
type GroupKey = (String, String, bool);

/// A run of records in the pool.
struct Segment {
    /// Pool slot (records `slot * SEGMENT ..`).
    slot: u32,
    len: u32,
    /// When the last of its particles dies (clock ticks).
    end: f32,
}

struct Batch {
    key: BatchKey,
    segments: Vec<Segment>,
    /// Its run in the index list (`Particles::index_list`): one draw.
    first: u32,
    count: u32,
    /// Records emitted this frame, not uploaded yet, with when they die.
    pending: Vec<(Record, f32)>,
}

/// An effect's render-side state (moho's `CEfxEmitter` / `CEfxTrailEmitter`
/// fields the shader doesn't hold), kept in a vector aligned with the
/// latest snapshot's effects (id order), so a frame walks both together
/// with no lookups and the emission runs in parallel over chunks.
#[derive(Clone, Copy)]
struct State {
    /// The last sim tick emitted for (`created - 1` at first).
    tick: i64,
    /// Emit-rate accumulator (particles owed).
    acc: f32,
    rng: u64,
    /// Ticks skipped off screen, still to catch up.
    missed: u32,
    /// `CreateIfVisible` failed: never emits.
    dead: bool,
    /// Lights: already spawned.
    spawned: bool,
    /// Unattached effects: where they started (posed bone), kept.
    anchor: Option<(Vec3, Quat)>,
    /// Trails: the last segment's direction (the next one's leading edge)
    /// and the distance along the trail so far (texture v).
    dir: Option<Vec3>,
    dist: f32,
    /// Trails: where the last segment ended and when (ticks the render
    /// thread skipped, when snapshots arrive in a burst, are bridged from
    /// there; moho replays the entity's position history).
    last_end: Option<(Vec3, f32)>,
}

enum Slot<T> {
    Loading,
    Ready(T),
    Failed,
}

enum Done {
    Blueprint(u32, Option<EffectBp>),
    /// Uploaded by the loader.
    Texture(String, Option<wgpu::TextureView>),
}

/// An effect's place this frame (see `Particles::update`).
struct Placed {
    /// A beam's far end, interpolated (NaN: along the anchor's facing).
    end: Vec3,
    /// Attached and off screen by the cheap test.
    offscreen: bool,
    /// Attached and on screen: on its posed bone.
    at: Option<(Vec3, Quat)>,
    /// How far the sim moved the anchor over the last tick (attached).
    delta: Vec3,
}

pub struct Particles {
    assets: Vec<Option<String>>,
    /// By asset index.
    blueprints: Vec<Option<Slot<Arc<EffectBp>>>>,
    /// The batch an asset's emitter / trail blueprint draws in, by asset
    /// index (`BATCH_UNRESOLVED` until its blueprint is in).
    asset_batch: Vec<u32>,
    textures: BTreeMap<String, Slot<wgpu::TextureView>>,
    vfs: Option<Arc<RwLock<Vfs>>>,
    /// For the loaders, which upload their textures themselves.
    gpu: (wgpu::Device, wgpu::Queue),
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    /// Per-effect state, aligned with the latest snapshot's effects
    /// (`state_ids` are their ids), rebuilt by a merge each new tick.
    states: Vec<State>,
    state_ids: Vec<u32>,
    batches: Vec<Batch>,
    batch_index: BTreeMap<BatchKey, u32>,
    groups: BTreeMap<GroupKey, wgpu::BindGroup>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    wrap_sampler: wgpu::Sampler,
    ramp_sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    /// Per blend mode: particles, trails (ribbons), beams (ribbons, glow).
    particle_pipelines: Vec<wgpu::RenderPipeline>,
    trail_pipelines: Vec<wgpu::RenderPipeline>,
    beam_pipelines: Vec<wgpu::RenderPipeline>,
    light_pipeline: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    background_sampler: wgpu::Sampler,
    background: wgpu::TextureView,
    frame_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    /// The record pool and its free slots.
    records: wgpu::Buffer,
    /// Instance -> record slot, every live batch's records in draw order
    /// (`index_list` uploaded), so a batch is one draw whatever segments
    /// it holds: a big battle's 250k particles were a thousand draws, and
    /// wgpu's command processing is paid per draw. Rebuilt when the pool
    /// changes (a new tick's records, retired segments).
    indices: wgpu::Buffer,
    index_list: Vec<u32>,
    /// The map's shoreline wave generators (moho's `WaveSystem`).
    waves: Option<Waves>,
    pool_dirty: bool,
    slots: u32,
    free: Vec<u32>,
    /// This frame's ribbon vertices and draws (batch, range).
    vertices: Vec<Vertex>,
    ribbon_draws: Vec<(usize, std::ops::Range<u32>)>,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    /// Batches with records, in draw order: under the water, above it,
    /// lights.
    underwater_order: Vec<usize>,
    draw_order: Vec<usize>,
    light_order: Vec<usize>,
    /// The clock at the last update.
    time: f32,
    /// Effect blueprints / textures that failed to load.
    failures: Vec<String>,
    /// Warned that the pool is full / the vertices went past the buffer limit.
    over_cap: bool,
    over_budget: bool,
    /// Updates so far (stamps the emitter / trail state each one touches).
    frame: u64,
    /// What this frame's views see: what's off screen in all of them
    /// neither emits nor builds geometry.
    view: Sight,
    /// The sim tick of the last update: emitters and trails only work on
    /// frames that bring a new one.
    last_tick: i64,
    /// An emitter deferred part of its catch-up to the next frame.
    catchup_pending: bool,
    /// Decided by `begin` for `finish`: anything to emit this frame, and
    /// whether the frame brought a new sim tick.
    work: bool,
    new_tick: bool,
    /// Effects from snapshots the render thread never prepared (two or
    /// more taken in one slow frame) that were gone again by the next
    /// one, such as a one-tick impact flash: emitted backdated once their
    /// blueprint is in. Each with its curve overrides and a fresh state.
    missed: Vec<(EffectView, Vec<EffectCurve>, State)>,
    /// Draw calls this frame (one per segment drawn; the status bar).
    pub draws: std::cell::Cell<u32>,
    /// Graphics fidelity emitters are filtered by.
    pub fidelity: Fidelity,
    /// Where each effect emitted from this frame (render-state tests).
    pub anchors: BTreeMap<u32, (Vec3, Quat)>,
    /// Trail segments laid this frame, by effect id: (from, to)
    /// (render-state tests, with `record_anchors`).
    pub trails_laid: Vec<(u32, Vec3, Vec3)>,
    /// Record `anchors` (render tests) and draw everything (no culling).
    pub record_anchors: bool,
}

const SHADER: &str = include_str!(concat!(env!("OUT_DIR"), "/particles.wgsl"));

/// Each blend mode's name (GPU labels), by [`blend_state`] index.
const BLEND_NAMES: [&str; BLENDS] = ["alpha", "modulate inverse", "modulate 2x inverse", "additive", "premultiplied", "add", "refract"];

fn blend_state(mode: usize) -> wgpu::BlendState {
    use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
    let c = |src, dst| C { src_factor: src, dst_factor: dst, operation: O::Add };
    let keep_alpha = c(F::Zero, F::One);
    match mode {
        // Modulate inverse: dst * (1 - src).
        1 => wgpu::BlendState { color: c(F::Zero, F::OneMinusSrc), alpha: keep_alpha },
        // Modulate 2x inverse: src * (1 - dst) + dst * (1 - src).
        2 => wgpu::BlendState { color: c(F::OneMinusDst, F::OneMinusSrc), alpha: keep_alpha },
        3 => wgpu::BlendState { color: c(F::SrcAlpha, F::One), alpha: keep_alpha },
        4 => wgpu::BlendState { color: c(F::One, F::OneMinusSrcAlpha), alpha: keep_alpha },
        5 => wgpu::BlendState { color: c(F::One, F::One), alpha: keep_alpha },
        _ => wgpu::BlendState { color: c(F::SrcAlpha, F::OneMinusSrcAlpha), alpha: keep_alpha },
    }
}

/// `Write_RGBA`: the glow (FA's frame alpha) blends with the colour's
/// factors (D3D9 blended alpha like colour).
fn glow_blend(mode: usize) -> wgpu::BlendState {
    let b = blend_state(mode);
    wgpu::BlendState { color: b.color, alpha: b.color }
}

/// Uniform random in [0, 1) (render-only RNG, not the sim's; moho's is a
/// global Mersenne Twister).
fn rand(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 40) as f32 / (1u64 << 24) as f32
}

/// A curve's value at cycle fraction `t`, with its random spread, unless
/// Lua overrode it.
fn curve(bp: &EmitterBp, overrides: &[EffectCurve], id: CurveId, t: f32, rng: &mut u64) -> f32 {
    let (v, r) = match overrides.iter().rev().find(|c| c.curve == id as u8) {
        Some(o) => (o.value, o.range),
        None => bp.curve(id).sample(t),
    };
    v + (rand(rng) - 0.5) * r
}

/// What the world views see this frame: each one's frustum (`n . p + d >= 0`
/// inside; none: everything is in view) and LOD plane. The first view's
/// camera faces the ribbons.
#[derive(Clone, Default)]
pub struct Sight {
    frustums: Vec<[glam::Vec4; 6]>,
    lods: Vec<LodPlane>,
}

impl Sight {
    /// Every view's frustum and LOD plane (`frustums`: false sees
    /// everything, LOD aside).
    pub fn new(views: &[([glam::Vec4; 6], LodPlane)], frustums: bool) -> Self {
        Self { frustums: if frustums { views.iter().map(|v| v.0).collect() } else { Vec::new() }, lods: views.iter().map(|v| v.1).collect() }
    }

    /// The LOD metric: the nearest view's.
    fn metric(&self, p: Vec3) -> f32 {
        self.lods.iter().map(|l| l.metric(p)).fold(f32::INFINITY, f32::min)
    }

    /// The first view's LOD plane (its camera).
    fn first(&self) -> LodPlane {
        self.lods.first().copied().unwrap_or(LodPlane { eye: Vec3::ZERO, forward: Vec3::NEG_Y, scale: 1.0 })
    }
}

/// Whether a sphere of `radius` at `p` touches a view (no frustums: always).
fn visible(view: &Sight, p: Vec3, radius: f32) -> bool {
    view.frustums.is_empty() || view.frustums.iter().any(|planes| planes.iter().all(|pl| pl.truncate().dot(p) + pl.w >= -radius))
}

/// Moho's emitter visibility: in the frustum and, with a cutoff, within it
/// of the camera plane.
fn emitter_visible(view: &Sight, p: Vec3, lod_cutoff: f32) -> bool {
    visible(view, p, EMITTER_MARGIN) && (lod_cutoff <= 0.0 || view.metric(p) <= lod_cutoff)
}

fn new_state(e: &EffectView) -> State {
    State { tick: e.created as i64 - 1, acc: 0.0, rng: (e.id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, missed: 0, dead: false, spawned: false, anchor: None, dir: None, dist: 0.0, last_end: None }
}

/// Carried effects older than this (ticks) are dropped unemitted: their
/// particles would be dead (moho catches up at most 24 ticks).
const MISSED_MAX_AGE: i64 = 24;

/// `asset_batch` entry: not resolved yet.
const BATCH_UNRESOLVED: u32 = u32::MAX;
/// `asset_batch` entry: nothing to draw (blend modes past refract, beams).
const BATCH_NONE: u32 = u32::MAX - 1;

fn vertex_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particles: beam vertices"),
        size: (capacity * std::mem::size_of::<Vertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn record_buffer(device: &wgpu::Device, slots: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particles: records"),
        size: slots as u64 * SEGMENT as u64 * RECORD_SIZE,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn frame_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView, sampler: &wgpu::Sampler, uniforms: &wgpu::Buffer, records: &wgpu::Buffer, indices: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("particles: frame"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: records.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: indices.as_entire_binding() },
        ],
    })
}

/// The instance -> record index list's buffer for a pool of `slots`
/// segments (one entry per record at most).
fn index_buffer(device: &wgpu::Device, slots: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some("particles: indices"), size: slots as u64 * SEGMENT as u64 * 4, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

/// Blend modes' draw order within a sort order: modulate, alpha,
/// premultiplied, then additive on top.
fn blend_order(blend: u32) -> u8 {
    match blend {
        1 | 2 => 0,
        0 => 1,
        4 => 2,
        _ => 3,
    }
}

/// A camera-facing ribbon through `points` of `width` (full); `at(k, d)`
/// gives (uv at point `k`, distance `d` along it, ramp uv, colour); u runs
/// across, v along via `at`.
fn strip(points: &[Vec3], width: f32, eye: Vec3, at: impl Fn(usize, f32) -> ([f32; 2], [f32; 2], [f32; 4])) -> Vec<Vertex> {
    let mut out = Vec::with_capacity((points.len() - 1) * 6);
    let mut d = 0.0;
    let side = |k: usize| {
        let dir = if k + 1 < points.len() { points[k + 1] - points[k] } else { points[k] - points[k - 1] };
        (points[k] - eye).cross(dir).normalize_or(Vec3::X) * (width * 0.5)
    };
    for k in 0..points.len() - 1 {
        let seg = points[k].distance(points[k + 1]);
        let (a, b) = (side(k), side(k + 1));
        let (ua, ra, ca) = at(k, d);
        let (ub, rb, cb) = at(k + 1, d + seg);
        let v = |p: Vec3, uv: [f32; 2], ramp: [f32; 2], color: [f32; 4]| Vertex { position: p.to_array(), uv, ramp, color };
        // Texture u across the ribbon, v along (FA's beams: u across, v along).
        let l0 = v(points[k] - a, [0.0, ua[1] + ua[0]], ra, ca);
        let r0 = v(points[k] + a, [1.0, ua[1] + ua[0]], ra, ca);
        let l1 = v(points[k + 1] - b, [0.0, ub[1] + ub[0]], rb, cb);
        let r1 = v(points[k + 1] + b, [1.0, ub[1] + ub[0]], rb, cb);
        out.extend_from_slice(&[l0, r0, r1, l0, r1, l1]);
        d += seg;
    }
    out
}

/// What one emit step needs besides the emitter's state.
struct EmitContext<'a> {
    e: &'a EffectView,
    bp: &'a EmitterBp,
    overrides: &'a [EffectCurve],
    /// Where the emitter is at the start and end of the tick (births
    /// interpolate between the two, as moho lerps toward the entity's
    /// pending transform).
    from: (Vec3, Quat),
    to: (Vec3, Quat),
    /// The tick's bounds on the clock (births spread over it).
    time_from: f32,
    time_to: f32,
    water: Option<f32>,
    ground: &'a (dyn Fn(f32, f32) -> f32 + Sync),
    flags: u32,
    repeat: f32,
}

impl Particles {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, globals: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> Self {
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particles: textures layout"),
            entries: &[
                tex(0),
                tex(1),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("particles: shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particles: frame layout"),
            entries: &[
                tex(0),
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("particles: pipeline layout"), bind_group_layouts: &[Some(globals), Some(&layout), Some(&frame_layout)], immediate_size: 0 });
        let ribbon_layout = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4],
        }];
        let pipeline = |label: &str, vs: &str, buffers: &[wgpu::VertexBufferLayout], fs: &str, blend: wgpu::BlendState, glow: super::globals::GlowWrite, depth_test: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), targets: &super::globals::scene_targets(format, Some(blend), glow), compilation_options: Default::default() }),
                // FA: `Rasterizer_Cull_None`, `Depth_Enable_Less_Write_None`
                // (lights: `Depth_Disable_Write_None`).
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(if depth_test { wgpu::CompareFunction::Less } else { wgpu::CompareFunction::Always }),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        use super::globals::GlowWrite;
        // Particle techniques write colour only (`Write_RGB`).
        let particle_pipelines = (0..BLENDS).map(|m| pipeline(&format!("particles: sprites, {}", BLEND_NAMES[m]), "vs_particle", &[], if m == REFRACT as usize { "fs_refract" } else { "fs" }, blend_state(m), GlowWrite::Keep, true)).collect();
        let trail_pipelines = (0..BLENDS).map(|m| pipeline(&format!("particles: trails, {}", BLEND_NAMES[m]), "vs_trail", &[], if m == REFRACT as usize { "fs_refract" } else { "fs" }, blend_state(m), GlowWrite::Keep, true)).collect();
        // Beams and lights write RGBA: their alpha goes to the glow.
        let beam_pipelines = (0..BLENDS).map(|m| pipeline(&format!("particles: beams, {}", BLEND_NAMES[m]), "vs_ribbon", &ribbon_layout, "fs_glow", blend_state(m), GlowWrite::Blend(glow_blend(m)), true)).collect();
        let light_pipeline = pipeline("particles: lights", "vs_particle", &[], "fs_glow", blend_state(3), GlowWrite::Blend(glow_blend(3)), false);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let background_sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("particles: refraction background sampler"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let white = super::models::solid(device, queue, "particles: white", [255; 4], wgpu::TextureFormat::Rgba8Unorm);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("particles: uniforms"), size: std::mem::size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let slots = 64;
        let records = record_buffer(device, slots);
        let indices = index_buffer(device, slots);
        let frame_group = frame_group(device, &frame_layout, &white, &background_sampler, &uniforms, &records, &indices);
        let vertex_capacity = 4096;
        Self {
            assets: Vec::new(),
            blueprints: Vec::new(),
            asset_batch: Vec::new(),
            textures: BTreeMap::new(),
            vfs: None,
            gpu: (device.clone(), queue.clone()),
            done_tx,
            done_rx,
            states: Vec::new(),
            state_ids: Vec::new(),
            batches: Vec::new(),
            batch_index: BTreeMap::new(),
            groups: BTreeMap::new(),
            layout,
            // Ramps clamp. Textures wrap along u (animation frames run past
            // the last; beams scroll) and clamp along v (strips), as FA's
            // `ParticleSampler0`.
            ramp_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("particles: ramp sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("particles: sampler"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            // Beams and trails: FA's `ParticleSampler0Wrap`, wrapping both ways.
            wrap_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("particles: wrapping sampler (beams, trails)"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            white: white.clone(),
            particle_pipelines,
            trail_pipelines,
            beam_pipelines,
            light_pipeline,
            frame_layout,
            background_sampler,
            background: white,
            frame_group,
            uniforms,
            records,
            indices,
            index_list: Vec::new(),
            waves: None,
            pool_dirty: false,
            slots,
            free: (0..slots).rev().collect(),
            vertices: Vec::new(),
            ribbon_draws: Vec::new(),
            vertex_buffer: vertex_buffer(device, vertex_capacity),
            vertex_capacity,
            underwater_order: Vec::new(),
            draw_order: Vec::new(),
            light_order: Vec::new(),
            time: 0.0,
            failures: Vec::new(),
            over_cap: false,
            over_budget: false,
            frame: 0,
            view: Sight::default(),
            last_tick: -1,
            catchup_pending: false,
            work: false,
            new_tick: false,
            missed: Vec::new(),
            draws: Default::default(),
            fidelity: Fidelity::High,
            anchors: BTreeMap::new(),
            trails_laid: Vec::new(),
            record_anchors: false,
        }
    }

    /// Takes newly named effect assets (and the VFS, once).
    pub fn receive(&mut self, assets: &mut Vec<(u32, String)>, vfs: Option<&Arc<RwLock<Vfs>>>) {
        if self.vfs.is_none() {
            self.vfs = vfs.cloned();
        }
        for (i, path) in assets.drain(..) {
            let i = i as usize;
            if self.assets.len() <= i {
                self.assets.resize(i + 1, None);
            }
            self.assets[i] = Some(path);
        }
    }

    /// The frame copy refracting particles read (the scene's, per resize).
    pub fn set_background(&mut self, device: &wgpu::Device, view: &wgpu::TextureView) {
        self.background = view.clone();
        self.frame_group = frame_group(device, &self.frame_layout, &self.background, &self.background_sampler, &self.uniforms, &self.records, &self.indices);
    }

    /// This frame's beam ribbons (render tests): each one's texture and
    /// whether its texture wraps along the beam.
    #[allow(dead_code)]
    pub fn beam_draws(&self) -> Vec<(String, bool)> {
        self.ribbon_draws.iter().map(|(b, _)| self.batches[*b].key.group()).map(|(texture, _, wrap)| (texture, wrap)).collect()
    }

    /// This frame draws refracting particles (the scene copies the frame
    /// for them first).
    pub fn refracts(&self) -> bool {
        self.draw_order.iter().chain(&self.underwater_order).any(|&b| self.batches[b].key.blend == REFRACT) || self.ribbon_draws.iter().any(|(b, _)| self.batches[*b].key.blend == REFRACT)
    }

    /// Records in the pool (live particles, give or take the ones that
    /// died early in a segment still in use).
    pub fn live(&self) -> usize {
        self.batches.iter().map(|b| b.segments.iter().map(|s| s.len as usize).sum::<usize>()).sum()
    }

    /// Something is still loading.
    #[allow(dead_code)]
    pub fn loading(&self) -> bool {
        self.blueprints.iter().any(|s| matches!(s, Some(Slot::Loading))) || self.textures.values().any(|s| matches!(s, Slot::Loading))
    }

    #[allow(dead_code)]
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// Files changed on disk (`--enable-disk-watch`; lowercase VFS paths):
    /// emitter, trail and beam blueprints and textures from them load again
    /// for the effects made from now on.
    pub fn reload(&mut self, changed: &[String]) {
        let hit = |p: &str| !p.is_empty() && changed.iter().any(|c| *c == rts_core_engine::engine::vfs::normalise(p));
        for (i, path) in self.assets.iter().enumerate() {
            if path.as_deref().is_some_and(hit) {
                if let Some(slot) = self.blueprints.get_mut(i) {
                    *slot = None;
                }
                if let Some(b) = self.asset_batch.get_mut(i) {
                    *b = BATCH_UNRESOLVED;
                }
            }
        }
        let textures: Vec<String> = self.textures.keys().filter(|p| hit(p)).cloned().collect();
        for path in &textures {
            self.textures.remove(path);
            self.request_texture(path);
        }
        if !textures.is_empty() {
            self.groups.retain(|(tex, ramp, _), _| !textures.contains(tex) && !textures.contains(ramp));
        }
    }

    fn request_texture(&mut self, path: &str) {
        if path.is_empty() || self.textures.contains_key(path) {
            return;
        }
        let Some(vfs) = self.vfs.clone() else { return };
        self.textures.insert(path.to_owned(), Slot::Loading);
        let (tx, path, gpu) = (self.done_tx.clone(), path.to_owned(), self.gpu.clone());
        rayon::spawn(move || {
            let view = super::texture_load::load(&gpu.0, &gpu.1, &vfs, &path, false, false, "particle");
            let _ = tx.send(Done::Texture(path, view));
        });
    }

    /// The blueprint of asset `i`, loading it on first request.
    fn blueprint(&mut self, i: u32) -> Option<Arc<EffectBp>> {
        if i == u32::MAX {
            return None;
        }
        if self.blueprints.len() <= i as usize {
            self.blueprints.resize_with(i as usize + 1, || None);
        }
        match &self.blueprints[i as usize] {
            Some(Slot::Ready(b)) => return Some(b.clone()),
            Some(_) => return None,
            None => {}
        }
        let (Some(vfs), Some(Some(path))) = (self.vfs.clone(), self.assets.get(i as usize).cloned()) else { return None };
        self.blueprints[i as usize] = Some(Slot::Loading);
        let tx = self.done_tx.clone();
        rayon::spawn(move || {
            let _s = rts_core_engine::prof_span!("load effect blueprint");
            let bp = vfs.read().ok().and_then(|v| v.read(&path).ok()).and_then(|b| EffectBp::parse(&String::from_utf8_lossy(&b)).ok());
            let _ = tx.send(Done::Blueprint(i, bp));
        });
        None
    }

    fn batch(&mut self, sort_order: f32, kind: Kind, blend: u32, texture: &str, ramp: &str) -> u32 {
        let key = BatchKey { sort: (sort_order * 100.0).round() as i32, kind, blend, texture: texture.to_owned(), ramp: ramp.to_owned() };
        if let Some(&b) = self.batch_index.get(&key) {
            return b;
        }
        let b = self.batches.len() as u32;
        self.batches.push(Batch { key: key.clone(), segments: Vec::new(), pending: Vec::new(), first: 0, count: 0 });
        self.batch_index.insert(key, b);
        b
    }

    /// Takes finished loads and uploads textures (render thread).
    fn collect(&mut self) {
        while let Ok(d) = self.done_rx.try_recv() {
            match d {
                Done::Blueprint(i, bp) => {
                    if bp.is_none() {
                        let path = self.assets.get(i as usize).cloned().flatten().unwrap_or_default();
                        self.failures.push(format!("effect blueprint {path}"));
                    }
                    if let Some(bp) = &bp {
                        let (tex, ramp) = match bp {
                            EffectBp::Emitter(e) => (e.texture.clone(), e.ramp.clone()),
                            EffectBp::Beam(b) => (b.texture.clone(), String::new()),
                            EffectBp::Trail(t) => (t.texture.clone(), t.ramp.clone()),
                        };
                        self.request_texture(&tex);
                        self.request_texture(&ramp);
                    }
                    if self.blueprints.len() <= i as usize {
                        self.blueprints.resize_with(i as usize + 1, || None);
                    }
                    self.blueprints[i as usize] = Some(bp.map_or(Slot::Failed, |b| Slot::Ready(Arc::new(b))));
                }
                Done::Texture(path, view) => {
                    if view.is_none() {
                        self.failures.push(format!("effect texture {path}"));
                    }
                    self.textures.insert(path, view.map_or(Slot::Failed, Slot::Ready));
                }
            }
        }
    }

    /// A batch's textures, if loaded (an empty ramp path = white; a
    /// texture that failed to load draws white too, so the effect shows).
    fn group(&mut self, device: &wgpu::Device, b: usize) -> Option<()> {
        let key = self.batches[b].key.clone();
        let pair = key.group();
        if self.groups.contains_key(&pair) {
            return Some(());
        }
        let tex = match self.textures.get(&key.texture)? {
            Slot::Ready(v) => v,
            Slot::Failed => &self.white,
            Slot::Loading => return None,
        };
        let ramp = if key.ramp.is_empty() {
            &self.white
        } else {
            match self.textures.get(&key.ramp)? {
                Slot::Ready(v) => v,
                Slot::Failed => &self.white,
                Slot::Loading => return None,
            }
        };
        let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("particles: {} + ramp {}{}", key.texture, key.ramp, if pair.2 { " (wrapped)" } else { "" })),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(tex) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(ramp) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(if pair.2 { &self.wrap_sampler } else { &self.sampler }) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.ramp_sampler) },
            ],
        });
        self.groups.insert(pair, g);
        Some(())
    }

    /// Advances to the latest snapshot (sim tick `tick`, drawn at clock
    /// `now` = tick - 1 + `alpha`): emits from the live effects (placed by
    /// interpolating with `prev`), retires dead particles, uploads the new
    /// ones and builds the ribbons for a camera at `eye` looking along
    /// `forward`. `water` is the map's water level and `ground` its terrain
    /// height (waterline snapping, over-water emission).
    /// Effects of a snapshot that was never prepared and are not in the
    /// next one: kept to emit backdated (`update`). Emitters and lights;
    /// a trail or beam that lived one tick has nothing to show.
    pub fn carry_missed(&mut self, views: &[EffectView], curves: &[EffectCurve]) {
        for e in views {
            if !matches!(e.kind, EffectViewKind::Emitter | EffectViewKind::Light) {
                continue;
            }
            let own = curves[e.curves_start as usize..(e.curves_start + e.curves_len) as usize].to_vec();
            let mut v = *e;
            v.curves_start = 0;
            v.curves_len = own.len() as u32;
            self.missed.push((v, own, new_state(e)));
        }
    }

    /// An effect gone from the snapshots before it emitted anything: its
    /// blueprint was still loading on its only tick, as a one-tick impact
    /// flash's is the first time it is used in a game. Kept (with the last
    /// snapshot's view of it) to emit backdated once the blueprint is in.
    fn carry_gone(&mut self, j: usize, prev: &[EffectView], prev_curves: &[EffectCurve]) {
        let st = self.states[j];
        let id = self.state_ids[j];
        let Ok(k) = prev.binary_search_by_key(&id, |e| e.id) else { return };
        let e = prev[k];
        let unemitted = match e.kind {
            EffectViewKind::Emitter => st.tick < e.created as i64 && !st.dead,
            EffectViewKind::Light => !st.spawned,
            _ => false,
        };
        if !unemitted {
            return;
        }
        tracing::debug!(id, created = e.created, "effect gone before it emitted: carried");
        let own = prev_curves[e.curves_start as usize..(e.curves_start + e.curves_len) as usize].to_vec();
        let mut v = e;
        v.curves_start = 0;
        v.curves_len = own.len() as u32;
        self.missed.push((v, own, st));
    }

    /// The draw batch of an effect's asset, resolved on first sight (the
    /// blueprint requested; unresolved until it is in).
    fn resolve_batch(&mut self, e: &EffectView) {
        let a = e.asset as usize;
        if e.asset == u32::MAX || matches!(e.kind, EffectViewKind::Light | EffectViewKind::Beam) {
            return;
        }
        if self.asset_batch.len() <= a {
            self.asset_batch.resize(a + 1, BATCH_UNRESOLVED);
        }
        if self.asset_batch[a] != BATCH_UNRESOLVED {
            return;
        }
        let Some(bp) = self.blueprint(e.asset) else { return };
        self.asset_batch[a] = match bp.as_ref() {
            EffectBp::Emitter(b) => match b.blend_mode {
                0..=4 => self.batch(b.sort_order, Kind::Particle, b.blend_mode, &b.texture, &b.ramp),
                5 => self.batch(b.sort_order, Kind::Particle, REFRACT, &b.texture, &b.ramp),
                _ => BATCH_NONE,
            },
            EffectBp::Trail(t) => self.batch(t.sort_order, Kind::Trail, t.blend_mode.min(4), &t.texture, &t.ramp),
            EffectBp::Beam(_) => BATCH_NONE,
        };
    }

    /// The step's bookkeeping, needing no posed bones (so it runs beside
    /// the pose pass): finished loads taken in, and on a new sim tick the
    /// per-effect states merged with this snapshot's effects and their
    /// assets resolved. `finish` places and emits.
    pub fn begin(&mut self, tick: u64, effects: &[EffectView], prev: &[EffectView], prev_curves: &[EffectCurve]) {
        let _s = rts_core_engine::prof_span!("particles begin");
        self.collect();
        self.frame += 1;
        let tick = tick as i64;
        // A new sim tick this frame: emitters and trails emit for it; on
        // other frames they have nothing to do (the GPU evaluates what
        // they emitted), unless a catch-up was deferred.
        let new_tick = tick > self.last_tick;
        self.last_tick = tick;
        self.new_tick = new_tick;
        self.work = new_tick || self.catchup_pending || self.record_anchors;
        self.catchup_pending = false;
        if !self.work {
            return;
        }
        // The states, aligned with this snapshot's effects: a merge of
        // the last ones (both in id order); new effects start fresh,
        // gone ones drop.
        if new_tick {
            let _s = rts_core_engine::prof_span!("particles states");
            let mut states = Vec::with_capacity(effects.len());
            let mut j = 0;
            for e in effects {
                while j < self.state_ids.len() && self.state_ids[j] < e.id {
                    self.carry_gone(j, prev, prev_curves);
                    j += 1;
                }
                if j < self.state_ids.len() && self.state_ids[j] == e.id {
                    states.push(self.states[j]);
                    j += 1;
                } else {
                    states.push(new_state(e));
                }
            }
            while j < self.state_ids.len() {
                self.carry_gone(j, prev, prev_curves);
                j += 1;
            }
            self.states = states;
            self.state_ids = effects.iter().map(|e| e.id).collect();
        }
        // Blueprints requested and batches resolved for assets seen for
        // the first time (serial: the loaders and the batch table).
        {
            let _s = rts_core_engine::prof_span!("particles assets");
            for e in effects {
                self.resolve_batch(e);
            }
            let missed = std::mem::take(&mut self.missed);
            for (e, _, _) in &missed {
                self.resolve_batch(e);
            }
            self.missed = missed;
        }
    }

    /// The step's placement and emission, with the posed bones to hand:
    /// every effect placed, the new tick's records emitted, lights and
    /// beams, the pool uploaded and the batches ordered.
    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        now: f32,
        effects: &[EffectView],
        prev: &[EffectView],
        curves: &[EffectCurve],
        alpha: f32,
        sight: &[([glam::Vec4; 6], LodPlane)],
        resolve: &(dyn Fn(u32, u32) -> Option<(Vec3, Quat)> + Sync),
        launches: &[(u32, [f32; 3])],
        water: Option<f32>,
        ground: &(dyn Fn(f32, f32) -> f32 + Sync),
        zoom: f32,
    ) {
        let _s = rts_core_engine::prof_span!("particles");
        self.view = Sight::new(sight, !self.record_anchors);
        self.time = now;
        let LodPlane { eye, forward, .. } = self.view.first();
        let tick = self.last_tick;
        let (work, new_tick) = (self.work, self.new_tick);
        let mut ribbons: Vec<(u32, Vec<Vertex>)> = Vec::new();
        self.anchors.clear();
        self.trails_laid.clear();
        let effects_span = rts_core_engine::prof_span!("particles effects");
        // Where every effect is, first, in parallel (pure: interpolation,
        // the off-screen test, and an attached one's posed bone, a matrix
        // product and decomposition each): ~200k effects in a big air battle.
        // The state they feed (emitters, trails, batches) is then updated
        // serially, in effect order.
        let view = self.view.clone();
        let lerp = |a: [f32; 3], b: [f32; 3]| Vec3::from_array(a).lerp(Vec3::from_array(b), alpha);
        let prev_of = |e: &EffectView| prev.binary_search_by_key(&e.id, |x| x.id).ok().map(|i| &prev[i]);
        // On the drawn (posed, interpolated) bone when the entity is on
        // screen, else the sim's rest-pose anchor.
        let fallback = |e: &EffectView, p: Option<&EffectView>| (p.map_or(Vec3::from_array(e.position), |p| lerp(p.position, e.position)), Quat::from_array(e.rotation).normalize());
        let posed = |e: &EffectView| (e.entity != u32::MAX).then(|| resolve(e.entity, e.bone)).flatten().map(|(t, r)| (t + r * Vec3::from_array(e.offset), r));
        let place = |e: &EffectView| -> Placed {
            let p = prev_of(e);
            let end = p.map_or(Vec3::from_array(e.end), |p| lerp(p.end, e.end));
            let rough = p.map_or(Vec3::from_array(e.position), |p| lerp(p.position, e.position));
            // Off screen (judged on the sim's rest-pose position, before any
            // posing): the posed bone isn't needed.
            let offscreen = e.kind != EffectViewKind::Light && e.attached && {
                let reach = if e.kind == EffectViewKind::Beam { if end.is_nan() { EMITTER_MARGIN } else { rough.distance(end) + EMITTER_MARGIN } } else { OFFSCREEN_MARGIN };
                !visible(&view, rough, reach)
            };
            let at = (e.attached && !offscreen).then(|| posed(e).unwrap_or_else(|| fallback(e, p)));
            // A new effect on a projectile launched this tick: the sim
            // already flew it a tick, from its launch point (moho's motion
            // stage runs after the script stage that fired it). Without
            // that tick's move its trail would start a tick of flight in
            // front of the muzzle.
            let launched = || launches.binary_search_by_key(&e.entity, |l| l.0).ok().map(|i| Vec3::from_array(e.position) - Vec3::from_array(launches[i].1));
            let delta = if e.attached { p.map_or_else(|| launched().unwrap_or(Vec3::ZERO), |p| Vec3::from_array(e.position) - Vec3::from_array(p.position)) } else { Vec3::ZERO };
            Placed { end, offscreen, at, delta }
        };
        if work {
            // Every effect's place, in parallel.
            let placed: Vec<Placed> = {
                use rayon::prelude::*;
                let _s = rts_core_engine::prof_span!("particles place");
                if effects.len() >= PLACE_PARALLEL_MIN {
                    effects.par_iter().with_min_len(512).map_init(|| rts_core_engine::prof_span!("particles place (worker)"), |_, e| place(e)).collect()
                } else {
                    effects.iter().map(place).collect()
                }
            };
            // Emission, in parallel over chunks of effects (each chunk its
            // own list of records, merged in effect order below): emitters
            // and trails for the new tick, plus any deferred catch-up.
            let budget = std::sync::atomic::AtomicU32::new(CATCHUP_BUDGET);
            let deferred = std::sync::atomic::AtomicBool::new(false);
            let fidelity = self.fidelity as usize;
            let view = self.view.clone();
            // The tick's own window on the render clock, [tick - 1, tick]:
            // not the clock's floor, which is the tick itself once the
            // clock has run to the end of a late interval (then the tick's
            // births and trail segment would be stamped a tick late, and
            // the next tick's on top of them).
            let shown = (tick - 1) as f32;
            let record = self.record_anchors;
            let Self { states, blueprints, asset_batch, .. } = self;
            let position_of = |e: &EffectView, pl: &Placed, st: &mut State| -> (Vec3, Quat) {
                match pl.at {
                    Some(at) => at,
                    None if e.attached => fallback(e, prev_of(e)),
                    None => *st.anchor.get_or_insert_with(|| posed(e).unwrap_or_else(|| fallback(e, prev_of(e)))),
                }
            };
            let chunk_work = |effects: &[EffectView], placed: &[Placed], states: &mut [State], anchors: &mut Vec<(u32, (Vec3, Quat))>, laid: &mut Vec<(u32, Vec3, Vec3)>| -> Vec<(u32, Record, f32)> {
                let mut out = Vec::new();
                for ((e, pl), st) in effects.iter().zip(placed).zip(states.iter_mut()) {
                    let (position, rotation) = position_of(e, pl, st);
                    if record {
                        anchors.push((e.id, (position, rotation)));
                    }
                    let batch = match asset_batch.get(e.asset as usize) {
                        Some(&b) if b != BATCH_UNRESOLVED && b != BATCH_NONE => b,
                        _ => continue,
                    };
                    let Some(Some(Slot::Ready(bp))) = blueprints.get(e.asset as usize) else { continue };
                    let overrides = &curves[e.curves_start as usize..(e.curves_start + e.curves_len) as usize];
                    match (e.kind, bp.as_ref()) {
                        (EffectViewKind::Emitter, EffectBp::Emitter(bp)) => {
                            if !bp.fidelity[fidelity] {
                                continue;
                            }
                            let seen = emitter_visible(&view, position, bp.lod_cutoff) && !(pl.offscreen && bp.emit_if_visible);
                            // Where the anchor was at the tick's start and
                            // will be at its end (the sim's displacement).
                            let from = (position - pl.delta * alpha, rotation);
                            let to = (position + pl.delta * (1.0 - alpha), rotation);
                            if emit_effect(e, bp, overrides, st, from, to, tick, shown, seen, water, ground, &budget, batch, &mut out) {
                                deferred.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        (EffectViewKind::Trail, EffectBp::Trail(t))
                            if new_tick && tick > st.tick => {
                                st.tick = tick;
                                let before = out.len();
                                emit_trail(e, t, st, pl, position, alpha, shown, &view, batch, &mut out);
                                if record {
                                    laid.extend(out[before..].iter().map(|(_, r, _)| (e.id, Vec3::new(r.pos_rot[0], r.pos_rot[1], r.pos_rot[2]), Vec3::new(r.vel_rate[0], r.vel_rate[1], r.vel_rate[2]))));
                                }
                            }
                        _ => {}
                    }
                }
                out
            };
            let emit_span = rts_core_engine::prof_span!("particles emit");
            let outputs: Vec<(Vec<(u32, Record, f32)>, Vec<(u32, (Vec3, Quat))>, Vec<(u32, Vec3, Vec3)>)> = {
                use rayon::prelude::*;
                let n = effects.len();
                let chunk = 1024;
                effects
                    .par_chunks(chunk)
                    .zip(placed.par_chunks(chunk))
                    .zip(states.par_chunks_mut(chunk))
                    .map_init(
                        || rts_core_engine::prof_span!("particles emit (worker)"),
                        |_, ((es, pls), sts)| {
                            let mut anchors = if record { Vec::with_capacity(es.len()) } else { Vec::new() };
                            let mut laid = Vec::new();
                            let out = chunk_work(es, pls, sts, &mut anchors, &mut laid);
                            (out, anchors, laid)
                        },
                    )
                    .collect::<Vec<_>>()
                    .into_iter()
                    .take(n.div_ceil(chunk).max(1))
                    .collect()
            };
            let _ = emit_span;
            self.catchup_pending = deferred.load(std::sync::atomic::Ordering::Relaxed);
            {
                let _s = rts_core_engine::prof_span!("particles merge");
                for (out, anchors, laid) in outputs {
                    self.trails_laid.extend(laid);
                    for (b, r, end) in out {
                        self.batches[b as usize].pending.push((r, end));
                    }
                    for (id, at) in anchors {
                        self.anchors.insert(id, at);
                    }
                }
            }
            // Effects carried past their snapshots (never prepared, or gone
            // before their blueprint was in): emitted backdated, where the
            // sim had them, once the blueprint is in (serial: few).
            if new_tick && !self.missed.is_empty() {
                let missed = std::mem::take(&mut self.missed);
                let mut out = Vec::new();
                for (e, own, mut st) in missed {
                    let age = tick - e.created as i64;
                    if age > MISSED_MAX_AGE {
                        continue;
                    }
                    let position = Vec3::from_array(e.position);
                    let rotation = Quat::from_array(e.rotation).normalize();
                    match e.kind {
                        EffectViewKind::Light => self.light_at(&e, &mut st, position, now - age as f32),
                        EffectViewKind::Emitter => {
                            let batch = match self.asset_batch.get(e.asset as usize) {
                                Some(&b) if b == BATCH_UNRESOLVED => {
                                    // Blueprint still loading: next tick.
                                    self.missed.push((e, own, st));
                                    continue;
                                }
                                Some(&b) if b != BATCH_NONE => b,
                                _ => continue,
                            };
                            let Some(Some(Slot::Ready(bp))) = self.blueprints.get(e.asset as usize) else { continue };
                            let EffectBp::Emitter(bp) = bp.as_ref() else { continue };
                            if !bp.fidelity[fidelity] {
                                continue;
                            }
                            let seen = emitter_visible(&view, position, bp.lod_cutoff) && visible(&view, position, OFFSCREEN_MARGIN);
                            if emit_effect(&e, bp, &own, &mut st, (position, rotation), (position, rotation), tick, shown, seen, water, ground, &budget, batch, &mut out) {
                                self.missed.push((e, own, st));
                            }
                        }
                        _ => {}
                    }
                }
                for (b, r, end) in out {
                    self.batches[b as usize].pending.push((r, end));
                }
            }
            // Lights spawn once (serial: their batch is per texture pair).
            if new_tick {
                for (i, e) in effects.iter().enumerate() {
                    if e.kind == EffectViewKind::Light && !self.states[i].spawned {
                        let (position, _) = position_of(e, &placed[i], &mut self.states[i]);
                        self.light(e, i, position, now);
                    }
                }
            }
            // Beams: ribbons every frame they're on screen (few).
            for (i, e) in effects.iter().enumerate() {
                if e.kind == EffectViewKind::Beam && !placed[i].offscreen {
                    let (position, rotation) = position_of(e, &placed[i], &mut self.states[i]);
                    self.beam(e, position, rotation, placed[i].end, now, &mut ribbons);
                }
            }
        } else {
            // No new tick: only the beams need placing (their ribbons follow
            // the drawn bones every frame). The walk over every effect to
            // find them is what this costs in a big battle.
            let _s = rts_core_engine::prof_span!("particles beams");
            for (i, e) in effects.iter().enumerate() {
                if e.kind != EffectViewKind::Beam {
                    continue;
                }
                let pl = place(e);
                if pl.offscreen {
                    continue;
                }
                let st = self.states.get_mut(i);
                let (position, rotation) = match (pl.at, st) {
                    (Some(at), _) => at,
                    (None, Some(st)) if !e.attached => *st.anchor.get_or_insert_with(|| posed(e).unwrap_or_else(|| fallback(e, prev_of(e)))),
                    _ => fallback(e, prev_of(e)),
                };
                self.beam(e, position, rotation, pl.end, now, &mut ribbons);
            }
        }
        let _ = effects_span;
        // The clock for the shader.
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&Uniforms { time: [self.time, 0.0, 0.0, 0.0] }));
        self.emit_waves(now, eye, forward, zoom);
        self.retire_and_upload(device, queue, now);
        self.build_ribbons(device, queue, ribbons);
        self.order(device, queue);
    }

    /// The map's shoreline wave generators, once (`water`: the surface
    /// they lie on).
    pub fn set_waves(&mut self, generators: Arc<Vec<WaveGenerator>>, water: Option<f32>) {
        if self.waves.as_ref().is_some_and(|w| Arc::ptr_eq(&w.generators, &generators)) {
            return;
        }
        let mut paths: Vec<&str> = generators.iter().flat_map(|g| [g.texture.as_str(), g.ramp.as_str()]).filter(|p| !p.is_empty()).collect();
        paths.sort_unstable();
        paths.dedup();
        let paths: Vec<String> = paths.into_iter().map(str::to_string).collect();
        for p in &paths {
            self.request_texture(p);
        }
        let batches = generators.iter().map(|g| self.batch(0.0, Kind::Particle, WAVE_BLEND, &g.texture, &g.ramp)).collect();
        self.waves = Some(Waves { generators, water, batches, timers: Vec::new(), visible: Vec::new(), frame: 0, rng: 0x9e37_79b9_7f4a_7c15 });
    }

    /// Lays this frame's shoreline waves (moho's `WaveSystem::Update`,
    /// MohoEngine.dll 0x1044d0a0, and `WaveGenerator::Update`, 0x1044bcd0):
    /// nothing while the camera is zoomed out past [`WAVE_ZOOM`]; every 5th
    /// frame the generators in view within `efx_WaveCutoff` (400) of the
    /// camera, at most 2048 (file order here), are picked; each lays a wave
    /// once its period has passed and draws the next period. A wave is one
    /// flat particle (`TRampAnimateFlat`, additive) at the generator's
    /// position on the water, turned by its rotation, drifting at its
    /// velocity, living a random lifetime (ticks), growing from scale.0 to
    /// scale.1 (half-size), coloured over its life by the ramp.
    ///
    /// Moho times the periods on the wall clock (seconds); here the particle
    /// clock in ticks / 10, so waves follow game speed and pause with the
    /// particles they lay (a paused game would otherwise pile them up).
    fn emit_waves(&mut self, now: f32, eye: Vec3, forward: Vec3, zoom: f32) {
        let Some(w) = self.waves.as_mut() else { return };
        if zoom > WAVE_ZOOM || w.generators.is_empty() {
            return;
        }
        let seconds = now as f64 / 10.0;
        let mut rng = w.rng;
        let range = |r: (f32, f32), rng: &mut u64| r.0 + (r.1 - r.0) * rand(rng);
        if w.timers.is_empty() {
            // `Finalize`: the last wave a random part of the longest period
            // ago, the next period drawn.
            w.timers = w.generators.iter().map(|g| (seconds - (g.period.1.max(0.0) * rand(&mut rng)) as f64, range(g.period, &mut rng) as f64)).collect();
        }
        if w.frame % 5 == 0 {
            w.visible.clear();
            let view = self.view.clone();
            for (i, g) in w.generators.iter().enumerate() {
                let p = Vec3::from_array(g.position);
                let reach = g.scale.1.max(g.scale.0).abs() + 1.0;
                if (p - eye).dot(forward) - reach > WAVE_CUTOFF || !visible(&view, p, reach) {
                    continue;
                }
                w.visible.push(i as u32);
                if w.visible.len() == WAVE_GENERATORS_IN_VIEW {
                    break;
                }
            }
        }
        w.frame = w.frame.wrapping_add(1);
        let mut laid: Vec<(u32, Record, f32)> = Vec::new();
        for &i in &w.visible {
            let (g, timer) = (&w.generators[i as usize], &mut w.timers[i as usize]);
            if seconds - timer.0 <= timer.1 {
                continue;
            }
            let life = range(g.lifetime, &mut rng).max(1.0);
            let frame_rate = range(g.frame_rate, &mut rng).max(0.0);
            let frames = g.frame_count.max(1.0);
            let strips = g.strip_count.max(1.0);
            let strip = (rand(&mut rng) * strips).floor().min(strips - 1.0);
            let y = w.water.unwrap_or(g.position[1]);
            let (start, end) = g.scale;
            laid.push((
                w.batches[i as usize],
                Record {
                    pos_rot: [g.position[0], y, g.position[2], g.rotation],
                    vel_rate: [g.velocity[0], g.velocity[1], g.velocity[2], 0.0],
                    accel_size: [0.0, 0.0, 0.0, start],
                    time: [now, life, frame_rate, (end - start) / life],
                    tex: [1.0 / frames, strip / strips, 1.0 / strips, 0.0],
                    drag_flags: [0.0, 0.0, 0.0, f32::from_bits(FLAG_FLAT | FLAG_ANIMATED)],
                },
                now + life,
            ));
            *timer = (seconds, range(g.period, &mut rng) as f64);
        }
        w.rng = rng;
        for (batch, record, end) in laid {
            self.batches[batch as usize].pending.push((record, end));
        }
    }

    /// A beam's ribbon this frame (`BeamVS`: `Thickness` to each side,
    /// colour from `StartColor` to `EndColor`, the texture scrolled by
    /// `UShift` / `VShift`).
    #[allow(clippy::too_many_arguments)]
    fn beam(&mut self, e: &EffectView, position: Vec3, rotation: Quat, end: Vec3, now: f32, ribbons: &mut Vec<(u32, Vec<Vertex>)>) {
        let path = self.assets.get(e.asset as usize).cloned().flatten().unwrap_or_default();
        let (texture, blend, thickness, c0, c1, repeat, us, vs, length, cutoff) = if path.ends_with(".bp") {
            let Some(bp) = self.blueprint(e.asset) else { return };
            let EffectBp::Beam(b) = bp.as_ref() else { return };
            (b.texture.clone(), b.blend_mode, b.thickness, b.start_color, b.end_color, b.repeat_rate, b.u_shift, b.v_shift, b.length, b.lod_cutoff)
        } else {
            // Textured beam (no blueprint).
            self.request_texture(&path);
            (path, 3, 0.2, [1.0; 4], [1.0; 4], 0.0, 0.0, 0.0, 0.0, 0.0)
        };
        let thickness = if e.params[3].is_nan() { thickness } else { e.params[3] } * e.scale.max(0.01);
        // No far end from the sim: the blueprint's `Length` along the bone
        // (negative: backwards, exhausts).
        let end = if end.is_nan() { position + rotation * Vec3::Z * length * e.scale.max(0.01) } else { end };
        let len = position.distance(end);
        if len < 1e-3 || !visible(&self.view, (position + end) * 0.5, len * 0.5 + EMITTER_MARGIN) || (cutoff > 0.0 && self.view.metric(position) > cutoff) {
            return;
        }
        let v_len = if repeat > 0.0 { len * repeat } else { 1.0 };
        let pts = [position, end];
        let verts = strip(&pts, 2.0 * thickness, self.view.first().eye, |_, d| {
            let f = d / len;
            let c = [c0[0] + (c1[0] - c0[0]) * f, c0[1] + (c1[1] - c0[1]) * f, c0[2] + (c1[2] - c0[2]) * f, (c0[3] + (c1[3] - c0[3]) * f).max(0.0)];
            // Beam colours carry alpha in w. Additive beams (`One, One`,
            // `Write_RGBA`) keep it: it scales only what they add to the
            // glow, so a zero-alpha beam (the Othuy's lightning, whose
            // texture is opaque black around the bolt) blooms nothing, as in
            // FA. Other modes: zero alpha means opaque.
            let c = if blend != 3 && c0[3] == 0.0 && c1[3] == 0.0 { [c[0], c[1], c[2], 1.0] } else { c };
            ([us * now, f * v_len + vs * now], [0.5, 0.5], c)
        });
        let b = self.batch(0.0, Kind::Beam, if blend == 3 { BEAM_ADD } else { blend.min(4) }, &texture, "");
        ribbons.push((b, verts));
    }

    /// A light particle (`CreateLightParticle`): FA's `TLight`, a flat quad
    /// of the given size and lifetime with no depth test, drawn to the glow.
    fn light(&mut self, e: &EffectView, index: usize, position: Vec3, now: f32) {
        let mut st = self.states[index];
        self.light_at(e, &mut st, position, now);
        self.states[index] = st;
    }

    /// Spawns a light (once per effect) born at `born` on the render clock.
    fn light_at(&mut self, e: &EffectView, st: &mut State, position: Vec3, born: f32) {
        if st.spawned {
            return;
        }
        st.spawned = true;
        let now = born;
        // Lights name their textures briefly ('glow_02'): moho looks them up
        // in /textures/particles/.
        let full = |name: String| if name.is_empty() || name.contains('/') { name } else { format!("/textures/particles/{name}.dds") };
        let texture = full(self.assets.get(e.asset as usize).cloned().flatten().unwrap_or_default());
        let ramp = if e.ramp == u32::MAX { String::new() } else { full(self.assets.get(e.ramp as usize).cloned().flatten().unwrap_or_default()) };
        self.request_texture(&texture);
        self.request_texture(&ramp);
        let size = if e.params[4].is_nan() { 1.0 } else { e.params[4] };
        let life = if e.params[0].is_nan() { 5.0 } else { e.params[0].max(0.5) };
        let batch = self.batch(0.0, Kind::Light, 3, &texture, &ramp);
        self.batches[batch as usize].pending.push((
            Record {
                pos_rot: [position.x, position.y, position.z, 0.0],
                vel_rate: [0.0; 4],
                accel_size: [0.0, 0.0, 0.0, size],
                time: [now, life, 0.0, 0.0],
                tex: [1.0, 0.0, 1.0, 0.5],
                drag_flags: [0.0, 0.0, 0.0, f32::from_bits(FLAG_FLAT)],
            },
            now + life,
        ));
    }

    /// Frees the segments whose particles have all died and uploads this
    /// frame's new records into the pool.
    fn retire_and_upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, now: f32) {
        let _s = rts_core_engine::prof_span!("particles upload");
        for b in &mut self.batches {
            b.segments.retain(|s| {
                if s.end <= now {
                    self.free.push(s.slot);
                    self.pool_dirty = true;
                    false
                } else {
                    true
                }
            });
        }
        let needed: u32 = self.batches.iter().map(|b| b.pending.len() as u32).sum();
        if needed == 0 {
            return;
        }
        self.pool_dirty = true;
        // Room for the worst case (every batch opening a new segment).
        let open: u32 = self.batches.iter().filter(|b| !b.pending.is_empty()).count() as u32;
        let want = needed.div_ceil(SEGMENT) + open;
        if (self.free.len() as u32) < want {
            self.grow(device, queue, want - self.free.len() as u32);
        }
        for bi in 0..self.batches.len() {
            if self.batches[bi].pending.is_empty() {
                continue;
            }
            let pending = std::mem::take(&mut self.batches[bi].pending);
            let mut i = 0;
            while i < pending.len() {
                let seg = match self.batches[bi].segments.last_mut() {
                    Some(s) if s.len < SEGMENT => s,
                    _ => {
                        let Some(slot) = self.free.pop() else {
                            if !self.over_cap {
                                self.over_cap = true;
                                tracing::warn!(cap = MAX_PARTICLES, "particles: particle cap exceeded, discarding excess");
                            }
                            break;
                        };
                        self.batches[bi].segments.push(Segment { slot, len: 0, end: 0.0 });
                        self.batches[bi].segments.last_mut().expect("just pushed")
                    }
                };
                let room = (SEGMENT - seg.len) as usize;
                let chunk = &pending[i..(i + room).min(pending.len())];
                let offset = (seg.slot as u64 * SEGMENT as u64 + seg.len as u64) * RECORD_SIZE;
                let records: Vec<Record> = chunk.iter().map(|(r, _)| *r).collect();
                queue.write_buffer(&self.records, offset, bytemuck::cast_slice(&records));
                seg.len += chunk.len() as u32;
                seg.end = chunk.iter().fold(seg.end, |m, (_, end)| m.max(*end));
                i += chunk.len();
            }
        }
    }

    /// Grows the pool by at least `more` segments (doubling), keeping the
    /// records in it, up to the cap.
    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, more: u32) {
        let max_slots = MAX_PARTICLES / SEGMENT;
        if self.slots >= max_slots {
            return;
        }
        let slots = (self.slots * 2).max(self.slots + more).min(max_slots);
        let buffer = record_buffer(device, slots);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("particles: grow record pool") });
        encoder.copy_buffer_to_buffer(&self.records, 0, &buffer, 0, self.slots as u64 * SEGMENT as u64 * RECORD_SIZE);
        queue.submit(std::iter::once(encoder.finish()));
        self.free.extend((self.slots..slots).rev());
        self.slots = slots;
        self.records = buffer;
        self.indices = index_buffer(device, slots);
        self.frame_group = frame_group(device, &self.frame_layout, &self.background, &self.background_sampler, &self.uniforms, &self.records, &self.indices);
        tracing::debug!(records = slots * SEGMENT, "particles: pool grown");
    }

    /// This frame's ribbon geometry (beams, trails) into the vertex buffer.
    fn build_ribbons(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, ribbons: Vec<(u32, Vec<Vertex>)>) {
        let _s = rts_core_engine::prof_span!("particles ribbon upload");
        let mut per_batch: BTreeMap<u32, Vec<Vertex>> = BTreeMap::new();
        for (b, v) in ribbons {
            per_batch.entry(b).or_default().extend(v);
        }
        self.vertices.clear();
        self.ribbon_draws.clear();
        let mut idx: Vec<u32> = per_batch.keys().copied().collect();
        idx.sort_by_key(|&b| (self.batches[b as usize].key.kind, self.batches[b as usize].key.sort, blend_order(self.batches[b as usize].key.blend), b));
        for b in idx {
            if self.group(device, b as usize).is_none() {
                continue;
            }
            let start = self.vertices.len() as u32;
            self.vertices.extend_from_slice(&per_batch[&b]);
            self.ribbon_draws.push((b as usize, start..self.vertices.len() as u32));
        }
        // At most what one buffer may hold (wgpu's `max_buffer_size`, 256 MB
        // on Metal). Batches that don't fit whole are skipped this frame.
        let most = (device.limits().max_buffer_size as usize / std::mem::size_of::<Vertex>()).max(1);
        if self.vertices.len() > most {
            self.ribbon_draws.retain(|(_, r)| r.end as usize <= most);
            self.vertices.truncate(most);
            if !self.over_budget {
                self.over_budget = true;
                tracing::warn!(vertices = most, "particles: over the vertex buffer limit, skipping ribbons");
            }
        }
        if self.vertices.len() > self.vertex_capacity {
            self.vertex_capacity = self.vertices.len().next_power_of_two().min(most);
            self.vertex_buffer = vertex_buffer(device, self.vertex_capacity);
        }
        if !self.vertices.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        }
    }

    /// The batches with records, in draw order: by sort order, then blend
    /// (modulate, alpha, premultiplied, additive), split at the water
    /// surface; lights apart.
    fn order(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let _s = rts_core_engine::prof_span!("particles order");
        self.underwater_order.clear();
        self.draw_order.clear();
        self.light_order.clear();
        let mut idx: Vec<usize> = (0..self.batches.len()).filter(|&b| self.batches[b].segments.iter().any(|s| s.len > 0)).collect();
        idx.sort_by_key(|&b| (self.batches[b].key.sort, blend_order(self.batches[b].key.blend), b));
        for b in idx {
            if self.group(device, b).is_none() {
                continue;
            }
            let key = &self.batches[b].key;
            match key.kind {
                Kind::Light => self.light_order.push(b),
                Kind::Particle | Kind::Trail if (key.sort as f32) < WATER_SURFACE_SORT * 100.0 => self.underwater_order.push(b),
                Kind::Particle | Kind::Trail => self.draw_order.push(b),
                Kind::Beam => {}
            }
        }
        if self.pool_dirty {
            self.index_list.clear();
            for &b in self.underwater_order.iter().chain(&self.draw_order).chain(&self.light_order) {
                let batch = &mut self.batches[b];
                batch.first = self.index_list.len() as u32;
                for s in &batch.segments {
                    let base = s.slot * SEGMENT;
                    self.index_list.extend(base..base + s.len);
                }
                batch.count = self.index_list.len() as u32 - batch.first;
            }
            if !self.index_list.is_empty() {
                queue.write_buffer(&self.indices, 0, bytemuck::cast_slice(&self.index_list));
            }
            self.pool_dirty = false;
        }
    }

    fn draw_batches<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, order: &[usize]) {
        let mut current = (Kind::Beam, usize::MAX);
        for &b in order {
            let batch = &self.batches[b];
            let blend = batch.key.blend as usize;
            if blend >= BLENDS {
                continue;
            }
            let Some(g) = self.groups.get(&batch.key.group()) else { continue };
            if (batch.key.kind, blend) != current {
                pass.set_pipeline(match batch.key.kind {
                    Kind::Light => &self.light_pipeline,
                    Kind::Trail => &self.trail_pipelines[blend],
                    _ => &self.particle_pipelines[blend],
                });
                current = (batch.key.kind, blend);
            }
            pass.set_bind_group(1, g, &[]);
            if batch.count > 0 {
                pass.draw(0..6, batch.first..batch.first + batch.count);
                self.draws.set(self.draws.get() + 1);
            }
        }
    }

    /// Particles sorted below the water surface (torpedo wakes, bubbles):
    /// drawn before the water.
    pub fn draw_underwater<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.underwater_order.is_empty() {
            return;
        }
        pass.set_bind_group(2, &self.frame_group, &[]);
        self.draw_batches(pass, &self.underwater_order);
    }

    /// Draws this frame's effects (after opaque geometry and water): beams,
    /// then particles and trails in sort order.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.draw_order.is_empty() && self.ribbon_draws.is_empty() {
            return;
        }
        pass.set_bind_group(2, &self.frame_group, &[]);
        if !self.ribbon_draws.is_empty() {
            pass.timed_push("particles: beams");
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            let mut current = usize::MAX;
            for (b, range) in &self.ribbon_draws {
                let key = &self.batches[*b].key;
                let blend = key.blend as usize;
                if blend >= BLENDS {
                    continue;
                }
                if blend != current {
                    pass.set_pipeline(&self.beam_pipelines[blend]);
                    current = blend;
                }
                let Some(g) = self.groups.get(&key.group()) else { continue };
                pass.set_bind_group(1, g, &[]);
                pass.draw(range.clone(), 0..1);
            }
            pass.timed_pop();
        }
        self.draw_batches(pass, &self.draw_order);
    }

    /// Light particles: FA's light pass (flat, no depth test, glowing).
    pub fn draw_lights<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.light_order.is_empty() {
            return;
        }
        pass.set_bind_group(2, &self.frame_group, &[]);
        self.draw_batches(pass, &self.light_order);
    }

}

/// Emits from one emitter for the sim ticks since its last emit (moho's
/// `CEfxEmitter::Tick`): off screen with `EmitIfVisible` it counts the
/// ticks missed; back on screen it catches them up (bounded by its
/// particles' longest life and the frame's budget) with backdated births.
/// Returns whether part of a catch-up was deferred to the next frame.
#[allow(clippy::too_many_arguments)]
fn emit_effect(e: &EffectView, bp: &EmitterBp, overrides: &[EffectCurve], st: &mut State, from: (Vec3, Quat), to: (Vec3, Quat), tick: i64, shown: f32, seen: bool, water: Option<f32>, ground: &(dyn Fn(f32, f32) -> f32 + Sync), budget: &std::sync::atomic::AtomicU32, batch: u32, out: &mut Vec<(u32, Record, f32)>) -> bool {
    use std::sync::atomic::Ordering;
    let created = e.created as i64;
    // Moho destroys an emitter created off screen with `CreateIfVisible`.
    if st.tick < created && bp.create_if_visible && !seen {
        st.dead = true;
    }
    if st.dead || tick <= st.tick {
        return false;
    }
    if !seen && bp.emit_if_visible {
        // Off screen: count the ticks, keep nothing else (catch-up
        // particles spawn where the emitter is when it returns).
        st.missed = st.missed.saturating_add((tick - st.tick) as u32);
        st.tick = tick;
        return false;
    }
    let lifetime = if e.params[0].is_nan() { bp.lifetime } else { e.params[0] };
    let repeat = if e.params[1].is_nan() { bp.repeat_time } else { e.params[1] }.max(1e-3);
    // Ticks to emit for: the newest (births spread over the tick the clock
    // is in, between where the anchor was and will be), plus any missed or
    // skipped ones caught up (backdated, where the emitter is now), at
    // most its particles' longest life (older ones would be dead) and the
    // frame's budget.
    let longest = bp.longest_particle_life().max(1.0).ceil() as i64;
    let fresh_ticks = tick - st.tick;
    // Ticks the render thread skipped (several snapshots in one slow
    // frame) are always caught up: moho's emitters tick with the sim, so
    // it never skips one. `CatchupEmit` decides only about ticks missed
    // off screen.
    let catchup = ((if bp.catchup_emit { st.missed as i64 } else { 0 }) + (fresh_ticks - 1).max(0)).min(longest);
    st.missed = 0;
    let flags = {
        let mut f = 0;
        // AlignRotation wins; AlignToBone with Flat is a flat particle
        // facing the bone's heading; AlignToBone alone never moves.
        if bp.align_rotation {
            f |= FLAG_ALIGN;
        } else if bp.align_to_bone && !bp.flat {
            f |= FLAG_ALIGN | FLAG_NO_MOVE;
        } else if bp.flat {
            f |= FLAG_FLAT;
        }
        if bp.frame_count > 1 || bp.strip_count > 1 {
            f |= FLAG_ANIMATED;
        }
        f
    };
    let mut deferred = false;
    let mut records = Vec::new();
    // Caught-up ticks first (oldest first), then the newest.
    let first = tick - catchup;
    let mut t = first.max(created);
    while t <= tick {
        let count = t - created;
        if lifetime >= 0.0 && count as f32 >= lifetime {
            break;
        }
        let live = t == tick;
        let ago = (tick - t) as f32;
        let ctx = EmitContext { e, bp, overrides, from: if live { from } else { to }, to, time_from: shown - ago, time_to: shown - ago + 1.0, water, ground, flags, repeat };
        let n = emit_tick(&ctx, st, count as f32, 0.0, 1.0, &mut records);
        if !live {
            // The frame's catch-up budget, shared by every emitter.
            let had = budget.fetch_sub(n, Ordering::Relaxed);
            if had < n {
                // Over budget: the rest of the catch-up waits a frame.
                st.tick = t;
                st.missed = (tick - t) as u32;
                deferred = true;
                break;
            }
        }
        st.tick = t;
        t += 1;
    }
    out.extend(records.into_iter().map(|(r, end)| (batch, r, end)));
    deferred
}

/// A trail's segment for this sim tick (moho's `CEfxTrailEmitter`): from
/// where the bone was at the tick's start to where it will be at its end,
/// fading over `TrailLength` ticks from each end's own time. Off screen or
/// out of range, nothing (the trail starts afresh when it returns). Ticks
/// the render thread never showed (snapshots taken in a burst after a sim
/// hitch) get one straight segment from the last laid end to this one's
/// start, so the trail has no hole.
#[allow(clippy::too_many_arguments)]
fn emit_trail(e: &EffectView, t: &crate::engine::assets::effect_bp::TrailBp, st: &mut State, pl: &Placed, position: Vec3, alpha: f32, shown: f32, view: &Sight, batch: u32, out: &mut Vec<(u32, Record, f32)>) {
    if pl.offscreen || (t.lod_cutoff > 0.0 && view.metric(position) > t.lod_cutoff) || !visible(view, position, t.length.max(1.0) * 4.0 + EMITTER_MARGIN) {
        st.dir = None;
        st.last_end = None;
        return;
    }
    let p0 = position - pl.delta * alpha;
    let p1 = position + pl.delta * (1.0 - alpha);
    let length = t.length.max(0.1);
    let rate = t.texture_repeat_rate;
    let width = t.size * e.scale.max(0.01);
    // One segment from `a` (laid at `ta`) to `b` (laid at `tb`).
    let mut lay = |st: &mut State, a: Vec3, ta: f32, b: Vec3, tb: f32| {
        let step = b.distance(a);
        let dir = (b - a).normalize_or(st.dir.unwrap_or(Vec3::X));
        let prev = st.dir.unwrap_or(dir);
        let (d0, d1) = (st.dist, st.dist + step);
        st.dir = Some(dir);
        st.dist = d1;
        let record = Record {
            pos_rot: [a.x, a.y, a.z, ta],
            vel_rate: [b.x, b.y, b.z, tb],
            accel_size: [prev.x, prev.y, prev.z, d0 * rate],
            time: [dir.x, dir.y, dir.z, d1 * rate],
            tex: [width, length, t.v_shift, 0.0],
            drag_flags: [0.0; 4],
        };
        out.push((batch, record, tb + length));
    };
    if let Some((last, laid)) = st.last_end
        && laid < shown - 0.5 && last.distance_squared(p0) > 1e-6 {
        lay(st, last, laid, p0, shown);
    }
    lay(st, p0, shown, p1, shown + 1.0);
    st.last_end = Some((p1, shown + 1.0));
}

/// Emits one sim tick's particles (`CEfxEmitter::Emit`, MohoEngine.dll
/// 0x102426b0) for an emitter `count` ticks into its life: the emit-rate
/// accumulator gives the number; with `InterpolateEmission` the k-th of n
/// is born k/n of the way through the tick (its curves sampled there, its
/// position and birth interpolated between `f0` and `f1` of the span from
/// the last emit to now). Returns how many.
fn emit_tick(ctx: &EmitContext, st: &mut State, count: f32, f0: f32, f1: f32, out: &mut Vec<(Record, f32)>) -> u32 {
    let (bp, overrides, e) = (ctx.bp, ctx.overrides, ctx.e);
    let repeat = ctx.repeat;
    let cycle_at = |sub: f32| (count + sub).rem_euclid(repeat) / repeat;
    let mut rng = st.rng;
    st.acc += curve(bp, overrides, CurveId::EmitRate, cycle_at(0.0), &mut rng);
    let n = st.acc.floor();
    st.acc -= n;
    // (A negative rate owes nothing and leaves the fraction, as moho.)
    let n = n.clamp(0.0, 10_000.0) as u32;
    if n == 0 {
        st.rng = rng;
        return 0;
    }
    let scale = e.scale.max(0.0);
    let strips = bp.strip_count.max(1) as f32;
    let frames = bp.frame_count.max(1) as f32;
    let (p0, r0) = ctx.from;
    let (p1, r1) = ctx.to;
    for k in 0..n {
        let sub = if bp.interpolate_emission { k as f32 / n as f32 } else { 1.0 };
        let cycle = cycle_at(if bp.interpolate_emission { sub } else { 0.0 });
        let c = |id, rng: &mut u64| curve(bp, overrides, id, cycle, rng);
        let f = f0 + (f1 - f0) * sub;
        let position = p0.lerp(p1, f);
        let rotation = r0.slerp(r1, f).normalize();
        let birth = ctx.time_from + (ctx.time_to - ctx.time_from) * f;
        // Position curves in the bone's frame; the `Size` spread on the
        // world XZ plane (a random direction, distance uniform over
        // +-Size/2); direction and acceleration rotated by the bone only
        // with `LocalVelocity` / `LocalAcceleration`; gravity in world
        // space after.
        let local = Vec3::new(c(CurveId::XPos, &mut rng), c(CurveId::YPos, &mut rng), c(CurveId::ZPos, &mut rng));
        let size = c(CurveId::Size, &mut rng).max(0.0);
        let mut pos = position + rotation * local * scale;
        if size > 0.0 {
            let flat = Vec3::new(rand(&mut rng) - 0.5, 0.0, rand(&mut rng) - 0.5).normalize_or_zero();
            pos += flat * ((rand(&mut rng) - 0.5) * size * scale);
        }
        if bp.only_emit_on_water && ctx.water.is_none_or(|w| (ctx.ground)(pos.x, pos.z) > w) {
            continue;
        }
        if bp.snap_to_waterline {
            // Above the surface for batches sorted above it, below for the
            // rest (`efx_ParticleWaterSurface`).
            let w = ctx.water.unwrap_or(-10_000.0);
            pos.y = if bp.sort_order >= WATER_SURFACE_SORT { pos.y.max(w + WATER_OFFSET) } else { pos.y.min(w - WATER_OFFSET) };
        }
        let dir = Vec3::new(c(CurveId::XDir, &mut rng), c(CurveId::YDir, &mut rng), c(CurveId::ZDir, &mut rng)) * scale;
        let dir = if bp.local_velocity { rotation * dir } else { dir };
        let speed = c(CurveId::Velocity, &mut rng);
        let accel = Vec3::new(c(CurveId::XAccel, &mut rng), c(CurveId::YAccel, &mut rng), c(CurveId::ZAccel, &mut rng)) * scale;
        let mut accel = if bp.local_acceleration { rotation * accel } else { accel };
        if bp.gravity {
            accel.y += GRAVITY;
        }
        let bone_z = (rotation * Vec3::Z).normalize_or(Vec3::Y);
        // AlignToBone: the velocity becomes the bone's +Z (the quad lies
        // along it, never moving); with Flat, the initial rotation follows
        // the bone's heading instead.
        let align_to_bone = bp.align_to_bone && !bp.align_rotation;
        let velocity = if align_to_bone && !bp.flat { bone_z } else { dir * speed };
        let mut rot = c(CurveId::InitialRotation, &mut rng).to_radians();
        if align_to_bone && bp.flat {
            rot = (-bone_z.x).atan2(bone_z.z);
        }
        let life = c(CurveId::Lifetime, &mut rng).max(0.0);
        if life <= 0.0 {
            continue;
        }
        let start = c(CurveId::StartSize, &mut rng) * scale;
        let end = c(CurveId::EndSize, &mut rng) * scale;
        let strip = c(CurveId::TextureSelection, &mut rng).floor().max(0.0);
        let k_drag = c(CurveId::Resistance, &mut rng);
        let mut flags = ctx.flags;
        let drag = if bp.resistance && k_drag > 1e-6 {
            flags |= FLAG_DRAG;
            [k_drag, 1.0 / k_drag, 1.0 / (k_drag * k_drag)]
        } else {
            [0.0; 3]
        };
        out.push((
            Record {
                pos_rot: [pos.x, pos.y, pos.z, rot],
                vel_rate: [velocity.x, velocity.y, velocity.z, c(CurveId::RotationRate, &mut rng).to_radians()],
                accel_size: [accel.x, accel.y, accel.z, start],
                time: [birth, life, c(CurveId::FrameRate, &mut rng).max(0.0), (end - start) / life],
                tex: [1.0 / frames, strip / strips, 1.0 / strips, c(CurveId::RampSelection, &mut rng)],
                drag_flags: [drag[0], drag[1], drag[2], f32::from_bits(flags)],
            },
            birth + life,
        ));
    }
    st.rng = rng;
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect() -> EffectView {
        EffectView { id: 7, kind: EffectViewKind::Trail, asset: 0, ramp: u32::MAX, army: 1, entity: 3, bone: 0, attached: true, offset: [0.0; 3], position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], end: [f32::NAN; 3], scale: 1.0, created: 10, params: [f32::NAN; 5], curves_start: 0, curves_len: 0, ended: false }
    }

    fn trail() -> crate::engine::assets::effect_bp::TrailBp {
        crate::engine::assets::effect_bp::TrailBp { lifetime: -1.0, length: 2.0, size: 0.1, blend_mode: 0, texture: String::new(), ramp: String::new(), texture_repeat_rate: 0.0, lod_cutoff: 0.0, sort_order: 0.0, u_shift: 0.0, v_shift: 0.0 }
    }

    /// Lays the segment for sim tick `tick` of an entity moving +8 x per
    /// tick, drawn `alpha` into the tick.
    fn lay(st: &mut State, tick: i64, alpha: f32, out: &mut Vec<(u32, Record, f32)>) {
        let e = effect();
        let delta = Vec3::new(8.0, 0.0, 0.0);
        let at = Vec3::new(8.0 * (tick - 1) as f32, 5.0, 0.0) + delta * alpha;
        let pl = Placed { end: Vec3::NAN, offscreen: false, at: Some((at, Quat::IDENTITY)), delta };
        emit_trail(&e, &trail(), st, &pl, at, alpha, (tick - 1) as f32, &Sight::new(&[([glam::Vec4::ZERO; 6], LodPlane { eye: Vec3::ZERO, forward: Vec3::X, scale: 1.0 })], false), 0, out);
    }

    fn ends(r: &Record) -> (Vec3, f32, Vec3, f32) {
        (Vec3::new(r.pos_rot[0], r.pos_rot[1], r.pos_rot[2]), r.pos_rot[3], Vec3::new(r.vel_rate[0], r.vel_rate[1], r.vel_rate[2]), r.vel_rate[3])
    }

    /// Consecutive ticks: one segment each, chained in space and time
    /// whatever point of the tick the frame fell on.
    #[test]
    fn trail_segments_chain() {
        let mut st = new_state(&effect());
        let mut out = Vec::new();
        for (tick, alpha) in [(10, 0.1), (11, 0.9), (12, 1.0), (13, 0.4)] {
            lay(&mut st, tick, alpha, &mut out);
        }
        assert_eq!(out.len(), 4);
        for (k, (_, r, _)) in out.iter().enumerate() {
            let (a, ta, b, tb) = ends(r);
            assert_eq!((ta, tb), ((9 + k) as f32, (10 + k) as f32));
            assert!((a - Vec3::new(8.0 * (9 + k) as f32, 5.0, 0.0)).length() < 1e-4, "segment {k} starts at {a}");
            assert!((b - a - Vec3::new(8.0, 0.0, 0.0)).length() < 1e-4, "segment {k} spans one tick");
        }
    }

    /// A tick the render thread never showed (two snapshots in one frame)
    /// gets a bridging segment from the last laid end, with its own time.
    #[test]
    fn trail_bridges_a_skipped_tick() {
        let mut st = new_state(&effect());
        let mut out = Vec::new();
        lay(&mut st, 10, 0.2, &mut out);
        lay(&mut st, 11, 0.2, &mut out);
        lay(&mut st, 13, 0.2, &mut out);
        assert_eq!(out.len(), 4, "tick 12 is bridged");
        let (_, _, p11_end, t11_end) = ends(&out[1].1);
        let (a, ta, b, tb) = ends(&out[2].1);
        let (p13_start, t13_start, _, _) = ends(&out[3].1);
        assert_eq!((ta, tb), (11.0, 12.0));
        assert_eq!(t11_end, 11.0);
        assert_eq!(t13_start, 12.0);
        assert!((a - p11_end).length() < 1e-4, "bridge starts where tick 11 ended");
        assert!((b - p13_start).length() < 1e-4, "bridge ends where tick 13 starts");
    }

    /// Off screen the trail restarts: no bridge back across the gap.
    #[test]
    fn trail_restarts_after_off_screen() {
        let mut st = new_state(&effect());
        let mut out = Vec::new();
        lay(&mut st, 10, 0.2, &mut out);
        let e = effect();
        let pl = Placed { end: Vec3::NAN, offscreen: true, at: None, delta: Vec3::ZERO };
        emit_trail(&e, &trail(), &mut st, &pl, Vec3::ZERO, 0.2, 10.0, &Sight::new(&[([glam::Vec4::ZERO; 6], LodPlane { eye: Vec3::ZERO, forward: Vec3::X, scale: 1.0 })], false), 0, &mut out);
        lay(&mut st, 14, 0.2, &mut out);
        assert_eq!(out.len(), 2, "no segment off screen, no bridge after");
    }

    const ONE_TICK_EMITTER: &str = "EmitterBlueprint {
	Lifetime = 1.00,
	Repeattime = 1.00,
	CatchupEmit = false,
	EmitRateCurve = { XRange = 1.00, Keys = { { x=0.00,y=3.00,z=0.00 }, }, },
	LifetimeCurve = { XRange = 1.00, Keys = { { x=0.00,y=5.00,z=0.00 }, }, },
	SizeCurve = { XRange = 1.00, Keys = { { x=0.00,y=1.00,z=0.00 }, }, },
}";

    /// An emitter that lived one tick, first seen a tick later (the frame
    /// that would have shown its tick never ran): emitted backdated,
    /// whatever its `CatchupEmit` (that is about ticks missed off screen).
    #[test]
    fn emitter_catches_up_a_tick_the_frame_skipped() {
        let EffectBp::Emitter(bp) = EffectBp::parse(ONE_TICK_EMITTER).unwrap() else { panic!() };
        let mut e = effect();
        e.kind = EffectViewKind::Emitter;
        let mut st = new_state(&e);
        let budget = std::sync::atomic::AtomicU32::new(CATCHUP_BUDGET);
        let ground = |_: f32, _: f32| 0.0;
        let at = (Vec3::new(1.0, 2.0, 3.0), Quat::IDENTITY);
        let mut out = Vec::new();
        // Created on tick 10; the first frame to see it shows tick 11.
        let deferred = emit_effect(&e, &bp, &[], &mut st, at, at, 11, 10.0, true, None, &ground, &budget, 0, &mut out);
        assert!(!deferred);
        assert_eq!(out.len(), 3, "the one tick it lived, emitted backdated");
        assert!(out.iter().all(|(_, r, _)| (9.0..=10.0).contains(&r.time[0])), "born during tick 10");
        out.clear();
        emit_effect(&e, &bp, &[], &mut st, at, at, 12, 11.0, true, None, &ground, &budget, 0, &mut out);
        assert!(out.is_empty(), "its lifetime was one tick");
    }
}
