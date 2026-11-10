//! Native `maui` controls (moho's `CMaui*`): the tree FAF's UI builds with
//! `InternalCreate*`, and the per-frame draw list made from it.
//!
//! FAF's Lua classes (`/lua/maui/*.lua`) own behaviour and layout; the
//! engine side of a control is its place in the tree, its visibility,
//! alpha and content (texture, solid colour, text). Layout lives in the
//! LazyVars the engine puts on each control when it's created (`Left`,
//! `Top`, `Right`, `Bottom`, `Width`, `Height`, `Depth`; bitmaps also
//! `BitmapWidth` / `BitmapHeight`, texts `TextAdvance` / `FontAscent` /
//! `FontDescent` / `FontExternalLeading`), then `OnInit` runs (FAF's `Control.OnInit` sets up the
//! default relations). Drawing reads those LazyVars back.
//!
//! Render-thread only, never sim state: nothing here needs to be
//! deterministic.

use crate::lua_api::shapes::List;
use crate::ui::shapes::Loose;
use std::collections::BTreeMap;

use mlua::Either;
use mlua::prelude::*;

use crate::engine::api::moho::{self, MohoClass};
use crate::lua_api::{Obj, Status, lua_fn, lua_method};
use crate::engine::api::script_object::{self, C_OBJECT};

pub type ControlId = u32;

#[derive(Clone, Copy, Debug)]
pub struct ControlHandle(pub ControlId);
impl LuaUserData for ControlHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

/// A camera's native side (`GetCamera`): cameras live as long as the
/// interface, so it is never destroyed.
pub struct CameraHandle;
impl LuaUserData for CameraHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

/// A border: textures for the vertical sides, the horizontal sides and the
/// four corners (`SetNewTextures`), or one colour (`SetSolidColor`).
#[derive(Clone, Debug, Default)]
pub struct Border {
    /// vertical, horizontal, upper left, upper right, lower left, lower right.
    pub textures: [Option<String>; 6],
    pub solid: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Default)]
pub struct Bitmap {
    /// Animation frames' texture paths (one for a still bitmap).
    pub textures: Vec<String>,
    pub frame: usize,
    /// `InternalSetSolidColor`: a flat colour instead of a texture.
    pub solid: Option<[f32; 4]>,
    /// Sub-rectangle of the texture `(u0, v0, u1, v1)`.
    pub uv: [f32; 4],
    pub tiled: bool,
    /// `SetColorMask` (FAF patch): multiplies the texture.
    pub mask: Option<[f32; 4]>,
    /// Frame animation (`Play`, `SetFrameRate`, `Set*Pattern`, `Loop`).
    pub anim: Animation,
}

/// A bitmap's frame animation: the frames in order (`None`: forward through
/// all of them), frames per second, whether it loops, and where it is.
#[derive(Clone, Debug, Default)]
pub struct Animation {
    pub pattern: Option<Vec<usize>>,
    /// Frames per second; 0 = unset (10, inferred).
    pub rate: f32,
    pub looping: bool,
    pub playing: bool,
    /// Index into the pattern, and time since the last frame.
    pub step: usize,
    pub elapsed: f32,
}

/// The frame order of a pattern name over `n` frames: forward, backward,
/// ping-pong (there and back, the end frames once each), and the looping
/// ping-pong (without repeating its first frame).
fn pattern(kind: &str, n: usize) -> Vec<usize> {
    let forward: Vec<usize> = (0..n).collect();
    match kind {
        "backward" => forward.into_iter().rev().collect(),
        "pingpong" => forward.iter().copied().chain((0..n.saturating_sub(1)).rev()).collect(),
        "looppingpong" => forward.iter().copied().chain((1..n.saturating_sub(1)).rev()).collect(),
        _ => forward,
    }
}

#[derive(Clone, Debug)]
pub struct Text {
    pub text: String,
    pub family: String,
    pub size: f32,
    pub color: [f32; 4],
    pub shadow: bool,
    pub center_h: bool,
    pub center_v: bool,
    pub clip: bool,
    /// `SetVertical` (engine extension, not moho): drawn turned a quarter
    /// anticlockwise, reading bottom to top, inside `[Left, Left + Height]` x
    /// `[Top, Top + Width]` (Width / Height stay the unturned line's).
    pub vertical: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self { text: String::new(), family: "Arial".into(), size: 12.0, color: [1.0; 4], shadow: false, center_h: false, center_v: false, clip: false, vertical: false }
    }
}

/// A text input (`CMauiEdit`).
#[derive(Clone, Debug)]
pub struct Edit {
    pub text: Text,
    pub background: [f32; 4],
    pub show_background: bool,
    pub max_chars: i64,
    pub enabled: bool,
    /// Caret position in characters.
    pub caret: i64,
    pub caret_color: [f32; 4],
    pub caret_shown: bool,
    /// `SetCaretCycle(seconds, minAlpha, maxAlpha)`: the caret's blink.
    pub caret_cycle: Option<(f32, f32, f32)>,
}

impl Default for Edit {
    fn default() -> Self {
        Self { text: Text::default(), background: [0.0, 0.0, 0.0, 0.0], show_background: false, max_chars: 255, enabled: true, caret: 0, caret_color: [1.0; 4], caret_shown: true, caret_cycle: None }
    }
}

/// A world view's settings (FAF's interface toggles them). The renderer
/// draws each shown main view's camera in its rectangle
/// ([`DrawItem::World`]), the minimap from its own state.
#[derive(Clone, Debug, Default)]
pub struct WorldView {
    /// A minimap (`isMiniMap`).
    pub mini: bool,
    /// The camera it shows (`GetCamera(name)`).
    pub camera: Option<String>,
    /// `LockInput` (cinematics): the world doesn't get the mouse over it.
    pub input_locked: bool,
    pub cartographic: bool,
    pub resources: bool,
    /// `SetCustomRender`: draws FAF's world shapes (not drawn yet).
    pub custom_render: bool,
    /// `GetsGlobalCameraCommands`: the arrow keys move its camera.
    pub global_camera: bool,
}

/// A list of text rows (`CMauiItemList`; FAF's combo boxes' drop-downs).
#[derive(Clone, Debug)]
pub struct ItemList {
    pub items: Vec<String>,
    /// Selected row (0-based), if any.
    pub selection: Option<usize>,
    /// First row shown.
    pub top: usize,
    pub family: String,
    pub size: f32,
    /// `SetNewColors`: foreground, background, selected foreground /
    /// background, mouseover foreground / background.
    pub colors: [Option<[f32; 4]>; 6],
    pub show_selection: bool,
    pub show_mouseover: bool,
    /// Row under the mouse.
    pub hover: Option<usize>,
}

impl Default for ItemList {
    fn default() -> Self {
        Self { items: Vec::new(), selection: None, top: 0, family: "Arial".into(), size: 12.0, colors: [None; 6], show_selection: true, show_mouseover: false, hover: None }
    }
}

/// A scrollbar: its axis, textures and the control it scrolls.
#[derive(Clone, Debug, Default)]
pub struct Scrollbar {
    pub vertical: bool,
    /// `SetNewTextures`: background, thumb middle, thumb top, thumb bottom.
    pub textures: [Option<String>; 4],
    /// The thumb top / bottom textures' length along the axis (pixels).
    pub caps: [f32; 2],
    pub scrollable: Option<ControlId>,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Frame,
    Group,
    Bitmap(Bitmap),
    Text(Text),
    Edit(Edit),
    /// A world view (`moho.UIWorldView`).
    WorldView(WorldView),
    /// A scrollbar (`CMauiScrollbar`).
    Scrollbar(Scrollbar),
    ItemList(ItemList),
    /// A frame round its rectangle (`moho.border_methods`).
    Border(Border),
    /// A control type not drawn yet (border, item list, ...).
    Other(&'static str),
}

pub struct Control {
    pub parent: Option<ControlId>,
    pub children: Vec<ControlId>,
    pub kind: Kind,
    pub hidden: bool,
    pub alpha: f32,
    pub hit_test: bool,
    pub name: String,
    pub needs_frame: bool,
    pub render_pass: i32,
    /// Its layout as last read ([`read_layouts`]); `None` to read again
    /// ([`forget_dirty_layouts`]).
    pub layout: Option<(f32, f32, f32, f32, f32)>,
    /// The control's Lua object.
    pub obj: LuaRegistryKey,
}

/// Every live control (app data of the user VM).
#[derive(Default)]
pub struct Controls {
    next: ControlId,
    pub map: BTreeMap<ControlId, Control>,
    /// Root frames by head index (`GetFrame`).
    pub frames: BTreeMap<i64, ControlId>,
    /// Each head's screen size in pixels (its root frame spans it): one
    /// per head, so its length is `GetNumRootFrames`.
    pub screens: Vec<(f32, f32)>,
    /// The head the mouse is on: hit tests and mouse events see only its
    /// root frame's controls (each head's start at its own top left).
    pub mouse_head: i64,
    /// The last button press on a control (button, control, time, x, y),
    /// for double clicks.
    last_press: Option<(u8, ControlId, f64, f32, f32)>,
    /// Controls holding the keyboard focus, oldest first, and whether each
    /// blocks the key map.
    focus: Vec<(ControlId, bool)>,
    /// A scrollbar whose thumb is held, and where on the thumb (pixels
    /// along its axis).
    scroll_drag: Option<(ControlId, f64)>,
}

/// A second press within this long (seconds) and this close (pixels each
/// way) is a double click: Windows' defaults (`GetDoubleClickTime`,
/// `SM_CXDOUBLECLK` / 2), which moho gets from the OS.
const DOUBLE_CLICK_TIME: f64 = 0.5;
const DOUBLE_CLICK_DISTANCE: f32 = 2.0;

/// What one visible control draws, in screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawItem {
    /// A textured (or, with `texture: None`, flat) rectangle.
    Quad { rect: [f32; 4], texture: Option<String>, uv: [f32; 4], color: [f32; 4], tiled: bool },
    /// The minimap: the map seen from above, filling `rect`.
    Minimap { rect: [f32; 4] },
    /// A main world view: camera `camera`'s view of the world fills `rect`
    /// (drawn under the interface, not by it).
    World { rect: [f32; 4], camera: String },
    /// A line of text with its top-left at `rect[0..2]`; `vertical`: turned a
    /// quarter anticlockwise about that corner's box (see [`Text::vertical`]).
    Text { rect: [f32; 4], text: String, family: String, size: f32, color: [f32; 4], shadow: bool, vertical: bool },
    /// Movie control `movie`'s current frame stretched over `rect`
    /// (YCbCr planes; the renderer converts them).
    Movie { rect: [f32; 4], movie: ControlId, frame: super::movie::FrameRef, color: [f32; 4] },
}

/// `"ffRRGGBB"` (ARGB), `"RRGGBB"`, or a moho colour name
/// ([`colors::NAMED`](crate::engine::api::colors::NAMED)) -> RGBA in 0..1.
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let argb = crate::engine::api::colors::decode(s)?;
    let [r, g, b] = crate::engine::api::colors::rgb(argb);
    Some([r, g, b, (argb >> 24) as f32 / 255.0])
}

pub(crate) fn controls(lua: &Lua) -> mlua::AppDataRefMut<'_, Controls> {
    lua.app_data_mut::<Controls>().expect("Controls not in app_data")
}

/// Every live control's id, ascending.
pub(crate) fn live(lua: &Lua) -> Vec<ControlId> {
    controls(lua).map.keys().copied().collect()
}

pub(crate) fn id_of(obj: &LuaTable) -> LuaResult<ControlId> {
    script_object::handle::<ControlHandle>(obj).map(|h| h.0)
}

/// FAF's `lazyvar.Create`, imported once.
fn lazyvar_create(lua: &Lua) -> LuaResult<LuaFunction> {
    if let Ok(f) = lua.named_registry_value::<LuaFunction>("ui.lazyvar.create") {
        return Ok(f);
    }
    let import: LuaFunction = lua.globals().get("import")?;
    let module: LuaTable = import.call("/lua/lazyvar.lua")?;
    let f: LuaFunction = module.get("Create")?;
    lua.set_named_registry_value("ui.lazyvar.create", f.clone())?;
    Ok(f)
}

/// Sets LazyVar `obj[name]` to `value`.
pub(crate) fn set_var(obj: &LuaTable, name: &str, value: impl IntoLua) -> LuaResult<()> {
    let var: LuaTable = obj.get(name)?;
    var.call_method::<()>("Set", value)
}

/// Reads LazyVar `obj[name]` (its cached value, else evaluates it).
fn read_var(obj: &LuaTable, name: &str) -> f32 {
    let Ok(LuaValue::Table(var)) = obj.raw_get::<LuaValue>(name) else { return 0.0 };
    if let Ok(Some(v)) = var.raw_get::<Option<f64>>(1) {
        return v as f32;
    }
    var.call::<Option<f64>>(()).ok().flatten().unwrap_or(0.0) as f32
}

/// A control's `(left, top, width, height, depth)` from its edges (moho
/// draws Left..Right, Top..Bottom; a StatusBar's bar moves its Right and
/// leaves Width at the texture's), in one call: Luau's own `pcall` per
/// var, so a control FAF never laid out (its default relations are
/// circular) costs a cheap failed evaluation, not an error with a
/// traceback crossing into Rust. `None` when an edge failed.
/// World view `view`'s camera name (`None`: not a world view, or none).
pub(crate) fn view_camera(lua: &Lua, view: &LuaTable) -> Option<String> {
    let id = id_of(view).ok()?;
    match &controls(lua).map.get(&id)?.kind {
        Kind::WorldView(v) => v.camera.clone(),
        _ => None,
    }
}

fn layout(lua: &Lua, obj: &LuaTable) -> Option<(f32, f32, f32, f32, f32)> {
    let f: LuaFunction = lua.named_registry_value("ui.layout").ok()?;
    let (l, t, w, h, d): (Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>) = f.call(obj).ok()?;
    Some((l? as f32, t? as f32, w? as f32, h? as f32, d.unwrap_or(0.0) as f32))
}

/// Controls' layouts are kept between frames (`Control::layout`): one is
/// read again only when it went dirty (its watcher in `LAYOUT_SRC`). Reading
/// every drawn control's edges each frame cost ~0.3 us a control (1 ms for a
/// Unit Manager's 3400). This forgets the dirty ones' layouts.
fn forget_dirty_layouts(lua: &Lua) -> LuaResult<()> {
    let take_dirty: LuaFunction = lua.named_registry_value("ui.take_dirty")?;
    if let Some(dirty) = take_dirty.call::<Option<mlua::Buffer>>(())? {
        let mut c = controls(lua);
        for id in dirty.to_vec().chunks_exact(4) {
            if let Some(ctl) = c.map.get_mut(&u32::from_le_bytes([id[0], id[1], id[2], id[3]])) {
                ctl.layout = None;
            }
        }
    }
    Ok(())
}

/// Reads the layouts of controls `stale` (in one Luau call) into
/// `Control::layout`; a control with no layout stays `None`, to be read
/// again next time.
fn read_layouts(lua: &Lua, stale: &[ControlId]) -> LuaResult<()> {
    if stale.is_empty() {
        return Ok(());
    }
    let read: LuaFunction = lua.named_registry_value("ui.layouts")?;
    let input = lua.create_buffer(stale.iter().flat_map(|id| id.to_le_bytes()).collect::<Vec<u8>>())?;
    let out = lua.create_buffer(vec![0u8; stale.len() * 20])?;
    read.call::<()>((input, stale.len(), &out))?;
    let bytes = out.to_vec();
    let mut c = controls(lua);
    for (id, r) in stale.iter().zip(bytes.chunks_exact(20)) {
        let at = |i: usize| f32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
        if let Some(ctl) = c.map.get_mut(id) {
            ctl.layout = (!at(0).is_nan()).then(|| (at(0), at(4), at(8), at(12), at(16)));
        }
    }
    Ok(())
}

const LAYOUT_SRC: &str = r#"
    local pcall = pcall
    local function get(o, name)
        local var = rawget(o, name)
        if not var then return nil end
        local cached = var[1]
        if cached ~= nil then return cached end
        local ok, v = pcall(var)
        if ok then return v end
        return nil
    end
    local layout = function(o)
        -- Stops at the first edge that fails: an unlaid-out control's
        -- circular evaluation is the expensive part.
        local l = get(o, 'Left') if not l then return nil end
        local r = get(o, 'Right') if not r then return nil end
        local t = get(o, 'Top') if not t then return nil end
        local b = get(o, 'Bottom') if not b then return nil end
        return l, t, r - l, b - t, get(o, 'Depth')
    end
    -- Every live control's object by id (kept by `create` / `destroy`).
    local objects = {}
    local readu32, writeu32, writef32, type = buffer.readu32, buffer.writeu32, buffer.writef32, type
    local nan = 0 / 0
    -- Each control's layout watcher (FAF's `lazyvar.Derive` pattern): a
    -- LazyVar reading the control's edges, so whatever they depend on going
    -- dirty puts the control on `dirty`, and the engine reads again only
    -- those. A control not laid out isn't cached: it's read every time.
    -- `dirty` holds each control once (`queued`), however often it goes
    -- dirty before the engine next asks.
    local watchers, dirty, queued = {}, {}, {}
    -- The users of every watcher: none (one is never read inside another LazyVar).
    local nobody = {}
    local create
    local function edge(o, name)
        local var = rawget(o, name)
        if not var then return nil end
        -- Called, not read from its cache: the call records the dependency.
        local ok, v = pcall(var)
        if ok then return v end
        return nil
    end
    local function watcher(id, o)
        local w = watchers[id]
        if w then return w end
        create = create or function()
            -- A LazyVar without `Create`'s two weak tables (thousands of
            -- weak tables slow every GC cycle): what it uses, kept strongly
            -- (it goes with its control), and nobody uses it.
            local module = import('/lua/lazyvar.lua')
            local meta = rawget(module, 'LazyVarMetaTable')
            if not meta then return module.Create() end
            return setmetatable({ nil, nil, nil, nil, {}, nobody }, meta)
        end
        w = create()
        w:Set(function()
            -- Anything but numbers is no layout, as `layout` has it.
            local l = edge(o, 'Left') if type(l) ~= 'number' then return nil end
            local r = edge(o, 'Right') if type(r) ~= 'number' then return nil end
            local t = edge(o, 'Top') if type(t) ~= 'number' then return nil end
            local b = edge(o, 'Bottom') if type(b) ~= 'number' then return nil end
            local d = edge(o, 'Depth')
            if d ~= nil and type(d) ~= 'number' then return nil end
            return { l, t, r - l, b - t, d or 0 }
        end)
        w.OnDirty = function()
            if not queued[id] then
                queued[id] = true
                dirty[#dirty + 1] = id
            end
        end
        watchers[id] = w
        return w
    end
    -- The layouts of `n` controls (ids as u32s in `ids`) into `out`: five
    -- f32s each (left, top, width, height, depth), left NaN for none.
    local layouts = function(ids, n, out)
        for i = 0, n - 1 do
            local id = readu32(ids, i * 4)
            local o = objects[id]
            local v = o and watcher(id, o)()
            local at = i * 20
            if v then
                writef32(out, at, v[1])
                writef32(out, at + 4, v[2])
                writef32(out, at + 8, v[3])
                writef32(out, at + 12, v[4])
                writef32(out, at + 16, v[5])
            else
                writef32(out, at, nan)
            end
        end
    end
    -- The controls whose layout went dirty since last asked (u32 ids), or nil.
    local take_dirty = function()
        local n = #dirty
        if n == 0 then return nil end
        local out = buffer.create(n * 4)
        for i = 1, n do
            writeu32(out, (i - 1) * 4, dirty[i])
        end
        table.clear(dirty)
        table.clear(queued)
        return out
    end
    -- A destroyed control: its object and watcher go.
    local forget = function(id)
        objects[id] = nil
        local w = watchers[id]
        if w then
            watchers[id] = nil
            w:Destroy()
        end
    end
    -- moho's default relations (FAF's `LayoutHelpers.Reset` restores them):
    -- each edge from the opposite edge and the size, each size from the
    -- edges. Whatever FAF sets breaks the circle; a control laid out only
    -- by Left/Right (a StatusBar's bar) keeps its Width unused.
    local reset = function(o)
        o.Left:SetFunction(function() return o.Right() - o.Width() end)
        o.Top:SetFunction(function() return o.Bottom() - o.Height() end)
        o.Right:SetFunction(function() return o.Left() + o.Width() end)
        o.Bottom:SetFunction(function() return o.Top() + o.Height() end)
        o.Width:SetFunction(function() return o.Right() - o.Left() end)
        o.Height:SetFunction(function() return o.Bottom() - o.Top() end)
    end
    return layout, reset, { layouts = layouts, take_dirty = take_dirty, forget = forget, objects = objects }
"#;

const LAYOUT_VARS: [&str; 7] = ["Left", "Top", "Right", "Bottom", "Width", "Height", "Depth"];

/// Makes `obj` a native control of `kind` under `parent`: its handle, its
/// LazyVars, then `OnInit`.
fn create(lua: &Lua, obj: &LuaTable, parent: Option<&LuaTable>, kind: Kind) -> LuaResult<ControlId> {
    let parent = parent.map(id_of).transpose()?;
    let extra: &[&str] = match &kind {
        Kind::Bitmap(_) => &["BitmapWidth", "BitmapHeight"],
        Kind::Text(_) => &["TextAdvance", "FontAscent", "FontDescent", "FontExternalLeading"],
        Kind::Other("movie") => &["MovieWidth", "MovieHeight"],
        Kind::Border(_) => &["BorderWidth", "BorderHeight"],
        _ => &[],
    };
    let id = {
        let mut c = controls(lua);
        let id = c.next;
        c.next += 1;
        let key = lua.create_registry_value(obj.clone())?;
        c.map.insert(id, Control { parent, children: Vec::new(), kind, hidden: false, alpha: 1.0, hit_test: true, name: String::new(), needs_frame: false, render_pass: 0, layout: None, obj: key });
        if let Some(p) = parent.and_then(|p| c.map.get_mut(&p)) {
            p.children.push(id);
        }
        id
    };
    obj.raw_set(C_OBJECT, lua.create_userdata(ControlHandle(id))?)?;
    lua.named_registry_value::<LuaTable>("ui.objects")?.raw_set(id, obj)?;
    let make = lazyvar_create(lua)?;
    for name in LAYOUT_VARS.iter().chain(extra) {
        obj.raw_set(*name, make.call::<LuaValue>(0)?)?;
    }
    for name in extra {
        set_var(obj, name, 0)?;
    }
    lua.named_registry_value::<LuaFunction>("ui.reset")?.call::<()>(obj.clone())?;
    if let Some(on_init) = obj.get::<Option<LuaFunction>>("OnInit")? {
        on_init.call::<()>(obj.clone())?;
    }
    Ok(id)
}

/// The Lua object of control `id`.
pub fn object(lua: &Lua, id: ControlId) -> LuaResult<Option<LuaTable>> {
    let c = controls(lua);
    match c.map.get(&id) {
        Some(ctl) => Ok(Some(lua.registry_value(&ctl.obj)?)),
        None => Ok(None),
    }
}

/// `id` and its descendants, parents first.
fn subtree(c: &Controls, id: ControlId) -> Vec<ControlId> {
    let mut out = vec![id];
    let mut i = 0;
    while i < out.len() {
        if let Some(ctl) = c.map.get(&out[i]) {
            out.extend_from_slice(&ctl.children);
        }
        i += 1;
    }
    out
}

/// Destroys `id` and its subtree: `OnDestroy` on each (children first), then
/// removes them.
fn destroy(lua: &Lua, id: ControlId) -> LuaResult<()> {
    let ids = subtree(&controls(lua), id);
    for &d in ids.iter().rev() {
        if let Some(obj) = object(lua, d)?
            && let Some(f) = obj.get::<Option<LuaFunction>>("OnDestroy")?
            && let Err(e) = f.call::<()>(obj.clone()) {
            tracing::warn!(target: "lua", "UI OnDestroy failed: {e}");
        }
    }
    let forget: LuaFunction = lua.named_registry_value("ui.forget")?;
    for &d in &ids {
        forget.call::<()>(d)?;
    }
    let mut c = controls(lua);
    if let Some(p) = c.map.get(&id).and_then(|ctl| ctl.parent)
        && let Some(p) = c.map.get_mut(&p) {
        p.children.retain(|&ch| ch != id);
    }
    for d in ids {
        if let Some(ctl) = c.map.remove(&d) {
            let _ = lua.remove_registry_value(ctl.obj);
        }
    }
    c.frames.retain(|_, f| *f != id);
    Ok(())
}

/// The root frame for head `index` (created on first use: FAF's `Frame`
/// spanning the screen).
fn frame(lua: &Lua, index: i64) -> LuaResult<Obj<moho::Frame>> {
    let existing = controls(lua).frames.get(&index).copied();
    if let Some(obj) = existing.map(|id| object(lua, id)).transpose()?.flatten() {
        return Ok(Obj::new(obj));
    }
    let import: LuaFunction = lua.globals().get("import")?;
    let module: LuaTable = import.call("/lua/maui/frame.lua")?;
    let class: LuaTable = module.get("Frame")?;
    let obj: LuaTable = class.call(format!("frame {index}"))?;
    let id = id_of(&obj)?;
    let (w, h) = {
        let mut c = controls(lua);
        c.frames.insert(index, id);
        usize::try_from(index).ok().and_then(|i| c.screens.get(i)).or(c.screens.first()).copied().unwrap_or((1024.0, 768.0))
    };
    set_var(&obj, "Left", 0)?;
    set_var(&obj, "Top", 0)?;
    set_var(&obj, "Width", w)?;
    set_var(&obj, "Height", h)?;
    Ok(Obj::new(obj))
}

/// Head `head`'s screen is `w` x `h` (adding the head if it's new):
/// its root frame is resized. Only on a real change: setting a LazyVar
/// dirties everything laid out from it, and FAF reacts to dirty edges (a
/// `SpecialGrid` clears its items when its `Right` goes dirty, so the
/// construction panel emptied itself every frame).
pub fn set_screen(lua: &Lua, head: usize, w: f32, h: f32) -> LuaResult<()> {
    let root = {
        let mut c = controls(lua);
        if c.screens.get(head) == Some(&(w, h)) {
            return Ok(());
        }
        if head >= c.screens.len() {
            c.screens.resize(head + 1, (w, h));
        }
        c.screens[head] = (w, h);
        c.frames.get(&(head as i64)).copied()
    };
    if let Some(obj) = root.map(|id| object(lua, id)).transpose()?.flatten() {
        set_var(&obj, "Width", w)?;
        set_var(&obj, "Height", h)?;
    }
    Ok(())
}

/// The number of heads (`GetNumRootFrames`): moho fixes it when it makes
/// the device, so it is set before the interface starts. Heads beyond `n`
/// lose their screen (their frames stay, drawn nowhere).
pub fn set_heads(lua: &Lua, n: usize) {
    let mut c = controls(lua);
    let first = c.screens.first().copied().unwrap_or((1024.0, 768.0));
    c.screens.resize(n.max(1), first);
}

/// Mouse input comes from head `head`'s screen.
pub fn set_mouse_head(lua: &Lua, head: usize) {
    controls(lua).mouse_head = head as i64;
}

/// The root frame `id` is under (itself, for a root).
fn root_of(c: &Controls, mut id: ControlId) -> ControlId {
    while let Some(p) = c.map.get(&id).and_then(|ctl| ctl.parent) {
        id = p;
    }
    id
}

/// A map preview's texture: this prefix, then the `.scmap`'s VFS path (the
/// renderer decodes the preview image stored in the map).
pub const SCMAP_PREVIEW: &str = "#scmap:";

/// Width and height of a `.dds` / `.png` texture from its header (0, 0 if it won't read).
pub fn texture_dimensions(lua: &Lua, path: &str) -> (u32, u32) {
    texture_size(lua, path)
}

fn texture_size(lua: &Lua, path: &str) -> (u32, u32) {
    let Some(vfs) = lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().map(|v| v.clone()) else { return (0, 0) };
    let Ok(bytes) = vfs.read().map_err(|_| ()).and_then(|v| v.read(&path.to_lowercase()).map_err(|_| ())) else { return (0, 0) };
    crate::engine::assets::dds::image_size(&bytes).unwrap_or((0, 0))
}

fn measure(lua: &Lua, family: &str, size: f32, text: &str) -> crate::ui::fonts::Metrics {
    match lua.app_data_mut::<crate::ui::fonts::Fonts>() {
        Some(mut f) => f.measure(family, size, text),
        None => crate::ui::fonts::Metrics { ascent: size, descent: size * 0.25, advance: text.chars().count() as f32 * size * 0.5, leading: 0.0 },
    }
}

/// Runs `f` on the control behind `this`. A destroyed control's methods do
/// nothing (and return their default): FAF calls them after the control's
/// own handler destroyed it (`button.lua`'s `OnRelease` plays the button
/// after an `OnClick` that rebuilt the construction panel), which moho lets
/// through without an error.
fn with<R: Default>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Control) -> R) -> LuaResult<R> {
    let id = id_of(this)?;
    let mut c = controls(lua);
    let Some(ctl) = c.map.get_mut(&id) else {
        tracing::debug!(target: "lua", id, "a method on a destroyed control (ignored)");
        return Ok(R::default());
    };
    Ok(f(ctl))
}

/// Recomputes a text control's metric LazyVars from its font and text.
fn update_text_metrics(lua: &Lua, this: &LuaTable) -> LuaResult<()> {
    let Some(t) = with(lua, this, |c| match &c.kind {
        Kind::Text(t) => Some(t.clone()),
        _ => None,
    })?
    else {
        return Ok(());
    };
    let m = match lua.app_data_mut::<crate::ui::fonts::Fonts>() {
        Some(mut f) => f.measure(&t.family, t.size, &t.text),
        None => crate::ui::fonts::Metrics { ascent: t.size, descent: t.size * 0.25, advance: t.text.chars().count() as f32 * t.size * 0.5, leading: 0.0 },
    };
    set_var(this, "TextAdvance", m.advance)?;
    set_var(this, "FontAscent", m.ascent)?;
    set_var(this, "FontExternalLeading", m.leading)?;
    set_var(this, "FontDescent", m.descent)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Controls { screens: vec![(1024.0, 768.0)], ..Default::default() });
    let (layout_fn, reset_fn, cache): (LuaFunction, LuaFunction, LuaTable) = lua.load(LAYOUT_SRC).set_name("=[engine ui layout]").eval()?;
    lua.set_named_registry_value("ui.layout", layout_fn)?;
    lua.set_named_registry_value("ui.reset", reset_fn)?;
    for name in ["layouts", "take_dirty", "forget", "objects"] {
        lua.set_named_registry_value(&format!("ui.{name}"), cache.get::<LuaValue>(name)?)?;
    }
    let g = lua.globals();
    lua_fn!(lua, global,
        /// Dev / scripted UI tests (`--ui-lua`): every live control's Lua
        /// object, in creation order, so a script can find one (a button by its
        /// label's text) when FAF keeps it in a local.
        #[lua(internal)]
        __engine_controls = |lua, ()| {
            let ids: Vec<ControlId> = controls(lua).map.keys().copied().collect();
            let out = lua.create_table_with_capacity(ids.len(), 0)?;
            for id in ids {
                if let Some(obj) = object(lua, id)? {
                    out.push(obj)?;
                }
            }
            Ok(List::<Obj<moho::Control>>::new(out))
        })?;
    lua_fn!(lua, global,
        /// Makes `this` a root frame (FAF's `Frame.__init`).
        InternalCreateFrame = |lua, this: Obj<moho::Frame>| create(lua, &this, None, Kind::Frame).map(|_| ()))?;
    lua_fn!(lua, global,
        /// Makes `this` a group control under `parent`.
        InternalCreateGroup = |lua, (this, parent): (Obj<moho::Group>, Obj<moho::Control>)| create(lua, &this, Some(&parent), Kind::Group).map(|_| ()))?;
    lua_fn!(lua, global,
        /// Makes `this` a bitmap control under `parent`.
        InternalCreateBitmap = |lua, (this, parent): (Obj<moho::Bitmap>, Obj<moho::Control>)| create(lua, &this, Some(&parent), Kind::Bitmap(Bitmap { uv: [0.0, 0.0, 1.0, 1.0], ..Default::default() })).map(|_| ()))?;
    lua_fn!(lua, global,
        /// A map preview (`CUIMapPreview`) draws as a bitmap of the map's image.
        InternalCreateMapPreview = |lua, (this, parent): (Obj<moho::UIMapPreview>, Obj<moho::Control>)| create(lua, &this, Some(&parent), Kind::Bitmap(Bitmap { uv: [0.0, 0.0, 1.0, 1.0], ..Default::default() })).map(|_| ()))?;
    lua_fn!(lua, global,
        /// Makes `this` a text control under `parent`.
        InternalCreateText = |lua, (this, parent): (Obj<moho::Text>, Obj<moho::Control>)| create(lua, &this, Some(&parent), Kind::Text(Text::default())).map(|_| ()))?;
    lua_fn!(lua, global,
        /// Makes `this` a text input under `parent`.
        InternalCreateEdit = |lua, (this, parent): (Obj<moho::Edit>, Obj<moho::Control>)| create(lua, &this, Some(&parent), Kind::Edit(Edit::default())).map(|_| ()))?;
    lua_fn!(lua, global,
        /// Makes `this` a border control (the parent may be nil).
        InternalCreateBorder = |lua, (this, parent): (Obj<moho::Border>, Option<Obj<moho::Control>>)| create(lua, &this, parent.as_deref(), Kind::Border(Border::default())).map(|_| ()))?;
    for (name, kind) in [
        ("InternalCreateHistogram", "histogram"),
        ("InternalCreateMovie", "movie"),
        ("InternalCreateMesh", "mesh"),
    ] {
        crate::lua_api::register_fn(lua, &g, "", name, None, "", Status::Implemented, false, move |lua, (this, parent): (Obj<moho::Control>, Option<Obj<moho::Control>>)| create(lua, &this, parent.as_deref(), Kind::Other(kind)).map(|_| ()))?;
    }
    lua_fn!(lua, global,
        /// Makes `this` an item list under `parent`.
        InternalCreateItemList = |lua, (this, parent): (Obj<moho::ItemList>, Obj<moho::Control>)| create(lua, &this, Some(&parent), Kind::ItemList(ItemList::default())).map(|_| ()))?;
    lua_fn!(lua, global,
        /// Makes `this` a scrollbar under `parent`; `axis` is `"Horz"` or vertical (the default).
        InternalCreateScrollbar = |lua, (this, parent, axis): (Obj<moho::Scrollbar>, Obj<moho::Control>, Option<String>)| {
            let vertical = axis.as_deref() != Some("Horz");
            create(lua, &this, Some(&parent), Kind::Scrollbar(Scrollbar { vertical, ..Default::default() })).map(|_| ())
        })?;
    // World views (`moho.UIWorldView`): moho's class brings its own
    // `__init(self, parent, cameraName, depth, isMiniMap, trackCamera)`. The
    // control marks where the 3D view is; its camera is registered by name
    // (`GetCamera`). The renderer still draws one full-window view.
    lua_method!(lua, moho::UIWorldView,
        /// Creates the world view under `parent` (moho's `__init(self, parent, cameraName, depth, isMiniMap, trackCamera)`); `depth` and `trackCamera` are ignored. The camera is registered by name for `GetCamera`.
        __init = |lua, (this, parent, camera, _depth, mini, _track): (Obj<moho::UIWorldView>, Obj<moho::Control>, Option<String>, LuaValue, Option<bool>, LuaValue)| {
            create(lua, &this, Some(&parent), Kind::WorldView(WorldView { mini: mini.unwrap_or(false), camera: camera.clone(), ..Default::default() }))?;
            if let Some(name) = camera {
                let cameras: LuaTable = lua.named_registry_value("user.cameras")?;
                if cameras.raw_get::<LuaValue>(name.as_str())? == LuaNil {
                    let cam = lua.create_table()?;
                    cam.raw_set("_c_name", name.as_str())?;
                    // Its native side: a live camera for `IsDestroyed`
                    // (FAF's reclaim labels stop updating on a dead one).
                    cam.raw_set(C_OBJECT, lua.create_userdata(CameraHandle)?)?;
                    let meta = lua.create_table()?;
                    meta.raw_set("__index", moho::class_table(lua, "CameraImpl_methods")?)?;
                    cam.set_metatable(Some(meta));
                    cameras.raw_set(name.as_str(), cam)?;
                }
            }
            Ok(())
        })?;
    lua.set_named_registry_value("user.cameras", lua.create_table()?)?;
    lua_fn!(lua, global,
        /// The camera object registered under `name` by a world view, or nil.
        GetCamera = |lua, name: String| {
            let cameras: LuaTable = lua.named_registry_value("user.cameras")?;
            Ok(cameras.raw_get::<Option<LuaTable>>(name)?.map(Obj::<moho::CameraImpl>::new))
        })?;
    lua_fn!(lua, global,
        /// The root frame for head `index` (default 0).
        GetFrame = |lua, index: Option<i64>| frame(lua, index.unwrap_or(0)))?;
    lua_fn!(lua, global,
        /// nil, a control whose native side was destroyed, or a unit that
        /// is gone (`units.lua`'s UserUnits have no native side: FAF's
        /// `capping.lua` asks this of the structure it's about to ring).
        IsDestroyed = |lua, obj: LuaValue| {
            let LuaValue::Table(t) = obj else { return Ok(true) };
            match t.raw_get::<LuaValue>(C_OBJECT)? {
                LuaValue::UserData(ud) => match ud.borrow::<ControlHandle>() {
                    Ok(h) if h.0 != u32::MAX => Ok(!controls(lua).map.contains_key(&h.0)),
                    _ => Ok(false),
                },
                _ if t.raw_get::<Option<u32>>("_id")?.is_some() => t.call_method("IsDead", ()),
                _ => Ok(true),
            }
        })?;
    lua_fn!(lua, global,
        /// Draggers: a pressed button posts one; it gets the mouse until release.
        InternalCreateDragger = |lua, this: Obj<moho::Dragger>| this.raw_set(C_OBJECT, lua.create_userdata(ControlHandle(u32::MAX))?))?;
    lua_fn!(lua, global,
        /// Makes `dragger` the current dragger: it gets the mouse until release.
        PostDragger = |lua, (_root, _key, dragger): (Loose<Obj<moho::Frame>>, Loose<f64>, Obj<moho::Dragger>)| lua.set_named_registry_value("user.dragger", dragger))?;
    lua_method!(lua, moho::Dragger,
        /// Releases the mouse if this is the current dragger.
        Destroy = |lua, this: Obj<moho::Dragger>| {
            if let Some(d) = lua.named_registry_value::<Option<LuaTable>>("user.dragger")?
                && d == *this {
                lua.set_named_registry_value("user.dragger", LuaNil)?;
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The number of root frames: one per head (a second with FAF's
        /// Secondary Adapter on).
        GetNumRootFrames = |lua, ()| Ok(controls(lua).screens.len() as i64))?;
    lua_fn!(lua, global,
        /// Destroys every control in head `head`'s root frame (nil: every
        /// head's); the frame stays (FAF's `multihead.CreateSecondView`
        /// clears head 1, then builds on `GetFrame(1)`).
        ClearFrame = |lua, head: Option<f64>| {
            let roots: Vec<ControlId> = {
                let c = controls(lua);
                match head {
                    Some(h) => c.frames.get(&(h as i64)).copied().into_iter().collect(),
                    None => c.frames.values().copied().collect(),
                }
            };
            for root in roots {
                let children = controls(lua).map.get(&root).map(|c| c.children.clone()).unwrap_or_default();
                for ch in children {
                    destroy(lua, ch)?;
                }
            }
            Ok(())
        })?;

    lua_method!(lua, moho::Control,
        /// Destroys the control and its descendants.
        Destroy = |lua, this: Obj<moho::Control>| match id_of(&this) {
            Ok(id) => destroy(lua, id),
            Err(_) => Ok(()),
        })?;
    lua_method!(lua, moho::Control,
        /// The parent control, or nil for a root frame.
        GetParent = |lua, this: Obj<moho::Control>| {
            let p = with(lua, &this, |c| c.parent)?;
            match p {
                Some(p) => Ok(object(lua, p)?.map(Obj::<moho::Control>::new)),
                None => Ok(None),
            }
        })?;
    lua_method!(lua, moho::Control,
        /// Destroys every child.
        ClearChildren = |lua, this: Obj<moho::Control>| {
            let children = with(lua, &this, |c| c.children.clone())?;
            for ch in children {
                destroy(lua, ch)?;
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Control,
        /// Moves the control under another parent.
        SetParent = |lua, (this, parent): (Obj<moho::Control>, Obj<moho::Control>)| {
            let (id, p) = (id_of(&this)?, id_of(&parent)?);
            let mut c = controls(lua);
            if let Some(old) = c.map.get(&id).and_then(|ctl| ctl.parent)
                && let Some(o) = c.map.get_mut(&old) {
                o.children.retain(|&ch| ch != id);
            }
            if let Some(ctl) = c.map.get_mut(&id) {
                ctl.parent = Some(p);
            }
            if let Some(pc) = c.map.get_mut(&p) {
                pc.children.push(id);
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Control,
        /// Hit testing: recursive by default when enabling (annotations).
        DisableHitTest = |lua, (this, recursive): (Obj<moho::Control>, Option<bool>)| {
            let id = id_of(&this)?;
            let mut c = controls(lua);
            let ids = if recursive.unwrap_or(false) { subtree(&c, id) } else { vec![id] };
            for i in ids {
                if let Some(ctl) = c.map.get_mut(&i) {
                    ctl.hit_test = false;
                }
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Control,
        EnableHitTest = |lua, (this, recursive): (Obj<moho::Control>, Option<bool>)| {
            let id = id_of(&this)?;
            let mut c = controls(lua);
            let ids = if recursive.unwrap_or(true) { subtree(&c, id) } else { vec![id] };
            for i in ids {
                if let Some(ctl) = c.map.get_mut(&i) {
                    ctl.hit_test = true;
                }
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Control,
        /// True when the control doesn't take mouse events.
        IsHitTestDisabled = |lua, this: Obj<moho::Control>| with(lua, &this, |c| !c.hit_test))?;
    fn set_hidden(lua: &Lua, this: &LuaTable, hidden: bool) -> LuaResult<()> {
        let changed = with(lua, this, |c| std::mem::replace(&mut c.hidden, hidden) != hidden)?;
        if changed {
            notify_hide(lua, id_of(this)?, hidden)?;
        }
        Ok(())
    }
    /// `OnHide(hidden)` down the tree, depth first; a control's returning
    /// true keeps it from that control's own children only (FAF's
    /// economy bar shows its rate texts from `warningBG.OnHide`, after
    /// siblings that return true).
    fn notify_hide(lua: &Lua, id: ControlId, hidden: bool) -> LuaResult<()> {
        if let Some(obj) = object(lua, id)?
            && let Some(f) = obj.get::<Option<LuaFunction>>("OnHide")?
            && f.call::<Option<bool>>((obj.clone(), hidden)).unwrap_or(None) == Some(true) {
            return Ok(());
        }
        let children = controls(lua).map.get(&id).map(|c| c.children.clone()).unwrap_or_default();
        for child in children {
            notify_hide(lua, child, hidden)?;
        }
        Ok(())
    }
    lua_method!(lua, moho::Control,
        /// Hides the control; `OnHide(true)` runs down the tree.
        Hide = |lua, this: Obj<moho::Control>| set_hidden(lua, &this, true))?;
    lua_method!(lua, moho::Control,
        /// Shows the control; `OnHide(false)` runs down the tree.
        Show = |lua, this: Obj<moho::Control>| set_hidden(lua, &this, false))?;
    lua_method!(lua, moho::Control,
        /// Hides (`true`) or shows the control.
        SetHidden = |lua, (this, hidden): (Obj<moho::Control>, bool)| set_hidden(lua, &this, hidden))?;
    lua_method!(lua, moho::Control,
        IsHidden = |lua, this: Obj<moho::Control>| with(lua, &this, |c| c.hidden))?;
    lua_method!(lua, moho::Control,
        /// Sets the render pass index.
        SetRenderPass = |lua, (this, pass): (Obj<moho::Control>, i32)| with(lua, &this, |c| c.render_pass = pass))?;
    lua_method!(lua, moho::Control,
        GetRenderPass = |lua, this: Obj<moho::Control>| with(lua, &this, |c| c.render_pass))?;
    lua_method!(lua, moho::Control,
        /// Sets the alpha (clamped to 0..1); `children` applies it to the whole subtree (default false).
        SetAlpha = |lua, (this, alpha, children): (Obj<moho::Control>, f32, Option<bool>)| {
            let id = id_of(&this)?;
            let mut c = controls(lua);
            let ids = if children.unwrap_or(false) { subtree(&c, id) } else { vec![id] };
            for i in ids {
                if let Some(ctl) = c.map.get_mut(&i) {
                    ctl.alpha = alpha.clamp(0.0, 1.0);
                }
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Control,
        GetAlpha = |lua, this: Obj<moho::Control>| with(lua, &this, |c| c.alpha))?;
    lua_method!(lua, moho::Control,
        /// Sets the debug name.
        SetName = |lua, (this, name): (Obj<moho::Control>, String)| with(lua, &this, |c| c.name = name))?;
    lua_method!(lua, moho::Control,
        GetName = |lua, this: Obj<moho::Control>| with(lua, &this, |c| c.name.clone()))?;
    lua_method!(lua, moho::Control,
        /// Asks for `OnFrame(self, dt)` every interface frame.
        SetNeedsFrameUpdate = |lua, (this, on): (Obj<moho::Control>, bool)| with(lua, &this, |c| c.needs_frame = on))?;
    lua_method!(lua, moho::Control,
        NeedsFrameUpdate = |lua, this: Obj<moho::Control>| with(lua, &this, |c| c.needs_frame))?;
    lua_method!(lua, moho::Control,
        /// Keyboard focus: the newest taker gets key presses as `KeyDown`
        /// events (`key_down`); `blocksKeyDown` keeps them from the key map.
        AcquireKeyboardFocus = |lua, (this, blocks): (Obj<moho::Control>, Option<bool>)| acquire_focus(lua, id_of(&this)?, blocks.unwrap_or(false)))?;
    lua_method!(lua, moho::Control,
        /// Gives up the keyboard focus.
        AbandonKeyboardFocus = |lua, this: Obj<moho::Control>| {
            let id = id_of(&this)?;
            controls(lua).focus.retain(|f| f.0 != id);
            Ok(())
        })?;
    lua_method!(lua, moho::Control,
        #[lua(stub)]
        GetCurrentFocusControl = |_, _args: LuaMultiValue| Ok(LuaNil))?;
    lua_method!(lua, moho::Control,
        #[lua(stub)]
        Dump = |_, _args: LuaMultiValue| Ok(()))?;
    lua_method!(lua, moho::Control,
        /// The root frame the control is under (its head's).
        GetRootFrame = |lua, this: Obj<moho::Control>| {
            let head = {
                let c = controls(lua);
                let root = id_of(&this).map(|id| root_of(&c, id)).ok();
                c.frames.iter().find(|&(_, f)| Some(*f) == root).map_or(0, |(h, _)| *h)
            };
            frame(lua, head)
        })?;
    lua_method!(lua, moho::Control,
        /// Calls `f(control)` on the control and each descendant, parents first.
        ApplyFunction = |lua, (this, f): (Obj<moho::Control>, LuaFunction)| {
            let ids = subtree(&controls(lua), id_of(&this)?);
            for id in ids {
                if let Some(obj) = object(lua, id)? {
                    f.call::<()>(obj)?;
                }
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Control,
        /// True when the screen point `(x, y)` is inside Left..Right, Top..Bottom.
        HitTest = |lua, (this, x, y): (Obj<moho::Control>, f32, f32)| {
            let _ = lua;
            let (l, t, r, b) = (read_var(&this, "Left"), read_var(&this, "Top"), read_var(&this, "Right"), read_var(&this, "Bottom"));
            Ok(x >= l && y >= t && x < r && y < b)
        })?;

    // World view settings: FAF's interface sets and reads them back
    // (minimap options, the NIS pre/post state, cinematics' input lock).
    fn with_view<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut WorldView) -> R) -> LuaResult<Option<R>> {
        with(lua, this, |c| match &mut c.kind {
            Kind::WorldView(v) => Some(f(v)),
            _ => None,
        })
    }
    for (set, get, field) in [
        ("SetCartographic", Some("IsCartographic"), (|v| &mut v.cartographic) as fn(&mut WorldView) -> &mut bool),
        ("EnableResourceRendering", Some("IsResourceRenderingEnabled"), |v| &mut v.resources),
        ("SetCustomRender", None, |v| &mut v.custom_render),
        ("GetsGlobalCameraCommands", None, |v| &mut v.global_camera),
    ] {
        crate::lua_api::add_class_fn(lua, moho::UIWorldView::CLASS, set, None, "", Status::Implemented, move |lua, (this, on): (Obj<moho::UIWorldView>, Option<bool>)| with_view(lua, &this, |v| *field(v) = on.unwrap_or(false)).map(|_| ()))?;
        if let Some(get) = get {
            crate::lua_api::add_class_fn(lua, moho::UIWorldView::CLASS, get, None, "", Status::Implemented, move |lua, this: Obj<moho::UIWorldView>| Ok(with_view(lua, &this, |v| *field(v))?.unwrap_or(false)))?;
        }
    }
    lua_method!(lua, moho::UIWorldView,
        /// The order a right click would give where the mouse is (the renderer
        /// works it out each frame, `UserVm::set_mouse_order`); FAF's cursor
        /// shows it. Command-graph highlighting isn't drawn yet: never.
        GetRightMouseButtonOrder = |lua, _this: Obj<moho::UIWorldView>| lua.globals().raw_get::<Option<String>>("__engine_mouse_order"))?;
    lua_method!(lua, moho::UIWorldView,
        /// Hovering a waypoint whose moves a right click would turn into a
        /// patrol (the renderer decides each frame, `UserVm::set_convert_to_patrol`).
        ShowConvertToPatrolCursor = |lua, _this: Obj<moho::UIWorldView>| Ok(lua.globals().raw_get::<Option<bool>>("__engine_convert_to_patrol")?.unwrap_or(false)))?;
    lua_method!(lua, moho::UIWorldView,
        HasHighlightCommand = |lua, _this: Obj<moho::UIWorldView>| Ok(lua.globals().raw_get::<LuaValue>("__engine_highlight_command")? != LuaNil))?;
    lua_method!(lua, moho::UIWorldView,
        /// The camera argument is ignored: a view locks its own camera.
        LockInput = |lua, (this, _camera): (Obj<moho::UIWorldView>, Loose<Option<Obj<moho::CameraImpl>>>)| with_view(lua, &this, |v| v.input_locked = true).map(|_| ()))?;
    lua_method!(lua, moho::UIWorldView,
        UnlockInput = |lua, (this, _camera): (Obj<moho::UIWorldView>, Loose<Option<Obj<moho::CameraImpl>>>)| with_view(lua, &this, |v| v.input_locked = false).map(|_| ()))?;
    lua_method!(lua, moho::UIWorldView,
        IsInputLocked = |lua, (this, _camera): (Obj<moho::UIWorldView>, Loose<Option<Obj<moho::CameraImpl>>>)| Ok(with_view(lua, &this, |v| v.input_locked)?.unwrap_or(false)))?;
    lua_method!(lua, moho::UIWorldView,
        /// Every camera object drives the renderer's one camera, and the minimap
        /// always shows the whole map: only a main view resets.
        CameraReset = |lua, this: Obj<moho::UIWorldView>| {
            let Some(Some(name)) = with_view(lua, &this, |v| v.camera.clone().filter(|_| !v.mini))? else { return Ok(()) };
            let cameras: LuaTable = lua.named_registry_value("user.cameras")?;
            match cameras.raw_get::<Option<LuaTable>>(name)? {
                Some(cam) => cam.call_method("Reset", ()),
                None => Ok(()),
            }
        })?;

    lua_method!(lua, moho::UIWorldView,
        /// A wheel turned over a panel that passes it to the world (FAF's
        /// `ForwardMouseWheelInput`): the renderer zooms toward the cursor.
        ZoomScale = |lua, (_this, _x, _y, rotation, delta): (Obj<moho::UIWorldView>, Loose<f64>, Loose<f64>, f32, Option<f32>)| {
            let delta = delta.filter(|d| *d != 0.0).unwrap_or(120.0);
            let g = lua.globals();
            let q = match g.raw_get::<Option<LuaTable>>("__engine_camera_requests")? {
                Some(q) => q,
                None => {
                    let q = lua.create_table()?;
                    g.raw_set("__engine_camera_requests", q.clone())?;
                    q
                }
            };
            let r = lua.create_table()?;
            r.raw_set("wheel", rotation / delta)?;
            q.raw_set(q.raw_len() + 1, r)
        })?;

    // Scrollbars drive a scrollable control (FAF's `Grid`, `ItemList`, ...)
    // through its `GetScrollValues` / `ScrollLines` / `ScrollPages`.
    fn with_scrollbar<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Scrollbar) -> R) -> LuaResult<Option<R>> {
        with(lua, this, |c| match &mut c.kind {
            Kind::Scrollbar(s) => Some(f(s)),
            _ => None,
        })
    }
    lua_method!(lua, moho::Scrollbar,
        /// A nil texture keeps the current one (FAF's LazyVars set one at a time).
        SetNewTextures = |lua, (this, bg, mid, top, bottom): (Obj<moho::Scrollbar>, Option<String>, Option<String>, Option<String>, Option<String>)| {
            let size = |t: &Option<String>| t.as_deref().map(|t| texture_size(lua, t));
            let (top_size, bottom_size) = (size(&top), size(&bottom));
            with_scrollbar(lua, &this, |s| {
                let along = |(w, h): (u32, u32)| if s.vertical { h } else { w } as f32;
                if let Some(d) = top_size {
                    s.caps[0] = along(d);
                }
                if let Some(d) = bottom_size {
                    s.caps[1] = along(d);
                }
                for (slot, t) in s.textures.iter_mut().zip([bg, mid, top, bottom]) {
                    if t.is_some() {
                        *slot = t;
                    }
                }
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::Scrollbar,
        /// The control this scrollbar scrolls (nil to detach).
        SetScrollable = |lua, (this, scrollable): (Obj<moho::Scrollbar>, Option<Obj<moho::Control>>)| {
            let target = scrollable.as_ref().map(|s| id_of(s)).transpose()?;
            with_scrollbar(lua, &this, |s| s.scrollable = target).map(|_| ())
        })?;
    for (method, pages) in [("DoScrollLines", false), ("DoScrollPages", true)] {
        crate::lua_api::add_class_fn(lua, moho::Scrollbar::CLASS, method, None, "", Status::Implemented, move |lua, (this, amount): (Obj<moho::Scrollbar>, f64)| {
            let Some((Some(target), vertical)) = with_scrollbar(lua, &this, |s| (s.scrollable, s.vertical))? else { return Ok(()) };
            scroll(lua, target, if vertical { "Vert" } else { "Horz" }, if pages { Scroll::Pages(amount) } else { Scroll::Lines(amount) })
        })?;
    }

    // Item lists: rows of text, scrollable like FAF's own lists.
    fn with_list<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut ItemList) -> R) -> LuaResult<Option<R>> {
        with(lua, this, |c| match &mut c.kind {
            Kind::ItemList(l) => Some(f(l)),
            _ => None,
        })
    }
    fn item_text(v: Loose<Either<String, f64>>) -> String {
        match v.into_inner() {
            LuaValue::String(s) => s.to_string_lossy(),
            LuaValue::Integer(i) => i.to_string(),
            LuaValue::Number(n) => n.to_string(),
            other => format!("{other:?}"),
        }
    }
    lua_method!(lua, moho::ItemList,
        /// Appends a row (numbers are formatted as text).
        AddItem = |lua, (this, text): (Obj<moho::ItemList>, Loose<Either<String, f64>>)| with_list(lua, &this, |l| l.items.push(item_text(text))).map(|_| ()))?;
    lua_method!(lua, moho::ItemList,
        /// Replaces row `i` (0-based); out of range does nothing.
        ModifyItem = |lua, (this, i, text): (Obj<moho::ItemList>, i64, Loose<Either<String, f64>>)| {
            with_list(lua, &this, |l| {
                if let Some(row) = usize::try_from(i).ok().and_then(|i| l.items.get_mut(i)) {
                    *row = item_text(text);
                }
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::ItemList,
        /// Removes row `i` (0-based), keeping the selection on the same row.
        DeleteItem = |lua, (this, i): (Obj<moho::ItemList>, i64)| {
            with_list(lua, &this, |l| {
                let Ok(i) = usize::try_from(i) else { return };
                if i < l.items.len() {
                    l.items.remove(i);
                    l.selection = match l.selection {
                        Some(s) if s == i => None,
                        Some(s) if s > i => Some(s - 1),
                        s => s,
                    };
                }
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::ItemList,
        DeleteAllItems = |lua, this: Obj<moho::ItemList>| {
            with_list(lua, &this, |l| {
                l.items.clear();
                l.selection = None;
                l.top = 0;
                l.hover = None;
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::ItemList,
        Empty = |lua, this: Obj<moho::ItemList>| Ok(with_list(lua, &this, |l| l.items.is_empty())?.unwrap_or(true)))?;
    lua_method!(lua, moho::ItemList,
        /// Row `i` (0-based) as text, or nil.
        GetItem = |lua, (this, i): (Obj<moho::ItemList>, i64)| with_list(lua, &this, |l| usize::try_from(i).ok().and_then(|i| l.items.get(i).cloned())).map(Option::flatten))?;
    lua_method!(lua, moho::ItemList,
        GetItemCount = |lua, this: Obj<moho::ItemList>| Ok(with_list(lua, &this, |l| l.items.len())?.unwrap_or(0)))?;
    lua_method!(lua, moho::ItemList,
        /// The selected row (0-based), or -1.
        GetSelection = |lua, this: Obj<moho::ItemList>| Ok(with_list(lua, &this, |l| l.selection.map_or(-1, |s| s as i64))?.unwrap_or(-1)))?;
    lua_method!(lua, moho::ItemList,
        /// Selects row `i` (0-based); out of range clears the selection.
        SetSelection = |lua, (this, i): (Obj<moho::ItemList>, i64)| with_list(lua, &this, |l| l.selection = usize::try_from(i).ok().filter(|&i| i < l.items.len())).map(|_| ()))?;
    lua_method!(lua, moho::ItemList,
        ShowSelection = |lua, (this, on): (Obj<moho::ItemList>, bool)| with_list(lua, &this, |l| l.show_selection = on).map(|_| ()))?;
    lua_method!(lua, moho::ItemList,
        ShowMouseoverItem = |lua, (this, on): (Obj<moho::ItemList>, bool)| with_list(lua, &this, |l| l.show_mouseover = on).map(|_| ()))?;
    lua_method!(lua, moho::ItemList,
        SetNewFont = |lua, (this, family, size): (Obj<moho::ItemList>, Option<String>, Option<f32>)| {
            with_list(lua, &this, |l| {
                if let Some(f) = family {
                    l.family = f;
                }
                if let Some(s) = size {
                    l.size = s;
                }
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::ItemList,
        /// A nil colour keeps the current one (FAF's LazyVars set one at a time).
        SetNewColors = |lua, (this, colors): (Obj<moho::ItemList>, LuaMultiValue)| {
            let parsed: Vec<Option<[f32; 4]>> = colors.iter().take(6).map(|v| match v {
                LuaValue::String(s) => parse_color(&s.to_string_lossy()),
                _ => None,
            }).collect();
            with_list(lua, &this, |l| {
                for (slot, c) in l.colors.iter_mut().zip(parsed) {
                    if c.is_some() {
                        *slot = c;
                    }
                }
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::ItemList,
        GetRowHeight = |lua, this: Obj<moho::ItemList>| Ok(with_list(lua, &this, |l| l.clone())?.map_or(0.0, |l| row_height(lua, &l))))?;
    lua_method!(lua, moho::ItemList,
        GetStringAdvance = |lua, (this, text): (Obj<moho::ItemList>, String)| {
            let Some((family, size)) = with_list(lua, &this, |l| (l.family.clone(), l.size))? else { return Ok(0.0) };
            Ok(measure(lua, &family, size, &text).advance)
        })?;
    lua_method!(lua, moho::ItemList,
        /// Scrolling: moho's own methods; its scrollbar and the wheel scroll it
        /// natively too (`scroll`), not through Lua (FAF's `Control` has no-op
        /// Lua versions of the scrollable interface, which `ItemList` keeps).
        ScrollToTop = |lua, this: Obj<moho::ItemList>| scroll(lua, id_of(&this)?, "Vert", Scroll::SetTop(0.0)))?;
    lua_method!(lua, moho::ItemList,
        ScrollToBottom = |lua, this: Obj<moho::ItemList>| scroll(lua, id_of(&this)?, "Vert", Scroll::SetTop(f64::MAX)))?;
    lua_method!(lua, moho::ItemList,
        ShowItem = |lua, (this, i): (Obj<moho::ItemList>, i64)| {
            let id = id_of(&this)?;
            let Some((_, _, top, bottom)) = list_values(lua, id)? else { return Ok(()) };
            let i = i as f64;
            let to = if i < top { i } else if i >= bottom { i - (bottom - top) + 1.0 } else { top };
            scroll(lua, id, "Vert", Scroll::SetTop(to))
        })?;
    lua_method!(lua, moho::ItemList,
        NeedsScrollBar = |lua, this: Obj<moho::ItemList>| Ok(list_values(lua, id_of(&this)?)?.is_some_and(|(min, max, top, bottom)| bottom - top < max - min)))?;

    lua_method!(lua, moho::UIMapPreview,
        /// `SetTexture(file)`: the scenario's own preview image, if it reads.
        SetTexture = |lua, (this, file): (Obj<moho::UIMapPreview>, Option<String>)| {
            let file = file.unwrap_or_default();
            let found = !file.is_empty() && texture_size(lua, &file) != (0, 0);
            if found {
                with(lua, &this, |c| if let Kind::Bitmap(b) = &mut c.kind { b.textures = vec![file] })?;
            }
            Ok(found)
        })?;
    lua_method!(lua, moho::UIMapPreview,
        /// `SetTextureFromMap(scmap)`: the preview image inside the map file
        /// (`#scmap:<path>`, which the renderer decodes).
        SetTextureFromMap = |lua, (this, map): (Obj<moho::UIMapPreview>, Option<String>)| {
            let map = map.unwrap_or_default().to_lowercase();
            let vfs = lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().map(|v| v.clone());
            let found = !map.is_empty() && vfs.is_some_and(|v| v.read().is_ok_and(|v| v.read(&map).is_ok()));
            if found {
                with(lua, &this, |c| if let Kind::Bitmap(b) = &mut c.kind { b.textures = vec![format!("{SCMAP_PREVIEW}{map}")] })?;
            }
            Ok(found)
        })?;
    lua_method!(lua, moho::UIMapPreview,
        ClearTexture = |lua, this: Obj<moho::UIMapPreview>| with(lua, &this, |c| if let Kind::Bitmap(b) = &mut c.kind { b.textures.clear() }))?;

    fn with_bitmap<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Bitmap) -> R) -> LuaResult<Option<R>> {
        with(lua, this, |c| match &mut c.kind {
            Kind::Bitmap(b) => Some(f(b)),
            _ => None,
        })
    }
    lua_method!(lua, moho::Bitmap,
        /// Sets the texture (a path, or a list of paths for an animation) and updates `BitmapWidth` / `BitmapHeight`.
        SetNewTexture = |lua, (this, file, _border): (Obj<moho::Bitmap>, Loose<Either<String, Vec<String>>>, Option<f64>)| {
        let textures: Vec<String> = match &*file {
            LuaValue::String(s) => vec![s.to_str()?.to_string()],
            LuaValue::Table(t) => t.clone().sequence_values::<String>().collect::<LuaResult<_>>()?,
            _ => Vec::new(),
        };
        let (w, h) = textures.first().map_or((0, 0), |t| texture_size(lua, t));
        with_bitmap(lua, &this, |b| {
            b.textures = textures;
            b.frame = 0;
            b.solid = None;
        })?;
        set_var(&this, "BitmapWidth", w)?;
        set_var(&this, "BitmapHeight", h)
    })?;
    lua_method!(lua, moho::Bitmap,
        InternalSetSolidColor = |lua, (this, color): (Obj<moho::Bitmap>, Option<String>)| {
            let rgba = color.as_deref().and_then(parse_color);
            with_bitmap(lua, &this, |b| {
                b.solid = rgba;
                if rgba.is_some() {
                    b.textures.clear();
                }
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::Bitmap,
        SetColorMask = |lua, (this, color): (Obj<moho::Bitmap>, Option<String>)| with_bitmap(lua, &this, |b| b.mask = color.as_deref().and_then(parse_color)).map(|_| ()))?;
    lua_method!(lua, moho::Bitmap,
        /// The sub-rectangle of the texture drawn, `(u0, v0, u1, v1)` in 0..1.
        SetUV = |lua, (this, u0, v0, u1, v1): (Obj<moho::Bitmap>, f32, f32, f32, f32)| with_bitmap(lua, &this, |b| b.uv = [u0, v0, u1, v1]).map(|_| ()))?;
    lua_method!(lua, moho::Bitmap,
        SetTiled = |lua, (this, on): (Obj<moho::Bitmap>, bool)| with_bitmap(lua, &this, |b| b.tiled = on).map(|_| ()))?;
    lua_method!(lua, moho::Bitmap,
        GetNumFrames = |lua, this: Obj<moho::Bitmap>| with_bitmap(lua, &this, |b| b.textures.len().max(1)).map(|n| n.unwrap_or(1)))?;
    lua_method!(lua, moho::Bitmap,
        /// Shows frame `f` (clamped to the last).
        SetFrame = |lua, (this, f): (Obj<moho::Bitmap>, usize)| with_bitmap(lua, &this, |b| b.frame = f.min(b.textures.len().saturating_sub(1))).map(|_| ()))?;
    lua_method!(lua, moho::Bitmap,
        GetFrame = |lua, this: Obj<moho::Bitmap>| with_bitmap(lua, &this, |b| b.frame).map(|f| f.unwrap_or(0)))?;
    lua_method!(lua, moho::Bitmap,
        /// Copies another bitmap's textures and solid colour.
        ShareTextures = |lua, (this, other): (Obj<moho::Bitmap>, Obj<moho::Control>)| {
        let src = with(lua, &other, |c| match &c.kind {
            Kind::Bitmap(b) => Some(b.clone()),
            _ => None,
        })?;
        if let Some(src) = src {
            with_bitmap(lua, &this, |b| {
                b.textures = src.textures;
                b.solid = src.solid;
            })?;
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Bitmap,
        /// Frame animation, advanced each interface frame (`animate`).
        Play = |lua, this: Obj<moho::Bitmap>| {
            with_bitmap(lua, &this, |b| {
                b.anim.playing = true;
                b.anim.step = 0;
                b.anim.elapsed = 0.0;
                let first = b.anim.pattern.as_ref().and_then(|p| p.first().copied()).unwrap_or(0);
                b.frame = first.min(b.textures.len().saturating_sub(1));
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::Bitmap,
        /// Stops the animation; `OnAnimationStopped` runs if it was playing.
        Stop = |lua, this: Obj<moho::Bitmap>| {
            let was = with_bitmap(lua, &this, |b| std::mem::replace(&mut b.anim.playing, false))?.unwrap_or(false);
            if was
                && let Some(f) = this.get::<Option<LuaFunction>>("OnAnimationStopped")? {
                f.call::<()>(this.clone())?;
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Bitmap,
        /// Sets whether the animation loops.
        Loop = |lua, (this, on): (Obj<moho::Bitmap>, bool)| with_bitmap(lua, &this, |b| b.anim.looping = on).map(|_| ()))?;
    lua_method!(lua, moho::Bitmap,
        /// Frames per second (negative clamps to 0).
        SetFrameRate = |lua, (this, fps): (Obj<moho::Bitmap>, f32)| with_bitmap(lua, &this, |b| b.anim.rate = fps.max(0.0)).map(|_| ()))?;
    lua_method!(lua, moho::Bitmap,
        /// The frame order, as a list of frame indices.
        SetFramePattern = |lua, (this, frames): (Obj<moho::Bitmap>, Vec<usize>)| with_bitmap(lua, &this, |b| b.anim.pattern = Some(frames)).map(|_| ()))?;
    for (name, kind) in [("SetForwardPattern", "forward"), ("SetBackwardPattern", "backward"), ("SetPingPongPattern", "pingpong"), ("SetLoopPingPongPattern", "looppingpong")] {
        crate::lua_api::add_class_fn(lua, moho::Bitmap::CLASS, name, None, "", Status::Implemented, move |lua, this: Obj<moho::Bitmap>| {
            with_bitmap(lua, &this, |b| {
                b.anim.pattern = Some(pattern(kind, b.textures.len().max(1)));
                if kind == "looppingpong" {
                    b.anim.looping = true;
                }
            })
            .map(|_| ())
        })?;
    }
    lua_method!(lua, moho::Bitmap,
        #[lua(stub)]
        UseAlphaHitTest = |_, _args: LuaMultiValue| Ok(()))?;

    fn with_border<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Border) -> R) -> LuaResult<Option<R>> {
        with(lua, this, |c| match &mut c.kind {
            Kind::Border(b) => Some(f(b)),
            _ => None,
        })
    }
    lua_method!(lua, moho::Border,
        /// `SetNewTextures(vertical, horizontal, ul, ur, ll, lr)`: nil leaves that
        /// one; the side textures set `BorderWidth` / `BorderHeight`.
        SetNewTextures = |lua, (this, v, h, ul, ur, ll, lr): (Obj<moho::Border>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)| {
        let (vs, hs) = (v.as_deref().map(|t| texture_size(lua, t)), h.as_deref().map(|t| texture_size(lua, t)));
        with_border(lua, &this, |b| {
            for (i, t) in [v, h, ul, ur, ll, lr].into_iter().enumerate() {
                if t.is_some() {
                    b.textures[i] = t;
                }
            }
            b.solid = None;
        })?;
        if let Some((w, _)) = vs {
            set_var(&this, "BorderWidth", w)?;
        }
        if let Some((_, hh)) = hs {
            set_var(&this, "BorderHeight", hh)?;
        }
        Ok(())
    })?;
    lua_method!(lua, moho::Border,
        SetSolidColor = |lua, (this, color): (Obj<moho::Border>, Option<String>)| with_border(lua, &this, |b| b.solid = color.as_deref().and_then(parse_color)).map(|_| ()))?;

    fn with_text<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Text) -> R) -> LuaResult<Option<R>> {
        with(lua, this, |c| match &mut c.kind {
            Kind::Text(t) => Some(f(t)),
            _ => None,
        })
    }
    lua_method!(lua, moho::Text,
        /// Sets the text (nil is empty, other values are converted to a string) and updates the text metrics.
        SetText = |lua, (this, text): (Obj<moho::Text>, Loose<Option<Either<String, f64>>>)| {
            let text = match text.into_inner() {
                LuaValue::String(s) => s.to_str()?.to_string(),
                LuaValue::Nil => String::new(),
                v => lua.coerce_string(v)?.map(|s| s.to_str().map(|s| s.to_string())).transpose()?.unwrap_or_default(),
            };
            with_text(lua, &this, |t| t.text = text)?;
            update_text_metrics(lua, &this)
        })?;
    lua_method!(lua, moho::Text,
        /// The text.
        GetText = |lua, this: Obj<moho::Text>| with_text(lua, &this, |t| t.text.clone()).map(Option::unwrap_or_default))?;
    lua_method!(lua, moho::Text,
        SetNewFont = |lua, (this, family, size): (Obj<moho::Text>, Option<String>, Option<f32>)| {
        with_text(lua, &this, |t| {
            if let Some(f) = family {
                t.family = f;
            }
            if let Some(s) = size {
                t.size = s;
            }
        })?;
        update_text_metrics(lua, &this)
    })?;
    lua_method!(lua, moho::Text,
        /// Sets the colour (`ffRRGGBB`, `RRGGBB` or a colour name); an unreadable one is white.
        SetNewColor = |lua, (this, color): (Obj<moho::Text>, Option<String>)| {
            let rgba = color.as_deref().and_then(parse_color).unwrap_or([1.0; 4]);
            with_text(lua, &this, |t| t.color = rgba).map(|_| ())
        })?;
    lua_method!(lua, moho::Text,
        SetDropShadow = |lua, (this, on): (Obj<moho::Text>, bool)| with_text(lua, &this, |t| t.shadow = on).map(|_| ()))?;
    lua_method!(lua, moho::Text,
        SetCenteredHorizontally = |lua, (this, on): (Obj<moho::Text>, bool)| with_text(lua, &this, |t| t.center_h = on).map(|_| ()))?;
    lua_method!(lua, moho::Text,
        SetCenteredVertically = |lua, (this, on): (Obj<moho::Text>, bool)| with_text(lua, &this, |t| t.center_v = on).map(|_| ()))?;
    lua_method!(lua, moho::Text,
        /// Draws the text turned a quarter turn (engine extension, not moho's:
        /// the ping grid's column labels).
        SetVertical = |lua, (this, on): (Obj<moho::Text>, bool)| with_text(lua, &this, |t| t.vertical = on).map(|_| ()))?;
    lua_method!(lua, moho::Text,
        SetNewClipToWidth = |lua, (this, on): (Obj<moho::Text>, bool)| with_text(lua, &this, |t| t.clip = on).map(|_| ()))?;
    lua_method!(lua, moho::Text,
        /// The width in pixels of `text` in this control's font.
        GetStringAdvance = |lua, (this, text): (Obj<moho::Text>, String)| {
            let font = with_text(lua, &this, |t| (t.family.clone(), t.size))?.unwrap_or(("Arial".into(), 12.0));
            Ok(match lua.app_data_mut::<crate::ui::fonts::Fonts>() {
                Some(mut f) => f.measure(&font.0, font.1, &text).advance,
                None => text.chars().count() as f32 * font.1 * 0.5,
            })
        })?;

    fn with_edit<R>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut Edit) -> R) -> LuaResult<Option<R>> {
        with(lua, this, |c| match &mut c.kind {
            Kind::Edit(e) => Some(f(e)),
            _ => None,
        })
    }
    lua_method!(lua, moho::Edit,
        /// Sets the text, cut to `MaxChars`; the caret moves to the end.
        SetText = |lua, (this, text): (Obj<moho::Edit>, Option<String>)| {
            with_edit(lua, &this, |e| {
                let t: String = text.unwrap_or_default().chars().take(e.max_chars.max(0) as usize).collect();
                e.caret = t.chars().count() as i64;
                e.text.text = t;
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::Edit,
        GetText = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.text.text.clone()).map(Option::unwrap_or_default))?;
    lua_method!(lua, moho::Edit,
        ClearText = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| {
            e.text.text.clear();
            e.caret = 0;
        }).map(|_| ()))?;
        lua_method!(lua, moho::Edit,
            SetNewFont = |lua, (this, family, size): (Obj<moho::Edit>, Option<String>, Option<f32>)| {
            with_edit(lua, &this, |e| {
                if let Some(f) = family {
                    e.text.family = f;
                }
                if let Some(s) = size {
                    e.text.size = s;
                }
            })
            .map(|_| ())
        })?;
    lua_method!(lua, moho::Edit,
        /// The line height in pixels (ascent + descent, rounded up).
        GetFontHeight = |lua, this: Obj<moho::Edit>| {
            let (family, size) = with_edit(lua, &this, |e| (e.text.family.clone(), e.text.size))?.unwrap_or(("Arial".into(), 12.0));
            let m = measure(lua, &family, size, "");
            Ok((m.ascent + m.descent).ceil())
        })?;
    lua_method!(lua, moho::Edit,
        /// The width in pixels of `text` in this edit's font.
        GetStringAdvance = |lua, (this, text): (Obj<moho::Edit>, String)| {
            let (family, size) = with_edit(lua, &this, |e| (e.text.family.clone(), e.text.size))?.unwrap_or(("Arial".into(), 12.0));
            Ok(measure(lua, &family, size, &text).advance)
        })?;
    lua_method!(lua, moho::Edit,
        SetNewForegroundColor = |lua, (this, c): (Obj<moho::Edit>, Option<String>)| with_edit(lua, &this, |e| e.text.color = c.as_deref().and_then(parse_color).unwrap_or([1.0; 4])).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        SetNewBackgroundColor = |lua, (this, c): (Obj<moho::Edit>, Option<String>)| with_edit(lua, &this, |e| e.background = c.as_deref().and_then(parse_color).unwrap_or([0.0; 4])).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        ShowBackground = |lua, (this, on): (Obj<moho::Edit>, bool)| with_edit(lua, &this, |e| e.show_background = on).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        IsBackgroundVisible = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.show_background).map(|v| v.unwrap_or(false)))?;
    lua_method!(lua, moho::Edit,
        SetMaxChars = |lua, (this, n): (Obj<moho::Edit>, i64)| with_edit(lua, &this, |e| e.max_chars = n).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        GetMaxChars = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.max_chars).map(|v| v.unwrap_or(0)))?;
    lua_method!(lua, moho::Edit,
        EnableInput = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.enabled = true).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        DisableInput = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.enabled = false).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        IsEnabled = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.enabled).map(|v| v.unwrap_or(false)))?;
    lua_method!(lua, moho::Edit,
        SetDropShadow = |lua, (this, on): (Obj<moho::Edit>, bool)| with_edit(lua, &this, |e| e.text.shadow = on).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        GetCaretPosition = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.caret).map(|v| v.unwrap_or(0)))?;
    lua_method!(lua, moho::Edit,
        SetCaretPosition = |lua, (this, p): (Obj<moho::Edit>, i64)| with_edit(lua, &this, |e| e.caret = p.clamp(0, e.text.text.chars().count() as i64)).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        /// An edit with the focus takes every key (typing doesn't run hotkeys).
        AcquireFocus = |lua, this: Obj<moho::Edit>| acquire_focus(lua, id_of(&this)?, true))?;
    lua_method!(lua, moho::Edit,
        AbandonFocus = |lua, this: Obj<moho::Edit>| {
            let id = id_of(&this)?;
            controls(lua).focus.retain(|f| f.0 != id);
            Ok(())
        })?;
    lua_method!(lua, moho::Edit,
        SetNewCaretColor = |lua, (this, c): (Obj<moho::Edit>, Option<String>)| with_edit(lua, &this, |e| e.caret_color = c.as_deref().and_then(parse_color).unwrap_or([1.0; 4])).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        ShowCaret = |lua, (this, on): (Obj<moho::Edit>, bool)| with_edit(lua, &this, |e| e.caret_shown = on).map(|_| ()))?;
    lua_method!(lua, moho::Edit,
        IsCaretVisible = |lua, this: Obj<moho::Edit>| with_edit(lua, &this, |e| e.caret_shown).map(|v| v.unwrap_or(false)))?;
    lua_method!(lua, moho::Edit,
        /// The caret blink: period in seconds, then min and max alpha (0..255, defaults 0 and 255).
        SetCaretCycle = |lua, (this, seconds, min, max): (Obj<moho::Edit>, f32, Option<f32>, Option<f32>)| {
            with_edit(lua, &this, |e| e.caret_cycle = Some((seconds, min.unwrap_or(0.0) / 255.0, max.unwrap_or(255.0) / 255.0))).map(|_| ())
        })?;
    // Text selection isn't kept: the highlight colours are accepted.
    for name in ["SetNewHighlightBackgroundColor", "SetNewHighlightForegroundColor"] {
        crate::lua_api::add_class_fn(lua, moho::Edit::CLASS, name, None, "", Status::Partial("text selection isn't kept; the colours are accepted"), |_, _args: LuaMultiValue| Ok(()))?;
    }

    lua_method!(lua, moho::Frame,
        /// The deepest control under the frame: FAF stacks popups and overlays
        /// at this plus a bit, so each new one goes over everything shown.
        GetTopmostDepth = |lua, this: Obj<moho::Frame>| {
            let objs: Vec<LuaTable> = {
                let c = controls(lua);
                subtree(&c, id_of(&this)?).iter().filter_map(|id| c.map.get(id)).map(|ctl| lua.registry_value(&ctl.obj)).collect::<LuaResult<_>>()?
            };
            Ok(objs.iter().map(|o| read_var(o, "Depth")).fold(0.0f32, f32::max))
        })?;
    lua_method!(lua, moho::Frame,
        #[lua(stub)]
        GetTargetHead = |_, _args: LuaMultiValue| Ok(0))?;
    lua_method!(lua, moho::Frame,
        #[lua(stub)]
        SetTargetHead = |_, _args: LuaMultiValue| Ok(()))?;
    Ok(())
}

/// Runs `OnFrame(self, dt)` on every control that asked for frame updates.
/// Advances playing bitmaps by `dt` seconds: `OnAnimationFrame(frame)` on
/// each new frame, and `OnAnimationFinished()` when a non-looping pattern
/// ends (callbacks after the step, in control order).
fn animate(lua: &Lua, dt: f32) -> LuaResult<()> {
    let mut events: Vec<(ControlId, Option<usize>, bool)> = Vec::new();
    {
        let mut c = controls(lua);
        for (&id, ctl) in c.map.iter_mut() {
            let Kind::Bitmap(b) = &mut ctl.kind else { continue };
            if !b.anim.playing {
                continue;
            }
            let frames = b.textures.len().max(1);
            let pat = b.anim.pattern.clone().unwrap_or_else(|| pattern("forward", frames));
            if pat.is_empty() {
                b.anim.playing = false;
                continue;
            }
            let rate = if b.anim.rate > 0.0 { b.anim.rate } else { 10.0 };
            b.anim.elapsed += dt;
            let mut changed = None;
            let mut finished = false;
            while b.anim.elapsed >= 1.0 / rate {
                b.anim.elapsed -= 1.0 / rate;
                if b.anim.step + 1 < pat.len() {
                    b.anim.step += 1;
                } else if b.anim.looping {
                    b.anim.step = 0;
                } else {
                    b.anim.playing = false;
                    finished = true;
                    break;
                }
                b.frame = pat[b.anim.step].min(frames - 1);
                changed = Some(b.frame);
            }
            if changed.is_some() || finished {
                events.push((id, changed, finished));
            }
        }
    }
    for (id, frame, finished) in events {
        let Some(obj) = object(lua, id)? else { continue };
        if let Some(f) = frame
            && let Some(cb) = obj.get::<Option<LuaFunction>>("OnAnimationFrame")?
            && let Err(e) = cb.call::<()>((obj.clone(), f)) {
            tracing::warn!(target: "lua", "UI OnAnimationFrame failed: {e}");
        }
        if finished
            && let Some(cb) = obj.get::<Option<LuaFunction>>("OnAnimationFinished")?
            && let Err(e) = cb.call::<()>(obj.clone()) {
            tracing::warn!(target: "lua", "UI OnAnimationFinished failed: {e}");
        }
    }
    Ok(())
}

pub fn on_frame(lua: &Lua, dt: f32) -> LuaResult<()> {
    animate(lua, dt)?;
    let ids: Vec<ControlId> = controls(lua).map.iter().filter(|(_, c)| c.needs_frame).map(|(&id, _)| id).collect();
    for id in ids {
        let Some(obj) = object(lua, id)? else { continue };
        if let Some(f) = obj.get::<Option<LuaFunction>>("OnFrame")?
            && let Err(e) = f.call::<()>((obj.clone(), dt)) {
            tracing::warn!(target: "lua", "UI OnFrame failed: {e}");
        }
    }
    Ok(())
}

/// The visible controls' draw items, back to front (by `Depth`, then tree
/// order), each at its own alpha: moho doesn't pass a parent's alpha down
/// (`SetAlpha(a, true)` sets the children's too), so FAF's economy bar can
/// fade its warning background without fading the rate texts on it.
pub fn draw_list(lua: &Lua, head: usize) -> LuaResult<Vec<DrawItem>> {
    let roots: Vec<ControlId> = controls(lua).frames.get(&(head as i64)).copied().into_iter().collect();
    // Each item with its depth, push order and whether it's drawn under the
    // world views.
    let mut items: Vec<(f32, usize, bool, DrawItem)> = Vec::new();
    let focused = focused_edit(lua);
    let now = lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0);
    // The visible controls that draw something, in tree order, under one
    // borrow; then the layouts not kept, in one call.
    forget_dirty_layouts(lua)?;
    let mut drawn: Vec<ControlId> = Vec::new();
    let mut stale: Vec<ControlId> = Vec::new();
    {
        let c = controls(lua);
        let mut stack: Vec<ControlId> = roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(ctl) = c.map.get(&id) else { continue };
            if ctl.hidden {
                continue;
            }
            stack.extend(ctl.children.iter().rev());
            let kind = &ctl.kind;
            let draws = matches!(kind, Kind::Other("movie") | Kind::Border(_)) || matches!(kind, Kind::Bitmap(b) if !b.textures.is_empty() || b.solid.is_some()) || matches!(kind, Kind::Text(t) if !t.text.is_empty()) || matches!(kind, Kind::Edit(_) | Kind::Scrollbar(_) | Kind::ItemList(_) | Kind::WorldView(WorldView { mini: true, .. }) | Kind::WorldView(WorldView { mini: false, camera: Some(_), .. }));
            if draws {
                drawn.push(id);
                if ctl.layout.is_none() {
                    stale.push(id);
                }
            }
        }
    }
    read_layouts(lua, &stale)?;
    for id in drawn {
        // Bitmaps and texts (most of them) straight from the control; the
        // rest, which ask the Lua or other app data, from a copy.
        let (kind, alpha, under, (l, t, w, h, depth)) = {
            let c = controls(lua);
            let Some(ctl) = c.map.get(&id) else { continue };
            // Not laid out (FAF left its relations circular): nothing to draw.
            let Some(rect) = ctl.layout else { continue };
            let (l, t, w, h, depth) = rect;
            let (alpha, under) = (ctl.alpha, ctl.render_pass == UIRP_UNDER_WORLD);
            let item = match &ctl.kind {
                Kind::Bitmap(b) => Some({
                    let base = b.solid.unwrap_or(b.mask.unwrap_or([1.0; 4]));
                    let color = [base[0], base[1], base[2], base[3] * alpha];
                    let texture = if b.solid.is_some() { None } else { b.textures.get(b.frame.min(b.textures.len() - 1)).cloned() };
                    DrawItem::Quad { rect: [l, t, l + w, t + h], texture, uv: b.uv, color, tiled: b.tiled }
                }),
                Kind::Text(tx) => Some({
                    let color = [tx.color[0], tx.color[1], tx.color[2], tx.color[3] * alpha];
                    DrawItem::Text { rect: [l, t, l + w, t + h], text: tx.text.clone(), family: tx.family.clone(), size: tx.size, color, shadow: tx.shadow, vertical: tx.vertical }
                }),
                _ => None,
            };
            if let Some(item) = item {
                items.push((depth, items.len(), under, item));
                continue;
            }
            (ctl.kind.clone(), alpha, under, rect)
        };
        let movie = match &kind {
            Kind::Other("movie") => match super::movie::frame(lua, id) {
                Some(frame) => Some(frame),
                None => continue,
            },
            _ => None,
        };
        let item = match kind {
            Kind::Other("movie") => movie.map(|frame| DrawItem::Movie { rect: [l, t, l + w, t + h], movie: id, frame, color: [1.0, 1.0, 1.0, alpha] }),
            Kind::WorldView(WorldView { mini: true, .. }) => (w > 1.0 && h > 1.0).then_some(DrawItem::Minimap { rect: [l, t, l + w, t + h] }),
            Kind::WorldView(WorldView { camera: Some(camera), .. }) => (w >= 1.0 && h >= 1.0).then_some(DrawItem::World { rect: [l, t, l + w, t + h], camera }),
            // Corners at their texture size, the sides stretched between
            // them (moho's `CMauiBorder`; sides `BorderWidth` / `BorderHeight`
            // thick), the inside left empty; or a solid frame that thick.
            Kind::Border(bd) => {
                let Some(obj) = object(lua, id)? else { continue };
                let get = |k: &str| obj.get::<Option<LuaTable>>(k).ok().flatten().and_then(|v| v.call::<Option<f32>>(()).ok().flatten());
                let (bw, bh) = (get("BorderWidth").unwrap_or(0.0), get("BorderHeight").unwrap_or(0.0));
                let (r, b) = (l + w, t + h);
                let corner = |i: usize| bd.textures[i].as_ref().map(|tex| texture_size(lua, tex)).map_or((bw, bh), |(x, y)| (x as f32, y as f32));
                let (ul, ur, ll, lr) = (corner(2), corner(3), corner(4), corner(5));
                let quad = |rect: [f32; 4], tex: &Option<String>| -> Option<DrawItem> {
                    if rect[2] <= rect[0] || rect[3] <= rect[1] {
                        return None;
                    }
                    match (&bd.solid, tex) {
                        (Some(c), _) => Some(DrawItem::Quad { rect, texture: None, uv: [0.0, 0.0, 1.0, 1.0], color: [c[0], c[1], c[2], c[3] * alpha], tiled: false }),
                        (None, Some(tex)) => Some(DrawItem::Quad { rect, texture: Some(tex.clone()), uv: [0.0, 0.0, 1.0, 1.0], color: [1.0, 1.0, 1.0, alpha], tiled: false }),
                        _ => None,
                    }
                };
                let quads = [
                    quad([l, t, l + ul.0, t + ul.1], &bd.textures[2]),
                    quad([r - ur.0, t, r, t + ur.1], &bd.textures[3]),
                    quad([l, b - ll.1, l + ll.0, b], &bd.textures[4]),
                    quad([r - lr.0, b - lr.1, r, b], &bd.textures[5]),
                    quad([l, t + ul.1, l + bw, b - ll.1], &bd.textures[0]),
                    quad([r - bw, t + ur.1, r, b - lr.1], &bd.textures[0]),
                    quad([l + ul.0, t, r - ur.0, t + bh], &bd.textures[1]),
                    quad([l + ll.0, b - bh, r - lr.0, b], &bd.textures[1]),
                ];
                for q in quads.into_iter().flatten() {
                    items.push((depth, items.len(), under, q));
                }
                None
            }
            Kind::Scrollbar(sb) => {
                let quad = |rect: [f32; 4], texture: &Option<String>| texture.as_ref().map(|tex| DrawItem::Quad { rect, texture: Some(tex.clone()), uv: [0.0, 0.0, 1.0, 1.0], color: [1.0, 1.0, 1.0, alpha], tiled: false });
                let mut quads = vec![quad([l, t, l + w, t + h], &sb.textures[0])];
                if let Some((a, b)) = thumb(lua, &sb)? {
                    // The thumb along the bar: top cap, stretched middle, bottom cap.
                    let (start, len) = if sb.vertical { (t, h) } else { (l, w) };
                    let (a, b) = (start + a * len, start + b * len);
                    let span = |from: f32, to: f32| if sb.vertical { [l, from, l + w, to] } else { [from, t, to, t + h] };
                    let [top, bottom] = sb.caps;
                    quads.push(quad(span(a + top, (b - bottom).max(a + top)), &sb.textures[1]));
                    quads.push(quad(span(a, a + top), &sb.textures[2]));
                    quads.push(quad(span(b - bottom, b), &sb.textures[3]));
                }
                for q in quads.into_iter().flatten() {
                    items.push((depth, items.len(), under, q));
                }
                None
            }
            Kind::ItemList(list) => {
                // Background, then each row that fits from the first shown:
                // selected / mouseover rows in their colours.
                let row_h = row_height(lua, &list);
                let fade = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * alpha];
                if let Some(bg) = list.colors[1].filter(|c| c[3] > 0.0) {
                    items.push((depth, items.len(), under, DrawItem::Quad { rect: [l, t, l + w, t + h], texture: None, uv: [0.0, 0.0, 1.0, 1.0], color: fade(bg), tiled: false }));
                }
                let rows = visible_rows(&list, h, row_h);
                for (i, row) in list.items.iter().enumerate().skip(list.top).take(rows) {
                    let y = t + (i - list.top) as f32 * row_h;
                    let selected = list.show_selection && list.selection == Some(i);
                    let over = list.show_mouseover && list.hover == Some(i);
                    let (fg, bg) = match (over, selected) {
                        (true, _) => (list.colors[4].or(list.colors[0]), list.colors[5]),
                        (false, true) => (list.colors[2].or(list.colors[0]), list.colors[3]),
                        _ => (list.colors[0], None),
                    };
                    if let Some(bg) = bg.filter(|c| c[3] > 0.0) {
                        items.push((depth, items.len(), under, DrawItem::Quad { rect: [l, y, l + w, y + row_h], texture: None, uv: [0.0, 0.0, 1.0, 1.0], color: fade(bg), tiled: false }));
                    }
                    items.push((depth, items.len(), under, DrawItem::Text { rect: [l, y, l + w, y + row_h], text: row.clone(), family: list.family.clone(), size: list.size, color: fade(fg.unwrap_or([1.0; 4])), shadow: false, vertical: false }));
                }
                None
            }
            Kind::Edit(e) => {
                let top = t;
                if e.show_background && e.background[3] > 0.0 {
                    let b = e.background;
                    items.push((depth, items.len(), under, DrawItem::Quad { rect: [l, top, l + w, top + h], texture: None, uv: [0.0, 0.0, 1.0, 1.0], color: [b[0], b[1], b[2], b[3] * alpha], tiled: false }));
                }
                // What fits from the first shown character (scrolled to
                // keep the caret in view), and the caret when focused.
                let chars: Vec<char> = e.text.text.chars().collect();
                let from = edit_scroll(lua, &e, w);
                let mut shown: String = chars[from..].iter().collect();
                let overflow = measure(lua, &e.text.family, e.text.size, &shown).advance > w;
                if overflow {
                    shown.clear();
                }
                for &ch in chars[from..].iter().take_while(|_| overflow) {
                    shown.push(ch);
                    if measure(lua, &e.text.family, e.text.size, &shown).advance > w {
                        shown.pop();
                        break;
                    }
                }
                if focused == Some(id) && e.caret_shown {
                    let caret = (e.caret.max(0) as usize).clamp(from, chars.len());
                    let m = measure(lua, &e.text.family, e.text.size, &chars[from..caret].iter().collect::<String>());
                    let blink = match e.caret_cycle {
                        Some((seconds, min, max)) if seconds > 0.0 => {
                            let phase = (now / seconds as f64).fract() as f32;
                            min + (max - min) * (1.0 - (2.0 * phase - 1.0).abs())
                        }
                        _ => 1.0,
                    };
                    let c = e.caret_color;
                    let x = l + m.advance;
                    items.push((depth, items.len(), under, DrawItem::Quad { rect: [x, top, x + 1.0, top + (m.ascent + m.descent).min(h.max(1.0))], texture: None, uv: [0.0, 0.0, 1.0, 1.0], color: [c[0], c[1], c[2], c[3] * blink * alpha], tiled: false }));
                }
                (!shown.is_empty()).then(|| {
                    let t = e.text;
                    DrawItem::Text { rect: [l, top, l + w, top + h], text: shown, family: t.family, size: t.size, color: [t.color[0], t.color[1], t.color[2], t.color[3] * alpha], shadow: t.shadow, vertical: false }
                })
            }
            _ => None,
        };
        // (A world view marked under is the pass itself: `under_world` keeps it.)
        if let Some(item) = item {
            items.push((depth, items.len(), under, item));
        }
    }
    items.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    Ok(under_world(items.into_iter().map(|(_, _, under, i)| (under, i)).collect()))
}

/// `UIRP_UnderWorld` (FAF's `UIUtil`): a control drawn in the world views'
/// pass rather than over them.
const UIRP_UNDER_WORLD: i32 = 1;

/// Moho's under-world pass: world views and the controls marked
/// `UIRP_UnderWorld` draw together by depth, the rest of the interface over
/// all of them. The renderer draws every world view first, so a marked item
/// (a `Window`'s middle, as the minimap's) gives way to each world view
/// above it: it's cut out of their rectangles. Over the views below it (the
/// main one), it draws as any item.
fn under_world(items: Vec<(bool, DrawItem)>) -> Vec<DrawItem> {
    let mut above: Vec<[f32; 4]> = Vec::new();
    let mut out: Vec<DrawItem> = Vec::with_capacity(items.len());
    for (under, item) in items.into_iter().rev() {
        match &item {
            DrawItem::World { rect, .. } | DrawItem::Minimap { rect } => above.push(*rect),
            _ if under && !above.is_empty() => {
                let mut pieces = vec![item];
                for hole in &above {
                    pieces = pieces.into_iter().flat_map(|p| cut(p, *hole)).collect();
                }
                out.extend(pieces.into_iter().rev());
                continue;
            }
            _ => {}
        }
        out.push(item);
    }
    out.reverse();
    out
}

/// `item` without the part inside `hole`: a quad as up to four quads (its
/// texture coordinates following), anything else whole unless the hole
/// covers it.
fn cut(item: DrawItem, hole: [f32; 4]) -> Vec<DrawItem> {
    let rect = match &item {
        DrawItem::Quad { rect, .. } | DrawItem::Text { rect, .. } | DrawItem::Movie { rect, .. } => *rect,
        _ => return vec![item],
    };
    let [l, t, r, b] = rect;
    let [hl, ht, hr, hb] = [hole[0].max(l), hole[1].max(t), hole[2].min(r), hole[3].min(b)];
    if hl >= hr || ht >= hb {
        return vec![item];
    }
    let DrawItem::Quad { texture, uv, color, tiled, .. } = item else {
        let covered = hl <= l && ht <= t && hr >= r && hb >= b;
        return if covered { Vec::new() } else { vec![item] };
    };
    let at = |x: f32, y: f32| {
        let fx = if r > l { (x - l) / (r - l) } else { 0.0 };
        let fy = if b > t { (y - t) / (b - t) } else { 0.0 };
        [uv[0] + (uv[2] - uv[0]) * fx, uv[1] + (uv[3] - uv[1]) * fy]
    };
    // Above and below the hole across the whole width, then left and right
    // of it beside it.
    [[l, t, r, ht], [l, hb, r, b], [l, ht, hl, hb], [hr, ht, r, hb]]
        .into_iter()
        .filter(|p| p[2] > p[0] && p[3] > p[1])
        .map(|p| {
            let (a, z) = (at(p[0], p[1]), at(p[2], p[3]));
            DrawItem::Quad { rect: p, texture: texture.clone(), uv: [a[0], a[1], z[0], z[1]], color, tiled }
        })
        .collect()
}

/// A scroll a scrollbar or the wheel asks of a scrollable.
#[derive(Clone, Copy, Debug)]
enum Scroll {
    Lines(f64),
    Pages(f64),
    /// To this top (in `GetScrollValues`' units).
    SetTop(f64),
}

/// Scrolls control `id`: an item list natively, any other through FAF's
/// Lua scrollable interface (`ScrollLines` / `ScrollPages` /
/// `ScrollSetTop(axis, n)`).
fn scroll(lua: &Lua, id: ControlId, axis: &str, op: Scroll) -> LuaResult<()> {
    if let Some((_, max, top, bottom)) = list_values(lua, id)? {
        let rows = bottom - top;
        let to = match op {
            Scroll::Lines(d) => top + d,
            Scroll::Pages(d) => top + d * rows,
            Scroll::SetTop(t) => t,
        };
        let to = to.floor().clamp(0.0, (max - rows).max(0.0)) as usize;
        if let Some(Control { kind: Kind::ItemList(l), .. }) = controls(lua).map.get_mut(&id) {
            l.top = to;
        }
        return Ok(());
    }
    let Some(obj) = object(lua, id)? else { return Ok(()) };
    match op {
        Scroll::Lines(d) => obj.call_method("ScrollLines", (axis, d)),
        Scroll::Pages(d) => obj.call_method("ScrollPages", (axis, d)),
        Scroll::SetTop(t) => obj.call_method("ScrollSetTop", (axis, t)),
    }
}

/// An item list's scroll values (0, rows, first shown, past the last
/// shown); `None` for other controls.
fn list_values(lua: &Lua, id: ControlId) -> LuaResult<Option<(f64, f64, f64, f64)>> {
    let list = match controls(lua).map.get(&id).map(|c| &c.kind) {
        Some(Kind::ItemList(l)) => l.clone(),
        _ => return Ok(None),
    };
    let h = object(lua, id)?.and_then(|o| layout(lua, &o)).map_or(0.0, |(_, _, _, h, _)| h);
    let rows = ((h / row_height(lua, &list)).floor().max(0.0) as usize).min(list.items.len());
    let top = list.top.min(list.items.len().saturating_sub(rows));
    Ok(Some((0.0, list.items.len() as f64, top as f64, (top + rows) as f64)))
}

/// An item list's row height: its font's line (ascent, descent and line
/// gap, as FAF's combo sizes its drop-down).
fn row_height(lua: &Lua, list: &ItemList) -> f32 {
    let m = measure(lua, &list.family, list.size, "");
    (m.ascent + m.descent + m.leading).max(1.0)
}

/// How many whole rows fit in `height`.
fn visible_rows(list: &ItemList, height: f32, row_h: f32) -> usize {
    ((height / row_h).floor().max(0.0) as usize).min(list.items.len().saturating_sub(list.top))
}

/// An item list's own handling of the mouse, as moho's: the row under the
/// mouse (`OnMouseoverItem(row)`, -1 when it leaves the rows), a press /
/// double click on a row (`OnClick(row, event)` / `OnDoubleClick`), and the
/// wheel scrolls a line a notch. Rows are 0-based. True if it took the
/// event (a row click, the wheel): it goes no further, so a combo box
/// doesn't also see the click that picked its item.
fn list_mouse(lua: &Lua, id: ControlId, x: f32, y: f32, ev: Option<&LuaTable>) -> LuaResult<bool> {
    let Some(obj) = object(lua, id)? else { return Ok(false) };
    let list = match controls(lua).map.get(&id).map(|c| &c.kind) {
        Some(Kind::ItemList(l)) => l.clone(),
        _ => return Ok(false),
    };
    let row = match layout(lua, &obj) {
        Some((l, t, w, h, _)) if x >= l && x < l + w && y >= t && y < t + h => {
            let r = list.top + ((y - t) / row_height(lua, &list)) as usize;
            (r < list.items.len()).then_some(r)
        }
        _ => None,
    };
    if row != list.hover {
        if let Some(Control { kind: Kind::ItemList(l), .. }) = controls(lua).map.get_mut(&id) {
            l.hover = row;
        }
        hook(lua, id, "OnMouseoverItem", row.map_or(-1, |r| r as i64))?;
    }
    let Some(ev) = ev else { return Ok(false) };
    match (ev.raw_get::<String>("Type")?.as_str(), row) {
        ("ButtonPress", Some(row)) => {
            hook(lua, id, "OnClick", (row as i64, ev.clone()))?;
        }
        ("ButtonDClick", Some(row)) => {
            hook(lua, id, "OnDoubleClick", (row as i64, ev.clone()))?;
        }
        ("WheelRotation", _) => {
            let notches: f64 = ev.raw_get("WheelRotation")?;
            scroll(lua, id, "Vert", Scroll::Lines(if notches > 0.0 { -1.0 } else { 1.0 }))?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// The part of its scrollable's range a scrollbar shows, as fractions of
/// the bar (`GetScrollValues(axis)`: range min, max, visible min, max).
fn thumb(lua: &Lua, sb: &Scrollbar) -> LuaResult<Option<(f32, f32)>> {
    let Some((min, max, top, bottom)) = scroll_values(lua, sb)? else { return Ok(None) };
    let frac = |v: f64| ((v - min) / (max - min)).clamp(0.0, 1.0) as f32;
    Ok(Some((frac(top), frac(bottom))))
}

/// The scrollable's `GetScrollValues(axis)` (range min, max, visible min,
/// max), when it has a range.
fn scroll_values(lua: &Lua, sb: &Scrollbar) -> LuaResult<Option<(f64, f64, f64, f64)>> {
    if let Some(v) = sb.scrollable.map(|id| list_values(lua, id)).transpose()?.flatten() {
        return Ok((v.1 > v.0).then_some(v));
    }
    let Some(obj) = sb.scrollable.map(|id| object(lua, id)).transpose()?.flatten() else { return Ok(None) };
    let Ok((Some(min), Some(max), Some(top), Some(bottom))) = obj.call_method::<(Option<f64>, Option<f64>, Option<f64>, Option<f64>)>("GetScrollValues", axis_name(sb)) else { return Ok(None) };
    Ok((max > min).then_some((min, max, top, bottom)))
}

fn axis_name(sb: &Scrollbar) -> &'static str {
    if sb.vertical {
        "Vert"
    } else {
        "Horz"
    }
}

/// A scrollbar's own handling of the mouse, as moho's: a press on the
/// thumb grabs it (`drag`: the bar and where on the thumb, in pixels along
/// the axis), elsewhere on the bar pages toward the press; moving a grabbed
/// thumb sets the scrollable's top (`ScrollSetTop`, in `GetScrollValues`'
/// units). True if the scrollbar took the event.
fn scrollbar_mouse(lua: &Lua, ev: MouseEvent, x: f32, y: f32, target: Option<ControlId>) -> LuaResult<bool> {
    let dragging = controls(lua).scroll_drag;
    let id = match (dragging, ev) {
        (Some((id, _)), MouseEvent::Move | MouseEvent::Release(_)) => id,
        (_, MouseEvent::Press(1)) => match target {
            Some(t) => t,
            None => return Ok(false),
        },
        _ => return Ok(false),
    };
    let sb = match controls(lua).map.get(&id) {
        Some(Control { kind: Kind::Scrollbar(sb), .. }) => sb.clone(),
        _ => {
            controls(lua).scroll_drag = None;
            return Ok(false);
        }
    };
    if let MouseEvent::Release(_) = ev {
        controls(lua).scroll_drag = None;
        return Ok(true);
    }
    let Some((l, t, w, h, _)) = object(lua, id)?.and_then(|o| layout(lua, &o)) else { return Ok(false) };
    let Some(scrollable) = sb.scrollable else { return Ok(true) };
    let Some((min, max, top, bottom)) = scroll_values(lua, &sb)? else { return Ok(true) };
    let (start, len, at) = if sb.vertical { (t as f64, h as f64, y as f64) } else { (l as f64, w as f64, x as f64) };
    let to_px = |v: f64| start + (v - min) / (max - min) * len;
    let (a, b) = (to_px(top), to_px(bottom));
    match (dragging, ev) {
        (Some((_, grab)), MouseEvent::Move) => {
            let value = min + (at - grab - start) / len.max(1.0) * (max - min);
            scroll(lua, scrollable, axis_name(&sb), Scroll::SetTop(value))?;
        }
        (_, MouseEvent::Press(_)) if at >= a && at <= b => controls(lua).scroll_drag = Some((id, at - a)),
        (_, MouseEvent::Press(_)) => scroll(lua, scrollable, axis_name(&sb), Scroll::Pages(if at < a { -1.0 } else { 1.0 }))?,
        _ => {}
    }
    Ok(true)
}

/// Mouse input for the interface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseEvent {
    Move,
    /// Button 1 left, 2 right, 3 middle.
    Press(u8),
    Release(u8),
    Wheel(f32),
}

/// Buttons and keys held, for `event.Modifiers`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifiers {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// A press on `target`: an enabled edit takes the focus with its caret
/// at the click; one that had it and lost the click hears
/// `OnLoseKeyboardFocus` (FAF's lobby chat takes it straight back).
fn press_focus(lua: &Lua, target: ControlId, x: f32) -> LuaResult<()> {
    let edit = match controls(lua).map.get(&target) {
        Some(Control { kind: Kind::Edit(e), .. }) if e.enabled => Some(e.clone()),
        _ => None,
    };
    let previous = focused_edit(lua).filter(|&p| p != target);
    if let Some(p) = previous {
        controls(lua).focus.retain(|f| f.0 != p);
        hook(lua, p, "OnLoseKeyboardFocus", ())?;
    }
    if let Some(e) = edit {
        acquire_focus(lua, target, true)?;
        if let Some((l, _, w, _, _)) = object(lua, target)?.and_then(|o| layout(lua, &o)) {
            let caret = caret_at(lua, &e, l, w, x);
            if let Some(Control { kind: Kind::Edit(e), .. }) = controls(lua).map.get_mut(&target) {
                e.caret = caret;
            }
        }
    }
    Ok(())
}

/// What's under the mouse.
#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Control(ControlId),
    /// A world view: input there belongs to the world.
    World(ControlId),
    /// A world view with its input locked (cinematics): nobody gets it.
    Locked,
}

/// The topmost visible, hit-testable control under (x, y).
fn hit(lua: &Lua, x: f32, y: f32) -> LuaResult<Option<Hit>> {
    let roots: Vec<ControlId> = {
        let c = controls(lua);
        c.frames.get(&c.mouse_head).copied().into_iter().collect()
    };
    // The visible, hit-testable controls (with their tree order), under
    // one borrow; then all their layouts in one call.
    forget_dirty_layouts(lua)?;
    let mut candidates: Vec<(ControlId, usize, Hit)> = Vec::new();
    let mut stale: Vec<ControlId> = Vec::new();
    {
        let c = controls(lua);
        let mut order = 0usize;
        let mut stack: Vec<ControlId> = roots.into_iter().rev().collect();
        while let Some(id) = stack.pop() {
            let Some(ctl) = c.map.get(&id) else { continue };
            if ctl.hidden {
                continue;
            }
            stack.extend(ctl.children.iter().rev());
            order += 1;
            if !ctl.hit_test {
                continue;
            }
            let what = match &ctl.kind {
                Kind::WorldView(v) if v.input_locked => Hit::Locked,
                Kind::WorldView(_) => Hit::World(id),
                _ => Hit::Control(id),
            };
            candidates.push((id, order, what));
            if ctl.layout.is_none() {
                stale.push(id);
            }
        }
    }
    read_layouts(lua, &stale)?;
    let c = controls(lua);
    let mut best: Option<(f32, usize, Hit)> = None;
    for (id, order, what) in candidates {
        let Some((l, t, w, h, depth)) = c.map.get(&id).and_then(|ctl| ctl.layout) else { continue };
        if x >= l && y >= t && x < l + w && y < t + h
            && best.is_none_or(|b| (depth, order) >= (b.0, b.1)) {
            best = Some((depth, order, what));
        }
    }
    Ok(best.map(|b| b.2))
}

fn event_table(lua: &Lua, kind: &str, x: f32, y: f32, mods: Modifiers, wheel: f32, key: i64) -> LuaResult<LuaTable> {
    let e = lua.create_table()?;
    e.raw_set("Type", kind)?;
    e.raw_set("MouseX", x)?;
    e.raw_set("MouseY", y)?;
    e.raw_set("WheelRotation", wheel)?;
    e.raw_set("KeyCode", key)?;
    let m = lua.create_table()?;
    for (k, v) in [("Left", mods.left), ("Right", mods.right), ("Middle", mods.middle), ("Shift", mods.shift), ("Ctrl", mods.ctrl), ("Alt", mods.alt)] {
        if v {
            m.raw_set(k, true)?;
        }
    }
    e.raw_set("Modifiers", m)?;
    Ok(e)
}

/// `HandleEvent` on `id`, then its parents until one returns true.
fn bubble(lua: &Lua, mut id: Option<ControlId>, event: &LuaTable) -> LuaResult<bool> {
    while let Some(i) = id {
        let Some(obj) = object(lua, i)? else { return Ok(false) };
        if let Some(f) = obj.get::<Option<LuaFunction>>("HandleEvent")? {
            match f.call::<Option<bool>>((obj.clone(), event.clone())) {
                Ok(Some(true)) => return Ok(true),
                Ok(_) => {}
                Err(e) => tracing::warn!(target: "lua", "UI HandleEvent failed: {e}"),
            }
        }
        id = controls(lua).map.get(&i).and_then(|c| c.parent);
    }
    Ok(false)
}

/// The (unlocked) world view the mouse was last over, if it still is one.
pub fn hovered_world_view(lua: &Lua) -> LuaResult<Option<LuaTable>> {
    let Some(id) = lua.named_registry_value::<Option<u32>>("user.hover")? else { return Ok(None) };
    match controls(lua).map.get(&id).map(|c| &c.kind) {
        Some(Kind::WorldView(v)) if !v.input_locked => {}
        _ => return Ok(None),
    }
    object(lua, id)
}

/// An event for `id`: a world view's own `HandleEvent` only (its input
/// goes on to the world, not up its parents), any other control's bubbling
/// up as usual.
fn deliver(lua: &Lua, id: ControlId, e: &LuaTable) -> LuaResult<()> {
    if matches!(controls(lua).map.get(&id).map(|c| &c.kind), Some(Kind::WorldView(_))) {
        hook(lua, id, "HandleEvent", e.clone())?;
        return Ok(());
    }
    bubble(lua, Some(id), e).map(|_| ())
}

/// Is (x, y) over a control other than a world view (or a locked one)?
pub fn over_interface(lua: &Lua, x: f32, y: f32) -> LuaResult<bool> {
    Ok(hit(lua, x, y)?.is_some_and(|h| !matches!(h, Hit::World(_))))
}

/// Whether a click at (x, y) is the minimap's: the topmost control under it
/// is the minimap or inside it (FAF's painting canvas over it; an event a
/// child leaves bubbles up to its parent). A window over it, like the cheat
/// menu at the top left, takes its clicks.
pub fn over_minimap(lua: &Lua, x: f32, y: f32) -> LuaResult<bool> {
    let Some(Hit::Control(id) | Hit::World(id)) = hit(lua, x, y)? else { return Ok(false) };
    let c = controls(lua);
    let mut at = Some(id);
    while let Some(ctl) = at.and_then(|i| c.map.get(&i)) {
        if matches!(ctl.kind, Kind::WorldView(WorldView { mini: true, .. })) {
            return Ok(true);
        }
        at = ctl.parent;
    }
    Ok(false)
}

/// Feeds a mouse event to the interface. Returns whether the interface took
/// it (false: it belongs to the world, e.g. over an unlocked world view or
/// nothing).
pub fn mouse(lua: &Lua, ev: MouseEvent, x: f32, y: f32, mods: Modifiers) -> LuaResult<bool> {
    lua.globals().raw_set("__engine_mouse_x", x)?;
    lua.globals().raw_set("__engine_mouse_y", y)?;
    // A posted dragger (a pressed button) follows the mouse until release.
    if let Some(dragger) = lua.named_registry_value::<Option<LuaTable>>("user.dragger")? {
        match ev {
            MouseEvent::Move => {
                if let Some(f) = dragger.get::<Option<LuaFunction>>("OnMove")?
                    && let Err(e) = f.call::<()>((dragger.clone(), x, y)) {
                    tracing::warn!(target: "lua", "UI dragger OnMove failed: {e}");
                }
                return Ok(true);
            }
            MouseEvent::Release(_) => {
                lua.set_named_registry_value("user.dragger", LuaNil)?;
                if let Some(f) = dragger.get::<Option<LuaFunction>>("OnRelease")?
                    && let Err(e) = f.call::<()>((dragger, x, y)) {
                    tracing::warn!(target: "lua", "UI dragger OnRelease failed: {e}");
                }
                return Ok(true);
            }
            _ => {}
        }
    }
    // Every press, wherever it lands, goes to FAF's `uimain.OnMouseButtonPress`
    // first (combo drop-downs and popups close on a click outside them).
    if let MouseEvent::Press(_) = ev {
        let import: LuaFunction = lua.globals().get("import")?;
        if let Ok(main) = import.call::<LuaTable>("/lua/ui/uimain.lua")
            && let Some(f) = main.get::<Option<LuaFunction>>("OnMouseButtonPress")? {
            let e = lua.create_table()?;
            e.raw_set("Type", "ButtonPress")?;
            e.raw_set("x", x)?;
            e.raw_set("y", y)?;
            if let Err(err) = f.call::<()>(e) {
                tracing::warn!(target: "lua", "uimain.OnMouseButtonPress failed: {err}");
            }
        }
    }
    let target = hit(lua, x, y)?;
    let over = match target {
        Some(Hit::Control(id)) => Some(id),
        _ => None,
    };
    if scrollbar_mouse(lua, ev, x, y, over.filter(|id| matches!(controls(lua).map.get(id).map(|c| &c.kind), Some(Kind::Scrollbar(_)))))? {
        return Ok(true);
    }
    // Hover: MouseExit / MouseEnter as the control under the mouse changes
    // (world views included: FAF's sets its cursor on them).
    let hovered = match target {
        Some(Hit::Control(id) | Hit::World(id)) => Some(id),
        _ => None,
    };
    let previous: Option<ControlId> = lua.named_registry_value::<Option<u32>>("user.hover")?;
    if previous != hovered {
        if let Some(p) = previous.filter(|p| matches!(controls(lua).map.get(p).map(|c| &c.kind), Some(Kind::ItemList(_)))) {
            list_mouse(lua, p, f32::NAN, f32::NAN, None)?;
        }
        if let Some(p) = previous.filter(|p| controls(lua).map.contains_key(p)) {
            let e = event_table(lua, "MouseExit", x, y, mods, 0.0, 0)?;
            deliver(lua, p, &e)?;
        }
        lua.set_named_registry_value("user.hover", hovered)?;
        if let Some(h) = hovered {
            let e = event_table(lua, "MouseEnter", x, y, mods, 0.0, 0)?;
            deliver(lua, h, &e)?;
        }
    }
    let world = matches!(target, Some(Hit::World(_)));
    let Some(target) = hovered else { return Ok(target == Some(Hit::Locked)) };
    let (kind, wheel, key) = match ev {
        MouseEvent::Move => ("MouseMotion", 0.0, 0),
        // A quick second press in the same place is a double click instead
        // (as Windows sends it; FAF's handlers take either).
        MouseEvent::Press(b) if world => {
            press_focus(lua, target, x)?;
            ("ButtonPress", 0.0, b as i64)
        }
        MouseEvent::Press(b) => {
            press_focus(lua, target, x)?;
            let now = lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0);
            let mut c = controls(lua);
            let double = c.last_press.is_some_and(|(pb, pid, t, px, py)| pb == b && pid == target && now - t <= DOUBLE_CLICK_TIME && (x - px).abs() <= DOUBLE_CLICK_DISTANCE && (y - py).abs() <= DOUBLE_CLICK_DISTANCE);
            c.last_press = if double { None } else { Some((b, target, now, x, y)) };
            (if double { "ButtonDClick" } else { "ButtonPress" }, 0.0, b as i64)
        }
        MouseEvent::Release(b) => ("ButtonRelease", 0.0, b as i64),
        MouseEvent::Wheel(d) => ("WheelRotation", d, 0),
    };
    let e = event_table(lua, kind, x, y, mods, wheel, key)?;
    if wheel != 0.0 {
        e.raw_set("WheelDelta", 120)?;
    }
    // Over a world view the view sees it (FAF's tracks the cursor and
    // rotates build templates there) and the world still gets it.
    if world {
        deliver(lua, target, &e)?;
        return Ok(false);
    }
    if matches!(controls(lua).map.get(&target).map(|c| &c.kind), Some(Kind::ItemList(_))) && list_mouse(lua, target, x, y, Some(&e))? {
        return Ok(true);
    }
    bubble(lua, Some(target), &e)?;
    // Over the interface (not a world view): the world doesn't get it.
    Ok(true)
}

/// moho's `KeyCode` for a key named FAF's way: wxWidgets 2.4's codes
/// (ASCII for characters, `WXK_*` from 300 for the rest; FAF's
/// `UIUtil.VK_PAUSE` is 310, and F1, which closes the key bindings dialog,
/// 342).
fn wx_key_code(name: &str) -> Option<i64> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(c.to_ascii_uppercase() as i64);
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<i64>().ok()) {
        return (1..=24).contains(&n).then_some(341 + n);
    }
    Some(match name {
        "Backspace" => 8,
        "Tab" => 9,
        "Enter" => 13,
        "Esc" => 27,
        "Space" => 32,
        "Delete" => 127,
        "Period" => '.' as i64,
        "Comma" => ',' as i64,
        "Slash" => '/' as i64,
        "LeftBracket" => '[' as i64,
        "RightBracket" => ']' as i64,
        "Minus" => '-' as i64,
        "Equals" => '=' as i64,
        "Pause" => 310,
        "PageUp" => 312,
        "PageDown" => 313,
        "End" => 314,
        "Home" => 315,
        "LeftArrow" => 316,
        "UpArrow" => 317,
        "RightArrow" => 318,
        "DownArrow" => 319,
        "Insert" => 324,
        "NumStar" => 336,
        "NumPlus" => 337,
        "NumMinus" => 339,
        "NumSlash" => 341,
        _ => return None,
    })
}

/// A key press (named FAF's way) for the control holding the keyboard
/// focus: a `KeyDown` event with moho's `KeyCode`, the Windows virtual key
/// as `RawKeyCode` (FAF's `/lua/keymap/keynames.lua`, read backwards) and
/// the modifiers, bubbling up from it. `None` when nothing has the focus,
/// else whether the focus blocks the key map.
pub fn key_down(lua: &Lua, name: &str, mods: Modifiers, repeat: bool) -> LuaResult<Option<bool>> {
    if let Some(id) = focused_edit(lua) {
        edit_key(lua, id, name, mods)?;
        return Ok(Some(true));
    }
    // Held keys repeat only into edits.
    if repeat {
        return Ok(None);
    }
    // An input capture (FAF's modal dialogs: `MakeInputModal`) takes every
    // key: its Esc / Enter answer the dialog, and nothing else hears them.
    let capture = lua.globals().get::<LuaFunction>("GetInputCapture")?.call::<Option<LuaTable>>(())?;
    if let Some(id) = capture.and_then(|c| id_of(&c).ok()).filter(|id| controls(lua).map.contains_key(id)) {
        let e = key_event(lua, name, mods)?;
        bubble(lua, Some(id), &e)?;
        return Ok(Some(true));
    }
    let focus = {
        let mut c = controls(lua);
        let live: Vec<(ControlId, bool)> = c.focus.iter().copied().filter(|f| c.map.contains_key(&f.0)).collect();
        c.focus = live;
        c.focus.last().copied()
    };
    let Some((id, blocks)) = focus else { return Ok(None) };
    let e = key_event(lua, name, mods)?;
    bubble(lua, Some(id), &e)?;
    Ok(Some(blocks))
}

/// A `KeyDown` event for key `name`.
fn key_event(lua: &Lua, name: &str, mods: Modifiers) -> LuaResult<LuaTable> {
    let raw = virtual_key(lua, name)?;
    let e = event_table(lua, "KeyDown", 0.0, 0.0, mods, 0.0, wx_key_code(name).or(raw).unwrap_or(0))?;
    e.raw_set("RawKeyCode", raw.unwrap_or(0))?;
    Ok(e)
}

/// Takes the keyboard focus for `id` (the newest holder gets keys);
/// `blocks` keeps them from the key map. The previous holder hears
/// `OnKeyboardFocusChange`.
fn acquire_focus(lua: &Lua, id: ControlId, blocks: bool) -> LuaResult<()> {
    let previous = {
        let mut c = controls(lua);
        let previous = c.focus.last().map(|f| f.0).filter(|&p| p != id && c.map.contains_key(&p));
        c.focus.retain(|f| f.0 != id);
        c.focus.push((id, blocks));
        previous
    };
    if let Some(p) = previous {
        hook(lua, p, "OnKeyboardFocusChange", ())?;
    }
    Ok(())
}

/// Calls Lua method `name` on control `id`, if it has one: its first
/// result (nil when missing or it failed; failures are logged).
fn hook(lua: &Lua, id: ControlId, name: &str, args: impl IntoLuaMulti) -> LuaResult<LuaValue> {
    let Some(obj) = object(lua, id)? else { return Ok(LuaNil) };
    let Some(f) = obj.get::<Option<LuaFunction>>(name)? else { return Ok(LuaNil) };
    let mut all = args.into_lua_multi(lua)?;
    all.push_front(LuaValue::Table(obj));
    match f.call::<LuaValue>(all) {
        Ok(v) => Ok(v),
        Err(e) => {
            tracing::warn!(target: "lua", "UI {name} failed: {e}");
            Ok(LuaNil)
        }
    }
}

/// Does an edit hold the keyboard focus?
pub fn typing(lua: &Lua) -> bool {
    focused_edit(lua).is_some()
}

/// The edit holding the keyboard focus, if the focus is an edit.
fn focused_edit(lua: &Lua) -> Option<ControlId> {
    let c = controls(lua);
    let (id, _) = c.focus.iter().rev().find(|f| c.map.contains_key(&f.0))?;
    matches!(c.map.get(id)?.kind, Kind::Edit(_)).then_some(*id)
}

/// The Windows virtual key for a key named FAF's way (its
/// `/lua/keymap/keynames.lua`, read backwards once).
fn virtual_key(lua: &Lua, name: &str) -> LuaResult<Option<i64>> {
    let vk: LuaTable = match lua.named_registry_value::<Option<LuaTable>>("ui.vk")? {
        Some(t) => t,
        None => {
            let t: LuaTable = lua
                .load("local t = {} for code, name in import('/lua/keymap/keynames.lua').keyNames do t[name] = tonumber(code, 16) end return t")
                .set_name("=[engine key codes]")
                .eval()?;
            lua.set_named_registry_value("ui.vk", t.clone())?;
            t
        }
    };
    vk.raw_get(name)
}

/// Changes edit `id`'s text and caret with `f` (given the text as chars
/// and the caret); then `OnTextChanged(new, old)` if the text changed.
fn edit_text(lua: &Lua, id: ControlId, f: impl FnOnce(&mut Vec<char>, &mut usize, usize)) -> LuaResult<()> {
    let change = {
        let mut c = controls(lua);
        let Some(Control { kind: Kind::Edit(e), .. }) = c.map.get_mut(&id) else { return Ok(()) };
        if !e.enabled {
            return Ok(());
        }
        let old = e.text.text.clone();
        let mut chars: Vec<char> = old.chars().collect();
        let mut caret = (e.caret.max(0) as usize).min(chars.len());
        f(&mut chars, &mut caret, e.max_chars.max(0) as usize);
        e.caret = caret.min(chars.len()) as i64;
        e.text.text = chars.into_iter().collect();
        (e.text.text != old).then(|| (e.text.text.clone(), old))
    };
    if let Some((new, old)) = change {
        hook(lua, id, "OnTextChanged", (new, old))?;
    }
    Ok(())
}

/// A key pressed while edit `id` has the focus: editing keys edit,
/// Enter / Esc go to `OnEnterPressed` (then the text is emptied) /
/// `OnEscPressed` (Esc clears the text unless that returns true), keys that type characters wait for
/// `edit_char`, and the rest go to `OnNonTextKeyPressed(vk, event)`. Keys
/// that don't type then go up from the edit as a `KeyDown` (FAF's key
/// bindings dialog closes on Esc / Enter / F1 that way while its filter
/// box has the focus).
fn edit_key(lua: &Lua, id: ControlId, name: &str, mods: Modifiers) -> LuaResult<()> {
    // Paste (typed in, as if each character were pressed) and copy (the
    // whole text: edits have no selection). Ctrl here is also Cmd.
    if mods.ctrl && !mods.alt && name.eq_ignore_ascii_case("v") {
        let text = crate::ui::clipboard::get().unwrap_or_default();
        tracing::info!(chars = text.chars().count(), "clipboard: paste into an edit");
        for ch in text.chars().filter(|c| !c.is_control()) {
            edit_char(lua, ch)?;
        }
        return Ok(());
    }
    if mods.ctrl && !mods.alt && name.eq_ignore_ascii_case("c") {
        if let Some(Control { kind: Kind::Edit(e), .. }) = controls(lua).map.get(&id) {
            crate::ui::clipboard::set(&e.text.text);
        }
        return Ok(());
    }
    let types = wx_key_code(name).is_some_and(|k| (32..127).contains(&k)) && !mods.ctrl && !mods.alt;
    edit_key_native(lua, id, name, mods, types)?;
    if !types && controls(lua).map.contains_key(&id) {
        let e = key_event(lua, name, mods)?;
        bubble(lua, Some(id), &e)?;
    }
    Ok(())
}

fn edit_key_native(lua: &Lua, id: ControlId, name: &str, mods: Modifiers, types: bool) -> LuaResult<()> {
    let text = || -> String {
        match controls(lua).map.get(&id).map(|c| &c.kind) {
            Some(Kind::Edit(e)) => e.text.text.clone(),
            _ => String::new(),
        }
    };
    match name {
        "Backspace" => edit_text(lua, id, |t, c, _| {
            if *c > 0 {
                t.remove(*c - 1);
                *c -= 1;
            }
        }),
        "Delete" => edit_text(lua, id, |t, c, _| {
            if *c < t.len() {
                t.remove(*c);
            }
        }),
        "LeftArrow" => edit_text(lua, id, |_, c, _| *c = c.saturating_sub(1)),
        "RightArrow" => edit_text(lua, id, |_, c, _| *c += 1),
        "Home" => edit_text(lua, id, |_, c, _| *c = 0),
        "End" => edit_text(lua, id, |t, c, _| *c = t.len()),
        // Then emptied (FAF's lobby chat and console send and never clear
        // the box themselves), without `OnTextChanged`.
        "Enter" => {
            hook(lua, id, "OnEnterPressed", text())?;
            if let Some(Control { kind: Kind::Edit(e), .. }) = controls(lua).map.get_mut(&id) {
                e.text.text.clear();
                e.caret = 0;
            }
            Ok(())
        }
        "Esc" => {
            let kept = hook(lua, id, "OnEscPressed", text())?;
            if matches!(kept, LuaNil | LuaValue::Boolean(false)) {
                edit_text(lua, id, |t, c, _| {
                    t.clear();
                    *c = 0;
                })?;
            }
            Ok(())
        }
        _ if types => Ok(()),
        _ => {
            let e = key_event(lua, name, mods)?;
            let vk: i64 = e.raw_get("RawKeyCode")?;
            hook(lua, id, "OnNonTextKeyPressed", (vk, e)).map(|_| ())
        }
    }
}

/// A typed character for the focused edit: `OnCharPressed(code)` may take
/// it, else it goes in at the caret (up to the edit's `MaxChars`). True if
/// an edit has the focus.
pub fn edit_char(lua: &Lua, ch: char) -> LuaResult<bool> {
    let Some(id) = focused_edit(lua) else { return Ok(false) };
    if ch.is_control() {
        return Ok(true);
    }
    if !matches!(hook(lua, id, "OnCharPressed", ch as u32)?, LuaNil | LuaValue::Boolean(false)) {
        return Ok(true);
    }
    edit_text(lua, id, |t, c, max| {
        if t.len() < max {
            t.insert(*c, ch);
            *c += 1;
        }
    })?;
    Ok(true)
}

/// Edit `id`'s character nearest to x (control space), for a click.
fn caret_at(lua: &Lua, e: &Edit, left: f32, width: f32, x: f32) -> i64 {
    let chars: Vec<char> = e.text.text.chars().collect();
    let shown = edit_scroll(lua, e, width);
    let mut best = (f32::MAX, shown);
    for i in shown..=chars.len() {
        let prefix: String = chars[shown..i].iter().collect();
        let d = (left + measure(lua, &e.text.family, e.text.size, &prefix).advance - x).abs();
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1 as i64
}

/// The first character shown of an edit `width` wide: text longer than
/// the edit scrolls so the caret stays in view.
fn edit_scroll(lua: &Lua, e: &Edit, width: f32) -> usize {
    let chars: Vec<char> = e.text.text.chars().collect();
    let caret = (e.caret.max(0) as usize).min(chars.len());
    let fits = |from: usize| measure(lua, &e.text.family, e.text.size, &chars[from..caret].iter().collect::<String>()).advance <= width;
    (0..caret).find(|&from| fits(from)).unwrap_or(caret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_argb_rgb_and_names() {
        assert_eq!(parse_color("ff00ff00"), Some([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(parse_color("80ffffff").map(|c| (c[3] * 255.0).round()), Some(128.0));
        assert_eq!(parse_color("0000ff"), Some([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(parse_color("White"), Some([1.0; 4]));
        assert_eq!(parse_color("nonsense"), None);
    }

    /// Moho's under-world pass: a marked item draws over the world views
    /// below it and gives way to those above it (cut out of their
    /// rectangles, texture coordinates following); unmarked items draw over
    /// every view.
    #[test]
    fn under_world_items_give_way_to_views_above_them() {
        let quad = |rect: [f32; 4]| DrawItem::Quad { rect, texture: Some("m.dds".into()), uv: [0.0, 0.0, 1.0, 1.0], color: [1.0; 4], tiled: false };
        let view = |rect: [f32; 4]| DrawItem::World { rect, camera: "PiP1".into() };
        let rects = |items: &[DrawItem]| items.iter().filter_map(|i| match i { DrawItem::Quad { rect, uv, .. } => Some((*rect, *uv)), _ => None }).collect::<Vec<_>>();
        // The main view below the window's middle, a popup's view above it
        // inside it: the middle becomes a frame around the popup's picture.
        let out = under_world(vec![(false, view([0.0, 0.0, 1000.0, 1000.0])), (true, quad([0.0, 0.0, 100.0, 100.0])), (false, view([10.0, 20.0, 90.0, 80.0]))]);
        let pieces = rects(&out);
        assert_eq!(pieces.len(), 4, "{pieces:?}");
        assert!(pieces.contains(&([0.0, 0.0, 100.0, 20.0], [0.0, 0.0, 1.0, 0.2])), "above the hole: {pieces:?}");
        assert!(pieces.contains(&([90.0, 20.0, 100.0, 80.0], [0.9, 0.2, 1.0, 0.8])), "right of the hole: {pieces:?}");
        let area: f32 = pieces.iter().map(|(r, _)| (r[2] - r[0]) * (r[3] - r[1])).sum();
        assert_eq!(area, 100.0 * 100.0 - 80.0 * 60.0);
        // Unmarked, or under no view: whole.
        assert_eq!(rects(&under_world(vec![(false, quad([0.0, 0.0, 100.0, 100.0])), (false, view([10.0, 20.0, 90.0, 80.0]))])).len(), 1);
        assert_eq!(rects(&under_world(vec![(false, view([10.0, 20.0, 90.0, 80.0])), (true, quad([0.0, 0.0, 100.0, 100.0]))])).len(), 1);
        // Covered completely: gone.
        assert!(rects(&under_world(vec![(true, quad([20.0, 30.0, 40.0, 50.0])), (false, view([10.0, 20.0, 90.0, 80.0]))])).is_empty());
    }
}
