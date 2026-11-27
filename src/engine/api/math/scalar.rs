//! Moho scalar math globals (`MATH_*`).
//!
//! Determinism: only IEEE-754 basic operations and `round_ties_even` are used,
//! which are exactly specified and bit-identical on x86-64 and AArch64.
//! No libm transcendental calls here.

use mlua::prelude::*;

use crate::lua_api::{lua_fn, LuaArgs, FnEntry, Scope, Status};

/// `MATH_Lerp(s, a, b)`, or `MATH_Lerp(s, sMin, sMax, a, b)` (`s` mapped
/// from `sMin..sMax` to `0..1` first): `a + (b - a) * t`, unclamped; with
/// four arguments, nothing. A Luau
/// shim: leaf arithmetic on Lua numbers is faster there than a native call
/// (see `vector_math.lua`), and it uses basic IEEE ops only. Moho computes
/// `a + s * (b - a)` too, in float32 (MohoEngine.dll 0x100c30f0).
const LERP_SRC: &str = r#"
function MATH_Lerp(s, a, b, c, d)
    if c ~= nil then
        -- Four arguments: nothing, as moho (dev/probes probe_api_spot).
        if d == nil then return end
        s = (s - a) / (b - a)
        a, b = c, d
    end
    return a + (b - a) * s
end
"#;

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.load(LERP_SRC).set_name("=[engine MATH_Lerp]").exec()?;
    // A Luau shim (above), recorded so it appears in the API.
    crate::lua_api::registry::record(
        lua,
        FnEntry {
            scope: Scope::Global,
            name: "MATH_Lerp".into(),
            params: Some(&[Some("s"), Some("a"), Some("b"), Some("c"), Some("d")]),
            args: <(f64, f64, f64, Option<f64>, Option<f64>) as LuaArgs>::types,
            rets: <f64 as LuaArgs>::types,
            doc: "`MATH_Lerp(s, a, b)`: `a + (b - a) * s`, unclamped; or `MATH_Lerp(s, sMin, sMax, a, b)`, with `s` mapped from `sMin..sMax` to `0..1` first.",
            status: Status::Implemented,
            src: crate::lua_api::Src { file: file!(), line: line!() },
        },
    );
    lua_fn!(lua, global,
        /// `n` rounded to the nearest integer, ties to even.
        MATH_IRound = |_, n: f64| Ok(iround(n)))?;
    // Native rather than a Luau shim: Lua's `math.sin/cos` go through the
    // platform libm, which differs between x86 and ARM; CORDIC doesn't.
    lua_fn!(lua, global,
        /// The quaternion `{x, y, z, w}` for the Euler angles (radians).
        EulerToQuaternion = |lua, (roll, pitch, yaw): (f64, f64, f64)| {
            let q = euler_to_quaternion(roll, pitch, yaw);
            let t = lua.create_table_with_capacity(4, 0)?;
            for (i, v) in q.into_iter().enumerate() {
                t.raw_set(i + 1, v)?;
            }
            t.set_metatable(lua.named_registry_value::<Option<LuaTable>>("vector_metatable")?);
            Ok(crate::lua_api::shapes::Quaternion(t))
        })?;
    lua_fn!(lua, global,
        /// Linear interpolation between quaternions `l` and `r` along the
        /// shorter arc, normalised.
        MinLerp = |lua, (alpha, l, r): (f64, crate::lua_api::shapes::Quaternion, crate::lua_api::shapes::Quaternion)| quat_table(lua, min_lerp(alpha, read_quat(&l)?, read_quat(&r)?)))?;
    lua_fn!(lua, global,
        /// Spherical interpolation between quaternions `l` and `r` along the
        /// shorter arc.
        MinSlerp = |lua, (alpha, l, r): (f64, crate::lua_api::shapes::Quaternion, crate::lua_api::shapes::Quaternion)| quat_table(lua, min_slerp(alpha, read_quat(&l)?, read_quat(&r)?)))?;
    Ok(())
}

type Quat = [crate::engine::sim::math::Fx; 4];

/// A Lua quaternion `{x, y, z, w}` in fixed point.
fn read_quat(t: &LuaTable) -> LuaResult<Quat> {
    let c = |i: usize| -> LuaResult<_> { Ok(crate::engine::sim::math::from_lua(t.raw_get::<f64>(i)?)) };
    Ok([c(1)?, c(2)?, c(3)?, c(4)?])
}

fn quat_table(lua: &Lua, q: Quat) -> LuaResult<crate::lua_api::shapes::Quaternion> {
    let t = lua.create_table_with_capacity(4, 0)?;
    for (i, v) in q.into_iter().enumerate() {
        t.raw_set(i + 1, v.to_num::<f64>())?;
    }
    t.set_metatable(lua.named_registry_value::<Option<LuaTable>>("vector_metatable")?);
    Ok(crate::lua_api::shapes::Quaternion(t))
}

/// `r` flipped onto `l`'s hemisphere (the shorter arc: "minimal"), and the
/// cosine between them.
fn nearer(l: Quat, r: Quat) -> (Quat, crate::engine::sim::math::Fx) {
    let d = l[0] * r[0] + l[1] * r[1] + l[2] * r[2] + l[3] * r[3];
    if d < 0 { (r.map(|v| -v), -d) } else { (r, d) }
}

fn normalized(q: Quat, fallback: Quat) -> Quat {
    use crate::engine::sim::math::{sqrt, Fx};
    let len = sqrt(q.iter().map(|&v| v * v).sum());
    if len == Fx::ZERO { fallback } else { q.map(|v| v / len) }
}

/// Moho's `MinLerp(alpha, L, R)`: the normalised lerp from `L` to `R`
/// along the shorter arc (docs/research/verifying-against-the-binary.md).
pub fn min_lerp(alpha: f64, l: Quat, r: Quat) -> Quat {
    let a = crate::engine::sim::math::from_lua(alpha);
    let (r, _) = nearer(l, r);
    normalized(std::array::from_fn(|i| l[i] + (r[i] - l[i]) * a), l)
}

/// Moho's `MinSlerp(alpha, L, R)`: the spherical lerp from `L` to `R` along
/// the shorter arc, with fixed-point trig (libm's differs across x86 and
/// ARM). Nearly equal rotations lerp instead (the angle's sine vanishes).
pub fn min_slerp(alpha: f64, l: Quat, r: Quat) -> Quat {
    use crate::engine::sim::math::{atan2, from_lua, sin_cos, sqrt, Fx};
    let a = from_lua(alpha);
    let (r, d) = nearer(l, r);
    if d > Fx::from_num(0.9995) {
        return min_lerp(alpha, l, r);
    }
    let d = d.min(Fx::ONE);
    let theta = atan2(sqrt(Fx::ONE - d * d), d);
    let sin_theta = sin_cos(theta).0;
    let wl = sin_cos((Fx::ONE - a) * theta).0 / sin_theta;
    let wr = sin_cos(a * theta).0 / sin_theta;
    std::array::from_fn(|i| l[i] * wl + r[i] * wr)
}

/// Moho's `EulerToQuaternion`: Y(yaw) * X(pitch) * Z(roll), as `{x, y, z, w}`.
/// Confirmed in MohoEngine.dll 0x100fa8a0 -> 0x100db030; fixed-point trig.
pub fn euler_to_quaternion(roll: f64, pitch: f64, yaw: f64) -> [f64; 4] {
    use crate::engine::sim::math::sin_cos;
    let half = |a: f64| sin_cos(crate::engine::sim::math::from_lua(a) / 2);
    let ((sr, cr), (sp, cp), (sy, cy)) = (half(roll), half(pitch), half(yaw));
    [
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        sr * cp * cy - cr * sp * sy,
        cr * cp * cy + sr * sp * sy,
    ]
    .map(|v| v.to_num::<f64>())
}

/// Round to nearest integer, ties to even (banker's rounding), as moho does.
#[inline]
pub fn iround(n: f64) -> f64 {
    n.round_ties_even()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lerp_both_forms() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: (f64, f64, f64) = lua.load("return MATH_Lerp(0.25, 10, 20), MATH_Lerp(5, 0, 10, 100, 200), MATH_Lerp(1.5, 0, 2)").eval().unwrap();
        assert_eq!(r, (12.5, 150.0, 3.0));
        let n: mlua::MultiValue = lua.load("return MATH_Lerp(1, 2, 3, 4)").eval().unwrap();
        assert!(n.is_empty(), "four arguments return nothing");
    }

    use super::*;

    #[test]
    fn euler_to_quaternion_matches_formula() {
        assert_eq!(euler_to_quaternion(0.0, 0.0, 0.0), [0.0, 0.0, 0.0, 1.0]);
        let (r, p, y) = (0.3f64, -1.1f64, 2.0f64);
        let (sr, cr, sp, cp, sy, cy) = ((r / 2.0).sin(), (r / 2.0).cos(), (p / 2.0).sin(), (p / 2.0).cos(), (y / 2.0).sin(), (y / 2.0).cos());
        let expect = [cr * sp * cy + sr * cp * sy, cr * cp * sy - sr * sp * cy, sr * cp * cy - cr * sp * sy, cr * cp * cy + sr * sp * sy];
        for (a, b) in euler_to_quaternion(r, p, y).iter().zip(expect) {
            assert!((a - b).abs() < 1e-7, "{a} vs {b}");
        }
    }

    #[test]
    fn euler_to_quaternion_uses_y_up_axes() {
        let angle = std::f64::consts::FRAC_PI_2;
        let s = std::f64::consts::FRAC_1_SQRT_2;
        for (angles, expected) in [
            ((angle, 0.0, 0.0), [0.0, 0.0, s, s]),
            ((0.0, angle, 0.0), [s, 0.0, 0.0, s]),
            ((0.0, 0.0, angle), [0.0, s, 0.0, s]),
        ] {
            for (actual, expected) in euler_to_quaternion(angles.0, angles.1, angles.2).into_iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-7, "{angles:?}: {actual} vs {expected}");
            }
        }
    }

    #[test]
    fn euler_terrain_tilt_does_not_turn_the_heading() {
        use crate::engine::sim::math::{from_lua, Fx, FxQuat, FxVec3};
        let lua = mlua::Lua::new();
        register(&lua).unwrap();
        for (roll, pitch) in [(-0.312, -0.038), (0.2, 0.1), (0.0, -0.3), (-0.2, 0.0)] {
            let values: LuaTable = lua.globals().get::<LuaFunction>("EulerToQuaternion").unwrap().call((roll, pitch, 0.0)).unwrap();
            let components = read_quat(&values).unwrap();
            let orientation = FxQuat { x: components[0], y: components[1], z: components[2], w: components[3] };
            assert!(orientation.heading().abs() < Fx::from_num(1e-7));
            let up = orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO });
            let expected = FxQuat::from_hpr(Fx::ZERO, from_lua(pitch), from_lua(roll)).rotate(FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO });
            assert!((up - expected).length() < Fx::from_num(1e-7));
        }
    }

    fn q(v: [f64; 4]) -> Quat {
        v.map(crate::engine::sim::math::from_lua)
    }

    fn close(a: Quat, b: [f64; 4]) {
        for (a, b) in a.iter().zip(b) {
            assert!((a.to_num::<f64>() - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn min_slerp_halfway_about_y() {
        // Identity to 90 degrees about Y: halfway is 45 degrees.
        let h = std::f64::consts::FRAC_PI_8;
        let (l, r) = (q([0.0, 0.0, 0.0, 1.0]), q([0.0, std::f64::consts::FRAC_1_SQRT_2, 0.0, std::f64::consts::FRAC_1_SQRT_2]));
        close(min_slerp(0.5, l, r), [0.0, h.sin(), 0.0, h.cos()]);
        close(min_slerp(0.0, l, r), [0.0, 0.0, 0.0, 1.0]);
        close(min_slerp(1.0, l, r), [0.0, std::f64::consts::FRAC_1_SQRT_2, 0.0, std::f64::consts::FRAC_1_SQRT_2]);
        // Same arc, at a quarter: 22.5 degrees.
        let e = std::f64::consts::PI / 16.0;
        close(min_slerp(0.25, l, r), [0.0, e.sin(), 0.0, e.cos()]);
    }

    #[test]
    fn min_interpolation_takes_the_shorter_arc() {
        // -R is the same rotation as R: the result is the same either way.
        let h = std::f64::consts::FRAC_PI_8;
        let (l, r) = (q([0.0, 0.0, 0.0, 1.0]), q([0.0, -std::f64::consts::FRAC_1_SQRT_2, 0.0, -std::f64::consts::FRAC_1_SQRT_2]));
        close(min_slerp(0.5, l, r), [0.0, h.sin(), 0.0, h.cos()]);
        close(min_lerp(0.5, l, r), [0.0, h.sin(), 0.0, h.cos()]);
    }

    #[test]
    fn min_lerp_from_lua_is_unit_length() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let len: f64 = lua
            .load("local q = MinLerp(0.3, {0, 0, 0, 1}, {1, 0, 0, 0}) return math.sqrt(q[1]*q[1] + q[2]*q[2] + q[3]*q[3] + q[4]*q[4])")
            .eval()
            .unwrap();
        assert!((len - 1.0).abs() < 1e-6, "{len}");
    }

    #[test]
    fn iround_ties_to_even() {
        assert_eq!(iround(0.5), 0.0);
        assert_eq!(iround(1.5), 2.0);
        assert_eq!(iround(2.5), 2.0);
        assert_eq!(iround(-2.5), -2.0);
        assert_eq!(iround(2.4), 2.0);
        assert_eq!(iround(2.6), 3.0);
    }

    #[test]
    fn iround_from_lua_is_integer() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let s: String = lua.load("return tostring(MATH_IRound(3.5))").eval().unwrap();
        assert_eq!(s, "4");
    }
}
