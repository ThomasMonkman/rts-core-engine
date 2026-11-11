//! `.dds` textures from the VFS onto the GPU, on a loader thread.
//!
//! A texture the GPU samples as stored (block-compressed, or 32-bit BGRA:
//! FAF's uncompressed unit textures, sampled as `Bgra8Unorm`) is streamed
//! from its file, decompressed from the archive as it's read, straight into
//! a mapped staging buffer (through a small cache-resident buffer: mapped
//! memory is write-only), laid out as the GPU copy wants it (rows padded to
//! 256 bytes); one GPU copy then puts it in the texture. Where reading the
//! file whole, splitting it into levels and `Queue::write_texture` each
//! copied all of it (the last on the render thread), it now goes to memory
//! once. The staging buffer is `MAP_WRITE` (else wgpu maps a hidden one,
//! zeroed, and copies it over at unmap), and a model's textures share one
//! submit ([`load_into`]): those two overheads had cost more than the copies
//! saved. Anything else (other uncompressed or packed pixels, BC on a GPU
//! without it, a file shorter than its header says) goes the old way: read
//! whole, [`dds::read`], [`upload_texture`], still on the caller's thread.
//!
//! Textures many models name (a faction's lookup and build textures) load
//! once through a [`Cache`].

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rts_core_engine::engine::assets::mip_cache::{self, MipCache};
use rts_core_engine::engine::assets::{dds, mips};
use rts_core_engine::engine::vfs::Vfs;

use super::models::{texture_format, upload_texture};

/// The texture at `path` in the VFS, uploaded (any thread). `bc`: the GPU
/// samples block-compressed textures (else everything is decoded to RGBA).
/// `label`: what it is (`"albedo"`, `"decal"`): the GPU objects are named
/// by it and the path. `None` if it's missing or unreadable.
pub fn load(device: &wgpu::Device, queue: &wgpu::Queue, vfs: &RwLock<Vfs>, path: &str, srgb: bool, bc: bool, label: &str) -> Option<wgpu::TextureView> {
    let vfs = vfs.read().ok()?;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(&format!("upload: {label} {path}")) });
    let view = load_into(device, queue, &mut encoder, &vfs, path, srgb, bc, label);
    queue.submit([encoder.finish()]);
    view
}

/// [`load`], its GPU copy recorded in `encoder` (to submit several
/// textures' at once: a submit costs as much as streaming a texture).
#[allow(clippy::too_many_arguments)]
pub fn load_into(device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, vfs: &Vfs, path: &str, srgb: bool, bc: bool, label: &str) -> Option<wgpu::TextureView> {
    load_with_cache(device, queue, encoder, vfs, path, srgb, bc, label, mip_cache())
}

/// [`load_into`] with `cache` holding generated mip levels.
#[allow(clippy::too_many_arguments)]
pub(super) fn load_with_cache(device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, vfs: &Vfs, path: &str, srgb: bool, bc: bool, label: &str, cache: Option<&MipCache>) -> Option<wgpu::TextureView> {
    let label = &format!("{label} {path}");
    if let Ok(Some(view)) = vfs.with_reader(path, |r| stream(device, encoder, r, srgb, bc, label)) {
        return Some(view);
    }
    // Stored without a full chain (or not streamable): the cached copy
    // with its generated levels streams instead, if there is one.
    let slot = cache.and_then(|cache| MipSlot::new(cache, vfs.fingerprint(path), path, srgb, bc));
    if let Some(view) = slot.and_then(|s| stream_cached(device, encoder, s, srgb, bc, label)) {
        return Some(view);
    }
    let _s = rts_core_engine::prof_span!("texture: read + decode + upload (old path)");
    let mut t = dds::read(&vfs.read(path).ok()?, bc).ok()?;
    fill_mips(&mut t, srgb, bc, label, slot);
    upload_texture(device, queue, &t, srgb, label)
}

/// The cache of generated mip levels ([`MipCache`]), in the engine's cache
/// folder; `None` without one, with `RTS_NO_MIP_CACHE` set (to time
/// generating them), and in tests (which never write the user's cache).
pub(super) fn mip_cache() -> Option<&'static MipCache> {
    static CACHE: OnceLock<Option<MipCache>> = OnceLock::new();
    CACHE
        .get_or_init(|| if cfg!(test) || std::env::var_os("RTS_NO_MIP_CACHE").is_some() { None } else { rts_core_engine::install::cache_dir().map(|d| MipCache::new(d.join("mips"))) })
        .as_ref()
}

/// A texture's entry in a [`MipCache`].
#[derive(Clone, Copy)]
pub(super) struct MipSlot<'a> {
    cache: &'a MipCache,
    key: u64,
}

impl<'a> MipSlot<'a> {
    /// `source`: the texture's fingerprint, or a hash of its bytes; `None`
    /// (no entry) without one, or when no levels are generated.
    pub(super) fn new(cache: &'a MipCache, source: Option<u64>, path: &str, srgb: bool, bc: bool) -> Option<Self> {
        fill_mips_enabled().then_some(())?;
        Some(Self { cache, key: mip_cache::key(source?, path, srgb, bc) })
    }
}

/// [`stream`]s the cached copy in `slot`, if there is a good one (a bad one
/// is dropped).
fn stream_cached(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, slot: MipSlot, srgb: bool, bc: bool, label: &str) -> Option<wgpu::TextureView> {
    let (cache, key) = (slot.cache, slot.key);
    let path = cache.get(key)?;
    let _s = rts_core_engine::prof_span!("texture: stream cached mips");
    match std::fs::File::open(&path).and_then(|f| stream(device, encoder, &mut std::io::BufReader::new(f), srgb, bc, label)) {
        Ok(Some(view)) => {
            tracing::warn!("{label}: has no full mip chain, using the cached generated levels");
            Some(view)
        }
        _ => {
            cache.remove(key);
            None
        }
    }
}

/// Whether textures stored without a full mip chain get the rest
/// generated ([`mips::complete`]): on unless `RTS_NO_GEN_MIPS` is set (to
/// compare against the stored levels alone).
pub(super) fn fill_mips_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RTS_NO_GEN_MIPS").is_none())
}

/// When the mip cache is pruned: once per process, after a map's textures
/// have loaded (so the entries it used are marked used) and the player's
/// options have both been applied (so a larger budget isn't pruned to the
/// default first).
#[derive(Default)]
struct PruneGate {
    budget_mb: Option<u64>,
    max_age_days: Option<u64>,
    loaded: bool,
    done: bool,
}

static PRUNE: std::sync::Mutex<PruneGate> = std::sync::Mutex::new(PruneGate { budget_mb: None, max_age_days: None, loaded: false, done: false });

/// A map's textures have loaded.
pub(super) fn mip_cache_loaded() {
    update_prune(|g| g.loaded = true);
}

/// `eng_TextureCacheMB` (the Video options' Texture Cache Disk Space).
pub(super) fn set_mip_cache_budget(mb: u64) {
    update_prune(|g| g.budget_mb = Some(mb));
}

/// `eng_TextureCacheDays` (the Video options' Texture Cache: Keep Unused).
pub(super) fn set_mip_cache_max_age(days: u64) {
    update_prune(|g| g.max_age_days = Some(days));
}

fn update_prune(f: impl FnOnce(&mut PruneGate)) {
    let Ok(mut g) = PRUNE.lock() else { return };
    f(&mut g);
    let (Some(budget_mb), Some(max_age_days), true, false) = (g.budget_mb, g.max_age_days, g.loaded, g.done) else { return };
    g.done = true;
    let Some(cache) = mip_cache() else { return };
    let settings = mip_cache::Settings { budget_mb, max_age_days };
    let spawned = std::thread::Builder::new().name("mip cache prune".into()).spawn(move || {
        match cache.prune(std::time::SystemTime::now(), settings.max_age(), settings.max_bytes()) {
            Ok((removed, kept)) => tracing::info!(removed, kept_mb = kept >> 20, budget_mb, max_age_days, "mip cache pruned"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("mip cache prune: {e}"),
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("mip cache prune: {e}");
    }
}

/// Generates the levels `t` lacks, with a warning naming it (`what`: its
/// label and path), as the real game fills them in at load. `slot`: its
/// cache entry; the levels come from there when it has them, and go into
/// it when generated. `bc`: as `t` was read.
pub(super) fn fill_mips(t: &mut dds::Texture, srgb: bool, bc: bool, what: &str, slot: Option<MipSlot>) {
    let full = mips::full_chain(t.format, t.width, t.height);
    if !fill_mips_enabled() || t.levels.len() >= full {
        return;
    }
    let cache = slot.map(|s| (s.key, s.cache));
    if let Some((key, cache)) = cache
        && let Some(path) = cache.get(key)
    {
        match std::fs::read(&path).and_then(|b| dds::read(&b, bc)) {
            Ok(c) if (c.width, c.height, c.format, c.levels.len()) == (t.width, t.height, t.format, full) => {
                tracing::warn!("{what}: {}x{} {:?} has no full mip chain, using the cached generated levels", t.width, t.height, t.format);
                *t = c;
                return;
            }
            _ => cache.remove(key),
        }
    }
    let stored = t.levels.len();
    let added = {
        let _s = rts_core_engine::prof_span!("texture: generate mips");
        mips::complete(t, srgb)
    };
    if added == 0 {
        return;
    }
    tracing::warn!(stored, added, "{what}: {}x{} {:?} has no full mip chain, generated the rest", t.width, t.height, t.format);
    // (BC textures that aren't whole blocks can't be uploaded: not cached.)
    if let Some((key, cache)) = cache
        && !(t.format.is_compressed() && (t.width % 4 != 0 || t.height % 4 != 0))
        && let Err(e) = cache.put(key, &dds::write(t))
    {
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!("mip cache: can't write ({e}); generated levels won't be cached");
        }
    }
}

/// Textures by path and colour space, each loaded once however many
/// loaders ask for it (172 models name the Seraphim lookup texture). A
/// loader asking for one another is loading waits for it.
#[derive(Clone, Default)]
pub struct Cache(Arc<Mutex<HashMap<(String, bool), Arc<OnceLock<Option<wgpu::TextureView>>>>>>);

impl Cache {
    /// Drops `path` (changed on disk: `--enable-disk-watch`), so the next
    /// load reads it again. Holders of the old view keep it.
    pub fn forget(&self, path: &str) {
        if let Ok(mut map) = self.0.lock() {
            map.retain(|(p, _), _| rts_core_engine::engine::vfs::normalise(p) != path);
        }
    }

    /// [`load`] through the cache. Submitted before it's shared, so a view
    /// handed out is never one whose copy hasn't been submitted yet.
    pub fn load(&self, device: &wgpu::Device, queue: &wgpu::Queue, vfs: &Vfs, path: &str, srgb: bool, bc: bool, label: &str) -> Option<wgpu::TextureView> {
        let cell = self.0.lock().ok()?.entry((path.to_owned(), srgb)).or_default().clone();
        cell.get_or_init(|| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(&format!("upload: {label} {path}")) });
            let view = load_into(device, queue, &mut encoder, vfs, path, srgb, bc, label);
            queue.submit([encoder.finish()]);
            view
        })
        .clone()
    }
}

/// How loaded textures are used: sampled, written by the upload (and in
/// tests read back, to compare the upload paths).
pub(super) const USAGE: wgpu::TextureUsages = if cfg!(test) {
    wgpu::TextureUsages::TEXTURE_BINDING.union(wgpu::TextureUsages::COPY_DST).union(wgpu::TextureUsages::COPY_SRC)
} else {
    wgpu::TextureUsages::TEXTURE_BINDING.union(wgpu::TextureUsages::COPY_DST)
};

/// The file goes into GPU memory in pieces of this size (see [`stream`]).
const CHUNK: usize = 64 * 1024;

/// Bytes per row in a buffer-to-texture copy are a multiple of this.
const ROW_ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Streams a `.dds` the GPU samples as stored ([`dds::layout`]; BC only
/// if `bc`) from `r` into a new texture; `None` when it isn't one (the
/// caller decodes it instead).
fn stream(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, r: &mut dyn Read, srgb: bool, bc: bool, label: &str) -> std::io::Result<Option<wgpu::TextureView>> {
    let mut header = [0u8; 128];
    r.read_exact(&mut header)?;
    let Some(layout) = dds::layout(&header)? else { return Ok(None) };
    // Missing levels are generated on the decoding path.
    if fill_mips_enabled() && layout.levels.len() < mips::full_chain(layout.format, layout.width, layout.height) {
        return Ok(None);
    }
    let compressed = layout.format.is_compressed();
    // BC textures must be whole blocks.
    if compressed && (!bc || layout.width % 4 != 0 || layout.height % 4 != 0) {
        return Ok(None);
    }
    // Per level: offset in the staging buffer, the row as stored, the row
    // as copied (padded), rows (of blocks, if compressed), extent.
    let mut at = 0u64;
    let levels: Vec<(u64, u32, u32, u32, (u32, u32))> = layout
        .levels
        .iter()
        .map(|&(w, h, _)| {
            let (row, rows, extent) = match layout.format {
                dds::Format::Bc1 => (w.div_ceil(4) * 8, h.div_ceil(4), (w.next_multiple_of(4), h.next_multiple_of(4))),
                _ if compressed => (w.div_ceil(4) * 16, h.div_ceil(4), (w.next_multiple_of(4), h.next_multiple_of(4))),
                _ => (w * 4, h, (w, h)),
            };
            let padded = row.next_multiple_of(ROW_ALIGN);
            let offset = at;
            at += u64::from(padded) * u64::from(rows);
            (offset, row, padded, rows, extent)
        })
        .collect();
    let staging = {
        let _s = rts_core_engine::prof_span!("texture: staging buffer");
        device.create_buffer(&wgpu::BufferDescriptor { label: Some(&format!("staging: {label}")), size: at, usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: true })
    };
    {
        let _s = rts_core_engine::prof_span!("texture: read into staging");
        // Mapped memory is write-only (it may be write-combined: never read
        // it, nor hand it to a decompressor that might), so the file comes
        // through a small buffer that stays in cache.
        let mut mapped = staging.slice(..).get_mapped_range_mut();
        let mut chunk = vec![0u8; CHUNK];
        for &(offset, row, padded, rows, _) in &levels {
            let (offset, row, padded) = (offset as usize, row as usize, padded as usize);
            if row == padded {
                // Rows already aligned (the large levels): straight through.
                let end = offset + padded * rows as usize;
                let mut at = offset;
                while at < end {
                    let n = (end - at).min(CHUNK);
                    r.read_exact(&mut chunk[..n])?;
                    mapped.slice(at..at + n).copy_from_slice(&chunk[..n]);
                    at += n;
                }
            } else {
                for y in 0..rows as usize {
                    let at = offset + y * padded;
                    r.read_exact(&mut chunk[..row])?;
                    mapped.slice(at..at + row).copy_from_slice(&chunk[..row]);
                }
            }
        }
    }
    let _s = rts_core_engine::prof_span!("texture: create + copy");
    staging.unmap();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: layout.width, height: layout.height, depth_or_array_layers: 1 },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: texture_format(layout.format, srgb),
        usage: USAGE,
        view_formats: &[],
    });
    for (l, &(offset, _, padded, rows, (w, h))) in levels.iter().enumerate() {
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo { buffer: &staging, layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: Some(padded), rows_per_image: Some(rows) } },
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: l as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }
    Ok(Some(super::view(&texture, label)))
}
