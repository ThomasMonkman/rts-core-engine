//! Hierarchical pathfinding (HPA*) over one footprint class's grid.
//!
//! The map is cut into `SECTOR` x `SECTOR` sectors. Wherever open cells
//! face each other across a sector border, the run of such pairs gets
//! portals: one pair in the middle of a short run, one at each end of a
//! long one. Each portal cell is a node of the sector it lies in, linked to
//! its twin across the border (cost 10, one straight step). Within a sector
//! every two nodes in the same local region (4-connected open cells, which
//! for 8-connected moves without corner cutting is the same connectivity)
//! get an edge with their exact in-sector path cost.
//!
//! A long search then runs A* over the nodes only, from the start (joined to
//! its sector's nodes by an in-sector Dijkstra) to the goal (likewise), and
//! the result is refined leg by leg back in `nav` (a straight line where it
//! is clear, an in-sector A* where it isn't). Paths are within a few
//! percent of optimal before smoothing.
//!
//! Connectivity comes for free: local regions joined through portal links
//! form the map's connected components, which replace whole-map labelling.
//!
//! Changes are local: a changed cell dirties its sector, and its
//! neighbour too when it lies on their shared border (the portals there may
//! have moved); a refresh rebuilds the dirty sectors. Links name the twin's *cell*, never a node
//! index, so sectors rebuild independently. Everything is integer and built
//! in sector order (in parallel, merged by index), so the graph is the same
//! on every machine.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// Hasher for cell-keyed maps that are only looked up, never iterated (so
/// their order can't leak into results): one multiply, where std's SipHash
/// was a fifth of a 63-army game's pathfinding time.
#[derive(Default, Clone, Copy)]
pub struct CellHasher(u64);

impl std::hash::Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    fn write_u32(&mut self, v: u32) {
        // Fibonacci hashing: the high bits (what hashbrown uses) mix every
        // bit of the cell.
        self.0 = (v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// A lookup-only map keyed by cell (see [`CellHasher`]).
#[allow(clippy::disallowed_types)]
pub type CellMap<V> = HashMap<u32, V, std::hash::BuildHasherDefault<CellHasher>>;

/// Sector side, cells.
pub const SECTOR: u32 = 32;
/// Border runs longer than this get a portal at each end, shorter ones one
/// in the middle.
const LONG_RUN: u32 = 6;
/// Rebuilding fewer sectors than this runs serially (rayon wake-up).
const PARALLEL_MIN: usize = 16;

/// A cell rectangle `[x0, x1) x [z0, z1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x0: u32,
    pub z0: u32,
    pub x1: u32,
    pub z1: u32,
}

impl Rect {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> u32 {
        self.z1 - self.z0
    }
    pub fn contains(&self, x: u32, z: u32) -> bool {
        x >= self.x0 && x < self.x1 && z >= self.z0 && z < self.z1
    }
}

pub fn octile(ax: u32, az: u32, bx: u32, bz: u32) -> u32 {
    let (dx, dz) = (ax.abs_diff(bx), az.abs_diff(bz));
    10 * dx.max(dz) + 4 * dx.min(dz)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Node {
    /// Global cell index.
    cell: u32,
    /// Local region (1-based) the cell is in.
    region: u16,
    /// Edges to other nodes of the sector: (node index, cost).
    intra: Vec<(u16, u32)>,
    /// Twin cells across the sector's borders.
    inter: Vec<u32>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Sector {
    /// Local region per cell (row-major within the sector; 0 = closed).
    local: Vec<u16>,
    /// Global component per local region (index = region; [0] = 0).
    comp: Vec<u32>,
    nodes: Vec<Node>,
    /// (cell, node index) sorted by cell: `node_of` by binary search.
    by_cell: Vec<(u32, u16)>,
    /// Local regions (labels 1..=regions in `local`).
    regions: u16,
}

/// One class's sector graph.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Hier {
    w: u32,
    h: u32,
    sw: u32,
    sh: u32,
    sectors: Vec<Sector>,
    dirty: Vec<bool>,
    any_dirty: bool,
    /// Dense node numbering: sector `s`'s nodes are `node_base[s]..`
    /// (`node_base[sectors]` = the node count). Rebuilt with the graph.
    node_base: Vec<u32>,
}

/// Per-thread buffers for searches over a [`Hier`].
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Scratch {
    dist: Vec<u32>,
    heap: BinaryHeap<(Reverse<u32>, u32)>,
    /// Abstract search: cell -> (g, parent cell, closed).
    nodes: CellMap<(u32, u32, bool)>,
    open: BinaryHeap<(Reverse<u32>, Reverse<u32>, Reverse<u32>)>,
}

impl Hier {
    /// An unbuilt graph for a `w` x `h` grid (every sector dirty).
    pub fn new(w: u32, h: u32) -> Self {
        let (sw, sh) = (w.div_ceil(SECTOR), h.div_ceil(SECTOR));
        let n = (sw * sh) as usize;
        Self { w, h, sw, sh, sectors: vec![Sector::default(); n], dirty: vec![true; n], any_dirty: true, node_base: vec![0; n + 1] }
    }

    pub fn is_fresh(&self) -> bool {
        !self.any_dirty
    }

    fn sector_of(&self, x: u32, z: u32) -> usize {
        ((z / SECTOR) * self.sw + x / SECTOR) as usize
    }

    pub fn sector_rect(&self, s: usize) -> Rect {
        let (sx, sz) = (s as u32 % self.sw, s as u32 / self.sw);
        Rect { x0: sx * SECTOR, z0: sz * SECTOR, x1: ((sx + 1) * SECTOR).min(self.w), z1: ((sz + 1) * SECTOR).min(self.h) }
    }

    /// The rect of the sector holding cell `(x, z)`.
    pub fn rect_of(&self, x: u32, z: u32) -> Rect {
        self.sector_rect(self.sector_of(x, z))
    }

    /// The graph is stale (queries see none) until the next refresh, even
    /// if no cell changed.
    pub fn touch(&mut self) {
        self.any_dirty = true;
    }

    /// Cells in `r` changed: their sectors need rebuilding, and so does a
    /// neighbour when `r` reaches the cells along their shared border (the
    /// only cells of a sector its neighbours' builds read: their portals).
    pub fn mark(&mut self, r: Rect) {
        if r.x0 >= r.x1 || r.z0 >= r.z1 || r.x0 >= self.w || r.z0 >= self.h {
            return;
        }
        let (sx0, sz0) = (r.x0 / SECTOR, r.z0 / SECTOR);
        let (sx1, sz1) = ((r.x1 - 1) / SECTOR, (r.z1 - 1) / SECTOR);
        for sz in sz0..=sz1.min(self.sh - 1) {
            for sx in sx0..=sx1.min(self.sw - 1) {
                let s = (sz * self.sw + sx) as usize;
                let sr = self.sector_rect(s);
                self.dirty[s] = true;
                if sx > 0 && r.x0 <= sr.x0 {
                    self.dirty[s - 1] = true;
                }
                if sx + 1 < self.sw && r.x1 >= sr.x1 {
                    self.dirty[s + 1] = true;
                }
                if sz > 0 && r.z0 <= sr.z0 {
                    self.dirty[s - self.sw as usize] = true;
                }
                if sz + 1 < self.sh && r.z1 >= sr.z1 {
                    self.dirty[s + self.sw as usize] = true;
                }
            }
        }
        self.any_dirty = true;
    }

    /// Rebuilds the sectors marked (`mark`), then the components. Sectors
    /// not marked read only unchanged cells, so they are what a rebuild
    /// would give. Returns the number of sectors rebuilt.
    pub fn refresh(&mut self, open: &(impl Fn(u32, u32) -> bool + Sync)) -> usize {
        use rayon::prelude::*;
        if !self.any_dirty {
            return 0;
        }
        self.any_dirty = false;
        let list: Vec<usize> = (0..self.sectors.len()).filter(|&s| self.dirty[s]).collect();
        if list.is_empty() {
            // Touched but no cell changed: the graph already matches.
            return 0;
        }
        let _s = crate::prof_span!("hpa rebuild sectors", sectors = list.len());
        let this = &*self;
        let built: Vec<Sector> = if list.len() >= PARALLEL_MIN {
            list.par_iter()
                .map(|&s| {
                    let _s = crate::prof_span!("hpa sector (worker)");
                    this.build_sector(s, open)
                })
                .collect()
        } else {
            list.iter().map(|&s| this.build_sector(s, open)).collect()
        };
        for (&s, sector) in list.iter().zip(built) {
            self.sectors[s] = sector;
            self.dirty[s] = false;
        }
        self.node_base.clear();
        let mut total = 0u32;
        for sector in &self.sectors {
            self.node_base.push(total);
            total += sector.nodes.len() as u32;
        }
        self.node_base.push(total);
        self.components();
        list.len()
    }

    fn build_sector(&self, s: usize, open: &impl Fn(u32, u32) -> bool) -> Sector {
        let r = self.sector_rect(s);
        let (rw, rh) = (r.width(), r.height());
        // The rect's open cells, read once, with a closed ring round them
        // (row stride `pw`): the fill and the in-sector searches below then
        // need no bounds checks and no calls back into the grid.
        let pw = rw as usize + 2;
        let pad = |x: u32, z: u32| (z as usize + 1) * pw + x as usize + 1;
        let mut cells = vec![false; pw * (rh as usize + 2)];
        let mut full = true;
        for z in 0..rh {
            for x in 0..rw {
                let o = open(r.x0 + x, r.z0 + z);
                cells[pad(x, z)] = o;
                full &= o;
            }
        }
        // Local regions, 4-connected, labelled in scan order.
        let mut local = vec![0u16; (rw * rh) as usize];
        let mut regions = 0u16;
        let mut stack: Vec<(u32, u32)> = Vec::new();
        for z in 0..rh {
            for x in 0..rw {
                let i = (z * rw + x) as usize;
                if local[i] != 0 || !cells[pad(x, z)] {
                    continue;
                }
                regions += 1;
                local[i] = regions;
                stack.push((x, z));
                while let Some((cx, cz)) = stack.pop() {
                    for (nx, nz) in [(cx.wrapping_sub(1), cz), (cx + 1, cz), (cx, cz.wrapping_sub(1)), (cx, cz + 1)] {
                        if nx < rw && nz < rh {
                            let j = (nz * rw + nx) as usize;
                            if local[j] == 0 && cells[pad(nx, nz)] {
                                local[j] = regions;
                                stack.push((nx, nz));
                            }
                        }
                    }
                }
            }
        }
        // Portal nodes on the four borders (south, west, east, north).
        let mut nodes: Vec<Node> = Vec::new();
        let w = self.w;
        let mut add = |x: u32, z: u32, tx: u32, tz: u32| {
            let cell = z * w + x;
            let twin = tz * w + tx;
            match nodes.iter_mut().find(|n| n.cell == cell) {
                Some(n) => n.inter.push(twin),
                None => nodes.push(Node { cell, region: local[((z - r.z0) * rw + x - r.x0) as usize], intra: Vec::new(), inter: vec![twin] }),
            }
        };
        let mut border = |len: u32, here: &dyn Fn(u32) -> (u32, u32), there: &dyn Fn(u32) -> (u32, u32)| {
            let mut k = 0;
            while k < len {
                let both = |k: u32| {
                    let (a, b) = (here(k), there(k));
                    open(a.0, a.1) && open(b.0, b.1)
                };
                if !both(k) {
                    k += 1;
                    continue;
                }
                let start = k;
                while k < len && both(k) {
                    k += 1;
                }
                let run = k - start;
                // Short runs: one portal in the middle; long ones: one at
                // each end. (Portals every 8 cells along long runs gave
                // paths 2% shorter for 3.7x the search time.)
                let picks: &[u32] = if run > LONG_RUN { &[start, k - 1] } else { &[start + run / 2] };
                for &p in picks {
                    let (a, b) = (here(p), there(p));
                    add(a.0, a.1, b.0, b.1);
                }
            }
        };
        if r.z0 > 0 {
            border(rw, &|k| (r.x0 + k, r.z0), &|k| (r.x0 + k, r.z0 - 1));
        }
        if r.x0 > 0 {
            border(rh, &|k| (r.x0, r.z0 + k), &|k| (r.x0 - 1, r.z0 + k));
        }
        if r.x1 < self.w {
            border(rh, &|k| (r.x1 - 1, r.z0 + k), &|k| (r.x1, r.z0 + k));
        }
        if r.z1 < self.h {
            border(rw, &|k| (r.x0 + k, r.z1 - 1), &|k| (r.x0 + k, r.z1));
        }
        // In-sector costs between nodes of the same region: octile when the
        // sector is all open, else a Dijkstra from each node, stopped once
        // the later nodes of its region (the only costs it gives) are
        // settled: settled costs are final, so they are what a full search
        // gives.
        let xy = |cell: u32| (cell % w, cell / w);
        let at = |cell: u32| pad(cell % w - r.x0, cell / w - r.z0);
        let mut dist = Vec::new();
        let mut heap = BinaryHeap::new();
        // Node index by padded cell (u16::MAX: none).
        let mut node_at = vec![u16::MAX; if full { 0 } else { cells.len() }];
        if !full {
            for (k, n) in nodes.iter().enumerate() {
                node_at[at(n.cell)] = k as u16;
            }
        }
        for i in 0..nodes.len() {
            if !full {
                let later = nodes[i + 1..].iter().filter(|n| n.region == nodes[i].region).count();
                if later > 0 {
                    padded_dijkstra(&cells, pw, at(nodes[i].cell), &node_at, i as u16, later, &mut dist, &mut heap);
                }
            }
            for j in i + 1..nodes.len() {
                if nodes[i].region != nodes[j].region {
                    continue;
                }
                let ((ax, az), (bx, bz)) = (xy(nodes[i].cell), xy(nodes[j].cell));
                let cost = if full { octile(ax, az, bx, bz) } else { dist[at(nodes[j].cell)] };
                if cost == u32::MAX {
                    continue;
                }
                nodes[i].intra.push((j as u16, cost));
                nodes[j].intra.push((i as u16, cost));
            }
        }
        let mut by_cell: Vec<(u32, u16)> = nodes.iter().enumerate().map(|(i, n)| (n.cell, i as u16)).collect();
        by_cell.sort_unstable();
        Sector { local, comp: Vec::new(), nodes, by_cell, regions }
    }

    /// Connected components: local regions joined by portal links
    /// (union-find over (sector, region), ids in sector/region order).
    fn components(&mut self) {
        let _s = crate::prof_span!("hpa components");
        let mut offset = Vec::with_capacity(self.sectors.len() + 1);
        let mut total = 0usize;
        for s in &self.sectors {
            offset.push(total);
            total += s.regions as usize + 1;
        }
        let mut parent: Vec<u32> = (0..total as u32).collect();
        fn find(p: &mut [u32], mut i: u32) -> u32 {
            while p[i as usize] != i {
                p[i as usize] = p[p[i as usize] as usize];
                i = p[i as usize];
            }
            i
        }
        for (s, sector) in self.sectors.iter().enumerate() {
            for n in &sector.nodes {
                for &t in &n.inter {
                    let (tx, tz) = (t % self.w, t / self.w);
                    let ts = self.sector_of(tx, tz);
                    let tr = self.sector_rect(ts);
                    let tl = self.sectors[ts].local[((tz - tr.z0) * tr.width() + tx - tr.x0) as usize];
                    let (a, b) = (find(&mut parent, (offset[s] + n.region as usize) as u32), find(&mut parent, (offset[ts] + tl as usize) as u32));
                    if a != b {
                        let (lo, hi) = (a.min(b), a.max(b));
                        parent[hi as usize] = lo;
                    }
                }
            }
        }
        let mut id = vec![0u32; total];
        let mut next = 0u32;
        for s in 0..self.sectors.len() {
            let regions = self.sectors[s].regions as usize;
            // (Its buffer reused: a refresh on an 81 km map would otherwise
            // allocate one per sector.)
            let mut comp = std::mem::take(&mut self.sectors[s].comp);
            comp.clear();
            comp.resize(regions + 1, 0);
            for (k, c) in comp.iter_mut().enumerate().skip(1) {
                let root = find(&mut parent, (offset[s] + k) as u32) as usize;
                if id[root] == 0 {
                    next += 1;
                    id[root] = next;
                }
                *c = id[root];
            }
            self.sectors[s].comp = comp;
        }
    }

    /// Connected component of cell `(x, z)` (0 = closed, or not fresh).
    pub fn component(&self, x: u32, z: u32) -> u32 {
        if self.any_dirty || x >= self.w || z >= self.h {
            return 0;
        }
        let s = self.sector_of(x, z);
        let r = self.sector_rect(s);
        let sector = &self.sectors[s];
        let l = sector.local[((z - r.z0) * r.width() + x - r.x0) as usize];
        sector.comp.get(l as usize).copied().unwrap_or(0)
    }

    fn node_of(&self, cell: u32) -> Option<(usize, usize)> {
        let s = self.sector_of(cell % self.w, cell / self.w);
        let by_cell = &self.sectors[s].by_cell;
        by_cell.binary_search_by_key(&cell, |&(c, _)| c).ok().map(|k| (s, by_cell[k].1 as usize))
    }

    /// Dense index (see `node_base`) of the graph node at `cell`.
    pub fn node_index(&self, cell: u32) -> Option<usize> {
        self.node_of(cell).map(|(s, i)| self.node_base[s] as usize + i)
    }

    /// Cost to `goal` from every graph node that can reach it: a Dijkstra
    /// over the whole graph, seeded with the goal sector's in-sector costs
    /// (flow fields, `flow`). By dense node index (`node_index`;
    /// `u32::MAX` = can't reach it). Ties pop by cell, as before the
    /// dense layout.
    pub fn costs_to(&self, open: &impl Fn(u32, u32) -> bool, scratch: &mut Scratch, goal: (u32, u32), work: &mut u64) -> Vec<u32> {
        let _s = crate::prof_span!("hpa costs_to");
        let w = self.w;
        let Scratch { dist, heap, .. } = scratch;
        let gs = self.sector_of(goal.0, goal.1);
        let gr = self.sector_rect(gs);
        *work += dijkstra(gr, open, goal, dist, heap) as u64 * 8;
        let at = |cell: u32| dist[((cell / w - gr.z0) * gr.width() + cell % w - gr.x0) as usize];
        let base = &self.node_base;
        let mut cost = vec![u32::MAX; base.last().copied().unwrap_or(0) as usize];
        // (cost, cell, sector, node).
        let mut queue: BinaryHeap<(Reverse<u32>, Reverse<u32>, u32, u16)> = BinaryHeap::new();
        for (i, n) in self.sectors[gs].nodes.iter().enumerate() {
            let c = at(n.cell);
            if c != u32::MAX {
                cost[base[gs] as usize + i] = c;
                queue.push((Reverse(c), Reverse(n.cell), gs as u32, i as u16));
            }
        }
        while let Some((Reverse(c), Reverse(_), s, i)) = queue.pop() {
            let (s, i) = (s as usize, i as usize);
            if cost[base[s] as usize + i] < c {
                continue;
            }
            *work += 8;
            let node = &self.sectors[s].nodes[i];
            let mut relax = |ts: usize, ti: usize, step: u32| {
                let nc = c + step;
                let k = base[ts] as usize + ti;
                if nc < cost[k] {
                    cost[k] = nc;
                    queue.push((Reverse(nc), Reverse(self.sectors[ts].nodes[ti].cell), ts as u32, ti as u16));
                }
            };
            for &(j, step) in &node.intra {
                relax(s, j as usize, step);
            }
            for &t in &node.inter {
                if let Some((ts, ti)) = self.node_of(t) {
                    relax(ts, ti, 10);
                }
            }
        }
        cost
    }

    /// In-sector costs of the sector holding `(x, z)` from `seeds` (cell,
    /// cost): `out` row-major in its rect (`u32::MAX` = unreached). Returns
    /// the cells settled.
    pub fn sector_costs(&self, open: &impl Fn(u32, u32) -> bool, x: u32, z: u32, seeds: &[((u32, u32), u32)], scratch: &mut Scratch, out: &mut Vec<u32>) -> u32 {
        let r = self.rect_of(x, z);
        dijkstra_from(r, open, seeds, out, &mut scratch.heap)
    }

    /// The node cells of the sector holding cell `(x, z)`, and its rect.
    pub fn sector_nodes(&self, x: u32, z: u32) -> (Rect, impl Iterator<Item = u32> + '_) {
        let s = self.sector_of(x, z);
        (self.sector_rect(s), self.sectors[s].nodes.iter().map(|n| n.cell))
    }

    /// Sector index of cell `(x, z)` (row-major over sectors).
    pub fn sector_index(&self, x: u32, z: u32) -> u32 {
        self.sector_of(x, z) as u32
    }

    /// Node cells (global indices) from `start` to `goal`, both included:
    /// consecutive cells are either in one sector or twins across a border.
    /// `None` if the goal isn't reached within `max_expand` node expansions.
    /// `work` grows by the cells and nodes looked at.
    pub fn search(&self, open: &impl Fn(u32, u32) -> bool, scratch: &mut Scratch, start: (u32, u32), goal: (u32, u32), max_expand: u32, work: &mut u64) -> Option<Vec<u32>> {
        let _s = crate::prof_span!("hpa search");
        let w = self.w;
        let (s_cell, g_cell) = (start.1 * w + start.0, goal.1 * w + goal.0);
        let (ss, gs) = (self.sector_of(start.0, start.1), self.sector_of(goal.0, goal.1));
        let Scratch { dist, heap, nodes, open: queue } = scratch;
        // Start: in-sector costs to its sector's nodes (and to the goal if
        // it's in the same sector).
        let sr = self.sector_rect(ss);
        *work += dijkstra(sr, open, start, dist, heap) as u64 * 8;
        let at = |r: Rect, cell: u32, dist: &[u32]| dist[((cell / w - r.z0) * r.width() + cell % w - r.x0) as usize];
        let start_edges: Vec<(u32, u32)> =
            self.sectors[ss].nodes.iter().map(|n| (n.cell, at(sr, n.cell, dist))).filter(|e| e.1 != u32::MAX).collect();
        let direct = if ss == gs { Some(at(sr, g_cell, dist)).filter(|&c| c != u32::MAX) } else { None };
        // Goal: in-sector costs from its sector's nodes (symmetric).
        let gr = self.sector_rect(gs);
        *work += dijkstra(gr, open, goal, dist, heap) as u64 * 8;
        let goal_costs: Vec<u32> = self.sectors[gs].nodes.iter().map(|n| at(gr, n.cell, dist)).collect();

        let h = |cell: u32| octile(cell % w, cell / w, goal.0, goal.1);
        nodes.clear();
        queue.clear();
        nodes.insert(s_cell, (0, s_cell, false));
        queue.push((Reverse(h(s_cell)), Reverse(h(s_cell)), Reverse(s_cell)));
        let mut expanded = 0u32;
        let relax = |nodes: &mut CellMap<(u32, u32, bool)>, queue: &mut BinaryHeap<_>, from: u32, g: u32, to: u32, cost: u32| {
            let ng = g + cost;
            let e = nodes.entry(to).or_insert((u32::MAX, to, false));
            if !e.2 && ng < e.0 {
                *e = (ng, from, false);
                let hh = h(to);
                queue.push((Reverse(ng + hh), Reverse(hh), Reverse(to)));
            }
        };
        while let Some((_, _, Reverse(c))) = queue.pop() {
            let (g, _, closed) = nodes[&c];
            if closed {
                continue;
            }
            nodes.get_mut(&c).expect("queued").2 = true;
            expanded += 1;
            *work += 8;
            if c == g_cell {
                let mut path = vec![c];
                let mut cur = c;
                while cur != s_cell {
                    cur = nodes[&cur].1;
                    path.push(cur);
                }
                path.reverse();
                return Some(path);
            }
            if expanded >= max_expand {
                return None;
            }
            if c == s_cell {
                for &(n, cost) in &start_edges {
                    relax(nodes, queue, c, g, n, cost);
                }
                if let Some(cost) = direct {
                    relax(nodes, queue, c, g, g_cell, cost);
                }
            }
            if let Some((s, i)) = self.node_of(c) {
                let node = &self.sectors[s].nodes[i];
                for &(j, cost) in &node.intra {
                    relax(nodes, queue, c, g, self.sectors[s].nodes[j as usize].cell, cost);
                }
                for &t in &node.inter {
                    relax(nodes, queue, c, g, t, 10);
                }
                if s == gs && goal_costs[i] != u32::MAX {
                    relax(nodes, queue, c, g, g_cell, goal_costs[i]);
                }
            }
        }
        None
    }
}

/// [`dijkstra`] over a sector's padded open cells (`cells`, rows of `pw`,
/// a closed ring round the rect) from padded cell `from`, stopping once
/// `targets` cells with a node index (`node_at`) above `after` are settled.
/// `dist` is by padded cell; costs settled before the stop are exact.
#[allow(clippy::too_many_arguments)]
fn padded_dijkstra(cells: &[bool], pw: usize, from: usize, node_at: &[u16], after: u16, mut targets: usize, dist: &mut Vec<u32>, heap: &mut BinaryHeap<(Reverse<u32>, u32)>) {
    dist.clear();
    dist.resize(cells.len(), u32::MAX);
    heap.clear();
    dist[from] = 0;
    heap.push((Reverse(0), from as u32));
    while let Some((Reverse(d), i)) = heap.pop() {
        let i = i as usize;
        if d > dist[i] {
            continue;
        }
        let k = node_at[i];
        if k != u16::MAX && k > after {
            targets -= 1;
            if targets == 0 {
                return;
            }
        }
        // (The ring is closed, so every neighbour index is in the buffer.)
        let (w, e, s, n) = (i - 1, i + 1, i - pw, i + pw);
        for (j, step) in [(w, 10), (e, 10), (s, 10), (n, 10)] {
            if cells[j] && d + step < dist[j] {
                dist[j] = d + step;
                heap.push((Reverse(d + step), j as u32));
            }
        }
        for (j, a, b) in [(s - 1, s, w), (s + 1, s, e), (n - 1, n, w), (n + 1, n, e)] {
            if cells[j] && cells[a] && cells[b] && d + 14 < dist[j] {
                dist[j] = d + 14;
                heap.push((Reverse(d + 14), j as u32));
            }
        }
    }
}

/// Dijkstra over the open cells of `r` from `from` (8-connected, no corner
/// cutting, 10 / 14): `dist` holds every cell's cost (row-major in `r`;
/// `u32::MAX` = unreached). Returns the cells settled.
fn dijkstra(r: Rect, open: &impl Fn(u32, u32) -> bool, from: (u32, u32), dist: &mut Vec<u32>, heap: &mut BinaryHeap<(Reverse<u32>, u32)>) -> u32 {
    dijkstra_from(r, open, &[(from, 0)], dist, heap)
}

/// [`dijkstra`] from several cells of `r`, each starting at its own cost.
fn dijkstra_from(r: Rect, open: &impl Fn(u32, u32) -> bool, seeds: &[((u32, u32), u32)], dist: &mut Vec<u32>, heap: &mut BinaryHeap<(Reverse<u32>, u32)>) -> u32 {
    let (rw, rh) = (r.width(), r.height());
    dist.clear();
    dist.resize((rw * rh) as usize, u32::MAX);
    heap.clear();
    let idx = |x: u32, z: u32| ((z - r.z0) * rw + x - r.x0) as usize;
    for &((x, z), cost) in seeds {
        let i = idx(x, z);
        if cost < dist[i] {
            dist[i] = cost;
            heap.push((Reverse(cost), i as u32));
        }
    }
    let ok = |x: u32, z: u32| r.contains(x, z) && open(x, z);
    let mut settled = 0;
    while let Some((Reverse(d), i)) = heap.pop() {
        if d > dist[i as usize] {
            continue;
        }
        settled += 1;
        let (x, z) = (r.x0 + i % rw, r.z0 + i / rw);
        for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (nx, nz) = (x.wrapping_add_signed(dx), z.wrapping_add_signed(dz));
            if !ok(nx, nz) {
                continue;
            }
            let diagonal = dx != 0 && dz != 0;
            if diagonal && (!ok(nx, z) || !ok(x, nz)) {
                continue;
            }
            let nd = d + if diagonal { 14 } else { 10 };
            let j = idx(nx, nz);
            if nd < dist[j] {
                dist[j] = nd;
                heap.push((Reverse(nd), j as u32));
            }
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every in-sector edge is the exact shortest in-sector cost (a full
    /// Dijkstra over the grid), on grids from open to mostly closed, with
    /// sectors cut short by the map edge.
    #[test]
    fn sector_edges_are_exact_in_sector_costs() {
        let mut rng = crate::engine::sim::rng::SimRng::new(7);
        for density in [0u64, 5, 20, 40, 60] {
            let (w, h) = (80u32, 70u32);
            let cells: Vec<bool> = (0..w * h).map(|_| rng.next_u64() % 100 >= density).collect();
            let open = |x: u32, z: u32| x < w && z < h && cells[(z * w + x) as usize];
            let mut g = Hier::new(w, h);
            g.refresh(&open);
            let (mut dist, mut heap) = (Vec::new(), BinaryHeap::new());
            for (s, sector) in g.sectors.iter().enumerate() {
                let r = g.sector_rect(s);
                for (i, a) in sector.nodes.iter().enumerate() {
                    dijkstra(r, &open, (a.cell % w, a.cell / w), &mut dist, &mut heap);
                    for (j, b) in sector.nodes.iter().enumerate() {
                        let want = dist[((b.cell / w - r.z0) * r.width() + b.cell % w - r.x0) as usize];
                        let got = a.intra.iter().find(|e| e.0 as usize == j).map(|e| e.1);
                        if i == j || want == u32::MAX {
                            assert_eq!(got, None);
                        } else {
                            assert_eq!(got, Some(want), "density {density} sector {s} nodes {i} -> {j}");
                        }
                    }
                }
            }
        }
    }
}
