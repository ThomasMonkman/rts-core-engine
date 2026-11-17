//! Swept collision tests in fixed point: a projectile's path over one tick
//! (a segment) against an entity's collision volume.

use crate::engine::sim::entity::CollisionShape;
use crate::engine::sim::math::{sqrt, Fx, FxQuat, FxVec3};

fn dot(a: FxVec3, b: FxVec3) -> Fx {
    a.x.saturating_mul(b.x).saturating_add(a.y.saturating_mul(b.y)).saturating_add(a.z.saturating_mul(b.z))
}

/// Where along `p0 -> p1` (0..=1) the segment first touches `shape` placed at
/// `pos` / `orientation`; `Some(0)` if it starts inside, `None` if it misses.
pub fn segment_hit(p0: FxVec3, p1: FxVec3, pos: FxVec3, orientation: FxQuat, shape: &CollisionShape) -> Option<Fx> {
    match *shape {
        CollisionShape::None => None,
        CollisionShape::Sphere { center, radius } => {
            let c = pos + orientation.rotate(center);
            let (d, m) = (p1 - p0, p0 - c);
            let r2 = radius.saturating_mul(radius);
            let cc = dot(m, m).saturating_sub(r2);
            if cc <= Fx::ZERO {
                return Some(Fx::ZERO);
            }
            let a = dot(d, d);
            let b = dot(m, d);
            // b > 0: moving away from the centre while outside.
            if a == Fx::ZERO || b >= Fx::ZERO {
                return None;
            }
            // Roots of a t² + 2 b t + cc: t = (-b - sqrt(b² - a cc)) / a.
            let disc = b.saturating_mul(b).saturating_sub(a.saturating_mul(cc));
            if disc < Fx::ZERO {
                return None;
            }
            let t = (-b - sqrt(disc)).saturating_div(a);
            (t <= Fx::ONE).then_some(t.max(Fx::ZERO))
        }
        CollisionShape::Box { center, half } => {
            // Into the box's frame: undo the entity rotation, then the offset.
            let inv = orientation.conjugate();
            let l0 = inv.rotate(p0 - pos) - center;
            let l1 = inv.rotate(p1 - pos) - center;
            let d = l1 - l0;
            let (mut t_min, mut t_max) = (Fx::ZERO, Fx::ONE);
            for (o, dir, h) in [(l0.x, d.x, half.x), (l0.y, d.y, half.y), (l0.z, d.z, half.z)] {
                if dir == Fx::ZERO {
                    if o < -h || o > h {
                        return None;
                    }
                    continue;
                }
                // Saturating: a near-parallel segment's entry/exit is
                // effectively at infinity (plain division overflows).
                let (mut a, mut b) = ((-h - o).saturating_div(dir), (h - o).saturating_div(dir));
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                t_min = t_min.max(a);
                t_max = t_max.min(b);
                if t_min > t_max {
                    return None;
                }
            }
            Some(t_min)
        }
    }
}

/// Blast radius above which a collision sphere is tested per axis plane
/// ([`sphere_touches`]).
const PLANAR_SPHERE_TEST_ABOVE: Fx = Fx::from_bits(3 << 32);

/// Whether the sphere `c` / `r` touches `shape` placed at `pos` /
/// `orientation`: moho's area-damage test (`DamageArea` gathers entities
/// whose collision shape meets the sphere, MohoEngine.dll 0x102f5780).
///
/// A collision sphere (a unit's, a shield bubble's) is met exactly by a
/// blast of radius 3 or less. A larger blast meets it when, in each of the
/// axis planes xy, yz and xz, its centre's projection is within `r` of the
/// sphere's projected square (half-size its radius): FAF probes
/// `probe_re3_wpn_b`/`_d`, bisected to 1/128, with the switch between radius
/// 3 and 3.25. An air scout's sphere of 1.6 is reached at 4.594 every way by
/// a blast of 3, but by one of 5 at 6.594 along an axis, 7.258 diagonally
/// in xz and 8.891 along (1, 1, 1).
pub fn sphere_touches(c: FxVec3, r: Fx, pos: FxVec3, orientation: FxQuat, shape: &CollisionShape) -> bool {
    match *shape {
        CollisionShape::None => false,
        CollisionShape::Sphere { center, radius } => {
            let d = c - (pos + orientation.rotate(center));
            if r > PLANAR_SPHERE_TEST_ABOVE {
                let out = |v: Fx| (v.abs() - radius).max(Fx::ZERO);
                let (x, y, z) = (out(d.x), out(d.y), out(d.z));
                let sq = |v: Fx| v.saturating_mul(v);
                let r2 = sq(r);
                return sq(x).saturating_add(sq(y)) <= r2 && sq(y).saturating_add(sq(z)) <= r2 && sq(x).saturating_add(sq(z)) <= r2;
            }
            let reach = r.saturating_add(radius);
            dot(d, d) <= reach.saturating_mul(reach)
        }
        CollisionShape::Box { center, half } => {
            // The sphere's centre in the box's frame, then its distance to
            // the nearest point of the box.
            let l = orientation.conjugate().rotate(c - pos) - center;
            let d = FxVec3 { x: l.x - l.x.clamp(-half.x, half.x), y: l.y - l.y.clamp(-half.y, half.y), z: l.z - l.z.clamp(-half.z, half.z) };
            dot(d, d) <= r.saturating_mul(r)
        }
    }
}

/// Conservative XZ reach of `shape` from the entity origin (any rotation):
/// how far beyond its position an entity can be hit.
/// The shape's centre in the entity's frame (origin for no shape).
pub fn center_of(shape: &CollisionShape) -> FxVec3 {
    match shape {
        CollisionShape::Box { center, .. } | CollisionShape::Sphere { center, .. } => *center,
        CollisionShape::None => FxVec3::default(),
    }
}

pub fn reach(shape: &CollisionShape) -> Fx {
    match *shape {
        CollisionShape::None => Fx::ZERO,
        CollisionShape::Sphere { center, radius } => center.x.abs() + center.z.abs() + center.y.abs() + radius,
        CollisionShape::Box { center, half } => {
            center.x.abs() + center.y.abs() + center.z.abs() + half.x + half.y + half.z
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> FxVec3 {
        FxVec3::from_f64(x, y, z)
    }

    #[test]
    fn sphere_touches_boxes_and_spheres() {
        // A 2-tall box on the ground: a small sphere on its top touches it,
        // though the entity origin (its foot) is out of the sphere's reach.
        let shape = CollisionShape::Box { center: v(0.0, 1.0, 0.0), half: v(1.0, 1.0, 1.0) };
        assert!(sphere_touches(v(0.0, 2.2, 0.0), Fx::from_num(0.5), FxVec3::default(), FxQuat::IDENTITY, &shape));
        assert!(!sphere_touches(v(0.0, 2.6, 0.0), Fx::from_num(0.5), FxVec3::default(), FxQuat::IDENTITY, &shape));
        // A corner is further than a face.
        assert!(!sphere_touches(v(1.4, 2.4, 1.4), Fx::from_num(0.5), FxVec3::default(), FxQuat::IDENTITY, &shape));
        // Turned 45 degrees about y, the box's corner reaches out along x.
        let q = FxQuat::from_hpr(Fx::FRAC_PI_4, Fx::ZERO, Fx::ZERO);
        assert!(sphere_touches(v(1.6, 1.0, 0.0), Fx::from_num(0.3), FxVec3::default(), q, &shape));
        assert!(!sphere_touches(v(1.6, 1.0, 0.0), Fx::from_num(0.3), FxVec3::default(), FxQuat::IDENTITY, &shape));
        let ball = CollisionShape::Sphere { center: v(0.0, 1.0, 0.0), radius: Fx::ONE };
        assert!(sphere_touches(v(0.0, 2.4, 0.0), Fx::from_num(0.5), FxVec3::default(), FxQuat::IDENTITY, &ball));
        assert!(!sphere_touches(v(0.0, 2.6, 0.0), Fx::from_num(0.5), FxVec3::default(), FxQuat::IDENTITY, &ball));
        assert!(!sphere_touches(FxVec3::default(), Fx::from_num(5), FxVec3::default(), FxQuat::IDENTITY, &CollisionShape::None));
        // Above radius 3 a sphere is met per axis plane (FAF probe
        // `probe_re3_wpn_b`, `uea0203`'s sphere of 1.6 against a blast of 5:
        // 6.594 along x, 7.258 diagonally in xz, 8.891 along (1, 1, 1)).
        let scout = CollisionShape::Sphere { center: FxVec3::default(), radius: Fx::from_num(1.6) };
        let hit = |p: FxVec3, r: f64| sphere_touches(p, Fx::from_num(r), FxVec3::default(), FxQuat::IDENTITY, &scout);
        let d2 = |d: f64| d / 2f64.sqrt();
        let d3 = |d: f64| d / 3f64.sqrt();
        assert!(hit(v(6.59, 0.0, 0.0), 5.0) && !hit(v(6.61, 0.0, 0.0), 5.0));
        assert!(hit(v(d2(7.25), 0.0, d2(7.25)), 5.0) && !hit(v(d2(7.27), 0.0, d2(7.27)), 5.0));
        assert!(hit(v(d3(8.88), d3(8.88), d3(8.88)), 5.0) && !hit(v(d3(8.90), d3(8.90), d3(8.90)), 5.0));
        // Radius 3 and under: the exact sphere (4.594 every way).
        assert!(hit(v(d2(4.59), 0.0, d2(4.59)), 3.0) && !hit(v(d2(4.61), 0.0, d2(4.61)), 3.0));
    }

    #[test]
    fn box_hits_including_rotation() {
        let shape = CollisionShape::Box { center: v(0.0, 0.5, 0.0), half: v(0.5, 0.5, 1.0) };
        let t = segment_hit(v(-2.0, 0.5, 0.0), v(2.0, 0.5, 0.0), FxVec3::default(), FxQuat::IDENTITY, &shape).unwrap();
        assert!((t.to_num::<f64>() - 0.375).abs() < 1e-6, "{t}");
        assert_eq!(segment_hit(v(-2.0, 1.5, 0.0), v(2.0, 1.5, 0.0), FxVec3::default(), FxQuat::IDENTITY, &shape), None, "above");
        assert_eq!(segment_hit(v(-2.0, 0.5, 0.0), v(-1.0, 0.5, 0.0), FxVec3::default(), FxQuat::IDENTITY, &shape), None, "short");
        // Rotated 90 degrees: the long (z) axis now lies along x.
        let q = FxQuat::from_hpr(Fx::FRAC_PI_2, Fx::ZERO, Fx::ZERO);
        let t = segment_hit(v(-2.0, 0.5, 0.0), v(2.0, 0.5, 0.0), FxVec3::default(), q, &shape).unwrap();
        assert!((t.to_num::<f64>() - 0.25).abs() < 1e-6, "{t}");
        let inside = segment_hit(v(0.1, 0.5, 0.0), v(3.0, 0.5, 0.0), FxVec3::default(), q, &shape);
        assert_eq!(inside, Some(Fx::ZERO));
    }

    #[test]
    fn sphere_hits() {
        let shape = CollisionShape::Sphere { center: v(0.0, 1.0, 0.0), radius: Fx::ONE };
        let pos = v(10.0, 0.0, 10.0);
        let t = segment_hit(v(10.0, 1.0, 6.0), v(10.0, 1.0, 14.0), pos, FxQuat::IDENTITY, &shape).unwrap();
        assert!((t.to_num::<f64>() - 0.375).abs() < 1e-6, "{t}");
        assert_eq!(segment_hit(v(10.0, 1.0, 14.0), v(10.0, 1.0, 20.0), pos, FxQuat::IDENTITY, &shape), None, "moving away");
        assert_eq!(segment_hit(v(12.0, 1.0, 6.0), v(12.0, 1.0, 14.0), pos, FxQuat::IDENTITY, &shape), None, "passes beside");
    }
}
