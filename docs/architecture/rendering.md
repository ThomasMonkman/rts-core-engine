# Rendering

## Stack
- **`wgpu`** — direct GPU access, no framework overhead
- **`winit`** — window + event loop
- **`egui`** via `egui-wgpu` + `egui-winit` — in-game overlay (logging UI, debug panels)

## Shaders (`src/render/shaders/*.wesl`)
- Written in [WESL](https://wesl-lang.dev) (WGSL plus imports). `build.rs` compiles each root module (`ROOTS`: bloom, models, particles, scene, shadows, terrain, ui) to `$OUT_DIR/<name>.wgsl` with the `wesl` crate, and each renderer module `include_str!`s its result; nothing compiles shaders at run time, and a shader error fails the build.
- `globals.wesl` is the shared library: bind group 0 (`Globals`, overlay, shadow map, horizon, environment cube, BRDF lookup) and the helpers (`sun_shadow`, `terrain_shadow`, `frame_rgb`). Scene shaders `import package::globals::{...}`; `render/globals.rs` keeps the matching Rust `Globals` struct, so change both together.
- A new shader file: add it to `ROOTS` if it's an entry module. WESL mangles names from imported modules in the output; entry points and the root's own names keep theirs.

## What's built (stage 1-2)
- `engine/snapshot.rs`: at the end of each tick, **only if a renderer attached a sink** (windowed mode; headless runs and tests never do), the sim packs live entities (position, rotation, collision box, army, health, build fraction, blueprint index), the terrain heights when they change, and blueprint names once, and `try_send`s them on a 4-deep channel. A slow or hidden renderer drops snapshots; the sim never blocks.
- `render/scene.rs`: 
    - terrain heightfield (every sample up to 20 km maps, every n-th on bigger ones to keep it ~1024²
    - levels of detail and tiles below
    - height/slope colouring until the map's textures load)
    - translucent water plane, every unit and prop as its model (below), or, until that loads or without one, an instanced box (its collision volume) in army colour; structures darker, constructions translucent, projectiles yellow, props olive. 
    - Army colours come from the sim (`Snapshot::army_colors`: the session's `GameColors` picks, then `SetArmyColor` / `SetArmyColorIndex`), kept as `Scene::army_rgb`.
    - Entities are interpolated between the last two snapshots (merge-walk by id; lerp + slerp), one tick of latency.
- `render/camera.rs`: RTS camera (WASD/arrows, wheel zoom with tilt, middle-drag pan, hold Space and move the mouse to turn and tilt it while zoomed in closer than 125, eased back on letting go unless `cam_Free` (FAF's Ctrl-V) is on; Insert / Delete turn it and it stays turned until the view is reset (FAF's V) or the interface reverts it). 
    - Zoom is moho's: the ground across the view's longer side, which the field of view also spans.
- egui: status bar (FPS, tick, entity counts), Logs and VFS as collapsible windows; the windows hidden until Ctrl+` (`RenderState::show_debug`), the bar shown with its own toggle at the bottom right (`show_status`).
- Stage 3, debug views
    - left-click picks an entity (camera ray vs its box)
    - the **Selected** window shows blueprint, army, layer, speed, health/build, focus, its order queue and weapon states, with its path drawn. 
    - **Overlays** window: unit paths, weapon ranges, nav grid per layer (impassable tinted red), threat map as seen by an army (heat over the terrain). 
    - The renderer asks for these through a shared `DebugRequest`; `publish` reads it and adds `DebugData` to the snapshot (read-only over the sim; it shapes the snapshot only). 
    - Health/build bars over damaged or unfinished units when zoomed in.
- Status bar: FPS, **sim ms/tick and achieved ticks/s**, game speed (pause, 0.25x..16x, max), tick, entity counts. Keys: `+`/`-` speed (Space doesn't pause: too easy to hit by accident). `SimControl` is shared with the real-time loop and only paces wall-clock time between ticks (determinism unaffected); interpolation uses the measured snapshot interval.
- Dev flags: `--speed <x|max>`, `--overlay paths,ranges,nav=land|hover|naval|amph,threat=<army>`, `--select <id>`, `--camera x,z,zoom`; `--screenshot <png> --screenshot-tick <n>` saves a frame once the sim reaches tick n and exits (visual checks without screen-recording permission).

## Sim/render separation
- Sim runs at default 10Hz tick rate
- Render runs at its own rate: with Vertical Sync on (FAF's default) at the display's refresh rate (`Fifo`, `Mailbox` on Wayland); with it off, no faster than FAF's Frametime option allows (`sc_FrameTimeClamp`, 2-16 ms a frame, 8 by default: ~125 fps), or uncapped if that's unset. A hidden window and offscreen runs are paced slower
- Render thread holds a **snapshot** of sim state; never writes back
- Entity positions are interpolated/extrapolated between sim ticks on the render side

- Snapshot cost on the sim thread: a snapshot is built only when the channel has a free slot (each snapshot holds a `Ticket` until the renderer takes it), so game speeds above the frame rate don't build snapshots that would be dropped; one-off data and `Sync` tables go with the next one. Static props are sent once (`Snapshot::props`) and then as changes (`prop_changes`: props spawned, destroyed, moved or reached through `EntityWorld::get_mut`); `entities` carries everything else plus posed props, and `Scene::receive` merges its prop copy back in by id, so the rest of the renderer sees one id-ordered list.

## The frame (`render/frame.rs` `render_frame`, `render/present.rs`, `render/scene.rs`)

Two threads draw the window. The **main thread** (winit requires it on macOS) has the window's events, input, the interface's Lua VM and egui, and the main side of the world (`render/world.rs` `World`: the snapshots' data, the cameras and views, picking against the last frame's entities). The **present thread** (`present::PresentThread`, named `present`) owns the surface and everything on the GPU (`present::Presenter`: the `Scene`, the interface's and egui's passes, the GPU profiler) and is paced by its swapchain: it never waits on the main thread's work for long, and the main thread never waits on vsync.

Each frame the present thread asks the main thread for it (a winit user event, `UserEvent::PresentReady`; at most one outstanding). The main thread takes the snapshots (`World::receive`, which keeps their data and passes what the GPU needs on as `SceneDelta`s, in order), handles input and moves the cameras, and sends the frame's first half (`present::Early`: the deltas and the frame's input, `FrameInput`). The present thread applies them and starts the heavy prepare (`Stage::run`: interpolation, posing, particles) on the render pool, while the main thread runs the interface's Lua and egui and sends the second half (`present::Late`: the input with the interface's part, its draw list, egui's output). The present thread then finishes the prepare, lays out the strategic icons and world overlays at the frame's interpolated positions (`render/overlays.rs`: lifebars, ETAs, the drag box; they stay on their units), encodes, finishes the encoders in parallel, submits and presents, and leaves what the main thread reads (`PresentOut`: the frame's entities, icons' rectangles for picking, the bounds of meshes it loaded for picking and drag selection, timings) for its next frame. A half that doesn't come within 50 ms (`MAIN_WAIT`: a Lua hitch, the interface starting up) is drawn from the last frame's again, interpolated afresh, so the picture keeps moving. Settings that need the device (shadow map size, texture detail), the window's size, vsync and the interface's images go over the same channel as commands (`present::Command`).

## World views (`camera::View`, `World::set_views`, `Scene::draw`)

The scene draws one or more world views, each a named camera (`World::cameras`: `WorldCamera`, FAF's split view's `WorldCamera2`) in a rectangle of the frame, from the interface's world view controls (docs/architecture/ui.md, Cameras and world views). With one view the frame is what it was before views (the golden images are unchanged).

- Shared once a frame (the stage, on the worker): interpolation, the pose pass and instances, particles' emission. An entity is culled only when outside every view's frustum and drawn at the finest LOD any view needs (the nearest on its LOD plane); strategic icons are decided per view (`Plan::icons`, at most `MAX_VIEWS`), so an entity past its last cutoff in one view shows its icon there while another view draws its mesh. Emitters emit when in any view (`particles::Sight`).
- Per view (`Scene::prepare_view`, into the view's own `ViewGpu`): the terrain's detail and tiles, decal culling (`terrain::DecalDraws`), order lines (`command_graph::GraphFrame`), deposit icons (`resource_icons::IconsFrame`), range overlays (`ranges::RangesFrame`), shadow cascades, their light matrices (`shadows::Lights`) and casters, and the shaders' globals with the view's group 0. Every view is prepared before any draws, and all views' encoders finish in parallel with the rest of the frame's (no submit between views). The screen-sized targets (depth, terrain normals, range mask, bloom, SMAA) and the shadow maps are shared: the GPU runs one view's passes after the other's. Particles face each view's camera from its globals (`particles.wesl` `cam_right`/`cam_up`), so they need nothing per view. Every pass of a view sets its viewport and scissor (`View::bind`), including the terrain normals and range overlay passes (the range shader takes the view's origin); the first view clears the frame and later ones load it. Bloom and SMAA run once over the whole frame.
- An entity's LOD comes from the nearest view that can see it (not the nearest of all views), so a zoomed-out view beside a close one doesn't give every unit on the map its full mesh.
- Not per view yet: beams and ribbons face the first view's camera (built on the CPU with its eye); wave emission follows the first view.

## The second head (`state::SecondHead`, `frame::second_frame`)

FAF's dual-head mode: root frame 1 (`GetFrame(1)`, where FAF's `multihead.CreateSecondView` puts a `CameraHead2` world view) shows in a second window with its own present thread (`present (head 2)`), surface and `Scene`, paced by its own display's vsync, so a 60 Hz and a 120 Hz screen each run at their own rate. Which head, decided once (moho reads Secondary Adapter as it makes the device): `--second-head windowed|offscreen|w,h,hz`, else the profile's `secondary_adapter` (`windowed` or `w,h,hz`; its choices are Windowed, the engine's own, and the other monitor's modes, `console::list_display_modes`), read as the first interface starts. `w,h,hz` is borderless over a monitor other than the main window's, at that monitor's own mode (macOS: simple fullscreen, not a Space of its own); `windowed` a second window on this display; `offscreen` no window (`--screenshot x.png` also saves `x-head2.png`, before the main one).

- The interface runs once per main frame; the second head's frames (`UserEvent::PresentReady(1)`) take the snapshots since its last (each head's scene gets every `SceneDelta`; one decided after the first snapshots gets those stashed before), the main frame's latest input with head 1's views (`World::fill_input(1, ..)`) and root frame 1's draw list, both halves at once. Each head steps the cameras it shows, at its own rate (`frame::update_camera`, `camera_head`): a camera only head 2 shows (`CameraHead2`) moves in head 2's frames (the wheel, a drag, a timed `MoveTo`), every other camera in the main frames (the keys and a followed unit move the main camera); the wheel and a drag wait for the frame of the head the cursor is on. The interface reads the cameras once per main frame.
- The cursor is on one head at a time (`RenderState::cursor_head`, from the window it moved on): picks, the view under a point, the wheel and drag (`World::set_active_head`), and the interface's hit tests (`UserVm::set_mouse_head`) are that head's. Keys are shared. egui's windows are the main window's only.
- Both heads' models load their textures through one cache (`texture_load::Cache`, `Models::share_textures`): each texture is in GPU memory once. The rest is the head's own: the screen-sized targets, shadow maps, the map's textures, mesh buffers, particles.
- Either window can be hidden without stopping the other. Both heads share one device, and an acquire or present waiting on a hidden window's vblank holds it. A window winit reports covered (`Occluded`: macOS, X11) or minimised (resized to nothing: Windows) isn't asked for drawables (`Command::Hidden`, `RenderState::apply_visibility`); its frames draw offscreen, at 10 Hz, except a hidden main window while head 2 shows: the interface runs in its frames, so they keep head 2's refresh. Wayland reports neither, and Mesa's Fifo present waits for the frame callback a covered window never gets, so on Wayland vsync is Mailbox where offered, paced by the present thread to the monitor's refresh (`state::present_mode`, `Presenter::pace`).
- Not yet: the minimap and the interface's engine images (`Command::Image`) draw on the main window only.

## GPU captures (Xcode, RenderDoc, Nsight)

Every wgpu object is labelled, and the labels reach the backend, so a frame capture reads as the pipeline. On Vulkan they reach the driver as markers only with `WGPU_DEBUG=1` (wgpu's environment variables apply, `InstanceDescriptor::new_without_display_handle_from_env`), which also puts debug info in every shader; under Nsight Graphics' GPU Trace that combination has lost the device, so for per-pass times use `--gpu-passes` (below). Names are `system: what` (`terrain: vertices`, `bloom: blur horizontal`, `models: opaque`). A texture loaded from the VFS carries its role and path (`albedo /units/uel0201/uel0201_albedo.dds`, `map: stratum 3 albedo /env/...`, `decal /env/common/decals/scorch_001_albedo.dds`); its upload encoder is `upload: ...` and its staging buffer `staging: ...`. Views are labelled like their textures (`render::view`), since wgpu doesn't carry a texture's label to its views.

A frame's command buffers, in submit order:
- `frame: start (GPU timing)`: the start of the GPU timing bracket (`GPU timing: frame start`, on adapters with timestamp queries; `GPU timing: frame end` closes it in the last buffer).
- `frame: shadows + terrain normals`: one pass per sun cascade (`shadows: cascade 0 (near)` ...), then `terrain: normals + depth` (groups `terrain`, `decals: map`, `decals: sim (scorch marks, ...)`, `decals: interface`), then the ground-depth copy for the range overlays.
- `frame: world`: the `world` pass, in debug groups in draw order: `sky`, `terrain` (then the albedo decals and `splats`), `boxes (models still loading) + icons`, `models`, `particles: underwater`, `water`, `models: shields + effect techniques`, then (after `ranges: mask` and `ranges: composite`, or the refraction copy, each resuming the pass as `world (after ...)`) `particles: beams`, `particles`, `particles: lights`, `ghosts: queued builds`, `resource icons`, `command graph (order lines)`, `ghosts: placement`.
- `frame: post (bloom, SMAA)`: group `bloom` (`bloom: copy glow`, the blur passes), `bloom: add to frame`, then `SMAA: edges`, `SMAA: blend weights`, `SMAA: resolve`.
- `frame: overlays + interface + debug windows`: `world overlays (selection, drag box)`, `interface`, `debug windows (egui)`.

A model's texture bind group is named by its mesh (`model textures: /units/uel0201/uel0201_lod0.scm`), so selecting a draw shows what it is. Draws aren't wrapped in a debug group or marker each: that makes `CommandEncoder::finish` noticeably slower, mostly in the shadow cascades, which draw the same meshes again.

### GPU time per pass (`--gpu-passes`)

`--gpu-passes` timestamps every pass (its `timestamp_writes`, `gpu_prof::pass_writes`) and, on adapters with `TIMESTAMP_QUERY_INSIDE_PASSES`, every debug group inside one (`gpu_prof::Timed::timed_push` / `timed_pop` in place of `push_debug_group` / `pop_debug_group`), from a query set of its own, and every 120 timed frames logs the mean GPU ms of each per frame (target `gpu_passes`): passes in frame order with their groups indented under them, a group's time including its nested groups', `(xN)` where it ran N times a frame. Recording is on the present thread (a thread-local recorder per frame); readback follows the frame timing's ring, so it never stalls. Exact on NVIDIA and AMD. On Apple GPUs passes overlap and stamps lag, so none of it is sound there: the first heavy pass (`world`) reads about right, later passes absorb the time they wait behind it, and the frame total reads low. On a Mac use Instruments' Metal System Trace (`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcrun xctrace record --template 'Metal System Trace' --launch -- ...`, run from the repo: game paths are relative) for per-pass time, and an Xcode frame capture (Debug > Debug Executable, GPU Frame Capture: Metal, validation off, then Profile) for per-draw and per-debug-group time. `--window-size WxH` gives the main window that many pixels whatever the display and profile (the profile's display mode isn't applied), to measure at another machine's resolution.

## Map terrain (`render/terrain.rs`)
FA/FAF's terrain as `effects/terrain.fx` draws it, from the map's `.scmap` (read by the renderer: snapshots carry the map path; `assets::scmap::read_render` gives shader name, lighting, water, strata, decals and the mask / water-map blobs). Reference: FAF's `effects/terrain.fx` / `water2.fx` and the FAF wiki pages "FAF Terrain Shaders" and "Mapping" (`../faf-wiki/Development/Shaders/terrain-shaders.md`, `Development/Mapping.md`).
- Two passes, like FA: a **normals pass** blends the strata normal maps (UDN, onto the heightfield normal or FAF's terrain normal texture) into a screen-sized `Rgba16Float` buffer, and normal decals blend into it (normal from `.ag`, weight `.r`); the **albedo pass** blends the strata albedos, reads the normal back at its pixel and lights it; albedo decals then draw lit the same way, skipping the lighting of pixels they leave unchanged (alpha 0: the texture's clear parts, outside its square, faded out by distance). The normals pass also writes a **light buffer** (`Rgba16Float`, `light_at`): the terrain's shadow (the sun's shadow map x its own), ambient occlusion and water depth a pixel, which the terrain, albedo decals and tread marks read instead of each taking the shadow map's nine samples again; decals stack several deep, so this saves most of their lighting cost. The shadow's normal offset uses the terrain's normal before normal decals. Units, water and overlays follow in the scene pass.
- Shader name -> features (`ShaderKind::parse`): `TTerrain` 4 strata, half-range albedo masks (`saturate(m*2-1)`), white specular from `1 - albedo.a`; `TTerrainXP` 8 strata, specular `albedo.a * SpecularColor`; FAF `TTerrainXPExt` / `TerrainABC[B]`: stratum 7 slots = terrain info (shadow `a`, AO `g`) and terrain normal (`.ag`) textures, A = 0 legacy / 1 PBR / 2 PBR + height-based splatting (2x2 atlas in the upper slot: roughness `x`/`z`, height `y`/`w`; even strata rotated by `SpecularColor.ba`; blur / contrast from `SpecularColor.rg`), B = 0 full / 5 half mask range, C = lower albedo as macro (1 lerp, 2 multiply); stratum 7's mask varies roughness. The `B` (biplanar) variants draw as their planar base.
- Tiling: a stratum repeats `map size / scale` times across the map.
- Lighting `LightingMultiplier * (SunColor * N.L * shadow + SunAmbience * AO) + ShadowFill * (1 - light)` in gamma space (textures sampled UNORM, the result converted for the sRGB target). Under water the ground is tinted from the water ramp at the water map's depth (`.g`); exponential absorption when the lighting multiplier is >= 2.2 (FAF's switch).
- Water: four scrolling wave normal maps, `lerp(ground, waterColor, clamp(depth, lerp.min, lerp.max))`, then toward the sky cube reflection by Schlick fresnel (F0 0.08, `SkyReflection`), plus the sun glint; drawn premultiplied over the already-tinted ground (no screen refraction offset).
- Terrain mesh and levels of detail (`Scene::upload_terrain`, `terrain::lod_indices`): one vertex buffer of every sample (maps up to 1024 cells; bigger ones every n-th), and an index buffer per level (every 1st, 2nd, 4th, 8th vertex, the same triangulation), each laid out in tiles of 64 cells of the finest. Each frame the coarsest level whose cells are at most 3 pixels across at the camera's target is drawn (moho chooses its terrain's detail by screen error, `ren_ViewError`), and only the tiles in the frustum (adjacent runs merged into one draw). The heights also go to a storage buffer (group 0 binding 10, `heights` / `terrain_vertex` in `globals.wesl`) with the level's step in `Globals::terrain`.
- Decals (`ALBEDO`, `NORMALS`, `NORMALS_ALPHA`): drawn as moho does (`terrain.fx` `DecalsVS`), on the terrain mesh's own cells under the decal's footprint with the texture projected onto them: clamped (FA's decal samplers) and nothing outside its 0..1 square. A decal is an instance (`terrain::DecalInstance`, 80 bytes: origin, the matrix's u and v rows over the scale, its footprint in samples on the mesh grid, heading, cutoff); `vs_decal` lays its grid at this frame's level from the height buffer exactly as the terrain's index buffer lays the cells (the same floats, triangulation and `@invariant` clip position), so it passes `LessEqual` with no lift or bias, and collapses vertices past its grid and cells wholly off the texture (tested on x and z alone for untilted decals, which is nearly all). Decals bigger than a tile are cut into tile pieces, each culled on its own; a run of consecutive pieces with the same texture and pass is one instanced draw while their grids stay within 2x of each other (each draws the largest). An albedo decal's second texture is its spec map (bind group = texture + spec, a black stand-in without one): TTerrain lights it with the map's red (FA's `DecalsPS`), XP with its alpha (`DecalAlbedoXP`), PBR's roughness is `0.9 (1 - red)`; none means no specular. Script decals' second texture (FAF's tarmac `Albedo2`) is the same. Built once in file order with a bounding sphere each. Each frame (`Terrain::cull_decals`, in the prepare's finish) the ones inside the camera frustum and not wholly beyond their `LODCutoff` along the view make the draw list, consecutive ones with the same texture and pass merged into one call; the cutoff fades over its last quarter in the vertex shader (view depth, like the timed splats: FAF sets moho's `ren_DecalFadeFraction` to 0.25, so they no longer pop; `ren_ClipDecalLevel` 4 is not applied, its meaning unverified; the sim's `CreateDecal` cutoff is not carried yet, those draw at any distance). The decal position is its corner (tarmacs placed in maps centre on half-grid building positions that way). Its texture matrix is moho's (`CWldTerrainDecal::Update`, MohoEngine.dll 0x1045fb50; `terrain::decal_matrix`): the offset from the corner turned by `rotation.y`, then `x`, then `z` (gpggal's `D3DXMatrixRotation*`, row vectors), uv = local (x, z) / scale, so a tilted decal projects along its turned up axis. The footprint (`ComputeCorner`, 0x1045f8b0; `terrain::decal_corner`) and the normal decals' tangent frame turn by the heading alone: u along (cos, sin), v along (-sin, cos). Checked against the maps' own preview images (the editor's render).
- Script decals: the sim's `CreateDecal` / `CreateSplat` (FAF's structure tarmacs, resource deposit markers, scorches and craters) reach the renderer as map decals whenever the set changes (`Snapshot::decals`, keyed by `Presentation::decal_version`; centre positions turned into corners, relative texture names under `/env/common/decals/`, `.dds` added). They draw over the map's decals with the same pipelines. Textures are read and decoded on the global pool the first time a decal needs one (decoding on the render thread would hitch frames); a decal waits, undrawn, until its texture is uploaded, and the layer is rebuilt then. Decals and splats with a `duration` expire after it (sim end of tick). The army argument (visibility) and LOD are ignored: every decal draws for everyone at any distance.
- Textures (strata, masks, water map, ramp, waves, sky cube, decal textures) load on a background thread at map load and upload once; until then the scene keeps the plain height-coloured terrain. Needs 30 sampled textures per stage (requested up to the adapter's limit; desktop GPUs allow >= 32).
- Not yet: the terrain casting into the shadow map (moho's `TTerrainDepth`; ours bakes a horizon map), glow and water-albedo decals, skybox, biplanar mapping, FA's screen-space refraction, decal mask textures (`DecalMaskTexture`).
- Check against the `.scmap`'s own preview image (the editor's top-down render, first blob in the file).

## Splats: tread marks, footprints (`render/terrain.rs` `lay_splats`)
Moho calls them splats (`CreateSplat`, `CreateSplatOnBone`; drawn by `TSplats`). FAF lays most
of them itself: tread marks from `TreadComponent` (a thread per
`Treads.TreadMarks` entry calling `CreateSplatOnBone` every `TreadMarksInterval`, life
`TreadLifeTime` or 1 s; it starts only once the engine has reported the terrain type the unit
is on, `OnTerrainTypeChange`), footprints from `OnAnimCollision`. Timed splats skip the decal
layer and come with the next snapshot (`Snapshot::splats`); the terrain renderer puts them in a
ring of 16384 instances per texture (`SplatInstance`: centre, half extents, heading, birth,
lifetime, LOD distance), written where its head is (nothing rebuilt per frame), drawn after the
albedo decals and lit like them (FA's `TSplats`). Like a decal, `vs_splat` lays each mark on the
terrain's own cells under it from the height buffer (every instance draws the grid the ring's
largest mark needs at this frame's step; the rest collapse), so marks follow the ground exactly
It fades a mark as moho does (opaque, eased
out over the last quarter of its life in game seconds `g.params.z`; faded over the last quarter
of its LOD camera depth, `GetLODAlpha`) and collapses marks not born yet, expired or out of
range. No
marks under water. Splat sizes are full extents (moho's `CreateSplat` places the corner at the
centre minus half the size), so small units' tracks (engineers: 0.55 x 0.4 of a two-strip
texture) are faint by design.
A splat whose texture is still loading waits (`Splats::waiting`, retried each frame) rather than
being dropped: its own request starts the load, so a one-off splat would otherwise never show.
It is dropped if the texture fails or its lifetime runs out first.

## Shoreline waves (`render/particles.rs` `emit_waves`)
The `.scmap`'s wave generators (`scmap::WaveGenerator`, field order as moho's
`WaveGenerator::Load`, MohoEngine.dll 0x1044b890; Seton's Clutch has 6939 along its coasts) lay
the foam that rolls onto beaches. Moho's `WaveSystem::Update` (0x1044d0a0) does nothing while
its float argument exceeds 200 (read here as the camera's zoom distance; inferred), and every
5th frame picks the generators in the view within `efx_WaveCutoff` (400), at most 2048;
`WaveGenerator::Update` (0x1044bcd0) lays one particle once its period (a random pick from its
range) has passed since the last, then draws the next. The particle (`TRampAnimateFlat`,
blend 3 additive: the record constructor's default, 0x102410d0) lies flat on the water at the
generator's position turned by its rotation, drifts at its velocity (per tick), lives a random
lifetime (ticks), grows from scale.0 to scale.1 (half-size), frame and strip as the generator
says, coloured over its life by the ramp (`waveramptest.dds` on Seton's). They go into the
ordinary particle batches, above the water. Moho times periods on the wall clock
(`gpg::time::Timer::ElapsedSeconds`); ours on the particle clock (ticks / 10), so a paused
game doesn't pile waves up and they follow game speed.

## Command graph (`render/command_graph.rs`)
The selected units' queued orders, as moho draws them: order lines (unit to
its first waypoint, then waypoint to waypoint) and a waypoint icon per order.
Only the viewing player's side's orders show (allies included): a selected
enemy has no command graph, inspector path or queue for the interface;
observers see every army's.
No arrowheads: FAF's params name arrowhead textures, but the base game draws
none on these lines. A mobile build's site is outlined at its structure's
build skirt (`Physics.SkirtOffset/SkirtSize`, `OrderMarker::site`), 2 px
wide: green while a selected unit is on it (its current order), blue while
queued (both colours sampled from the base game).

Lines are straight and turn tightly at each waypoint on a chain (the first
unit's chain, for a shared line): the corner is rounded (a quadratic through
the waypoint) from `ui_CurveSmoothness` (35) pixels before it to 35 after, at
most half of either line, in up to `ui_CurveSegments` (32) pieces: one per
4 px of the turn's radius on screen (`CURVE_PX_PER_SEGMENT`; past that they
are under a pixel apart); each line draws its half of the turn.
Queued builds whose site has no structure yet show their model as a green
ghost (FAF's `UnitPlace` technique, the placement ghost's), under the lines.
Each waypoint has its ETA at the icon's upper right, `ETA: %d.%02d` (minutes,
seconds, rounded down; the binary's format, FAF's `display_eta` option),
worked out as moho's command graph does (`snapshot::order_etas`;
MohoEngine.dll 0x103ef7c0): a waypoint is reached at the latest of its
incoming lines (from a unit, or from the waypoint before at that one's
ETA) plus the line's travel at the slowest top speed of the order's units;
a build order then adds what's left of the structure at the summed
`BuildRate / BuildTime` of its ENGINEER / FACTORY units (`GetConstructEconomyModel`).
The economy isn't counted: a stalled build's ETA holds still. Ours also
treats one structure as one job: repair orders helping build it add
their units' rates and share the build's ETA, units already at work on it
have arrived, and labels with the same time on the same spot draw once. A
waypoint with no time to it has no label. Patrols close their loop: a line from the
last patrol waypoint back to the first (the sim cycles them). Units sharing an order
share its waypoint (`OrderMarker::command` / `node`). The engine draws these;
FAF's `/lua/ui/game/commandgraphparams.lua` only describes them: per
`UNITCOMMAND_*` the waypoint and line textures, the line's UV
aspect and scroll rate, normal / selected / highlight colours and glow, with
`inherit_from` chains and a `default` (read once on the global pool). Orders
whose entry has no waypoint texture (stop, upgrade, factory builds, silo
builds) aren't drawn, as that file says.

Everything lies on the ground like a splat: terrain-following quads (bilinear
heights, lifted 0.25), drawn over the models without depth testing (in the
base game a build line crosses the factory it builds), `texture * colour` with the glow where the texture shows
(`primbatcher.fx` `TCommand` / `TCommandGlow`, the glow as a max into the
glow target). A line is FAF's `CalculateWaypointLineWidth` (`sqrt(units on
the line)`, at most 10) world units wide, at least 6 px when zoomed out (the
texture's band is about half the quad; measured against a base-game
screenshot beside a factory); the texture repeats
every `width / orderline_uv_aspect_ratio` and scrolls toward the waypoint at
`orderline_anim_rate`. Waypoints are 1.25 world units, held between
`ui_MinWaypointSize` and `ui_MaxWaypointSize` (7 and 100 px,
`commandwaypoint.lua`), north up; the one under the cursor takes the highlight
colours. Lines are cut into ground-following pieces every 2 units, from a
budget of 50k pieces a frame shared over the lines (4 to 64 each).
A waypoint under 32 px is one quad (its corners on the ground) rather than a
4x4 ground-following grid: drawn over everything, so small an icon's grid
moves it by under a pixel. Lines and waypoints wholly off the view (turns,
width and icon size included) aren't built.

Spread orders (FAF's spread attack, or units spread out) give every unit its
own waypoints and lines, which is a lot of geometry with thousands of units
selected (`spread_orders_command_graph_images` holds the look at two zooms).
Each texture's batch is kept between frames and uploaded from where it was
built, with no copy into one array. Not done yet: indexed line strips and a
smaller vertex (the colour as bytes) would about halve the line data again.


## Selection brackets (`render/command_graph.rs` `Bracket`)
Each selected unit gets FA's `selection_brackets_player` texture (four
corners) as moho draws it (MohoEngine.dll 0x103c72f0; see
verifying-against-the-binary.md): a quarter of the texture at each corner of the
unit's half extents (`SelectionSizeX/Z` x `ren_UnitSelectionScale` 0.75;
the collision box x `ren_SelectionSizeFudge` 1.85 without them), each a
square of half size `SelectionThickness` (0.2 when unset) x the larger
extent, at least 3 px, centred on its corner; turned with the unit, at its
position + its rotation x `SelectionCenterOffsetX/Y/Z`, +
`ren_SelectionHeightFudge` (0.12), white (`ren_SelectColor`), depth-tested
so the unit stands over it (`Snapshot::blueprint_selection`). Drawn with the
command graph, with its own depth-tested pipeline, instanced: one 48-byte
record per selected unit (`BracketInstance`: centre, piece size, rotation,
half extents) and `command_graph.wesl` `vs_bracket` builds the 24 vertices
(`bracket_pieces` is its tested reference), so a selection of thousands of
units costs one small record each rather than CPU-built vertices.

## Sun shadows (`render/shadows.rs`, `render/globals.rs`)
One shadow system for everything: every model casts into a cascaded sun shadow map, and every receiver (terrain, decals, models, water glint) samples it through `sun_shadow()` in the shared group-0 prelude (`globals::PRELUDE`, which all scene shaders prepend so their uniform declarations can't drift).
- 3 cascades, each `ren_ShadowSize` texels square (FAF's Shadow Resolution, 512-2048, default 1024; 2048 without a profile) (Depth32Float array), split mostly logarithmically over the view depth that can hold ground (from where the view first meets the highest terrain to where it last meets the lowest), out to `ren_ShadowLOD` at most. Each is an orthographic sun view around its slice's bounding sphere, snapped to whole texels (no shimmer when panning). Receivers pick the cascade by view depth, offset along the normal, and filter 3x3 PCF (comparison sampler).
- Casters are culled per cascade (only those in its column along the sun); cutout meshes (foliage) alpha-test in the caster pass; effect and glow meshes don't cast.
- The terrain doesn't cast: its self-shadow is a horizon map baked from the heightfield at map load (march toward the sun, steepest rise vs the sun's elevation, a slope band for soft edges; 1 texel per cell), read bilinearly (`terrain_shadow`; read nearest, a small shadow showed as hard-edged world-unit blocks close up). FA draws the terrain into its shadow map instead (`TTerrainDepth`). Terrain uses it unless the map's FAF shader ships a baked shadow (info texture); models sample it at their position, so mountains shade units.
- The `shadow_cost` test measures what shadows add to a frame. Per-cascade culling is what keeps it small: without it every cascade draws every tree.
- `Scene::shadows_enabled` turns them off.

## Particle effects (`render/particles.rs`)
FA's emitters, light flashes, poly trails and beams (`effects/particle.fx`; `CEfxEmitter` in MohoEngine.dll; FAF `engine/Core/Blueprints/*Blueprint.lua`; wiki "Modding Emitters").
- Sim side: `api/effect.rs` records each `moho.IEffect` in `Presentation` (never read back by the sim). Effects end when Lua destroys them, when their blueprint `Lifetime` ticks (or a `LIFETIME` parameter) run out (an emitter of lifetime L emits ceil(L) ticks, as moho), or with the entity they're attached to / beam to (`effect::expire`, end of tick, id order). Snapshots carry the live effects (`EffectView`: blueprint, spawn entity + bone, rest-pose anchor fallback, beam end (NaN when the sim knows none: the renderer uses the blueprint `Length` along the bone), scale, parameter and curve overrides; collision beams end at the beam's hit point).
- Blueprints parse without Lua (`assets::effect_bp`, every FAF effect blueprint parses), every field moho reads: the curves, the flags (`EmitIfVisible`, `CatchupEmit`, `CreateIfVisible`, `SnapToWaterline`, `OnlyEmitOnWater`, `ParticleResistance`, ...), the fidelity flags, `SortOrder`, `LODCutoff`.
- **Moho's model, which the renderer follows: the CPU only emits; the GPU evaluates.** Each sim tick an emitter samples its curves at that point of its repeat cycle (value +- range/2, moho's global random stream stands in for a per-emitter xorshift) and writes one 96-byte record per new particle (FA's instanced vertex: birth position, velocity, acceleration, half-size and its growth per tick, rotation and rate, birth tick, lifetime, frame rate, strip and ramp row, drag coefficients, flags). The record is uploaded once into a pooled storage buffer and never touched again: `particles.wesl` (a port of `WorldVS` / `WorldVSAlign`) evaluates position `p + v t + a t^2/2` (or the closed-form drag solution with `ParticleResistance`), size, rotation, the billboard (camera-facing, `Flat` on the world XZ plane, `AlignRotation` along the direction of travel, `AlignToBone` along the bone's axis without moving), the texture frame / strip and the ramp coordinate (age over lifetime) from the record and the clock. Dead particles collapse to a degenerate triangle; ones born later in the tick are drawn run backwards from their birth, as FA does. Per-frame CPU cost is emission plus placing the emitters, not the live particles.
- Clock: sim ticks. The renderer shows snapshot `T` at `T - 1 + alpha`, so a tick's births are spread over `T - 1 .. T` (moho's `InterpolateEmission`: the k-th of n at k/n, curves sampled there, position interpolated between where the anchor was at the tick's start and where it will be at its end, from the sim's per-tick displacement). Effects follow game speed and pause; particles outlive their emitter.
- Emission and orientation follow `CEfxEmitter::Emit` (`MohoEngine.dll` 0x102426b0): position curves in the bone's frame, the `Size` spread on the world XZ plane (uniform distance over +-Size/2); direction / acceleration rotated by the bone only with `LocalVelocity` / `LocalAcceleration`; gravity -0.02 units/tick^2 after. `EmitRate` is per tick; a negative rate owes nothing. `SnapToWaterline` clamps the birth above the water for batches sorted at or above `efx_ParticleWaterSurface` (-101), below it for the rest; `OnlyEmitOnWater` drops births over land.
- Visibility (moho's emitter tick): an emitter with `EmitIfVisible` emits only while in the frustum and, with `LODCutoff > 0`, within it of the camera plane (`<= 0` never cuts); off screen it counts the missed ticks and, back on screen with `CatchupEmit`, emits them backdated (bounded by its particles' longest life, since older ones would be dead, and a per-frame budget of 50k records that carries the rest over). `CreateIfVisible` emitters created off screen never emit. Emitters whose fidelity flag for `Particles::fidelity` (high by default) is off don't show.
- Storage: fixed 256-record segments of one storage buffer (grown by doubling up to 262,144 records; moho logs "Particle cap exceeded" past its cap, so do we) owned by batches (sort order, kind, blend, texture, ramp); a segment is freed when every particle in it has died.
- Drawing: one instanced draw per segment (six vertices, the particle index from the instance index). Batches draw by `SortOrder`, then blend mode (modulate, alpha, premultiplied, additive); those sorted below -101 draw before the water surface (torpedo wakes, bubbles), the rest after it; light particles (`CreateLightParticle`: FA's `TLight`, a flat additive quad of the given size and lifetime, depth test off) last. A pipeline per blend mode (0 alpha, 1 modulate inverse, 2 modulate 2x inverse, 3 add `SrcAlpha, One`, 4 premultiplied; beams' add is `One, One` like FA's `TBeam_*_ADD`; 5 refract: `WorldRefractPS`, the frame behind nudged by `0.005 * (2 * texture.rg - 1)`, read from a copy the scene takes before particles draw, only on frames that have refracting particles). Depth test, no depth write, no culling. Blending happens in the gamma-space frame (see Frame and bloom), as in D3D9 FA. Particle techniques write colour only (FA's `Write_RGB`); beams and lights write their alpha into the glow too (`Write_RGBA`), so lasers and flashes bloom. Emitter textures wrap along u and clamp along v (strips, FA's `ParticleSampler0`); beam and trail textures wrap both ways (`ParticleSampler0Wrap`, which `BeamPS` and `TrailPS` sample), so a beam's `VShift` scroll and `RepeatRate` repeat the texture along it (clamped, the Seraphim build beams scrolled onto their texture's black edge row and vanished, and the Aeon ones lost the dark debris specks running along them); ramps clamp. Visual check: `cargo run -- --no-ai --script dev/scenes/visual/build_beams.lua --camera 394,610,36`.
- Trails (moho's `CEfxTrailEmitter`) are records too: one segment per sim tick from where the bone was at the tick's start to where it will be at its end (the sim's displacement), each fading over `TrailLength` ticks from its own end times; `vs_trail` extrudes `Size` to each side across the view, the ramp's u is the age over the fade (the fragment drops what's outside it, so a segment grows in over its tick) and its v the side, the repeat texture's v is `TextureRepeatRate` x the distance along the trail. Segments are stamped with the tick's own window on the render clock (tick - 1 to tick), never the clock's floor, which is the tick itself once the clock has run to the end of a late interval. A tick the render thread never showed (two snapshots taken in one slow frame) gets one straight bridging segment from the last laid end, so a shell's trail has no hole and reaches the shell; `render::particles::tests` pins both. A trail whose entity is destroyed on a tick (a shell hitting) is in that tick's snapshot once more, flagged `ended`, at the entity's final transform (`Presentation::ended_trails`, recorded by effect expiry before compaction), so the last segment reaches the impact point instead of stopping a tick short; `snapshot::ended_trail_tests` pins it. Off screen a trail emits nothing and starts afresh when it returns (moho catches up from the entity's position history). Beams are CPU ribbons: `Thickness` to each side, colour from `StartColor` to `EndColor`, texture scrolled by `UShift` / `VShift`, `Length` along the bone when the sim gives no far end (negative: backwards, exhausts).
- Attached effects sit on the bone the model is drawn with (posed + interpolated: muzzle flashes and trails follow aimed barrels); unattached ones start on that posed bone and stay.
- No sim tick's effects are lost to a slow frame: ticks the render thread skipped are always caught up (`CatchupEmit` only governs ticks missed off screen), a snapshot replaced before it was prepared hands the effects only it had to the particle system, and an effect gone before its blueprint had loaded (its first use in a game) is kept with its last view; all are emitted backdated, where the sim had them, within 24 ticks (`Particles::missed`; `render::particles::tests` pins the catch-up).
- Per-frame CPU: effects are placed in parallel; on frames that bring a new sim tick the per-effect state (a vector aligned with the snapshot's effects, merged by id) is walked in parallel chunks that emit records, merged into batches in effect order; other frames only place the beams. To time it, run `dev/scenes/stress/air.lua` with the camera on the dogfight under Tracy and read the particles step.
- Visual checks: `cargo run -- --script dev/scenes/visual/particle_cases.lua --no-ai --camera 370,545,16` (units firing: Triad, Ravager, Aurora, flak, gunship, artillery, torpedoes; a medium explosion with a light flash; sonance hits, a beam emitter, drag; cameras per row in the script's header), `cargo run -- --script dev/scenes/visual/all_emitters.lua --no-ai --camera 383,560,60` (every emitter blueprint in a grid, the index logged as `EMITTER` lines), `cargo run -- --script dev/scenes/visual/effects.lua --camera 412,598,24`.
- Not yet: moho's position history for catch-up births (they spawn where the emitter is now, which is why the data turns `CatchupEmit` off for contrails), a fidelity setting in the options, `EFFECT_POSITION` and the other `SetEmitterParam` names beyond `LIFETIME` / `REPEATTIME` / `LENGTH` / `THICKNESS` / `SIZE`, `SetBeamFx` on collision beams.

## Models (`render/models.rs`)
- The sim numbers each mesh blueprint the first time an entity uses it (`api::mesh::MeshRegistry`: unit/prop spawn from `Display.MeshBlueprint`, and `SetMesh`, e.g. wrecks) and records the VFS paths: LOD0 `MeshName` (else `<Source stem>_lod0.scm`), `AlbedoName` / `SpecularName` (else the mesh file's `<stem>_albedo.dds` / `<stem>_specteam.dds`, else the mesh blueprint's `<Source stem>…` when the mesh file ships elsewhere — the Megalith's cross-folder mesh; relative names resolve against the blueprint's folder, `..` included; defaulted LOD1+ names keep the file's `_lod<n>` infix in the Source folder). `EntityView` carries the index and `Display.UniformScale`; snapshots send newly named meshes incrementally and the `Arc<RwLock<Vfs>>` once. Render-only: nothing flows back into the sim.
- `.scm` vertices are 68 bytes (pos, normal, tangent, binormal, uv0, uv1, 4 bone indices); `assets::scm::read_mesh` keeps pos/normal/uv0, the first bone index (FA skins rigidly) and the u16 triangle list, plus the bones' rest poses (parent-relative, f32).
- `.dds` (`assets::dds`): DXT1/3/5 upload as BC1/2/3 when the adapter has `TEXTURE_COMPRESSION_BC` (requested when offered), else decode to RGBA8 via `texpresso`; 32-bit uncompressed is swizzled. Mips below one 4x4 block are dropped.
- Loading: on first draw, a 2-thread pool (not the sim's rayon pool) reads and decodes; the render thread uploads at most 4 finished models per frame. Failed loads stay boxes; missing textures fall back to white albedo / no team mask.
- Shader names: LOD `ShaderName`s go through moho's old-name table first (from the exe: `TMeshGlow` -> `NormalMappedGlow`, `TMeshAlpha` -> `NormalMappedAlpha`, `TMeshNoNormals` -> `VertexNormal`, `TMeshNoLighting` -> `Flat`, `Simple` / `Team` -> `Unit`, ...; `api::mesh::technique_name`), and a technique we don't have falls back to `Unit` as moho's does.
- Shading (`api::mesh::Shading`, ports of FAF's `mesh.fx` in gamma space): units use FAF's PBR pixel shaders per faction (`PBR_UEF`, `PBR_Cybran`, `PBR_Aeon`, `PBR_Seraphim`, `PBR_AeonCZARPS`): normal maps (uv1; x from green, y from alpha onto binormal / tangent / normal), the map's environment cube (`<default>` entry, else `defaultenvcube.dds`; mips picked as if faces were 256 texels), the BRDF lookup `anisotropiclookup.dds`, and the map's lighting (sun colour x `LightingMultiplier`, ambience, shadow fill). Props: `VertexNormal`, `NormalMappedAlpha` (trees), `NormalMappedTerrain` (rocks), the `*Clutter` techniques (`ClutterPS`: albedo lit by the normal map, unshadowed, alpha-tested at `0x70`; they still cast shadows here, which FA's don't). Projectiles: `NormalMappedGlow` (`NormalMappedPS`, specular blue glows). Wrecks: `WreckagePS` (the `wreckage_noise.dds` FAF's Lua swaps in, scrolled per wreck; the vertex shader's crumple included); burnt trees: `BlackenedNormalMappedPS`. Aeon build pool: `PBR_AeonBuildPuddlePS`. `Flat` is unlit. `*Alpha` shaders (foliage) discard alpha < 0.5 and draw two-sided; the rest cull back faces (CCW in our right-handed view). Instances are grouped per mesh: one draw per mesh.
- Moving vertices (`api::mesh::VertexMotion`, instance `bones.y`, `shaders/vertex_motion.wesl`): FA's undulating and bloating vertex shaders, by LOD `ShaderName`. `UndulatingNormalMappedAlpha` (the trees' LOD0; their farther LODs are `NormalMappedAlpha` and stand still, as in FA) leans each vertex along `windDirection` (0.707, 0, 0.707) by `0.003 * y` (mesh units, before `UniformScale`) times `sin^2(0.05 time - windDirection . translation)`: a lean one way and back every 63 ticks, neighbouring trees out of step; `UndulatingClutter` (FAF's `Clutter02_prop`; kelp is `UnderwaterClutter` and stands still) the same at `0.1 * y` and 0.0625; `BloatingNormalMappedAlpha` (Red Rocks pods) swells along the direction from the mesh origin by `2 |dir.xz| sin(0.1 time + |translation.xz|)`. `time` is game ticks (the frame's interpolated tick, `Globals::params.z` x 10; moho's `MeshRenderer::ConfigureShader` sets tick + fraction), so they follow game speed and stop when paused. Shadow casters move the same (moho's `UndulatingDepthClip`; the pods use it too, so their shadows sway rather than swell): the cascades' uniform carries the time. Normals aren't bent (nor are FA's). No CPU cost: the motion rides in an unused instance field. Visual check: `cargo run -- --no-ai --script dev/scenes/visual/tree_sway.lua --camera 400,603,12`.
- Glow: each technique's alpha (`emission + glowMinimum` etc.) goes to the glow target for the bloom; opaque meshes write it, alpha-tested / effect / build / placement meshes keep what's behind.
- Shields (`ShieldUEF` / `ShieldCybran` / `ShieldAeon` / `ShieldSeraphim`, FAF's `ShieldPS` and kin with their four scrolling texture coordinate sets): drawn after the water, blended (Seraphim additive, Cybran's second pass pushed out 0.01), back faces culled in place of moho's depth-only `ShieldFill`; instance `team.a` carries the health fraction. The shield's albedo is converted back to gamma (FAF compares raw texture values). Their alpha blends into the glow too.
- Projectiles: the mesh and scale are resolved once per blueprint into the projectile template (spawns are per shot) and set on the entity. Projectiles without a mesh (effect carriers, lasers) aren't drawn until particle effects exist.
- Effect meshes (`/effects/` blueprints, `AlphaFade`: the UEF build cube) draw last as faint unlit two-sided overlays without depth writes (FA's own effect shaders aren't reproduced).
- Levels of detail: each mesh blueprint `LODs` entry beyond the first is registered as its own render asset (`<stem>_lod<n>.scm` or its `MeshName`; its own textures if named or present, else LOD0's; a level whose mesh is missing keeps the previous one), and the LOD0 asset lists `(LODCutoff, asset)` per level. Each frame an entity draws the first level whose cutoff is beyond its depth along the view (moho's LOD plane, `GeomCamera3::LODMetric`: depth times the camera's LOD scale, `render::camera::LodPlane`, the same for decals, splats, particles and resource icons, and in the shaders through `Globals::forward.w`; FAF's Level Of Detail option sets the scale through the console, Low / Medium / High / Extreme 1.2 / 1.0 / 0.9 / 0.7, `render::console::lod`; the distance from the eye, used before, made meshes pop in a circle round the camera) (LOD0 stands in while a level loads; clips bind to each level's skeleton). Beyond the last cutoff no mesh is drawn: units and structures with a strategic icon show it alone, and, as moho, a unit without an icon draws nothing zoomed out (FAF's `blueprints-lod.lua` recalculates the blueprint cutoffs from the unit's size, so icon-less build drones vanish past about 56), props not at all. Render tests that inspect bones keep LOD0; `stock_game_assets_all_load` loads every level.
- Strategic icons: a unit's icon shows from the camera zoom in its blueprint's `Display.IconFadeInZoom` (130 for nearly every unit, 175-2000 for the big ones; 130 when unset), fading in over the next 15% of zoom (the width is inferred: the wiki gives only where it begins), over the mesh while that still draws; past the mesh's last LOD cutoff (215 for most units, so further out) the icon stands alone. FAF's Always Show Strategic Icons (`ui_AlwaysRenderStrategicIcons 0/1`, bare from its key action: a toggle; applied at start from the profile) puts every unit's icon over its mesh at any zoom, fully opaque. FAF's Scale strategic icons (`ui_StrategicIconScale`, a FAF binary patch variable: `hooks/Strategic.hook`) multiplies each icon's half size, taken in whole texels as moho does, for every state and for blips; also applied at start. The patch's `ui_DepositIconScale` and `ui_ProjectileIconScale` aren't handled yet. Radar blips show their generic shape at any zoom. A unit without an icon name past its last cutoff draws nothing zoomed out, as moho does (the Cybran build drones stood in as small army-coloured marker cubes there before). The icon pass (`frame::icon_items`) lays them out at the resting texture's size in logical pixels, tinted with the army colour; the hovered and selected states draw their own `_over` / `_selected` textures at their own size on the same centre (the selected ones are 4 texels bigger each way: a frame around the icon). The icon textures carry grey colour under their fully transparent texels (the frame's footprint); the atlas, sampler and straight-alpha blend leave those pixels untouched (`strategic_icon_transparent_texels_draw_nothing` draws one over a flat ground and checks). The faint, roughly square halo seen around aircraft icons between the icon zoom and the mesh's last LOD cutoff is not the icon: it is the bloom of the mesh's glow underneath (moho's near-flat 7-tap separable blur makes a square-ish footprint about 36 px across, in the mesh's team-tinted colour), gone once the mesh stops drawing.
- Frustum culling: entities whose bounding sphere (model radius x scale, plus 12 units for shadows cast into view) lies outside the camera frustum aren't posed or instanced.
- To time the whole frame with many units, run the `frame_cost_16k` test (the 16-army stress: about 16k units, CPU prepare and draw + GPU, whole map and zoomed on one army).
- Build-in effect: FAF's Lua swaps a unit under construction to its generated `<mesh>_build` blueprint (`StartBeingBuiltEffects` -> `SetMesh`), whose LODs name `UEFBuild` / `CybranBuild` / `AeonBuild` / `AeonBuildNoAnimation` / `SeraphimBuild` and the `<Faction>BuildSpecular.dds` secondary texture. Those assets (`RenderAsset::build`, plus the UEF build cube's `UEFBuildCube`) draw after the opaque models with ports of FA's `mesh.fx` passes (`vs_build` / `fs_build` / `fs_build_overlay`): instance `team.a` carries the fraction complete (0 for projectiles), `params.x` the technique, group 3 the secondary texture. UEF: the grid pattern pulsing toward blue, at least half opaque, plus two scrolling overlay copies fading out over the last 5%; Cybran (`cybran_build`): lit by the faction's own shader at the Fidelity (High `PBR_CybranBuildPS` = `PBR_Cybran` with the team colour ramped in over the last 10%, Medium `CybranBuildPS`'s insect lighting, Low `CybranBuildLoFiPS`), 40% visible until 70% then ramping to opaque, and drawn over black, not over what's behind it: the technique is in moho's depth stage, and the real game shows no ground (or anything else) through the structure's nearest surface (`dev/probes/maps/probe_cybranbuild`; ours used to blend it over the ground and it read as glass). Its red circuit overlay (`CybranBuildOverlayPS`) is gamma-space colour (it went through `frame_rgb`, brightening the thin lines' soft edges into thick blocks) masked by the secondary's raw red; Aeon: rises one world unit from 75% size, with a metallic liquid overlay (simple lighting, FAF's is PBR); Seraphim (`seraphim_build`): grows from 25% size, lit by the faction's own shader at the Fidelity (High `PBR_SeraphimBuildPS` = `PBR_Seraphim`, Medium `SeraphimBuildPS`, Low `CybranBuildLoFiPS`) with its albedo, normal and spec lookups rippled by the secondary texture (`* 0.03`, lerped to none by `(fraction - 0.9) * 10`, so ten times as strong at 0%), blended at `max(fraction, 0.25)` and writing no glow (`Write_RGB`); its team colour shows from the start. (It drew with the generic `build_lit` before, on the raw albedo, whose Seraphim textures are near black: the unit barely showed;.) The dark purple cloud over it is FAF's inner build emitter (`CreateSeraphimBuildThread`). Other factions: team colour fades in over the last 10%. Shader `time` is taken as ticks of the render clock (guess). Visual check: `cargo run -- --script dev/scenes/visual/build_effects.lua --camera 410,606,22`; Seraphim at several fractions: `cargo run -- --no-ai --script dev/scenes/visual/seraphim_build.lua --camera 388,602,46`.
- Not yet: water colour on submerged units (`ApplyWaterColor`), Aeon CZAR's scrolling secondary, the UI-side meshes (`CommandFeedback*`, `RallyPoint`, `FakeRings`: the renderer doesn't draw order feedback yet).

## Frame and bloom (`render/bloom.rs`)
- The scene pass renders into an offscreen frame (`globals::FRAME_FORMAT`, `Rgba8Unorm`) holding gamma-space colour, as D3D9 FA's framebuffer did, so every blend (additive effects above all) matches FA; shaders that light in linear space convert with `frame_rgb`. A second target (`GLOW_FORMAT`, `R8Unorm`) holds each pixel's glow, which FA kept in the frame's alpha: cleared to `glowMinimum` (0.01, what terrain writes), written by opaque meshes, blended by shields, left alone by everything else (`globals::GlowWrite`).
- Bloom (moho's `CRenFrame` + `frame.fx`, constants from the exe): copy into a half-size buffer `colour * saturate((glow - 0.02) * 2.0 + map Bloom)` (`TCopyGlowingStuff`; the map's `Bloom` lighting value is the copy's add), blur 7 taps horizontally then vertically twice (`ren_BloomBlurCount` 2), each pass scaled 1.5 (`ren_BloomBlurKernelScale`), and add onto the frame (`TFrameAdd`), which is then written to the sRGB surface. The UI draws on top.
- Visual checks: `dev/scenes/visual/graphics_showcase.lua`, `dev/scenes/visual/all_units.lua`, `dev/scenes/visual/effect_meshes.lua` (commands in `docs/guides/running.md`).

## Range overlays (`render/ranges.rs`, `shaders/ranges.wesl`)
FAF's military / intel overlay toggles (`SetOverlayFilter(s)`, colours and
line widths from `rangeoverlayparams.lua`) draw as moho's do (`frame.fx`
`RangeMask` / `RangeFill` / `RangeBurn`): each ring a volume over the
ground, overlapping rings one outline that follows the terrain, the line
opaque in the filter's colour with its glow (`AA` of `AARRGGBB`); the
ground inside is left as it is (`frame.fx` has a `RangeFill` technique, but
the game shows no tint). Moho uses stencil cylinders (FAF's binary patch
clamps them to the map's height band, lowest ground or water - 5 to the
highest); selected and hovered units' rings draw once, in their own colour,
as FAF's other patch has it. We don't draw volumes: that shades every pixel
under a ring's box once per ring, far too slow with thousands of units
selected. Instead we copy the ground's
depth after the terrain's normals pass; each group of rings (a filter's
normal, selected or rollover colour) gets a grid over the ground its rings
reach (cells about a quarter of the group's mean radius, at most 2^20 cells
a view), binned each frame by compute passes (`shaders/ranges_bin.wesl`):
a cell lying wholly inside one ring's radius less its line is covered, and
each cell lists the rings that could still draw in it (every ring reaching
it; on a covered cell only those whose minimum range line does), with
conservative margins so no cell misses a ring a point inside it would find.
Then one full-screen pass per group finds the ground under each pixel and
tests its cell's rings for the edge (inside a ring's radius but not inside
any ring's radius less its line, or on a minimum range's line), stopping at
the first ring that decides it. It is the test the volumes made, so the
picture is bit-identical (`selected_range_ring_images`: a blob with
artillery inside, a ring of units whose lines all cross its centre, a
hollow ring; goldens recorded with the volumes). A cell whose list doesn't
fit the list buffer (2^22 entries) tests every ring of its group, slow but
the same picture (the test draws every view that way too).
Units still being built (an upgrade inside its structure, a factory's
unit) draw no rings: their weapons and intel aren't working yet.
Composites are depth-tested at the ground's depth (units on a ring hide
it); over water the rings lie on the surface. Line widths go from the
filter's zoomed-in to zoomed-out value linearly with the camera's distance.
Selected and hovered units show their own rings with no overlay on
(moho's `range_RenderSelected` / `range_RenderHighlighted`, which FAF's
overlay menu sets through `ConExecute`; on by default): every ring they have
(weapons with their minimum range, shields, build range, radar / sonar /
omni, stealth and cloak fields), each in its kind's overlay's selected or
rollover colour, skipping kinds an active overlay already draws for them.
While placing a structure the ghost shows every range it has, whatever is
toggled (FAF's `range_RenderBuild`), each kind in its own overlay's normal
colour and widths (`UserVm::range_styles`; countermeasure and shields as
FAF's Defense overlay).

Which radius: the sim sends each blueprint's ranges once
(`Snapshot::blueprint_ranges`, by `RANGE_KINDS`): weapons by
`RangeCategory` (anti-air, anti-navy, direct / indirect fire,
countermeasure: min and max radius), shields (`OVERLAYDEFENSE`: half
`ShieldSize`), `AI.StagingPlatformScanRadius` (`OVERLAYMISC`, "Build Range":
FAF writes engineers' build range there), radar / sonar / omni radii, and
the largest stealth or cloak field (counter-intel). The interface turns the
filters into which blueprints each covers and which kinds it takes in (a
kind's `OVERLAY<kind>` category wholly inside the filter, so anti-air
doesn't draw a destroyer's guns; `UserVm::take_overlay_filters`).

## Movies in the interface (`render/ui.rs`, `shaders/ui.wesl` `fs_movie`)
The interface pass has a second pipeline for movies (same vertex shader and
vertices, its own bind group layout: screen, sampler, three R8 planes). Each
playing movie control keeps Y / Cb / Cr textures (plane sizes, macroblock
padded; the quad's UVs crop to the picture), uploaded with
`queue.write_texture` only when its frame changes (at most 30 times a
second), and freed when the movie
isn't drawn. The fragment shader converts BT.601 studio-range YCbCr to sRGB
and then linear for the sRGB target, times the control's alpha.

Decoding (`ui::movie`, `engine::assets::mpeg`): SupCom's `.sfd` movies are
plain MPEG-1 streams with no audio. The vendored pl_mpeg (`vendor/pl_mpeg`,
built by `build.rs`) decodes them on a thread per movie, three frames ahead,
streaming from the file through Rust callbacks. The movie's clock is
interface time; if decoding falls behind, the clock waits rather than
skipping frames. A missing or broken movie finishes as soon as it's played,
with a warning, so FAF's flows waiting on it carry on.

## Sky (`render/sky.rs`, `shaders/sky.wesl`)
FA's sky dome (moho's `SkyDome`, `effects/sky.fx`), drawn first in the scene
pass without depth, so it shows where the camera sees past the terrain
(tilted views). The map's v60 sky box (`scmap::SkyBox`) gives the dome, its
colours, planets and cirrus; v56 maps get moho's `SkyDome` constructor
defaults (centred on the map: where moho centres it isn't traced).
- Dome (`CreateDomeVertexBuffer`): a spherical cap, base radius `scale` at
  elevation angle `sub_height` on a sphere of radius `scale / cos(sub_height)`,
  `subdivisions` vertices around and rows up to an apex, centred on
  `position` and raised by `horizon_height`; seen only from inside.
- Atmosphere: horizon colour to zenith colour from `horizon_height` up to
  `horizon_height + zenith_height`, shaped by `horizonLookup.dds` (A8: its
  first row by angle around, its second by elevation).
- Planets: billboards of part of the sky's albedo (alpha blended), then
  their glow texture's alpha x `decal_glow` into the glow.
- Cirrus: four scrolling layers of `cirrus000.dds` (one channel each),
  multiplied, `cirrus_multiplier` x, in `cirrus_color`; time in ticks.
- Not drawn: moho's cumulus clouds (`RenderCumulus`; no map data seen).

## SMAA (`render/smaa.rs`, `shaders/smaa.wesl`)
After the bloom, SMAA 1x (Jimenez et al.; the reference implementation at
its Medium preset: luma threshold 0.1, 8 search steps, no diagonal or corner
detection; MIT, `render/smaa/LICENSE.txt`) smooths the frame's edges in three
passes: luma edges (point-sampled, local contrast adaptation), blending
weights from each edge's length and crossings through the reference's area
and search tables (`render/smaa/area.bin`, `search.bin`, extracted from its
`AreaTex.h` / `SearchTex.h`), then each pixel blended with the neighbour
across its edge. The bloom's add writes the gamma-space frame into SMAA's
input instead of the surface, and the blend writes it onto the sRGB surface
(the same `pow 2.2` the add did), so edges are found on perceptual luma; the
interface draws after it, unsmoothed. FA offered MSAA; the Video options'
Anti-Aliasing keys are FA's sample counts, 0 off and 1 SMAA
(`GetAntiAliasingOptions` returns `{1}`; a profile's 2 / 4 / 8 from FA reads
as SMAA until MSAA is drawn). The Overlays window toggles it; `--no-aa`
(alias `--no-fxaa`) keeps it off.

## Video options (`render/console.rs`)
FAF's Options > Video items reach the renderer as the console commands their
setters send (`ConExecute`, queued by the user layer). Moho runs every
setter at start (`optionslogic.Apply(true)`), but some skip themselves then
(moho reads those from the profile when it makes the device): `apply_profile`
sends those from the profile when an interface (front end or game) starts.

| Option | Command | Here |
|---|---|---|
| Primary Display (engine) | `eng_PrimaryDisplay n\|key` | the monitor the main window goes on: the option's "Display n" (0: the system's main display) or its key; the window moves there, fullscreen again if it is. The profile's applies at start, except in a `--screenshot` run |
| Primary Adapter | `SC_PrimaryAdapter windowed\|w,h,hz` (not at start) | windowed, or fullscreen: always borderless over the window's monitor at the desktop's mode, never exclusive (on Windows winit's exclusive fullscreen stays always-on-top, so Alt-Tab can't leave it). The option's choices are the monitor's modes (filled in when an interface starts). At start the profile's value is applied unless it's FAF's default `1024,768,60` (written into every new profile, so not a choice) or a `--screenshot` run |
| Secondary Adapter | `SC_SecondaryAdapter` (refills its list) | not supported (a second world view on another monitor): its only choice is Disabled |
| Vertical Sync | `SC_VerticalSync 0\|1` (not at start) | the surface's present mode: Fifo on (Wayland: Mailbox where offered, paced to the refresh); off, Immediate where offered, else Mailbox |
| Frametime | `sc_FrameTimeClamp ms` (not at start) | the shortest frame, with vsync off: the render loop keeps to one frame per `ms` (a frame rate cap). With vsync on it is ignored (the display paces frames; an engine choice) |
| Lock Fullscreen Cursor To Window | `SC_ToggleCursorClip 0\|1` | the cursor confined to the window while fullscreen (re-applied on focus); macOS can't confine it, so there it stays free |
| Anti-Aliasing | `SC_AntiAliasingSamples n` (not at start) | 0 off, 1 SMAA (see SMAA above) |
| Level Of Detail | `SC_CameraScaleLOD`, `cam_SetLOD` | the camera's LOD scale |
| Render World Border | `UpdateWorldBorderState` (Lua): `MapBorderClear`, `MapBorderAdd(mesh)` | the skin's border mesh round the map (`ui/world_mesh.rs`): centred on the map, scaled by its width x the blueprint's `UniformScale` (the model spans 512 units a map), drawn with `MapImager` (`MapImagerPS0` / `PS1`); casts no shadow |
| Render Sky | `ren_Skydome 0\|1` | the sky dome (see Sky above) drawn or not |
| Fidelity | `graphics_Fidelity 0-2` | FAF's mesh technique tiers (`effects/mesh.fx` `_HighFidelity` / `_MedFidelity` / `_LowFidelity`): High the PBR set; Medium the pre-PBR unit shaders (`NormalMappedPS` UEF, `NormalMappedInsectPS` Cybran with `insectlookup.dds`, `AeonPS` Aeon and CZAR, `UnitFalloffPS` Seraphim), props as High; Low vertex-normal shaders (`ColorMaskPS_LowFidelity`, `VertexNormalPS_LowFidelity`, `LowFiUnitFalloffPS`, `BlackenedLoFiPS`, `WreckagePS_LowFidelity`, `AeonBuildPuddleLoFiPS`), no glow. Build (except Seraphim's), shield and effect techniques, terrain (FAF's has no tiers) and water draw as High. FAF's default is Medium |
| Shadow Fidelity | `shadow_Fidelity 0-3` | Off: no shadow pass, receivers lit; Low: 2 cascades; Medium, High: 3 (FA's softness is a separate setting, `ren_ShadowBlur`, so it doesn't change here; see verifying-against-the-binary.md) |
| Shadow Resolution | `ren_ShadowSize n` | texels per shadow cascade side (FA had one map of that size; ours is per cascade) |
| Shadow render distance | `ren_ShadowLOD m` | the LOD metric (view depth x the LOD scale) past which nothing is shadowed (the exe: "At what LODMetric do we stop rendering shadows"): the cascades end at `m / scale`, receivers past it are lit. FAF's default 260; the slider's top (440) is no limit, shadows at every zoom |
| Texture Detail | `ren_MipSkipLevels n` (2 low, 1 medium, 0 high; FAF's default medium) | models and the terrain's tiled strata sampled from n mips down (the samplers' `lod_min_clamp`; a change rebinds, nothing reloads). Textures stay loaded whole, so it saves bandwidth, not memory as FA's did |
| Bloom Render | `ren_bloom 0\|1` | off: no glow copy or blur, the frame added with none. FA's and FAF's default is off |
| Bloom Intensity | `ren_BloomBlurKernelScale s` (slider 10-17 / 10) | the blur passes' kernel scale (default 1.5, FAF's 15) |

## Effect techniques (`render/models.rs` `fs_effect`, `api::mesh::EffectPass`)
- Ports of `mesh.fx`'s effect section: `Effect` (`TMeshAdd`), `Explosion`, `Cloud`, `OuterCloud`, `NukeUEF`, `NukeEMP`, `NukeQuantum`, `PhalanxEffect`, `UEFQuantumGate`, `ShieldImpact`, `CybranShieldImpact`, `GlassAlpha`, `AlphaFade`, `TACloak`, plus `Flat` and `Metal` with the opaque models. Phase shields (`PhaseShield`, `AeonPhaseShield`, `CybranPhaseShield`, `SeraphimPersonalShield`) and stunned units (`*Stunned`) are two-pass: their faction's unit shader, then the overlay (`*PhaseShieldPS` pushed 0.02 out; `StunnedUnit`, black at 66%).
- Drawn after the water with the shields, each with its technique's blend and depth state (`EffectPass`: alpha, alpha writing depth, two-sided, additive, premultiplied, overlay, and feedback: the interface's order markers and rally flags, last and without depth, see `docs/architecture/ui.md`); alpha tests from the techniques' `d3d_Greater` references become discards.
- Timing: these shaders run on the mesh's age (`time - material.x`). The renderer records the frame an entity first draws with its current mesh (only entities with these techniques are tracked) and sends the age in ticks in instance `shading.w`. `SetScaleVelocity` now grows / shrinks a projectile's draw scale (the nuke meshes animate this way).
- The mesh bind group carries the LOD's `LookupName` texture (else `SecondaryName`) at binding 4 (moho's `lookupTexture`).

## Animation (`snapshot::PoseOp`, `render_pose`)
- Each snapshot carries pose ops per entity (`EntityView::pose_start/len` into `Snapshot::poses`), read from the sim's manipulators (render-only; the sim never reads them back): animators -> `Clip { clip, time, duration, looping }` (clip paths numbered by the snapshot sink, sent once in `new_clips`), rotators -> `Rotate` about their bone-local axis, weapon aims -> `Rotate` the yaw bone about Y by the weapon's `aim_heading` and the pitch bone about X by `-aim_pitch` (script aims: `SetHeadingPitch`), sliders -> `Translate`, `HideBone` -> `Hide`. Hidden bones are found through `HiddenBoneOwners` (entities that ever hid one), not a scan of every entity.
- `.sca` clips (`assets::sca::read_clip`): per-frame parent-relative bone poses, bound to a mesh's bones by name; loaded on the global pool, as models are.
- Per frame, each posed instance's ops are interpolated with the previous snapshot's (same manipulator, bone and kind; angles take the short way round, looping clips wrap), then `render_pose::evaluate` builds bone matrices: rest pose, clip bones replaced, other ops composed on top, parents first, skin = animated world x inverse rest world. Matrices go in one storage-buffer palette per frame; the instance carries its first index (`NO_BONES` = unskinned, so static instances cost nothing).
- Builder arms (`CreateBuilderArmController`) turn each tick toward their unit's focus (what it builds, repairs, reclaims or captures; its collision centre), within `SetAimingArc` and its slew rates, and back to rest without one (`builder_arms_tick`, after construction); drawn and posed in the sim like weapon aims. Where a builder arm and a weapon aim share bones (the UEF ACU's arm), only one shows: an active builder arm at precedence >= the weapon's, else the weapon (`Manipulators::aim_hidden`).
- Not yet: general manipulator precedence (clips first, then the rest in creation order), `SetBoneEnabled`. Slaves (`CreateSlaver`; FAF slaves a weapon's other rack bones to its pitch bone with `RackSlavedToTurret`) copy the turns the owner's other manipulators give the source bone onto the dest bone, in its own frame (`SetMaxRate` not yet). Thrust controllers (UEF engine pods) pose their bone through `Op::Rotate` from `Thrust::step`.
- Visual check: `cargo run -- --script dev/scenes/visual/animation.lua --camera 410,603,9` (tanks with targets abeam, walkers walking, radars spinning).

## Fog of war

The player sees the game as the **view army**: the interface's focus army
(`UserVm::focus_army`, copied each frame into `DebugRequest::view_army`;
observers have none and see everything; `--overlay fog=<army>` forces a view
for debugging). While the game's `FogOfWar` option isn't `'none'`, each
snapshot is cut down at publish time (`engine/snapshot/fog.rs`, read-only
over the sim's intel grids) to what that army's side knows:

- own and allied entities as they are; props always;
- enemy units seen now (vision, water vision, omni, through cloak rules) as
  they are; radar / sonar contacts it doesn't see as **blips**
  (`Sighting::Blip`): no mesh, never handed to the interface (no
  `GetUnitById`, selection data, rollover, range rings, health bar), drawn
  at any zoom as an unidentified strategic icon, the unit icon's shape
  without tech or role (`icon_land2_antiair` -> `icon_land_generic`) in
  FAF's `UnidentifiedColor` grey (inferred icon); minimap dots grey too;
- enemy structures seen before and out of sight now at their last-seen
  spot as it was when last seen (`Sighting::Remembered`): the mesh keeps
  the last-sighting's rest pose, greyed by the vision pass's 0.67 factor
  (`models.wesl fogged`), with no life bar and no range rings; its
  strategic icon stays as the **remembered blip** — the structure's own
  icon in FAF's `UnidentifiedColor` grey, the normal zoom rules. Kept by
  the sink until the view has eyes on the spot again (moho's
  `UserEntity::OrphanUpdate` 0x1047b500 drops an orphaned entity once its
  position is visible) — then it draws live — or the sim forgets the
  sighting (`FlushIntelInRect`); minimap dots grey too;
- other enemy or army-less entities (projectiles, shield bubbles), effects
  and tread marks only where the side has eyes; jammer fakes
  (`Snapshot::fake_blips`) as grey land blips.

The terrain gets a **fog texture** (group 0 binding 9, R8, one texel per
2x2 vision cell, the 4x4 radar / sonar / omni cells covering four): `vision::fog_level` per cell (unexplored / explored /
radar / visible), built by `IntelGrids::fog_row` and sent as runs of the
rows whose intel changed since the last delivered snapshot (a full grid for
a new view, army, mode or alliance change), written into the texture in
place. The clear ground, though, is drawn as moho draws it: exact circles
round the side's vision sources, not the grid. Each snapshot carries
those sources (`Snapshot::vision`: entity, x, z, vision radius, own and
allied armies); each frame the renderer blends them between the last two
snapshots by id as the units are (`snapshot::blend_vision`), drops those
that can't reach a view, and bins them on the GPU into a grid
(`render/vision.rs`, the range overlays' binning: a cell wholly inside a
circle is clear, an empty one fogged, an edge cell lists its circles).
`fog_factor` (globals.wesl) multiplies the colour of the terrain, decals,
tread marks, water and unit meshes: 1 where the point's xz is inside a
circle (a hard per-pixel edge), else 0.67 (moho's vision pass blends black
at alpha 0.33 over every pixel no `vision.fx` `CastVision` cylinder holds:
`frame.fx` `VisionPS(0.33)`, MohoEngine.dll 0x103e5580; radar coverage
isn't shown differently), and black where never explored (`'unexplored'`
games, the grid sampled bilinearly; inferred from moho's unused gray fog
texture path 0x10472ca0, which gives unexplored cells full intensity). So
ground can show clear before a unit standing on it is seen: the grid's
discs move only once a source has gone a third of its radius (as moho's).
See docs/research/verifying-against-the-binary.md for the pass. Not yet: the minimap image
isn't fogged (in `'unexplored'` games the minimap still shows the whole
map); scorch-mark decals (`CreateDecal`) aren't culled; moho's cylinders are
45-gons spanning y -256..256 (ours round, any height).
