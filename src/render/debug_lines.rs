//! Debug lines on the GPU: the sim's debug canvas (`DrawLine`, `DrawLinePop`,
//! `DrawCircle`; the latest snapshot's `debug.draw`) and each world view's
//! interface shapes (`UI_DrawLine`, `UI_DrawCircle`, `UI_DrawRect` from its
//! `OnRenderWorld`; `camera::View::shapes`). Every segment is one instance
//! (a circle becomes [`CIRCLE_SEGMENTS`] of them); `shaders/debug_lines.wesl`
//! projects its ends, clips it at the near plane and widens it on screen.
//! So the CPU only lists segments when the canvas or the shapes change, and
//! never projects or tessellates per frame.
//!
//! Drawn after the order lines, alpha blended and without depth (FAF's
//! patch draws its shapes with `TAlphaBlendLinearSampleNoDepth`; the sim
//! canvas is drawn the same way so a script's debug lines show through
//! hills). Presentation only.

use bytemuck::{Pod, Zeroable};
use rts_core_engine::engine::api::debug_draw::DebugDraw;

const SHADER: &str = include_str!(concat!(env!("OUT_DIR"), "/debug_lines.wgsl"));

/// Segments a circle is drawn with.
pub const CIRCLE_SEGMENTS: usize = 32;

/// One segment: ends `a` / `b` (xyz), `a.w` its width in world units,
/// `b.w` the fewest pixels it is wide; RGBA colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Instance {
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub color: [f32; 4],
}

/// The view's viewport for the shader (`Line` in the shader).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    viewport: [f32; 4],
}

fn rgba(argb: u32) -> [f32; 4] {
    [(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, (argb >> 24) as u8].map(|c| c as f32 / 255.0)
}

/// `draw`'s segments, appended to `out`.
pub fn segments(draw: &DebugDraw, out: &mut Vec<Instance>) {
    out.reserve(draw.lines.len() + draw.circles.len() * CIRCLE_SEGMENTS);
    for l in &draw.lines {
        out.push(Instance { a: [l.a[0], l.a[1], l.a[2], l.width], b: [l.b[0], l.b[1], l.b[2], l.min_px], color: rgba(l.color) });
    }
    for c in &draw.circles {
        let color = rgba(c.color);
        let at = |i: usize| {
            let t = i as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            [c.center[0] + c.radius * t.cos(), c.center[1], c.center[2] + c.radius * t.sin()]
        };
        for i in 0..CIRCLE_SEGMENTS {
            let (p, q) = (at(i), at(i + 1));
            out.push(Instance { a: [p[0], p[1], p[2], c.width], b: [q[0], q[1], q[2], c.min_px], color });
        }
    }
}

pub struct DebugLines {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// The sim canvas's segments, and which canvas they were made from (its
    /// snapshot's tick and the canvas's address: a paused sim republishes a
    /// tick, the same canvas).
    sim: Vec<Instance>,
    sim_from: (u64, usize),
}

/// One view's buffers.
#[derive(Default)]
pub struct LinesFrame {
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    count: u32,
    uniform: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    /// What the buffer holds: the sim canvas it was made from, and whether
    /// the view's shapes were in it (they change every frame they're on).
    uploaded: Option<((u64, usize), bool)>,
    scratch: Vec<Instance>,
}

impl DebugLines {
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug lines: viewport layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("debug lines: shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("debug lines: pipeline layout"), bind_group_layouts: &[Some(globals), Some(&layout)], immediate_size: 0 });
        let instances = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
        }];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug lines"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &instances, compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &super::globals::scene_targets(format, Some(wgpu::BlendState::ALPHA_BLENDING), super::globals::GlowWrite::Keep),
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
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
        Self { pipeline, layout, sim: Vec::new(), sim_from: (0, 0) }
    }

    /// The sim canvas the views draw (the latest snapshot's, of `tick`): its
    /// segments are listed again only when it's a new one.
    pub fn set_canvas(&mut self, canvas: Option<&DebugDraw>, tick: u64) {
        let from = (tick, canvas.map_or(0, |c| c as *const DebugDraw as usize));
        if from != self.sim_from {
            self.sim.clear();
            if let Some(c) = canvas {
                segments(c, &mut self.sim);
            }
            self.sim_from = from;
        }
    }

    /// View `frame`'s segments: the sim canvas's and its `shapes`; its
    /// viewport `viewport` (pixels) at `px_per_distance` (world units a
    /// pixel covers per unit of view depth).
    pub fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &mut LinesFrame, shapes: Option<&DebugDraw>, viewport: [f32; 2], px_per_distance: f32) {
        let uniform = Uniform { viewport: [viewport[0].max(1.0), viewport[1].max(1.0), px_per_distance, 0.0] };
        let (buffer, _) = frame.uniform.get_or_insert_with(|| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("debug lines: viewport"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("debug lines: viewport"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            (buffer, group)
        });
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
        let key = (self.sim_from, shapes.is_some());
        if frame.uploaded == Some(key) && shapes.is_none() {
            return;
        }
        let instances: &[Instance] = match shapes {
            None => &self.sim,
            Some(s) => {
                frame.scratch.clear();
                frame.scratch.extend_from_slice(&self.sim);
                segments(s, &mut frame.scratch);
                &frame.scratch
            }
        };
        frame.count = instances.len() as u32;
        if !instances.is_empty() {
            if frame.buffer.is_none() || instances.len() > frame.capacity {
                frame.capacity = instances.len().max(1024).next_power_of_two();
                frame.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("debug lines: segments"),
                    size: (frame.capacity * std::mem::size_of::<Instance>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            if let Some(b) = &frame.buffer {
                queue.write_buffer(b, 0, bytemuck::cast_slice(instances));
            }
        }
        frame.uploaded = Some(key);
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, frame: &'a LinesFrame) {
        if let (true, Some(buffer), Some((_, group))) = (frame.count > 0, &frame.buffer, &frame.uniform) {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(1, group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..frame.count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rts_core_engine::engine::api::debug_draw::{DebugCircle, DebugLine};

    #[test]
    fn lines_and_circles_become_segments() {
        let draw = DebugDraw {
            lines: vec![DebugLine { a: [1.0, 2.0, 3.0], b: [4.0, 5.0, 6.0], color: 0x80FF_0000, width: 0.15, min_px: 2.0 }],
            circles: vec![DebugCircle { center: [10.0, 0.0, 10.0], radius: 5.0, color: 0xFF00_FF00, width: 0.0, min_px: 1.5 }],
        };
        let mut out = Vec::new();
        segments(&draw, &mut out);
        assert_eq!(out.len(), 1 + CIRCLE_SEGMENTS);
        assert_eq!(out[0], Instance { a: [1.0, 2.0, 3.0, 0.15], b: [4.0, 5.0, 6.0, 2.0], color: [1.0, 0.0, 0.0, 128.0 / 255.0] });
        // The ring closes on its first point, at the radius, flat.
        let ring = &out[1..];
        assert_eq!(ring[0].a[..3], [15.0, 0.0, 10.0]);
        let last = ring[CIRCLE_SEGMENTS - 1].b;
        assert!((last[0] - 15.0).abs() < 1e-4 && (last[2] - 10.0).abs() < 1e-4 && last[1] == 0.0);
        assert!(ring.iter().all(|s| s.b[3] == 1.5 && s.color == [0.0, 1.0, 0.0, 1.0]));
    }
}
