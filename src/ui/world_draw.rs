//! FAF's world shapes (its binary patch, `section/DrawFunc.cpp`): after
//! `WorldView:SetCustomRender(true)` the engine calls the view's
//! `OnRenderWorld(self, delta)` each frame, and in it `UI_DrawLine`,
//! `UI_DrawCircle` and `UI_DrawRect` draw into that view (FAF's
//! `WorldViewShapeComponent`, its map painting). Called anywhere else they
//! draw nothing: the patch only has a view to draw in during the callback.
//!
//! From the patch: positions are world coordinates, a colour must be one
//! (else "Unknown color"), and `thickness` (default 0.15) is in world units
//! but never under about 2 pixels (`max(thickness / lod, 2) * lod`, lod the
//! camera's size of a pixel at that depth). A circle takes its radius; a
//! rectangle has a corner at `pos` and `size` along +x and +z (the batcher's
//! rectangle, FA exe 0x455480: corners pos, pos + b, pos + a + b, pos + a);
//! all lie flat. The patch draws alpha blended without depth. Ours keeps a
//! line's width on screen rather than on the ground (the renderer's line
//! pass, `render/debug_lines.rs`, which the sim's debug canvas shares).

use std::sync::Arc;

use mlua::prelude::*;

use crate::engine::api::debug_draw::{DebugDraw, DebugLine, point};
use crate::lua_api::lua_fn;
use crate::lua_api::shapes::{Color, Vector};

const DEFAULT_THICKNESS: f64 = 0.15;
const MIN_PX: f32 = 2.0;
/// Most shapes one view keeps a frame (later ones are dropped).
const MAX_SHAPES: usize = 100_000;

/// The view whose `OnRenderWorld` is running: what it has drawn so far.
#[derive(Default)]
struct Collecting(Option<DebugDraw>);

/// The frame's time, handed to `OnRenderWorld`.
#[derive(Clone, Copy, Default)]
pub struct FrameDelta(pub f32);

fn color(c: &Color) -> LuaResult<u32> {
    let text = match &**c {
        LuaValue::String(s) => s.to_string_lossy(),
        other => other.to_string()?,
    };
    crate::engine::api::colors::decode(&text).ok_or_else(|| LuaError::runtime(format!("Unknown color: {text}")))
}

/// Adds to the running callback's shapes; nothing outside one.
fn add(lua: &Lua, f: impl FnOnce(&mut DebugDraw)) {
    if let Some(mut c) = lua.app_data_mut::<Collecting>()
        && let Some(d) = c.0.as_mut()
        && d.lines.len() + d.circles.len() < MAX_SHAPES
    {
        f(d);
    }
}

/// Runs world view `obj`'s `OnRenderWorld` (when it has one) and returns
/// what it drew. Its errors are logged, as the patch's `WarningF`.
pub fn render_world(lua: &Lua, obj: &LuaTable) -> LuaResult<Option<Arc<DebugDraw>>> {
    let Some(f) = obj.get::<Option<LuaFunction>>("OnRenderWorld")? else { return Ok(None) };
    let delta = lua.app_data_ref::<FrameDelta>().map_or(0.0, |d| d.0);
    lua.set_app_data(Collecting(Some(DebugDraw::default())));
    if let Err(e) = f.call::<()>((obj.clone(), delta)) {
        tracing::warn!(target: "lua", "OnRenderWorld: {e}");
    }
    let drawn = lua.remove_app_data::<Collecting>().and_then(|c| c.0).unwrap_or_default();
    Ok((!drawn.lines.is_empty() || !drawn.circles.is_empty()).then(|| Arc::new(drawn)))
}

fn thickness(t: Option<f64>) -> f32 {
    t.unwrap_or(DEFAULT_THICKNESS) as f32
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// Draws a line in the world view whose `OnRenderWorld` is running.
        UI_DrawLine = |lua, (a, b, c, t): (Vector, Vector, Color, Option<f64>)| {
            let line = DebugLine { a: point(&a)?, b: point(&b)?, color: color(&c)?, width: thickness(t), min_px: MIN_PX };
            add(lua, |d| d.lines.push(line));
            Ok(())
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm() -> Lua {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua
    }

    #[test]
    fn shapes_are_drawn_only_inside_on_render_world() {
        let lua = vm();
        // Outside the callback: nothing, and no error (the patch has no view).
        lua.load("UI_DrawLine({0, 0, 0}, {1, 0, 0}, 'ffffff')").exec().unwrap();
        let view: LuaTable = lua
            .load(
                "return { OnRenderWorld = function(self, delta)
                    UI_DrawLine({0, 1, 0}, {10, 1, 0}, 'ff0000')
                end }",
            )
            .eval()
            .unwrap();
        let d = render_world(&lua, &view).unwrap().unwrap();
        assert_eq!(d.lines[0], DebugLine { a: [0.0, 1.0, 0.0], b: [10.0, 1.0, 0.0], color: 0xFFFF_0000, width: 0.15, min_px: MIN_PX });
        // Each frame starts empty; a view without the callback draws nothing.
        assert!(render_world(&lua, &lua.create_table().unwrap()).unwrap().is_none());
    }

    #[test]
    fn an_unknown_colour_raises() {
        let lua = vm();
        let view: LuaTable = lua.load("return { OnRenderWorld = function() UI_DrawLine({0, 0, 0}, {1, 0, 0}, 'nope') end }").eval().unwrap();
        // The callback's error is logged, not raised; nothing is drawn.
        assert!(render_world(&lua, &view).unwrap().is_none());
        let err = lua.load("local ok, e = pcall(function() return UI_DrawLine({0, 0, 0}, {1, 0, 0}, 'nope') end) return tostring(e)").eval::<String>().unwrap();
        assert!(err.contains("Unknown color: nope"), "{err}");
    }
}
