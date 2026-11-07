//! Ground pathfinding: passability grids per footprint class, windowed A*
//! and the sector graph (`hpa`).
//!
//! One cell per world unit. Units path by footprint class, as moho does:
//! FA's `/lua/footprints.lua` registers the classes (`SpecFootprints`: a
//! size, layer caps, slope and water-depth limits, and `IgnoreStructures`),
//! and each unit uses the class with its motion type's caps nearest its
//! blueprint footprint ([`NavGrid::class_for`]).
//!
//! Moho's rule (`OCCUPY_MobileCheck`, MohoEngine.dll 0x101532e0; the 1x1
//! form 0x101535d0) takes a footprint's heightmap corners: its WATER / SUB
//! layers need the shallowest corner at least `MinWaterDepth` under the
//! water, its LAND / SEABED layers the deepest at most `MaxWaterDepth`
//! under and no edge between neighbouring corners rising more than
//! `MaxSlope` (0: no limit); it can stand there if a layer is left. So a
//! cell (not a blocking terrain type) suits a class's ground layers, its
//! water layers, or both, by its own four corners; a class of size N is
//! open at a cell when the N x N block of cells around it (the cell at its
//! centre, odd sizes; just below and left of it, even) all suit its ground
//! layers or all suit its water layers (a hovercraft's block is all beach
//! or all sea, never half a cliff) and, unless the class ignores
//! structures, none is covered by a structure. Searches then treat a unit
//! as a point on its class's grid.
//! Classes are computed the first time a unit of that class appears
//! ([`NavGrid::ensure`]); every 1x1 class from the start.
//!
//! Search: short trips run 8-connected A* (no corner cutting, integer costs
//! 10/14, octile heuristic, ties broken by (f, h, cell index) so the result
//! is the same on every machine) in a window around start and goal, with
//! window-sized, generation-stamped buffers reused per thread. Longer ones
//! search the class's sector graph (`hpa`) and refine it leg by leg. The
//! cell path is shortened to waypoints by line-of-sight smoothing. If the
//! goal can't be reached the path ends at the reachable cell nearest it.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::engine::sim::math::{Fx, FxVec3};
use crate::engine::sim::terrain::Terrain;

/// Layer caps (`footprints.lua`).
pub mod caps {
    pub const LAND: u8 = 0x01;
    pub const SEABED: u8 = 0x02;
    pub const SUB: u8 = 0x04;
    pub const WATER: u8 = 0x08;
}

/// Layer caps a blueprint `Physics.MotionType` paths with (`None`: flies or
/// doesn't move).
pub fn caps_of_motion_type(motion_type: &str) -> Option<u8> {
    use caps::*;
    Some(match motion_type {
        // Biped moves as Land (the footprint caps, `blueprints.rs`).
        "RULEUMT_Land" | "RULEUMT_Biped" => LAND,
        "RULEUMT_Amphibious" => LAND | SEABED,
        "RULEUMT_Hover" | "RULEUMT_AmphibiousFloating" => LAND | WATER,
        "RULEUMT_Water" => WATER,
        "RULEUMT_SurfacingSub" => SUB | WATER,
        _ => return None,
    })
}

/// A footprint class (`SpecFootprints` entry).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassSpec {
    pub name: String,
    /// Square side, cells (FA's specs are all square: `max(SizeX, SizeZ)`).
    pub size: u32,
    pub caps: u8,
    pub ignore_structures: bool,
    pub max_slope: Option<Fx>,
    pub min_water_depth: Option<Fx>,
    pub max_water_depth: Option<Fx>,
}

impl ClassSpec {
    /// Whether ground with these corner stats suits the class's ground
    /// layers (LAND / SEABED) and its water layers (WATER / SUB): `step` the
    /// largest rise between neighbouring heightmap corners, `shallowest` /
    /// `deepest` the water depth at the highest / lowest corner (negative
    /// above the water). Moho's `OCCUPY_MobileCheck` (see the module doc).
    pub fn suits(&self, step: Fx, shallowest: Fx, deepest: Fx) -> (bool, bool) {
        let ground = self.caps & (caps::LAND | caps::SEABED) != 0
            && self.max_slope.is_none_or(|s| s == Fx::ZERO || step <= s)
            && deepest <= self.max_water_depth.unwrap_or(if self.caps & caps::SEABED != 0 { Fx::from_num(25) } else { Fx::ZERO });
        let water = self.caps & (caps::WATER | caps::SUB) != 0 && shallowest >= self.min_water_depth.unwrap_or(Fx::ZERO);
        (ground, water)
    }
}

/// Moho's `OCCUPY_MobileCheck` (MohoEngine.dll 0x101532e0) for one
/// footprint: the layers of `spec.caps` a footprint of `size_x` x `size_z`
/// cells from cell (`x0`, `z0`) can stand on, judged by its
/// (size_x + 1) x (size_z + 1) heightmap corners; layers outside LAND /
/// SEABED / WATER / SUB (air) always stay. No occupancy (structures) and no
/// terrain types: the build-placement form (`OCCUPY_Check`'s mobile branch,
/// 0x10153ab0).
pub fn mobile_layers(t: &Terrain, spec: &ClassSpec, x0: i64, z0: i64, size_x: u32, size_z: u32) -> u8 {
    let corner = |x: i64, z: i64| t.height_at(Fx::from_num(x), Fx::from_num(z));
    let (mut step, mut lo, mut hi) = (Fx::ZERO, Fx::MAX, Fx::MIN);
    for dz in 0..=size_z as i64 {
        for dx in 0..=size_x as i64 {
            let h = corner(x0 + dx, z0 + dz);
            lo = lo.min(h);
            hi = hi.max(h);
            if dx > 0 {
                step = step.max((h - corner(x0 + dx - 1, z0 + dz)).abs());
            }
            if dz > 0 {
                step = step.max((h - corner(x0 + dx, z0 + dz - 1)).abs());
            }
        }
    }
    let water = t.water_surface.unwrap_or(Fx::from_num(-10000));
    let (ground_ok, water_ok) = spec.suits(step, water - hi, water - lo);
    let mut layers = spec.caps;
    if !ground_ok {
        layers &= !(caps::LAND | caps::SEABED);
    }
    if !water_ok {
        layers &= !(caps::WATER | caps::SUB);
    }
    layers
}

/// FA's `/lua/footprints.lua` table, used when no `SpecFootprints` ran
/// (tests, benches).
pub fn default_classes() -> Vec<ClassSpec> {
    use caps::*;
    let c = |name: &str, size: u32, caps: u8, ignore: bool, slope: Option<f64>, min_d: Option<f64>, max_d: Option<f64>| ClassSpec {
        name: name.into(),
        size,
        caps,
        ignore_structures: ignore,
        max_slope: slope.map(Fx::from_num),
        min_water_depth: min_d.map(Fx::from_num),
        max_water_depth: max_d.map(Fx::from_num),
    };
    vec![
        c("Vehicle1x1", 1, LAND, false, Some(0.75), None, Some(0.05)),
        c("Vehicle2x2", 2, LAND, false, Some(0.75), None, Some(0.05)),
        c("Vehicle5x5", 5, LAND, true, Some(0.75), None, Some(0.05)),
        c("Amphibious1x1", 1, LAND | SEABED, false, Some(0.75), None, Some(25.0)),
        c("Amphibious3x3", 3, LAND | SEABED, true, Some(0.75), None, Some(25.0)),
        c("Amphibious6x6", 6, LAND | SEABED, true, Some(0.75), None, Some(25.0)),
        c("WaterLand1x1", 1, LAND | WATER, false, Some(0.75), Some(0.1), Some(1.0)),
        c("WaterLand2x2", 2, LAND | WATER, false, Some(0.75), Some(0.1), Some(1.0)),
        c("WaterLand3x3", 3, LAND | WATER, false, Some(0.75), Some(0.0), Some(5.0)),
        c("WaterLand5x5", 5, LAND | WATER, false, Some(0.75), Some(0.0), Some(5.0)),
        c("SurfacingSub2x2", 2, SUB | WATER, false, None, Some(1.5), None),
        c("SurfacingSub3x3", 3, SUB | WATER, false, None, Some(1.5), None),
        c("SurfacingSub4x4", 4, SUB | WATER, false, None, Some(1.5), None),
        c("SurfacingSub12x12", 12, SUB | WATER, false, None, Some(1.5), None),
        c("Water1x1", 1, WATER, false, None, Some(1.5), None),
        c("Water3x3", 3, WATER, false, None, Some(0.25), None),
        c("Water4x4", 4, WATER, false, None, Some(1.5), None),
        c("Water6x6", 6, WATER, false, None, Some(1.5), None),
        c("Water8x8", 8, WATER, false, None, Some(1.5), None),
        c("Water11x11", 11, WATER, false, None, Some(1.5), None),
    ]
}

/// Refits smaller than this many cells run serially.
const FIT_PARALLEL_MIN: u64 = 64 * 1024;

/// Most classes a grid holds (a bit each per cell).
pub const MAX_CLASSES: usize = 32;

/// A footprint class of the grid (index into its class list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct NavClass(pub u8);

impl NavClass {
    fn bit(self) -> u32 {
        1 << self.0
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct NavGrid {
    pub width: u32,
    pub height: u32,
    classes: Vec<ClassSpec>,
    /// Per cell, two bits per class: the cell's terrain suits the class's
    /// ground layers (bit `c`) or its water layers (bit `32 + c`).
    suits: Vec<u64>,
    /// Cells covered by structures (count, so overlaps undo cleanly).
    blocked: Vec<u16>,
    /// Per class (once in `ready`), a bit per cell, rows of `stride` words:
    /// a unit of the class can stand here (its whole footprint suits it and
    /// is free of structures). Bit-packed per class so a search streams
    /// through 1 bit per cell (2 MB at 4096^2), not every class's bits.
    open: Vec<Vec<u64>>,
    stride: usize,
    ready: u32,
    /// Bumped whenever passability changes (paths planned on an older grid
    /// are re-planned).
    pub version: u32,
    /// Per class (once in `ready`), its sector graph: long searches and
    /// connectivity (`hpa`). Rebuilt where the grid changed by
    /// [`NavGrid::refresh`].
    hier: Vec<Option<crate::engine::sim::hpa::Hier>>,
}

/// Cells from which [`NavGrid::recompute`] splits rows across threads
/// (~50 ns a cell serially, against rayon's ~100 µs wake-up).
const RECOMPUTE_PARALLEL_CELLS: u32 = 4096;

impl NavGrid {
    /// Builds the grids from the terrain with FA's default classes;
    /// `blocking[code]` = terrain type `code` blocks movement.
    pub fn build(t: &Terrain, blocking: &[bool; 256]) -> Self {
        Self::build_with(t, blocking, default_classes())
    }

    /// Builds the grids for `classes` (at most [`MAX_CLASSES`]; extras are
    /// dropped). Every 1x1 class is computed now, the rest on first use.
    pub fn build_with(t: &Terrain, blocking: &[bool; 256], mut classes: Vec<ClassSpec>) -> Self {
        if classes.len() > MAX_CLASSES {
            tracing::warn!(count = classes.len(), "more footprint classes than the grid holds; extras ignored");
            classes.truncate(MAX_CLASSES);
        }
        let (w, h) = (t.width, t.height);
        let n = (w * h) as usize;
        let mut grid = Self {
            width: w,
            height: h,
            hier: vec![None; classes.len()],
            classes,
            suits: vec![0; n],
            blocked: vec![0; n],
            open: Vec::new(),
            stride: (w as usize).div_ceil(64),
            ready: 0,
            version: 0,
        };
        grid.open = vec![Vec::new(); grid.classes.len()];
        grid.recompute(t, blocking, 0, 0, w, h);
        for c in 0..grid.classes.len() {
            if grid.classes[c].size == 1 {
                grid.ensure(NavClass(c as u8));
            }
        }
        grid
    }

    pub fn classes(&self) -> &[ClassSpec] {
        &self.classes
    }

    pub fn class_named(&self, name: &str) -> Option<NavClass> {
        self.classes.iter().position(|c| c.name == name).map(|i| NavClass(i as u8))
    }

    /// The class a unit with layer `caps` and a blueprint footprint of
    /// `size_x` x `size_z` cells paths as: of the classes with exactly those
    /// caps, the one whose larger side difference is smallest, the first
    /// listed on a tie (MohoEngine.dll `RRuleGameRulesImpl::FindFootprint`
    /// 0x10117ea0; a 2 x 6 frigate is `Water4x4`). Moho then gives the
    /// blueprint that class's footprint (see `blueprints.rs`).
    pub fn class_for(&self, caps: u8, size_x: u32, size_z: u32) -> Option<NavClass> {
        let off = |c: &ClassSpec| c.size.abs_diff(size_x).max(c.size.abs_diff(size_z));
        let pick = self.classes.iter().enumerate().filter(|(_, c)| c.caps == caps).min_by_key(|(i, c)| (off(c), *i));
        pick.map(|(i, _)| NavClass(i as u8))
    }

    /// Computes `class`'s grid if it isn't yet (a unit of it appeared).
    /// Doesn't change paths of other classes, so leaves `version` alone.
    pub fn ensure(&mut self, class: NavClass) {
        if self.ready & class.bit() != 0 || class.0 as usize >= self.classes.len() {
            return;
        }
        let _s = crate::prof_span!("nav ensure class", class = %self.classes[class.0 as usize].name);
        self.ready |= class.bit();
        self.open[class.0 as usize] = vec![0; self.stride * self.height as usize];
        self.fit(class, 0, 0, self.width, self.height);
    }

    /// Recomputes terrain passability for cells in `[x0, x1) x [z0, z1)`
    /// (after the heightmap changed there, e.g. `FlattenMapRect`).
    pub fn recompute(&mut self, t: &Terrain, blocking: &[bool; 256], x0: u32, z0: u32, x1: u32, z1: u32) {
        use rayon::prelude::*;
        let (x1, z1) = (x1.min(self.width), z1.min(self.height));
        if x0 >= x1 || z0 >= z1 {
            return;
        }
        let corner = |x: u32, z: u32| t.height_at(Fx::from_num(x), Fx::from_num(z));
        // Moho's water level without water: -10000 (0x1076c620).
        let water = t.water_surface.unwrap_or(Fx::from_num(-10000));
        let classes = &self.classes;
        // Each cell depends only on the terrain: compute rows in parallel
        // (big rects only: a structure's flatten is a few dozen cells, where
        // rayon's ~100 µs wake-up was 60% of spawning a 63-army game's
        // bases), then write them back in order.
        let row = |z: u32| -> Vec<u64> {
                (x0..x1)
                    .map(|x| {
                        let (h00, h10, h01, h11) = (corner(x, z), corner(x + 1, z), corner(x, z + 1), corner(x + 1, z + 1));
                        let step = (h00 - h10).abs().max((h00 - h01).abs()).max((h10 - h11).abs()).max((h01 - h11).abs());
                        // Depths at the highest and the lowest corner (negative above the water).
                        let shallowest = water - h00.max(h10).max(h01).max(h11);
                        let deepest = water - h00.min(h10).min(h01).min(h11);
                        if blocking[t.type_at(Fx::from_num(x), Fx::from_num(z)) as usize] {
                            return 0;
                        }
                        let mut bits = 0u64;
                        for (i, c) in classes.iter().enumerate() {
                            let (ground, water) = c.suits(step, shallowest, deepest);
                            bits |= (ground as u64) << i | (water as u64) << (32 + i);
                        }
                        bits
                    })
                    .collect()
        };
        let rows: Vec<Vec<u64>> = if (x1 - x0) * (z1 - z0) >= RECOMPUTE_PARALLEL_CELLS {
            (z0..z1).into_par_iter().map(row).collect()
        } else {
            (z0..z1).map(row).collect()
        };
        for (dz, row) in rows.into_iter().enumerate() {
            let start = ((z0 + dz as u32) * self.width + x0) as usize;
            self.suits[start..start + row.len()].copy_from_slice(&row);
        }
        self.refit(x0, z0, x1, z1, true);
        self.version = self.version.wrapping_add(1);
    }

    /// Refits the computed classes whose open cells can see a change in
    /// cells `[x0, x1) x [z0, z1)` (terrain: every class; structures: those
    /// that don't ignore them).
    fn refit(&mut self, x0: u32, z0: u32, x1: u32, z1: u32, terrain: bool) {
        for c in 0..self.classes.len() {
            let class = NavClass(c as u8);
            let spec = &self.classes[c];
            if self.ready & class.bit() == 0 || (!terrain && spec.ignore_structures) {
                continue;
            }
            // Anchors whose footprint overlaps the rect.
            let (lo, n) = ((spec.size - 1) / 2, spec.size);
            let hi = n - 1 - lo;
            self.fit(class, x0.saturating_sub(hi), z0.saturating_sub(hi), (x1 + lo).min(self.width), (z1 + lo).min(self.height));
        }
    }

    /// Sets `class`'s open bits for anchors in `[x0, x1) x [z0, z1)`: the
    /// N x N block `[x - lo, x - lo + N)` (lo = (N - 1) / 2) all suits its
    /// ground layers or all suits its water layers (and is free of
    /// structures unless it ignores them). Two passes: runs along each row,
    /// then down the columns, for both at once.
    fn fit(&mut self, class: NavClass, x0: u32, z0: u32, x1: u32, z1: u32) {
        use rayon::prelude::*;
        if x0 >= x1 || z0 >= z1 {
            return;
        }
        let _s = crate::prof_span!("nav fit", cells = (x1 - x0) * (z1 - z0));
        let spec = &self.classes[class.0 as usize];
        let (n, lo) = (spec.size.max(1) as i64, (spec.size.max(1) as i64 - 1) / 2);
        let ignore = spec.ignore_structures;
        let (w, h) = (self.width as i64, self.height as i64);
        let (ground_bit, water_bit) = (class.bit() as u64, (class.bit() as u64) << 32);
        let (suits, blocked) = (&self.suits, &self.blocked);
        // (Suits the ground layers, suits the water layers.)
        let cell_ok = |x: i64, z: i64| -> (bool, bool) {
            if x < 0 || z < 0 || x >= w || z >= h {
                return (false, false);
            }
            let i = (z * w + x) as usize;
            let free = ignore || blocked[i] == 0;
            (free && suits[i] & ground_bit != 0, free && suits[i] & water_bit != 0)
        };
        // Pass 1: rows `z0 - lo .. z1 - lo + n - 1`; each anchor column's
        // run of n cells along the row all suit.
        let (rz0, rz1) = (z0 as i64 - lo, z1 as i64 - lo + n - 1);
        // Small refits (a structure placed) run serially: a rayon wake-up
        // (~100 us) would cost more than the work.
        let parallel = (x1 - x0) as u64 * (z1 - z0) as u64 >= FIT_PARALLEL_MIN;
        let row_runs = |z: i64| {
                let mut out = Vec::with_capacity((x1 - x0) as usize);
                let (mut ground, mut water) = (0i64, 0i64);
                let mut step = |x: i64| {
                    let (g, wt) = cell_ok(x, z);
                    ground = if g { ground + 1 } else { 0 };
                    water = if wt { water + 1 } else { 0 };
                    (ground >= n, water >= n)
                };
                // Prime the runs with the cells left of the first window end.
                let start = x0 as i64 - lo;
                for x in start..start + n - 1 {
                    step(x);
                }
                for ax in x0 as i64..x1 as i64 {
                    out.push(step(ax - lo + n - 1));
                }
                out
            };
        let rows: Vec<Vec<(bool, bool)>> = if parallel { (rz0..rz1).into_par_iter().map(row_runs).collect() } else { (rz0..rz1).map(row_runs).collect() };
        // Pass 2: an anchor is open when its n rows' runs all are.
        let anchors = |dz: usize| -> Vec<bool> {
            (0..(x1 - x0) as usize).map(|dx| (0..n as usize).all(|k| rows[dz + k][dx].0) || (0..n as usize).all(|k| rows[dz + k][dx].1)).collect()
        };
        let open_rows: Vec<Vec<bool>> =
            if parallel { (0..(z1 - z0) as usize).into_par_iter().map(anchors).collect() } else { (0..(z1 - z0) as usize).map(anchors).collect() };
        if let Some(h) = self.hier[class.0 as usize].as_mut() {
            h.mark(crate::engine::sim::hpa::Rect { x0, z0, x1, z1 });
        }
        let (stride, bits) = (self.stride, &mut self.open[class.0 as usize]);
        for (dz, row) in open_rows.into_iter().enumerate() {
            let base = (z0 as usize + dz) * stride;
            for (dx, open) in row.into_iter().enumerate() {
                let x = x0 as usize + dx;
                let (word, b) = (&mut bits[base + x / 64], 1u64 << (x % 64));
                *word = if open { *word | b } else { *word & !b };
            }
        }
    }

    /// Brings every computed class's sector graph up to date with the grid
    /// (only sectors that changed are rebuilt; a new class builds all of
    /// its own). Call before searching; the search itself only reads.
    pub fn refresh(&mut self) {
        for c in 0..self.classes.len() {
            let class = NavClass(c as u8);
            if self.ready & class.bit() == 0 {
                continue;
            }
            let mut h = self.hier[c].take().unwrap_or_else(|| crate::engine::sim::hpa::Hier::new(self.width, self.height));
            // Spanned only when there is work (this runs every tick).
            let _s = (!h.is_fresh()).then(|| crate::prof_span!("nav refresh class", class = %self.classes[c].name));
            h.refresh(&self.open_at(class));
            self.hier[c] = Some(h);
        }
    }

    /// `class`'s sector graph, if it's up to date.
    fn hier(&self, class: NavClass) -> Option<&crate::engine::sim::hpa::Hier> {
        self.hier.get(class.0 as usize)?.as_ref().filter(|h| h.is_fresh())
    }

    /// Whether a unit of `class` standing at `from` can reach the goal at
    /// `to`: its footprint there is open and in the unit's connected region
    /// (moho's `CanPathTo`, MohoEngine.dll 0x102a99e8..0x102a9a74: the
    /// occupancy check at the goal, then the navigator on the footprint's
    /// grid rect; its navigator gives up on some far goals, ours doesn't).
    /// Brings the class's graph up to date first.
    pub fn reachable(&mut self, class: NavClass, from: FxVec3, to: FxVec3) -> bool {
        self.ensure(class);
        self.refresh();
        let Some(start) = nearest_where(self, self.cell_for(class, from), SNAP_RADIUS, |x, z| self.passable(class, x, z)) else { return false };
        let region = self.region(class, start);
        if region == 0 {
            return false;
        }
        // The unit's footprint at the goal must fit there (moho checks it
        // can stand there first, `OCCUPY_MobileCheck` through 0x102f42c0,
        // then asks its pathfinder about the footprint's rect).
        let goal = self.cell_for(class, to);
        self.passable(class, goal.0, goal.1) && self.region(class, goal) == region
    }

    /// `class`'s sector graph, if it's up to date (flow fields).
    pub(crate) fn hier_of(&self, class: NavClass) -> Option<&crate::engine::sim::hpa::Hier> {
        self.hier(class)
    }

    /// Passability test for `class` (flow fields).
    pub(crate) fn open_fn(&self, class: NavClass) -> impl Fn(u32, u32) -> bool + Sync + '_ {
        self.open_at(class)
    }

    /// The cell holding world point `p` (clamped to the grid).
    pub fn cell_of(&self, p: FxVec3) -> (u32, u32) {
        self.cell(p)
    }

    /// Connected component of cell `c` for `class` (0 = not open, or the
    /// graph isn't up to date).
    fn region(&self, class: NavClass, (x, z): (u32, u32)) -> u32 {
        self.hier(class).map_or(0, |h| h.component(x, z))
    }

    fn cells_of(&self, x0: Fx, z0: Fx, x1: Fx, z1: Fx) -> (u32, u32, u32, u32) {
        let c = |v: Fx, max: u32| v.max(Fx::ZERO).floor().to_num::<u32>().min(max);
        (c(x0, self.width), c(z0, self.height), c(x1.ceil(), self.width), c(z1.ceil(), self.height))
    }

    /// Marks (or unmarks) the cells under a structure footprint.
    pub fn block_rect(&mut self, x0: Fx, z0: Fx, x1: Fx, z1: Fx, add: bool) {
        let (cx0, cz0, cx1, cz1) = self.cells_of(x0, z0, x1, z1);
        for z in cz0..cz1 {
            for x in cx0..cx1 {
                let b = &mut self.blocked[(z * self.width + x) as usize];
                *b = if add { b.saturating_add(1) } else { b.saturating_sub(1) };
            }
        }
        self.refit(cx0, cz0, cx1, cz1, false);
        self.version = self.version.wrapping_add(1);
    }

    pub fn passable(&self, class: NavClass, x: u32, z: u32) -> bool {
        if x >= self.width || z >= self.height {
            return false;
        }
        self.open_at(class)(x, z)
    }

    /// A structure covers the cell (off the grid counts as covered).
    pub fn blocked_cell(&self, x: u32, z: u32) -> bool {
        x >= self.width || z >= self.height || self.blocked[(z * self.width + x) as usize] > 0
    }

    /// The cell's terrain suits `class`'s layer (slope, water depth, terrain
    /// type), ignoring structures and the class's size.
    pub fn terrain_suits(&self, class: NavClass, x: u32, z: u32) -> bool {
        x < self.width && z < self.height && self.suits[(z * self.width + x) as usize] & (class.bit() as u64 | (class.bit() as u64) << 32) != 0
    }

    /// A fast passability test for one class (bounds-checked; false for a
    /// class not computed yet), for searches.
    fn open_at(&self, class: NavClass) -> impl Fn(u32, u32) -> bool + Sync + '_ {
        let (w, h, stride) = (self.width, self.height, self.stride);
        let bits: &[u64] = self.open.get(class.0 as usize).map_or(&[], |v| v.as_slice());
        move |x: u32, z: u32| x < w && z < h && bits.get(z as usize * stride + x as usize / 64).is_some_and(|word| word >> (x % 64) & 1 != 0)
    }

    /// Is the polyline from `from` through `points` still walkable?
    pub fn path_clear(&self, layer: NavClass, from: FxVec3, points: &[FxVec3]) -> bool {
        let mut a = self.cell_for(layer, from);
        for &p in points {
            let b = self.cell_for(layer, p);
            if !self.line_clear(layer, a, b) {
                return false;
            }
            a = b;
        }
        true
    }

    /// The cell `class`'s open bit for a unit at `p` sits on. Moho puts a
    /// footprint of N cells at `p` on cells from round(p - N / 2) (x87
    /// rounding, halves to even; `COORDS_ToGridRect`, MohoEngine.dll
    /// 0x100f8660); our bit for that block is at its anchor (`fit`: the
    /// block's first cell + (N - 1) / 2). For odd sizes that's the cell
    /// `p` is in, for even ones the cell below-left of the corner nearest it.
    pub fn cell_for(&self, class: NavClass, p: FxVec3) -> (u32, u32) {
        let n = self.classes.get(class.0 as usize).map_or(1, |c| c.size.max(1)) as i64;
        let lo = (n - 1) / 2;
        let half = Fx::from_num(n) / 2;
        let axis = |v: Fx, max: u32| -> u32 {
            let start = round_half_even(v - half) + lo;
            start.clamp(0, max as i64 - 1) as u32
        };
        (axis(p.x, self.width), axis(p.z, self.height))
    }

    /// Where a unit of `class` on cell `c` stands (the inverse of
    /// [`NavGrid::cell_for`]): its footprint's centre, the cell's centre
    /// for odd sizes and its upper corner for even ones.
    pub fn point_of(&self, class: NavClass, (x, z): (u32, u32)) -> (Fx, Fx) {
        let n = self.classes.get(class.0 as usize).map_or(1, |c| c.size.max(1));
        let off = Fx::from_num(n) / 2 - Fx::from_num((n - 1) / 2);
        (Fx::from_num(x) + off, Fx::from_num(z) + off)
    }

    /// `class` is open for a unit at `p` (its footprint there fits).
    pub fn passable_at(&self, class: NavClass, p: FxVec3) -> bool {
        p.x >= Fx::ZERO && p.z >= Fx::ZERO && {
            let (x, z) = self.cell_for(class, p);
            self.passable(class, x, z)
        }
    }

    fn cell(&self, p: FxVec3) -> (u32, u32) {
        let c = |v: Fx, max: u32| v.max(Fx::ZERO).floor().to_num::<u32>().min(max.saturating_sub(1));
        (c(p.x, self.width), c(p.z, self.height))
    }

    /// Every cell the straight segment between two cell centres touches is
    /// passable (supercover walk, so diagonals can't slip between blocks).
    fn line_clear(&self, layer: NavClass, a: (u32, u32), b: (u32, u32)) -> bool {
        self.line_walk(layer, a, b).is_ok()
    }

    /// Walks the segment `a` -> `b` (as [`NavGrid::line_clear`]; `a` itself
    /// isn't tested, it's where the walker stands): `Ok` if it's all
    /// passable, else `Err` with the last cell before the first blocked one
    /// (`None` if that's `a`'s first step).
    fn line_walk(&self, layer: NavClass, (ax, az): (u32, u32), (bx, bz): (u32, u32)) -> Result<(), Option<(u32, u32)>> {
        let (mut x, mut z) = (ax as i64, az as i64);
        let (dx, dz) = ((bx as i64 - x).abs(), (bz as i64 - z).abs());
        let (sx, sz) = (if bx as i64 > x { 1 } else { -1 }, if bz as i64 > z { 1 } else { -1 });
        let mut err = dx - dz;
        let (mut nx, mut nz) = (0, 0);
        let mut last = None;
        while nx < dx || nz < dz {
            // The start cell is where the unit already is: it doesn't block.
            if (nx, nz) != (0, 0) && !self.passable(layer, x as u32, z as u32) {
                return Err(last);
            }
            last = Some((x as u32, z as u32));
            let e2 = 2 * err;
            if e2 > -dz && e2 < dx {
                // Diagonal step: both orthogonal neighbours must be open.
                if !self.passable(layer, (x + sx) as u32, z as u32) || !self.passable(layer, x as u32, (z + sz) as u32) {
                    return Err(last);
                }
                err += -dz + dx;
                x += sx;
                z += sz;
                nx += 1;
                nz += 1;
            } else if e2 > -dz {
                err -= dz;
                x += sx;
                nx += 1;
            } else {
                err += dx;
                z += sz;
                nz += 1;
            }
        }
        if (dx, dz) == (0, 0) || self.passable(layer, x as u32, z as u32) { Ok(()) } else { Err(last) }
    }

    /// `to`, or if the straight way from `from` is blocked, the centre of
    /// the last passable cell along it (`from` if that's blocked itself).
    /// Final formation slots `(class, point)` round `target` that land where
    /// their unit can't stand (a cliff, water, a structure, ground cut off
    /// from the target) or on another slot move to the nearest free cell of
    /// the target's region, searched from the last open point on the line
    /// from the target: the block folds in at its edges instead of
    /// scattering. Each slot holds a `spacing`-wide block of cells (at least
    /// its footprint), so moved slots keep the layout's spacing. Slots are
    /// kept in order (call with them sorted); good ones keep their place.
    /// Returns how many moved.
    pub fn fit_slots(&self, target: FxVec3, spacing: u32, slots: &mut [(NavClass, FxVec3)]) -> usize {
        // Each unit holds a size x size block of cells round its own.
        let block = |(x, z): (u32, u32), size: u32| {
            let lo = |v: u32| v.saturating_sub((size - 1) / 2);
            let (x0, z0) = (lo(x), lo(z));
            (z0..z0 + size).flat_map(move |bz| (x0..x0 + size).map(move |bx| (bx, bz)))
        };
        let size = |c: NavClass| self.classes().get(c.0 as usize).map_or(1, |s| s.size).max(spacing).max(1);
        // The target's region per class (its nearest open cell's), and where
        // lines to edge slots start.
        let mut regions: std::collections::BTreeMap<u8, Option<((u32, u32), u32)>> = Default::default();
        let mut start_of = |c: NavClass| *regions.entry(c.0).or_insert_with(|| nearest_where(self, self.cell_for(c, target), SNAP_RADIUS, |x, z| self.passable(c, x, z)).map(|o| (o, self.region(c, o))));
        let mut taken: std::collections::BTreeSet<(u32, u32)> = Default::default();
        let fits = |taken: &std::collections::BTreeSet<(u32, u32)>, c: NavClass, cell: (u32, u32), region: u32| {
            self.passable(c, cell.0, cell.1) && (region == 0 || self.region(c, cell) == region) && block(cell, size(c)).all(|b| !taken.contains(&b))
        };
        let mut bad = Vec::new();
        for (i, (c, p)) in slots.iter().enumerate() {
            let Some((_, region)) = start_of(*c) else { continue };
            let cell = self.cell_for(*c, *p);
            if fits(&taken, *c, cell, region) {
                taken.extend(block(cell, size(*c)));
            } else {
                bad.push(i);
            }
        }
        let mut moved = 0;
        for i in bad {
            let (c, p) = slots[i];
            let Some((open, region)) = start_of(c) else { continue };
            let (fx, fz) = self.point_of(c, open);
            let from = FxVec3 { x: fx, y: target.y, z: fz };
            let start = self.cell_for(c, self.clamp_along(c, from, p));
            if let Some(cell) = nearest_where(self, start, FIT_RADIUS, |x, z| fits(&taken, c, (x, z), region)) {
                taken.extend(block(cell, size(c)));
                let (sx, sz) = self.point_of(c, cell);
                slots[i].1 = FxVec3 { x: sx, y: p.y, z: sz };
                moved += 1;
            }
        }
        moved
    }

    pub fn clamp_along(&self, layer: NavClass, from: FxVec3, to: FxVec3) -> FxVec3 {
        match self.line_walk(layer, self.cell_for(layer, from), self.cell_for(layer, to)) {
            Ok(()) => to,
            Err(Some(c)) => {
                let (x, z) = self.point_of(layer, c);
                FxVec3 { x, y: to.y, z }
            }
            Err(None) => from,
        }
    }
}

/// `v` rounded to the nearest integer, halves to even (x87 `fistp` in its
/// default mode, as moho rounds positions to grid cells).
pub(crate) fn round_half_even(v: Fx) -> i64 {
    let floor = v.floor();
    let below = floor.to_num::<i64>();
    match (v - floor).cmp(&(Fx::ONE / 2)) {
        std::cmp::Ordering::Less => below,
        std::cmp::Ordering::Greater => below + 1,
        std::cmp::Ordering::Equal => below + (below & 1),
    }
}

/// Reusable A* buffers (one per thread).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Search {
    generation: u32,
    /// Per cell: generation it was touched in, g cost, parent cell, closed.
    stamp: Vec<u32>,
    g: Vec<u32>,
    parent: Vec<u32>,
    closed: Vec<bool>,
    open: BinaryHeap<(Reverse<u32>, Reverse<u32>, Reverse<u32>)>,
    /// Nodes expanded by the last search (for benches / tuning).
    pub expanded: u32,
    /// Cells the last `find_path` looked at (8 per A* / Dijkstra / graph
    /// expansion, plus the cells of each line-of-sight test). Tracks search
    /// time, and unlike it is deterministic: the per-tick path budget is
    /// counted in it (`movement::PathQueue`).
    pub work: u64,
    hier: crate::engine::sim::hpa::Scratch,
}

impl Search {
    fn prepare(&mut self, cells: usize) {
        // Window-sized: grows to the largest window searched, never to the
        // whole map unless a search needs it.
        if self.stamp.len() < cells {
            self.stamp = vec![0; cells];
            self.g = vec![0; cells];
            self.parent = vec![0; cells];
            self.closed = vec![false; cells];
            self.generation = 0;
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.generation = 1;
        }
        self.open.clear();
        self.expanded = 0;
    }

    fn touch(&mut self, i: usize) {
        if self.stamp[i] != self.generation {
            self.stamp[i] = self.generation;
            self.g[i] = u32::MAX;
            self.closed[i] = false;
        }
    }
}

use crate::engine::sim::hpa::{octile, Rect};

/// Trips up to this octile cost (10 per cell) try windowed A* first.
pub const SHORT_TRIP: u32 = 1000;
/// Expansions windowed A* gets before a trip goes to the sector graph.
pub const SHORT_BUDGET: u32 = 4096;
/// Margin (cells) round start and goal of a windowed A*.
const WINDOW_MARGIN: u32 = 32;

/// Waypoints (world positions where the class stands on each cell,
/// [`NavGrid::point_of`]; the last one is `goal` itself
/// when reachable) from `start` toward `goal` for `class`. `max_expand`
/// bounds the search. Short trips run A* in a window round start and goal;
/// the rest (or a short trip that has to leave its window) search the
/// class's sector graph. Without an up-to-date graph (`refresh` not run)
/// it falls back to A* over the whole map.
pub fn find_path(grid: &NavGrid, search: &mut Search, layer: NavClass, start: FxVec3, goal: FxVec3, max_expand: u32) -> Vec<FxVec3> {
    search.work = 0;
    let s = grid.cell_for(layer, start);
    let mut g = grid.cell_for(layer, goal);
    if s == g {
        return vec![goal];
    }
    // A goal on a blocked cell (a structure's footprint for a build or
    // guard order, a cliff) can never be reached, and searching for it
    // explores the whole reachable map before giving up (~150 ms on a 1024
    // map). Head for the nearest open cell instead; the path then ends
    // there, short of the goal.
    let mut goal = Some(goal);
    if !grid.passable(layer, g.0, g.1) {
        match nearest_open(grid, layer, g, SNAP_RADIUS) {
            Some(c) if c == s => return vec![start],
            Some(c) => {
                g = c;
                goal = None;
            }
            None => {}
        }
    }
    let mut goal_point = goal;
    // A goal in another component than the start can't be reached: head
    // for the start component's cell nearest the goal instead, so the
    // search below succeeds rather than exploring everything reachable.
    let from = match grid.region(layer, s) {
        0 => nearest_open(grid, layer, s, 2).map(|c| grid.region(layer, c)).unwrap_or(0),
        r => r,
    };
    if from != 0 && grid.region(layer, g) != from {
        match nearest_in_region(grid, layer, g, from, REGION_RADIUS) {
            Some(c) if c == s => return vec![start],
            Some(c) => {
                g = c;
                goal_point = None;
            }
            // Far from anything reachable (e.g. across a sea): a short
            // search towards it, ending at its nearest explored cell.
            None => {
                let (points, _) = astar(grid, search, layer, s, g, window(grid, s, g, WINDOW_MARGIN), SHORT_BUDGET.min(max_expand));
                return to_waypoints(grid, layer, &points, None);
            }
        }
    }
    if octile(s.0, s.1, g.0, g.1) <= SHORT_TRIP {
        let (points, reached) = astar(grid, search, layer, s, g, window(grid, s, g, WINDOW_MARGIN), SHORT_BUDGET.min(max_expand));
        if reached {
            return to_waypoints(grid, layer, &points, goal_point);
        }
    }
    if let Some(h) = grid.hier(layer) {
        let open = grid.open_at(layer);
        let mut work = 0;
        let nodes = h.search(&open, &mut search.hier, s, g, max_expand, &mut work);
        search.work += work;
        if let Some(nodes) = nodes {
            let cells = refine(grid, search, layer, h, &nodes);
            return to_waypoints(grid, layer, &cells, goal_point);
        }
    }
    let full = Rect { x0: 0, z0: 0, x1: grid.width, z1: grid.height };
    let (points, reached) = astar(grid, search, layer, s, g, full, max_expand);
    to_waypoints(grid, layer, &points, if reached { goal_point } else { None })
}

/// The cells' bounding box grown by `margin`, clipped to the grid.
fn window(grid: &NavGrid, a: (u32, u32), b: (u32, u32), margin: u32) -> Rect {
    Rect {
        x0: a.0.min(b.0).saturating_sub(margin),
        z0: a.1.min(b.1).saturating_sub(margin),
        x1: (a.0.max(b.0) + margin + 1).min(grid.width),
        z1: (a.1.max(b.1) + margin + 1).min(grid.height),
    }
}

/// Cell path along graph nodes: twins step across, other legs go straight
/// where the line is clear, else by A* inside their sector.
fn refine(grid: &NavGrid, search: &mut Search, layer: NavClass, h: &crate::engine::sim::hpa::Hier, nodes: &[u32]) -> Vec<u32> {
    let w = grid.width;
    let xy = |c: u32| (c % w, c / w);
    let mut cells = vec![nodes[0]];
    for leg in nodes.windows(2) {
        let (a, b) = (xy(leg[0]), xy(leg[1]));
        if a.0.abs_diff(b.0) <= 1 && a.1.abs_diff(b.1) <= 1 {
            cells.push(leg[1]);
            continue;
        }
        search.work += a.0.abs_diff(b.0).max(a.1.abs_diff(b.1)) as u64;
        if grid.line_clear(layer, a, b) {
            cells.push(leg[1]);
            continue;
        }
        let (part, _) = astar(grid, search, layer, a, b, h.rect_of(a.0, a.1), u32::MAX);
        cells.extend_from_slice(&part[1..]);
    }
    cells
}

/// How far (cells) from an unreachable goal to look for a reachable cell.
const REGION_RADIUS: u32 = 64;

/// The cell of region `region` nearest `c` within `max_r` rings (same
/// tie-breaks as [`nearest_open`]).
fn nearest_in_region(grid: &NavGrid, layer: NavClass, (cx, cz): (u32, u32), region: u32, max_r: u32) -> Option<(u32, u32)> {
    for r in 1..=max_r as i64 {
        let mut best: Option<(u32, u32, (u32, u32))> = None;
        let (x, z) = (cx as i64, cz as i64);
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= grid.width as i64 || nz >= grid.height as i64 {
                    continue;
                }
                let (nx, nz) = (nx as u32, nz as u32);
                if grid.region(layer, (nx, nz)) != region {
                    continue;
                }
                let key = (octile(cx, cz, nx, nz), nz * grid.width + nx, (nx, nz));
                if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
                    best = Some(key);
                }
            }
        }
        if let Some(b) = best {
            return Some(b.2);
        }
    }
    None
}

/// How far (cells) a blocked goal is moved to find an open cell.
const SNAP_RADIUS: u32 = 32;

/// The cell nearest `c` (itself first) within `max_r` rings that `ok`
/// accepts: nearest by octile distance within the first ring that has
/// one, ties by index (as [`nearest_open`]).
fn nearest_where(grid: &NavGrid, (cx, cz): (u32, u32), max_r: u32, mut ok: impl FnMut(u32, u32) -> bool) -> Option<(u32, u32)> {
    if cx < grid.width && cz < grid.height && ok(cx, cz) {
        return Some((cx, cz));
    }
    for r in 1..=max_r as i64 {
        let mut best: Option<(u32, u32, (u32, u32))> = None;
        let (x, z) = (cx as i64, cz as i64);
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= grid.width as i64 || nz >= grid.height as i64 || !ok(nx as u32, nz as u32) {
                    continue;
                }
                let (nx, nz) = (nx as u32, nz as u32);
                let key = (octile(cx, cz, nx, nz), nz * grid.width + nx, (nx, nz));
                if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
                    best = Some(key);
                }
            }
        }
        if let Some(b) = best {
            return Some(b.2);
        }
    }
    None
}

/// How far (cells) a final formation slot moves to find ground its unit can
/// stand on.
const FIT_RADIUS: u32 = 16;

/// The open cell nearest `c` within `max_r` rings (Chebyshev), nearest by
/// octile distance within the first ring that has one, ties by index.
fn nearest_open(grid: &NavGrid, layer: NavClass, (cx, cz): (u32, u32), max_r: u32) -> Option<(u32, u32)> {
    for r in 1..=max_r as i64 {
        let mut best: Option<(u32, u32, (u32, u32))> = None;
        let (x, z) = (cx as i64, cz as i64);
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || !grid.passable(layer, nx as u32, nz as u32) {
                    continue;
                }
                let (nx, nz) = (nx as u32, nz as u32);
                let key = (octile(cx, cz, nx, nz), nz * grid.width + nx, (nx, nz));
                if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
                    best = Some(key);
                }
            }
        }
        if let Some(b) = best {
            return Some(b.2);
        }
    }
    None
}

/// A* inside `win` (which holds both ends): the cell path (global cell
/// indices, start..end) and whether it reached the goal (if not, it ends at
/// the explored cell nearest the goal).
pub(crate) fn astar(grid: &NavGrid, search: &mut Search, layer: NavClass, (sx, sz): (u32, u32), (gx, gz): (u32, u32), win: Rect, max_expand: u32) -> (Vec<u32>, bool) {
    let ww = win.width();
    search.prepare((win.width() * win.height()) as usize);
    let local = |x: u32, z: u32| ((z - win.z0) * ww + x - win.x0) as usize;
    let start_i = local(sx, sz);
    search.touch(start_i);
    search.g[start_i] = 0;
    search.parent[start_i] = start_i as u32;
    let h0 = octile(sx, sz, gx, gz);
    search.open.push((Reverse(h0), Reverse(h0), Reverse(start_i as u32)));
    // Nearest explored cell to the goal (by heuristic, then index).
    let mut best = (h0, start_i as u32);
    let goal_i = win.contains(gx, gz).then(|| local(gx, gz));
    let mut reached = false;
    let open = grid.open_at(layer);
    let open_at = |x: u32, z: u32| win.contains(x, z) && open(x, z);
    let before = search.work;
    while let Some((_, Reverse(h), Reverse(i))) = search.open.pop() {
        let i = i as usize;
        if search.closed[i] {
            continue;
        }
        search.closed[i] = true;
        search.expanded += 1;
        if (h, i as u32) < best {
            best = (h, i as u32);
        }
        if Some(i) == goal_i {
            reached = true;
            break;
        }
        if search.expanded >= max_expand {
            break;
        }
        let (x, z) = (win.x0 + i as u32 % ww, win.z0 + i as u32 / ww);
        for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            // Wrapping keeps out-of-range neighbours out (they exceed w / h).
            let (nx, nz) = (x.wrapping_add_signed(dx), z.wrapping_add_signed(dz));
            if !open_at(nx, nz) {
                continue;
            }
            let diagonal = dx != 0 && dz != 0;
            if diagonal && (!open_at(nx, z) || !open_at(x, nz)) {
                continue;
            }
            let ni = local(nx, nz);
            search.touch(ni);
            if search.closed[ni] {
                continue;
            }
            let g = search.g[i] + if diagonal { 14 } else { 10 };
            if g < search.g[ni] {
                search.g[ni] = g;
                search.parent[ni] = i as u32;
                let h = octile(nx, nz, gx, gz);
                search.open.push((Reverse(g + h), Reverse(h), Reverse(ni as u32)));
            }
        }
    }
    search.work = before + 8 * search.expanded as u64;
    let end = if reached { goal_i.expect("reached") } else { best.1 as usize };
    let mut path = vec![end as u32];
    let mut cur = end;
    while cur != start_i {
        cur = search.parent[cur] as usize;
        path.push(cur as u32);
    }
    path.reverse();
    let w = grid.width;
    for c in path.iter_mut() {
        *c = (win.z0 + *c / ww) * w + win.x0 + *c % ww;
    }
    (path, reached)
}

/// Total octile cost of a point path (consecutive points straight/diagonal).
#[cfg(test)]
fn path_cost(w: u32, points: &[u32]) -> u32 {
    points.windows(2).map(|p| octile(p[0] % w, p[0] / w, p[1] % w, p[1] / w)).sum()
}

/// Points further ahead than this aren't tried by line-of-sight smoothing.
const SMOOTH_LOOKAHEAD: usize = 8;

/// Line-of-sight smoothing to world waypoints: the path's turning points,
/// then from each kept point a jump to the farthest of the next few still
/// in sight; `goal` replaces the last point when the goal was reached.
fn to_waypoints(grid: &NavGrid, layer: NavClass, points: &[u32], goal: Option<FxVec3>) -> Vec<FxVec3> {
    let w = grid.width;
    let xy = |i: u32| (i % w, i / w);
    // Turning points: drop cells in the middle of straight runs.
    let mut turns: Vec<u32> = Vec::with_capacity(points.len().min(64));
    for (k, &p) in points.iter().enumerate() {
        if k > 0 && k + 1 < points.len() {
            let (a, b, c) = (xy(points[k - 1]), xy(p), xy(points[k + 1]));
            let d1 = (b.0 as i64 - a.0 as i64, b.1 as i64 - a.1 as i64);
            let d2 = (c.0 as i64 - b.0 as i64, c.1 as i64 - b.1 as i64);
            // Same direction (adjacent cells): collinear, skip.
            if d1.0.abs() <= 1 && d1.1.abs() <= 1 && d1 == d2 {
                continue;
            }
        }
        turns.push(p);
    }
    let mut kept = vec![turns[0]];
    let mut k = 0;
    while k < turns.len() - 1 {
        let mut j = (k + SMOOTH_LOOKAHEAD).min(turns.len() - 1);
        while j > k + 1 && !grid.line_clear(layer, xy(turns[k]), xy(turns[j])) {
            j -= 1;
        }
        kept.push(turns[j]);
        k = j;
    }
    let mut out: Vec<FxVec3> = kept
        .into_iter()
        .skip(1)
        .map(|i| {
            let (x, z) = grid.point_of(layer, xy(i));
            FxVec3 { x, y: Fx::ZERO, z }
        })
        .collect();
    if let (Some(goal), Some(last)) = (goal, out.last_mut()) {
        *last = goal;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Vehicle1x1` in the default class list.
    const LAND: NavClass = NavClass(0);
    use crate::engine::assets::scmap::Heightmap;

    /// A flat 32x32 map with a wall at x = 16 for z in 0..28 (gap at the top).
    fn walled() -> NavGrid {
        let mut samples = vec![0u16; 33 * 33];
        for z in 0..29 {
            samples[z * 33 + 16] = 50 * 128;
        }
        let t = Terrain::from_heightmap(&Heightmap { width: 32, height: 32, scale: 1.0 / 128.0, samples });
        NavGrid::build(&t, &[false; 256])
    }

    #[test]
    fn routes_round_a_wall_and_is_deterministic() {
        let g = walled();
        assert!(!g.passable(LAND, 16, 5) && !g.passable(LAND, 15, 5), "cliff cells either side of the ridge");
        let mut s = Search::default();
        let (a, b) = (FxVec3::from_f64(5.5, 0.0, 5.5), FxVec3::from_f64(27.5, 0.0, 5.5));
        let path = find_path(&g, &mut s, LAND, a, b, 100_000);
        assert_eq!(*path.last().unwrap(), b);
        assert!(path.iter().any(|p| p.z > Fx::from_num(28)), "goes through the gap: {path:?}");
        assert!(path.len() <= 4, "smoothed to a few waypoints: {path:?}");
        let mut s2 = Search::default();
        assert_eq!(find_path(&g, &mut s2, LAND, a, b, 100_000), path, "same result from fresh buffers");
        assert_eq!(find_path(&g, &mut s, LAND, a, b, 100_000), path, "and from reused ones");
    }

    #[test]
    fn sector_graph_paths_are_near_optimal_and_agree_on_reachability() {
        use crate::engine::sim::rng::SimRng;
        let mut rng = SimRng::new(77);
        let (mut total, mut optimal) = (0u64, 0u64);
        for round in 0..12 {
            // 96 x 96: 3 x 3 sectors of 32.
            let n = 96usize;
            let mut samples = vec![0u16; (n + 1) * (n + 1)];
            for _ in 0..(60 + round * 20) {
                let (x, z) = ((rng.next_u64() % n as u64) as usize, (rng.next_u64() % n as u64) as usize);
                let len = 3 + (rng.next_u64() % 20) as usize;
                let vertical = rng.next_u64() % 2 == 0;
                for k in 0..len {
                    let (px, pz) = if vertical { (x, (z + k).min(n)) } else { ((x + k).min(n), z) };
                    samples[pz * (n + 1) + px] = 40 * 128;
                }
            }
            let t = Terrain::from_heightmap(&Heightmap { width: n as u32, height: n as u32, scale: 1.0 / 128.0, samples });
            let mut g = NavGrid::build(&t, &[false; 256]);
            g.refresh();
            let h = g.hier(LAND).expect("fresh after refresh");
            let open = g.open_at(LAND);
            let full = Rect { x0: 0, z0: 0, x1: g.width, z1: g.height };
            let mut s = Search::default();
            for _ in 0..12 {
                let a = ((rng.next_u64() % n as u64) as u32, (rng.next_u64() % n as u64) as u32);
                let b = ((rng.next_u64() % n as u64) as u32, (rng.next_u64() % n as u64) as u32);
                if !g.passable(LAND, a.0, a.1) || !g.passable(LAND, b.0, b.1) || a == b {
                    continue;
                }
                let (best, reached) = astar(&g, &mut s, LAND, a, b, full, u32::MAX);
                let mut work = 0;
                let nodes = h.search(&open, &mut s.hier, a, b, u32::MAX, &mut work);
                assert_eq!(nodes.is_some(), reached, "same reachability {a:?} -> {b:?}");
                assert_eq!(h.component(a.0, a.1) == h.component(b.0, b.1), reached, "components agree {a:?} -> {b:?}");
                if let Some(nodes) = nodes {
                    let cells = refine(&g, &mut s, LAND, h, &nodes);
                    assert_eq!((cells[0], *cells.last().unwrap()), (a.1 * g.width + a.0, b.1 * g.width + b.0));
                    let (c, o) = (path_cost(g.width, &cells), path_cost(g.width, &best));
                    // Trips long enough to use the graph (shorter ones run
                    // windowed A*).
                    if o > SHORT_TRIP {
                        assert!(c * 100 <= o * 110, "long trip within 10% of optimal {a:?} -> {b:?}: {c} vs {o}");
                    }
                    total += c as u64;
                    optimal += o as u64;
                }
            }
        }
        assert!(total * 100 <= optimal * 105, "within 5% overall: {total} vs {optimal}");
    }

    #[test]
    fn sector_graph_rebuilds_only_around_changes() {
        let mut g = flat(128);
        g.refresh();
        let mut h = g.hier[0].clone().unwrap();
        // One structure in sector (1, 1): it and its 4 neighbours rebuild.
        g.block_rect(Fx::from_num(40), Fx::from_num(40), Fx::from_num(42), Fx::from_num(42), true);
        h.mark(Rect { x0: 40, z0: 40, x1: 42, z1: 42 });
        assert_eq!(h.refresh(&g.open_at(LAND)), 5);
        // And the result is the same as a fresh build.
        let mut fresh = crate::engine::sim::hpa::Hier::new(g.width, g.height);
        fresh.refresh(&g.open_at(LAND));
        let mut s = Search::default();
        let (a, b) = ((2, 41), (120, 41));
        let (mut w1, mut w2) = (0, 0);
        assert_eq!(h.search(&g.open_at(LAND), &mut s.hier, a, b, u32::MAX, &mut w1), fresh.search(&g.open_at(LAND), &mut s.hier, a, b, u32::MAX, &mut w2));
    }

    #[test]
    fn goal_in_a_walled_pocket_ends_outside_without_a_full_search() {
        let mut g = walled();
        // An open 3x3 pocket at (6..9, 20..23) ringed by structures.
        for (x0, z0, x1, z1) in [(4, 18, 11, 20), (4, 23, 11, 25), (4, 20, 6, 23), (9, 20, 11, 23)] {
            g.block_rect(Fx::from_num(x0), Fx::from_num(z0), Fx::from_num(x1), Fx::from_num(z1), true);
        }
        g.refresh();
        let goal = FxVec3::from_f64(7.5, 0.0, 21.5);
        assert!(g.passable(LAND, 7, 21), "the pocket itself is open");
        let mut s = Search::default();
        let path = find_path(&g, &mut s, LAND, FxVec3::from_f64(7.5, 0.0, 5.5), goal, 100_000);
        let end = *path.last().unwrap();
        assert!(end.z < Fx::from_num(18) || end.x < Fx::from_num(4) || end.x > Fx::from_num(11) || end.z > Fx::from_num(25), "stops outside the ring: {end:?}");
        assert!(s.expanded < 500, "no whole-map search: {}", s.expanded);
        // Without labels the old behaviour (full search) still finds the same side.
    }

    #[test]
    fn region_labels_follow_grid_changes() {
        let mut g = walled();
        g.refresh();
        let (a, b) = (g.region(LAND, (5, 5)), g.region(LAND, (27, 5)));
        assert!(a != 0 && a == b, "connected through the gap");
        g.block_rect(Fx::from_num(14), Fx::from_num(27), Fx::from_num(19), Fx::from_num(32), true);
        assert_eq!(g.region(LAND, (5, 5)), 0, "stale labels read as unknown");
        g.refresh();
        assert_ne!(g.region(LAND, (5, 5)), g.region(LAND, (27, 5)), "gap closed: two regions");
    }

    #[test]
    fn goal_inside_a_structure_heads_for_its_edge_cheaply() {
        let mut g = walled();
        // A 6x6 building at (4..10, 14..20), goal in its middle.
        g.block_rect(Fx::from_num(4), Fx::from_num(14), Fx::from_num(10), Fx::from_num(20), true);
        let mut s = Search::default();
        let goal = FxVec3::from_f64(7.5, 0.0, 17.5);
        let path = find_path(&g, &mut s, LAND, FxVec3::from_f64(7.5, 0.0, 5.5), goal, 100_000);
        let end = *path.last().unwrap();
        assert_ne!(end, goal, "the goal itself can't be reached");
        assert!(g.passable(LAND, end.x.to_num::<u32>(), end.z.to_num::<u32>()), "ends on an open cell: {end:?}");
        // Nearest open cell to the goal: just outside the building.
        let (ex, ez) = (end.x.to_num::<f64>(), end.z.to_num::<f64>());
        assert!((3.0..11.0).contains(&ex) && (13.0..21.0).contains(&ez), "beside the building: {end:?}");
        assert!(s.expanded < 200, "a small search, not the whole map: {}", s.expanded);
    }

    #[test]
    fn unreachable_goal_ends_nearest() {
        let mut g = walled();
        // Close the gap.
        g.block_rect(Fx::from_num(14), Fx::from_num(27), Fx::from_num(19), Fx::from_num(32), true);
        let mut s = Search::default();
        let path = find_path(&g, &mut s, LAND, FxVec3::from_f64(5.5, 0.0, 5.5), FxVec3::from_f64(27.5, 0.0, 5.5), 100_000);
        let end = *path.last().unwrap();
        assert!(end.x < Fx::from_num(16), "stays on its side: {end:?}");
        assert!(end.x > Fx::from_num(13), "but as close as it can get: {end:?}");
        // Opening it again restores the route.
        g.block_rect(Fx::from_num(14), Fx::from_num(27), Fx::from_num(19), Fx::from_num(32), false);
        let path = find_path(&g, &mut s, LAND, FxVec3::from_f64(5.5, 0.0, 5.5), FxVec3::from_f64(27.5, 0.0, 5.5), 100_000);
        assert_eq!(*path.last().unwrap(), FxVec3::from_f64(27.5, 0.0, 5.5));
    }

    fn flat(n: u32) -> NavGrid {
        let t = Terrain::from_heightmap(&Heightmap { width: n, height: n, scale: 1.0 / 128.0, samples: vec![0; ((n + 1) * (n + 1)) as usize] });
        NavGrid::build(&t, &[false; 256])
    }

    /// A footprint of N cells at p covers cells from round(p - N / 2),
    /// halves to even (`COORDS_ToGridRect`); the class's bit sits at that
    /// block's anchor (first cell + (N - 1) / 2), and units stand at the
    /// footprint's centre.
    #[test]
    fn cells_follow_moho_footprint_rects() {
        let g = flat(32);
        let class = |name: &str| g.class_named(name).unwrap();
        let at = |c: NavClass, x: f64| g.cell_for(c, FxVec3::from_f64(x, 0.0, x)).0;
        // 1x1: the cell it's in; a tie (an integer position: p - 0.5 is a
        // half) rounds to even.
        let one = class("Vehicle1x1");
        assert_eq!((at(one, 10.3), at(one, 10.9), at(one, 10.0), at(one, 11.0)), (10, 10, 10, 10));
        // 4x4 (anchor = first + 1): from 10.3 the block starts at round(8.3)
        // = 8, from 10.7 at 9; at 10.5 the half rounds to the even 8.
        let four = class("Water4x4");
        assert_eq!((at(four, 10.3), at(four, 10.7), at(four, 10.5), at(four, 11.5)), (9, 10, 9, 11));
        // Where they stand: a 1x1 at the cell's centre, a 4x4 on the corner
        // at the middle of its block.
        assert_eq!(g.point_of(one, (10, 10)).0, Fx::from_num(10.5));
        assert_eq!(g.point_of(four, (9, 9)).0, Fx::from_num(10));
        assert_eq!(at(four, 10.0), 9, "its own point maps back to the cell");
    }

    #[test]
    fn units_get_the_nearest_class_to_their_footprint() {
        let g = flat(8);
        let name = |caps, x, z| g.class_for(caps, x, z).map(|c| g.classes()[c.0 as usize].name.clone());
        use caps::*;
        // As the real game gives these blueprints (probe_bp).
        assert_eq!(name(WATER, 2, 6).as_deref(), Some("Water4x4"), "frigate ues0103");
        assert_eq!(name(LAND | SEABED, 1, 2).as_deref(), Some("Amphibious1x1"), "Cybran amphibious tank url0203");
        assert_eq!(name(LAND, 1, 1).as_deref(), Some("Vehicle1x1"));
        assert_eq!(name(LAND, 3, 3).as_deref(), Some("Vehicle2x2"), "a tie: the first listed");
        assert_eq!(name(LAND, 4, 4).as_deref(), Some("Vehicle5x5"));
        assert_eq!(name(LAND | SEABED, 2, 2).as_deref(), Some("Amphibious1x1"), "a tie with 3x3: the first listed");
        assert_eq!(name(WATER, 16, 16).as_deref(), Some("Water11x11"));
        assert_eq!(name(SUB | WATER, 1, 1).as_deref(), Some("SurfacingSub2x2"));
        assert_eq!(name(SEABED, 1, 1), None, "no class with those caps");
    }

    #[test]
    fn big_classes_dont_fit_through_narrow_gaps() {
        let mut g = walled();
        // The ridge's gap is the 3 cells z = 29..32.
        let class = |n: &str| g.class_named(n).unwrap();
        let (v2, a3, v5) = (class("Vehicle2x2"), class("Amphibious3x3"), class("Vehicle5x5"));
        for c in [v2, a3, v5] {
            g.ensure(c);
        }
        g.refresh();
        let (a, b) = (FxVec3::from_f64(5.5, 0.0, 5.5), FxVec3::from_f64(27.5, 0.0, 5.5));
        let mut s = Search::default();
        for (c, through) in [(LAND, true), (v2, true), (a3, true), (v5, false)] {
            let end = *find_path(&g, &mut s, c, a, b, 100_000).last().unwrap();
            assert_eq!(end == b, through, "{}: ends at {end:?}", g.classes()[c.0 as usize].name);
        }
        // A 3x3 can't stand with its footprint over the ridge or the map edge.
        assert!(!g.passable(a3, 14, 5) && g.passable(a3, 13, 5) && !g.passable(a3, 0, 5) && g.passable(a3, 1, 5));
    }

    #[test]
    fn ignore_structures_classes_walk_through_bases() {
        let mut g = flat(32);
        let (v1, a3) = (LAND, g.class_named("Amphibious3x3").unwrap());
        g.ensure(a3);
        // A wall of structures right across the map.
        g.block_rect(Fx::from_num(14), Fx::ZERO, Fx::from_num(16), Fx::from_num(32), true);
        g.refresh();
        let (a, b) = (FxVec3::from_f64(5.5, 0.0, 15.5), FxVec3::from_f64(27.5, 0.0, 15.5));
        let mut s = Search::default();
        assert_ne!(*find_path(&g, &mut s, v1, a, b, 100_000).last().unwrap(), b, "tanks are walled off");
        assert_eq!(*find_path(&g, &mut s, a3, a, b, 100_000).last().unwrap(), b, "experimentals walk over");
    }

    #[test]
    fn refitting_after_changes_matches_a_fresh_fit() {
        use crate::engine::sim::rng::SimRng;
        let mut rng = SimRng::new(5);
        let mut g = walled();
        let all: Vec<NavClass> = (0..g.classes().len()).map(|c| NavClass(c as u8)).collect();
        for &c in &all {
            g.ensure(c);
        }
        let mut rects = Vec::new();
        for _ in 0..30 {
            let (x, z) = ((rng.next_u64() % 30) as i32, (rng.next_u64() % 30) as i32);
            let (w, h) = (1 + (rng.next_u64() % 4) as i32, 1 + (rng.next_u64() % 4) as i32);
            let add = rects.is_empty() || rng.next_u64() % 3 != 0;
            let r = if add { (x, z, x + w, z + h) } else { rects.remove((rng.next_u64() % rects.len() as u64) as usize) };
            if add {
                rects.push(r);
            }
            g.block_rect(Fx::from_num(r.0), Fx::from_num(r.1), Fx::from_num(r.2), Fx::from_num(r.3), add);
        }
        // The same structures placed before the classes are fitted.
        let mut fresh = walled();
        for r in &rects {
            fresh.block_rect(Fx::from_num(r.0), Fx::from_num(r.1), Fx::from_num(r.2), Fx::from_num(r.3), true);
        }
        for &c in &all {
            fresh.ensure(c);
        }
        assert_eq!(g.open, fresh.open);
    }
}
