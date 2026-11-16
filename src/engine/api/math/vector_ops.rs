use mlua::prelude::*;

/// Registers `VAdd`, `VDiff`, `VMult`, `VDist2(Sq)`, `VDist3(Sq)`, `VDot`,
/// `VPerpDot` (Luau shim; see the file header for why). Requires the vector
/// metatable from [`super::vector2`].
pub fn register(lua: &Lua) -> LuaResult<()> {
    let vector_mt: LuaTable = lua.named_registry_value("vector_metatable")?;
    const SOURCE: &str = include_str!("../../lua/shims/vector_math.lua");
    lua.load(SOURCE).set_name("=[engine vector_math]").call::<()>(vector_mt)?;
    crate::lua_api::script::record(lua, SOURCE);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn vector_helpers() {
        let lua = mlua::Lua::new();
        super::super::register(&lua).unwrap();
        let r: (f64, f64, f64, f64, f64, f64) = lua
            .load(
                "local a, b = Vector(1, 2, 3), Vector(4, 6, 3)
                 local s = VAdd(a, b)
                 return VDist3(a, b), VDist3Sq(a, b), VDist2(0, 0, 3, 4), s.x + s.z, VDot(a, b), VPerpDot(a, b)",
            )
            .eval()
            .unwrap();
        assert_eq!(r, (5.0, 25.0, 5.0, 11.0, 25.0, 1.0 * 3.0 - 3.0 * 4.0));
    }

    #[test]
    fn vector_and_quaternion_operators() {
        let lua = mlua::Lua::new();
        super::super::register(&lua).unwrap();
        let r: (f64, f64, f64, f64, f64) = lua
            .load(
                "local mt = getmetatable(Vector(0, 0, 0))
                 local qy = setmetatable({ 0, math.sqrt(0.5), 0, math.sqrt(0.5) }, mt) -- 90 deg about Y
                 local q = qy * qy                                                     -- 180 deg about Y
                 local v = -(Vector(1, 2, 3) + Vector(1, 1, 1)) * 2
                 return q.y, q.w, v.x, v.z, (2 * Vector(1, 1, 1)).y",
            )
            .eval()
            .unwrap();
        assert!((r.0 - 1.0).abs() < 1e-12 && r.1.abs() < 1e-12, "{r:?}");
        assert_eq!((r.2, r.3, r.4), (-4.0, -8.0, 2.0));
    }
}
