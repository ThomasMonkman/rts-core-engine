//! Moho's sim debug drawing: `DrawLine`, `DrawLinePop` and `DrawCircle`
//! (FA exe 0x75db80, 0x75dda0, 0x75e0b0) add lines and circles to the sim's
//! debug canvas (got through 0x746720), which is drawn over the world and
//! emptied each beat, so scripts redraw what they want kept every tick (AI
//! mods' visualisers do: RNGAI, FAF's adaptive AI platoons).
//!
//! From the binary: a circle lies flat (axis {0, 1, 0}) and its number is
//! the radius (both functions hand the same circle helper a radius);
//! `DrawLinePop(start, finish)` draws the line and a circle of radius 1
//! centred 2 units from `start` toward `finish` (at `start` when the two
//! points meet). Moho draws its circles with few segments; ours are smooth.
//!
//! Presentation only: the sim never reads the canvas, and it isn't saved,
//! checksummed or replayed. Its numbers are `f32` for the renderer.

use mlua::prelude::*;

use crate::lua_api::lua_fn;
use crate::lua_api::shapes::{Color, Vector};

/// Most primitives one tick keeps (a runaway draw loop can't grow the canvas
/// without bound; later ones are dropped).
const MAX_PRIMITIVES: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugLine {
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// ARGB.
    pub color: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugCircle {
    pub center: [f32; 3],
    pub radius: f32,
    /// ARGB.
    pub color: u32,
}

/// The beat's debug canvas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DebugDraw {
    pub lines: Vec<DebugLine>,
    pub circles: Vec<DebugCircle>,
}

impl DebugDraw {
    fn full(&self) -> bool {
        self.lines.len() + self.circles.len() >= MAX_PRIMITIVES
    }
}

/// Empties the canvas (each beat, before the scripts run).
pub fn clear(lua: &Lua) {
    if let Some(mut d) = lua.app_data_mut::<DebugDraw>() {
        d.lines.clear();
        d.circles.clear();
    }
}

/// The beat's canvas, for the snapshot. A copy: a paused sim republishes
/// the same beat, which must still show it.
pub fn current(lua: &Lua) -> DebugDraw {
    lua.app_data_ref::<DebugDraw>().map(|d| d.clone()).unwrap_or_default()
}

fn point(v: &Vector) -> LuaResult<[f32; 3]> {
    let c = |i: i64| -> LuaResult<f32> { Ok(v.get::<Option<f64>>(i)?.unwrap_or(0.0) as f32) };
    Ok([c(1)?, c(2)?, c(3)?])
}

/// A `Color` string as ARGB (opaque white when it isn't one).
fn argb(color: &Color) -> u32 {
    match &**color {
        LuaValue::String(s) => crate::engine::api::colors::decode(&s.to_string_lossy()).unwrap_or(0xFFFF_FFFF),
        _ => 0xFFFF_FFFF,
    }
}

fn add(lua: &Lua, f: impl FnOnce(&mut DebugDraw)) {
    if lua.app_data_ref::<DebugDraw>().is_none() {
        lua.set_app_data(DebugDraw::default());
    }
    if let Some(mut d) = lua.app_data_mut::<DebugDraw>() {
        if !d.full() {
            f(&mut d);
        }
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// Draws a line on the debug canvas this beat.
        DrawLine = |lua, (a, b, color): (Vector, Vector, Color)| {
            let line = DebugLine { a: point(&a)?, b: point(&b)?, color: argb(&color) };
            add(lua, |d| d.lines.push(line));
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Draws a line and a small circle 2 units from `start` toward
        /// `finish` on the debug canvas this beat.
        DrawLinePop = |lua, (start, finish, color): (Vector, Vector, Color)| {
            let (a, b, color) = (point(&start)?, point(&finish)?, argb(&color));
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let k = if len > 0.0 { 2.0 / len } else { 0.0 };
            let center = [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k];
            add(lua, |c| {
                c.lines.push(DebugLine { a, b, color });
                c.circles.push(DebugCircle { center, radius: 1.0, color });
            });
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Draws a flat circle of that radius on the debug canvas this beat.
        DrawCircle = |lua, (center, radius, color): (Vector, f64, Color)| {
            let circle = DebugCircle { center: point(&center)?, radius: radius as f32, color: argb(&color) };
            add(lua, |d| d.circles.push(circle));
            Ok(())
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_collect_until_cleared() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load("DrawLine({0, 1, 2}, {3, 4, 5}, 'ff0000') DrawCircle({1, 2, 3}, 10, 'aaffffff') DrawLinePop({0, 0, 0}, {10, 0, 0}, 'Red')")
            .exec()
            .unwrap();
        let d = current(&lua);
        assert_eq!(d.lines.len(), 2);
        assert_eq!(d.lines[0], DebugLine { a: [0.0, 1.0, 2.0], b: [3.0, 4.0, 5.0], color: 0xFFFF_0000 });
        assert_eq!(d.circles[0], DebugCircle { center: [1.0, 2.0, 3.0], radius: 10.0, color: 0xAAFF_FFFF });
        // The pop: radius 1, 2 units along the line from its start.
        assert_eq!(d.circles[1], DebugCircle { center: [2.0, 0.0, 0.0], radius: 1.0, color: 0xFFFF_0000 });
        // Republishing the beat shows it again; the next beat starts empty.
        assert_eq!(current(&lua), d);
        clear(&lua);
        assert_eq!(current(&lua), DebugDraw::default());
    }
}
