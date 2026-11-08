//! Per-army intel coverage (fog of war) and what each army detects.
//!
//! Each army has one coverage grid per sensing intel type (vision, water
//! vision, radar, sonar, omni): a count per cell of how many of its sources
//! cover it, the cells 2 world units across for vision and 4 for the rest,
//! as in moho. Sources are entities with that intel enabled and a radius
//! (units once finished; script entities such as viz markers), stamped as
//! discs with moho's raster ([`stamp_disc`]). Stamps are incremental: a
//! source is re-stamped only when it has moved a third of its radius, or
//! its radius or on/off state changes, so a still base costs nothing per
//! tick. All integer arithmetic; armies and sources are processed in index
//! / id order, so coverage is identical everywhere.
//!
//! Allies share intel: army `a` covers a cell if any army allied to `a`
//! (including itself) does. Detection of a unit by an army
//! ([`IntelGrids::detect`]):
//! - seen: vision (above water) or water vision (in water layers), unless
//!   the unit is cloaked; omni always sees;
//! - on radar: radar, if the unit is above water and not radar-stealthed;
//! - on sonar: sonar, if it's in water and not sonar-stealthed.
//! A unit is cloaked / stealthed by its own `Cloak` / `RadarStealth` /
//! `SonarStealth` intel, or by standing in an allied `CloakField` /
//! `RadarStealthFieldRadius` / `SonarStealthFieldRadius` (the field grids
//! are stamped like the sensors, for the field's owner). Omni counters all of
//! them (FAF wiki: "Countered by omni sensors").
//!
//! Jammers (`Jammer` intel with blueprint `JammerBlips` / `JamRadius`) put
//! that many fake radar contacts around themselves, re-placed every
//! [`JAM_PERIOD`] ticks from a hash of (unit, period, blip), so they're
//! the same on every machine; an enemy sees a fake on its radar unless omni
//! covers it.
//!
//! Inferred, not verified: that unfinished units have no intel.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::intel::IntelType;
use crate::engine::sim::layer::Layer;
use crate::engine::sim::math::{Fx, FxVec3};

/// Cell edge = 2^shift world units: 2 for the vision grid, 4 for the others
/// (`CAiReconDBImpl` constructor, MohoEngine.dll 0x101ad89c builds the fog
/// grid `CIntelGrid(map, 2)`, every other grid with 4; a grid is
/// `map size / scale` cells across, 0x100f4280).
const VISION_SHIFT: u32 = 1;
const CELL_SHIFT: u32 = 2;

/// A moved source's disc is redrawn once it's this fraction of its radius
/// from where it was drawn (moho's 0.333), squared.
const RESTAMP_FRACTION2: Fx = Fx::lit("0.110889");

/// The vision sensor's grid index (`sensor_index(IntelType::Vision)`).
const VISION: usize = 0;

/// A sensor's cell shift.
const fn shift_of(sensor: usize) -> u32 {
    if sensor == VISION { VISION_SHIFT } else { CELL_SHIFT }
}

/// The intel types stamped as discs: sensors (seen by the owner's side) and
/// counter-intel fields (protecting the owner's side).
pub const SENSORS: [IntelType; 8] = [
    IntelType::Vision,
    IntelType::WaterVision,
    IntelType::Radar,
    IntelType::Sonar,
    IntelType::Omni,
    IntelType::RadarStealthField,
    IntelType::SonarStealthField,
    IntelType::CloakField,
];

/// The sensors whose coverage explores the ground (fog of war's
/// 'unexplored' mode): a cell once covered stays explored. Vision only:
/// moho gives only vision handles the explored grid (MohoEngine.dll
/// 0x10338cf3; `CIntelXGrid::AddCircle` 0x100f48a0 as a vision circle is
/// stamped at 0x1033a580); water vision, radar and omni don't explore.
const EXPLORING: [IntelType; 1] = [IntelType::Vision];

/// Fog levels of [`IntelGrids::fog_row`], one byte per cell.
pub mod fog_level {
    /// Never explored ('unexplored' mode only): black.
    pub const UNEXPLORED: u8 = 0;
    /// Explored, nothing covering it now: darkened.
    pub const EXPLORED: u8 = 1;
    /// Radar or sonar covers it (no eyes): dimmed.
    pub const RADAR: u8 = 2;
    /// Vision (or water vision / omni) covers it: clear.
    pub const VISIBLE: u8 = 3;
}

/// The game's fog of war option (`ScenarioInfo.Options.FogOfWar`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FogMode {
    /// 'none': everything visible.
    None,
    /// 'explored': the map starts explored; units need intel.
    #[default]
    Explored,
    /// 'unexplored': nothing shows until explored, then as 'explored'.
    Unexplored,
}

impl FogMode {
    pub fn parse(option: Option<&str>) -> Self {
        match option {
            Some("none") => FogMode::None,
            Some("unexplored") => FogMode::Unexplored,
            _ => FogMode::Explored,
        }
    }
}

/// Ticks between jammer blip re-placements (inferred).
pub const JAM_PERIOD: u64 = 50;

/// A unit's own counter-intel (`Cloak`, `RadarStealth`, `SonarStealth`).
const CLOAK: u8 = 1;
const RADAR_STEALTH: u8 = 2;
const SONAR_STEALTH: u8 = 4;

/// A jammer's blueprint numbers (`Intel.JammerBlips`, `Intel.JamRadius`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JammerSpec {
    pub blips: u32,
    pub min: Fx,
    pub max: Fx,
}

fn sensor_index(t: IntelType) -> Option<usize> {
    SENSORS.iter().position(|&s| s == t)
}

/// One army's coverage counts, per sensor, row-major cells (a sensor's
/// grid is allocated with its first stamp: most armies only have vision and
/// radar).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ArmyGrids {
    counts: [Vec<u16>; 8],
}

/// A source's current disc: the cell it's in (the sensor's cells) and its
/// radius, for one army and sensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Stamp {
    army: u32,
    sensor: usize,
    cx: i32,
    cz: i32,
    /// Radius in whole world units (moho keeps intel radii as integers).
    radius: u32,
}

/// What an army knows of a unit right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Detection {
    pub seen: bool,
    pub radar: bool,
    pub sonar: bool,
    pub omni: bool,
}

/// Recon types an army's intel on a unit changes by (`OnIntelChange`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Recon {
    LosNow,
    Radar,
    Sonar,
    Omni,
}

impl Recon {
    pub const ALL: [Recon; 4] = [Recon::LosNow, Recon::Radar, Recon::Sonar, Recon::Omni];

    /// FAF's `ReconTypes` name.
    pub fn name(self) -> &'static str {
        match self {
            Recon::LosNow => "LOSNow",
            Recon::Radar => "Radar",
            Recon::Sonar => "Sonar",
            Recon::Omni => "Omni",
        }
    }
}

/// An enemy army gained or lost one kind of intel on a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IntelChange {
    pub unit: EntityId,
    pub army: u32,
    pub recon: Recon,
    pub on: bool,
}

impl Detection {
    /// The army has any intel on the unit (a blip exists).
    pub fn any(self) -> bool {
        self.seen || self.radar || self.sonar || self.omni
    }
}

/// Army masks are one `u64`, bit = army index (1-based): sessions are
/// capped at [`MAX_ARMIES`] (checked at army creation). Wider masks measured
/// 40% slower at 16 armies (per-army detection is the hot path).
use crate::engine::sim::army::MAX_ARMIES;


/// Ops (disc adds / removes) from which [`IntelGrids::apply_all`] runs the
/// armies in parallel.
const APPLY_PARALLEL_MIN: usize = 256;

/// `v` as atomics, for disjoint-bit writes from several threads while the
/// exclusive borrow is held (`AtomicU64` has `u64`'s size and, on the 64-bit
/// targets this engine supports, its alignment).
fn atomic_view(v: &mut [u64]) -> &[AtomicU64] {
    const { assert!(std::mem::align_of::<AtomicU64>() == std::mem::align_of::<u64>()) };
    // SAFETY: same layout; the &mut borrow makes these the only accesses.
    unsafe { &*(v as *mut [u64] as *const [AtomicU64]) }
}

/// Adds or removes one disc on an army's `counts` for a sensor, keeping its
/// bit in `cover` (count non-zero) and `explored`, and noting changed rows
/// (`row_changed` and `explored` are at vision resolution).
///
/// Moho's raster (`CIntelGrid::Raster`, MohoEngine.dll 0x100f3e30), with
/// `r = radius / cell size` (rounded down) and (cx, cz) the source's cell:
/// columns `cx - r <= x < cx + r`, each with rows `cz - h <= z < cz + h`,
/// `h = trunc(sqrt(r^2 - (cx - x)^2))` (column `cx - r` is empty). So in x
/// the disc is centred on the source's cell, in z on that cell's low-z
/// edge: it reaches a cell further toward -z than +z. Row by row: row
/// `z = cz + dz` for `-r <= dz < r` covers `|x - cx| <= isqrt(r^2 - e^2)`,
/// `e = -dz` above the source's row and `dz + 1` from it down.
#[allow(clippy::too_many_arguments)]
fn stamp_disc(s: &Stamp, add: bool, cols: i32, rows: i32, counts: &mut [u16], cover: &[AtomicU64], explored: &[AtomicU64], row_changed: &[AtomicU64], update: u64) {
    let bit = 1u64 << s.army.min(MAX_ARMIES);
    let shift = shift_of(s.sensor);
    let r = (s.radius >> shift) as i32;
    let explores = add && EXPLORING.contains(&SENSORS[s.sensor]);
    // A row of this grid's cells is this many vision rows.
    let fine = shift - VISION_SHIFT;
    for z in (s.cz - r).max(0)..(s.cz + r).min(rows) {
        let dz = z - s.cz;
        let e = if dz < 0 { -dz } else { dz + 1 };
        let k = (r * r - e * e).unsigned_abs().isqrt() as i32;
        let (x0, x1) = ((s.cx - k).max(0), (s.cx + k).min(cols - 1));
        if x0 > x1 {
            continue;
        }
        let span = (z * cols) as usize + x0 as usize..=(z * cols) as usize + x1 as usize;
        // The army's bit follows whether its count is non-zero: it flips
        // only where the count crosses zero.
        for (c, m) in counts[span.clone()].iter_mut().zip(&cover[span.clone()]) {
            if add {
                *c = c.saturating_add(1);
                if *c == 1 {
                    m.fetch_or(bit, Ordering::Relaxed);
                }
            } else if *c > 0 {
                *c -= 1;
                if *c == 0 {
                    m.fetch_and(!bit, Ordering::Relaxed);
                }
            }
        }
        // Seen once, explored for good.
        if explores {
            for m in &explored[span] {
                if m.load(Ordering::Relaxed) & bit == 0 {
                    m.fetch_or(bit, Ordering::Relaxed);
                }
            }
        }
        for fz in (z << fine)..((z + 1) << fine) {
            if let Some(c) = row_changed.get(fz as usize) {
                c.store(update, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct IntelGrids {
    /// The map's size in world units.
    width: u32,
    height: u32,
    /// By army index (1-based armies; index 0 unused).
    armies: Vec<Option<ArmyGrids>>,
    /// Current stamps, sorted by (entity, sensor), with where each was
    /// stamped.
    stamps: Vec<((EntityId, usize), Stamp, FxVec3)>,
    /// Reused per-tick buffer of wanted stamps (same order).
    wanted: Vec<((EntityId, usize), Stamp, FxVec3)>,
    /// This update's disc adds / removes (reused buffer).
    ops: Vec<(Stamp, bool)>,
    /// Allied-or-self masks by army index (bit `b` = army `b` shares intel
    /// with it), refreshed by [`IntelGrids::set_alliances`].
    allies: Vec<u64>,
    /// (alliance version, army count) `allies` was last built from.
    alliance_source: Option<(u32, u32)>,
    /// Per sensor, per cell: the armies whose own coverage count there is
    /// non-zero (kept by `apply`), so "is this covered for army a's side" is
    /// one AND with a's allies mask, and who sees a unit is a few lookups
    /// rather than a pass per army. Allocated with the sensor's first stamp.
    cover: [Vec<u64>; 8],
    /// Per army: the armies whose intel it gets (`allies` transposed).
    shared_by: Vec<u64>,
    /// Units some enemy has seen (seen, not just radar): the armies that
    /// have, as a mask. Lookups only.
    seen_ever: BTreeMap<EntityId, u64>,
    /// Units with their own cloak / stealth on, sorted (refreshed per
    /// update, so detection needn't read cold entity data).
    own_flags: Vec<(EntityId, u8)>,
    /// Active jammers: (unit, army, position, spec), id order.
    jammers: Vec<(EntityId, u32, FxVec3, JammerSpec)>,
    tick: u64,
    /// Stamps per (army, sensor), and per sensor the armies with any: lets
    /// coverage checks skip sensors nobody on a side has (most of them).
    stamp_count: Vec<[u32; 8]>,
    active: [u64; 8],
    /// Per unit (id order), the enemy armies detecting it now, by
    /// [`Recon`]: diffed each update into `changes`.
    now: Vec<(EntityId, [u64; 4])>,
    now_next: Vec<(EntityId, [u64; 4])>,
    /// Per vision cell: the armies whose own exploring sensors
    /// ([`EXPLORING`], vision only) have ever covered it (set as stamps go down, never cleared).
    /// Presentation bookkeeping: nothing in the sim reads it.
    explored: Vec<u64>,
    /// Per row of vision cells: the update (`tick`) its coverage or explored
    /// masks last changed (a 4-unit row marks both its vision rows), so a fog view rebuilds only rows that changed.
    row_changed: Vec<u64>,
    /// Changes whenever the alliance masks do (a fog view then rebuilds).
    alliance_version: u64,
    /// This update's intel changes, (unit, army, recon) order.
    pub changes: Vec<IntelChange>,
    /// Enemy armies that saw a unit for the first time this update
    /// (`OnDetectedBy`), (unit, army) order.
    pub first_seen: Vec<(EntityId, u32)>,
}

impl IntelGrids {
    /// Grids for a `width` x `height` map (world units).
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, ..Default::default() }
    }

    /// A grid's cells across and down for cell shift `shift`.
    fn dims(&self, shift: u32) -> (u32, u32) {
        ((self.width >> shift).max(1), (self.height >> shift).max(1))
    }

    /// The index of `p`'s cell in a grid of cell shift `shift`, if on it.
    fn cell_index(&self, shift: u32, p: FxVec3) -> Option<usize> {
        let (cols, rows) = self.dims(shift);
        let (cx, cz) = (Self::cell_of(p.x, shift), Self::cell_of(p.z, shift));
        (cx >= 0 && cz >= 0 && cx < cols as i32 && cz < rows as i32).then(|| (cz as u32 * cols + cx as u32) as usize)
    }

    /// Army `army`'s count grid for `sensor`, the sensor's cover masks,
    /// the explored masks and the rows' change marks.
    fn grids_for(&mut self, army: u32, sensor: usize) -> (&mut Vec<u16>, &mut Vec<u64>, &mut Vec<u64>, &mut Vec<u64>) {
        let (cols, rows) = self.dims(shift_of(sensor));
        let n = (cols * rows) as usize;
        let (fine_cols, fine_rows) = self.dims(VISION_SHIFT);
        let i = army as usize;
        if self.armies.len() <= i {
            self.armies.resize_with(i + 1, || None);
        }
        let counts = &mut self.armies[i].get_or_insert_with(ArmyGrids::default).counts[sensor];
        if counts.is_empty() {
            *counts = vec![0; n];
        }
        let cover = &mut self.cover[sensor];
        if cover.is_empty() {
            *cover = vec![0; n];
        }
        if self.explored.is_empty() {
            self.explored = vec![0; (fine_cols * fine_rows) as usize];
        }
        if self.row_changed.is_empty() {
            self.row_changed = vec![0; fine_rows as usize];
        }
        (counts, cover, &mut self.explored, &mut self.row_changed)
    }

    /// `floor(v / cell size)` (moho's cell of a point, 0x100f2ad0).
    fn cell_of(v: Fx, shift: u32) -> i32 {
        (v.floor().to_num::<i64>() >> shift) as i32
    }

    /// Adds or removes this update's discs (`ops`, in merge order).
    ///
    /// The per-army counts are the army's own; the shared per-cell masks
    /// (coverage, explored) take only that army's bit. So each army's ops
    /// run in order on one thread and armies run in parallel, the masks
    /// written with atomic bit sets / clears, which commute: the result is
    /// the serial one, bit for bit. (63 armies of aircraft crossing cells
    /// every few ticks: a serial pass was most of intel.)
    fn apply_all(&mut self, ops: &[(Stamp, bool)]) {
        use rayon::prelude::*;
        if ops.is_empty() {
            return;
        }
        // Counts of stamps per (army, sensor) and the armies with any, and
        // every grid an op needs allocated, serially first.
        let prepass = crate::prof_span!("intel apply prepass");
        for &(st, add) in ops {
            let a = st.army as usize;
            if self.stamp_count.len() <= a {
                self.stamp_count.resize(a + 1, [0; 8]);
            }
            let n = &mut self.stamp_count[a][st.sensor];
            *n = if add { *n + 1 } else { n.saturating_sub(1) };
            let bit = 1u64 << st.army.min(MAX_ARMIES);
            if *n > 0 {
                self.active[st.sensor] |= bit;
            } else {
                self.active[st.sensor] &= !bit;
            }
            let _ = self.grids_for(st.army, st.sensor);
        }
        drop(prepass);
        let dims = [VISION_SHIFT, CELL_SHIFT].map(|shift| self.dims(shift));
        let update = self.tick;
        let Self { armies, cover, explored, row_changed, .. } = self;
        let cover: Vec<&[AtomicU64]> = cover.iter_mut().map(|c| atomic_view(c)).collect();
        let explored = atomic_view(explored);
        let row_changed = atomic_view(row_changed);
        let run = |army: u32, grids: &mut ArmyGrids| {
            for &(st, add) in ops.iter().filter(|(st, _)| st.army == army) {
                let (cols, rows) = dims[(shift_of(st.sensor) - VISION_SHIFT) as usize];
                stamp_disc(&st, add, cols as i32, rows as i32, &mut grids.counts[st.sensor], cover[st.sensor], explored, row_changed, update);
            }
        };
        if ops.len() >= APPLY_PARALLEL_MIN {
            // (Each task filters the op list for its army: cheap next to
            // the discs, and keeps every army's ops in merge order.)
            armies.par_iter_mut().enumerate().for_each(|(a, g)| {
                if let Some(g) = g {
                    let _s = crate::prof_span!("intel discs (worker)");
                    run(a as u32, g);
                }
            });
        } else {
            for (a, g) in armies.iter_mut().enumerate() {
                if let Some(g) = g {
                    run(a as u32, g);
                }
            }
        }
    }

    /// Which armies share intel with which (call when alliances change, and
    /// before the first update). `allied(a, b)` = b's intel counts for a.
    /// [`IntelGrids::set_alliances`] unless `source` (the armies' alliance
    /// version and count) is what it last took them from: the n² rebuild
    /// was a sixth of intel in a 63-army game, every tick.
    pub fn set_alliances_from(&mut self, source: (u32, u32), allied: impl Fn(u32, u32) -> bool) {
        if self.alliance_source == Some(source) {
            return;
        }
        self.alliance_source = Some(source);
        self.set_alliances(source.1, allied);
    }

    pub fn set_alliances(&mut self, armies: u32, allied: impl Fn(u32, u32) -> bool) {
        let allies: Vec<u64> = (0..=armies).map(|a| (1..=armies.min(MAX_ARMIES)).filter(|&b| a == b || allied(a, b)).fold(0u64, |m, b| m | 1 << b)).collect();
        if allies == self.allies {
            return;
        }
        self.alliance_version += 1;
        self.allies = allies;
        self.shared_by = (0..=armies as usize).map(|b| self.allies.iter().enumerate().filter(|(_, m)| *m & (1 << b.min(MAX_ARMIES as usize)) != 0).fold(0u64, |acc, (a, _)| acc | 1 << a)).collect();
    }

    /// Per tick: re-stamps sources that moved far enough or changed, refreshes
    /// cloak / stealth flags and jammers, then records which enemy units
    /// each army sees. `jammer` gives a unit's jammer spec, if it has one.
    pub fn update(&mut self, world: &EntityWorld, jammer: impl Fn(&crate::engine::sim::entity::Entity) -> Option<JammerSpec>) {
        self.tick += 1;
        let gather = crate::prof_span!("intel gather");
        let mut wanted = std::mem::take(&mut self.wanted);
        wanted.clear();
        self.own_flags.clear();
        self.jammers.clear();
        for e in world.iter() {
            if e.destroyed || e.fraction_complete < Fx::ONE {
                continue;
            }
            let Some(army) = e.army else { continue };
            let mut flags = 0u8;
            for (&t, s) in &e.cold.intel {
                match t {
                    IntelType::Cloak if s.enabled => flags |= CLOAK,
                    IntelType::RadarStealth if s.enabled => flags |= RADAR_STEALTH,
                    IntelType::SonarStealth if s.enabled => flags |= SONAR_STEALTH,
                    IntelType::Jammer if s.enabled => {
                        if let Some(spec) = jammer(e).filter(|j| j.blips > 0) {
                            self.jammers.push((e.id, army, e.position(), spec));
                        }
                    }
                    _ => {}
                }
                let Some(sensor) = sensor_index(t) else { continue };
                if !s.enabled || s.radius <= Fx::ZERO {
                    continue;
                }
                let p = e.position();
                let shift = shift_of(sensor);
                let radius = s.radius.to_num::<i64>().clamp(0, u32::MAX as i64) as u32;
                let stamp = Stamp { army, sensor, cx: Self::cell_of(p.x, shift), cz: Self::cell_of(p.z, shift), radius };
                wanted.push(((e.id, sensor), stamp, p));
            }
            if flags != 0 {
                self.own_flags.push((e.id, flags));
            }
        }
        drop(gather);
        // `wanted` is in (entity, sensor) order (world id order, intel types
        // in sensor order), like `stamps`: merge the two sorted lists.
        let merge = crate::prof_span!("intel merge");
        let old = std::mem::take(&mut self.stamps);
        let mut ops = std::mem::take(&mut self.ops);
        ops.clear();
        let mut i = 0;
        for w in wanted.iter_mut() {
            let key = w.0;
            while i < old.len() && old[i].0 < key {
                ops.push((old[i].1, false));
                i += 1;
            }
            if i < old.len() && old[i].0 == key {
                // A moved source keeps its disc where it was until it has
                // gone a third of its radius from there (3D), unless its
                // side can't see where it is now (`CIntelPosHandle` update,
                // MohoEngine.dll 0x1033a290: 0.333 at 0x1076c760, the LOS
                // check `ReconCanDetect(pos, RECON_LOSNow)`).
                let (was, at) = (old[i].1, old[i].2);
                let same = was.army == w.1.army && was.radius == w.1.radius;
                let d = w.2 - at;
                let moved2 = d.x.saturating_mul(d.x).saturating_add(d.y.saturating_mul(d.y)).saturating_add(d.z.saturating_mul(d.z));
                let r = Fx::from_num(w.1.radius);
                if same && moved2 < r.saturating_mul(r).saturating_mul(RESTAMP_FRACTION2) && self.covered(w.1.army, VISION, w.2) {
                    w.1 = was;
                    w.2 = at;
                } else if was != w.1 {
                    ops.push((was, false));
                    ops.push((w.1, true));
                }
                i += 1;
            } else {
                ops.push((w.1, true));
            }
        }
        for &(_, s, _) in &old[i..] {
            ops.push((s, false));
        }
        drop(merge);
        self.apply_all(&ops);
        self.ops = ops;
        // Wanted becomes current; the old list's storage is reused next tick.
        self.stamps = wanted;
        self.wanted = old;
        self.record_intel(world);
    }

    /// Covered by `sensor` for `army` (own or allied) at `p`.
    fn covered(&self, army: u32, sensor: usize, p: FxVec3) -> bool {
        let Some(cell) = self.cell_index(shift_of(sensor), p) else { return false };
        let side = self.allies.get(army as usize).copied().unwrap_or(1 << army.min(MAX_ARMIES));
        self.cover_at(sensor, cell) & side != 0
    }

    /// The armies whose own `sensor` covers the cell at index `cell` of its
    /// grid.
    fn cover_at(&self, sensor: usize, cell: usize) -> u64 {
        self.cover[sensor].get(cell).copied().unwrap_or(0)
    }

    /// Does `army`'s side have eyes on `p`: vision, or water vision under
    /// the surface (`under_water`, the Sub and Seabed layers)? Moho's world
    /// sound filter (`CUserSoundManager::FilterSound`) asks only this: no
    /// omni, no radar.
    pub fn sees_point(&self, army: u32, p: FxVec3, under_water: bool) -> bool {
        let sensor = if under_water { IntelType::WaterVision } else { IntelType::Vision };
        self.covered(army, sensor_index(sensor).expect("vision"), p)
    }

    /// What `army` would know of an unhidden unit at `p` on `layer` (raw
    /// sensor coverage; see [`IntelGrids::detect`] for a real unit).
    pub fn detection(&self, army: u32, p: FxVec3, layer: Layer) -> Detection {
        self.detection_with(army, p, layer, 0)
    }

    fn detection_with(&self, army: u32, p: FxVec3, layer: Layer, hidden: u8) -> Detection {
        let wet = matches!(layer, Layer::Water | Layer::Sub | Layer::Seabed);
        let under = matches!(layer, Layer::Sub | Layer::Seabed);
        let s = |t: IntelType| self.covered(army, sensor_index(t).expect("sensor"), p);
        let omni = s(IntelType::Omni);
        let optical = if under { s(IntelType::WaterVision) } else { s(IntelType::Vision) || (wet && s(IntelType::WaterVision)) };
        Detection {
            seen: omni || (optical && hidden & CLOAK == 0),
            radar: !under && hidden & RADAR_STEALTH == 0 && s(IntelType::Radar),
            sonar: wet && hidden & SONAR_STEALTH == 0 && s(IntelType::Sonar),
            omni,
        }
    }

    /// The counter-intel protecting `e`: its own, plus allied fields over it.
    fn hidden(&self, e: &crate::engine::sim::entity::Entity) -> u8 {
        let own = self.own_flags.binary_search_by_key(&e.id, |f| f.0).map(|i| self.own_flags[i].1).unwrap_or(0);
        let Some(owner) = e.army else { return own };
        let p = e.position();
        let field = |t: IntelType, bit: u8| if self.covered(owner, sensor_index(t).expect("field"), p) { bit } else { 0 };
        own | field(IntelType::CloakField, CLOAK) | field(IntelType::RadarStealthField, RADAR_STEALTH) | field(IntelType::SonarStealthField, SONAR_STEALTH)
    }

    /// What `army` knows of unit `e` now (its layer, cloak, stealth).
    pub fn detect(&self, army: u32, e: &crate::engine::sim::entity::Entity) -> Detection {
        let layer = e.unit.as_deref().map(|u| u.layer).unwrap_or_default();
        self.detection_with(army, e.position(), layer, self.hidden(e))
    }

    /// Fake radar contacts `army` sees from enemy jammers this period:
    /// positions in jammer id order, then blip order.
    pub fn fake_blips(&self, army: u32) -> Vec<FxVec3> {
        let period = self.tick / JAM_PERIOD;
        let mut out = Vec::new();
        for &(id, owner, at, spec) in &self.jammers {
            if self.allies.get(army as usize).is_some_and(|m| m & (1 << owner.min(MAX_ARMIES)) != 0) {
                continue;
            }
            for k in 0..spec.blips {
                let h = mix(mix(mix(id as u64) ^ period) ^ k as u64);
                // Angle from the low bits (a turn in 2^16 steps), distance
                // from the next ones, both exact fixed point.
                let angle = Fx::from_num((h & 0xFFFF) as u32) / Fx::from_num(0x10000) * (Fx::PI * 2);
                let t = Fx::from_num(((h >> 16) & 0xFFFF) as u32) / Fx::from_num(0x10000);
                let d = spec.min + (spec.max - spec.min).saturating_mul(t);
                let (s, c) = crate::engine::sim::math::sin_cos(angle);
                let p = FxVec3 { x: at.x + s.saturating_mul(d), y: at.y, z: at.z + c.saturating_mul(d) };
                let radar = sensor_index(IntelType::Radar).expect("radar");
                let omni = sensor_index(IntelType::Omni).expect("omni");
                if self.covered(army, radar, p) && !self.covered(army, omni, p) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// The fog view's cells across and down (the vision grid's, the
    /// finest), and a cell's edge in world units.
    pub fn dimensions(&self) -> (u32, u32, u32) {
        let (cols, rows) = self.dims(VISION_SHIFT);
        (cols, rows, 1 << VISION_SHIFT)
    }

    /// The vision sources of `army`'s side (its own and its allies'): each
    /// entity with vision on, and its radius in whole world units, in
    /// entity order. As of the last update; read-only (the fog of war's
    /// drawn circles, `snapshot::fog`).
    pub fn vision_sources(&self, army: u32) -> impl Iterator<Item = (EntityId, u32)> + '_ {
        let side = self.allies.get(army as usize).copied().unwrap_or(1 << army.min(MAX_ARMIES));
        self.stamps.iter().filter(move |((_, sensor), s, _)| *sensor == VISION && side & (1 << s.army.min(MAX_ARMIES)) != 0).map(|((id, _), s, _)| (*id, s.radius))
    }

    /// The intel update count, which [`IntelGrids::row_changed_since`]
    /// compares against.
    pub fn update_count(&self) -> u64 {
        self.tick
    }

    /// Changes whenever who shares intel with whom does.
    pub fn alliance_version(&self) -> u64 {
        self.alliance_version
    }

    /// Whether row `z`'s coverage or explored state changed after update
    /// `since` (a row never stamped never changed).
    pub fn row_changed_since(&self, z: u32, since: u64) -> bool {
        self.row_changed.get(z as usize).is_some_and(|&t| t > since)
    }

    /// Whether `army`'s side has explored the cell at `p`.
    pub fn explored(&self, army: u32, p: FxVec3) -> bool {
        let Some(cell) = self.cell_index(VISION_SHIFT, p) else { return false };
        let side = self.allies.get(army as usize).copied().unwrap_or(1 << army.min(MAX_ARMIES));
        self.explored.get(cell).is_some_and(|m| m & side != 0)
    }

    /// Row `z` of `army`'s side's fog view ([`fog_level`] per vision cell)
    /// into `out` (`cols` long). With `all_explored` ('explored' mode)
    /// nothing is darker than [`fog_level::EXPLORED`]. Integer masks only.
    pub fn fog_row(&self, army: u32, z: u32, all_explored: bool, out: &mut [u8]) {
        use fog_level::*;
        let side = self.allies.get(army as usize).copied().unwrap_or(1 << army.min(MAX_ARMIES));
        let (cols, _) = self.dims(VISION_SHIFT);
        let (coarse_cols, _) = self.dims(CELL_SHIFT);
        let (cols, coarse_cols) = (cols as usize, coarse_cols as usize);
        let out = &mut out[..cols];
        out.fill(if all_explored { EXPLORED } else { UNEXPLORED });
        // Brightest wins: one straight pass per source (each vectorises),
        // skipping grids nobody has; a 4-unit cell is two vision cells of
        // its row.
        let raise = |out: &mut [u8], masks: Option<&[u64]>, level: u8, shift: u32| {
            let Some(m) = masks else { return };
            let lit = |o: &mut u8, m: u64| *o = if m & side != 0 { (*o).max(level) } else { *o };
            if shift == VISION_SHIFT {
                out.iter_mut().zip(m).for_each(|(o, &m)| lit(o, m));
            } else {
                out.chunks_mut(2).zip(m).for_each(|(o, &m)| o.iter_mut().for_each(|o| lit(o, m)));
            }
        };
        let row = |shift: u32| {
            let n = if shift == VISION_SHIFT { cols } else { coarse_cols };
            let start = (z >> (shift - VISION_SHIFT)) as usize * n;
            start..start + n
        };
        raise(out, self.explored.get(row(VISION_SHIFT)), EXPLORED, VISION_SHIFT);
        for (sensor, level) in [(IntelType::Radar, RADAR), (IntelType::Sonar, RADAR), (IntelType::Vision, VISIBLE), (IntelType::WaterVision, VISIBLE), (IntelType::Omni, VISIBLE)] {
            let i = sensor_index(sensor).expect("sensor");
            if self.active[i] & side != 0 {
                raise(out, self.cover[i].get(row(shift_of(i))), level, shift_of(i));
            }
        }
    }

    /// Whether `army` has ever seen `unit` (with vision or omni).
    pub fn ever_seen(&self, army: u32, unit: EntityId) -> bool {
        self.seen_ever.get(&unit).is_some_and(|m| m & (1 << army.min(MAX_ARMIES)) != 0)
    }

    /// The armies detecting `e` now by each [`Recon`] (as `detect` would
    /// answer, sharing included), as masks: from its cell's cover masks, so
    /// one pass over the units serves every army.
    fn masks(&self, e: &crate::engine::sim::entity::Entity) -> [u64; 4] {
        let p = e.position();
        // Its cell in the vision grid and in the 4-unit grids (off the map
        // in either: nothing sees it).
        let (Some(fine), Some(coarse)) = (self.cell_index(VISION_SHIFT, p), self.cell_index(CELL_SHIFT, p)) else {
            return [0; 4];
        };
        let layer = e.unit.as_deref().map(|u| u.layer).unwrap_or_default();
        let wet = matches!(layer, Layer::Water | Layer::Sub | Layer::Seabed);
        let under = matches!(layer, Layer::Sub | Layer::Seabed);
        let at = |t: IntelType| {
            let i = sensor_index(t).expect("sensor");
            self.cover_at(i, if shift_of(i) == VISION_SHIFT { fine } else { coarse })
        };
        let optical = if under { at(IntelType::WaterVision) } else { at(IntelType::Vision) | if wet { at(IntelType::WaterVision) } else { 0 } };
        let omni = at(IntelType::Omni);
        let radar = if under { 0 } else { at(IntelType::Radar) };
        let sonar = if wet { at(IntelType::Sonar) } else { 0 };
        // Counter-intel only matters where some sensor could see it (it's
        // rare, so `hidden` only runs then).
        let hidden = if (optical & !omni) | radar | sonar != 0 { self.hidden(e) } else { 0 };
        let seen = omni | if hidden & CLOAK == 0 { optical } else { 0 };
        let radar = if hidden & RADAR_STEALTH == 0 { radar } else { 0 };
        let sonar = if hidden & SONAR_STEALTH == 0 { sonar } else { 0 };
        // Every army sharing intel with one of those knows it too.
        let share = |mut raw: u64| {
            let mut out = 0u64;
            while raw != 0 {
                let b = raw.trailing_zeros() as usize;
                raw &= raw - 1;
                out |= self.shared_by.get(b).copied().unwrap_or(1 << b);
            }
            out
        };
        [share(seen), share(radar), share(sonar), share(omni)]
    }

    /// Per unit: enemy intel now, diffed against last update into
    /// `changes`; first sightings into `seen_ever` and `first_seen`.
    fn record_intel(&mut self, world: &EntityWorld) {
        self.changes.clear();
        self.first_seen.clear();
        let any = self.active[0] | self.active[1] | self.active[2] | self.active[3] | self.active[4];
        let mut next = std::mem::take(&mut self.now_next);
        next.clear();
        let scan = crate::prof_span!("intel blip scan");
        if any != 0 {
            for e in world.units() {
                let (Some(owner), Some(_)) = (e.army, e.unit.as_deref()) else { continue };
                if e.destroyed {
                    continue;
                }
                // Its own side doesn't count.
                let own = self.shared_by.get(owner as usize).copied().unwrap_or(1 << owner.min(MAX_ARMIES));
                let m = self.masks(e).map(|m| m & !own);
                if m != [0; 4] {
                    next.push((e.id, m));
                }
            }
        }
        drop(scan);
        // Merge with last update's (both in id order).
        let _diff = crate::prof_span!("intel blip diff");
        let prev = std::mem::take(&mut self.now);
        let (mut i, mut j) = (0, 0);
        while i < prev.len() || j < next.len() {
            let (id, before, after) = match (prev.get(i), next.get(j)) {
                (Some(&(a, ma)), Some(&(b, mb))) if a == b => {
                    i += 1;
                    j += 1;
                    (a, ma, mb)
                }
                (Some(&(a, ma)), Some(&(b, _))) if a < b => {
                    i += 1;
                    (a, ma, [0; 4])
                }
                (Some(&(a, ma)), None) => {
                    i += 1;
                    (a, ma, [0; 4])
                }
                (_, Some(&(b, mb))) => {
                    j += 1;
                    (b, [0; 4], mb)
                }
                (None, None) => break,
            };
            // A unit that's gone (destroyed) isn't reported as lost.
            if after == [0; 4] && world.get(id).is_none_or(|e| e.destroyed) {
                continue;
            }
            let mut by_army: [u64; 4] = [0; 4];
            for k in 0..4 {
                by_army[k] = before[k] ^ after[k];
            }
            let mut armies = by_army.iter().fold(0u64, |a, m| a | m);
            while armies != 0 {
                let b = armies.trailing_zeros();
                armies &= armies - 1;
                for (k, recon) in Recon::ALL.into_iter().enumerate() {
                    if by_army[k] & (1 << b) != 0 {
                        self.changes.push(IntelChange { unit: id, army: b, recon, on: after[k] & (1 << b) != 0 });
                    }
                }
            }
            if after[0] != 0 {
                let ever = self.seen_ever.entry(id).or_insert(0);
                let mut new = after[0] & !*ever;
                *ever |= after[0];
                while new != 0 {
                    let b = new.trailing_zeros();
                    new &= new - 1;
                    self.first_seen.push((id, b));
                }
            }
        }
        self.now = next;
        self.now_next = prev;
    }

    /// Forgets the sightings of units inside the XZ rectangle
    /// (`FlushIntelInRect`): their remembered blips go; current intel
    /// re-detects whatever is still covered.
    pub fn flush_in_rect(&mut self, world: &EntityWorld, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx) {
        self.seen_ever.retain(|id, _| {
            world.get(*id).is_none_or(|e| {
                let p = e.position();
                p.x < min_x || p.x > max_x || p.z < min_z || p.z > max_z
            })
        });
    }

    /// Forgets destroyed units (call after compaction removed them).
    pub fn forget(&mut self, gone: impl Fn(EntityId) -> bool) {
        self.seen_ever.retain(|&id, _| !gone(id));
    }
}

/// SplitMix64 finaliser: a well-mixed, platform-independent hash.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::EntityKind;
    use crate::engine::sim::intel::IntelState;
    use crate::engine::sim::math::FxQuat;

    fn unit(w: &mut EntityWorld, army: u32, x: f64, z: f64, vision: f64) -> EntityId {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(army), FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        if vision > 0.0 {
            w.get_mut(id).unwrap().cold.intel.insert(IntelType::Vision, IntelState { enabled: true, radius: Fx::from_num(vision) });
        }
        id
    }

    fn grids(w: &EntityWorld) -> IntelGrids {
        let mut g = IntelGrids::new(256, 256);
        g.set_alliances(2, |_, _| false);
        g.update(w, |_| None);
        g
    }

    #[test]
    fn vision_covers_its_radius_and_follows_the_source() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let scout = unit(&mut w, 1, 100.0, 100.0, 20.0);
        let far = FxVec3::from_f64(140.0, 0.0, 100.0);
        let mut g = grids(&w);
        assert!(g.detection(1, FxVec3::from_f64(115.0, 0.0, 100.0), Layer::Land).seen);
        assert!(!g.detection(1, far, Layer::Land).seen);
        assert!(!g.detection(2, FxVec3::from_f64(100.0, 0.0, 100.0), Layer::Land).seen, "army 2 has no intel");
        w.set_position(scout, FxVec3::from_f64(135.0, 0.0, 100.0));
        g.update(&w, |_| None);
        assert!(g.detection(1, far, Layer::Land).seen, "moved with the scout");
        assert!(!g.detection(1, FxVec3::from_f64(100.0, 0.0, 100.0), Layer::Land).seen, "old area dark again");
    }

    /// The row-span stamp covers exactly the cells moho's column loop does
    /// (`CIntelGrid::Raster`, transcribed below: columns `cx - r..cx + r`,
    /// rows `cz - h..cz + h`, `h = trunc(sqrt(r^2 - (cx - x)^2))`), for
    /// radii and positions across cell sizes and the map edges, on the
    /// vision grid (2-unit cells) and a 4-unit one.
    #[test]
    fn stamp_spans_match_moho_raster() {
        for (x, z, r) in [(100.0, 100.0, 20.0), (2.0, 3.0, 35.0), (250.0, 10.0, 60.0), (128.0, 128.0, 4.0), (130.0, 70.0, 0.5), (0.0, 255.0, 113.0), (101.0, 102.0, 18.0), (99.0, 97.0, 3.0)] {
            for sensor in [VISION, sensor_index(IntelType::Radar).unwrap()] {
                let mut w = EntityWorld::default();
                w.set_bounds(256, 256);
                let id = unit(&mut w, 1, x, z, 0.0);
                give(&mut w, id, SENSORS[sensor], r);
                let g = grids(&w);
                let shift = shift_of(sensor);
                let (cols, rows) = g.dims(shift);
                let p = FxVec3::from_f64(x, 0.0, z);
                let (cx, cz) = (IntelGrids::cell_of(p.x, shift), IntelGrids::cell_of(p.z, shift));
                let rc = (r as i32) >> shift;
                let mut want = vec![false; (cols * rows) as usize];
                for x in (cx - rc).max(0)..(cx + rc).min(cols as i32) {
                    let h = ((rc * rc - (cx - x) * (cx - x)) as f64).sqrt() as i32;
                    for z in (cz - h).max(0)..(cz + h).min(rows as i32) {
                        want[(z as u32 * cols + x as u32) as usize] = true;
                    }
                }
                let counts = &g.armies[1].as_ref().unwrap().counts[sensor];
                let counts = if counts.is_empty() { &vec![0; want.len()] } else { counts };
                for (i, &want) in want.iter().enumerate() {
                    assert_eq!(counts[i] > 0, want, "sensor {sensor}, r {r} at ({x}, {z}): cell ({}, {})", i as u32 % cols, i as u32 / cols);
                }
            }
        }
    }

    /// The real game's `probe_cells` (`dev/probes/maps/probe_cells`): an
    /// engineer (vision 18) at a 4-unit cell's centre sees an enemy at
    /// (4, 0), (4, 1) and (2, 3) cells off, not at (5, 0), (3, 3) or (4, 2).
    #[test]
    fn vision_cells_match_the_real_game() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        unit(&mut w, 1, 102.0, 102.0, 18.0);
        let g = grids(&w);
        for (dx, dz, seen) in [(4, 0, true), (5, 0, false), (4, 1, true), (3, 3, false), (4, 2, false), (2, 3, true)] {
            let p = FxVec3::from_f64(102.0 + 4.0 * dx as f64, 0.0, 102.0 + 4.0 * dz as f64);
            assert_eq!(g.detection(1, p, Layer::Land).seen, seen, "({dx}, {dz})");
        }
    }

    #[test]
    fn incremental_stamps_match_a_fresh_build() {
        use crate::engine::sim::rng::SimRng;
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let mut rng = SimRng::new(5);
        let ids: Vec<_> = (0..60).map(|i| unit(&mut w, 1 + i % 2, (rng.next_u64() % 250) as f64, (rng.next_u64() % 250) as f64, 10.0 + (i % 5) as f64 * 7.0)).collect();
        let mut g = grids(&w);
        for _ in 0..20 {
            for &id in &ids {
                if rng.next_u64() % 3 == 0 {
                    let p = w.get(id).unwrap().position();
                    let d = |v: Fx, r: u64| (v + Fx::from_num((r % 21) as i32 - 10)).clamp(Fx::ZERO, Fx::from_num(250));
                    w.set_position(id, FxVec3 { x: d(p.x, rng.next_u64()), y: p.y, z: d(p.z, rng.next_u64()) });
                }
            }
            if rng.next_u64() % 4 == 0 {
                w.destroy(ids[(rng.next_u64() % 60) as usize]);
            }
            g.update(&w, |_| None);
        }
        // Cover masks track the counts exactly.
        for sensor in 0..8 {
            for (i, &m) in g.cover[sensor].iter().enumerate() {
                for a in 1..=2usize {
                    let n = g.armies[a].as_ref().map_or(0, |ag| ag.counts[sensor].get(i).copied().unwrap_or(0));
                    assert_eq!(m >> a & 1 == 1, n > 0, "sensor {sensor} cell {i} army {a}");
                }
            }
        }
        // Who sees whom matches asking each army in turn.
        for e in w.iter().filter(|e| !e.destroyed) {
            let owner = e.army.unwrap();
            for a in 1..=2u32 {
                let d = g.detect(a, e);
                let expect = a != owner && d.seen;
                let m = g.masks(e);
                assert_eq!(m[0] >> a & 1 == 1 && a != owner, expect, "unit {} army {a}", e.id);
                assert_eq!([m[1] >> a & 1 == 1, m[2] >> a & 1 == 1, m[3] >> a & 1 == 1], [d.radar, d.sonar, d.omni], "unit {} army {a}", e.id);
                if expect {
                    assert!(g.ever_seen(a, e.id));
                }
            }
        }
        // Discs lag their sources (redrawn a third of a radius on): a fresh
        // build from where each was drawn matches the incremental one.
        for &((id, _), _, at) in &g.stamps.clone() {
            w.set_position(id, at);
        }
        let fresh = grids(&w);
        for a in 1..=2usize {
            assert_eq!(g.armies[a].as_ref().unwrap().counts, fresh.armies[a].as_ref().unwrap().counts, "army {a}");
        }
    }

    #[test]
    fn allies_share_vision_and_layers_pick_the_sensor() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let radar = unit(&mut w, 2, 50.0, 50.0, 0.0);
        w.get_mut(radar).unwrap().cold.intel.insert(IntelType::Radar, IntelState { enabled: true, radius: Fx::from_num(40) });
        unit(&mut w, 2, 200.0, 200.0, 20.0);
        let mut g = IntelGrids::new(256, 256);
        g.set_alliances(3, |a, b| (a == 2 && b == 3) || (a == 3 && b == 2));
        g.update(&w, |_| None);
        let p = FxVec3::from_f64(60.0, 0.0, 50.0);
        let d = g.detection(3, p, Layer::Land);
        assert!(d.radar && !d.seen, "ally's radar, not vision");
        assert!(!g.detection(3, p, Layer::Sub).radar, "radar doesn't see under water");
        assert!(g.detection(3, FxVec3::from_f64(205.0, 0.0, 200.0), Layer::Air).seen);
        assert!(!g.detection(1, p, Layer::Land).any(), "enemies get nothing");
    }

    /// Vision explores for good (radar doesn't), allies share it, and the
    /// fog view reads visible / radar / explored / unexplored per cell.
    #[test]
    fn vision_explores_and_the_fog_view_remembers() {
        use fog_level::*;
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let scout = unit(&mut w, 1, 50.0, 50.0, 12.0);
        give(&mut w, scout, IntelType::Radar, 60.0);
        let mut g = IntelGrids::new(256, 256);
        g.set_alliances(3, |a, b| (a == 1 && b == 3) || (a == 3 && b == 1));
        g.update(&w, |_| None);
        let at = |x: f64, z: f64| FxVec3::from_f64(x, 0.0, z);
        assert!(g.explored(1, at(55.0, 50.0)));
        assert!(!g.explored(1, at(100.0, 50.0)), "radar doesn't explore");
        assert!(g.explored(3, at(55.0, 50.0)), "allies share it");
        assert!(!g.explored(2, at(55.0, 50.0)));
        let since = g.update_count();
        w.set_position(scout, FxVec3::from_f64(150.0, 0.0, 150.0));
        g.update(&w, |_| None);
        assert!(g.explored(1, at(55.0, 50.0)), "still explored after it left");
        assert!(g.row_changed_since(50 / 2, since) && g.row_changed_since(150 / 2, since));
        assert!(!g.row_changed_since(250 / 2, since), "untouched rows unchanged");
        let (cols, _, _) = g.dimensions();
        let mut row = vec![9u8; cols as usize];
        g.fog_row(1, 150 / 2, false, &mut row);
        assert_eq!(row[150 / 2], VISIBLE);
        assert_eq!(row[(150 + 40) / 2], RADAR);
        assert_eq!(row[250 / 2], UNEXPLORED);
        g.fog_row(1, 50 / 2, false, &mut row);
        assert_eq!(row[50 / 2], EXPLORED, "explored, out of sight");
        g.fog_row(1, 250 / 2, true, &mut row);
        assert!(row.iter().all(|&l| l == EXPLORED), "'explored' games start explored");
        g.fog_row(2, 150 / 2, false, &mut row);
        assert!(row.iter().all(|&l| l == UNEXPLORED), "enemies get nothing");
    }

    fn give(w: &mut EntityWorld, id: EntityId, t: IntelType, radius: f64) {
        w.get_mut(id).unwrap().cold.intel.insert(t, IntelState { enabled: true, radius: Fx::from_num(radius) });
    }

    #[test]
    fn cloak_hides_from_vision_but_not_omni() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let eye = unit(&mut w, 1, 100.0, 100.0, 30.0);
        let ghost = unit(&mut w, 2, 110.0, 100.0, 0.0);
        give(&mut w, ghost, IntelType::Cloak, 0.0);
        let mut g = grids(&w);
        assert!(!g.detect(1, w.get(ghost).unwrap()).any(), "cloaked: unseen");
        give(&mut w, eye, IntelType::Omni, 20.0);
        g.update(&w, |_| None);
        let d = g.detect(1, w.get(ghost).unwrap());
        assert!(d.seen && d.omni, "omni counters cloak");
    }

    #[test]
    fn stealth_field_hides_allies_from_radar_only() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let radar = unit(&mut w, 1, 50.0, 100.0, 0.0);
        give(&mut w, radar, IntelType::Radar, 120.0);
        let field = unit(&mut w, 2, 150.0, 100.0, 0.0);
        give(&mut w, field, IntelType::RadarStealthField, 20.0);
        let hidden = unit(&mut w, 2, 155.0, 100.0, 0.0);
        let outside = unit(&mut w, 2, 150.0, 140.0, 0.0);
        let g = grids(&w);
        assert!(!g.detect(1, w.get(hidden).unwrap()).radar, "inside the field");
        assert!(g.detect(1, w.get(outside).unwrap()).radar, "outside it");
        let mut w2 = w;
        let scout = unit(&mut w2, 1, 155.0, 105.0, 15.0);
        let _ = scout;
        let g2 = grids(&w2);
        assert!(g2.detect(1, w2.get(hidden).unwrap()).seen, "stealth doesn't stop vision");
    }

    #[test]
    fn jammer_fakes_show_on_enemy_radar_deterministically() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let radar = unit(&mut w, 1, 100.0, 100.0, 0.0);
        give(&mut w, radar, IntelType::Radar, 150.0);
        let jam = unit(&mut w, 2, 130.0, 130.0, 0.0);
        give(&mut w, jam, IntelType::Jammer, 0.0);
        let spec = JammerSpec { blips: 6, min: Fx::from_num(5), max: Fx::from_num(15) };
        let run = |w: &EntityWorld| {
            let mut g = IntelGrids::new(256, 256);
            g.set_alliances(2, |_, _| false);
            g.update(w, |e| (e.id == jam).then_some(spec));
            g
        };
        let g = run(&w);
        let fakes = g.fake_blips(1);
        assert_eq!(fakes.len(), 6);
        assert_eq!(fakes, run(&w).fake_blips(1), "same every run");
        let jp = w.get(jam).unwrap().position();
        for f in &fakes {
            let (dx, dz) = ((f.x - jp.x).to_num::<f64>(), (f.z - jp.z).to_num::<f64>());
            let d = (dx * dx + dz * dz).sqrt();
            assert!((4.99..=15.01).contains(&d), "within JamRadius: {d}");
        }
        assert!(g.fake_blips(2).is_empty(), "owner sees no fakes");
        give(&mut w, radar, IntelType::Omni, 150.0);
        assert!(run(&w).fake_blips(1).is_empty(), "omni exposes them");
    }
}
