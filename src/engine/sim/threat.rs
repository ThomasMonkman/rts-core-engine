//! AI threat map (moho's "IMAP"): per-army threat summed over square map
//! cells, queried by AI brains (`GetHighestThreatPosition`,
//! `GetThreatAtPosition`, ...).
//!
//! Cell size follows the map like moho's (FAF's `IMAPConfiguration` mirrors
//! it): 32 world units up to 10 km maps, 64 at 20 km, 128 at 40 km, 256
//! beyond. Each unit carries a threat value per [`ThreatType`], computed once
//! at spawn from its blueprint (`Defense.*ThreatLevel`) and categories.
//!
//! The map changes only on moho's 30-tick update grid ([`UPDATE_TICKS`],
//! `api::threat::tick`); queries in between read the last update. Each brain
//! sees other armies' threat only as far as it knows their units (`knows` in
//! [`ThreatMap::update`]): allies exactly, enemies detected now, structures
//! it has seen, and what it remembers of units it has lost sight of
//! (moho's recon blips outlive sight): a mobile unit's threat where it was
//! last detected, whole for [`MEMORY_HOLD_TICKS`] then fading over 50
//! updates; a structure's for good, also once destroyed out of sight. Its
//! own army's threat is exact.
//!
//! Each jammer fake a brain believes in (on its radar, not known fake:
//! `sim::vision::Fake`) adds its jammer's whole threat at the fake's spot
//! (probe_intel_jam). Not modelled: the probe's previous generation of
//! fakes still counting after a jammer reset.
//!
//! Script-assigned threat (`AssignThreatAtPosition`) counts from the next
//! update and loses `decay` x its amount per update while positive;
//! negative amounts never fade and subtract from the cell's unit threat.
//!
//! From dev/probes `probe_ai_threat` / `probe_ai_find` (FAF's game), not the
//! binary: the update grid, memory timings and assigned decay. Inferred:
//! which categories feed each type and that `restriction` doesn't change the
//! intel rule (it's ignored).

use crate::engine::sim::entity::{EntityKind, EntityWorld};
use crate::engine::sim::math::{Fx, FxVec3};

/// Moho `BrainThreatType`, in its declared order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum ThreatType {
    Overall,
    OverallNotAssigned,
    StructuresNotMex,
    Structures,
    Naval,
    Air,
    Land,
    Experimental,
    Commander,
    Artillery,
    AntiAir,
    AntiSurface,
    AntiSub,
    Economy,
    Unknown,
}

pub const THREAT_TYPES: usize = 15;

impl ThreatType {
    pub const ALL: [ThreatType; THREAT_TYPES] = [
        ThreatType::Overall,
        ThreatType::OverallNotAssigned,
        ThreatType::StructuresNotMex,
        ThreatType::Structures,
        ThreatType::Naval,
        ThreatType::Air,
        ThreatType::Land,
        ThreatType::Experimental,
        ThreatType::Commander,
        ThreatType::Artillery,
        ThreatType::AntiAir,
        ThreatType::AntiSurface,
        ThreatType::AntiSub,
        ThreatType::Economy,
        ThreatType::Unknown,
    ];

    /// The type a script names, ignoring case: moho's is the reflected enum
    /// `EThreatType` (`THREATTYPE_*`, FA's exe 0xa80e00), read the way gpg
    /// reads any enum, `_stricmp` (gpgcore.dll `REnumType::GetEnumValue`
    /// 0x100153f0). LOUD's bomber AI asks for 'LAND'.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.name().eq_ignore_ascii_case(s))
    }

    pub fn name(self) -> &'static str {
        match self {
            ThreatType::Overall => "Overall",
            ThreatType::OverallNotAssigned => "OverallNotAssigned",
            ThreatType::StructuresNotMex => "StructuresNotMex",
            ThreatType::Structures => "Structures",
            ThreatType::Naval => "Naval",
            ThreatType::Air => "Air",
            ThreatType::Land => "Land",
            ThreatType::Experimental => "Experimental",
            ThreatType::Commander => "Commander",
            ThreatType::Artillery => "Artillery",
            ThreatType::AntiAir => "AntiAir",
            ThreatType::AntiSurface => "AntiSurface",
            ThreatType::AntiSub => "AntiSub",
            ThreatType::Economy => "Economy",
            ThreatType::Unknown => "Unknown",
        }
    }
}

/// A unit's threat per type.
pub type ThreatLevels = [Fx; THREAT_TYPES];

/// What a unit is, for the category-based threat types.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreatKinds {
    pub structure: bool,
    pub mass_extraction: bool,
    pub naval: bool,
    pub air: bool,
    pub land: bool,
    pub experimental: bool,
    pub commander: bool,
    pub artillery: bool,
}

/// A unit's threat from its blueprint `Defense` levels and categories:
/// the anti-* and economy types take their own level; overall and every
/// category type the unit belongs to take the sum of all levels.
pub fn unit_threat(air: Fx, surface: Fx, sub: Fx, economy: Fx, k: ThreatKinds) -> ThreatLevels {
    let total = air + surface + sub + economy;
    let mut t = [Fx::ZERO; THREAT_TYPES];
    let mut set = |ty: ThreatType, v: Fx| t[ty as usize] = v;
    set(ThreatType::Overall, total);
    set(ThreatType::OverallNotAssigned, total);
    set(ThreatType::AntiAir, air);
    set(ThreatType::AntiSurface, surface);
    set(ThreatType::AntiSub, sub);
    set(ThreatType::Economy, economy);
    let mut member = |ty: ThreatType, yes: bool| {
        if yes {
            t[ty as usize] = total;
        }
    };
    member(ThreatType::Structures, k.structure);
    member(ThreatType::StructuresNotMex, k.structure && !k.mass_extraction);
    member(ThreatType::Naval, k.naval);
    member(ThreatType::Air, k.air);
    member(ThreatType::Land, k.land);
    member(ThreatType::Experimental, k.experimental);
    member(ThreatType::Commander, k.commander);
    member(ThreatType::Artillery, k.artillery);
    t
}

/// Threat a brain placed by script (`AssignThreatAtPosition`), one entry
/// per (cell, type): a second assignment there adds to `value` and keeps the
/// first one's `step` (dev/probes probe_ai_threat: 70 + 30 at 0.1 went on
/// losing 10 per update).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Assigned {
    cell: u32,
    ty: ThreatType,
    value: Fx,
    /// Lost per map update while the value is positive: the first
    /// assignment's amount x its decay fraction.
    step: Fx,
}

/// What a brain knows of one unit of another army at a map update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knowledge {
    /// An allied unit: its threat counts exactly, never remembered.
    Exact,
    /// Detected now (any intel).
    Detected,
    /// A structure seen before, not detected now.
    SeenStructure,
    /// Neither: only what the brain remembers of it counts.
    Unknown,
}

/// A brain's memory of an enemy unit (moho keeps the unit's recon blip after
/// sight is lost): its threat where it was last known.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Memory {
    army: u32,
    cell: u32,
    threat: ThreatLevels,
    /// The last map update that detected it (none: a structure seen only
    /// between updates).
    seen: Option<u64>,
    /// Structures are remembered undecayed.
    structure: bool,
}

/// Ticks between threat map updates: moho's map changes only on a 30-tick
/// grid (dev/probes probe_ai_threat: a unit placed at +45 counts from +60,
/// an assignment at +305 from +330).
pub const UPDATE_TICKS: u64 = 30;
/// A mobile unit's remembered threat stays whole for this long after the
/// last update that detected it, then loses a fiftieth of itself per update
/// (probe_ai_threat / probe_ai_find: sight lost at +85 and at +75, last
/// detected at the +60 update both times, first loss at +360; the ACU's 45
/// then reads 44.1, 43.2 ... 36.9 at +600, the Fatboy's 993.36 973.49,
/// 953.62). Structures seen keep their threat until seen gone.
pub const MEMORY_HOLD_TICKS: u64 = 300;
pub const MEMORY_DECAY_STEPS: u64 = 50;

impl Memory {
    /// Fiftieths of its threat still remembered at update `now`; none once
    /// it has all decayed.
    fn left(&self, now: u64) -> Option<Fx> {
        let steps = match self.seen {
            Some(seen) if !self.structure && now >= seen + MEMORY_HOLD_TICKS => (now - seen - MEMORY_HOLD_TICKS) / UPDATE_TICKS + 1,
            _ => 0,
        };
        (steps < MEMORY_DECAY_STEPS).then(|| Fx::from_num(MEMORY_DECAY_STEPS - steps))
    }
}

/// `grids[army]`, made (zeroed, `n` cells) if missing.
fn grid_of(grids: &mut Vec<Vec<ThreatLevels>>, army: u32, n: usize) -> &mut Vec<ThreatLevels> {
    let army = army as usize;
    if grids.len() <= army {
        grids.resize_with(army + 1, Vec::new);
    }
    let grid = &mut grids[army];
    if grid.len() != n {
        grid.resize(n, [Fx::ZERO; THREAT_TYPES]);
    }
    grid
}

fn add(grid: &mut [ThreatLevels], index: usize, threat: &ThreatLevels) {
    for (acc, v) in grid[index].iter_mut().zip(threat.iter()) {
        *acc = acc.saturating_add(*v);
    }
}

/// Cell edge (world units) for a map of the given size.
pub fn cell_size(width: u32, height: u32) -> u32 {
    match width.max(height) {
        0..=512 => 32,
        513..=1024 => 64,
        1025..=2048 => 128,
        _ => 256,
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct ThreatMap {
    cell: u32,
    cols: u32,
    rows: u32,
    /// `cells[army][cell]`, army = moho army index (1-based; 0 unused).
    cells: Vec<Vec<ThreatLevels>>,
    /// `known[brain][army][cell]`: what `brain` knows of `cells[army]` (its
    /// allies exactly, enemies detected or remembered), as of the last
    /// update.
    known: Vec<Vec<Vec<ThreatLevels>>>,
    /// Built at least once (queries before the first update build it).
    built: bool,
    /// Per brain: what it remembers of other armies' units, by unit id.
    memory: Vec<std::collections::BTreeMap<u32, Memory>>,
    /// Script-assigned threat per brain (army index), as of the last update.
    assigned: Vec<Vec<Assigned>>,
    /// Assignments made since the last update: they count from the next.
    pending: Vec<(u32, Assigned)>,
}

impl ThreatMap {
    pub fn new(width: u32, height: u32) -> Self {
        let cell = cell_size(width, height);
        let cols = width.div_ceil(cell).max(1);
        let rows = height.div_ceil(cell).max(1);
        Self { cell, cols, rows, ..Default::default() }
    }

    pub fn cell_size(&self) -> u32 {
        self.cell
    }

    /// Whether an update has run yet.
    pub fn is_built(&self) -> bool {
        self.built
    }

    fn axis(&self, v: Fx, n: u32) -> u32 {
        let c = v.floor().to_num::<i64>() / self.cell as i64;
        c.clamp(0, n as i64 - 1) as u32
    }

    /// (col, row) of world position `p`.
    pub fn cell_of(&self, p: FxVec3) -> (u32, u32) {
        (self.axis(p.x, self.cols), self.axis(p.z, self.rows))
    }

    fn index_of(&self, p: FxVec3) -> u32 {
        let (c, r) = self.cell_of(p);
        r * self.cols + c
    }

    /// World centre of a cell (y = 0).
    pub fn center(&self, col: u32, row: u32) -> FxVec3 {
        let half = Fx::from_num(self.cell) / 2;
        FxVec3 {
            x: Fx::from_num(col * self.cell) + half,
            y: Fx::ZERO,
            z: Fx::from_num(row * self.cell) + half,
        }
    }

    /// The map update at tick `now`: rebuilds the per-army sums, then for each
    /// of `brains` what it knows of other armies (`knows(brain, unit)`),
    /// updating and ageing its memory; then ages assigned threat and adds
    /// the assignments made since the last update. Units are visited in id
    /// order and memories in id order, so the (exact, fixed-point) sums are
    /// the same on every machine.
    pub fn update(&mut self, world: &EntityWorld, now: u64, brains: &[u32], knows: &dyn Fn(u32, &crate::engine::sim::entity::Entity) -> Knowledge) {
        self.update_with_fakes(world, now, brains, knows, &[]);
    }

    /// [`ThreatMap::update`] plus jammer fakes: each `(brain, jammer, at)`
    /// adds the jammer's whole threat at `at` to what `brain` knows of the
    /// jammer's army (probe_intel_jam: every fake on the brain's radar and
    /// not known fake counts the jammer's full threat at its own spot; not
    /// remembered once it goes). In the order given.
    pub fn update_with_fakes(&mut self, world: &EntityWorld, now: u64, brains: &[u32], knows: &dyn Fn(u32, &crate::engine::sim::entity::Entity) -> Knowledge, fakes: &[(u32, crate::engine::sim::entity::EntityId, FxVec3)]) {
        let n = (self.cols * self.rows) as usize;
        self.built = true;
        for grid in self.cells.iter_mut() {
            grid.iter_mut().for_each(|c| *c = [Fx::ZERO; THREAT_TYPES]);
        }
        let live = |e: &&crate::engine::sim::entity::Entity| e.kind == EntityKind::Unit && !e.destroyed && e.army.is_some() && e.unit.is_some();
        for e in world.iter().filter(live) {
            let (Some(army), Some(data)) = (e.army, e.unit.as_deref()) else { continue };
            let index = self.index_of(e.position()) as usize;
            add(grid_of(&mut self.cells, army, n), index, &data.threat);
        }
        for &brain in brains {
            let b = brain as usize;
            if self.known.len() <= b {
                self.known.resize_with(b + 1, Vec::new);
                self.memory.resize_with(b + 1, Default::default);
            }
            for grid in self.known[b].iter_mut() {
                grid.iter_mut().for_each(|c| *c = [Fx::ZERO; THREAT_TYPES]);
            }
            let mut memory = std::mem::take(&mut self.memory[b]);
            // Ids counted live this update (ascending: the world is in id order).
            let mut counted: Vec<u32> = Vec::new();
            for e in world.iter().filter(live) {
                let (Some(army), Some(data)) = (e.army, e.unit.as_deref()) else { continue };
                if army == brain {
                    continue;
                }
                let index = self.index_of(e.position());
                let structure = data.threat[ThreatType::Structures as usize] > Fx::ZERO;
                match knows(brain, e) {
                    Knowledge::Exact => {
                        memory.remove(&e.id);
                    }
                    Knowledge::Detected => {
                        memory.insert(e.id, Memory { army, cell: index, threat: data.threat, seen: Some(now), structure });
                    }
                    Knowledge::SeenStructure => {
                        let seen = memory.get(&e.id).and_then(|m| m.seen);
                        memory.insert(e.id, Memory { army, cell: index, threat: data.threat, seen, structure: true });
                    }
                    Knowledge::Unknown => continue,
                }
                counted.push(e.id);
                add(grid_of(&mut self.known[b], army, n), index as usize, &data.threat);
            }
            // Units out of intel, and gone ones: one that died in sight
            // (detected at the last update) is forgotten, one destroyed out
            // of sight stays remembered (probe_ai_threat: the tank and point
            // defence destroyed unseen at +165 still count).
            memory.retain(|&id, m| {
                if counted.binary_search(&id).is_ok() {
                    return true;
                }
                let alive = world.get(id).is_some_and(|e| !e.destroyed && e.kind == EntityKind::Unit);
                if !alive && m.seen.is_some_and(|s| s + UPDATE_TICKS >= now) {
                    return false;
                }
                let Some(left) = m.left(now) else { return false };
                let grid = grid_of(&mut self.known[b], m.army, n);
                for (acc, v) in grid[m.cell as usize].iter_mut().zip(m.threat.iter()) {
                    *acc = acc.saturating_add(v.saturating_mul(left) / Fx::from_num(MEMORY_DECAY_STEPS));
                }
                true
            });
            self.memory[b] = memory;
            for &(_, jammer, at) in fakes.iter().filter(|f| f.0 == brain) {
                let Some((army, data)) = world.get(jammer).and_then(|e| Some((e.army?, e.unit.as_deref()?))) else { continue };
                let index = self.index_of(at) as usize;
                add(grid_of(&mut self.known[b], army, n), index, &data.threat);
            }
        }
        self.age_assigned();
    }

    /// Ages assigned threat by one update (a positive value loses its step
    /// and goes when it reaches 0; a negative one stays as it is), then adds
    /// the assignments made since the last update, which take their first
    /// step at once (100 at 0.1 placed at +305 reads 90 from +330).
    fn age_assigned(&mut self) {
        for (brain, a) in std::mem::take(&mut self.pending) {
            let brain = brain as usize;
            if self.assigned.len() <= brain {
                self.assigned.resize_with(brain + 1, Vec::new);
            }
            let list = &mut self.assigned[brain];
            match list.iter_mut().find(|x| x.cell == a.cell && x.ty == a.ty) {
                Some(x) => x.value = x.value.saturating_add(a.value),
                None => list.push(a),
            }
        }
        for list in self.assigned.iter_mut() {
            list.retain_mut(|a| {
                if a.value <= Fx::ZERO {
                    return a.value != Fx::ZERO;
                }
                a.value = a.value.saturating_sub(a.step);
                a.value > Fx::ZERO
            });
        }
    }

    /// `AssignThreatAtPosition`: `value` more threat `brain` believes is at
    /// `p`, losing `decay` x `value` per update while positive; it counts
    /// from the next update.
    pub fn assign(&mut self, brain: u32, p: FxVec3, value: Fx, decay: Fx, ty: ThreatType) {
        let cell = self.index_of(p);
        self.pending.push((brain, Assigned { cell, ty, value, step: value.saturating_mul(decay.max(Fx::ZERO)) }));
    }

    /// `brain`'s knowledge tables, once built.
    fn known_for(&self, brain: u32) -> Option<&[Vec<ThreatLevels>]> {
        let b = brain as usize;
        (self.built && b < self.known.len()).then(|| self.known[b].as_slice())
    }

    /// Threat of type `ty` in cell `index` from armies passing `counts`,
    /// plus what `brain` assigned there (assigned threat feeds its own type
    /// and `Overall`, never `OverallNotAssigned`).
    fn cell_threat(&self, index: u32, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool) -> Fx {
        self.cell_threat_with(self.known_for(brain), index, ty, brain, counts)
    }

    fn cell_threat_with(&self, known: Option<&[Vec<ThreatLevels>]>, index: u32, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool) -> Fx {
        let mut sum = Fx::ZERO;
        let (i, t) = (index as usize, ty as usize);
        match known {
            // Other armies through what this brain knows (intel on).
            Some(k) => {
                for army in 0..self.cells.len().max(k.len()) {
                    if !counts(army as u32) {
                        continue;
                    }
                    let grid = if army as u32 == brain { self.cells.get(army) } else { k.get(army) };
                    if let Some(g) = grid.filter(|g| !g.is_empty()) {
                        sum = sum.saturating_add(g[i][t]);
                    }
                }
            }
            None => {
                for (army, grid) in self.cells.iter().enumerate() {
                    if !grid.is_empty() && counts(army as u32) {
                        sum = sum.saturating_add(grid[i][t]);
                    }
                }
            }
        }
        if let Some(list) = self.assigned.get(brain as usize) {
            for a in list.iter().filter(|a| a.cell == index) {
                if a.ty == ty || (ty == ThreatType::Overall && a.ty != ThreatType::OverallNotAssigned) {
                    sum = sum.saturating_add(a.value);
                }
            }
        }
        sum
    }

    /// Every cell's threat (row-major) for debug views: (cols, rows, cell
    /// size, values).
    pub fn grid_values(&self, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool) -> (u32, u32, u32, Vec<f32>) {
        let values = (0..self.cols * self.rows).map(|i| self.cell_threat(i, ty, brain, counts).to_num::<f32>()).collect();
        (self.cols, self.rows, self.cell, values)
    }

    /// Cells within `rings` of (col, row), row-major.
    fn ring(&self, col: u32, row: u32, rings: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        let (c0, c1) = (col.saturating_sub(rings), (col + rings).min(self.cols - 1));
        let (r0, r1) = (row.saturating_sub(rings), (row + rings).min(self.rows - 1));
        (r0..=r1).flat_map(move |r| (c0..=c1).map(move |c| (c, r)))
    }

    fn ring_threat(&self, col: u32, row: u32, rings: u32, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool) -> Fx {
        let known = self.known_for(brain);
        self.ring(col, row, rings)
            .map(|(c, r)| self.cell_threat_with(known, r * self.cols + c, ty, brain, counts))
            .fold(Fx::ZERO, |a, b| a.saturating_add(b))
    }

    /// Threat around `p`: the cell plus `rings` cells around it.
    pub fn threat_at(&self, p: FxVec3, rings: u32, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool) -> Fx {
        let (c, r) = self.cell_of(p);
        self.ring_threat(c, r, rings, ty, brain, counts)
    }

    /// The cell whose ring holds the most threat: its centre (y = 0) and the
    /// value. Ties (the empty map included) go to the cell whose centre is
    /// nearest `origin` (the brain's start position; then row-major order):
    /// dev/probes probe_ai_threat, army 1 starting at (672.5, 346.5): nothing
    /// known gives (672, 0, 352), the Fatboy's cell (5, 13) with ring 1 the
    /// ring cell nearest the start, (416, 0, 800).
    pub fn highest(&self, rings: u32, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool, origin: crate::engine::sim::math::FxVec2) -> (FxVec3, Fx) {
        let dist2 = |c: u32, r: u32| {
            let m = self.center(c, r);
            let (dx, dz) = (m.x - origin.x, m.z - origin.z);
            dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
        };
        let mut best = (0, 0, Fx::MIN, Fx::MAX);
        for r in 0..self.rows {
            for c in 0..self.cols {
                let t = self.ring_threat(c, r, rings, ty, brain, counts);
                if t > best.2 || (t == best.2 && dist2(c, r) < best.3) {
                    best = (c, r, t, dist2(c, r));
                }
            }
        }
        (self.center(best.0, best.1), best.2)
    }

    /// Cells around `p` (within `rings`) holding threat: (centre x, z, value),
    /// row-major.
    pub fn around(&self, p: FxVec3, rings: u32, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool) -> Vec<(Fx, Fx, Fx)> {
        let (c, r) = self.cell_of(p);
        self.ring(c, r, rings)
            .filter_map(|(c, r)| {
                let t = self.cell_threat(r * self.cols + c, ty, brain, counts);
                (t > Fx::ZERO).then(|| {
                    let m = self.center(c, r);
                    (m.x, m.z, t)
                })
            })
            .collect()
    }

    /// Threat summed over the distinct cells the segment `a -> b` crosses
    /// (sampled every half cell, so no cell is skipped).
    pub fn between(&self, a: FxVec3, b: FxVec3, ty: ThreatType, brain: u32, counts: &dyn Fn(u32) -> bool) -> Fx {
        let d = b - a;
        let len = d.length();
        let step = Fx::from_num(self.cell) / 2;
        let samples = (len / step).ceil().to_num::<u32>().max(1);
        let mut seen: Vec<u32> = Vec::new();
        let mut sum = Fx::ZERO;
        for i in 0..=samples {
            let p = a + d * (Fx::from_num(i) / Fx::from_num(samples));
            let (c, r) = self.cell_of(p);
            let index = r * self.cols + c;
            if !seen.contains(&index) {
                seen.push(index);
                sum = sum.saturating_add(self.cell_threat(index, ty, brain, counts));
            }
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::{Entity, EntityId};
    use crate::engine::sim::math::FxQuat;

    fn unit(w: &mut EntityWorld, army: u32, x: f64, z: f64, surface: i32, kinds: ThreatKinds) -> EntityId {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(army), FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        let t = unit_threat(Fx::ZERO, Fx::from_num(surface), Fx::ZERO, Fx::ZERO, kinds);
        w.get_mut(id).unwrap().unit.as_mut().unwrap().threat = t;
        id
    }

    fn exact(_: u32, _: &Entity) -> Knowledge {
        Knowledge::Exact
    }

    #[test]
    fn sizes_follow_the_map() {
        assert_eq!(cell_size(256, 256), 32);
        assert_eq!(cell_size(1024, 512), 64);
        assert_eq!(cell_size(4096, 4096), 256);
    }

    #[test]
    fn highest_and_rings_and_army_filter() {
        let mut w = EntityWorld::default();
        let land = ThreatKinds { land: true, ..Default::default() };
        let structure = ThreatKinds { structure: true, ..Default::default() };
        unit(&mut w, 2, 40.0, 40.0, 10, land); // cell (1,1)
        unit(&mut w, 2, 50.0, 45.0, 5, structure); // cell (1,1)
        unit(&mut w, 3, 200.0, 200.0, 12, land); // cell (6,6)
        unit(&mut w, 1, 210.0, 210.0, 99, land); // own army: never counted
        let mut map = ThreatMap::new(256, 256);
        map.update(&w, 0, &[], &exact);
        let enemies = |a: u32| a != 1;
        let (pos, t) = map.highest(0, ThreatType::Overall, 1, &enemies, Default::default());
        assert_eq!((pos, t), (FxVec3::from_f64(48.0, 0.0, 48.0), Fx::from_num(15)));
        let (pos, t) = map.highest(0, ThreatType::Land, 1, &enemies, Default::default());
        assert_eq!((pos, t), (FxVec3::from_f64(208.0, 0.0, 208.0), Fx::from_num(12)));
        assert_eq!(map.threat_at(FxVec3::from_f64(70.0, 0.0, 70.0), 0, ThreatType::Overall, 1, &enemies), Fx::ZERO);
        assert_eq!(map.threat_at(FxVec3::from_f64(70.0, 0.0, 70.0), 1, ThreatType::Overall, 1, &enemies), Fx::from_num(15));
        let only3 = |a: u32| a == 3;
        assert_eq!(map.highest(0, ThreatType::Overall, 1, &only3, Default::default()).1, Fx::from_num(12));
        assert_eq!(map.around(FxVec3::from_f64(40.0, 0.0, 40.0), 1, ThreatType::Structures, 1, &enemies), vec![(Fx::from_num(48), Fx::from_num(48), Fx::from_num(5))]);
        let line = map.between(FxVec3::from_f64(10.0, 0.0, 10.0), FxVec3::from_f64(250.0, 0.0, 250.0), ThreatType::Overall, 1, &enemies);
        assert_eq!(line, Fx::from_num(27));
    }

    #[test]
    fn brains_only_count_enemies_they_know() {
        let mut w = EntityWorld::default();
        let land = ThreatKinds { land: true, ..Default::default() };
        unit(&mut w, 2, 40.0, 40.0, 10, land);
        unit(&mut w, 2, 200.0, 200.0, 12, land);
        let mut map = ThreatMap::new(256, 256);
        // Brain 1 detects only units west of x = 100; brain 2 owns them all.
        let knows = |_: u32, e: &Entity| if e.position().x < Fx::from_num(100) { Knowledge::Detected } else { Knowledge::Unknown };
        map.update(&w, 0, &[1, 2], &knows);
        let enemies = |a: u32| a == 2;
        assert_eq!(map.highest(0, ThreatType::Overall, 1, &enemies, Default::default()).1, Fx::from_num(10), "the unknown 12 doesn't count");
        assert_eq!(map.threat_at(FxVec3::from_f64(200.0, 0.0, 200.0), 0, ThreatType::Overall, 1, &enemies), Fx::ZERO);
        let own = |a: u32| a == 2;
        assert_eq!(map.highest(0, ThreatType::Overall, 2, &own, Default::default()).1, Fx::from_num(12), "own threat is exact");
    }

    /// dev/probes probe_ai_threat / probe_ai_find: a mobile unit out of sight
    /// keeps its threat where it was last detected for 300 ticks after that
    /// update, then loses 2% of it per 30-tick update; a structure seen stays
    /// whole, also when destroyed out of sight; a unit that dies in sight is
    /// forgotten.
    #[test]
    fn lost_units_are_remembered_then_fade() {
        let mut w = EntityWorld::default();
        let land = ThreatKinds { land: true, ..Default::default() };
        let structure = ThreatKinds { structure: true, ..Default::default() };
        let acu = unit(&mut w, 2, 40.0, 40.0, 45, land);
        let pd = unit(&mut w, 2, 200.0, 40.0, 16, structure);
        let tank = unit(&mut w, 2, 40.0, 200.0, 2, land);
        let seen = std::cell::Cell::new(true);
        let knows = |_: u32, _: &Entity| if seen.get() { Knowledge::Detected } else { Knowledge::Unknown };
        let enemies = |a: u32| a == 2;
        let at = |m: &ThreatMap, x: f64, z: f64| m.threat_at(FxVec3::from_f64(x, 0.0, z), 0, ThreatType::Overall, 1, &enemies).to_num::<f64>();
        let mut map = ThreatMap::new(256, 256);
        map.update(&w, 60, &[1], &knows);
        assert_eq!(at(&map, 40.0, 40.0), 45.0);
        // Sight lost; the ACU walks off unseen, the point defence dies unseen
        // (after the update that last saw it: one destroyed before the next
        // update counts as dying in sight).
        seen.set(false);
        w.set_position(acu, FxVec3::from_f64(200.0, 0.0, 200.0));
        for now in (90..=330).step_by(30) {
            if now == 120 {
                w.get_mut(pd).unwrap().destroyed = true;
            }
            map.update(&w, now, &[1], &knows);
            assert_eq!(at(&map, 40.0, 40.0), 45.0, "held at {now}");
        }
        assert_eq!(at(&map, 200.0, 200.0), 0.0, "where it went is unknown");
        map.update(&w, 360, &[1], &knows);
        assert!((at(&map, 40.0, 40.0) - 44.1).abs() < 1e-6);
        assert_eq!(at(&map, 200.0, 40.0), 16.0, "a structure doesn't fade");
        map.update(&w, 390, &[1], &knows);
        assert!((at(&map, 40.0, 40.0) - 43.2).abs() < 1e-6);
        for now in (420..=330 + 50 * 30).step_by(30) {
            map.update(&w, now, &[1], &knows);
        }
        assert_eq!(at(&map, 40.0, 40.0), 0.0, "gone after 50 updates");
        assert_eq!(at(&map, 200.0, 40.0), 16.0);

        // Dies in sight: forgotten at the next update.
        seen.set(true);
        map.update(&w, 2000, &[1], &knows);
        assert_eq!(at(&map, 40.0, 200.0), 2.0);
        w.get_mut(tank).unwrap().destroyed = true;
        map.update(&w, 2030, &[1], &knows);
        assert_eq!(at(&map, 40.0, 200.0), 0.0);
    }

    /// dev/probes probe_ai_threat: `decay` is the fraction of the amount lost
    /// per update; an assignment counts (after its first step) from the next
    /// update; a second one in the cell adds and keeps the first's step;
    /// negative amounts stay and subtract from unit threat.
    #[test]
    fn assigned_threat_steps_per_update() {
        let mut w = EntityWorld::default();
        unit(&mut w, 2, 100.0, 200.0, 45, ThreatKinds::default());
        let mut map = ThreatMap::new(256, 256);
        map.update(&w, 0, &[], &exact);
        let p = FxVec3::from_f64(100.0, 0.0, 100.0);
        let q = FxVec3::from_f64(200.0, 0.0, 100.0);
        let r = FxVec3::from_f64(100.0, 0.0, 200.0);
        let none = |_: u32| false;
        let enemies = |a: u32| a == 2;
        // To the probe's two decimals (0.1 isn't exact in fixed point).
        let get = |m: &ThreatMap, p: FxVec3, ty: ThreatType| (m.threat_at(p, 0, ty, 1, &enemies).to_num::<f64>() * 100.0).round() / 100.0;
        map.assign(1, p, Fx::from_num(100), Fx::from_num(0.1), ThreatType::Land);
        map.assign(1, q, Fx::from_num(-50), Fx::from_num(0.1), ThreatType::Overall);
        map.assign(1, r, Fx::from_num(-20), Fx::from_num(0.1), ThreatType::AntiSurface);
        assert_eq!(get(&map, p, ThreatType::Land), 0.0, "not before the next update");
        map.update(&w, 30, &[], &exact);
        assert_eq!(get(&map, p, ThreatType::Land), 90.0);
        assert_eq!(get(&map, p, ThreatType::Overall), 90.0);
        assert_eq!(get(&map, p, ThreatType::OverallNotAssigned), 0.0);
        assert_eq!(map.threat_at(p, 0, ThreatType::Land, 2, &none), Fx::ZERO, "only the assigning brain sees it");
        assert_eq!(get(&map, q, ThreatType::Overall), -50.0);
        assert_eq!(get(&map, r, ThreatType::Overall), 25.0, "45 - 20");
        map.update(&w, 60, &[], &exact);
        assert_eq!(get(&map, p, ThreatType::Land), 80.0);
        map.assign(1, p, Fx::from_num(30), Fx::from_num(0.1), ThreatType::Land);
        map.update(&w, 90, &[], &exact);
        assert_eq!(get(&map, p, ThreatType::Land), 100.0, "80 + 30, less the first step of 10");
        for now in (120..=390).step_by(30) {
            map.update(&w, now, &[], &exact);
        }
        assert_eq!(get(&map, p, ThreatType::Land), 0.0);
        assert_eq!(get(&map, q, ThreatType::Overall), -50.0, "negative amounts never fade");
        map.assign(1, p, Fx::from_num(100), Fx::ONE, ThreatType::Land);
        map.update(&w, 420, &[], &exact);
        assert_eq!(get(&map, p, ThreatType::Land), 0.0, "decay 1: gone at the first update");
    }

    #[test]
    fn highest_ties_go_to_the_cell_nearest_the_start() {
        let mut w = EntityWorld::default();
        let land = ThreatKinds { land: true, ..Default::default() };
        let mut map = ThreatMap::new(1024, 1024);
        map.update(&w, 0, &[], &exact);
        let enemies = |a: u32| a == 2;
        let start = crate::engine::sim::math::FxVec2 { x: Fx::from_num(672.5), z: Fx::from_num(346.5) };
        assert_eq!(map.highest(0, ThreatType::Overall, 1, &enemies, start), (FxVec3::from_f64(672.0, 0.0, 352.0), Fx::ZERO));
        unit(&mut w, 2, 352.0, 880.0, 10, land); // cell (5, 13)
        map.update(&w, 30, &[], &exact);
        assert_eq!(map.highest(1, ThreatType::Overall, 1, &enemies, start), (FxVec3::from_f64(416.0, 0.0, 800.0), Fx::from_num(10)));
    }
}
