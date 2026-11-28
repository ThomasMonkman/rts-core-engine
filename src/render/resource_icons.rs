//! Resource deposits' icons on the zoomed-out view, as moho draws them
//! (MohoEngine.dll 0x10427b90, `primbatcher.fx` `TResourceIcon`): past
//! `UI_ResourceLODCutoff` (75, .data 0x10898d74; "When to draw icons
//! instead of resource deposit textures") a deposit's view depth, its
//! `/env/common/splats/mass_strategic.dds` or `hydrocarbon_strategic.dds`
//! at half the texture's size (32 x 32 -> 16 pixels), centred on the
//! deposit's projected point (rounded to a pixel), point sampled, no depth
//! test, at the texture's own colour. With fog of war on, only
//! deposits on explored ground (moho asks `IsExplored` at the centre). These are separate from the map's
//! deposit splats (`mass_marker.dds`, FAF's `CreateResources`), which fade
//! at their own LOD.
//!
//! They glow: `TResourceIcon`'s second pass writes only the frame alpha (our
//! glow target) with `SrcAlpha_InvSrcAlpha`, its alpha the texel's times
//! `0.6 sin²(time - 10 R)`, R = 0.00277 x the vertex's distance in pixels
//! from the screen's corner: a pulse rippling out across the screen.
//!
//! Not drawn: moho's `UI_RenResources` switch (on) isn't read, and it also
//! skips deposits outside the playable area. Inferred: the shader's `time`
//! in seconds.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::render::camera::LodPlane;

use crate::engine::assets::dds;
use crate::engine::vfs::Vfs;

const SHADER: &str = include_str!(concat!(env!("OUT_DIR"), "/resource_icons.wgsl"));

/// `UI_ResourceLODCutoff`'s default: view depth past which icons draw.
pub const LOD_CUTOFF: f32 = 75.0;
/// The icons by deposit type (1 mass, 2 hydrocarbon).
const TEXTURES: [&str; 2] = ["/env/common/splats/mass_strategic.dds", "/env/common/splats/hydrocarbon_strategic.dds"];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    /// Clip space (the quad is laid out in pixels).
    ndc: [f32; 2],
    uv: [f32; 2],
    /// The deposit's world x, z (fog of war).
    at: [f32; 2],
    /// `ResourceVS`'s R: 0.00277 x the vertex's distance (pixels) from
    /// the screen's corner.
    ripple: f32,
}

/// A deposit as the scene has it: type, world x, z, size.
pub type Deposit = (u8, f64, f64, f64);

pub struct ResourceIcons {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Per type: the texture's bind group and size, once loaded.
    textures: [Option<(wgpu::BindGroup, [f32; 2])>; 2],
    loading: bool,
    failures: Vec<String>,
    done_tx: Sender<Vec<(usize, Option<dds::Texture>)>>,
    done_rx: Receiver<Vec<(usize, Option<dds::Texture>)>>,
}

/// A world view's deposit icons this frame ([`ResourceIcons::update`]).
#[derive(Default)]
pub struct IconsFrame {
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    vertices: Vec<Vertex>,
    /// Vertex ranges per type.
    draws: [std::ops::Range<u32>; 2],
}

impl IconsFrame {
    /// Icons drawn (tests).
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.vertices.len() / 6
    }
}

fn vertex_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("resource icons: vertices"),
        size: (capacity * std::mem::size_of::<Vertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl ResourceIcons {
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resource icons: layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("resource icons: shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("resource icons: pipeline layout"), bind_group_layouts: &[Some(globals), Some(&layout)], immediate_size: 0 });
        // Pass 1's `AlphaBlend_SrcAlpha_InvSrcAlpha_Write_A` on the glow
        // target: glow = a² + glow (1 - a).
        let glow = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Src, dst_factor: wgpu::BlendFactor::OneMinusSrc, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resource icons"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                // Pass 0's `AlphaBlend_SrcAlpha_InvSrcAlpha_Write_RGB`.
                targets: &super::globals::scene_targets(format, Some(wgpu::BlendState::ALPHA_BLENDING), super::globals::GlowWrite::Blend(glow)),
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            // `Depth_Disable`.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // `PointSampler`.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("resource icons: sampler"), ..Default::default() });
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        Self {
            pipeline,
            layout,
            sampler,
            textures: [None, None],
            loading: false,
            failures: Vec::new(),
            done_tx,
            done_rx,
        }
    }

    /// Starts reading the two icons (once).
    pub fn set_vfs(&mut self, vfs: Option<&Arc<RwLock<Vfs>>>) {
        if self.loading || self.textures.iter().any(Option::is_some) {
            return;
        }
        let Some(vfs) = vfs.cloned() else { return };
        self.loading = true;
        let tx = self.done_tx.clone();
        rayon::spawn(move || {
            let read = |path: &str| vfs.read().ok().and_then(|v| v.read(path).ok()).and_then(|b| dds::read(&b, false).ok());
            let _ = tx.send(TEXTURES.iter().enumerate().map(|(i, p)| (i, read(p))).collect());
        });
    }

    #[allow(dead_code)]
    pub fn loading(&self) -> bool {
        self.loading
    }

    #[allow(dead_code)]
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    fn collect(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        while let Ok(done) = self.done_rx.try_recv() {
            self.loading = false;
            for (i, t) in done {
                let size = t.as_ref().map(|t| [t.width as f32, t.height as f32]);
                let label = format!("resource icon {}", TEXTURES.get(i).unwrap_or(&"?"));
                let view = t.as_ref().and_then(|t| super::models::upload_texture(device, queue, t, false, &label));
                match (view, size) {
                    (Some(v), Some(size)) => {
                        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some(&label),
                            layout: &self.layout,
                            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&v) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
                        });
                        self.textures[i] = Some((group, size));
                    }
                    _ => self.failures.push(format!("resource icon {}", TEXTURES[i])),
                }
            }
        }
    }

    /// This frame's icons: each deposit whose LOD metric (`lod`, at
    /// `height(x, z)`) passes the cutoff, at its point on the
    /// `screen` (`project`). `scale`: physical pixels per interface pixel.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, deposits: &[Deposit], lod: LodPlane, height: &dyn Fn(f32, f32) -> f32, project: &dyn Fn(Vec3) -> Option<[f32; 2]>, screen: [f32; 2], scale: f32, out: &mut IconsFrame) {
        self.collect(device, queue);
        out.vertices.clear();
        out.draws = [0..0, 0..0];
        let [w, h] = screen;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        for kind in 0..2 {
            let Some((_, [tw, th])) = &self.textures[kind] else { continue };
            // Half extents: a quarter of the texture's size.
            let (hw, hh) = (0.25 * tw * scale, 0.25 * th * scale);
            let start = out.vertices.len() as u32;
            for &(_, x, z, _) in deposits.iter().filter(|d| d.0 as usize == kind + 1) {
                let (x, z) = (x as f32, z as f32);
                let p = Vec3::new(x, height(x, z), z);
                if lod.metric(p) <= LOD_CUTOFF {
                    continue;
                }
                let Some([sx, sy]) = project(p) else { continue };
                let (sx, sy) = (sx.round(), sy.round());
                if sx + hw < 0.0 || sy + hh < 0.0 || sx - hw > w || sy - hh > h {
                    continue;
                }
                let corner = |u: f32, v: f32| {
                    let (px, py) = (sx + (2.0 * u - 1.0) * hw, sy + (2.0 * v - 1.0) * hh);
                    Vertex { ndc: [px / w * 2.0 - 1.0, 1.0 - py / h * 2.0], uv: [u, v], at: [x, z], ripple: 0.00277 * (px / scale).hypot(py / scale) }
                };
                let (a, b, c, d) = (corner(0.0, 0.0), corner(1.0, 0.0), corner(0.0, 1.0), corner(1.0, 1.0));
                out.vertices.extend_from_slice(&[a, b, c, c, b, d]);
            }
            out.draws[kind] = start..out.vertices.len() as u32;
        }
        if out.vertices.is_empty() {
            return;
        }
        if out.buffer.is_none() || out.vertices.len() > out.capacity {
            out.capacity = out.vertices.len().max(1024).next_power_of_two();
            out.buffer = Some(vertex_buffer(device, out.capacity));
        }
        if let Some(buffer) = &out.buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&out.vertices));
        }
    }

    /// Draws into the scene pass (group 0 bound).
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, frame: &'a IconsFrame) {
        let (false, Some(buffer)) = (frame.vertices.is_empty(), &frame.buffer) else { return };
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, buffer.slice(..));
        for (kind, range) in frame.draws.iter().enumerate() {
            if let (Some((group, _)), false) = (&self.textures[kind], range.is_empty()) {
                pass.set_bind_group(1, group, &[]);
                pass.draw(range.clone(), 0..1);
            }
        }
    }
}
