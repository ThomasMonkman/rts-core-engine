use globset::{Glob, GlobSetBuilder};
use memmap2::Mmap;
use rayon::prelude::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    io::{self, Cursor, Read},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Instant,
};
use tracing::{debug, info};
use walkdir::WalkDir;
use zip::{ZipArchive, read::ZipArchiveMetadata};

pub mod tree;

#[derive(Debug, Clone)]
pub struct MountPoint {
    pub source: PathBuf,
    pub mountpoint: String,
}

struct ArchiveIndex {
    mmap: Arc<Mmap>,
    zip_meta: Arc<ZipArchiveMetadata>,
    entries: Vec<(String, usize)>,
}

struct FolderIndex {
    mount_source: PathBuf,
    mount_path: String,
    entries: Vec<PathBuf>,
}

enum Mounted {
    Archieve(ArchiveIndex),
    Folder(FolderIndex),
}

#[derive(Clone)]
struct ArchiveAccess {
    id: ArchiveId,
    mmap: Arc<Mmap>,
    meta: Arc<ZipArchiveMetadata>,
}

#[derive(Debug, Clone)]
enum FileLocation {
    Archive {
        archive_idx: usize,
        zip_entry_idx: usize,
    },
    Folder {
        real_path: PathBuf,
    },
    /// A folder mount's `<name>.zst`, served decompressed as `<name>`: test
    /// content kept small in the repo (the audit's 81 km map, 64 MB raw).
    Zstd {
        real_path: PathBuf,
    },
    /// Built into the binary ([`Vfs::new_with_embedded`]).
    Embedded(&'static [u8]),
}

#[derive(Hash, Eq, PartialEq, Clone, Copy)]
struct ArchiveId(usize);

// Arc<Mmap> only impls AsRef<Mmap>, not AsRef<[u8]>, so Cursor<Arc<Mmap>> can't impl Read.
// Newtype bridges the gap while keeping the data 'static for thread_local storage.
#[derive(Clone)]
struct ArcMmap(Arc<Mmap>);
impl AsRef<[u8]> for ArcMmap {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

thread_local! {
    /// Each thread's open archives, by [`ArchiveId`] (unique in the
    /// process, so a later file system's archives never hit an earlier
    /// one's entries: the engine thread mounts one per session). The map
    /// also holds the mapping, to tell when no file system uses it any more.
    static ARCHIVES: RefCell<HashMap<ArchiveId, (Arc<Mmap>, ZipArchive<Cursor<ArcMmap>>)>>
        = RefCell::new(HashMap::new());
}

/// The next [`ArchiveId`].
static NEXT_ARCHIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Runs `f` on this thread's open copy of `access`'s archive.
fn with_zip<R>(access: &ArchiveAccess, f: impl FnOnce(&mut ZipArchive<Cursor<ArcMmap>>) -> io::Result<R>) -> io::Result<R> {
    ARCHIVES.with(|cell| {
        let mut zip_map = cell.borrow_mut();
        if !zip_map.contains_key(&access.id) {
            // Archives of dropped file systems go (only this map's two
            // references to the mapping left).
            zip_map.retain(|_, (m, _)| Arc::strong_count(m) > 2);
        }
        let (_, zip) = zip_map.entry(access.id).or_insert_with(|| {
            let mmap = access.mmap.clone();
            (mmap.clone(), unsafe { ZipArchive::unsafe_new_with_metadata(Cursor::new(ArcMmap(mmap)), access.meta.clone()) })
        });
        f(zip)
    })
}

impl FileLocation {
    pub fn get_file(&self, archive_accesses: &[ArchiveAccess]) -> io::Result<Vec<u8>> {
        match self {
            FileLocation::Archive { archive_idx, zip_entry_idx } => with_zip(&archive_accesses[*archive_idx], |zip| {
                let mut entry = zip.by_index(*zip_entry_idx)?;
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf)?;
                Ok(buf)
            }),
            FileLocation::Folder { real_path } => std::fs::read(real_path),
            FileLocation::Zstd { real_path } => zstd::stream::decode_all(io::BufReader::new(File::open(real_path)?)),
            FileLocation::Embedded(bytes) => Ok(bytes.to_vec()),
        }
    }

    /// Runs `f` on a reader over the file (see [`Vfs::with_reader`]).
    fn with_reader<R>(&self, archive_accesses: &[ArchiveAccess], f: impl FnOnce(&mut dyn Read) -> io::Result<R>) -> io::Result<R> {
        match self {
            FileLocation::Archive { archive_idx, zip_entry_idx } => with_zip(&archive_accesses[*archive_idx], |zip| f(&mut zip.by_index(*zip_entry_idx)?)),
            FileLocation::Folder { real_path } => f(&mut io::BufReader::new(File::open(real_path)?)),
            FileLocation::Zstd { real_path } => f(&mut zstd::Decoder::new(File::open(real_path)?)?),
            FileLocation::Embedded(bytes) => f(&mut Cursor::new(bytes)),
        }
    }

    /// Uncompressed size in bytes, without reading/decompressing the contents.
    pub fn size(&self, archive_accesses: &[ArchiveAccess]) -> io::Result<u64> {
        match self {
            FileLocation::Archive { archive_idx, zip_entry_idx } => with_zip(&archive_accesses[*archive_idx], |zip| Ok(zip.by_index_raw(*zip_entry_idx)?.size())),
            FileLocation::Folder { real_path } => Ok(std::fs::metadata(real_path)?.len()),
            // The frame header's content size (decoding the whole file when
            // the writer left it out).
            FileLocation::Zstd { real_path } => {
                let mut header = Vec::with_capacity(18);
                File::open(real_path)?.take(18).read_to_end(&mut header)?;
                match zstd::zstd_safe::get_frame_content_size(&header) {
                    Ok(Some(n)) => Ok(n),
                    _ => Ok(self.get_file(archive_accesses)?.len() as u64),
                }
            }
            FileLocation::Embedded(bytes) => Ok(bytes.len() as u64),
        }
    }
}

/// A file opened for streaming ([`Vfs::open`]).
pub trait ReadSeek: Read + io::Seek + Send {}
impl<T: Read + io::Seek + Send> ReadSeek for T {}

/// Result of [`Vfs::file_info`]. Deliberately has no timestamps: modification
/// times differ between machines and must never reach the sim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    pub size: u64,
    pub is_folder: bool,
    pub read_only: bool,
}

/// A per-extension transformation applied to a file's raw bytes the first time it
/// is read via [`Vfs::read_processed`]. The VFS stores these as opaque closures so
/// it never has to depend on higher-level layers (e.g. Lua translation).
pub type Transform = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

/// The mounted files, read-only: the same mounts give the same index, so
/// sessions that mount the same game share one.
#[derive(Clone)]
struct VfsIndex {
    mounts: Vec<MountPoint>,
    archieves: Vec<ArchiveAccess>,
    file_system: HashMap<String, FileLocation>,
    /// Hook folders (moho's `hook` list from the init file, then the
    /// engine's own), normalised, in order: a script read through
    /// [`Vfs::read_processed`] has `{folder}{path}` appended from each.
    hook_dirs: Vec<String>,
    /// All file paths, sorted. Built once after mounting so directory queries are a
    /// binary-searched prefix range and always return a deterministic order.
    sorted_paths: Vec<String>,
    /// Folder mounts as (canonical lowercase source, mount point), resolved
    /// once for [`Vfs::disk_to_local`] (canonicalising is several syscalls).
    folder_roots: Vec<(String, String)>,
}

/// The transforms and the files they produced: shared with every clone of
/// the [`Vfs`], so a script is translated once per process, not once per
/// session.
#[derive(Default)]
struct VfsLayer {
    /// Lowercase extension (no leading dot) -> transformer applied on first read.
    transforms: RwLock<HashMap<String, Transform>>,
    /// Normalised path -> processed (transformed) bytes. Lives for the process
    /// lifetime; archives are immutable per run, so the path alone is a safe key.
    processed_cache: RwLock<HashMap<String, Arc<[u8]>>>,
}

/// The mounted game files. Cloning is cheap: the index and the processed
/// files are shared, so every session reads the same archives once.
#[derive(Clone)]
pub struct Vfs {
    index: Arc<VfsIndex>,
    layer: Arc<VfsLayer>,
}

impl std::fmt::Debug for Vfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Vfs { .. }")
    }
}

impl VfsIndex {
    // Scan a batch of archives/dirs in parallel, then merge them into `self` in
    // order (first wins).
    fn mount_batch(&mut self) -> io::Result<()> {
        let index_span = crate::prof_span!("vfs index mounts");
        // A mount point that doesn't exist (an init file's user folder on a
        // machine without it) is skipped, as the game does.
        for m in self.mounts.iter().filter(|m| !m.source.exists()) {
            tracing::warn!(source = %m.source.display(), "vfs: mount point not found (skipped)");
        }
        let mounted_list: Vec<Mounted> = self
            .mounts
            .par_iter()
            .filter(|mount_point| mount_point.source.exists())
            .map(|mount_point| -> io::Result<Mounted> {
                let _s = crate::prof_span!("vfs index mount (worker)");
                if mount_point.source.is_dir() {
                    Ok(Mounted::Folder(get_folder_index(mount_point)?))
                } else {
                    get_archive_index(mount_point).map(Mounted::Archieve)
                }
            })
            .collect::<io::Result<_>>()?;
        let _ = index_span;

        // First mount wins: merge in mount order.
        let _s = crate::prof_span!("vfs merge mounts");
        for mounted in mounted_list {
            match mounted {
                Mounted::Archieve(index) => {
                    let archive_idx = self.archieves.len();
                    self.archieves.push(ArchiveAccess {
                        id: ArchiveId(NEXT_ARCHIVE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
                        mmap: index.mmap,
                        meta: index.zip_meta,
                    });
                    for (vpath, zip_entry_idx) in index.entries {
                        self.file_system
                            .entry(vpath)
                            .or_insert(FileLocation::Archive {
                                archive_idx,
                                zip_entry_idx,
                            });
                    }
                }
                Mounted::Folder(index) => {
                    self.merge_folder(index)?;
                }
            }
        }
        Ok(())
    }

    /// Adds a folder's files the earlier mounts don't have (first mount
    /// wins); returns the paths added.
    fn merge_folder(&mut self, index: FolderIndex) -> io::Result<Vec<String>> {
        let mut added = Vec::new();
        for real_path in index.entries {
            let rel = real_path
                .strip_prefix(&index.mount_source)
                .map_err(io::Error::other)?;
            let rel = rel.to_string_lossy();
            let (rel, compressed) = match rel.strip_suffix(".zst") {
                Some(inner) => (inner, true),
                None => (rel.as_ref(), false),
            };
            let vpath = normalise(&format!("{}/{}", index.mount_path, rel));
            if let std::collections::hash_map::Entry::Vacant(e) = self.file_system.entry(vpath) {
                added.push(e.key().clone());
                e.insert(if compressed {
                    FileLocation::Zstd { real_path }
                } else {
                    FileLocation::Folder { real_path }
                });
            }
        }
        Ok(added)
    }
}

/// A folder mount as [`VfsIndex::folder_roots`] keeps it.
fn folder_root(m: &MountPoint) -> (String, String) {
    let src = std::fs::canonicalize(&m.source).map_or_else(|_| m.source.to_string_lossy().replace('\\', "/"), |c| c.to_string_lossy().replace('\\', "/"));
    (src.to_lowercase().trim_end_matches('/').to_string(), m.mountpoint.clone())
}

impl Vfs {
    pub fn new(mount_points: Vec<MountPoint>, hook_paths: Vec<String>) -> io::Result<Self> {
        Self::new_with_embedded(mount_points, hook_paths, &[])
    }

    /// Like [`Self::new`], with `embedded` files (VFS path, contents) built
    /// into the binary mounted ahead of every mount point, so they win.
    pub fn new_with_embedded(mount_points: Vec<MountPoint>, hook_paths: Vec<String>, embedded: &[(&str, &'static [u8])]) -> io::Result<Self> {
        info!("starting vfs");
        let start = Instant::now();
        let mut index = VfsIndex {
            mounts: mount_points,
            archieves: Vec::new(),
            file_system: HashMap::new(),
            hook_dirs: hook_paths.iter().map(|d| normalise(d).trim_end_matches('/').to_string()).filter(|d| !d.is_empty()).collect(),
            sorted_paths: Vec::new(),
            folder_roots: Vec::new(),
        };
        for (path, bytes) in embedded {
            index.file_system.insert(normalise(path), FileLocation::Embedded(bytes));
        }
        index.mount_batch()?;
        index.folder_roots = index
            .mounts
            .iter()
            .filter(|m| m.source.is_dir())
            .map(folder_root)
            .collect();
        let _s = crate::prof_span!("vfs sort paths");
        index.sorted_paths = index.file_system.keys().cloned().collect();
        index.sorted_paths.par_sort_unstable();
        let elapsed = Instant::now() - start;
        info!("loaded vfs in {}ms", elapsed.as_millis());
        Ok(Self {
            index: Arc::new(index),
            layer: Arc::new(VfsLayer::default()),
        })
    }

    /// Mounts one more folder behind the others (files already mounted keep
    /// their place): a map the lobby downloaded (`net::map_download`). Every
    /// holder of this `Vfs` sees it; clones made before it don't.
    pub fn mount_folder(&mut self, mount_point: MountPoint) -> io::Result<()> {
        let folder = get_folder_index(&mount_point)?;
        let index = Arc::make_mut(&mut self.index);
        let added = index.merge_folder(folder)?;
        index.sorted_paths.extend(added);
        index.sorted_paths.par_sort_unstable();
        index.folder_roots.push(folder_root(&mount_point));
        index.mounts.push(mount_point);
        Ok(())
    }

    /// Stops applying the hook folder `dir` (as given to [`Self::new`]).
    pub fn remove_hook_dir(&mut self, dir: &str) {
        let dir = normalise(dir).trim_end_matches('/').to_string();
        Arc::make_mut(&mut self.index).hook_dirs.retain(|d| *d != dir);
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.index.file_system.keys().map(|s| s.as_str())
    }

    pub fn paths_by_patterns(
        &self,
        patterns: &[&str],
    ) -> Result<impl Iterator<Item = &str> + use<'_>, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }

        let glob = builder.build()?;
        Ok(self.paths().filter(move |path| glob.is_match(path)))
    }

    /// Metadata for a file or (implicit) folder in the VFS, `None` if absent.
    pub fn file_info(&self, path: &str) -> Option<FileInfo> {
        let key = normalise(path);
        if let Some(loc) = self.index.file_system.get(&key) {
            return Some(FileInfo {
                size: loc.size(&self.index.archieves).unwrap_or(0),
                is_folder: false,
                read_only: !matches!(loc, FileLocation::Folder { .. }),
            });
        }
        let mut prefix = key;
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        let i = self.index.sorted_paths.partition_point(|p| p.as_str() < prefix.as_str());
        self.index.sorted_paths
            .get(i)
            .filter(|p| p.starts_with(prefix.as_str()))
            .map(|_| FileInfo {
                size: 0,
                is_folder: true,
                read_only: false,
            })
    }

    /// A value that changes when the file's contents change, read from its
    /// metadata alone: an archive entry's CRC-32 and sizes, a loose file's
    /// size and modified time. For render-side disk caches only (generated
    /// mip levels): it holds modification times, which differ between
    /// machines and must never reach the sim (see [`FileInfo`]).
    pub fn fingerprint(&self, path: &str) -> Option<u64> {
        let disk = |p: &std::path::Path| -> Option<[u64; 3]> {
            let m = std::fs::metadata(p).ok()?;
            let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some([1, m.len(), t.as_nanos() as u64])
        };
        let parts = match self.index.file_system.get(&normalise(path))? {
            FileLocation::Archive { archive_idx, zip_entry_idx } => with_zip(&self.index.archieves[*archive_idx], |zip| {
                let e = zip.by_index_raw(*zip_entry_idx)?;
                Ok([0, e.size() ^ (e.compressed_size() << 32), e.crc32() as u64])
            })
            .ok()?,
            FileLocation::Folder { real_path } | FileLocation::Zstd { real_path } => disk(real_path)?,
            FileLocation::Embedded(bytes) => [2, bytes.len() as u64, xxhash_rust::xxh3::xxh3_64(bytes)],
        };
        let bytes: Vec<u8> = parts.iter().flat_map(|v| v.to_le_bytes()).collect();
        Some(xxhash_rust::xxh3::xxh3_64(&bytes))
    }

    /// Converts a real disk path into a VFS path using the folder mount points
    /// (moho `DiskToLocal`). Paths not under any folder mount are returned as-is,
    /// so already-local paths pass straight through.
    pub fn disk_to_local(&self, path: &str) -> String {
        let unixy = path.replace('\\', "/");
        // Already a VFS path (FAF's blueprint loader passes chunk names,
        // once per blueprint): nothing to resolve, no file-system calls.
        if self.index.file_system.contains_key(&normalise(&unixy)) {
            return unixy;
        }
        // Compare canonical paths when the file exists, so `../x/..` and
        // absolute spellings of a mounted folder both match.
        let lower = std::fs::canonicalize(path).map_or_else(|_| unixy.clone(), |c| c.to_string_lossy().replace('\\', "/")).to_lowercase();
        for (src, mountpoint) in &self.index.folder_roots {
            if let Some(rest) = lower.strip_prefix(src.as_str())
                && (rest.is_empty() || rest.starts_with('/')) {
                return normalise(&format!("{}/{}", mountpoint.trim_end_matches('/'), rest.trim_start_matches('/')));
            }
        }
        unixy
    }

    /// Recursively finds files under `directory` whose file name matches the
    /// wildcard `pattern` (moho `DiskFindFiles` semantics). Results are sorted.
    pub fn find_files(
        &self,
        directory: &str,
        pattern: &str,
    ) -> Result<impl Iterator<Item = &str> + use<'_>, globset::Error> {
        let mut prefix = normalise(directory);
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        let matcher = Glob::new(&pattern.replace('\\', "/").to_lowercase())?.compile_matcher();

        let start = self.index.sorted_paths.partition_point(|p| p.as_str() < prefix.as_str());
        Ok(self.index.sorted_paths[start..]
            .iter()
            .take_while(move |p| p.starts_with(prefix.as_str()))
            .map(|p| p.as_str())
            .filter(move |p| {
                let name = p.rsplit_once('/').map_or(*p, |(_, n)| n);
                matcher.is_match(name)
            }))
    }

    /// A hash of the game code: every `.lua` and `.bp` file in the folders
    /// the game runs ([`GAME_CODE`]): not maps, mods or preferences (those
    /// vary per player; the lobby deals with them), nor a FAF checkout's
    /// tooling (tests, editor settings, annotation stubs, files at its
    /// root). Peers whose hashes differ would run different code, so the
    /// network handshake compares it. Carriage returns are skipped, so a
    /// checkout with CRLF line ends hashes like one with LF.
    pub fn code_hash(&self) -> io::Result<u64> {
        let code: Vec<&String> = self
            .index
            .sorted_paths
            .iter()
            .filter(|p| matches!(extension(p), Some("lua" | "bp")))
            .filter(|p| GAME_CODE.iter().any(|d| p.starts_with(d)))
            .collect();
        // Each file on any core; combined in path order, so the result
        // doesn't depend on which finished first.
        let hashes: Vec<u64> = code
            .par_iter()
            .map(|p| {
                let bytes = self.index.file_system[p.as_str()].get_file(&self.index.archieves)?;
                let mut h = xxhash_rust::xxh3::Xxh3::new();
                for part in bytes.split(|&b| b == b'\r') {
                    h.update(part);
                }
                Ok(h.digest())
            })
            .collect::<io::Result<_>>()?;
        let mut all = xxhash_rust::xxh3::Xxh3::new();
        for (path, hash) in code.iter().zip(hashes) {
            all.update(path.as_bytes());
            all.update(&hash.to_le_bytes());
        }
        Ok(all.digest())
    }

    pub fn dump_paths(&self, out_path: &str) -> io::Result<()> {
        std::fs::write(out_path, self.index.sorted_paths.join("\n"))
    }

    pub fn bench_read_all(&self) -> (usize, usize, std::time::Duration) {
        let start = std::time::Instant::now();
        let results: Vec<Vec<u8>> = self
            .index
            .file_system
            .par_iter()
            .map(|(_, loc)| loc.get_file(&self.index.archieves).unwrap_or_default())
            .collect();
        let elapsed = start.elapsed();
        let total_bytes: usize = results.iter().map(|b| b.len()).sum();
        let file_count = results.len();
        info!(file_count, "bench_read_all timing");
        (file_count, total_bytes, elapsed)
    }

    /// Opens a file for streaming: a folder mount's file is read from disk
    /// as it's used (movies run to 177 MB); an archive's is read whole.
    pub fn open(&self, path: &str) -> io::Result<Box<dyn ReadSeek>> {
        let loc = self.index.file_system.get(&normalise(path)).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("not in VFS: {path}"))
        })?;
        match loc {
            FileLocation::Folder { real_path } => Ok(Box::new(File::open(real_path)?)),
            FileLocation::Embedded(bytes) => Ok(Box::new(Cursor::new(*bytes))),
            FileLocation::Archive { .. } | FileLocation::Zstd { .. } => Ok(Box::new(Cursor::new(loc.get_file(&self.index.archieves)?))),
        }
    }

    /// The file on disk behind `path`, when it comes from a folder mount
    /// (not an archive): big files (the game's sound banks) can then be
    /// memory-mapped instead of read whole.
    pub fn real_path(&self, path: &str) -> Option<PathBuf> {
        match self.index.file_system.get(&normalise(path))? {
            FileLocation::Folder { real_path } => Some(real_path.clone()),
            FileLocation::Archive { .. } | FileLocation::Zstd { .. } | FileLocation::Embedded(_) => None,
        }
    }

    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let loc = self.index.file_system.get(&normalise(path)).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("not in VFS: {path}"))
        })?;
        loc.get_file(&self.index.archieves)
    }

    /// Runs `f` on a reader over a file's bytes (decompressed as they're
    /// read, for an archive entry), so a caller can put them straight where
    /// they go instead of into a `Vec` first. `f` mustn't read other files.
    pub fn with_reader<R>(&self, path: &str, f: impl FnOnce(&mut dyn Read) -> io::Result<R>) -> io::Result<R> {
        let loc = self.index.file_system.get(&normalise(path)).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("not in VFS: {path}"))
        })?;
        loc.with_reader(&self.index.archieves, f)
    }

    /// Registers a transformer for files with the given extension (no leading dot,
    /// case-insensitive). Applied lazily the first time a matching file is read via
    /// [`read_processed`](Self::read_processed).
    pub fn register_transform(&self, ext: &str, f: Transform) {
        self.layer.transforms.write().unwrap().insert(ext.to_lowercase(), f);
    }

    /// Reads a file, applying any transformer registered for its extension, and
    /// caches the result keyed by normalised path. Repeated reads of the same path
    /// return the cached bytes without re-running the transform. Files with no
    /// registered transform are returned (and cached) unchanged.
    /// Like [`read_processed`](Self::read_processed) but bypasses the cache in both
    /// directions. For callers that keep their own derived cache (e.g. compiled
    /// bytecode) and don't want the source text held in memory twice.
    pub fn read_processed_uncached(&self, path: &str) -> io::Result<Vec<u8>> {
        self.process(&normalise(path), path, &[])
    }

    /// Like [`read_processed_uncached`](Self::read_processed_uncached), with
    /// the hook folders `extra` (normalised, no trailing slash: a session's
    /// active mods, `engine::mods`) applied after the VFS's own.
    pub fn read_processed_with_hooks(&self, path: &str, extra: &[String]) -> io::Result<Vec<u8>> {
        self.process(&normalise(path), path, extra)
    }

    /// Whether `path` is a file in the VFS.
    pub fn contains(&self, path: &str) -> bool {
        self.index.file_system.contains_key(&normalise(path))
    }

    /// A file's contents with its hooks appended (scripts only), then
    /// transformed for its extension.
    fn process(&self, key: &str, path: &str, extra: &[String]) -> io::Result<Vec<u8>> {
        let loc = self.index.file_system.get(key).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("not in VFS: {path}"))
        })?;
        let mut raw = loc.get_file(&self.index.archieves)?;
        self.append_hooks(key, &mut raw, extra)?;
        let transforms = self.layer.transforms.read().unwrap();
        Ok(match extension(key).and_then(|e| transforms.get(e)) {
            Some(f) => {
                let _span = tracing::info_span!("transform", path = %key).entered();
                f(&raw)
            }
            None => raw,
        })
    }

    /// Moho's script hooks: for each hook folder, in order, the file at
    /// `{folder}{key}` (first mount wins, as for any file) is appended to
    /// the script's text, so it runs after it, in the same chunk and
    /// environment. Hook files themselves aren't hooked.
    fn append_hooks(&self, key: &str, raw: &mut Vec<u8>, extra: &[String]) -> io::Result<()> {
        if !matches!(extension(key), Some("lua" | "bp")) || self.index.hook_dirs.iter().chain(extra).any(|d| key.starts_with(d.as_str()) && key[d.len()..].starts_with('/')) {
            return Ok(());
        }
        for dir in self.index.hook_dirs.iter().chain(extra) {
            let hook_path = format!("{dir}{key}");
            let Some(hook) = self.index.file_system.get(&hook_path) else { continue };
            debug!(hook = %hook_path, base = key, "vfs: applying hook");
            raw.push(b'\n');
            raw.extend_from_slice(&hook.get_file(&self.index.archieves)?);
        }
        Ok(())
    }

    /// The files a script's chunk is made of, as [`append_hooks`](Self::append_hooks)
    /// joins them: the script, then each hook (with `extra`'s), each with
    /// the chunk line its first line lands on. The translation keeps every
    /// line where it was, so a chunk line maps back to a line of one of
    /// these files (the Lua debugger, `engine::lua::debugger`).
    pub fn chunk_segments(&self, path: &str, extra: &[String]) -> io::Result<Vec<(String, u32)>> {
        let key = normalise(path);
        let loc = self.index.file_system.get(&key).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("not in VFS: {path}")))?;
        let mut out = vec![(key.clone(), 1)];
        if !matches!(extension(&key), Some("lua" | "bp")) || self.index.hook_dirs.iter().chain(extra).any(|d| key.starts_with(d.as_str()) && key[d.len()..].starts_with('/')) {
            return Ok(out);
        }
        let lines = |bytes: &[u8]| bytes.iter().filter(|&&b| b == b'\n').count() as u32;
        let mut prev = loc;
        for dir in self.index.hook_dirs.iter().chain(extra) {
            let hook_path = format!("{dir}{key}");
            let Some(hook) = self.index.file_system.get(&hook_path) else { continue };
            // The previous part's lines, then the `\n` that joins them.
            let start = out.last().unwrap().1 + lines(&prev.get_file(&self.index.archieves)?) + 1;
            out.push((hook_path, start));
            prev = hook;
        }
        Ok(out)
    }

    /// All VFS file paths in sorted order.
    pub fn sorted_paths(&self) -> &[String] {
        &self.index.sorted_paths
    }

    pub fn read_processed(&self, path: &str) -> io::Result<Arc<[u8]>> {
        let key = normalise(path);
        if let Some(hit) = self.layer.processed_cache.read().unwrap().get(&key) {
            return Ok(hit.clone());
        }
        let out: Arc<[u8]> = Arc::from(self.process(&key, path, &[])?);
        self.layer.processed_cache
            .write()
            .unwrap()
            .insert(key, out.clone());
        Ok(out)
    }
}

/// Returns the lowercase-implied extension (the substring after the last `.` that
/// follows the last `/`), or `None` if there is no dot in the final path segment.
/// `path` is expected to already be normalised (lowercased) by [`normalise`].
/// The folders of game code ([`Vfs::code_hash`]).
const GAME_CODE: &[&str] = &["/effects/", "/env/", "/loc/", "/lua/", "/meshes/", "/projectiles/", "/props/", "/schook/", "/textures/", "/units/"];

fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map(|(_, ext)| ext)
}

fn get_archive_index(path: &MountPoint) -> io::Result<ArchiveIndex> {
    let _s = crate::prof_span!("index archive", source = %path.source.display());
    let start = Instant::now();
    info!(source = %path.source.display(), "opening archive");
    let mmap = {
        let _s = crate::prof_span!("open + mmap");
        let file = File::open(&path.source)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {}", path.source.display(), e)))?;
        Arc::new(unsafe { Mmap::map(&file)? })
    };

    let zip = {
        let _s = crate::prof_span!("read zip central directory");
        ZipArchive::new(Cursor::new(&mmap[..]))?
    };
    let zip_meta = zip.metadata();
    let count = zip.len();
    let _names = crate::prof_span!("zip entry paths", count);
    let mut entries = Vec::with_capacity(count);
    // Names come from the central directory alone; `by_index_raw` would also
    // read each entry's local header, faulting in pages across the whole
    // archive (300 ms for units.scd with a cold file cache).
    for i in 0..count {
        let Some(name) = zip.name_for_index(i) else { continue };
        if name.ends_with('/') {
            continue;
        }
        let vpath = normalise(&format!("{}{}", path.mountpoint, name));
        entries.push((vpath, i));
    }

    let elapsed = Instant::now() - start;
    info!(
        "indexed '{}' in {}ms ({} entries)",
        path.source.display(),
        elapsed.as_millis(),
        entries.len()
    );
    Ok(ArchiveIndex {
        mmap,
        zip_meta,
        entries,
    })
}

fn get_folder_index(mount_point: &MountPoint) -> io::Result<FolderIndex> {
    let _s = crate::prof_span!("index folder", source = %mount_point.source.display());
    let start = Instant::now();

    let mut entries = Vec::new();
    for entry in WalkDir::new(&mount_point.source) {
        let entry = entry?;
        if entry.file_type().is_file() {
            entries.push(entry.path().to_path_buf());
        }
    }
    let elapsed = Instant::now() - start;
    info!(
        "indexed '{}' in {}ms ({} entries)",
        mount_point.source.display(),
        elapsed.as_millis(),
        entries.len()
    );
    Ok(FolderIndex {
        mount_source: mount_point.source.clone(),
        mount_path: mount_point.mountpoint.clone(),
        entries,
    })
}

/// A VFS key: forward slashes, lowercase, rooted.
pub fn normalise(raw: &str) -> String {
    let s = raw.replace('\\', "/").replace("//", "/").to_lowercase();
    if s.starts_with('/') {
        s
    } else {
        format!("/{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `DiskToLocal`: VFS paths pass through untouched (the blueprint
    /// loader's case, no file-system calls); disk paths under a folder mount,
    /// in any spelling, become VFS paths; others pass through.
    #[test]
    fn disk_to_local_maps_folder_paths_and_passes_vfs_paths() {
        let dir = std::env::temp_dir().join(format!("vfs_d2l_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("units")).unwrap();
        std::fs::write(dir.join("units/a_unit.bp"), b"").unwrap();
        let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        assert_eq!(vfs.disk_to_local("/units/a_unit.bp"), "/units/a_unit.bp");
        assert_eq!(vfs.disk_to_local(&dir.join("units/a_unit.bp").to_string_lossy()), "/units/a_unit.bp");
        assert_eq!(vfs.disk_to_local(&dir.join("units/../units/a_unit.bp").to_string_lossy()), "/units/a_unit.bp");
        assert_eq!(vfs.disk_to_local("/elsewhere/x.lua"), "/elsewhere/x.lua");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Embedded files win over every mount and read like any other file.
    #[test]
    fn embedded_files_override_mounts() {
        let dir = std::env::temp_dir().join(format!("vfs_embed_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(dir.join("lua/config.lua"), b"disk").unwrap();
        std::fs::write(dir.join("lua/other.lua"), b"other").unwrap();
        let vfs = Vfs::new_with_embedded(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![], &[("/Lua/Config.lua", b"built in")]).unwrap();
        assert_eq!(vfs.read("/lua/config.lua").unwrap(), b"built in");
        assert_eq!(vfs.read("/lua/other.lua").unwrap(), b"other");
        let info = vfs.file_info("/lua/config.lua").unwrap();
        assert_eq!((info.size, info.read_only), (8, true));
        assert!(vfs.real_path("/lua/config.lua").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A folder mount's `<name>.zst` reads, streams, opens and sizes as
    /// `<name>`, decompressed; the `.zst` name itself isn't a VFS path.
    #[test]
    fn zst_files_in_folders_read_decompressed() {
        let dir = std::env::temp_dir().join(format!("vfs_zst_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("maps/m")).unwrap();
        let raw = b"heightmap ".repeat(1000);
        std::fs::write(dir.join("maps/m/m.scmap.zst"), zstd::encode_all(&raw[..], 19).unwrap()).unwrap();
        let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
        assert_eq!(vfs.read("/maps/m/m.scmap").unwrap(), raw);
        assert_eq!(vfs.with_reader("/maps/m/m.scmap", |r| { let mut v = Vec::new(); r.read_to_end(&mut v)?; Ok(v) }).unwrap(), raw);
        let mut opened = Vec::new();
        vfs.open("/maps/m/m.scmap").unwrap().read_to_end(&mut opened).unwrap();
        assert_eq!(opened, raw);
        let info = vfs.file_info("/maps/m/m.scmap").unwrap();
        assert_eq!((info.size, info.read_only), (raw.len() as u64, true));
        assert!(!vfs.contains("/maps/m/m.scmap.zst"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Moho's hooks: each hook folder's file of the same path is appended
    /// to a script, in hook-folder order, the first mount's copy winning
    /// (an empty one shadows a later mount's); hook files aren't hooked and
    /// non-scripts aren't either.
    #[test]
    fn hooks_append_in_folder_order() {
        let base = std::env::temp_dir().join(format!("vfs_hooks_{}", std::process::id()));
        let (first, second) = (base.join("first"), base.join("second"));
        for (root, name, body) in [
            (&first, "lua/a.lua", "base"),
            (&first, "lua/b.txt", "text"),
            (&first, "schook/lua/a.lua", "-- shadow"),
            (&first, "engine_hook/lua/a.lua", "engine"),
            (&first, "engine_hook/lua/b.txt", "not hooked"),
            (&second, "schook/lua/a.lua", "shadowed"),
        ] {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let mounts = [&first, &second].map(|d| MountPoint { source: d.to_path_buf(), mountpoint: "/".into() }).to_vec();
        let vfs = Vfs::new(mounts, vec!["/SCHook".into(), "/engine_hook/".into()]).unwrap();
        assert_eq!(&*vfs.read_processed("/lua/A.lua").unwrap(), b"base\n-- shadow\nengine");
        assert_eq!(vfs.read_processed_uncached("/lua/a.lua").unwrap(), b"base\n-- shadow\nengine");
        assert_eq!(&*vfs.read_processed("/lua/b.txt").unwrap(), b"text");
        assert_eq!(&*vfs.read_processed("/schook/lua/a.lua").unwrap(), b"-- shadow");
        assert_eq!(vfs.read("/lua/a.lua").unwrap(), b"base");
        std::fs::remove_dir_all(&base).ok();
    }

    /// The code hash follows `.lua` / `.bp` contents (not line ends) and
    /// ignores maps and everything else.
    #[test]
    fn code_hash_covers_game_code_only() {
        let dir = std::env::temp_dir().join(format!("vfs_hash_{}", std::process::id()));
        let hash = |files: &[(&str, &[u8])]| {
            std::fs::remove_dir_all(&dir).ok();
            for (name, body) in files {
                let p = dir.join(name);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, body).unwrap();
            }
            Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap().code_hash().unwrap()
        };
        let base = hash(&[("lua/a.lua", b"x = 1\n"), ("units/u.bp", b"UnitBlueprint {}")]);
        assert_eq!(base, hash(&[("lua/a.lua", b"x = 1\r\n"), ("units/u.bp", b"UnitBlueprint {}")]));
        assert_eq!(base, hash(&[("lua/a.lua", b"x = 1\n"), ("units/u.bp", b"UnitBlueprint {}"), ("maps/m/m_script.lua", b"y"), ("textures/t.dds", b"z")]));
        // A checkout's tooling and stray root files don't count.
        assert_eq!(base, hash(&[("lua/a.lua", b"x = 1\n"), ("units/u.bp", b"UnitBlueprint {}"), ("init_local_development.lua", b"local x"), ("tests/t.lua", b"t"), (".vscode/p.lua", b"p")]));
        assert_ne!(base, hash(&[("lua/a.lua", b"x = 2\n"), ("units/u.bp", b"UnitBlueprint {}")]));
        assert_ne!(base, hash(&[("lua/b.lua", b"x = 1\n"), ("units/u.bp", b"UnitBlueprint {}")]));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extension_extracts_final_segment_suffix() {
        assert_eq!(extension("/lua/ruleinit.lua"), Some("lua"));
        assert_eq!(extension("/a.b/c.scd"), Some("scd"));
        assert_eq!(extension("/noext"), None);
        assert_eq!(extension("/dir.with.dot/file"), None);
    }

    #[test]
    fn read_processed_transforms_once_and_caches() {
        // Mount a temp folder holding one file with a fake extension.
        let dir = std::env::temp_dir().join(format!("vfs_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("greeting.fake"), b"world").unwrap();

        let vfs = Vfs::new(
            vec![MountPoint {
                source: dir.clone(),
                mountpoint: "/".to_string(),
            }],
            vec![],
        )
        .unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();
        vfs.register_transform(
            "fake",
            Arc::new(move |b: &[u8]| {
                calls_in.fetch_add(1, Ordering::SeqCst);
                let mut out = b"hello ".to_vec();
                out.extend_from_slice(b);
                out
            }),
        );

        let first = vfs.read_processed("/greeting.fake").unwrap();
        let second = vfs.read_processed("/greeting.fake").unwrap();

        assert_eq!(&*first, b"hello world", "transform should be applied");
        assert_eq!(&*second, b"hello world");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "second read must hit the cache, not re-run the transform"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_files_filters_by_directory_and_name_sorted() {
        let dir = std::env::temp_dir().join(format!("vfs_test_find_{}", std::process::id()));
        for f in ["units/b/b_unit.bp", "units/a/a_unit.bp", "units/a/a.lua", "projectiles/p/p_proj.bp"] {
            let path = dir.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let vfs = Vfs::new(
            vec![MountPoint {
                source: dir.clone(),
                mountpoint: "/".to_string(),
            }],
            vec![],
        )
        .unwrap();

        let found: Vec<&str> = vfs.find_files("/Units", "*.bp").unwrap().collect();
        assert_eq!(found, vec!["/units/a/a_unit.bp", "/units/b/b_unit.bp"]);

        assert_eq!(vfs.file_info("/units/a/a.lua").map(|i| i.is_folder), Some(false));
        assert_eq!(vfs.file_info("/UNITS/a").map(|i| i.is_folder), Some(true));
        assert_eq!(vfs.file_info("/units/missing.bp"), None);
        assert_eq!(vfs.file_info("/unit"), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_processed_passes_through_unregistered_extension() {
        let dir = std::env::temp_dir().join(format!("vfs_test_raw_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("data.bin"), b"untouched").unwrap();

        let vfs = Vfs::new(
            vec![MountPoint {
                source: dir.clone(),
                mountpoint: "/".to_string(),
            }],
            vec![],
        )
        .unwrap();

        let out = vfs.read_processed("/data.bin").unwrap();
        assert_eq!(&*out, b"untouched");

        std::fs::remove_dir_all(&dir).ok();
    }
}
