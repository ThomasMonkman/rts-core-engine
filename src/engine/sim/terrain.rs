//! Terrain heightfield in fixed-point (deterministic height queries).

use crate::engine::assets::scmap::Heightmap;
use crate::engine::sim::math::Fx;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Terrain {
    /// Size in cells (world units); samples are `(width + 1) x (height + 1)`.
    pub width: u32,
    pub height: u32,
    /// Sample heights pre-multiplied by the map's height scale.
    heights: Vec<Fx>,
    /// Water surface elevation, if the map has water.
    pub water_surface: Option<Fx>,
    /// Fraction of height samples below the water surface (0 without water).
    pub water_ratio: f64,
    /// Terrain type code per cell (`width * height`), see /lua/TerrainTypes.lua.
    types: Vec<u8>,
    /// Bumped on every height change (the renderer re-uploads on change).
    pub version: u32,
}

impl Terrain {
    pub fn from_heightmap(hm: &Heightmap) -> Self {
        // Scale is a dyadic f32 (1/128 on stock maps), so this is exact.
        let scale = Fx::from_num(hm.scale);
        Self {
            width: hm.width,
            height: hm.height,
            heights: hm.samples.iter().map(|&s| Fx::from_num(s) * scale).collect(),
            water_surface: None,
            water_ratio: 0.0,
            types: Vec::new(),
            version: 0,
        }
    }

    pub fn with_types(mut self, types: Vec<u8>) -> Self {
        debug_assert!(types.is_empty() || types.len() == (self.width * self.height) as usize);
        self.types = types;
        self
    }

    /// Terrain type code of the cell containing `(x, z)`; 1 = `Default`
    /// off the map (the real game's `GetTerrainType(-1, -1)` and
    /// `(1024, 1024)` on a 1024 map are `Default`, not the edge cell's type:
    /// dev/probes probe_api_spot, probe_ai_brain) or when the map has no type
    /// data.
    pub fn type_at(&self, x: Fx, z: Fx) -> u8 {
        if self.types.is_empty() || x < Fx::ZERO || z < Fx::ZERO {
            return 1;
        }
        let (ix, iz) = (x.int().to_num::<u64>(), z.int().to_num::<u64>());
        if ix >= u64::from(self.width) || iz >= u64::from(self.height) {
            return 1;
        }
        self.types[(iz as u32 * self.width + ix as u32) as usize]
    }

    pub fn with_water(mut self, surface: Option<f32>) -> Self {
        self.water_surface = surface.map(Fx::saturating_from_num);
        self.water_ratio = match self.water_surface {
            Some(w) => {
                let below = self.heights.iter().filter(|&&h| h < w).count();
                below as f64 / self.heights.len() as f64
            }
            None => 0.0,
        };
        self
    }

    /// Terrain height, or the water surface where it is higher.
    pub fn surface_at(&self, x: Fx, z: Fx) -> Fx {
        let h = self.height_at(x, z);
        match self.water_surface {
            Some(w) if w > h => w,
            _ => h,
        }
    }

    fn sample(&self, x: u32, z: u32) -> Fx {
        let x = x.min(self.width);
        let z = z.min(self.height);
        self.heights[(z * (self.width + 1) + x) as usize]
    }

    /// Sets every height sample inside the world rectangle `[x0, x1] x
    /// [z0, z1]` (clamped to the map) to `elevation` (`FlattenMapRect`).
    /// Heights as floats for the renderer (`(width + 1) x (height + 1)`).
    pub fn heights_f32(&self) -> Vec<f32> {
        self.heights.iter().map(|h| h.to_num::<f32>()).collect()
    }

    pub fn flatten(&mut self, x0: Fx, z0: Fx, x1: Fx, z1: Fx, elevation: Fx) {
        self.version = self.version.wrapping_add(1);
        let sample = |v: Fx, max: u32| v.max(Fx::ZERO).min(Fx::from_num(max));
        let (sx0, sx1) = (sample(x0, self.width).ceil().to_num::<u32>(), sample(x1, self.width).floor().to_num::<u32>());
        let (sz0, sz1) = (sample(z0, self.height).ceil().to_num::<u32>(), sample(z1, self.height).floor().to_num::<u32>());
        for z in sz0..=sz1 {
            for x in sx0..=sx1 {
                self.heights[(z * (self.width + 1) + x) as usize] = elevation;
            }
        }
    }

    /// Lowest and highest height sample over the vertices `x0..=x1`,
    /// `z0..=z1` (clamped to the map), or with `edge` over the ring of
    /// vertices just outside that rectangle: what moho's build placement
    /// holds to a blueprint's `MaxGroundVariation` (`OCCUPY_CheckAreaFlatness`
    /// / `OCCUPY_CheckEdgeFlatness`, MohoEngine.dll 0x101539c0 / 0x10153750).
    pub fn height_range(&self, x0: i32, z0: i32, x1: i32, z1: i32, edge: bool) -> (Fx, Fx) {
        let (w, h) = (self.width as i32, self.height as i32);
        let at = |x: i32, z: i32| self.sample(x.clamp(0, w) as u32, z.clamp(0, h) as u32);
        let (mut lo, mut hi) = (Fx::MAX, Fx::MIN);
        let mut see = |v: Fx| {
            lo = lo.min(v);
            hi = hi.max(v);
        };
        if edge {
            for x in x0 - 1..=x1 + 1 {
                see(at(x, z0 - 1));
                see(at(x, z1 + 1));
            }
            for z in z0..=z1 {
                see(at(x0 - 1, z));
                see(at(x1 + 1, z));
            }
        } else {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    see(at(x, z));
                }
            }
        }
        (lo, hi)
    }

    /// Bilinearly interpolated terrain height at world `(x, z)`; positions
    /// outside the map are clamped to the edge.
    pub fn height_at(&self, x: Fx, z: Fx) -> Fx {
        let x = x.clamp(Fx::ZERO, Fx::from_num(self.width));
        let z = z.clamp(Fx::ZERO, Fx::from_num(self.height));
        let (ix, iz) = (x.int().to_num::<u32>(), z.int().to_num::<u32>());
        let (fx, fz) = (x.frac(), z.frac());
        let h00 = self.sample(ix, iz);
        let h10 = self.sample(ix + 1, iz);
        let h01 = self.sample(ix, iz + 1);
        let h11 = self.sample(ix + 1, iz + 1);
        let top = h00 + (h10 - h00) * fx;
        let bottom = h01 + (h11 - h01) * fx;
        top + (bottom - top) * fz
    }

    /// The terrain's up-facing unit normal at world `(x, z)`, from the
    /// heights half a unit either side on each axis (`CHeightField::GetNormal`,
    /// MohoEngine.dll 0x1004c9f0).
    pub fn normal_at(&self, x: Fx, z: Fx) -> crate::engine::sim::math::FxVec3 {
        let half = Fx::from_num(0.5);
        let n = crate::engine::sim::math::FxVec3 {
            x: self.height_at(x - half, z) - self.height_at(x + half, z),
            y: Fx::ONE,
            z: self.height_at(x, z - half) - self.height_at(x, z + half),
        };
        n.normalized().unwrap_or(crate::engine::sim::math::FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO })
    }
}

/// Shape of a shot for line-of-fire checks (`CheckBlockingTerrain`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FireArc {
    /// Straight line.
    None,
    Low,
    High,
}

impl FireArc {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "low" => FireArc::Low,
            "high" => FireArc::High,
            _ => FireArc::None,
        }
    }

    /// Apex height above the chord as a fraction of the horizontal distance:
    /// a parabola leaving at ~31 degrees (low) or ~63 degrees (high) relative
    /// to the chord. Inferred; moho's arcs aren't documented.
    fn apex(self) -> Fx {
        match self {
            FireArc::None => Fx::ZERO,
            FireArc::Low => Fx::from_num(0.15),
            FireArc::High => Fx::from_num(0.5),
        }
    }
}

impl Terrain {
    /// Does the terrain rise above the shot from `from` to `to`? Samples the
    /// path once per world unit (the heightmap resolution), skipping the
    /// endpoints themselves.
    pub fn blocks(&self, from: crate::engine::sim::math::FxVec3, to: crate::engine::sim::math::FxVec3, arc: FireArc) -> bool {
        let (dx, dz) = (to.x - from.x, to.z - from.z);
        let horizontal = crate::engine::sim::math::sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
        let samples = horizontal.ceil().to_num::<u32>();
        if samples < 2 {
            return false;
        }
        let apex = horizontal * arc.apex();
        let n = Fx::from_num(samples);
        (1..samples).any(|i| {
            let t = Fx::from_num(i) / n;
            let y = from.y + (to.y - from.y) * t + apex * 4 * t * (Fx::ONE - t);
            self.height_at(from.x + dx * t, from.z + dz * t) > y
        })
    }

    /// Does the terrain block a weapon's shot at a target candidate
    /// (`AI_TestForTerrainBlockage`, MohoEngine.dll 0x101c38d0)? `from` is
    /// the shooter's position raised by twice its `SizeZ`, `to` the
    /// candidate's position raised by 0.25 (the caller's part). A straight
    /// weapon (`RULEUBA_None`) tests the one segment; arcs test 4 equal steps,
    /// each raised by k x {0.707, 0.293, -0.293, -0.707} x the step's length
    /// (k 0.5 low, 2 high), so the path bulges above the chord by an eighth
    /// (low) or half (high) of the distance at its middle.
    pub fn blocks_fire(&self, from: crate::engine::sim::math::FxVec3, to: crate::engine::sim::math::FxVec3, arc: FireArc) -> bool {
        use crate::engine::sim::math::FxVec3;
        let k = match arc {
            FireArc::None => return self.segment_hits(from, to),
            FireArc::Low => Fx::from_num(0.5),
            FireArc::High => Fx::from_num(2),
        };
        let quarter = Fx::from_num(0.25);
        let step = FxVec3 { x: (to.x - from.x) * quarter, y: (to.y - from.y) * quarter, z: (to.z - from.z) * quarter };
        let length = step.length();
        let mut at = from;
        for bulge in [0.707, 0.293, -0.293, -0.707] {
            let next = FxVec3 { x: at.x + step.x, y: at.y + step.y + Fx::from_num(bulge) * length * k, z: at.z + step.z };
            if self.segment_hits(at, next) {
                return true;
            }
            at = next;
        }
        false
    }

    /// Does the terrain rise above the segment anywhere (moho's exact
    /// heightfield intersection; ours samples every half heightmap cell,
    /// the ends included)?
    fn segment_hits(&self, from: crate::engine::sim::math::FxVec3, to: crate::engine::sim::math::FxVec3) -> bool {
        let (dx, dz) = (to.x - from.x, to.z - from.z);
        let horizontal = crate::engine::sim::math::sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
        let samples = (horizontal * 2).ceil().to_num::<u32>().max(1);
        let n = Fx::from_num(samples);
        (0..=samples).any(|i| {
            let t = Fx::from_num(i) / n;
            self.height_at(from.x + dx * t, from.z + dz * t) > from.y + (to.y - from.y) * t
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_type_off_the_map_is_default() {
        let t = Terrain::from_heightmap(&Heightmap { width: 4, height: 4, scale: 1.0 / 128.0, samples: vec![0u16; 25] }).with_types(vec![9; 16]);
        let f = |v: f64| Fx::from_num(v);
        assert_eq!(t.type_at(f(0.0), f(0.0)), 9);
        assert_eq!(t.type_at(f(3.9), f(3.9)), 9);
        assert_eq!(t.type_at(f(-1.0), f(-1.0)), 1);
        assert_eq!(t.type_at(f(-0.5), f(2.0)), 1);
        assert_eq!(t.type_at(f(4.0), f(4.0)), 1);
        assert_eq!(t.type_at(f(2.0), f(100.0)), 1);
    }

    #[test]
    fn hill_blocks_direct_fire_but_not_high_arcs() {
        // 33x33 flat map with a 10-high ridge across x = 16.
        let mut samples = vec![0u16; 33 * 33];
        for z in 0..33 {
            samples[z * 33 + 16] = 10 * 128;
        }
        let t = Terrain::from_heightmap(&Heightmap { width: 32, height: 32, scale: 1.0 / 128.0, samples });
        let (a, b) = (crate::engine::sim::math::FxVec3::from_f64(2.0, 1.0, 10.0), crate::engine::sim::math::FxVec3::from_f64(30.0, 1.0, 10.0));
        assert!(t.blocks(a, b, FireArc::None));
        assert!(t.blocks(a, b, FireArc::Low), "low arc apex 4.2 < 10");
        assert!(!t.blocks(a, b, FireArc::High), "high arc apex 14 > 10");
        let (c, d) = (crate::engine::sim::math::FxVec3::from_f64(2.0, 1.0, 10.0), crate::engine::sim::math::FxVec3::from_f64(12.0, 1.0, 10.0));
        assert!(!t.blocks(c, d, FireArc::None), "short of the ridge");
    }

    #[test]
    fn weapon_blockage_follows_mohos_lines() {
        // 33x33 flat map with a 3-high ridge across x = 16.
        let mut samples = vec![0u16; 33 * 33];
        for z in 0..33 {
            samples[z * 33 + 16] = 3 * 128;
        }
        let t = Terrain::from_heightmap(&Heightmap { width: 32, height: 32, scale: 1.0 / 128.0, samples });
        let at = |x: f64, y: f64| crate::engine::sim::math::FxVec3::from_f64(x, y, 10.0);
        // A tank 2.2 up (2 x SizeZ 1.1) at a target 0.25 up, 12 away.
        let (from, to) = (at(10.0, 2.2), at(22.0, 0.25));
        assert!(t.blocks_fire(from, to, FireArc::None), "the chord passes 1.6 over the ridge's foot");
        assert!(t.blocks_fire(from, to, FireArc::Low), "a low arc rises 1.5 at its middle: still under 3");
        assert!(!t.blocks_fire(from, to, FireArc::High), "a high arc rises 6");
        assert!(!t.blocks_fire(at(2.0, 2.2), at(12.0, 0.25), FireArc::None), "short of the ridge");
        // Long enough, a low arc clears it: 32 away it rises 4 at its middle.
        assert!(!t.blocks_fire(at(0.0, 2.2), at(32.0, 0.25), FireArc::Low));
    }

    #[test]
    fn height_range_over_an_area_or_the_ring_around_it() {
        // 8x8 flat at 0 with a 5-high spike at (4, 4).
        let mut samples = vec![0u16; 9 * 9];
        samples[4 * 9 + 4] = 5 * 128;
        let t = Terrain::from_heightmap(&Heightmap { width: 8, height: 8, scale: 1.0 / 128.0, samples });
        assert_eq!(t.height_range(3, 3, 5, 5, false), (Fx::ZERO, Fx::from_num(5)), "the spike is inside");
        assert_eq!(t.height_range(3, 3, 5, 5, true), (Fx::ZERO, Fx::ZERO), "but not on the ring around it");
        assert_eq!(t.height_range(5, 5, 6, 6, true), (Fx::ZERO, Fx::from_num(5)), "the ring of a rect beside it touches it");
        // Off the map: clamped to the edge.
        assert_eq!(t.height_range(-3, -3, 0, 0, false), (Fx::ZERO, Fx::ZERO));
    }

    #[test]
    fn bilinear_and_clamped() {
        // 1x1 cell: heights 0, 128, 256, 384 (samples * 1/128 scale = 0,1,2,3).
        let t = Terrain::from_heightmap(&Heightmap {
            width: 1,
            height: 1,
            scale: 1.0 / 128.0,
            samples: vec![0, 128, 256, 384],
        });
        assert_eq!(t.height_at(Fx::ZERO, Fx::ZERO), Fx::ZERO);
        assert_eq!(t.height_at(Fx::from_num(0.5), Fx::from_num(0.5)), Fx::from_num(1.5));
        assert_eq!(t.height_at(Fx::from_num(5), Fx::from_num(5)), Fx::from_num(3));
        let t = t.with_water(Some(2.0));
        assert_eq!(t.water_ratio, 0.5);
        assert_eq!(t.surface_at(Fx::ZERO, Fx::ZERO), Fx::from_num(2));
        assert_eq!(t.surface_at(Fx::from_num(1), Fx::from_num(1)), Fx::from_num(3));
    }
}
