# Crates

Every dependency in `Cargo.toml` and what it's for. Keep this in step when adding or removing one.

## Sim and maths
| Crate | Purpose |
|---|---|
| `fixed` | Fixed-point sim maths: `Fx` is `I32F32` (`src/engine/sim/math.rs`) |
| `libm` | Deterministic `sin` / `pow` / `log` / ... for the Luau VM (`src/engine/lua/det_math.rs`), instead of the platform's C library |
| `xxhash-rust` (XXH3) | Per-tick state checksums for desync detection, and the Lua code hash in the build id |
| `bitflags` | Flag sets (layers, footprint caps) |
| `glam` | Vector and matrix maths on the render side only, never in the sim |

## Lua
| Crate | Purpose |
|---|---|
| `mlua` (feature `luau`) | The Lua VMs. Luau itself comes from `vendor/luau0-src`, with our VM patches (`MOHO_PATCHES.md`) |
| `full_moon` | Parses Lua for the SupCom Lua → Luau translation passes (`src/engine/lua_translation`) |
| `lua-api-macros` (in `crates/`) | The typed Lua API's macros: `lua_fn!`, `lua_method!`, `callbacks!`, typed tables and enums |
| `inventory` | Declarations collected at link time (`callbacks!` blocks, `LuaTable` / `LuaEnum` types) for the generated API types |
| `miette` | Readable Lua error reports with source snippets (`src/engine/lua/error.rs`) |

## Files and assets
| Crate | Purpose |
|---|---|
| `zip` | `.scd` / `.nx2` archives (they're plain zip files) |
| `zstd` | `.zst` files in folder mounts (the audit map in `tests/vault`) |
| `memmap2` | Memory-mapped sound banks (streamed banks page in as they play) |
| `walkdir` | Walking folder mounts and sound folders |
| `globset` | Glob patterns over VFS paths |
| `texpresso` | Decoding DXT textures where the GPU has no BC support |
| `png` | Screenshots, and PNG textures |
| `serde`, `postcard` | Keyframe and replay serialisation |
| `serde_json` | The Lua debugger's Debug Adapter Protocol messages (`engine::lua::debugger`) |
| `bytemuck` | Zero-copy byte views for GPU uploads and hashing |

## Rendering and interface
| Crate | Purpose |
|---|---|
| `wgpu` | GPU rendering: Metal on macOS, Vulkan elsewhere |
| `winit` | Windows and the event loop |
| `pollster` | Blocking on wgpu's async set-up calls |
| `egui`, `egui-wgpu`, `egui-winit` | The debug windows |
| `egui_tracing` | The log panel (filters, search, colour by level) |
| `egui_ltreeview` | Tree views in the debug windows |
| `ab_glyph` | Font rendering for the SupCom interface's text (`src/ui/fonts.rs`) |
| `arboard` | Text clipboard (copy and paste in the interface) |
| `objc2-metal` (macOS) | GPU memory in use, for the status bar |
| `wesl` (build) | Compiles the WESL shaders in `src/render/shaders/` to WGSL at build time (`build.rs`) |

## Audio
| Crate | Purpose |
|---|---|
| `cpal` | Audio output (CoreAudio, WASAPI, ALSA). We only need a raw f32 stream: our own mixer plays the XACT banks |
| `rtrb` | Lock-free ring buffer between the audio thread and the device callback (no locks or allocation in the callback) |
| `cc` (build) | Builds `vendor/pl_mpeg`, which decodes SupCom's movies |

## Networking
| Crate | Purpose |
|---|---|
| `socket2`, `if-addrs` | The UDP sockets for LAN and direct connect, and each interface's broadcast address for LAN discovery |
| `iroh` | Internet play by join code: finding the host, relays and hole punching |
| `tokio` | The async runtime iroh needs, kept on the network thread (the sim stays synchronous) |
| `bytes` | Packet buffers for iroh |
| `data-encoding` | Base32 for join codes, and encoding in replay files |
| `reqwest`, `rustls` | Downloading a host's map from FAF's vault in the lobby, over the same TLS as iroh |

## Logging, profiling and the CLI
| Crate | Purpose |
|---|---|
| `tracing`, `tracing-subscriber`, `tracing-appender` | Structured logging to `game.log` |
| `tracing-tracy`, `tracy-client`, `tracy-client-sys`, `profiling` (optional, `--features tracy`) | Spans, frame marks and log events to the Tracy GUI. Pinned to the Tracy 0.13.1 protocol (Homebrew's `tracy`): bump them together with the GUI |
| `memory-stats` | Process memory, for the status bar and Tracy plots |
| `sysinfo` | The machine a log came from: OS, CPU, memory (`src/diagnostics.rs`) |
| `clap` | Command-line options (`src/cli.rs`) |
| `lazy-regex` | Compiled-once regexes (log parsing, audits) |
| `thiserror` | Error types |
| `rfd` | The native "where is Forged Alliance?" folder picker when the install isn't found (`src/install.rs`) |
| `libc` (unix), `windows-sys` (Windows) | Thread and process priority, watching the parent process, the terminal width |
| `criterion` (dev) | Benchmarks (`benches/engine.rs`) |

## Not used, on purpose
- **No physics engine** (`rapier3d`, `parry3d`): collision is our own fixed-point code (`src/engine/sim/collision.rs`), since floating-point physics wouldn't be deterministic across machines.
- **No ECS framework** (Bevy ECS, `hecs`): entities live in our own id-ordered storage (`docs/architecture/sim.md`, "Entity storage").
- **No JIT**: Luau's code generator and LuaJIT can't give bit-identical results on x86 and ARM.
