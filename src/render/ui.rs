//! The SupCom interface pass: the user layer's draw list
//! (`rts_core_engine::ui`) drawn over the scene in one draw call, from one
//! texture atlas, as SupCom does ("the entire interface ... in a single draw
//! call", "a single texture which acts like a texture atlas", regenerated
//! on the fly when new sprites are needed: Adrian Courrèges' graphics
//! study).
//!
//! The atlas holds UI textures (decoded `.dds`), font glyphs (rasterised by
//! ab_glyph from FA's TrueType fonts) and one white texel for flat fills,
//! packed in shelves. When it fills up it's cleared and refilled with what
//! this frame needs. Images too big to share it (FAF's 1900x1080 faction
//! backgrounds, the lobby's 1024x768 frame: two of them leave the atlas no
//! room for the rest) get a texture of their own; the quads are drawn in
//! order, one draw call per run that uses the same texture. Movies draw
//! from their own YCbCr plane textures with a second pipeline (`fs_movie`).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ab_glyph::{Font, ScaleFont};
use rts_core_engine::engine::vfs::Vfs;
use rts_core_engine::ui::controls::{ControlId, DrawItem};
use rts_core_engine::ui::movie::VideoFrame;
use rts_core_engine::ui::fonts::{self, Fonts};

const ATLAS: u32 = 2048;
/// Images wider or taller than this get their own texture, not the atlas.
const OWN_TEXTURE: u32 = 512;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// Where an image sits in the atlas (texels) and, for glyphs, how to place it.
#[derive(Clone, Copy, Debug)]
struct Slot {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    /// Glyphs: offset of the bitmap's top-left from the pen (pixels).
    offset: [f32; 2],
    /// In a texture of its own (`UiRenderer::own`), not the atlas.
    own: Option<usize>,
}

/// An image with a texture of its own (bound with the screen and sampler).
struct OwnTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    w: u32,
    h: u32,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum Key {
    White,
    Texture(String),
    /// Font file, pixel size x 4 (quantised), glyph id.
    Glyph(&'static str, u32, u16),
}

/// Shelf packer over the atlas.
#[derive(Default)]
struct Packer {
    x: u32,
    y: u32,
    shelf: u32,
}

impl Packer {
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > ATLAS || h > ATLAS {
            return None;
        }
        if self.x + w > ATLAS {
            self.y += self.shelf + 1;
            self.x = 0;
            self.shelf = 0;
        }
        if self.y + h > ATLAS {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w + 1;
        self.shelf = self.shelf.max(h);
        Some(at)
    }
}

pub struct UiRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    screen: wgpu::Buffer,
    atlas: wgpu::Texture,
    vertices: wgpu::Buffer,
    vertex_capacity: usize,
    vertex_count: u32,
    packer: Packer,
    slots: HashMap<Key, Option<Slot>>,
    fonts: Option<Fonts>,
    vfs: Option<Arc<RwLock<Vfs>>>,
    /// Times the atlas was regenerated (it filled up).
    pub regenerations: u32,
    /// Images made in memory (`#name` textures, e.g. the minimap), RGBA.
    images: HashMap<String, (u32, u32, Vec<u8>)>,
    /// Physical pixels per logical pixel: text shadows sit one logical
    /// pixel down and right.
    pub scale: f32,
    /// Images with their own texture (`None`: replaced), and which is whose;
    /// kept when the atlas is regenerated.
    own: Vec<Option<OwnTexture>>,
    own_by_path: HashMap<String, usize>,
    /// This frame's draw calls in order: the vertices of each, and its
    /// texture.
    batches: Vec<(Tex, std::ops::Range<u32>)>,
    /// Movies: YCbCr planes converted in the shader (`fs_movie`).
    movie_pipeline: wgpu::RenderPipeline,
    movie_layout: wgpu::BindGroupLayout,
    movies: HashMap<ControlId, MovieTexture>,
    /// How far into the files changed on disk the textures have been
    /// reloaded (`--enable-disk-watch`: `engine::disk_watch`).
    disk_watch_cursor: usize,
}

/// What a draw call samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tex {
    Atlas,
    /// An image's own texture (`UiRenderer::own`).
    Own(usize),
    /// A movie control's frame (`UiRenderer::movies`).
    Movie(ControlId),
}

/// A movie's planes on the GPU (R8: luma, then the half-size chroma
/// planes), re-uploaded when its frame changes.
struct MovieTexture {
    planes: [wgpu::Texture; 3],
    bind_group: wgpu::BindGroup,
    /// Luma plane size.
    size: (u32, u32),
    serial: u64,
    /// Drawn this frame (textures of movies no longer drawn are freed).
    used: bool,
}

const SHADER: &str = include_str!(concat!(env!("OUT_DIR"), "/ui.wgsl"));

/// sRGB channel -> linear (vertex colours multiply linear texels).
fn linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

impl UiRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("interface: glyph + image atlas"),
            size: wgpu::Extent3d { width: ATLAS, height: ATLAS, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let screen = device.create_buffer(&wgpu::BufferDescriptor { label: Some("interface: screen size"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("interface: sampler"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("interface: layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let view = super::view(&atlas, "interface: glyph + image atlas");
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("interface: glyph + image atlas"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: screen.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        // Movies: the screen, the sampler and three R8 planes.
        let plane = |binding: u32| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let movie_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("interface: movie layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                plane(3),
                plane(4),
                plane(5),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("interface: shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline = Self::pipeline(device, &shader, &layout, "interface", "fs", format);
        let movie_pipeline = Self::pipeline(device, &shader, &movie_layout, "interface: movies", "fs_movie", format);
        let vertices = device.create_buffer(&wgpu::BufferDescriptor { label: Some("interface: vertices"), size: 6 * 1024 * std::mem::size_of::<Vertex>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self {
            pipeline,
            layout,
            sampler,
            bind_group,
            screen,
            atlas,
            vertices,
            vertex_capacity: 6 * 1024,
            vertex_count: 0,
            packer: Packer::default(),
            slots: HashMap::new(),
            fonts: None,
            vfs: None,
            regenerations: 0,
            images: HashMap::new(),
            scale: 1.0,
            own: Vec::new(),
            own_by_path: HashMap::new(),
            batches: Vec::new(),
            movie_pipeline,
            movie_layout,
            movies: HashMap::new(),
            disk_watch_cursor: rts_core_engine::engine::disk_watch::change_count(),
        }
    }

    /// Textures changed on disk (`--enable-disk-watch`) are read again at
    /// their next use (an atlas image's old space is reclaimed when the
    /// atlas is next regenerated).
    fn reload_changed(&mut self) {
        let changed = rts_core_engine::engine::disk_watch::changes_since(&mut self.disk_watch_cursor);
        if changed.is_empty() {
            return;
        }
        let hit = |p: &str| changed.iter().any(|c| *c == rts_core_engine::engine::vfs::normalise(p));
        self.slots.retain(|k, _| !matches!(k, Key::Texture(p) if !p.starts_with('#') && hit(p)));
        let own: Vec<String> = self.own_by_path.keys().filter(|p| !p.starts_with('#') && hit(p)).cloned().collect();
        for p in own {
            if let Some(i) = self.own_by_path.remove(&p) {
                self.own[i] = None;
            }
        }
    }

    /// The interface's pipeline over `layout`, with fragment shader `fs`.
    fn pipeline(device: &wgpu::Device, shader: &wgpu::ShaderModule, layout: &wgpu::BindGroupLayout, label: &str, fs: &str, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Movie `id`'s planes on the GPU, uploading `frame` if it's new.
    fn movie_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: ControlId, frame: &VideoFrame) {
        let p = &frame.planes;
        let size = (p.plane_width, p.plane_height);
        if self.movies.get(&id).is_none_or(|m| m.size != size) {
            let (cw, ch) = p.chroma_size();
            let make = |label: &str, w: u32, h: u32| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            };
            let planes = [make("interface: movie Y", size.0, size.1), make("interface: movie Cb", cw, ch), make("interface: movie Cr", cw, ch)];
            let views: Vec<wgpu::TextureView> = planes.iter().zip(["interface: movie Y", "interface: movie Cb", "interface: movie Cr"]).map(|(t, l)| super::view(t, l)).collect();
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("interface: movie"),
                layout: &self.movie_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.screen.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&views[0]) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&views[1]) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&views[2]) },
                ],
            });
            self.movies.insert(id, MovieTexture { planes, bind_group, size, serial: 0, used: false });
        }
        let m = self.movies.get_mut(&id).expect("movie texture");
        m.used = true;
        if m.serial == frame.serial {
            return;
        }
        m.serial = frame.serial;
        for (texture, data) in m.planes.iter().zip([&p.y, &p.cb, &p.cr]) {
            let (w, h) = (texture.width(), texture.height());
            if data.len() < (w * h) as usize {
                continue;
            }
            queue.write_texture(texture.as_image_copy(), data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) }, texture.size());
        }
    }

    /// Provides image `#name` (RGBA8, `w` x `h`) for quads to use.
    pub fn set_image(&mut self, name: &str, w: u32, h: u32, rgba: Vec<u8>) {
        self.slots.remove(&Key::Texture(name.to_string()));
        if let Some(i) = self.own_by_path.remove(name) {
            self.own[i] = None;
        }
        self.images.insert(name.to_string(), (w, h, rgba));
    }

    /// `name`'s pixels replaced in place when it is already a texture of
    /// that size (the minimap's unit dots, every sim tick); else as
    /// `set_image` (the texture is made at its next use).
    pub fn update_image(&mut self, queue: &wgpu::Queue, name: &str, w: u32, h: u32, rgba: &[u8]) {
        let key = Key::Texture(name.to_string());
        if let Some(Some(s)) = self.slots.get(&key)
            && s.w == w && s.h == h {
            match s.own {
                Some(i) => {
                    if let Some(t) = &self.own[i] {
                        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
                        queue.write_texture(t.texture.as_image_copy(), rgba, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) }, size);
                        return;
                    }
                }
                None => {
                    self.upload(queue, s.x, s.y, w, h, rgba);
                    return;
                }
            }
        }
        self.set_image(name, w, h, rgba.to_vec());
    }

    /// Draw calls of the last `draw` (one per batch).
    pub fn draw_count(&self) -> u32 {
        self.batches.len() as u32
    }

    pub fn set_vfs(&mut self, vfs: Arc<RwLock<Vfs>>) {
        if self.vfs.is_none() {
            self.fonts = Some(Fonts::new(vfs.clone()));
            self.vfs = Some(vfs);
        }
    }

    fn upload(&self, queue: &wgpu::Queue, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.atlas, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// A texture of its own for a large image.
    fn own_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &str, w: u32, h: u32, rgba: &[u8]) -> usize {
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(path),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(texture.as_image_copy(), rgba, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) }, size);
        let view = super::view(&texture, path);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(path),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.screen.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.own.push(Some(OwnTexture { texture, bind_group, w, h }));
        self.own_by_path.insert(path.to_string(), self.own.len() - 1);
        self.own.len() - 1
    }

    /// The atlas slot for `key` (or the image's own texture), adding it if
    /// new. `None`: it can't load or doesn't fit (`full` is set when the
    /// atlas ran out of room).
    fn slot(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, key: &Key, full: &mut bool) -> Option<Slot> {
        if let Some(s) = self.slots.get(key) {
            return *s;
        }
        if let Key::Texture(path) = key
            && let Some(&i) = self.own_by_path.get(path) {
            let t = self.own[i].as_ref()?;
            let s = Slot { x: 0, y: 0, w: t.w, h: t.h, offset: [0.0; 2], own: Some(i) };
            self.slots.insert(key.clone(), Some(s));
            return Some(s);
        }
        let (w, h, rgba, offset) = match key {
            Key::White => (1, 1, vec![255u8; 4], [0.0; 2]),
            // A map's preview image, read from the `.scmap` (lobby map previews).
            Key::Texture(path) if path.starts_with(rts_core_engine::ui::controls::SCMAP_PREVIEW) => {
                let map = &path[rts_core_engine::ui::controls::SCMAP_PREVIEW.len()..];
                let tex = self.vfs.as_ref().and_then(|v| {
                    let bytes = v.read().ok()?.read(map).ok()?;
                    let dds = rts_core_engine::engine::assets::scmap::read_preview(&bytes).ok()?.to_vec();
                    rts_core_engine::engine::assets::dds::read(&dds, false).ok()
                });
                match tex {
                    Some(t) if !t.levels.is_empty() => (t.width, t.height, t.levels[0].clone(), [0.0; 2]),
                    _ => {
                        tracing::warn!(map, "map preview failed to load");
                        self.slots.insert(key.clone(), None);
                        return None;
                    }
                }
            }
            Key::Texture(path) if path.starts_with('#') => match self.images.get(path) {
                Some((w, h, rgba)) => (*w, *h, rgba.clone(), [0.0; 2]),
                None => return None,
            },
            Key::Texture(path) => {
                let tex = self.vfs.as_ref().and_then(|v| v.read().ok()?.read(&path.to_lowercase()).ok()).and_then(|b| rts_core_engine::engine::assets::dds::read_image(&b).ok());
                match tex {
                    Some(t) if !t.levels.is_empty() => (t.width, t.height, t.levels[0].clone(), [0.0; 2]),
                    _ => {
                        tracing::warn!(path, "UI texture failed to load");
                        self.slots.insert(key.clone(), None);
                        return None;
                    }
                }
            }
            Key::Glyph(file, size4, id) => {
                let font = self.fonts.as_mut()?.font_file(file)?;
                let scale = ab_glyph::PxScale::from(*size4 as f32 / 4.0);
                let glyph = ab_glyph::GlyphId(*id).with_scale_and_position(scale, ab_glyph::point(0.0, 0.0));
                let Some(outline) = font.outline_glyph(glyph) else {
                    // Whitespace: nothing to draw.
                    self.slots.insert(key.clone(), None);
                    return None;
                };
                let b = outline.px_bounds();
                let (w, h) = (b.width().ceil() as u32, b.height().ceil() as u32);
                let mut rgba = vec![0u8; (w * h * 4) as usize];
                outline.draw(|x, y, c| {
                    let i = ((y * w + x) * 4) as usize;
                    if i + 3 < rgba.len() {
                        rgba[i..i + 3].copy_from_slice(&[255, 255, 255]);
                        rgba[i + 3] = (c.clamp(0.0, 1.0) * 255.0) as u8;
                    }
                });
                (w.max(1), h.max(1), rgba, [b.min.x, b.min.y])
            }
        };
        if let (Key::Texture(path), true) = (key, w > OWN_TEXTURE || h > OWN_TEXTURE) {
            let i = self.own_texture(device, queue, path, w, h, &rgba);
            let s = Slot { x: 0, y: 0, w, h, offset, own: Some(i) };
            self.slots.insert(key.clone(), Some(s));
            return Some(s);
        }
        let Some((x, y)) = self.packer.alloc(w, h) else {
            *full = true;
            return None;
        };
        self.upload(queue, x, y, w, h, &rgba);
        let s = Slot { x, y, w, h, offset, own: None };
        self.slots.insert(key.clone(), Some(s));
        Some(s)
    }

    /// Builds this frame's vertices from the draw list (`screen` in pixels).
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, items: &[DrawItem], screen: [f32; 2]) {
        self.reload_changed();
        queue.write_buffer(&self.screen, 0, bytemuck::cast_slice(&[screen[0], screen[1], 0.0, 0.0]));
        let mut verts = Vec::with_capacity(items.len() * 6);
        for m in self.movies.values_mut() {
            m.used = false;
        }
        for attempt in 0..2 {
            verts.clear();
            let mut full = false;
            self.batches.clear();
            self.build(device, queue, items, &mut verts, &mut full);
            if !full || attempt == 1 {
                break;
            }
            // Regenerate: start over with only what this frame needs.
            self.slots.clear();
            self.packer = Packer::default();
            self.regenerations += 1;
        }
        // Movies no longer drawn give their planes back.
        self.movies.retain(|_, m| m.used);
        if verts.len() > self.vertex_capacity {
            self.vertex_capacity = verts.len().next_power_of_two();
            self.vertices = device.create_buffer(&wgpu::BufferDescriptor { label: Some("interface: vertices"), size: (self.vertex_capacity * std::mem::size_of::<Vertex>()) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        }
        queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&verts));
        self.vertex_count = verts.len() as u32;
    }

    fn build(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, items: &[DrawItem], out: &mut Vec<Vertex>, full: &mut bool) {
        let a = ATLAS as f32;
        // Vertices so far go to the current draw call; a quad from another
        // texture starts the next.
        let batch = |batches: &mut Vec<(Tex, std::ops::Range<u32>)>, own: Tex, at: usize| {
            if batches.last().is_none_or(|(b, _)| *b != own) {
                batches.push((own, at as u32..at as u32));
            }
        };
        // Consecutive quads mostly share a colour (thousands of one army's
        // strategic icons): the sRGB -> linear conversion is remembered.
        let mut last_colour: Option<([f32; 4], [f32; 4])> = None;
        // `turned`: the texture a quarter anticlockwise in `r` (vertical text).
        let mut quad = |out: &mut Vec<Vertex>, r: [f32; 4], uv: [f32; 4], c: [f32; 4], turned: bool| {
            let c = match last_colour {
                Some((from, to)) if from == c => to,
                _ => {
                    let to = [linear(c[0]), linear(c[1]), linear(c[2]), c[3]];
                    last_colour = Some((c, to));
                    to
                }
            };
            let v = |x: f32, y: f32, u: f32, w: f32| Vertex { pos: [x, y], uv: [u, w], color: c };
            // Each corner's texture coordinate: top left, top right, bottom
            // left, bottom right.
            let [tl, tr, bl, br] = if turned {
                [(uv[2], uv[1]), (uv[2], uv[3]), (uv[0], uv[1]), (uv[0], uv[3])]
            } else {
                [(uv[0], uv[1]), (uv[2], uv[1]), (uv[0], uv[3]), (uv[2], uv[3])]
            };
            out.extend_from_slice(&[v(r[0], r[1], tl.0, tl.1), v(r[2], r[1], tr.0, tr.1), v(r[0], r[3], bl.0, bl.1), v(r[0], r[3], bl.0, bl.1), v(r[2], r[1], tr.0, tr.1), v(r[2], r[3], br.0, br.1)]);
        };
        let mut memo: HashMap<&str, Option<Slot>> = HashMap::new();
        for item in items {
            match item {
                // Expanded into quads by the frame (`frame::minimap_items`);
                // world views are the scene's.
                DrawItem::Minimap { .. } | DrawItem::World { .. } => {}
                DrawItem::Quad { rect, texture, uv, color, .. } => {
                    // This frame's lookups by path, memoised without copying
                    // the path (one per strategic icon was most of this).
                    let slot = match texture {
                        Some(path) => match memo.get(path.as_str()) {
                            Some(&s) => s,
                            None => {
                                let s = self.slot(device, queue, &Key::Texture(path.clone()), full);
                                memo.insert(path.as_str(), s);
                                s
                            }
                        },
                        None => self.slot(device, queue, &Key::White, full),
                    };
                    let Some(s) = slot else { continue };
                    // Half a texel in from the edges so filtering never bleeds
                    // a neighbour in.
                    let (x0, y0) = (s.x as f32 + 0.5, s.y as f32 + 0.5);
                    let (sw, sh) = ((s.w as f32 - 1.0).max(0.0), (s.h as f32 - 1.0).max(0.0));
                    let (tw, th) = if s.own.is_some() { (s.w as f32, s.h as f32) } else { (a, a) };
                    let tuv = [(x0 + uv[0] * sw) / tw, (y0 + uv[1] * sh) / th, (x0 + uv[2] * sw) / tw, (y0 + uv[3] * sh) / th];
                    batch(&mut self.batches, s.own.map_or(Tex::Atlas, Tex::Own), out.len());
                    quad(out, *rect, tuv, *color, false);
                }
                DrawItem::Text { rect, text, family, size, color, shadow, vertical } => {
                    let file = fonts::file_for(family);
                    let Some(font) = self.fonts.as_mut().and_then(|f| f.font(family)) else { continue };
                    let px = fonts::px(*size);
                    let scaled = font.as_scaled(px);
                    let baseline = rect[1] + scaled.ascent();
                    let size4 = (px.y * 4.0).round() as u32;
                    let mut pen = rect[0];
                    let mut prev = None;
                    let mut glyphs = Vec::new();
                    for ch in text.chars() {
                        let g = scaled.glyph_id(ch);
                        if let Some(p) = prev {
                            pen += scaled.kern(p, g);
                        }
                        glyphs.push((g, pen));
                        pen += scaled.h_advance(g);
                        prev = Some(g);
                    }
                    let pen_end = (pen - rect[0]).round();
                    for pass in [true, false] {
                        if pass && !shadow {
                            continue;
                        }
                        let shadow_offset = self.scale.round().max(1.0);
                        let (dx, c) = if pass { (shadow_offset, [0.0, 0.0, 0.0, color[3]]) } else { (0.0, *color) };
                        for &(g, x) in &glyphs {
                            let Some(s) = self.slot(device, queue, &Key::Glyph(file, size4, g.0), full) else { continue };
                            let (gx, gy) = ((x + s.offset[0]).round(), (baseline + s.offset[1]).round());
                            let (w, h) = (s.w as f32, s.h as f32);
                            // Vertical: the line turned about its box, so a
                            // point `along` the line at `down` from its top
                            // lands at (left + down, top + length - along).
                            let r = if *vertical {
                                let (along, down) = (gx - rect[0], gy - rect[1]);
                                [rect[0] + down, rect[1] + pen_end - along - w, rect[0] + down + h, rect[1] + pen_end - along]
                            } else {
                                [gx, gy, gx + w, gy + h]
                            };
                            let r = [r[0] + dx, r[1] + dx, r[2] + dx, r[3] + dx];
                            let uv = [s.x as f32 / a, s.y as f32 / a, (s.x + s.w) as f32 / a, (s.y + s.h) as f32 / a];
                            batch(&mut self.batches, Tex::Atlas, out.len());
                            quad(out, r, uv, c, *vertical);
                        }
                    }
                }
                DrawItem::Movie { rect, movie, frame, color } => {
                    let f = &frame.0;
                    if f.planes.plane_width == 0 || f.planes.plane_height == 0 {
                        continue;
                    }
                    self.movie_texture(device, queue, *movie, f);
                    // The picture, without the planes' macroblock padding.
                    let uv = [0.0, 0.0, f.planes.width as f32 / f.planes.plane_width as f32, f.planes.height as f32 / f.planes.plane_height as f32];
                    batch(&mut self.batches, Tex::Movie(*movie), out.len());
                    quad(out, *rect, uv, *color, false);
                }
            }
        }
        // Each draw call runs to the next one's start.
        let ends: Vec<u32> = self.batches.iter().skip(1).map(|(_, r)| r.start).chain(std::iter::once(out.len() as u32)).collect();
        for ((_, range), end) in self.batches.iter_mut().zip(ends) {
            range.end = end;
        }
    }

    /// Draws the interface over `view`: a draw call per run of quads from
    /// one texture (the atlas, or an image's own).
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        if self.vertex_count == 0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("interface"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: super::gpu_prof::pass_writes("interface"),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        let mut movie_pipeline = None;
        for (tex, range) in &self.batches {
            let (bind_group, movie) = match tex {
                Tex::Own(i) => match &self.own[*i] {
                    Some(t) => (&t.bind_group, false),
                    None => continue,
                },
                Tex::Atlas => (&self.bind_group, false),
                Tex::Movie(id) => match self.movies.get(id) {
                    Some(m) => (&m.bind_group, true),
                    None => continue,
                },
            };
            if movie_pipeline != Some(movie) {
                pass.set_pipeline(if movie { &self.movie_pipeline } else { &self.pipeline });
                movie_pipeline = Some(movie);
            }
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(range.clone(), 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelves_pack_left_to_right_then_down_and_report_full() {
        let mut p = Packer::default();
        assert_eq!(p.alloc(1000, 10), Some((0, 0)));
        assert_eq!(p.alloc(1000, 20), Some((1001, 0)));
        assert_eq!(p.alloc(100, 5), Some((0, 21)), "next shelf below the tallest");
        assert_eq!(p.alloc(ATLAS + 1, 1), None);
        let mut q = Packer::default();
        assert!(q.alloc(ATLAS, ATLAS).is_some());
        assert_eq!(q.alloc(1, 1), None, "full");
    }
}
