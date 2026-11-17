//! The player's fog of war: what a snapshot shows the view army's side.
//!
//! With fog of war on (`ScenarioInfo.Options.FogOfWar` isn't `'none'`) and
//! a view army (the UI's focus army; observers have none), each snapshot is
//! cut down to what that army's side knows, from the sim's intel grids
//! (`sim::vision`), read-only at publish time:
//! - its own and allied entities as they are;
//! - enemy units it sees now as they are; radar / sonar contacts it doesn't
//!   see as blips ([`Sighting::Blip`]: an unidentified icon, no mesh);
//! - enemy structures it has seen and doesn't see now as last seen
//!   ([`Sighting::Remembered`]), until it sees the spot again;
//! - other enemy (or army-less) entities that aren't props, effects and
//!   tread marks only where it has eyes;
//! - the terrain's fog texture ([`FogUpdate`]): per intel cell visible,
//!   radar, explored, or (in `'unexplored'` games) never explored, sent as
//!   the rows that changed since the last delivered snapshot;
//! - the side's vision sources ([`Snapshot::vision`]): the renderer draws
//!   the clear ground as their circles, as moho's vision pass does.
//! Nothing here flows back into the sim: the memory of seen structures is
//! the renderer's knowledge, kept by the sink.

use std::collections::BTreeMap;

use super::{DebugRequest, EntityView, FogUpdate, GroundSplat, Sighting, Snapshot, ViewKind, VisionSource};
use crate::engine::sim::army::{Alliance, Armies};
use crate::engine::sim::entity::EntityWorld;
use crate::engine::sim::layer::Layer;
use crate::engine::sim::math::{Fx, FxVec3};
use crate::engine::sim::vision::{FogMode, IntelGrids};

/// The fog view's state across snapshots (presentation only).
#[derive(Default)]
pub(super) struct FogSink {
    /// What the delivered fog texture shows: `Some(None)` = fog off.
    sent: Option<Option<FogKey>>,
    /// The intel update the delivered texture is current to.
    sent_update: u64,
    /// The snapshot being built's texture state, committed once delivered.
    pending: Option<(Option<FogKey>, u64)>,
    /// Enemy structures the view has seen: as last seen, and the snapshot
    /// count that last saw them (id order).
    memory: BTreeMap<u32, (EntityView, u64)>,
    /// Who the memory belongs to.
    memory_of: Option<(u32, FogMode)>,
    /// Snapshots built with a view (memory freshness).
    builds: u64,
    /// Scratch: a row of fog bytes.
    row: Vec<u8>,
}

/// What a fog texture was built for: a change means a full rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FogKey {
    army: u32,
    mode: FogMode,
    alliances: u64,
    cols: u32,
    rows: u32,
}

impl FogSink {
    /// The built snapshot went out: its fog texture state is the renderer's.
    pub(super) fn delivered(&mut self) {
        if let Some((key, update)) = self.pending.take() {
            self.sent = Some(key);
            self.sent_update = update;
        }
    }

    /// It didn't: the next snapshot carries its changes again.
    pub(super) fn dropped(&mut self) {
        self.pending = None;
    }
}

/// The game's `FogOfWar` option.
pub(super) fn game_mode(lua: &mlua::Lua) -> FogMode {
    let option = || -> mlua::Result<Option<String>> { lua.globals().get::<mlua::Table>("ScenarioInfo")?.get::<mlua::Table>("Options")?.get("FogOfWar") };
    FogMode::parse(option().ok().flatten().as_deref())
}

/// Who the snapshot is for and in which mode: the debug fog army (fog
/// forced on), else the view army while the game has fog. `None`: no fog.
pub(super) fn view(lua: &mlua::Lua, request: &DebugRequest) -> Option<(u32, FogMode)> {
    let mode = game_mode(lua);
    match (request.fog_army, request.view_army) {
        (Some(army), _) => Some((army, if mode == FogMode::None { FogMode::Explored } else { mode })),
        (None, Some(army)) if mode != FogMode::None => Some((army, mode)),
        _ => None,
    }
}

fn fx3(p: [f32; 3]) -> FxVec3 {
    FxVec3 { x: Fx::from_num(p[0]), y: Fx::from_num(p[1]), z: Fx::from_num(p[2]) }
}

/// Effects / entities from which the fog filter's lookups run in parallel.
const FOG_PARALLEL_MIN: usize = 8192;

/// Eyes (vision, water vision or omni) on `p` for `army`'s side.
fn eyes_on(grids: &IntelGrids, army: u32, p: FxVec3) -> bool {
    grids.detection(army, p, Layer::Land).eyes() || grids.detection(army, p, Layer::Sub).eyes()
}

/// Cuts `snap` down to what `view` knows and adds its fog texture changes.
pub(super) fn apply(lua: &mlua::Lua, fog: &mut FogSink, view: Option<(u32, FogMode)>, snap: &mut Snapshot) {
    let _s = crate::prof_span!("snapshot fog of war");
    let (Some((army, mode)), Some(world), Some(armies), Some(grids)) = (view, lua.app_data_ref::<EntityWorld>(), lua.app_data_ref::<Armies>(), lua.app_data_ref::<IntelGrids>()) else {
        if fog.sent != Some(None) {
            snap.fog = Some(FogUpdate::Off);
            fog.pending = Some((None, 0));
        }
        fog.memory.clear();
        fog.memory_of = None;
        return;
    };
    let (world, armies, grids): (&EntityWorld, &Armies, &IntelGrids) = (&world, &armies, &grids);
    snap.fog = texture(fog, grids, army, mode);
    snap.vision = vision_sources(world, grids, army);
    if fog.memory_of != Some((army, mode)) {
        fog.memory.clear();
        fog.memory_of = Some((army, mode));
    }
    fog.builds += 1;
    let side = |owner: u32| owner == army || armies.alliance(army, owner) == Alliance::Ally;
    filter_entities(world, grids, army, &side, fog, &mut snap.entities);
    let theirs = |owner: u32, at: [f32; 3]| (owner != 0 && side(owner)) || eyes_on(grids, army, fx3(at));
    // Who sees each effect, worked out in parallel (two detection lookups
    // each, for ~200k emitters in a big air battle), then kept in order.
    let keep: Vec<bool> = {
        use rayon::prelude::*;
        if snap.effects.len() >= FOG_PARALLEL_MIN {
            snap.effects.par_iter().with_min_len(1024).map_init(|| crate::prof_span!("fog effects (worker)"), |_, e| theirs(e.army, e.position)).collect()
        } else {
            snap.effects.iter().map(|e| theirs(e.army, e.position)).collect()
        }
    };
    let mut k = keep.into_iter();
    snap.effects.retain(|_| k.next().unwrap_or(false));
    snap.splats.retain(|s: &GroundSplat| theirs(s.army.unwrap_or(0), s.centre));
    // The jammer fakes it believes in (known fakes aren't drawn), with
    // their jammer's blueprint for the contact's shape.
    snap.fake_blips = grids.fake_blips(army).map(|f| super::FakeBlip { position: super::f3(f.position), blueprint: world.get(f.jammer).and_then(|e| e.category_index).unwrap_or(u32::MAX) }).collect();
}

/// `army`'s side's vision sources at their positions this tick (the
/// circles the renderer draws clear).
fn vision_sources(world: &EntityWorld, grids: &IntelGrids, army: u32) -> Vec<VisionSource> {
    grids
        .vision_sources(army)
        .filter_map(|(id, radius)| {
            let p = world.get(id)?.position();
            Some(VisionSource { id, position: [p.x.to_num(), p.z.to_num()], radius: radius as f32 })
        })
        .collect()
}

/// The fog texture's changes for `army`'s side: every row for a new view,
/// else the runs of rows whose intel changed since the delivered one.
fn texture(fog: &mut FogSink, grids: &IntelGrids, army: u32, mode: FogMode) -> Option<FogUpdate> {
    let (cols, rows, cell) = grids.dimensions();
    let key = FogKey { army, mode, alliances: grids.alliance_version(), cols, rows };
    let full = fog.sent != Some(Some(key));
    let since = fog.sent_update;
    let all_explored = mode != FogMode::Unexplored;
    fog.row.resize(cols as usize, 0);
    let mut bands: Vec<(u32, Vec<u8>)> = Vec::new();
    for z in 0..rows {
        if !full && !grids.row_changed_since(z, since) {
            continue;
        }
        grids.fog_row(army, z, all_explored, &mut fog.row);
        match bands.last_mut() {
            Some((first, bytes)) if *first + (bytes.len() / cols as usize) as u32 == z => bytes.extend_from_slice(&fog.row),
            _ => {
                let mut bytes = Vec::with_capacity(if full { (cols * rows) as usize } else { cols as usize * 4 });
                bytes.extend_from_slice(&fog.row);
                bands.push((z, bytes));
            }
        }
    }
    fog.pending = Some((Some(key), grids.update_count()));
    (full || !bands.is_empty()).then_some(FogUpdate::Grid { cols, rows, cell: cell as f32, full, bands })
}

/// Keeps the entities `army`'s side knows, as it knows them, plus the
/// structures it remembers (id order kept).
fn filter_entities(world: &EntityWorld, grids: &IntelGrids, army: u32, side: &(dyn Fn(u32) -> bool + Sync), fog: &mut FogSink, entities: &mut Vec<EntityView>) {
    let now = fog.builds;
    // What the side detects of each enemy unit, in parallel (read-only);
    // the keep / remember / blip decisions below stay serial, in order.
    let detections: Vec<Option<crate::engine::sim::vision::Detection>> = {
        use rayon::prelude::*;
        let one = |v: &EntityView| {
            let e = world.get(v.id)?;
            let owner = e.army.unwrap_or(0);
            (v.kind != ViewKind::Prop && !(owner != 0 && side(owner)) && e.unit.is_some()).then(|| grids.detect(army, e))
        };
        if entities.len() >= FOG_PARALLEL_MIN {
            entities.par_iter().with_min_len(1024).map_init(|| crate::prof_span!("fog entities (worker)"), |_, v| one(v)).collect()
        } else {
            entities.iter().map(one).collect()
        }
    };
    let mut detected = detections.into_iter();
    // Remembered structures on the side's radar now (id order).
    let mut on_radar: Vec<u32> = Vec::new();
    entities.retain_mut(|v| {
        let d = detected.next().flatten();
        // Props are the map's.
        if v.kind == ViewKind::Prop {
            return true;
        }
        let Some(e) = world.get(v.id) else { return false };
        let owner = e.army.unwrap_or(0);
        if owner != 0 && side(owner) {
            return true;
        }
        if e.unit.is_none() {
            // Projectiles, shields, script entities: only where it has eyes.
            return eyes_on(grids, army, e.position());
        }
        let Some(d) = d else { return false };
        if d.eyes() {
            if v.kind == ViewKind::Structure {
                let remembered = EntityView { pose_start: 0, pose_len: 0, sighting: Sighting::Remembered, ..*v };
                fog.memory.insert(v.id, (remembered, now));
            }
            return true;
        }
        if fog.memory.contains_key(&v.id) {
            // Shown as remembered, below (identified while on radar).
            if d.any() {
                on_radar.push(v.id);
            }
            return false;
        }
        // A dying unit leaves no contact behind (it isn't a unit any more).
        // A contact seen before keeps its identity (probe_intel_icons: its
        // full icon in army colour); one never seen is a plain blip.
        if d.any() && v.kind != ViewKind::Dying {
            // (Under omni it's eyes: shown as seen above.)
            let sighting = if grids.ever_seen(army, v.id) { Sighting::Identified } else { Sighting::Blip };
            *v = EntityView { pose_start: 0, pose_len: 0, sighting, ..*v };
            return true;
        }
        false
    });
    // Remembered structures not seen this time: until the view has eyes on
    // the spot again (then they're gone, or cloaked), or the sim forgot the
    // sighting (`FlushIntelInRect`).
    fog.memory.retain(|&id, (v, seen)| {
        *seen == now || {
            let alive = world.get(id).filter(|e| !e.destroyed);
            !eyes_on(grids, army, fx3(v.position)) && alive.is_none_or(|_| grids.ever_seen(army, id))
        }
    });
    if fog.memory.values().all(|(_, seen)| *seen == now) {
        return;
    }
    let kept = std::mem::take(entities);
    entities.reserve(kept.len() + fog.memory.len());
    let identified = |v: EntityView| if on_radar.binary_search(&v.id).is_ok() { EntityView { sighting: Sighting::Identified, ..v } } else { v };
    let mut remembered = fog.memory.values().filter(|(_, seen)| *seen != now).map(|(v, _)| identified(*v)).peekable();
    for v in kept {
        while let Some(r) = remembered.next_if(|r| r.id < v.id) {
            entities.push(r);
        }
        entities.push(v);
    }
    entities.extend(remembered);
}

/// Moho's line-of-sight sound culling (`CUserSoundManager::FilterSound`,
/// `snd_CheckLOS`, see docs/research/verifying-against-the-binary.md): while fog of war is
/// on, a world sound (or entity loop) the view army's side can't see is
/// dropped. Vision decides, water vision for sounds of units on the Sub /
/// Seabed layers; observers (no view army) hear everything. Reads the sim's
/// grids at publish time only: presentation, never sim state.
pub(super) struct SoundSight<'a> {
    listener: Option<(u32, mlua::AppDataRef<'a, IntelGrids>)>,
    world: Option<mlua::AppDataRef<'a, EntityWorld>>,
}

impl<'a> SoundSight<'a> {
    pub(super) fn new(lua: &'a mlua::Lua, view: Option<(u32, FogMode)>) -> Self {
        let listener = view.and_then(|(a, _)| Some((a, lua.app_data_ref::<IntelGrids>()?)));
        let world = listener.as_ref().and_then(|_| lua.app_data_ref::<EntityWorld>());
        Self { listener, world }
    }

    pub(super) fn hears(&self, entity: Option<u32>, at: [f32; 3]) -> bool {
        let Some((army, grids)) = &self.listener else { return true };
        let layer = entity.and_then(|id| Some(self.world.as_ref()?.get(id)?.unit.as_deref()?.layer));
        let under_water = matches!(layer, Some(Layer::Sub | Layer::Seabed));
        grids.sees_point(*army, fx3(at), under_water)
    }
}
