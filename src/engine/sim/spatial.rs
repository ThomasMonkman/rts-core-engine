//! Uniform spatial grid over the map's XZ plane, for unit queries
//! (`GetUnitsAroundPoint`, `GetUnitsInRect`, ...).
//!
//! Fixed-point throughout: a position's cell is the integer part of its
//! coordinate divided by a power-of-two cell size, so membership is exact
//! and identical on every machine. Order inside a cell is arbitrary (removal
//! swap-removes); queries sort their result by id, so callers always see id
//! order. Positions off the map clamp into the border cells.
//!
//! A point entity is listed in the one cell under it. Something bigger than
//! a cell (a shield bubble) can instead be listed in every cell its extent
//! covers ([`SpatialGrid::insert_rect`]), so a query finds it from any cell
//! it reaches into without widening by the largest extent in the world.

use crate::engine::sim::entity::EntityId;
use crate::engine::sim::math::{Fx, FxVec3};

/// Cell edge = 2^CELL_SHIFT world units (32).
const CELL_SHIFT: u32 = 5;

/// An inclusive range of grid columns (`x0..=x1`) and rows (`z0..=z1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CellRect {
    pub x0: u32,
    pub z0: u32,
    pub x1: u32,
    pub z1: u32,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SpatialGrid {
    cols: u32,
    rows: u32,
    cells: Vec<Vec<EntityId>>,
}

impl Default for SpatialGrid {
    /// One cell covering everything until the map size is known.
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl SpatialGrid {
    /// A grid covering a `width` x `height` map (world units).
    pub fn new(width: u32, height: u32) -> Self {
        let cols = (width >> CELL_SHIFT).max(1) + 1;
        let rows = (height >> CELL_SHIFT).max(1) + 1;
        Self { cols, rows, cells: vec![Vec::new(); (cols * rows) as usize] }
    }

    fn axis(v: Fx, n: u32) -> u32 {
        // floor(v / 2^CELL_SHIFT), clamped to the grid.
        let c = v.floor().to_num::<i64>() >> CELL_SHIFT;
        c.clamp(0, n as i64 - 1) as u32
    }

    /// Cell index of world position `p`.
    pub fn cell_of(&self, p: FxVec3) -> u32 {
        Self::axis(p.z, self.rows) * self.cols + Self::axis(p.x, self.cols)
    }

    pub fn insert(&mut self, id: EntityId, cell: u32) {
        self.cells[cell as usize].push(id);
    }

    pub fn remove(&mut self, id: EntityId, cell: u32) {
        let c = &mut self.cells[cell as usize];
        if let Some(i) = c.iter().position(|&e| e == id) {
            c.swap_remove(i);
        }
    }

    /// Indices of the cells overlapping the XZ box [min, max], appended.
    pub fn cells_in(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx, out: &mut Vec<u32>) {
        let (x0, x1) = (Self::axis(min_x, self.cols), Self::axis(max_x, self.cols));
        let (z0, z1) = (Self::axis(min_z, self.rows), Self::axis(max_z, self.rows));
        for z in z0..=z1 {
            out.extend((x0..=x1).map(|x| z * self.cols + x));
        }
    }

    /// The ids in one cell (unordered).
    pub fn cell(&self, i: u32) -> &[EntityId] {
        &self.cells[i as usize]
    }

    /// The cells overlapping the XZ box [min, max], as an inclusive range
    /// of columns and rows (clamped to the grid like [`SpatialGrid::cell_of`]).
    pub fn rect_of(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx) -> CellRect {
        CellRect {
            x0: Self::axis(min_x, self.cols),
            z0: Self::axis(min_z, self.rows),
            x1: Self::axis(max_x, self.cols),
            z1: Self::axis(max_z, self.rows),
        }
    }

    /// Lists `id` in every cell of `r` (for something bigger than a cell).
    pub fn insert_rect(&mut self, id: EntityId, r: CellRect) {
        for z in r.z0..=r.z1 {
            for x in r.x0..=r.x1 {
                self.insert(id, z * self.cols + x);
            }
        }
    }

    /// Takes `id` out of every cell of `r` ([`SpatialGrid::insert_rect`]).
    pub fn remove_rect(&mut self, id: EntityId, r: CellRect) {
        for z in r.z0..=r.z1 {
            for x in r.x0..=r.x1 {
                self.remove(id, z * self.cols + x);
            }
        }
    }

    /// Ids in every cell overlapping the XZ box [min, max] (unsorted,
    /// unfiltered: callers apply the exact test, then sort).
    pub fn candidates(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx, out: &mut Vec<EntityId>) {
        self.for_each_candidate(min_x, min_z, max_x, max_z, |id| out.push(id));
    }

    /// [`SpatialGrid::candidates`] one at a time, without a buffer (in no
    /// particular order: callers' results mustn't depend on it).
    pub fn for_each_candidate(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx, mut f: impl FnMut(EntityId)) {
        let (x0, x1) = (Self::axis(min_x, self.cols), Self::axis(max_x, self.cols));
        let (z0, z1) = (Self::axis(min_z, self.rows), Self::axis(max_z, self.rows));
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &id in &self.cells[(z * self.cols + x) as usize] {
                    f(id);
                }
            }
        }
    }
}
