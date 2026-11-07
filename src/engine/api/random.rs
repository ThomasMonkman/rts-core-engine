//! `Random([min,] [max])` — the sim's deterministic RNG. FAF also installs it
//! as `math.random` (`/lua/system/config.lua`), so it follows Lua semantics:
//! `Random()` → float in [0,1), `Random(n)` → int in [1,n], `Random(m,n)` →
//! int in [m,n]. Bounds are truncated toward zero like Lua 5.0's checkint.
//!
//! It never raises. Moho's (MohoEngine.dll 0x10326d30) is
//! `m + ((u32)MT() * (n + 1 - m)) >> 32` in unsigned 32-bit arithmetic with no
//! check, and `Random(n)` is `((u32)MT() * n >> 32) + 1`: so an empty range
//! (`Random(1, 0)`, `Random(0)`, FAF's `Random(1, table.getn(t))` on an empty
//! table) gives `m`, and a reversed one wraps. Ours draws its in-range results
//! its own way (an unbiased range), and the empty and reversed ranges with
//! moho's formula.

use mlua::prelude::*;

use crate::engine::sim::rng::SimRng;
use crate::lua_api::lua_fn;

pub fn register(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<SimRng>().is_none() {
        lua.set_app_data(SimRng::default());
    }
    lua_fn!(lua, global,
        /// The sim's deterministic RNG, with Lua semantics: `Random()` is a
        /// float in [0,1), `Random(n)` an integer in [1,n], `Random(m,n)` an
        /// integer in [m,n] (bounds truncated toward zero).
        Random = |lua, (a, b): (Option<f64>, Option<f64>)| {
            let mut rng = lua.app_data_mut::<SimRng>().expect("SimRng not in app_data");
            let (lo, hi) = match (a, b) {
                (None, _) => return Ok(rng.next_f64()),
                (Some(max), None) => (1, max.trunc() as i64),
                (Some(min), Some(max)) => (min.trunc() as i64, max.trunc() as i64),
            };
            if lo > hi {
                return Ok(wrapped(lo, hi, rng.next_u64() as u32) as f64);
            }
            Ok(rng.range_i64(lo, hi) as f64)
        })
}

/// Moho's `m + ((u32)r * (n + 1 - m)) >> 32` in 32-bit arithmetic, for a
/// range with `m > n`: `m` when it's empty (`n = m - 1`), else wrapped.
fn wrapped(lo: i64, hi: i64, r: u32) -> i32 {
    let (lo, hi) = (lo as i32, hi as i32);
    let span = hi.wrapping_add(1).wrapping_sub(lo) as u32;
    lo.wrapping_add(((r as u64 * span as u64) >> 32) as u32 as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_semantics() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let ok: bool = lua
            .load(
                "for i = 1, 200 do
                    local f = Random(); if f < 0 or f >= 1 then return false end
                    local a = Random(5); if a < 1 or a > 5 or a % 1 ~= 0 then return false end
                    local b = Random(-2, 2); if b < -2 or b > 2 then return false end
                 end
                 return true",
            )
            .eval()
            .unwrap();
        assert!(ok);
        // Empty ranges give the lower bound, as moho's; reversed ones don't raise.
        assert_eq!(lua.load("return Random(0), Random(1, 0), Random(5, 4)").eval::<(f64, f64, f64)>().unwrap(), (1.0, 1.0, 5.0));
        assert!(lua.load("return Random(3, 1)").eval::<f64>().is_ok());
        assert_eq!(wrapped(3, 1, 0), 3);
        assert_eq!(wrapped(3, 1, u32::MAX), 3 + ((u32::MAX as u64 * 0xffff_ffff) >> 32) as u32 as i32);
    }
}
