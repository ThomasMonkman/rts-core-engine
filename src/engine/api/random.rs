//! `Random([min,] [max])` — the sim's deterministic RNG. FAF also installs it
//! as `math.random` (`/lua/system/config.lua`), so it follows Lua semantics:
//! `Random()` → float in [0,1), `Random(n)` → int in [1,n], `Random(m,n)` →
//! int in [m,n]. Bounds are truncated toward zero like Lua 5.0's checkint.

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
                return Ok(empty_interval(&mut rng, lo, hi) as f64);
            }
            Ok(rng.range_i64(lo, hi) as f64)
        })
}

/// `Random(m, n)` with `m > n` (and `Random(n)` with `n < 1`, as `(1, n)`):
/// moho has no empty-interval check (MohoEngine binding 0x759010 in FAF's
/// exe). It takes a 32-bit draw `u` and returns `m + ((u * w) >> 32)` with
/// the width `w = n - m + 1` as an unsigned 32-bit number, in 32-bit
/// integers. So `m = n + 1` (width 0) is always `m`: LOUD's death
/// animations call `Random(min * 10, max * 10)` on blueprints whose min is
/// above their max. A wider gap wraps to near the full 32-bit range.
fn empty_interval(rng: &mut SimRng, lo: i64, hi: i64) -> i32 {
    let (lo, hi) = (lo as i32, hi as i32);
    let width = hi.wrapping_sub(lo).wrapping_add(1) as u32;
    let u = (rng.next_u64() >> 32) as u32;
    lo.wrapping_add(((u as u64 * width as u64) >> 32) as u32 as i32)
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
    }

    /// No empty-interval error, as moho: a width of 0 gives the low bound
    /// (`Random(3, 2)` is 3, `Random(0)` is 1); a wider gap still returns a
    /// whole number.
    #[test]
    fn empty_intervals_follow_moho() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let ok: bool = lua
            .load(
                "for i = 1, 50 do
                    if Random(3, 2) ~= 3 or Random(0) ~= 1 or Random(1.9, 0.5) ~= 1 then return false end
                    local w = Random(5, 1); if w % 1 ~= 0 then return false end
                 end
                 return true",
            )
            .eval()
            .unwrap();
        assert!(ok);
    }
}
