//! Sim -> render snapshots.
//!
//! At the end of each tick, if a renderer is attached (windowed mode only:
//! headless runs never install a sink and pay nothing), the sim packs what
//! the renderer draws into a [`Snapshot`] and offers it on a bounded channel.
//! The sim never blocks: if the renderer is behind, the snapshot is dropped.
//! Snapshots hold render-side floats; nothing in them flows back into the
//! sim, so float conversion here can't affect determinism.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};

use crate::engine::sim::entity::{CollisionShape, EntityKind, EntityWorld};
use crate::engine::sim::terrain::Terrain;

mod fog;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Unit,
    Structure,
    Prop,
    Projectile,
    /// A killed unit still in the world (a plane falling, a wreck sinking,
    /// a death animation): drawn, but no longer a unit to the interface: no
    /// strategic icon, bars or order lines, not selectable, and gone from
    /// the selection and the user layer (`UserUnit:IsDead()`).
    Dying,
    Other,
}

impl ViewKind {
    /// A live unit (a structure too): not a `Dying` one.
    pub fn is_unit(self) -> bool {
        matches!(self, ViewKind::Unit | ViewKind::Structure)
    }
}

/// One drawable entity.
#[derive(Clone, Copy, Debug)]
pub struct EntityView {
    pub id: u32,
    pub kind: ViewKind,
    /// Army index (1-based), 0 = none.
    pub army: u32,
    pub position: [f32; 3],
    /// Quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Collision volume: centre offset (entity frame) and half extents.
    /// All zero: not clickable (an upgrade's unit, built inside the
    /// structure with no collision: clicks and its icon go to the builder).
    pub center: [f32; 3],
    pub half: [f32; 3],
    /// 0..1.
    pub health: f32,
    /// Its maximum health now (`GetMaxHealth`: a buff or script may have
    /// changed it from the blueprint's).
    pub max_health: f32,
    /// Its build rate now (`GetBuildRate`: an enhancement, buff or
    /// veterancy may have changed it from the blueprint's); 0 if not a unit.
    pub build_rate: f32,
    /// 0..1 (1 = finished).
    pub build: f32,
    /// 0..1 of its current job (`GetWorkProgress`: building, an upgrade, an
    /// enhancement, a silo missile, reclaim, capture); 0 = none.
    pub work: f32,
    /// Its shield's strength, 0..1 (`SetShieldRatio`); -1 = no shield.
    pub shield: f32,
    /// Index into [`Snapshot::blueprints`] (`u32::MAX` = unknown); for a
    /// projectile, its strategic icon (`Snapshot::new_projectile_icons`).
    pub blueprint: u32,
    /// Index of its mesh in the snapshots' `new_meshes` (`u32::MAX` = none).
    pub mesh: u32,
    /// Mesh scale per axis (blueprint `Display.UniformScale`, or `SetScale`).
    pub scale: [f32; 3],
    /// Its bone poses: `Snapshot::poses[pose_start..pose_start + pose_len]`.
    pub pose_start: u32,
    pub pose_len: u32,
    /// What the viewing army knows of it (fog of war).
    pub sighting: Sighting,
    /// Not selectable: a unit still being built attached to its builder (a
    /// factory's, a carrier's), as moho's interface won't pick or select it
    /// until it's finished.
    pub in_builder: bool,
    /// A finished unit attached to another (carried by a transport, on a
    /// carrier): moho's lifebars skip it unless the profile's
    /// `show_attached_unit_lifebars` is on.
    pub attached: bool,
    /// Texture scroll (treads): last tick's pair, then this tick's; the
    /// renderer blends them. All zero: none.
    pub scroll: [f32; 4],
    /// What the right-click order reads of a unit beyond the above
    /// ([`order_flags`] bits).
    pub order_flags: u8,
}

/// [`EntityView::order_flags`] bits.
pub mod order_flags {
    /// Can be captured (`SetCapturable`; moho's unit attributes +0x69).
    pub const CAPTURABLE: u8 = 1;
    /// In the `Upgrading` state.
    pub const UPGRADING: u8 = 2;
    /// On fire state 2 (ground fire): the attack mode's ground click gives
    /// it an attack on the ground, not an attack-move (MohoEngine.dll
    /// 0x103e7960 reads UserUnit +0x1b8).
    pub const GROUND_FIRE: u8 = 4;
}

/// [`Snapshot::blueprint_order_categories`] bits: the categories moho's
/// right-click order asks of the unit under the cursor (MohoEngine.dll
/// 0x103e7a40).
pub mod order_categories {
    pub const RECLAIMABLE: u16 = 1;
    pub const RECLAIMFRIENDLY: u16 = 2;
    pub const FACTORY: u16 = 4;
    pub const CAMPAIGNGATE: u16 = 8;
    pub const COMMAND: u16 = 16;
    pub const CANTRANSPORTCOMMANDER: u16 = 32;
    pub const AIRSTAGINGPLATFORM: u16 = 64;
    pub const TRANSPORTATION: u16 = 128;
    pub const TELEPORTATION: u16 = 256;
    pub const FERRYBEACON: u16 = 512;
    pub const TRANSPORTFOCUS: u16 = 1024;
    pub const EXPERIMENTAL: u16 = 2048;
    /// Not a category: `Physics.MotionType` is `RULEUMT_Air` (an air unit).
    pub const AIR: u16 = 4096;
}

impl EntityView {
    /// A unit the interface knows (a `UserUnit`): live and identified. A
    /// killed unit still falling or sinking isn't (`UserUnit:IsDead()`),
    /// nor is a radar blip.
    pub fn is_interface_unit(&self) -> bool {
        self.kind.is_unit() && self.sighting != Sighting::Blip
    }
}

/// How the viewing army knows an entity it is shown (fog of war; see
/// [`DebugRequest::view_army`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sighting {
    /// Seen now (or its own side's, or no fog): drawn as it is.
    #[default]
    Seen,
    /// A radar / sonar contact it doesn't see: only an unidentified
    /// strategic icon at its position (no mesh, no blueprint for the
    /// interface).
    Blip,
    /// A structure seen before and out of sight now: its last-seen state
    /// (it may be gone).
    Remembered,
}

/// The fog of war texture's changes (see [`Snapshot::fog`]).
#[derive(Clone, Debug, PartialEq)]
pub enum FogUpdate {
    /// No fog: the whole map shows clear (fog off, an observer, `'none'`).
    Off,
    /// The intel grid's fog levels (`vision::fog_level`, one byte a cell,
    /// row-major): `cols` x `rows` cells of `cell` world units; `bands` are
    /// runs of changed rows `(first row, rows' bytes)`. `full`: the bands
    /// cover every row (a new view: the renderer reallocates).
    Grid { cols: u32, rows: u32, cell: f32, full: bool, bands: Vec<(u32, Vec<u8>)> },
}

/// A vision source of the view army's side (see [`Snapshot::vision`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisionSource {
    pub id: u32,
    /// World x, z.
    pub position: [f32; 2],
    /// Vision radius, whole world units.
    pub radius: f32,
}

/// The vision circles a frame draws (presentation only): each of `latest`'s
/// sources as `[x, z, radius]`, blended from its place and radius in
/// `previous` (the same id) by `alpha`, as the units are, so the circles
/// move with them (moho lerps its circles' previous and current position
/// and radius by the frame's interpolant, MohoEngine.dll 0x103e4350). Those
/// `keep` passes go into `out` (cleared first). Both lists are in id order:
/// one walk.
pub fn blend_vision(latest: &[VisionSource], previous: &[VisionSource], alpha: f32, mut keep: impl FnMut([f32; 3]) -> bool, out: &mut Vec<[f32; 3]>) {
    out.clear();
    out.reserve(latest.len());
    let mut before = previous.iter().peekable();
    for s in latest {
        while before.next_if(|p| p.id < s.id).is_some() {}
        let circle = match before.peek() {
            Some(p) if p.id == s.id => {
                let lerp = |a: f32, b: f32| a + (b - a) * alpha;
                [lerp(p.position[0], s.position[0]), lerp(p.position[1], s.position[1]), lerp(p.radius, s.radius)]
            }
            _ => [s.position[0], s.position[1], s.radius],
        };
        if keep(circle) {
            out.push(circle);
        }
    }
}

/// A timed splat for the renderer (see [`Snapshot::splats`]).
#[derive(Clone, Debug, PartialEq)]
pub struct GroundSplat {
    /// Full texture path.
    pub texture: String,
    pub centre: [f32; 3],
    pub heading: f32,
    pub size: [f32; 2],
    /// Sim tick it was made on.
    pub created: u64,
    /// Seconds.
    pub duration: f32,
    /// Camera depth it's drawn to (`lodParam`).
    pub lod: f32,
    /// Seen by this army's allies only (`None`: everyone).
    pub army: Option<u32>,
}

/// One change to an entity's rest pose, from a manipulator (or a hidden
/// bone). Applied in order: clips set bone poses, the rest compose on top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PoseOp {
    /// Plays `clip` (index into the snapshots' `new_clips`) at `time`
    /// seconds of `duration`.
    Clip { clip: u32, time: f32, duration: f32, looping: bool },
    /// Turns `bone` about its local `axis`.
    Rotate { bone: u32, axis: [f32; 3], radians: f32 },
    /// Moves `bone` by `offset` (mesh units, parent frame).
    Translate { bone: u32, offset: [f32; 3] },
    Hide { bone: u32 },
    /// Puts `bone` at `position` / `rotation` (`[x, y, z, w]`) in the
    /// entity's mesh space, whatever its parents do (a bone following
    /// another entity: `AttachBoneToEntityBone`); its children follow it.
    Place { bone: u32, position: [f32; 3], rotation: [f32; 4] },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectViewKind {
    Emitter,
    Light,
    Trail,
    Beam,
}

/// A live particle effect (`moho.IEffect`), render-only.
#[derive(Clone, Copy, Debug)]
pub struct EffectView {
    pub id: u32,
    pub kind: EffectViewKind,
    /// Index into the snapshots' `new_effect_assets`: the blueprint path
    /// (lights and textured beams: the texture).
    pub asset: u32,
    /// Lights: the ramp texture (`u32::MAX` = none).
    pub ramp: u32,
    /// Army (1-based), 0 = none.
    pub army: u32,
    /// The entity and bone it spawned at (`u32::MAX` = none / origin), so
    /// the renderer can place it on the posed, interpolated bone; followed
    /// every frame if `attached`, else only where it started.
    pub entity: u32,
    pub bone: u32,
    pub attached: bool,
    /// Offset in the bone's frame (`OffsetEmitter`).
    pub offset: [f32; 3],
    /// Where it emits from now (attached: its entity's rest-pose bone, plus
    /// offset; the renderer's fallback).
    pub position: [f32; 3],
    /// That bone's world orientation `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Beams: the far end.
    pub end: [f32; 3],
    pub scale: f32,
    /// Sim tick it was created on.
    pub created: u64,
    /// Overrides from Lua (NaN = blueprint value): lifetime, repeat time,
    /// beam length, beam thickness, light size.
    pub params: [f32; 5],
    /// Its curve overrides: `Snapshot::effect_curves[curves_start..+curves_len]`.
    pub curves_start: u32,
    pub curves_len: u32,
    /// Its entity was destroyed this tick: this is its last view, at the
    /// entity's final transform (a trail lays its last segment there).
    pub ended: bool,
}

/// A `SetEmitterCurveParam` override: curve (`effect_bp::CurveId` index),
/// constant value and random range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectCurve {
    pub curve: u8,
    pub value: f32,
    pub range: f32,
}

/// A pose op and the manipulator it came from (`u32::MAX`: hidden bone),
/// to pair it with the previous snapshot's for interpolation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub source: u32,
    pub op: PoseOp,
}

/// Heightfield for the terrain mesh.
#[derive(Debug)]
pub struct TerrainView {
    pub width: u32,
    pub height: u32,
    /// `(width + 1) x (height + 1)` samples, row-major.
    pub heights: Vec<f32>,
    pub water: Option<f32>,
    /// VFS path of the `.scmap`, for the renderer to read its textures,
    /// lighting and decals.
    pub map: Option<String>,
}

/// The loaded map's `.scmap` path (render plumbing; the sim never reads it).
pub struct MapPath(pub String);

/// Game speed, set by the renderer and read by the real-time sim loop. It
/// only paces wall-clock time between ticks; the ticks themselves are
/// identical at any speed.
#[derive(Debug)]
pub struct SimControl {
    /// Percent of normal speed (100 = 10 ticks/s); `MAX` = no waiting.
    speed: std::sync::atomic::AtomicU32,
    paused: std::sync::atomic::AtomicBool,
    /// The sim stops before stepping past this tick (`--screenshot-tick`),
    /// so the frame is captured at exactly that tick whatever the speed or
    /// window start-up time. `u64::MAX` = never.
    hold_at: std::sync::atomic::AtomicU64,
    /// A networked game's speed, as everyone applied it (percent); 0 when
    /// not networked. `speed` is then only this player's request: changes
    /// travel as commands and apply on every client at the same tick
    /// (`net::lockstep::apply_game_speed`).
    agreed: std::sync::atomic::AtomicU32,
    /// Who changed the speed last: their `GetSessionClients()` index
    /// (1-based), for FAF's "adjusting game speed" message.
    changed_by: std::sync::atomic::AtomicU32,
    /// The speed the game actually runs at (percent; 0 until known): the
    /// agreed or set one, capped by the slowest machine in a networked
    /// game, and as measured when running flat out ("max").
    effective: std::sync::atomic::AtomicU32,
    /// The player left the session (the interface's `SessionEndGame`: the
    /// score screen, quitting): this client's sim runs no more ticks.
    left: std::sync::atomic::AtomicBool,
    /// A networked game whose lobby fixed the game speed (`normal`, `fast`):
    /// the speed selector is off.
    speed_fixed: std::sync::atomic::AtomicBool,
    /// The renderer is done with the session (back to the main menu, or
    /// launching another): the real-time loop returns and the engine
    /// thread starts the next session.
    closed: std::sync::atomic::AtomicBool,
}

impl SimControl {
    pub const MAX: u32 = u32::MAX;
    /// Speed steps for `+` / `-`, percent.
    pub const STEPS: [u32; 8] = [25, 50, 100, 200, 400, 800, 1600, Self::MAX];

    pub fn new(speed: u32) -> Arc<Self> {
        Arc::new(Self { speed: speed.into(), paused: false.into(), hold_at: u64::MAX.into(), agreed: 0.into(), changed_by: 1.into(), effective: 0.into(), left: false.into(), speed_fixed: false.into(), closed: false.into() })
    }

    pub fn speed(&self) -> u32 {
        self.speed.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_speed(&self, percent: u32) {
        self.speed.store(percent.max(1), std::sync::atomic::Ordering::Relaxed);
    }

    /// A networked game's applied speed (percent), `None` when not networked.
    pub fn agreed(&self) -> Option<u32> {
        Some(self.agreed.load(std::sync::atomic::Ordering::Relaxed)).filter(|&p| p != 0)
    }

    /// Everyone applied speed `percent`, asked for by session client `by`
    /// (1-based): the request becomes it too.
    pub fn set_agreed(&self, percent: u32, by: u32) {
        self.changed_by.store(by, std::sync::atomic::Ordering::Relaxed);
        self.agreed.store(percent.max(1), std::sync::atomic::Ordering::Relaxed);
        self.set_speed(percent);
    }

    /// The speed the game runs at, as players see it: the agreed one in a
    /// networked game, else the set one.
    pub fn shown(&self) -> u32 {
        self.agreed().unwrap_or_else(|| self.speed())
    }

    /// The speed the game actually runs at (percent), as the real-time
    /// loop last paced or measured it; else the shown one.
    pub fn effective(&self) -> u32 {
        Some(self.effective.load(std::sync::atomic::Ordering::Relaxed)).filter(|&p| p != 0).unwrap_or_else(|| self.shown())
    }

    pub fn set_effective(&self, percent: u32) {
        self.effective.store(percent.max(1), std::sync::atomic::Ordering::Relaxed);
    }

    /// The session client (1-based) who changed the speed last.
    pub fn changed_by(&self) -> u32 {
        self.changed_by.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_paused(&self, on: bool) {
        self.paused.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// The lobby fixed the game speed (a networked game).
    pub fn speed_fixed(&self) -> bool {
        self.speed_fixed.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_speed_fixed(&self, on: bool) {
        self.speed_fixed.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// The player left the session (`SessionEndGame`).
    pub fn left(&self) -> bool {
        self.left.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn leave(&self) {
        self.left.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// The renderer is done with the session ([`Self::close`]).
    pub fn closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Ends the session: its sim runs no more ticks and the engine thread
    /// moves on to the next launch.
    pub fn close(&self) {
        self.leave();
        self.closed.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// A new session starts on this control: the last one's state (left,
    /// closed, paused, a networked game's agreed speed) goes; the speed
    /// asked for and a hold stay.
    pub fn begin_session(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.left.store(false, Relaxed);
        self.closed.store(false, Relaxed);
        self.paused.store(false, Relaxed);
        self.agreed.store(0, Relaxed);
        self.changed_by.store(1, Relaxed);
        self.effective.store(0, Relaxed);
    }

    pub fn hold_at(&self) -> u64 {
        self.hold_at.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_hold_at(&self, tick: u64) {
        self.hold_at.store(tick, std::sync::atomic::Ordering::Relaxed);
    }

    /// One moho step faster (`up`) or slower: rate x10^(1/10), as
    /// `WLD_IncreaseSimRate` / `WLD_DecreaseSimRate`.
    pub fn step_moho(&self, up: bool) {
        let step = Self::step_of(self.speed());
        self.set_speed(Self::percent_of(if up { step + 1 } else { step - 1 }));
    }

    /// The step past which the speed is "max" (as fast as the machines
    /// go): 10^(77/10) times normal no longer fits a percent.
    pub const MAX_STEP: i32 = 77;

    /// Moho's game speed step (0 normal) nearest `percent`: the rate is
    /// 10^(step/10) (`RATE_GameSpeedToRate`, MohoEngine.dll 0x1014a3d0), so
    /// +10 is 10x, +20 100x. Moho stops at -10..=10; we don't: "max" is
    /// [`Self::MAX_STEP`], and 1% (-20) the slowest.
    pub fn step_of(percent: u32) -> i32 {
        if percent == Self::MAX {
            return Self::MAX_STEP;
        }
        (10.0 * (percent.max(1) as f64 / 100.0).log10()).round() as i32
    }

    /// The speed in percent for moho step `step`: at least 1%, and "max"
    /// from [`Self::MAX_STEP`] up.
    pub fn percent_of(step: i32) -> u32 {
        if step >= Self::MAX_STEP {
            return Self::MAX;
        }
        (100.0 * 10f64.powf(step as f64 / 10.0)).round().max(1.0) as u32
    }

    pub fn label(percent: u32) -> String {
        if percent == Self::MAX { "max".into() } else { format!("{}x", percent as f32 / 100.0) }
    }
}

/// What the renderer wants beyond the basics (written by the renderer,
/// read at publish time; it only shapes the snapshot, never the sim).
#[derive(Clone, Debug, Default)]
pub struct DebugRequest {
    /// The entity shown in the inspector.
    pub selected: Option<u32>,
    /// The player's selection (sorted): their queued orders come back in
    /// [`DebugData::orders`].
    pub selection: Vec<u32>,
    /// The unit whose build queue the construction panel shows, when it
    /// isn't in [`Self::selection`] (a mobile factory's external factory:
    /// FAF shows the selected Fatboy's `GetCreator()` queue): its details
    /// come back in [`DebugData::units`] too.
    pub queue_factory: Option<u32>,
    /// Shift is held: the command graph shows every unit's orders, not just
    /// the selection's (moho draws the whole army's queues while Shift is
    /// down). [`DebugData::orders`] then also carries every unit of
    /// [`Self::view_army`] with orders (every army's for observers).
    pub all_orders: bool,
    pub paths: bool,
    pub weapon_ranges: bool,
    /// Nav grid layer to show: 0 land, 1 hover, 2 naval, 3 amphibious.
    pub nav_layer: Option<u8>,
    /// Show the threat map as seen by this army (Overall threat of enemies).
    pub threat_army: Option<u32>,
    /// The army the player sees and hears as: the UI's focus army (`None`
    /// for observers, who see everything). While fog of war is on (the
    /// game's `FogOfWar` option isn't `'none'`) snapshots carry only what
    /// its side knows (see [`Sighting`], [`Snapshot::fog`]) and world
    /// sounds it can't see are dropped, as moho's `FilterSound`.
    pub view_army: Option<u32>,
    /// Debug: fog of war as this army sees it, whatever the game's option
    /// and the view army: its vision / radar coverage as the grid overlay,
    /// the snapshot filtered for it, its fake jammer blips ringed.
    pub fog_army: Option<u32>,
    /// Build placement under the cursor (one ghost, or a drag build's line /
    /// a template's structures): may blueprint `.0` be built at `.1`?
    /// Answered in [`Snapshot::placement`], in order (a read-only query: the
    /// sim's `CanBuildStructureAt` rules). Empty: no query.
    pub placement: Vec<(String, [f32; 3])>,
}

pub type SharedDebugRequest = Arc<std::sync::Mutex<DebugRequest>>;

/// Details of the selected entity.
#[derive(Clone, Debug, Default)]
pub struct Selected {
    pub id: u32,
    pub blueprint: String,
    pub army: u32,
    pub health: (f32, f32),
    pub build: f32,
    pub layer: String,
    pub orders: Vec<String>,
    pub weapons: Vec<String>,
    pub focus: Option<u32>,
    pub path: Vec<[f32; 3]>,
    pub speed: f32,
}

/// A grid of values over the map (nav passability 0/1, threat).
#[derive(Clone, Debug, Default)]
pub struct GridOverlay {
    pub cols: u32,
    pub rows: u32,
    /// World units per cell.
    pub cell: f32,
    pub values: Vec<f32>,
    /// Changes when the content changes (renderer re-uploads).
    pub version: u64,
}

/// Bits of a blueprint's `General.CommandCaps` the order UI needs.
pub mod caps {
    pub const MOVE: u16 = 1;
    pub const ATTACK: u16 = 2;
    pub const GUARD: u16 = 4;
    pub const PATROL: u16 = 8;
    pub const RECLAIM: u16 = 16;
    pub const REPAIR: u16 = 32;
    pub const CAPTURE: u16 = 64;
    pub const STOP: u16 = 128;
    /// Can carry others (right-clicking a unit with it selected flies out
    /// to pick it up: `TransportReverseLoadUnits`).
    pub const TRANSPORT: u16 = 256;
    /// Can board a transport (right-clicking one walks it aboard:
    /// `TransportLoadUnits`).
    pub const CALL_TRANSPORT: u16 = 512;
    /// Can dock at an air staging platform (`IssueDockCommand`).
    pub const DOCK: u16 = 1024;
    /// `RULEUCC_*` name of each bit, in bit order.
    pub const NAMES: [&str; 11] = ["Move", "Attack", "Guard", "Patrol", "Reclaim", "Repair", "Capture", "Stop", "Transport", "CallTransport", "Dock"];
}

/// A selected unit's state for the interface (FAF's unit panels).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitDetail {
    pub id: u32,
    /// Its queue, front first: (`CommandKind` number, blueprint, count, where).
    pub commands: Vec<(u8, Option<String>, u32, [f32; 3])>,
    /// A factory's command queue (its rally orders), front first:
    /// (`CommandKind` number, where).
    pub factory_commands: Vec<(u8, [f32; 3])>,
    pub focus: Option<u32>,
    /// Per second: mass consumed, energy consumed, mass produced, energy
    /// produced, mass requested, energy requested.
    pub econ: [f32; 6],
    pub fuel: f32,
    pub fire_state: i32,
    pub script_bits: u16,
    pub paused: bool,
    /// Overcharge auto-fire paused (`SetOverchargePaused`).
    pub overcharge_paused: bool,
    /// Silo missiles (`UserUnit:GetMissileInfo`): nuke building (the
    /// missile in progress as a fraction, FA-Binary-Patches#14), nuke max
    /// storage, nukes stored, then the same for tactical missiles.
    pub missiles: [f32; 6],
    /// The unit that made it (a pod's carrier, an external factory's
    /// unit): `UserUnit:GetCreator`.
    pub creator: Option<u32>,
    /// `SetAutoMode` / `SetAutoSurfaceMode`.
    pub auto_mode: bool,
    pub auto_surface: bool,
    /// Under water in the Sub layer (`GetIsSubmerged`).
    pub submerged: bool,
    /// Units carried or docked on it, id order (`GetAttachedUnitsList`).
    pub cargo: Vec<u32>,
    /// Units guarding or repairing it now, id order (`GetAssistingUnitsList`).
    pub assisters: Vec<u32>,
    /// Stunned now (`UserUnit:IsStunned`).
    pub stunned: bool,
    /// The unit its current Guard order guards (`GetGuardedEntity`).
    pub guarding: Option<u32>,
    /// Blueprints its `BuildableCategory` allows but build restrictions
    /// forbid now (sorted ids).
    pub restricted: Vec<String>,
    /// Unit statistics (`SetStat` / `UpdateStat`: veterancy level and
    /// experience, mass reclaimed, ...), name order, for the interface's
    /// `UserUnit:GetStat`.
    pub stats: Vec<(String, f32)>,
}

/// One queued order of a selected unit, where it leads.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderMarker {
    /// `CommandKind` number.
    pub kind: u8,
    /// The command: units sharing an order share its waypoint.
    pub command: u32,
    /// Where that unit is headed: the target, plus its formation slot.
    pub at: [f32; 3],
    /// The order's waypoint (the target itself; the command graph's node).
    pub node: [f32; 3],
    /// A mobile build's site: the structure's build skirt in world XZ
    /// (x0, z0, x1, z1).
    pub site: Option<[f32; 4]>,
    /// A mobile build's structure blueprint (lowercase).
    pub blueprint: Option<String>,
    /// Seconds until the unit gets here (and, for a build, finishes it).
    pub eta: f32,
    /// The unit the order is on (attack, guard, repair...); `None` on the
    /// ground.
    pub target: Option<u32>,
    /// Its waypoint may be dragged (`SetCommandTarget`): not an order on a
    /// prop, nor a mobile build whose structure is started.
    pub movable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DebugData {
    pub selected: Option<Selected>,
    /// Queued orders of the selected units (`DebugRequest::selection`).
    pub orders: Vec<(u32, Vec<OrderMarker>)>,
    /// The selected units' state for the interface.
    pub units: Vec<UnitDetail>,
    /// (unit, remaining waypoints from its position).
    pub paths: Vec<(u32, Vec<[f32; 3]>)>,
    /// (unit, max range) per weapon.
    pub ranges: Vec<(u32, f32)>,
    pub grid: Option<Arc<GridOverlay>>,
    /// The scripts' debug canvas this beat (`DrawLine` / `DrawCircle`).
    pub draw: crate::engine::api::debug_draw::DebugDraw,
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub tick: u64,
    /// The sim's own tick (`GetGameTick`), the clock effects are timed in.
    pub sim_tick: u64,
    /// Wall time the sim spent on this tick, ms (display only: measured
    /// outside sim state, never fed back into it).
    pub tick_ms: f32,
    pub entities: Vec<EntityView>,
    /// Present when the terrain changed since the last snapshot (and in the
    /// first one).
    pub terrain: Option<Arc<TerrainView>>,
    /// Blueprint ids by index; sent with the first snapshot.
    pub blueprints: Option<Arc<Vec<String>>>,
    /// Resource deposits (type 1 mass / 2 hydrocarbon, x, z, size); with
    /// the first snapshot, for the interface's `GetDepositsAroundPoint`.
    pub deposits: Option<Arc<Vec<(u8, f64, f64, f64)>>>,
    /// Each blueprint's [`caps`] bits (same index), with `blueprints`.
    pub blueprint_caps: Option<Arc<Vec<u16>>>,
    /// Each blueprint's [`order_categories`] bits (same index).
    pub blueprint_order_categories: Option<Arc<Vec<u16>>>,
    /// Each blueprint's cargo-slot classes (bit `class - 1`), cargo class
    /// and `ClassGenericUpTo`, same index: the cursor's load orders where
    /// boarding can work.
    pub blueprint_transport: Option<Arc<Vec<(u8, u8, u8)>>>,
    /// Each blueprint's drag-selection priority (`General.SelectionPriority`,
    /// 1 = highest), bit 7 set for `LOWSELECTPRIO` units (priority 6 while
    /// being built: FAF's exe patch), same index.
    pub blueprint_selection_priority: Option<Arc<Vec<u8>>>,
    /// Each blueprint's `StrategicIconName` (empty: none), same index.
    pub blueprint_icons: Option<Arc<Vec<String>>>,
    /// Each blueprint's `Display.IconFadeInZoom`: the camera zoom from
    /// which its strategic icon shows (over the mesh, while that still
    /// draws), same index.
    pub blueprint_icon_zoom: Option<Arc<Vec<f32>>>,
    /// Each blueprint's strategic icon draw layer ([`icon_layer`]), same index.
    pub blueprint_icon_layer: Option<Arc<Vec<u8>>>,
    /// Timed splats made since the last delivered snapshot (FAF's tread
    /// marks and footprints): the renderer lays and fades them.
    pub splats: Vec<GroundSplat>,
    /// Each structure blueprint's mesh render index and scale (same index;
    /// `u32::MAX` for none and for other blueprints): the build placement
    /// ghost.
    pub blueprint_meshes: Option<Arc<Vec<(u32, f32)>>>,
    /// Each blueprint's selection bracket: `SelectionSizeX`, `SelectionSizeZ`,
    /// `SelectionThickness`, `SelectionCenterOffsetX/Y/Z` (same index; zeros
    /// without them).
    pub blueprint_selection: Option<Arc<Vec<[f32; 6]>>>,
    /// Each blueprint's pick and drag-selection box fields:
    /// `SelectionMeshUseTopAmount` (0), `SelectionYOffset` (0.5),
    /// `SelectionMeshScaleX/Y/Z` (1) (moho's defaults, REntityBlueprint ctor
    /// 0x10100840).
    pub blueprint_pick: Option<Arc<Vec<[f32; 5]>>>,
    /// Each blueprint's lifebar: `LifeBarSize` (width), `LifeBarHeight`,
    /// `LifeBarOffset` (towards the bottom of the screen), in world units
    /// (same index; zeros without them, width -1 for `LifeBarRender =
    /// false`).
    pub blueprint_lifebars: Option<Arc<Vec<[f32; 3]>>>,
    /// Each blueprint's build skirt about its position (`x0, z0, x1, z1`,
    /// world XZ offsets; same index; none for mobile units): placing a
    /// structure outlines the skirts of those around it.
    pub blueprint_skirts: Option<Arc<Vec<Option<[f32; 4]>>>>,
    /// Each blueprint's ranges for the strategic range overlays: (kind, an
    /// index into [`RANGE_KINDS`], min radius, max radius).
    pub blueprint_ranges: Option<Arc<Vec<Vec<(u8, f32, f32)>>>>,
    /// Each blueprint's assist range for its placement ghost:
    /// `AI.GuardScanRadius` when `AI.ShowAssistRangeOnSelect`, else 0.
    pub blueprint_assist_range: Option<Arc<Vec<f32>>>,
    /// The answers to [`DebugRequest::placement`], in its order: each query,
    /// the skirt there (world XZ `x0, z0, x1, z1`, if the blueprint has one)
    /// and whether the structure may be built there.
    pub placement: Vec<(String, [f32; 3], Option<[f32; 4]>, bool)>,
    /// The sim's ground decals and splats (tarmacs, deposit markers,
    /// scorch marks) as map decals, when they changed: corner-placed, full
    /// texture paths. Decals in id order, then splats in creation order.
    pub decals: Option<Arc<Vec<crate::engine::assets::scmap::Decal>>>,
    /// Every live prop's view (id order), in the first snapshot: ~85% of a
    /// map's entities are static props, so `entities` leaves them out
    /// (except props with pose ops or hidden bones) and the renderer keeps
    /// its own copy, merged back in by id.
    pub props: Option<Arc<Vec<EntityView>>>,
    /// Props changed since the last snapshot (id order): the new view, or
    /// `None` for one gone. Applied to the renderer's copy.
    pub prop_changes: Vec<(u32, Option<EntityView>)>,
    /// Counts this snapshot as queued until the renderer takes it (drop it
    /// on receipt): the sim skips building snapshots the channel can't take.
    pub ticket: Option<Ticket>,
    /// Bone poses of the entities, in entity order.
    pub poses: Vec<Pose>,
    /// Units with orders, ascending (the rest are idle: the interface's
    /// `+idle` selections).
    pub busy: Vec<u32>,
    /// Upgrades completed since the last snapshot (old structure, new one):
    /// the interface moves its selection and control groups to the new one.
    pub upgrades: Vec<(u32, u32)>,
    /// Launches a camera following their launcher follows, since the last
    /// delivered snapshot (`Display.CameraFollowsProjectile`).
    pub camera_follows: Vec<crate::engine::sim::presentation::CameraFollow>,
    /// Projectiles launched since the last delivered snapshot and where
    /// from (id, position), in id order: the renderer lerps a projectile
    /// with no last-snapshot position from its launch point.
    pub launches: Vec<(u32, [f32; 3])>,
    /// Live particle effects, in id order.
    pub effects: Vec<EffectView>,
    pub effect_curves: Vec<EffectCurve>,
    /// Effect blueprint / texture paths first used since the last delivered
    /// snapshot, by index.
    pub new_effect_assets: Vec<(u32, String)>,
    /// Animation clips (VFS paths) first played since the last delivered
    /// snapshot, by index.
    pub new_clips: Vec<(u32, String)>,
    /// Meshes first used since the last delivered snapshot, by index.
    pub new_meshes: Vec<(u32, crate::engine::api::mesh::RenderAsset)>,
    /// Projectile strategic icons first used since the last delivered
    /// snapshot, by index.
    pub new_projectile_icons: Vec<(u32, crate::engine::sim::projectile::ProjectileIcon)>,
    /// The game's file system, for the renderer to load meshes and textures
    /// from; sent with the first snapshot.
    pub vfs: Option<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>,
    /// The first snapshot of a session's engine (it carries `vfs`): the
    /// renderer drops the last session's world (entities, meshes, terrain,
    /// effects) before taking it. Every engine numbers its meshes, clips
    /// and effect assets from 0 again.
    pub new_session: bool,
    /// The sim's compiled-script cache (prewarmed), sent with `vfs`: the
    /// interface's VM shares it instead of compiling every script again.
    pub script_cache: Option<Arc<crate::engine::lua::script_cache::ScriptCache>>,
    pub debug: DebugData,
    /// Every army's economy (the UI's `GetEconomyTotals`).
    pub economy: Vec<ArmyEconomy>,
    /// The session's armies; with the first snapshot.
    pub armies: Option<Arc<Vec<ArmyInfo>>>,
    /// The army this client plays (its command source's), if any; with
    /// `armies`.
    pub local_army: Option<u32>,
    /// The army this client now views as (1-based; -1: observer) when the
    /// sim's `SetFocusArmy` changed it.
    pub focus_army: Option<i32>,
    /// How every army is shown; with the first snapshot and when it
    /// changed (`SetArmyColor`, `SetArmyShowScore`).
    pub army_display: Option<Arc<Vec<ArmyDisplay>>>,
    /// Every unit's custom name (`SetCustomName`: AI and campaign
    /// commanders), by id; with the first snapshot and when one changed.
    pub custom_names: Option<Arc<Vec<(u32, String)>>>,
    /// The explicit one-way alliances `(from, to, alliance)` (unset pairs
    /// are enemies); when they changed.
    pub alliances: Option<Arc<Vec<(u32, u32, crate::engine::sim::army::Alliance)>>>,
    /// The sim's `ScenarioInfo` (the user layer's `SessionGetScenarioInfo`),
    /// with the playable area added; with the first snapshot.
    pub scenario: Option<Arc<crate::engine::sync::SyncValue>>,
    /// `Sync` tables of the beats since the last delivered snapshot, oldest
    /// first (the user layer's `OnSync`).
    pub sync: Vec<crate::engine::sync::SyncValue>,
    /// World sounds since the last delivered snapshot, oldest first
    /// (`Entity:PlaySound`, weapons): the renderer hands them to the audio
    /// thread. Presentation only.
    pub sounds: Vec<crate::audio::WorldSound>,
    /// Every entity's looping ambient cue now (entity order), at the
    /// entity's position (`Entity:SetAmbientSound`).
    pub ambients: Vec<crate::audio::Ambient>,
    /// The fog of war texture's changes since the last delivered snapshot
    /// (`None`: unchanged).
    pub fog: Option<FogUpdate>,
    /// Fake radar contacts (enemy jammers) the view army sees now.
    pub fake_blips: Vec<[f32; 3]>,
    /// While fog of war is on: the view army's side's vision sources, in
    /// entity order. The renderer draws the clear ground as their circles,
    /// blended between snapshots as the units are (moho's vision pass), while
    /// what shows of other armies still follows the intel grids.
    pub vision: Vec<VisionSource>,
    /// The game is over (`EndGame` in an earlier tick): the sim no longer
    /// advances. The user layer's `SessionIsGameOver`, `NoteGameOver`.
    pub game_over: bool,
}

/// An army as the interface describes it (sent with the first snapshot).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmyInfo {
    pub index: u32,
    pub name: String,
    pub nickname: String,
    /// 1-based (1 UEF, 2 Aeon, 3 Cybran, 4 Seraphim).
    pub faction: u32,
    pub human: bool,
    pub civilian: bool,
}

/// How an army is shown (sim-set presentation state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmyDisplay {
    pub army: u32,
    /// ARGB: units, strategic icons, the interface's `iconColor`.
    pub color: u32,
    /// ARGB: the interface's `color`.
    pub player_color: u32,
    /// The interface's `showScore`.
    pub show_score: bool,
}

/// One army's resources this tick, per tick: `[stored, capacity, income,
/// requested, spent, reclaimed]` for mass and energy (`reclaimed`: the
/// game's running total; income includes it, a tick late, as moho's).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArmyEconomy {
    pub army: u32,
    pub mass: [f32; 6],
    pub energy: [f32; 6],
    /// Overflow goes to allies (`GetResourceSharing` for the focus army).
    pub resource_sharing: bool,
}

/// Installed by the windowed app; absent in headless runs.
pub struct SnapshotSink {
    /// Snapshots sent and not yet taken by the renderer.
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
    tx: SyncSender<Snapshot>,
    tick: u64,
    terrain_version: Option<u32>,
    sent_blueprints: bool,
    /// `Armies::alliance_version` last sent.
    sent_alliances: Option<u32>,
    /// The army display version the render side has.
    sent_army_display: Option<u32>,
    /// The focus army the render side has (from the first snapshot on).
    sent_focus_army: Option<i32>,
    /// `Presentation::decal_version` last sent.
    sent_decals: Option<u64>,
    /// `Presentation::custom_name_version` last sent.
    sent_custom_names: Option<u64>,
    /// The full prop list went out (later snapshots send changes).
    sent_props: bool,
    /// Prop changes taken from the world but not delivered yet.
    pending_props: Vec<u32>,
    /// Meshes the renderer has been told about (`MeshRegistry` prefix).
    sent_meshes: usize,
    sent_projectile_icons: usize,
    sent_vfs: bool,
    /// Clip paths seen so far (render-side numbering, not sim state).
    clip_ids: std::collections::BTreeMap<String, u32>,
    clips: Vec<String>,
    sent_clips: usize,
    sent_effect_assets: usize,
    /// Scratch: manipulator pose ops sorted by owner.
    pose_ops: Vec<(u32, Pose)>,
    hidden: Vec<(u32, u32)>,
    request: SharedDebugRequest,
    /// Last grid overlay sent (key: what it shows + source version).
    grid_key: Option<(u8, u32, u32)>,
    grid: Option<Arc<GridOverlay>>,
    /// The view army's fog of war (texture rows delivered, structures seen).
    fog: fog::FogSink,
    /// A snapshot saying the game is over was delivered.
    sent_game_over: bool,
}

/// A connected sink/receiver pair (capacity: a few snapshots) and the
/// renderer's debug request handle.
/// Snapshots the channel holds.
const CHANNEL_SLOTS: usize = 4;

/// A queued snapshot's place in the channel (see [`Snapshot::ticket`]).
#[derive(Debug)]
pub struct Ticket(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Ticket {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

pub fn channel() -> (SnapshotSink, Receiver<Snapshot>, SharedDebugRequest) {
    let (tx, rx) = std::sync::mpsc::sync_channel(CHANNEL_SLOTS);
    let request = SharedDebugRequest::default();
    let sink = SnapshotSink::new(tx, Default::default(), request.clone());
    (sink, rx, request)
}

impl SnapshotSink {
    fn new(tx: SyncSender<Snapshot>, in_flight: Arc<std::sync::atomic::AtomicUsize>, request: SharedDebugRequest) -> Self {
        SnapshotSink { in_flight, tx, tick: 0, terrain_version: None, sent_blueprints: false, sent_alliances: None, sent_focus_army: None, sent_army_display: None, sent_decals: None, sent_custom_names: None, sent_props: false, pending_props: Vec::new(), sent_meshes: 0, sent_projectile_icons: 0, sent_vfs: false, clip_ids: Default::default(), clips: Vec::new(), sent_clips: 0, sent_effect_assets: 0, pose_ops: Vec::new(), hidden: Vec::new(), request, grid_key: None, grid: None, fog: Default::default(), sent_game_over: false }
    }

    /// The same channel to the renderer for another session, with nothing
    /// sent yet (the next engine's assets, terrain and meshes all go again).
    pub fn renew(self) -> Self {
        Self::new(self.tx, self.in_flight, self.request)
    }
}

fn view_of(e: &crate::engine::sim::entity::Entity, structure: Option<&crate::engine::sim::category::CategorySet>) -> EntityView {
    let kind = match e.kind {
        EntityKind::Unit if e.cold.killed => ViewKind::Dying,
        EntityKind::Unit if e.category_index.zip(structure).is_some_and(|(i, s)| s.contains(i)) => ViewKind::Structure,
        EntityKind::Unit => ViewKind::Unit,
        EntityKind::Prop => ViewKind::Prop,
        EntityKind::Projectile => ViewKind::Projectile,
        _ => ViewKind::Other,
    };
    let f = |v: crate::engine::sim::math::Fx| v.to_num::<f32>();
    let (center, half) = match e.collision_shape() {
        CollisionShape::Box { center, half } => ([f(center.x), f(center.y), f(center.z)], [f(half.x), f(half.y), f(half.z)]),
        CollisionShape::Sphere { center, radius } => ([f(center.x), f(center.y), f(center.z)], [f(radius); 3]),
        // FAF takes an upgrade's collision away while it's built
        // (`SetCollisionShape('None')`).
        CollisionShape::None if e.fraction_complete < crate::engine::sim::math::Fx::ONE => ([0.0; 3], [0.0; 3]),
        CollisionShape::None => ([0.0, 0.25, 0.0], [0.25; 3]),
    };
    let p = e.position();
    let q = e.orientation;
    let health = if e.max_health > crate::engine::sim::math::Fx::ZERO { f(e.health / e.max_health).min(1.0) } else { 1.0 };
    EntityView {
        id: e.id,
        kind,
        army: e.army.unwrap_or(0),
        position: [f(p.x), f(p.y), f(p.z)],
        rotation: [f(q.x), f(q.y), f(q.z), f(q.w)],
        center,
        half,
        health,
        max_health: f(e.max_health),
        build_rate: e.unit.as_deref().map_or(0.0, |u| f(u.build_rate)),
        build: f(e.fraction_complete),
        work: e.unit.as_deref().map_or(0.0, |u| f(u.work_progress)),
        shield: e.unit.as_deref().filter(|u| u.has_shield).map_or(-1.0, |u| f(u.shield_ratio)),
        blueprint: if e.kind == EntityKind::Projectile { e.cold.strategic_icon } else { e.category_index }.unwrap_or(u32::MAX),
        mesh: e.cold.render_mesh.unwrap_or(u32::MAX),
        scale: [f(e.cold.draw_scale.x), f(e.cold.draw_scale.y), f(e.cold.draw_scale.z)],
        pose_start: 0,
        pose_len: 0,
        sighting: Sighting::Seen,
        in_builder: e.attachment.is_some() && e.kind == EntityKind::Unit && e.fraction_complete < crate::engine::sim::math::Fx::ONE,
        attached: e.attachment.is_some() && e.kind == EntityKind::Unit && e.fraction_complete >= crate::engine::sim::math::Fx::ONE,
        scroll: e.cold.scroller.map_or([0.0; 4], |s| [f(s.previous.0), f(s.previous.1), f(s.current.0), f(s.current.1)]),
        order_flags: e.unit.as_deref().map_or(0, |u| {
            static UPGRADING: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            let upgrading = *UPGRADING.get_or_init(|| crate::engine::sim::unit_state::UnitStates::mask(&["Upgrading"]));
            (if u.capturable { order_flags::CAPTURABLE } else { 0 })
                | (if u.states.any(upgrading) { order_flags::UPGRADING } else { 0 })
                | if u.fire_state == 2 { order_flags::GROUND_FIRE } else { 0 }
        }),
    }
}

/// Packs the drawable state (live entities in id order).
pub fn build(
    world: &EntityWorld,
    structure: Option<&crate::engine::sim::category::CategorySet>,
    out: &mut Vec<EntityView>,
) {
    out.clear();
    // Each snapshot's list is new (it moves to the renderer): size it once
    // rather than regrowing through ~6k pushes every tick. (Building views
    // in parallel measured 3x slower in game: rayon's wake + unindexed
    // collect for ~150 us of work.)
    out.reserve(world.len());
    out.extend(world.iter().filter(|e| !e.destroyed && drawn(e) && !stored(e)).map(|e| view_of(e, structure)));
}

/// Shield entities are drawn only while their bubble mesh is on (FAF sets
/// it when the shield is up and clears it when it drops). Entities that set
/// `SetVizTo*('Never')` for everyone aren't drawn at all.
fn drawn(e: &crate::engine::sim::entity::Entity) -> bool {
    (e.kind != EntityKind::Shield || e.cold.render_mesh.is_some()) && !e.cold.viz.hidden_from_all()
}

/// A unit inside a carrier: not drawn (only attached units are checked).
fn stored(e: &crate::engine::sim::entity::Entity) -> bool {
    e.attachment.is_some() && e.unit.as_deref().is_some_and(|u| u.stored_in.is_some())
}

/// [`build`] for the live entities `pick` selects.
pub fn build_where(
    world: &EntityWorld,
    structure: Option<&crate::engine::sim::category::CategorySet>,
    out: &mut Vec<EntityView>,
    pick: impl Fn(&crate::engine::sim::entity::Entity) -> bool + Sync,
) {
    use rayon::prelude::*;
    out.clear();
    let keep = |e: &&crate::engine::sim::entity::Entity| !e.destroyed && drawn(e) && !stored(e) && pick(e);
    let all = world.as_slice();
    if all.len() < VIEWS_PARALLEL_MIN {
        out.extend(all.iter().filter(keep).map(|e| view_of(e, structure)));
        return;
    }
    // Chunks in parallel, appended in id order.
    let chunks: Vec<Vec<EntityView>> = all
        .par_chunks(2048)
        .map(|chunk| {
            let _s = crate::prof_span!("snapshot entities (worker)");
            chunk.iter().filter(keep).map(|e| view_of(e, structure)).collect()
        })
        .collect();
    out.reserve(chunks.iter().map(Vec::len).sum());
    for c in chunks {
        out.extend(c);
    }
}

/// Entities from which [`build_where`] splits across threads.
const VIEWS_PARALLEL_MIN: usize = 16384;

/// Entities that have hidden a bone (`HideBone`), so snapshots visit only
/// those rather than every entity's cold half. Render bookkeeping: stale
/// ids (bones shown again, entity gone) drop out when next visited.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct HiddenBoneOwners(std::collections::BTreeSet<u32>);

impl FromIterator<u32> for HiddenBoneOwners {
    fn from_iter<I: IntoIterator<Item = u32>>(ids: I) -> Self {
        Self(ids.into_iter().collect())
    }
}

pub fn note_hidden_bones(lua: &mlua::Lua, id: u32) {
    if lua.app_data_ref::<HiddenBoneOwners>().is_none() {
        lua.set_app_data(HiddenBoneOwners::default());
    }
    if let Some(mut h) = lua.app_data_mut::<HiddenBoneOwners>() {
        h.0.insert(id);
    }
}

/// `(entity, bone)` for every hidden bone, in entity order; and, into
/// `placed`, a `Place` op for every bone in a copied pose or following another entity
/// (`AttachBoneToEntityBone`; hidden once that entity is gone).
pub fn hidden_bones(world: &EntityWorld, owners: &mut HiddenBoneOwners, out: &mut Vec<(u32, u32)>, placed: &mut Vec<(u32, Pose)>) {
    out.clear();
    placed.clear();
    let f = |v: crate::engine::sim::math::Fx| v.to_num::<f32>();
    owners.0.retain(|&id| match world.get(id) {
        Some(e) if !e.destroyed && (!e.cold.hidden_bones.is_empty() || !e.cold.bone_links.is_empty() || !e.cold.copied_pose.is_empty()) => {
            out.extend(e.cold.hidden_bones.iter().map(|&b| (id, b)));
            // A copied pose (`TryCopyPose`: a wreck in its unit's death pose).
            for &(bone, (p, r)) in &e.cold.copied_pose {
                let op = PoseOp::Place { bone, position: [f(p.x), f(p.y), f(p.z)], rotation: [f(r.x), f(r.y), f(r.z), f(r.w)] };
                placed.push((id, Pose { source: u32::MAX - 1, op }));
            }
            let inverse = e.orientation.conjugate();
            let scale = if e.cold.scale > crate::engine::sim::math::Fx::ZERO { e.cold.scale } else { crate::engine::sim::math::Fx::ONE };
            for &(bone, other, other_bone) in &e.cold.bone_links {
                match world.get(other).filter(|o| !o.destroyed) {
                    Some(o) => {
                        let local = inverse.rotate(o.bone_world_position(other_bone) - e.position());
                        let r = inverse.mul(o.bone_world_rotation(other_bone));
                        let op = PoseOp::Place { bone, position: [f(local.x / scale), f(local.y / scale), f(local.z / scale)], rotation: [f(r.x), f(r.y), f(r.z), f(r.w)] };
                        placed.push((id, Pose { source: u32::MAX - 1, op }));
                    }
                    None => out.push((id, bone)),
                }
            }
            true
        }
        _ => false,
    });
    out.sort_unstable();
}

/// The pose ops of every manipulator, by owner (manipulator order within
/// one owner): `sim::pose::ops_of`, the ops the sim poses bones with, in
/// floats. `clip` numbers clip paths.
pub fn manipulator_poses<'m>(
    manips: &'m crate::engine::sim::manipulator::Manipulators,
    weapons: Option<&crate::engine::sim::weapon::Weapons>,
    world: &EntityWorld,
    mut clip: impl FnMut(&str) -> u32,
    out: &mut Vec<(u32, Pose)>,
) {
    use crate::engine::sim::pose::Op;
    use rayon::prelude::*;
    let f = |v: crate::engine::sim::math::Fx| v.to_num::<f32>();
    let v3 = |v: crate::engine::sim::math::FxVec3| [f(v.x), f(v.y), f(v.z)];
    out.clear();
    // Owner order straight from the (owner, id) index (what sorting the
    // id-ordered list by owner gave, without the sort).
    // (owner, id) pairs, then each fetched in the parallel pass below: a
    // serial lookup per manipulator was most of this step.
    let ids: Vec<crate::engine::sim::manipulator::ManipulatorId> = manips.owner_order().collect();
    let list: Vec<&'m crate::engine::sim::manipulator::Manipulator> = if ids.len() >= POSES_PARALLEL_MIN {
        ids.par_chunks(2048).flat_map_iter(|c| c.iter().filter_map(|&id| manips.get(id)).filter(|m| crate::engine::sim::manipulator::Manipulators::poses(m)).collect::<Vec<_>>()).collect()
    } else {
        ids.iter().filter_map(|&id| manips.get(id)).filter(|m| crate::engine::sim::manipulator::Manipulators::poses(m)).collect()
    };
    // An op with its clip still a path (interned serially below).
    enum Pending<'a> {
        Ready(PoseOp),
        Clip { path: &'a str, time: f32, duration: f32, looping: bool },
        /// A slave (`CreateSlaver`): its owner's turns of `src` copied
        /// onto `dest`, once the owner's other ops are known (below).
        Slave { dest: u32, src: u32 },
    }
    let ops_for = |m: &&'m crate::engine::sim::manipulator::Manipulator| -> Vec<(u32, u32, Pending<'m>)> {
        let m: &'m crate::engine::sim::manipulator::Manipulator = m;
        let mut v = Vec::new();
        if manips.aim_hidden(m) {
            return v;
        }
        if let crate::engine::sim::manipulator::ManipulatorKind::Slave { dest: Some(dest), src: Some(src) } = m.kind {
            v.push((m.owner, m.id, Pending::Slave { dest, src }));
            return v;
        }
        let scale = world.get(m.owner).map_or(crate::engine::sim::math::Fx::ONE, |e| e.cold.scale);
        crate::engine::sim::pose::ops_of(&m.kind, m.owner, scale, weapons, None, |op| {
            let p = match op {
                Op::Clip { clip: path, time, duration, looping, .. } => Pending::Clip { path, time: f(time), duration: f(duration), looping },
                Op::Rotate { bone, axis, radians } => Pending::Ready(PoseOp::Rotate { bone, axis: v3(axis), radians: f(radians) }),
                Op::Translate { bone, offset } => Pending::Ready(PoseOp::Translate { bone, offset: v3(offset) }),
            };
            v.push((m.owner, m.id, p));
        });
        v
    };
    // (Thousands of units each with a few pods / rotators / clips: one
    // thread per chunk, merged in order.)
    let pending: Vec<Vec<(u32, u32, Pending<'m>)>> = if list.len() >= POSES_PARALLEL_MIN {
        list.par_chunks(512)
            .map(|chunk| {
                let _s = crate::prof_span!("snapshot poses (worker)");
                chunk.iter().flat_map(&ops_for).collect()
            })
            .collect()
    } else {
        vec![list.iter().flat_map(&ops_for).collect()]
    };
    out.reserve(pending.iter().map(Vec::len).sum());
    // Slaves of the owner being merged: (index in `out` it goes at, slave
    // id, dest, src). Resolved when the owner's run ends, from its other
    // ops (what `pose::ops_of` does per slave in the sim, without a
    // manipulator lookup per slave).
    let mut slaves: Vec<(usize, u32, u32, u32)> = Vec::new();
    let mut run = (u32::MAX, 0usize);
    let resolve = |out: &mut Vec<(u32, Pose)>, slaves: &mut Vec<(usize, u32, u32, u32)>, (owner, start): (u32, usize)| {
        if slaves.is_empty() {
            return;
        }
        let ops = out.split_off(start);
        let mut next = slaves.iter().peekable();
        for i in 0..=ops.len() {
            while let Some(&(_, source, dest, src)) = next.next_if(|s| s.0 == start + i) {
                for (_, p) in &ops {
                    if let PoseOp::Rotate { bone, axis, radians } = p.op
                        && bone == src {
                        out.push((owner, Pose { source, op: PoseOp::Rotate { bone: dest, axis, radians } }));
                    }
                }
            }
            if let Some(&o) = ops.get(i) {
                out.push(o);
            }
        }
        slaves.clear();
    };
    for (owner, source, p) in pending.into_iter().flatten() {
        if owner != run.0 {
            resolve(out, &mut slaves, run);
            run = (owner, out.len());
        }
        let op = match p {
            Pending::Ready(op) => op,
            Pending::Clip { path, time, duration, looping } => PoseOp::Clip { clip: clip(path), time, duration, looping },
            Pending::Slave { dest, src } => {
                slaves.push((out.len(), source, dest, src));
                continue;
            }
        };
        out.push((owner, Pose { source, op }));
    }
    resolve(out, &mut slaves, run);
}

/// The most placement spots answered a snapshot (a drag build's longest
/// line, `render::player::DRAG_BUILD_MAX`, with room for a template).
const PLACEMENT_QUERIES_MAX: usize = 128;

/// Manipulators from which [`manipulator_poses`] splits across threads.
const POSES_PARALLEL_MIN: usize = 4096;

/// Gives each view its slice of `poses`: its manipulator ops (`ops`, sorted
/// by owner) then its hidden bones (`hidden`, sorted by entity).
pub fn attach_poses(views: &mut [EntityView], ops: &[(u32, Pose)], hidden: &[(u32, u32)], poses: &mut Vec<Pose>) {
    poses.clear();
    let (mut i, mut j) = (0, 0);
    for v in views.iter_mut() {
        while i < ops.len() && ops[i].0 < v.id {
            i += 1;
        }
        while j < hidden.len() && hidden[j].0 < v.id {
            j += 1;
        }
        let start = poses.len();
        while i < ops.len() && ops[i].0 == v.id {
            poses.push(ops[i].1);
            i += 1;
        }
        while j < hidden.len() && hidden[j].0 == v.id {
            poses.push(Pose { source: u32::MAX, op: PoseOp::Hide { bone: hidden[j].1 } });
            j += 1;
        }
        v.pose_start = start as u32;
        v.pose_len = (poses.len() - start) as u32;
    }
}

/// Where `(entity, bone)` is now and which way it faces (rest pose).
fn anchor(world: &EntityWorld, entity: u32, bone: Option<u32>) -> Option<(crate::engine::sim::math::FxVec3, crate::engine::sim::math::FxQuat)> {
    world.get(entity).filter(|e| !e.destroyed)?;
    last_anchor(world, entity, bone)
}

/// As [`anchor`], for a destroyed entity too (its final transform, kept
/// until compaction).
pub(crate) fn last_anchor(world: &EntityWorld, entity: u32, bone: Option<u32>) -> Option<(crate::engine::sim::math::FxVec3, crate::engine::sim::math::FxQuat)> {
    let e = world.get(entity)?;
    let rotation = match (bone, &e.cold.skeleton) {
        (Some(b), Some(sk)) => sk.model.get(b as usize).map_or(e.orientation, |&(_, r)| e.orientation.mul(r)),
        _ => e.orientation,
    };
    Some((e.bone_world_position(bone), rotation))
}

/// The live effects as views (id order).
///
/// Views are built in parallel (each reads only the world, the effect and
/// the beams; nothing borrows Lua), then their asset names are interned
/// and curves gathered serially in id order, so the output doesn't depend
/// on the thread count. 63 armies of 1000 aircraft carry ~200k emitters:
/// serially this was 18 ms a tick.
fn effect_views(
    lua: &mlua::Lua,
    world: &EntityWorld,
    p: &crate::engine::sim::presentation::Presentation,
    out: &mut Vec<EffectView>,
    curves: &mut Vec<EffectCurve>,
) {
    use crate::engine::assets::effect_bp::CurveId;
    use crate::engine::sim::math::{Fx, FxQuat, FxVec3};
    use crate::engine::sim::presentation::EffectKind;
    use rayon::prelude::*;
    out.clear();
    curves.clear();
    let beams = lua.app_data_ref::<crate::engine::api::beam::Beams>();
    let beams = beams.as_deref();
    let f3 = |v: FxVec3| [v.x.to_num::<f32>(), v.y.to_num::<f32>(), v.z.to_num::<f32>()];
    let view_at = |id: u32, e: &crate::engine::sim::presentation::Effect, pos: FxVec3, rot: FxQuat, ended: bool| -> (EffectView, Vec<EffectCurve>) {
        let r = [rot.x.to_num::<f32>(), rot.y.to_num::<f32>(), rot.z.to_num::<f32>(), rot.w.to_num::<f32>()];
        let q = glam::Quat::from_array(r).normalize();
        let position = glam::Vec3::from_array(f3(pos)) + q * glam::Vec3::new(e.offset[0] as f32, e.offset[1] as f32, e.offset[2] as f32);
        let param = |k: &str| e.params.get(k).map_or(f32::NAN, |v| *v as f32);
        // Beam ends: the far entity, a collision beam's hit point, else
        // `LENGTH` along the anchor's facing.
        let end = match (e.kind, e.beam_to) {
            (EffectKind::Beam, Some((to, bone))) => anchor(world, to, bone).map(|(p, _)| f3(p)),
            (EffectKind::Beam, None) => e.attached.and_then(|(entity, _)| beams.and_then(|b| b.end(entity))).map(f3),
            _ => None,
        }
        .unwrap_or_else(|| match e.params.get("LENGTH") {
            Some(len) => (position + q * glam::Vec3::Z * (*len as f32)).to_array(),
            // No end known here: the renderer uses the blueprint's `Length`
            // along the bone (`CreateBeamEmitterOnEntity` exhausts, lasers).
            None => [f32::NAN; 3],
        });
        let own_curves: Vec<EffectCurve> = e
            .curves
            .iter()
            .filter_map(|(name, &(value, range))| CurveId::ALL.iter().find(|c| c.param() == name).map(|c| EffectCurve { curve: *c as u8, value: value as f32, range: range as f32 }))
            .collect();
        let view = EffectView {
            id,
            kind: match e.kind {
                EffectKind::Emitter => EffectViewKind::Emitter,
                EffectKind::Light { .. } => EffectViewKind::Light,
                EffectKind::Trail => EffectViewKind::Trail,
                EffectKind::Beam => EffectViewKind::Beam,
            },
            // Filled in below (interned when the effect was made).
            asset: u32::MAX,
            ramp: u32::MAX,
            army: e.army.unwrap_or(0),
            entity: e.source.map_or(u32::MAX, |(id, _)| id),
            bone: e.source.and_then(|(_, b)| b).unwrap_or(u32::MAX),
            attached: e.attached.is_some(),
            offset: [e.offset[0] as f32, e.offset[1] as f32, e.offset[2] as f32],
            position: position.to_array(),
            rotation: q.to_array(),
            end,
            scale: e.scale as f32,
            created: e.created,
            params: [param("LIFETIME"), param("REPEATTIME"), param("LENGTH"), param("THICKNESS"), param("SIZE")],
            curves_start: 0,
            curves_len: 0,
            ended,
        };
        (view, own_curves)
    };
    let view = |(&id, e): (&u32, &crate::engine::sim::presentation::Effect)| -> Option<(EffectView, Vec<EffectCurve>)> {
        let (pos, rot) = match e.attached {
            Some((entity, bone)) => {
                let (p, r) = anchor(world, entity, bone)?;
                match beams.and_then(|b| b.end(entity)) {
                    // A collision beam's frame runs along the beam (its +Z
                    // start to end; bone 1 is the far end), not its launcher's
                    // facing: the CZAR's beam fires straight down.
                    Some(end) => {
                        let start = world.get(entity).map_or(p, |e| e.position());
                        let r = match (end - start).normalized() {
                            Some(dir) => FxQuat::between(r.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE }), dir).mul(r).normalized(),
                            None => r,
                        };
                        (if bone == Some(1) { end } else { p }, r)
                    }
                    None => (p, r),
                }
            }
            None => (FxVec3::from_f64(e.position[0], e.position[1], e.position[2]), FxQuat::IDENTITY),
        };
        Some(view_at(id, e, pos, rot, false))
    };
    // Each chunk builds its finished views and curves (curve offsets local
    // to the chunk); the merge just appends, shifting the offsets.
    let chunk = |items: &[(&u32, &crate::engine::sim::presentation::Effect)]| -> (Vec<EffectView>, Vec<EffectCurve>) {
        let (mut views, mut own_curves) = (Vec::with_capacity(items.len()), Vec::new());
        for &(id, e) in items {
            let Some((mut v, own)) = view((id, e)) else { continue };
            v.asset = e.asset;
            v.ramp = e.ramp_asset;
            v.curves_start = own_curves.len() as u32;
            v.curves_len = own.len() as u32;
            own_curves.extend(own);
            views.push(v);
        }
        (views, own_curves)
    };
    let all: Vec<(&u32, &crate::engine::sim::presentation::Effect)> = p.effects().iter().map(|(k, e)| (k, &**e)).collect();
    let parts: Vec<(Vec<EffectView>, Vec<EffectCurve>)> = if all.len() >= EFFECTS_PARALLEL_MIN {
        all.par_chunks(1024)
            .map(|c| {
                let _s = crate::prof_span!("snapshot effects (worker)");
                chunk(c)
            })
            .collect()
    } else {
        vec![chunk(&all)]
    };
    out.reserve(parts.iter().map(|(v, _)| v.len()).sum());
    for (views, own_curves) in parts {
        let base = curves.len() as u32;
        out.extend(views.into_iter().map(|mut v| {
            v.curves_start += base;
            v
        }));
        curves.extend(own_curves);
    }
    // Trails whose entity died this tick: one last view each, at the
    // final transform, merged in id order (ids never repeat).
    if !p.ended_trails.is_empty() {
        let mut extra = Vec::with_capacity(p.ended_trails.len());
        for t in &p.ended_trails {
            let (mut v, own) = view_at(t.id, &t.effect, t.position, t.rotation, true);
            v.asset = t.effect.asset;
            v.ramp = t.effect.ramp_asset;
            v.curves_start = curves.len() as u32;
            v.curves_len = own.len() as u32;
            curves.extend(own);
            extra.push(v);
        }
        extra.sort_by_key(|v| v.id);
        let live = std::mem::take(out);
        out.reserve(live.len() + extra.len());
        let (mut i, mut j) = (0, 0);
        while i < live.len() || j < extra.len() {
            if j >= extra.len() || (i < live.len() && live[i].id < extra[j].id) {
                out.push(live[i]);
                i += 1;
            } else {
                out.push(extra[j]);
                j += 1;
            }
        }
    }
}

/// Effects from which [`effect_views`] splits the work across threads
/// (rayon's wake-up is ~100 µs; a view is ~90 ns).
const EFFECTS_PARALLEL_MIN: usize = 4096;

/// `General.CommandCaps` of each blueprint as [`caps`] bits (read once,
/// with the first snapshot).
fn blueprint_caps(lua: &mlua::Lua, ids: &[String]) -> Vec<u16> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let general = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten()).and_then(|bp| bp.raw_get::<Option<LuaTable>>("General").ok().flatten());
            use crate::engine::sim::unit_state::{bitfield_bits, cap_bit, COMMAND_CAPS};
            let Some(c) = general.and_then(|g| g.raw_get::<LuaValue>("CommandCaps").ok()) else { return 0 };
            let bits = bitfield_bits(&COMMAND_CAPS, "", &c).unwrap_or(0);
            caps::NAMES
                .iter()
                .enumerate()
                .filter(|(_, n)| cap_bit(&COMMAND_CAPS, &format!("RULEUCC_{n}")).is_some_and(|b| bits & b != 0))
                .fold(0u16, |acc, (i, _)| acc | 1 << i)
        })
        .collect()
}

/// Each blueprint's [`order_categories`] bits, in domain order.
fn blueprint_order_categories(lua: &mlua::Lua, d: &crate::engine::sim::category::CategoryDomain) -> Vec<u16> {
    use mlua::prelude::*;
    use order_categories as oc;
    let named = [
        ("RECLAIMABLE", oc::RECLAIMABLE),
        ("RECLAIMFRIENDLY", oc::RECLAIMFRIENDLY),
        ("FACTORY", oc::FACTORY),
        ("CAMPAIGNGATE", oc::CAMPAIGNGATE),
        ("COMMAND", oc::COMMAND),
        ("CANTRANSPORTCOMMANDER", oc::CANTRANSPORTCOMMANDER),
        ("AIRSTAGINGPLATFORM", oc::AIRSTAGINGPLATFORM),
        ("TRANSPORTATION", oc::TRANSPORTATION),
        ("TELEPORTATION", oc::TELEPORTATION),
        ("FERRYBEACON", oc::FERRYBEACON),
        ("TRANSPORTFOCUS", oc::TRANSPORTFOCUS),
        ("EXPERIMENTAL", oc::EXPERIMENTAL),
    ];
    let sets: Vec<(Option<&crate::engine::sim::category::CategorySet>, u16)> = named.iter().map(|&(n, b)| (d.named.get(n), b)).collect();
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    let air = |id: &str| -> bool {
        let physics = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id).ok().flatten()).and_then(|bp| bp.raw_get::<Option<LuaTable>>("Physics").ok().flatten());
        physics.and_then(|p| p.raw_get::<Option<String>>("MotionType").ok().flatten()).is_some_and(|m| m == "RULEUMT_Air")
    };
    (0..d.entries.len() as u32)
        .map(|i| {
            let cats = sets.iter().filter(|(s, _)| s.is_some_and(|s| s.contains(i))).fold(0, |acc, (_, b)| acc | b);
            cats | if air(&d.entries[i as usize].0) { oc::AIR } else { 0 }
        })
        .collect()
}

/// Each blueprint's `General.SelectionPriority` (moho's default 1, the
/// `RUnitBlueprintGeneral` constructor, MohoEngine.dll 0x1010d8bc; at least
/// 1, at most 127), with bit 7 for the `LOWSELECTPRIO` category.
fn blueprint_selection_priority(lua: &mlua::Lua, ids: &[String]) -> Vec<u8> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let bp = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten());
            let priority = bp
                .as_ref()
                .and_then(|bp| bp.raw_get::<Option<LuaTable>>("General").ok().flatten())
                .and_then(|g| g.raw_get::<Option<f64>>("SelectionPriority").ok().flatten())
                .unwrap_or(1.0)
                .clamp(1.0, 127.0) as u8;
            let low = bp
                .and_then(|bp| bp.raw_get::<Option<LuaTable>>("CategoriesHash").ok().flatten())
                .and_then(|c| c.raw_get::<Option<bool>>("LOWSELECTPRIO").ok().flatten())
                .unwrap_or(false);
            priority | if low { 0x80 } else { 0 }
        })
        .collect()
}

/// Per blueprint, its cargo-slot classes, its cargo class and its
/// `Transport.ClassGenericUpTo`, same index as [`blueprint_caps`]. The slot
/// classes are bits (bit `class - 1`, classes 1..4) from the model's
/// `attachpoint` bones — the same ones
/// [`crate::engine::api::transport::free_slots`] boards into, so the cursor
/// offers loads only where boarding can actually work (a docking pad's
/// `attachpoint` bones are class 1 only: an air-staging dock takes the
/// gunships that land on it, not the engineers classed 2 and 3). Class is
/// `Transport.TransportClass` as the blueprint has it, 1 when unset.
fn blueprint_transport(lua: &mlua::Lua, ids: &[String]) -> Vec<(u8, u8, u8)> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let Some(bp) = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten()) else { return (0, 1, 0) };
            let slots = crate::engine::api::mesh::skeleton_for(lua, &bp).ok().flatten().map_or(0u8, |s| {
                s.bones.iter().fold(0u8, |m, b| crate::engine::api::transport::slot_class(&b.name).map_or(m, |c| m | 1 << (c - 1)))
            });
            let transport = bp.raw_get::<Option<LuaTable>>("Transport").ok().flatten();
            let field = |name: &str| transport.as_ref().and_then(|t| t.raw_get::<Option<f64>>(name).ok().flatten());
            let class = field("TransportClass").map_or(1, |c| c.clamp(0.0, 255.0) as u8);
            let generic_up_to = field("ClassGenericUpTo").map_or(0, |c| c.clamp(0.0, 255.0) as u8);
            (slots, class, generic_up_to)
        })
        .collect()
}

/// The sim's `ScenarioInfo` as the user layer sees it: plus
/// `PlayableAreaWidth` / `PlayableAreaHeight` (the map size: playable rects
/// aren't implemented yet), which moho adds for the interface.
fn scenario_info(lua: &mlua::Lua) -> Option<crate::engine::sync::SyncValue> {
    use crate::engine::sync::SyncValue as V;
    let info: mlua::Value = lua.globals().raw_get("ScenarioInfo").ok()?;
    let mut v = V::from_lua(&info, 0);
    if let (V::Table(pairs), Some(t)) = (&mut v, lua.app_data_ref::<Terrain>()) {
        pairs.push((V::Str("PlayableAreaWidth".into()), V::Num(t.width as f64)));
        pairs.push((V::Str("PlayableAreaHeight".into()), V::Num(t.height as f64)));
    }
    Some(v)
}

impl SnapshotSink {
    /// Sends the file system and the compiled-script cache at once, before
    /// the game starts up, so the renderer can start the interface (its
    /// blueprint load and `userInit.lua`) while the sim is still loading.
    pub fn send_assets(&mut self, lua: &mlua::Lua) {
        let snap = Snapshot {
            vfs: lua.app_data_ref::<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().map(|v| v.clone()),
            script_cache: lua.app_data_ref::<Arc<crate::engine::lua::script_cache::ScriptCache>>().map(|c| c.clone()),
            new_session: true,
            ..Default::default()
        };
        if snap.vfs.is_none() {
            return;
        }
        if let Some(mut w) = lua.app_data_mut::<EntityWorld>() {
            w.track_props();
        }
        self.in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let snap = Snapshot { ticket: Some(Ticket(self.in_flight.clone())), ..snap };
        if self.tx.try_send(snap).is_ok() {
            self.sent_vfs = true;
        }
    }

    /// Sends the terrain as soon as the map is loaded, before the session's
    /// scripts run (`BeginSession`: FAF's navigation mesh takes seconds), so
    /// the renderer reads the map's textures, bakes its shadow and builds
    /// its mesh meanwhile instead of after the first tick.
    pub fn send_terrain(&mut self, lua: &mlua::Lua) {
        let Some(t) = lua.app_data_ref::<Terrain>() else { return };
        let snap = Snapshot {
            terrain: Some(Arc::new(TerrainView {
                width: t.width,
                height: t.height,
                heights: t.heights_f32(),
                water: t.water_surface.map(|w| w.to_num::<f32>()),
                map: lua.app_data_ref::<MapPath>().map(|p| p.0.clone()),
            })),
            ..Default::default()
        };
        self.in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let snap = Snapshot { ticket: Some(Ticket(self.in_flight.clone())), ..snap };
        if self.tx.try_send(snap).is_ok() {
            self.terrain_version = Some(t.version);
        }
    }
}

/// A `CreateDecal` / `CreateSplat` texture name as a VFS path: absolute
/// names as given, relative ones (FAF's tarmacs: `Tarmacs/Tar6x_01_albedo`)
/// under `/env/common/decals/`, `.dds` added when missing.
fn decal_texture(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let path = if name.starts_with('/') { name.to_lowercase() } else { format!("/env/common/decals/{}", name.to_lowercase()) };
    if path.ends_with(".dds") { path } else { format!("{path}.dds") }
}

/// The sim's decals and splats as map decals (see [`Snapshot::decals`]).
fn ground_decals(p: &crate::engine::sim::presentation::Presentation) -> Vec<crate::engine::assets::scmap::Decal> {
    use crate::engine::assets::scmap::{decal_kind, Decal};
    // Script positions are centres; map decals are placed by their corner,
    // u along (cos, sin) of the heading and v along (-sin, cos) (moho's
    // `CWldTerrainDecal::ComputeCorner`; `terrain::decal_corner`).
    // `lodParam` is the decal's `LODCutoff` as given; without one (<= 0)
    // moho's `ComputeCutoffLOD(-1)` sizes it from the diagonal:
    // `ren_DecalAlbedoLodCutoff` (15) for albedo / water albedo / glow mask,
    // `ren_DecalNormalLodCutoff` (6) for the rest.
    let decal = |kind: i32, textures: Vec<String>, centre: [f64; 3], heading: f64, size: [f64; 2], lod: f64, army: Option<u32>| {
        let lod = if lod > 0.0 {
            lod
        } else {
            let per_unit = if matches!(kind, decal_kind::ALBEDO | decal_kind::WATER_ALBEDO | decal_kind::GLOW_MASK) { 15.0 } else { 6.0 };
            size[0].hypot(size[1]) * per_unit
        };
        let (s, c) = (heading as f32).sin_cos();
        let (hx, hz) = (size[0] as f32 / 2.0, size[1] as f32 / 2.0);
        let corner = [centre[0] as f32 - (hx * c - hz * s), centre[1] as f32, centre[2] as f32 - (hx * s + hz * c)];
        Decal { kind, textures, scale: [size[0] as f32, 1.0, size[1] as f32], position: corner, rotation: [0.0, heading as f32, 0.0], cutoff: lod as f32, cutoff_min: 0.0, army: army.map_or(-1, |a| a as i32) }
    };
    let mut out = Vec::with_capacity(p.decals.len() + p.splats.len());
    for d in p.decals.values() {
        let kind = match d.kind.as_str() {
            "Albedo" => decal_kind::ALBEDO,
            "Normals" => decal_kind::NORMALS,
            "Alpha Normals" => decal_kind::NORMALS_ALPHA,
            "Glow" => decal_kind::GLOW,
            "Glow Mask" => decal_kind::GLOW_MASK,
            "Water Albedo" => decal_kind::WATER_ALBEDO,
            "Water Normals" => decal_kind::WATER_NORMALS,
            "Water Mask" => decal_kind::WATER_MASK,
            _ => continue,
        };
        let textures = d.textures.iter().filter(|t| !t.is_empty()).map(|t| decal_texture(t)).collect();
        out.push(decal(kind, textures, d.position, d.heading, d.size, d.lod, d.army));
    }
    for sp in &p.splats {
        out.push(decal(decal_kind::ALBEDO, vec![splat_texture(&sp.texture)], sp.position, sp.heading, sp.size, sp.lod, sp.army));
    }
    out
}

/// The range overlays' kinds, by their `OVERLAY<kind>` category (FAF adds
/// them to unit blueprints in `blueprints-units.lua`).
pub const RANGE_KINDS: [&str; 11] = ["ANTIAIR", "ANTINAVY", "DIRECTFIRE", "INDIRECTFIRE", "COUNTERMEASURE", "DEFENSE", "MISC", "RADAR", "SONAR", "OMNI", "COUNTERINTEL"];

/// Each blueprint's placement ghost assist ring (MohoEngine.dll 0x103b5fd0:
/// `AI.ShowAssistRangeOnSelect` ? `AI.GuardScanRadius` (if above 0) : 0;
/// read only by the placement ghost's rings, 0x103b7e50).
fn blueprint_assist_range(lua: &mlua::Lua, ids: &[String]) -> Vec<f32> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let ai = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten()).and_then(|bp| bp.raw_get::<Option<LuaTable>>("AI").ok().flatten());
            match ai {
                Some(ai) if ai.raw_get::<Option<bool>>("ShowAssistRangeOnSelect").ok().flatten() == Some(true) => (ai.raw_get::<Option<f64>>("GuardScanRadius").ok().flatten().unwrap_or(0.0) as f32).max(0.0),
                _ => 0.0,
            }
        })
        .collect()
}

/// Each blueprint's overlay ranges (see [`Snapshot::blueprint_ranges`]):
/// weapons by `RangeCategory` (`UWRC_<kind>`, min and max radius; a weapon
/// shows its `EffectiveRadius` when it has one, as moho's unit sync and range
/// overlay do, MohoEngine.dll 0x1028b6fa, 0x104814a0), the
/// shield's radius (defense), `AI.StagingPlatformScanRadius` (misc: FAF puts
/// engineers' build range there), radar / sonar / omni radii, and the
/// largest stealth or cloak field (counter-intel).
fn blueprint_ranges(lua: &mlua::Lua, ids: &[String]) -> Vec<Vec<(u8, f32, f32)>> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    let kind = |name: &str| RANGE_KINDS.iter().position(|k| *k == name).map(|k| k as u8);
    let num = |t: &LuaTable, k: &str| t.raw_get::<Option<f64>>(k).ok().flatten().unwrap_or(0.0) as f32;
    let sub = |t: &LuaTable, k: &str| t.raw_get::<Option<LuaTable>>(k).ok().flatten();
    ids.iter()
        .map(|id| {
            let mut out = Vec::new();
            let Some(bp) = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten()) else { return out };
            if let Some(weapons) = sub(&bp, "Weapon") {
                for w in weapons.sequence_values::<LuaTable>().flatten() {
                    let category: Option<String> = w.raw_get("RangeCategory").ok().flatten();
                    let Some(k) = category.as_deref().and_then(|c| c.strip_prefix("UWRC_")).and_then(|c| kind(&c.to_uppercase())) else { continue };
                    // `EffectiveRadius` defaults to -1: unset.
                    let effective = w.raw_get::<Option<f64>>("EffectiveRadius").ok().flatten().filter(|&e| e >= 0.0).map(|e| e as f32);
                    let max = effective.unwrap_or_else(|| num(&w, "MaxRadius"));
                    // A minimum beyond the effective range doesn't show.
                    let min = num(&w, "MinRadius");
                    let min = if effective.is_some_and(|e| min > e) { 0.0 } else { min };
                    if max > 0.0 {
                        out.push((k, min, max));
                    }
                }
            }
            let shield = sub(&bp, "Defense").and_then(|d| sub(&d, "Shield")).map_or(0.0, |s| num(&s, "ShieldSize") * 0.5);
            let staging = sub(&bp, "AI").map_or(0.0, |a| num(&a, "StagingPlatformScanRadius"));
            let intel = sub(&bp, "Intel");
            let intel_num = |k: &str| intel.as_ref().map_or(0.0, |i| num(i, k));
            let counter = intel_num("RadarStealthFieldRadius").max(intel_num("SonarStealthFieldRadius")).max(intel_num("CloakFieldRadius"));
            for (name, r) in [("DEFENSE", shield), ("MISC", staging), ("RADAR", intel_num("RadarRadius")), ("SONAR", intel_num("SonarRadius")), ("OMNI", intel_num("OmniRadius")), ("COUNTERINTEL", counter)] {
                if r > 0.0 {
                    out.extend(kind(name).map(|k| (k, 0.0, r)));
                }
            }
            out
        })
        .collect()
}

/// `StrategicIconName` of each blueprint (empty when it has none).
/// Each blueprint's selection box (see [`Snapshot::blueprint_selection`]).
fn blueprint_selection(lua: &mlua::Lua, ids: &[String]) -> Vec<[f32; 6]> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let Some(bp) = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten()) else { return [0.0; 6] };
            let get = |k: &str| bp.raw_get::<Option<f64>>(k).ok().flatten().unwrap_or(0.0) as f32;
            [get("SelectionSizeX"), get("SelectionSizeZ"), get("SelectionThickness"), get("SelectionCenterOffsetX"), get("SelectionCenterOffsetY"), get("SelectionCenterOffsetZ")]
        })
        .collect()
}

/// Each blueprint's pick box fields (see [`Snapshot::blueprint_pick`]).
fn blueprint_pick(lua: &mlua::Lua, ids: &[String]) -> Vec<[f32; 5]> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let Some(bp) = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten()) else { return [0.0, 0.5, 1.0, 1.0, 1.0] };
            let get = |k: &str, d: f64| bp.raw_get::<Option<f64>>(k).ok().flatten().unwrap_or(d) as f32;
            [get("SelectionMeshUseTopAmount", 0.0), get("SelectionYOffset", 0.5), get("SelectionMeshScaleX", 1.0), get("SelectionMeshScaleY", 1.0), get("SelectionMeshScaleZ", 1.0)]
        })
        .collect()
}

/// Each blueprint's lifebar (see [`Snapshot::blueprint_lifebars`]).
fn blueprint_lifebars(lua: &mlua::Lua, ids: &[String]) -> Vec<[f32; 3]> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let Some(bp) = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten()) else { return [0.0; 3] };
            if bp.raw_get::<Option<bool>>("LifeBarRender").ok().flatten() == Some(false) {
                return [-1.0, 0.0, 0.0];
            }
            let get = |k: &str| bp.raw_get::<Option<f64>>(k).ok().flatten().unwrap_or(0.0) as f32;
            [get("LifeBarSize"), get("LifeBarHeight"), get("LifeBarOffset")]
        })
        .collect()
}

/// The mesh render index and scale of each unit a player can build (a
/// `BUILTBY*` category; `u32::MAX`: other blueprints, or no mesh), for the
/// placement ghost. Mobile units too, not just structures: the cheat menu's
/// spawn mode (FAF's `createunit.lua`) shows the unit, a ship or an
/// aircraft, as the ghost. Civilian and campaign units are left out (several
/// name meshes the game data lacks). Registers the meshes (render-side
/// numbering, not sim state); the renderer loads one only when it draws it.
fn blueprint_meshes(lua: &mlua::Lua, ids: &[String]) -> Vec<(u32, f32)> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    let domain = crate::engine::api::categories::domain(lua).ok();
    let built: Vec<_> = domain.iter().flat_map(|d| d.named.iter().filter(|(n, _)| n.starts_with("BUILTBY")).map(|(_, s)| s.clone())).collect();
    ids.iter()
        .enumerate()
        .map(|(i, id)| {
            if !built.iter().any(|s| s.contains(i as u32)) {
                return (u32::MAX, 1.0);
            }
            bps.as_ref()
                .and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten())
                .and_then(|bp| {
                    let mesh = crate::engine::api::mesh::render_index_for(lua, &bp).ok().flatten()?;
                    let scale = crate::engine::api::entity::blueprint_scale(&bp).map_or(1.0, |s| s.to_num::<f32>());
                    Some((mesh, scale))
                })
                .unwrap_or((u32::MAX, 1.0))
        })
        .collect()
}

fn blueprint_icons(lua: &mlua::Lua, ids: &[String]) -> Vec<String> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            bps.as_ref()
                .and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten())
                .and_then(|bp| bp.raw_get::<Option<String>>("StrategicIconName").ok().flatten())
                .unwrap_or_default()
        })
        .collect()
}

/// `Display.IconFadeInZoom` of each blueprint: the camera zoom its
/// strategic icon shows from. 130 (nearly every unit's) when unset.
fn blueprint_icon_zoom(lua: &mlua::Lua, ids: &[String]) -> Vec<f32> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            bps.as_ref()
                .and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten())
                .and_then(|bp| bp.raw_get::<Option<LuaTable>>("Display").ok().flatten())
                .and_then(|d| d.raw_get::<Option<f64>>("IconFadeInZoom").ok().flatten())
                .map_or(130.0, |z| z as f32)
        })
        .collect()
}

/// Which layer a blueprint's strategic icon draws in, under a selected
/// unit's (3) and the hovered one: 2 when `StrategicIconSortPriority` is
/// under 65 (commanders, experimentals, T3 aircraft, mass extractors, intel structures), 1 for aircraft
/// (`Air.CanFly`), else 0. FA's icon pass (SupremeCommander.exe 0x8616a2,
/// the priority test at 0x861eb0) keeps four lists and draws them in that
/// order, each in entity order; it doesn't sort by the priority itself.
pub fn icon_layer(priority: Option<f64>, can_fly: bool) -> u8 {
    if priority.unwrap_or(0.0) < 65.0 {
        2
    } else if can_fly {
        1
    } else {
        0
    }
}

fn blueprint_icon_layer(lua: &mlua::Lua, ids: &[String]) -> Vec<u8> {
    use mlua::prelude::*;
    let bps: Option<LuaTable> = lua.globals().raw_get("__blueprints").ok().flatten();
    ids.iter()
        .map(|id| {
            let bp = bps.as_ref().and_then(|b| b.raw_get::<Option<LuaTable>>(id.as_str()).ok().flatten());
            let priority = bp.as_ref().and_then(|bp| bp.raw_get::<Option<f64>>("StrategicIconSortPriority").ok().flatten());
            let can_fly = bp
                .as_ref()
                .and_then(|bp| bp.raw_get::<Option<LuaTable>>("Air").ok().flatten())
                .and_then(|a| a.raw_get::<Option<bool>>("CanFly").ok().flatten())
                .unwrap_or(false);
            icon_layer(priority, can_fly)
        })
        .collect()
}

/// A splat texture's path: bare names live in `/env/common/splats/`.
fn splat_texture(name: &str) -> String {
    let path = if name.starts_with('/') { name.to_lowercase() } else { format!("/env/common/splats/{}", name.to_lowercase()) };
    if path.ends_with(".dds") { path } else { format!("{path}.dds") }
}

/// Where each queued order of `unit` leads (formation orders: its slot),
/// then a factory's command queue (its rally orders: the command graph
/// draws them from the factory, as moho's does for a factory's
/// `GetFactoryCommandQueue`). Their ETAs are [`order_etas`]'s, over every
/// unit's markers at once.
fn order_markers(
    world: &EntityWorld,
    commands: &crate::engine::sim::command::Commands,
    unit: u32,
    started: &std::collections::BTreeSet<crate::engine::sim::command::CommandId>,
    skirt: &dyn Fn(&str) -> Option<[f32; 4]>,
) -> Vec<OrderMarker> {
    use crate::engine::sim::command::{CommandKind, CommandTarget};
    if world.get(unit).is_none() {
        return Vec::new();
    }
    commands
        .queue(unit)
        .chain(commands.factory_queue(unit))
        .filter_map(|c| {
            let (at, node, target, movable) = match c.target {
                CommandTarget::Ground(p) => {
                    let movable = !(c.kind == CommandKind::BuildMobile && started.contains(&c.id));
                    match c.slot(unit) {
                        Some(s) => (crate::engine::sim::math::FxVec3 { x: p.x + s.dx, y: p.y, z: p.z + s.dz }, p, None, movable),
                        None => (p, p, None, movable),
                    }
                }
                CommandTarget::Entity(id) => {
                    let t = world.get(id).filter(|e| !e.destroyed)?;
                    let p = t.position();
                    (p, p, Some(id), t.kind == crate::engine::sim::entity::EntityKind::Unit)
                }
                CommandTarget::None => return None,
            };
            let blueprint = match (c.kind, &c.blueprint) {
                (CommandKind::BuildMobile, Some(bp)) => Some(bp.to_lowercase()),
                _ => None,
            };
            let site = blueprint.as_deref().and_then(skirt).map(|[x0, z0, x1, z1]| {
                let (x, z) = (node.x.to_num::<f32>(), node.z.to_num::<f32>());
                [x + x0, z + z0, x + x1, z + z1]
            });
            Some(OrderMarker { kind: c.kind as u8, command: c.id, at: f3(at), node: f3(node), site, blueprint, eta: 0.0, target, movable })
        })
        .collect()
}

/// What [`order_etas`] reads about the sim.
struct EtaInputs<'a> {
    world: &'a EntityWorld,
    commands: &'a crate::engine::sim::command::Commands,
    /// The structure each started build command is on.
    build_targets: &'a std::collections::BTreeMap<crate::engine::sim::command::CommandId, u32>,
    /// Blueprint `Economy.BuildTime`.
    build_time: &'a dyn Fn(&str) -> Option<f32>,
    /// The unit is an ENGINEER or a FACTORY (whose build rate counts).
    builds: &'a dyn Fn(u32) -> bool,
}

/// The waypoints' ETAs, as moho's command graph works them out
/// (`UICommandGraph.cpp`, MohoEngine.dll 0x103ef7c0, in ticks): a waypoint is
/// reached at the latest of its incoming lines (from a unit: now; from the
/// waypoint before: that one's ETA) plus the line's travel,
/// `ceil(distance x 10 / speed)` with the slowest `MaxSpeed` of the
/// order's units (0x103ef170); then a build order (BuildMobile /
/// BuildFactory) adds its work, `remaining x 10 / sum(1 / (BuildTime /
/// BuildRate))` over the order's ENGINEER / FACTORY units, remaining being
/// `1 - FractionComplete` of the structure they're on (0x103ef430, through
/// `/lua/game.lua` `GetConstructEconomyModel`). The economy doesn't come
/// into it: a stalled build's ETA holds still while it waits.
///
/// Ours also gathers everything working on one structure: the build
/// order's waypoint and the repair orders on the structure (helping build
/// it) share one ETA, the work done at the summed rate of every unit on it,
/// so the structure carries one label rather than one per order; and a
/// unit already at work on it has arrived (moho still counts its line to
/// the structure's centre, a few seconds for a builder standing in range).
fn order_etas(inputs: &EtaInputs, orders: &mut [(u32, Vec<OrderMarker>)]) {
    use crate::engine::sim::command::CommandKind as K;
    use std::collections::{BTreeMap, BTreeSet};
    let f = |v: crate::engine::sim::math::Fx| v.to_num::<f32>();
    let unit_of = |id: u32| inputs.world.get(id).filter(|e| !e.destroyed).and_then(|e| e.unit.as_deref().map(|u| (e, u)));
    // The structure (unfinished) each waypoint works on: a started build's,
    // or a repair order's target while it's being built.
    let job_of = |m: &OrderMarker| -> Option<u32> {
        let t = match K::from_u8(m.kind)? {
            K::BuildMobile | K::BuildFactory => *inputs.build_targets.get(&m.command)?,
            K::Repair => m.target?,
            _ => return None,
        };
        inputs.world.get(t).filter(|e| !e.destroyed && e.fraction_complete < crate::engine::sim::math::Fx::ONE).map(|_| t)
    };
    // Incoming lines by waypoint: (from: a unit or a waypoint, from point, to point).
    #[derive(Clone, Copy)]
    enum From {
        /// A unit; `true` when it's already at work on the waypoint's
        /// structure (it has arrived).
        Unit(bool),
        Node(u32),
    }
    let mut incoming: BTreeMap<u32, Vec<(From, [f32; 3], [f32; 3])>> = BTreeMap::new();
    let mut kinds: BTreeMap<u32, (u8, Option<String>, Option<u32>)> = BTreeMap::new();
    for (unit, marks) in orders.iter() {
        let Some((e, _)) = unit_of(*unit) else { continue };
        let focus = e.unit.as_deref().and_then(|u| u.focus);
        let mut from = (From::Unit(false), f3(e.position()));
        for (k, m) in marks.iter().enumerate() {
            if k == 0 {
                from.0 = From::Unit(focus.is_some() && focus == job_of(m));
            }
            incoming.entry(m.command).or_default().push((from.0, from.1, m.at));
            kinds.entry(m.command).or_insert_with(|| (m.kind, inputs.commands.get(m.command).and_then(|c| c.blueprint.as_ref()).map(|b| b.to_lowercase()), job_of(m)));
            from = (From::Node(m.command), m.at);
        }
    }
    // Each job's builders: every unit on it, and the units of every order
    // on it (those still on their way).
    let mut job_builders: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for (&command, (_, _, job)) in &kinds {
        if let (Some(t), Some(c)) = (job, inputs.commands.get(command)) {
            job_builders.entry(*t).or_default().extend(c.units.iter().copied());
        }
    }
    if !job_builders.is_empty() {
        for e in inputs.world.iter().filter(|e| !e.destroyed) {
            if let Some(t) = e.unit.as_deref().and_then(|u| u.focus).filter(|t| job_builders.contains_key(t)) {
                job_builders.entry(t).or_default().insert(e.id);
            }
        }
    }
    // Rate (work per second, 1 = the whole structure) of a set of builders.
    let rate = |units: &mut dyn Iterator<Item = u32>, bp: &str| -> f32 {
        let Some(time) = (inputs.build_time)(bp).filter(|&t| t > 0.0) else { return 0.0 };
        units.filter(|&u| (inputs.builds)(u)).filter_map(|u| unit_of(u).map(|(_, d)| f(d.build_rate))).filter(|&r| r > 0.0).map(|r| r / time).sum()
    };
    // Work ticks by job: what's left of it at its builders' summed rate.
    let mut job_work: BTreeMap<u32, i64> = BTreeMap::new();
    for (&t, builders) in &job_builders {
        let Some(bp) = inputs.world.get(t).map(|e| e.cold.blueprint_id.to_lowercase()) else { continue };
        let left = 1.0 - inputs.world.get(t).map_or(0.0, |e| f(e.fraction_complete));
        let r = rate(&mut builders.iter().copied(), &bp);
        job_work.insert(t, if r > 0.0 { (left.max(0.0) * 10.0 / r) as i64 } else { 0 });
    }
    // Waypoints by job, so a job's waypoints finish together.
    let mut job_nodes: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&command, (_, _, job)) in &kinds {
        if let Some(t) = job {
            job_nodes.entry(*t).or_default().push(command);
        }
    }
    // Slowest top speed of an order's units.
    let speed_of = |command: u32| -> f32 {
        inputs.commands.get(command).map_or(0.0, |c| {
            c.units.iter().filter_map(|&u| unit_of(u).map(|(_, d)| f(d.motion_params.max_speed))).filter(|&s| s > 0.0).fold(f32::INFINITY, f32::min)
        })
    };
    let speed_of = |command: u32| Some(speed_of(command)).filter(|s| s.is_finite()).unwrap_or(0.0);
    struct State<'s> {
        eta: BTreeMap<u32, i64>,
        busy: BTreeSet<u32>,
        incoming: &'s BTreeMap<u32, Vec<(From, [f32; 3], [f32; 3])>>,
        kinds: &'s BTreeMap<u32, (u8, Option<String>, Option<u32>)>,
        job_nodes: &'s BTreeMap<u32, Vec<u32>>,
        job_work: &'s BTreeMap<u32, i64>,
        /// A build not started yet: all of it, at its own units' rate.
        own_work: &'s BTreeMap<u32, i64>,
    }
    // When the units on `command`'s lines get there (ticks from now).
    fn arrival(s: &mut State, command: u32, speed_of: &dyn Fn(u32) -> f32) -> i64 {
        let speed = speed_of(command);
        let lines = s.incoming.get(&command).cloned().unwrap_or_default();
        let mut at = 0i64;
        for (from, a, b) in lines {
            // A unit's own node is a tick behind now (moho's, at 0x103ef869).
            let (start, there) = match from {
                From::Unit(there) => (-1, there),
                From::Node(n) => (node_eta(s, n, speed_of), false),
            };
            let d = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();
            let travel = if speed > 0.0 && !there { (d * 10.0 / speed).ceil() as i64 } else { 0 };
            at = at.max(start + travel);
        }
        at
    }
    fn node_eta(s: &mut State, command: u32, speed_of: &dyn Fn(u32) -> f32) -> i64 {
        if let Some(&e) = s.eta.get(&command) {
            return e;
        }
        // A loop back (patrols): as moho, the waypoint counts as now.
        if !s.busy.insert(command) {
            return 0;
        }
        let (_, _, job) = s.kinds.get(&command).cloned().unwrap_or((0, None, None));
        let eta = match job {
            Some(t) => {
                let nodes = s.job_nodes.get(&t).cloned().unwrap_or_default();
                let at = nodes.iter().map(|&n| arrival(s, n, speed_of)).max().unwrap_or(0);
                let eta = at + s.job_work.get(&t).copied().unwrap_or(0);
                for n in nodes {
                    s.eta.insert(n, eta);
                }
                eta
            }
            None => arrival(s, command, speed_of) + s.own_work.get(&command).copied().unwrap_or(0),
        };
        s.eta.insert(command, eta);
        eta
    }
    let mut own_work: BTreeMap<u32, i64> = BTreeMap::new();
    for (&command, (kind, bp, job)) in &kinds {
        if let (Some(K::BuildMobile | K::BuildFactory), Some(bp), None, Some(c)) = (K::from_u8(*kind), bp, job, inputs.commands.get(command)) {
            let r = rate(&mut c.units.iter().copied(), bp);
            own_work.insert(command, if r > 0.0 { (10.0 / r) as i64 } else { 0 });
        }
    }
    let mut s = State { eta: BTreeMap::new(), busy: BTreeSet::new(), incoming: &incoming, kinds: &kinds, job_nodes: &job_nodes, job_work: &job_work, own_work: &own_work };
    let commands: Vec<u32> = kinds.keys().copied().collect();
    for &command in &commands {
        node_eta(&mut s, command, &speed_of);
    }
    for (_, marks) in orders.iter_mut() {
        for m in marks {
            // Seconds; a waypoint with no time to it isn't labelled (moho's
            // ETA a tick before now, skipped at 0x103f0b96).
            m.eta = s.eta.get(&m.command).map_or(-1.0, |&t| if t > 0 { t as f32 / 10.0 } else { -1.0 });
        }
    }
}

/// Most selected units whose orders a snapshot carries.
const MAX_ORDER_UNITS: usize = 2000;

fn f3(v: crate::engine::sim::math::FxVec3) -> [f32; 3] {
    let (x, y, z) = v.to_f64();
    [x as f32, y as f32, z as f32]
}

/// Debug extras the renderer asked for (read-only over the sim).
fn debug_data(lua: &mlua::Lua, req: &DebugRequest, sink: &mut SnapshotSink) -> DebugData {
    use crate::engine::sim::command::{CommandTarget, Commands};
    use crate::engine::sim::weapon::{WeaponTarget, Weapons};
    let mut out = DebugData::default();
    let Some(world) = lua.app_data_ref::<EntityWorld>() else { return out };
    let commands = lua.app_data_ref::<Commands>();
    let weapons = lua.app_data_ref::<Weapons>();
    // Whose orders the player may see: its own side's (allies included);
    // observers (no view army) see everyone's. A selected enemy shows no
    // command graph, path or orders, and the interface gets no queue for it.
    let armies = lua.app_data_ref::<crate::engine::sim::army::Armies>();
    let orders_visible = |owner: Option<u32>| match (req.view_army, owner) {
        (None, _) => true,
        (Some(v), Some(o)) => o == v || armies.as_ref().is_some_and(|a| a.alliance(v, o) == crate::engine::sim::army::Alliance::Ally),
        (Some(_), None) => false,
    };
    if let Some(e) = req.selected.and_then(|id| world.get(id)).filter(|e| !e.destroyed) {
        let f = |v: crate::engine::sim::math::Fx| v.to_num::<f32>();
        let mut sel = Selected {
            id: e.id,
            blueprint: e.cold.blueprint_id.clone(),
            army: e.army.unwrap_or(0),
            health: (f(e.health), f(e.max_health)),
            build: f(e.fraction_complete),
            ..Default::default()
        };
        if let Some(u) = e.unit.as_deref() {
            sel.layer = u.layer.as_str().to_string();
            sel.focus = u.focus;
            sel.speed = f(u.motion_state.speed);
            if let Some(p) = u.path.as_ref().filter(|_| orders_visible(e.army)) {
                sel.path = p.waypoints[p.next.min(p.waypoints.len())..].iter().map(|w| f3(*w)).collect();
            }
        }
        if let Some(c) = &commands {
            sel.orders = c
                .queue(e.id)
                .filter(|_| orders_visible(e.army))
                .map(|cmd| {
                    let target = match cmd.target {
                        CommandTarget::Entity(t) => format!(" -> entity {t}"),
                        CommandTarget::Ground(p) => format!(" -> ({:.0}, {:.0})", p.x.to_num::<f32>(), p.z.to_num::<f32>()),
                        CommandTarget::None => String::new(),
                    };
                    let bp = cmd.blueprint.as_deref().map(|b| format!(" {b}")).unwrap_or_default();
                    format!("{:?}{bp}{target}", cmd.kind)
                })
                .collect();
        }
        if let Some(w) = &weapons {
            sel.weapons = w
                .iter()
                .filter(|s| s.unit == e.id)
                .map(|s| {
                    let target = match s.target {
                        Some(WeaponTarget::Entity(t)) => format!("target {t}"),
                        Some(WeaponTarget::Ground(_)) => "ground".to_string(),
                        None => "no target".to_string(),
                    };
                    format!("#{} range {:.0}, {target}, {}, reload {}", s.index, f(s.max_radius), if s.on_target { "on target" } else { "aiming" }, s.fire_clock)
                })
                .collect();
        }
        out.selected = Some(sel);
    }
    // Blueprint `Economy.BuildTime`, read once per blueprint per snapshot.
    let build_times = std::cell::RefCell::new(std::collections::BTreeMap::<String, Option<f32>>::new());
    let build_time = |bp: &str| -> Option<f32> {
        *build_times.borrow_mut().entry(bp.to_owned()).or_insert_with(|| {
            let bp = crate::engine::api::entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp).ok()?;
            bp.get::<Option<mlua::Table>>("Economy").ok()??.get::<Option<f64>>("BuildTime").ok()?.map(|v| v as f32)
        })
    };
    if let Some(c) = &commands {
        // Shift held: the view army's busy units join the selection (both
        // sorted by id, so the merge stays in id order).
        let mut units = req.selection.clone();
        if req.all_orders {
            // Factories' rally orders too.
            units.extend(c.busy_units().chain(c.factories_with_orders()).filter(|&id| req.view_army.is_none() || world.get(id).is_some_and(|e| e.army == req.view_army)));
            units.sort_unstable();
            units.dedup();
        }
        let (started, build_targets) = lua
            .app_data_ref::<crate::engine::api::construction::Builds>()
            .map(|b| (b.started_commands(), b.targets()))
            .unwrap_or_default();
        out.orders = units
            .iter()
            .filter(|&&id| world.get(id).is_some_and(|e| !e.destroyed && !e.cold.killed && orders_visible(e.army)))
            .take(MAX_ORDER_UNITS)
            .map(|&id| (id, order_markers(&world, c, id, &started, &|bp| crate::engine::api::build::skirt_of(lua, bp).ok().flatten())))
            .filter(|(_, m)| !m.is_empty())
            .collect();
        if !out.orders.is_empty() {
            // (The world is borrowed here: categories by hand.)
            let domain = crate::engine::api::categories::domain(lua).ok();
            let sets: Vec<_> = ["ENGINEER", "FACTORY"].iter().filter_map(|n| domain.as_ref()?.named.get(*n).cloned()).collect();
            let builds = |u: u32| world.get(u).and_then(|e| e.category_index).is_some_and(|c| sets.iter().any(|s| s.contains(c)));
            order_etas(&EtaInputs { world: &world, commands: c, build_targets: &build_targets, build_time: &build_time, builds: &builds }, &mut out.orders);
        }
    }
    if let Some(c) = &commands {
        let f = |v: crate::engine::sim::math::Fx| v.to_num::<f32>();
        // Who assists whom (current Guard / Repair orders), for the details.
        let mut assisting: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        // And what each carries: one pass over the world, not one per unit.
        let mut cargo: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        if !req.selection.is_empty() {
            for e in world.iter().filter(|e| !e.destroyed && e.unit.is_some()) {
                if let Some(a) = e.attachment {
                    cargo.entry(a.parent).or_default().push(e.id);
                }
            }
            use crate::engine::sim::command::CommandKind as K;
            for u in c.busy_units() {
                if let Some(CommandTarget::Entity(t)) = c.current(u).filter(|cmd| matches!(cmd.kind, K::Guard | K::Repair)).map(|cmd| cmd.target) {
                    assisting.entry(t).or_default().push(u);
                }
            }
        }
        let queue_factory = req.queue_factory.filter(|f| req.selection.binary_search(f).is_err());
        out.units = req
            .selection
            .iter()
            .take(MAX_ORDER_UNITS)
            .chain(queue_factory.as_ref())
            .filter_map(|&id| {
                let e = world.get(id).filter(|e| !e.destroyed && !e.cold.killed)?;
                let u = e.unit.as_deref()?;
                let commands = c
                    .queue(id)
                    .filter(|_| orders_visible(e.army))
                    .map(|cmd| {
                        let at = match cmd.target {
                            CommandTarget::Ground(p) => f3(p),
                            CommandTarget::Entity(t) => world.get(t).map_or([0.0; 3], |t| f3(t.position())),
                            CommandTarget::None => f3(e.position()),
                        };
                        (cmd.kind as u8, cmd.blueprint.clone(), cmd.count, at)
                    })
                    .collect();
                let factory_commands = c
                    .factory_queue(id)
                    .filter(|_| orders_visible(e.army))
                    .map(|cmd| {
                        let at = match cmd.target {
                            CommandTarget::Ground(p) => f3(p),
                            CommandTarget::Entity(t) => world.get(t).map_or([0.0; 3], |t| f3(t.position())),
                            CommandTarget::None => f3(e.position()),
                        };
                        (cmd.kind as u8, at)
                    })
                    .collect();
                let served = f(u.resource_consumed);
                let (cm, ce) = if u.consumption_active { (f(u.consumption_mass), f(u.consumption_energy)) } else { (0.0, 0.0) };
                let (pm, pe) = if u.production_active { (f(u.production_mass), f(u.production_energy)) } else { (0.0, 0.0) };
                Some(UnitDetail {
                    id,
                    commands,
                    factory_commands,
                    focus: u.focus,
                    econ: [cm * served, ce * served, pm, pe, cm, ce],
                    fuel: if u.fuel_use_time > crate::engine::sim::math::Fx::ZERO { f(u.fuel_ratio) } else { -1.0 },
                    fire_state: u.fire_state,
                    script_bits: u.script_bits,
                    paused: u.paused,
                    overcharge_paused: u.overcharge_paused,
                    missiles: {
                        use crate::engine::sim::weapon::SiloKind;
                        let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
                        let max = |kind: SiloKind| weapons.as_ref().map_or(0, |w| w.of(id).iter().filter(|s| s.silo == Some(kind)).map(|s| s.max_storage).max().unwrap_or(0)) as f32;
                        let building = u.states.contains(crate::engine::sim::unit_state::UnitStates::index("SiloBuildingAmmo").expect("unit state"));
                        let (nuke_max, tac_max) = (max(SiloKind::Nuke), max(SiloKind::Tactical));
                        let progress = if building { f(u.work_progress) } else { 0.0 };
                        let nuke_building = if nuke_max > 0.0 && (u.nuke_ammo as f32) < nuke_max { progress } else { 0.0 };
                        let tac_building = if nuke_max == 0.0 && tac_max > 0.0 { progress } else { 0.0 };
                        [nuke_building, nuke_max, u.nuke_ammo as f32, tac_building, tac_max, u.tactical_ammo as f32]
                    },
                    creator: u.creator,
                    auto_mode: u.auto_mode,
                    auto_surface: u.auto_surface,
                    submerged: u.layer == crate::engine::sim::layer::Layer::Sub,
                    cargo: cargo.get(&id).cloned().unwrap_or_default(),
                    assisters: assisting.get(&id).cloned().unwrap_or_default(),
                    stunned: u.stunned_until > crate::engine::api::sim_time::current_tick(lua),
                    guarding: c.current(id).filter(|cmd| cmd.kind == crate::engine::sim::command::CommandKind::Guard).and_then(|cmd| match cmd.target {
                        CommandTarget::Entity(t) => Some(t),
                        _ => None,
                    }),
                    restricted: {
                        // Its own, and those of the upgrades it has queued
                        // (the interface offers what they will build).
                        let mut r = crate::engine::api::build::restricted_buildable(lua, &e.cold.blueprint_id, e.army, u.build_restrictions.clone()).unwrap_or_default();
                        for bp in c.queue(id).filter(|cmd| cmd.kind == crate::engine::sim::command::CommandKind::Upgrade).filter_map(|cmd| cmd.blueprint.as_deref()) {
                            r.extend(crate::engine::api::build::restricted_buildable(lua, bp, e.army, None).unwrap_or_default());
                        }
                        r.sort_unstable();
                        r.dedup();
                        r
                    },
                    // `BTreeMap` iterates in name order (deterministic).
                    stats: u.stats.iter().map(|(k, v)| (k.clone(), *v as f32)).collect(),
                })
            })
            .collect();
    }
    if req.paths {
        out.paths = world
            .iter()
            .filter(|e| !e.destroyed)
            .filter_map(|e| {
                let p = e.unit.as_deref()?.path.as_deref()?;
                let mut pts = vec![f3(e.position())];
                pts.extend(p.waypoints[p.next.min(p.waypoints.len())..].iter().map(|w| f3(*w)));
                Some((e.id, pts))
            })
            .collect();
    }
    if req.weapon_ranges
        && let Some(w) = &weapons {
        out.ranges = w.iter().filter(|s| world.get(s.unit).is_some_and(|e| !e.destroyed)).map(|s| (s.unit, s.max_radius.to_num::<f32>())).collect();
    }
    // Grid overlay: rebuilt only when what it shows changed.
    let key = if let Some(layer) = req.nav_layer {
        lua.app_data_ref::<crate::engine::sim::nav::NavGrid>().map(|g| (layer, 0, g.version))
    } else if let Some(army) = req.threat_army {
        // Threat changes every tick: refresh about once a second.
        Some((10, army, (sink.tick / 10) as u32))
    } else {
        None
    };
    if key != sink.grid_key {
        sink.grid_key = key;
        sink.grid = match key {
            Some((layer, _, version)) if layer < 4 => lua.app_data_ref::<crate::engine::sim::nav::NavGrid>().map(|g| {
                // The 1x1 class of each overlay layer.
                let name = ["Vehicle1x1", "WaterLand1x1", "Water1x1", "Amphibious1x1"][layer as usize];
                let values = match g.class_named(name) {
                    Some(l) => (0..g.height).flat_map(|z| (0..g.width).map(move |x| (x, z))).map(|(x, z)| g.passable(l, x, z) as u8 as f32).collect(),
                    None => vec![0.0; (g.width * g.height) as usize],
                };
                Arc::new(GridOverlay { cols: g.width, rows: g.height, cell: 1.0, values, version: (layer as u64) << 32 | version as u64 })
            }),
            Some((_, army, version)) => {
                let armies = lua.app_data_ref::<crate::engine::sim::army::Armies>();
                lua.app_data_mut::<crate::engine::sim::threat::ThreatMap>().and_then(|mut m| {
                    crate::engine::api::threat::refresh(lua, &mut m, army);
                    let armies = armies?;
                    let enemy = |a: u32| armies.alliance(army, a) == crate::engine::sim::army::Alliance::Enemy;
                    let (cols, rows, cell, values) = m.grid_values(crate::engine::sim::threat::ThreatType::Overall, army, &enemy);
                    Some(Arc::new(GridOverlay { cols, rows, cell: cell as f32, values, version: 1 << 40 | (army as u64) << 32 | version as u64 }))
                })
            }
            None => None,
        };
    }
    out.grid = sink.grid.clone();
    out
}

/// End of tick: offer a snapshot if a renderer is attached. `tick_time` is
/// how long the tick took (for the renderer's status bar).
pub fn publish(lua: &mlua::Lua, tick_time: std::time::Duration) {
    // Renderer-only work: it must not move the gcid counter (a run with a
    // window and one without are the same game; `docs/architecture/replays.md`).
    let _frozen = has_sink(lua).then(|| crate::engine::lua::heap_snapshot::FrozenGcids::new(lua));
    offer(lua, tick_time, true);
}

/// Offers the current tick's snapshot again (no tick advance): while the sim
/// is held (`--screenshot-tick`), so a snapshot dropped on a full channel
/// still reaches the renderer.
pub fn republish(lua: &mlua::Lua) {
    let _frozen = has_sink(lua).then(|| crate::engine::lua::heap_snapshot::FrozenGcids::new(lua));
    offer(lua, std::time::Duration::ZERO, false);
}

/// The game is over and the renderer (if any) has been told: nothing more
/// to send.
/// A renderer is attached.
fn has_sink(lua: &mlua::Lua) -> bool {
    lua.app_data_ref::<SnapshotSink>().is_some()
}

pub fn game_over_delivered(lua: &mlua::Lua) -> bool {
    lua.app_data_ref::<SnapshotSink>().is_none_or(|s| s.sent_game_over)
}

fn offer(lua: &mlua::Lua, tick_time: std::time::Duration, advance: bool) {
    let Some(mut sink) = lua.app_data_mut::<SnapshotSink>() else {
        // No renderer to hear them.
        if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
            p.sounds.clear();
            p.camera_shakes.clear();
            p.upgrades.clear();
            p.camera_follows.clear();
            p.launches.clear();
        }
        return;
    };
    if advance {
        sink.tick += 1;
    }
    // The renderer is behind (game speed above its frame rate): this
    // snapshot couldn't be sent, so don't build it. One-off data and Sync
    // tables go with the next one; sounds too, the latest few.
    if sink.in_flight.load(std::sync::atomic::Ordering::Acquire) >= CHANNEL_SLOTS {
        if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
            let excess = p.sounds.len().saturating_sub(crate::engine::sim::presentation::MAX_PENDING_SOUNDS);
            p.sounds.drain(..excess);
            // Skipped snapshots: the next one's previous lacks these shots
            // by more than a tick; drawing them where they are is fine.
            p.launches.clear();
        }
        return;
    }
    let request = sink.request.lock().map(|r| r.clone()).unwrap_or_default();
    // Who the snapshot shows the world to (fog of war), if anyone.
    let view = fog::view(lua, &request);
    let domain = crate::engine::api::categories::domain(lua).ok();
    let structure = domain.as_ref().and_then(|d| d.named.get("STRUCTURE"));
    let mut snap = Snapshot { tick: sink.tick, sim_tick: crate::engine::api::sim_time::current_tick(lua), tick_ms: tick_time.as_secs_f32() * 1000.0, game_over: crate::engine::api::sim_time::has_ended(lua), ..Default::default() };
    // Props changed since the last delivered snapshot.
    let mut prop_ids = std::mem::take(&mut sink.pending_props);
    if let Some(mut w) = lua.app_data_mut::<EntityWorld>() {
        prop_ids.extend(w.take_prop_changes());
        prop_ids.sort_unstable();
        prop_ids.dedup();
    }
    if let Some(world) = lua.app_data_ref::<EntityWorld>() {
        let mut hidden = std::mem::take(&mut sink.hidden);
        let poses_span = crate::prof_span!("snapshot poses");
        let mut placed = Vec::new();
        match lua.app_data_mut::<HiddenBoneOwners>() {
            Some(mut owners) => hidden_bones(&world, &mut owners, &mut hidden, &mut placed),
            None => hidden.clear(),
        }
        let mut ops = std::mem::take(&mut sink.pose_ops);
        if let Some(m) = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>() {
            let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
            let SnapshotSink { clip_ids, clips, .. } = &mut *sink;
            // Exact path first (no allocation), then lowercased.
            let intern = |path: &str| {
                if let Some(&i) = clip_ids.get(path) {
                    return i;
                }
                let lower = path.to_lowercase();
                let i = match clip_ids.get(&lower) {
                    Some(&i) => i,
                    None => {
                        let i = clips.len() as u32;
                        clips.push(lower.clone());
                        clip_ids.insert(lower, i);
                        i
                    }
                };
                clip_ids.insert(path.to_owned(), i);
                i
            };
            manipulator_poses(&m, weapons.as_deref(), &world, intern, &mut ops);
        } else {
            ops.clear();
        }
        if !placed.is_empty() {
            // After each owner's manipulator ops (a stable sort keeps them in order).
            ops.extend(placed);
            ops.sort_by_key(|(owner, _)| *owner);
        }
        let _ = poses_span;
        {
            let _s = crate::prof_span!("snapshot entities");
            // Props only when posed (their ops must attach to a view here);
            // the rest come in `props` when they change.
            let posed = |id: u32| ops.binary_search_by_key(&id, |(o, _)| *o).is_ok() || hidden.binary_search_by_key(&id, |(o, _)| *o).is_ok();
            build_where(&world, structure, &mut snap.entities, |e| e.kind != EntityKind::Prop || posed(e.id));
            if let Some(c) = lua.app_data_ref::<crate::engine::sim::command::Commands>() {
                snap.busy.extend(c.busy_units());
                snap.busy.sort_unstable();
                snap.busy.dedup();
            }
            if !sink.sent_props {
                let mut props = Vec::new();
                build_where(&world, structure, &mut props, |e| e.kind == EntityKind::Prop);
                snap.props = Some(Arc::new(props));
            } else {
                snap.prop_changes = prop_ids
                    .iter()
                    .map(|&id| (id, world.get(id).filter(|e| !e.destroyed && e.kind == EntityKind::Prop).map(|e| view_of(e, structure))))
                    .collect();
            }
        }
        attach_poses(&mut snap.entities, &ops, &hidden, &mut snap.poses);
        sink.pose_ops = ops;
        sink.hidden = hidden;
    }
    if let (Some(world), Some(p)) = (lua.app_data_ref::<EntityWorld>(), lua.app_data_ref::<crate::engine::sim::presentation::Presentation>()) {
        let _s = crate::prof_span!("snapshot effects");
        effect_views(lua, &world, &p, &mut snap.effects, &mut snap.effect_curves);
        snap.new_effect_assets = p.assets.iter().enumerate().skip(sink.sent_effect_assets).map(|(i, a)| (i as u32, a.clone())).collect();
    }
    snap.new_clips = sink.clips.iter().enumerate().skip(sink.sent_clips).map(|(i, c)| (i as u32, c.clone())).collect();
    if let Some(t) = lua.app_data_ref::<Terrain>()
        && sink.terrain_version != Some(t.version) {
        snap.terrain = Some(Arc::new(TerrainView {
            width: t.width,
            height: t.height,
            heights: t.heights_f32(),
            water: t.water_surface.map(|w| w.to_num::<f32>()),
            map: lua.app_data_ref::<MapPath>().map(|p| p.0.clone()),
        }));
    }
    if !sink.sent_blueprints {
        if let Some(d) = &domain {
            let ids: Vec<String> = d.entries.iter().map(|(id, _)| id.clone()).collect();
            snap.blueprint_caps = Some(Arc::new(blueprint_caps(lua, &ids)));
            snap.blueprint_order_categories = Some(Arc::new(blueprint_order_categories(lua, d)));
            snap.blueprint_transport = Some(Arc::new(blueprint_transport(lua, &ids)));
            snap.blueprint_selection_priority = Some(Arc::new(blueprint_selection_priority(lua, &ids)));
            snap.blueprint_icons = Some(Arc::new(blueprint_icons(lua, &ids)));
            snap.blueprint_icon_zoom = Some(Arc::new(blueprint_icon_zoom(lua, &ids)));
            snap.blueprint_icon_layer = Some(Arc::new(blueprint_icon_layer(lua, &ids)));
            snap.blueprint_meshes = Some(Arc::new(blueprint_meshes(lua, &ids)));
            snap.blueprint_selection = Some(Arc::new(blueprint_selection(lua, &ids)));
            snap.blueprint_pick = Some(Arc::new(blueprint_pick(lua, &ids)));
            snap.blueprint_lifebars = Some(Arc::new(blueprint_lifebars(lua, &ids)));
            snap.blueprint_skirts = Some(Arc::new(ids.iter().map(|id| crate::engine::api::build::skirt_of(lua, id).ok().flatten()).collect()));
            snap.blueprint_ranges = Some(Arc::new(blueprint_ranges(lua, &ids)));
            snap.blueprint_assist_range = Some(Arc::new(blueprint_assist_range(lua, &ids)));
            snap.blueprints = Some(Arc::new(ids));
        }
        snap.deposits = lua.app_data_ref::<crate::engine::sim::deposit::Deposits>().map(|d| Arc::new(crate::engine::api::deposits::list(&d)));
    }
    if let Some(r) = lua.app_data_ref::<crate::engine::api::mesh::MeshRegistry>() {
        snap.new_meshes = r.assets.iter().enumerate().skip(sink.sent_meshes).map(|(i, a)| (i as u32, a.clone())).collect();
    }
    if let Some(icons) = lua.app_data_ref::<crate::engine::sim::projectile::ProjectileIcons>() {
        snap.new_projectile_icons = icons.0.iter().enumerate().skip(sink.sent_projectile_icons).map(|(i, icon)| (i as u32, icon.clone())).collect();
    }
    if !sink.sent_vfs {
        snap.vfs = lua.app_data_ref::<Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().map(|v| v.clone());
        snap.script_cache = lua.app_data_ref::<Arc<crate::engine::lua::script_cache::ScriptCache>>().map(|c| c.clone());
    }
    if let Some(armies) = lua.app_data_ref::<crate::engine::sim::army::Armies>() {
        let r = |x: &crate::engine::sim::economy::ResourceState| [x.stored, x.capacity, x.income, x.requested, x.expense, x.reclaimed].map(|v| v.to_num::<f32>());
        snap.economy = armies.list.iter().map(|a| ArmyEconomy { army: a.index, mass: r(&a.economy.mass), energy: r(&a.economy.energy), resource_sharing: a.resource_sharing }).collect();
        if !sink.sent_blueprints {
            snap.scenario = scenario_info(lua).map(Arc::new);
            let source = crate::engine::api::command_source::local_source(lua);
            snap.local_army = armies.list.iter().find(|a| a.command_sources.contains(&source)).map(|a| a.index);
            snap.armies = Some(Arc::new(armies.list.iter().map(|a| ArmyInfo { index: a.index, name: a.name.clone(), nickname: a.nickname.clone(), faction: a.faction, human: a.human, civilian: a.civilian }).collect()));
        }
        // The first snapshot's `local_army` covers the start (the player
        // may override it); later `SetFocusArmy` changes are sent.
        let focus = lua.app_data_ref::<crate::engine::api::army::FocusArmy>().map_or(-1, |f| f.0);
        if sink.sent_focus_army.is_some_and(|f| f != focus) {
            snap.focus_army = Some(focus);
        }
        sink.sent_focus_army = Some(focus);
        if sink.sent_army_display != Some(armies.display_version()) {
            snap.army_display = Some(Arc::new(armies.list.iter().map(|a| ArmyDisplay { army: a.index, color: a.color, player_color: a.player_color, show_score: a.show_score }).collect()));
            sink.sent_army_display = Some(armies.display_version());
        }
        if sink.sent_alliances != Some(armies.alliance_version()) {
            snap.alliances = Some(Arc::new(armies.alliances().collect()));
            sink.sent_alliances = Some(armies.alliance_version());
        }
    }
    if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
        snap.splats = std::mem::take(&mut p.fresh_splats)
            .into_iter()
            .map(|sp| GroundSplat {
                texture: splat_texture(&sp.texture),
                centre: sp.position.map(|v| v as f32),
                heading: sp.heading as f32,
                size: sp.size.map(|v| v as f32),
                created: sp.created,
                duration: sp.duration as f32,
                lod: sp.lod as f32,
                army: sp.army,
            })
            .collect();
        if sink.sent_decals != Some(p.decal_version) {
            snap.decals = Some(Arc::new(ground_decals(&p)));
            sink.sent_decals = Some(p.decal_version);
        }
        if sink.sent_custom_names != Some(p.custom_name_version)
            && let Some(world) = lua.app_data_ref::<EntityWorld>()
        {
            snap.custom_names = Some(Arc::new(world.iter().filter(|e| !e.destroyed).filter_map(|e| Some((e.id, e.unit.as_deref()?.custom_name.clone()?))).collect()));
            sink.sent_custom_names = Some(p.custom_name_version);
        }
        let _s = crate::prof_span!("snapshot sounds");
        // Nothing reads the shakes yet; don't let them pile up.
        p.camera_shakes.clear();
        snap.upgrades = std::mem::take(&mut p.upgrades);
        snap.camera_follows = std::mem::take(&mut p.camera_follows);
        snap.launches = std::mem::take(&mut p.launches);
        let sight = fog::SoundSight::new(lua, view);
        snap.sounds = p
            .sounds
            .drain(..)
            .filter(|e| e.position.is_none_or(|at| sight.hears(e.entity, at)))
            .map(|e| crate::audio::WorldSound { bank: e.bank, cue: e.cue, position: e.position, lod_cutoff: e.lod_cutoff })
            .collect();
        if !p.ambient.is_empty()
            && let Some(world) = lua.app_data_ref::<EntityWorld>() {
            // Loops of entities gone are dropped here (the sim never
            // reads this map).
            p.ambient.retain(|&id, _| world.get(id).is_some_and(|e| !e.destroyed));
            snap.ambients = p
                .ambient
                .iter()
                .filter_map(|(&entity, a)| {
                    let at = f3(world.get(entity)?.position());
                    sight.hears(Some(entity), at).then(|| crate::audio::Ambient { entity, bank: a.bank.clone(), cue: a.cue.clone(), position: at })
                })
                .collect();
        }
    }
    snap.placement = request
        .placement
        .iter()
        .take(PLACEMENT_QUERIES_MAX)
        .map(|(bp, at)| {
            use crate::engine::sim::math::{Fx, FxVec3};
            let pos = FxVec3 { x: Fx::from_num(at[0]), y: Fx::from_num(at[1]), z: Fx::from_num(at[2]) };
            let ok = crate::engine::api::build::can_build_at(lua, bp, pos).unwrap_or(false);
            let site = crate::engine::api::build::skirt_of(lua, bp).ok().flatten().map(|[x0, z0, x1, z1]| [at[0] + x0, at[2] + z0, at[0] + x1, at[2] + z1]);
            (bp.clone(), *at, site, ok)
        })
        .collect();
    snap.debug = {
        let _s = crate::prof_span!("snapshot debug + unit details");
        debug_data(lua, &request, &mut sink)
    };
    snap.debug.draw = crate::engine::api::debug_draw::current(lua);
    fog::apply(lua, &mut sink.fog, view, &mut snap);
    let (terrain_version, blueprints) = (lua.app_data_ref::<Terrain>().map(|t| t.version), snap.blueprints.is_some());
    let full_props = snap.props.is_some();
    let (meshes, vfs) = (snap.new_meshes.last().map(|(i, _)| *i as usize + 1), snap.vfs.is_some());
    let clips = snap.new_clips.last().map(|(i, _)| *i as usize + 1);
    let projectile_icons = snap.new_projectile_icons.last().map(|(i, _)| *i as usize + 1);
    let effect_assets = snap.new_effect_assets.last().map(|(i, _)| *i as usize + 1);
    let game_over = snap.game_over;
    snap.sync = lua.app_data_mut::<crate::engine::sync::PendingSync>().map(|mut p| std::mem::take(&mut p.0)).unwrap_or_default();
    let _send = crate::prof_span!("snapshot send");
    sink.in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    snap.ticket = Some(Ticket(sink.in_flight.clone()));
    match sink.tx.try_send(snap) {
        Ok(()) => {
            // Only mark one-off data as delivered once it actually went out.
            if terrain_version.is_some() {
                sink.terrain_version = terrain_version;
            }
            sink.sent_blueprints |= blueprints;
            sink.sent_props |= full_props;
            sink.sent_vfs |= vfs;
            sink.sent_game_over |= game_over;
            sink.fog.delivered();
            if let Some(n) = meshes {
                sink.sent_meshes = n;
            }
            if let Some(n) = clips {
                sink.sent_clips = n;
            }
            if let Some(n) = projectile_icons {
                sink.sent_projectile_icons = n;
            }
            if let Some(n) = effect_assets {
                sink.sent_effect_assets = n;
            }
        }
        // Not delivered: keep its syncs, splats and prop changes for the next one.
        Err(TrySendError::Full(mut snap)) | Err(TrySendError::Disconnected(mut snap)) => {
            sink.pending_props = prop_ids;
            sink.fog.dropped();
            if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
                let later = std::mem::take(&mut p.fresh_splats);
                p.fresh_splats = snap
                    .splats
                    .iter()
                    .map(|g| crate::engine::sim::presentation::Splat {
                        position: g.centre.map(f64::from),
                        heading: g.heading as f64,
                        texture: g.texture.clone(),
                        size: g.size.map(f64::from),
                        lod: g.lod as f64,
                        duration: g.duration as f64,
                        army: g.army,
                        created: g.created,
                    })
                    .collect();
                p.fresh_splats.extend(later);
            }
            if !snap.camera_follows.is_empty()
                && let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
                let later = std::mem::take(&mut p.camera_follows);
                p.camera_follows = snap.camera_follows.clone();
                p.camera_follows.extend(later);
            }
            if !snap.launches.is_empty()
                && let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
                let later = std::mem::take(&mut p.launches);
                p.launches = std::mem::take(&mut snap.launches);
                p.launches.extend(later);
            }
            if !snap.sounds.is_empty()
                && let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
                let later = std::mem::take(&mut p.sounds);
                p.sounds = snap
                    .sounds
                    .into_iter()
                    .map(|w| crate::engine::sim::presentation::SoundEvent { entity: None, bank: w.bank, cue: w.cue, lod_cutoff: w.lod_cutoff, position: w.position })
                    .collect();
                p.sounds.extend(later);
            }
            if !snap.sync.is_empty()
                && let Some(mut p) = lua.app_data_mut::<crate::engine::sync::PendingSync>() {
                let later = std::mem::take(&mut p.0);
                p.0 = snap.sync;
                p.0.extend(later);
            }
        }
    }
}

#[cfg(test)]
mod speed_tests {
    use super::SimControl;

    #[test]
    fn moho_speed_steps() {
        assert_eq!(SimControl::percent_of(0), 100);
        assert_eq!(SimControl::percent_of(10), 1000);
        assert_eq!(SimControl::percent_of(-10), 10);
        assert_eq!(SimControl::percent_of(3), 200);
        assert_eq!(SimControl::step_of(200), 3);
        assert_eq!(SimControl::step_of(50), -3);
        // No limit at +10 (moho's): +20 is 100x, +30 1000x, then "max".
        assert_eq!(SimControl::percent_of(20), 10_000);
        assert_eq!(SimControl::percent_of(30), 100_000);
        assert_eq!(SimControl::percent_of(SimControl::MAX_STEP), SimControl::MAX);
        assert_eq!(SimControl::percent_of(500), SimControl::MAX);
        assert_eq!(SimControl::step_of(SimControl::MAX), SimControl::MAX_STEP);
        assert_eq!(SimControl::percent_of(-40), 1);
        // +/- move exactly one step each way.
        let c = SimControl::new(100);
        for want in [1, 2, 3, 2, 1, 0, -1] {
            c.step_moho(want > SimControl::step_of(c.speed()));
            assert_eq!(SimControl::step_of(c.speed()), want);
        }
        // Whole percents tell steps apart from -15 (3%) up.
        for step in -15..=SimControl::MAX_STEP {
            assert_eq!(SimControl::step_of(SimControl::percent_of(step)), step);
        }
    }
}

#[cfg(test)]
mod viz_tests {
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::{EntityKind, EntityWorld, VisionMode};
    use crate::engine::sim::math::{FxQuat, FxVec3};

    /// FAF's shield collider (`ShieldCollider_proj.bp`, a box mesh that
    /// falls with every dying plane) sets `SetVizTo*('Never')` for all;
    /// it must not be drawn (bright boxes on dying planes).
    #[test]
    fn hidden_from_all_is_not_drawn() {
        let mut world = EntityWorld::default();
        let shown = world.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        let hidden = world.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        let partly = world.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        let v = &mut world.get_mut(hidden).unwrap().cold.viz;
        (v.focus, v.allies, v.enemies, v.neutrals) = (VisionMode::Never, VisionMode::Never, VisionMode::Never, VisionMode::Never);
        world.get_mut(partly).unwrap().cold.viz.enemies = VisionMode::Never;
        let mut out = Vec::new();
        super::build(&world, None, &mut out);
        let ids: Vec<u32> = out.iter().map(|v| v.id).collect();
        assert_eq!(ids, vec![shown, partly]);
        out.clear();
        super::build_where(&world, None, &mut out, |_| true);
        assert_eq!(out.iter().map(|v| v.id).collect::<Vec<_>>(), vec![shown, partly]);
    }

    /// A killed unit still falling or sinking is drawn, but as `Dying`: the
    /// interface's unit checks (icons, picking, selection) skip it.
    #[test]
    fn killed_unit_is_drawn_as_dying() {
        use super::ViewKind;
        let mut world = EntityWorld::default();
        let alive = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        let killed = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        world.get_mut(killed).unwrap().cold.killed = true;
        let mut out = Vec::new();
        super::build(&world, None, &mut out);
        let kinds: Vec<(u32, ViewKind)> = out.iter().map(|v| (v.id, v.kind)).collect();
        assert_eq!(kinds, vec![(alive, ViewKind::Unit), (killed, ViewKind::Dying)]);
        // Only the live one reaches the interface.
        assert_eq!(out.iter().map(|v| v.is_interface_unit()).collect::<Vec<_>>(), vec![true, false]);
    }
}

#[cfg(test)]
mod ended_trail_tests {
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::{EntityKind, EntityWorld};
    use crate::engine::sim::math::{FxQuat, FxVec3};
    use crate::engine::sim::presentation::{Effect, EffectKind, Presentation};

    fn trail_on(entity: u32) -> Effect {
        Effect {
            kind: EffectKind::Trail,
            blueprint: "/effects/emitters/riot_gun_polytrail_01_emit.bp".into(),
            army: Some(1),
            position: [0.0; 3],
            attached: Some((entity, None)),
            source: Some((entity, None)),
            scale: 1.0,
            offset: [0.0; 3],
            params: Default::default(),
            curves: Default::default(),
            curve_lengths: Default::default(),
            ramp: None,
            asset: 0,
            ramp_asset: u32::MAX,
            beam_to: None,
            created: 1,
            lifetime: None,
        }
    }

    /// A shell destroyed on the tick it hits: its trail is shown once more
    /// at the impact point (flagged `ended`), then not at all, so the
    /// renderer lays the last segment instead of stopping a tick short.
    #[test]
    fn trail_of_a_destroyed_entity_ends_at_its_final_position() {
        let lua = mlua::Lua::new();
        let mut world = EntityWorld::default();
        let shell = world.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(1), FxVec3::from_f64(8.0, 5.0, 0.0), FxQuat::IDENTITY);
        let mut p = Presentation::default();
        let trail = p.add_effect(trail_on(shell));
        // The impact tick: moved to the target, then destroyed there.
        world.set_position(shell, FxVec3::from_f64(16.0, 5.0, 0.0));
        assert!(world.destroy(shell));
        lua.set_app_data(world);
        lua.set_app_data(p);
        crate::engine::api::effect::expire(&lua);
        let world = lua.remove_app_data::<EntityWorld>().unwrap();
        let p = lua.remove_app_data::<Presentation>().unwrap();
        assert!(p.effects().is_empty(), "the orphaned trail is dropped");
        let (mut views, mut curves) = (Vec::new(), Vec::new());
        super::effect_views(&lua, &world, &p, &mut views, &mut curves);
        assert_eq!(views.len(), 1);
        assert_eq!((views[0].id, views[0].ended), (trail, true));
        assert_eq!(views[0].position, [16.0, 5.0, 0.0]);
        // Next tick: gone.
        lua.set_app_data(world);
        lua.set_app_data(p);
        crate::engine::api::effect::expire(&lua);
        let world = lua.remove_app_data::<EntityWorld>().unwrap();
        let p = lua.remove_app_data::<Presentation>().unwrap();
        super::effect_views(&lua, &world, &p, &mut views, &mut curves);
        assert!(views.is_empty());
    }
}

#[cfg(test)]
mod decal_tests {
    use super::*;
    use crate::engine::sim::presentation::{Decal, Presentation, Splat};

    /// `CreateDecal` / `CreateSplat`'s `lodParam` reaches the renderer as the
    /// decal's `LODCutoff` (FAF's scorches: 200).
    #[test]
    fn script_decals_carry_their_lod_cutoff() {
        let mut p = Presentation::default();
        let decal = Decal { position: [10.0, 0.0, 10.0], heading: 0.0, textures: ["Scorch_generic_002_albedo".into(), String::new()], kind: "Albedo".into(), size: [7.0, 7.0], lod: 200.0, duration: 0.0, army: None, created: 0 };
        p.add_decal(decal);
        p.add_splat(Splat { position: [0.0; 3], heading: 0.0, texture: "tread".into(), size: [1.0, 1.0], lod: 80.0, duration: 0.0, army: None, created: 0 });
        let out = ground_decals(&p);
        assert_eq!(out.iter().map(|d| d.cutoff).collect::<Vec<_>>(), [200.0, 80.0]);
    }

    /// Without a `lodParam` the cutoff is the diagonal times 15 (albedo) or
    /// 6 (normals), as moho's `ComputeCutoffLOD(-1)`.
    #[test]
    fn script_decals_without_lod_size_their_cutoff() {
        let mut p = Presentation::default();
        let decal = |kind: &str| Decal { position: [0.0; 3], heading: 0.0, textures: ["crater01".into(), String::new()], kind: kind.into(), size: [3.0, 4.0], lod: 0.0, duration: 0.0, army: None, created: 0 };
        p.add_decal(decal("Albedo"));
        p.add_decal(decal("Normals"));
        let out = ground_decals(&p);
        assert_eq!(out.iter().map(|d| d.cutoff).collect::<Vec<_>>(), [75.0, 30.0]);
    }
}
