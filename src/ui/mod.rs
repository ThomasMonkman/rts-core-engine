//! The user layer: moho's second Lua state, which runs FAF's UI scripts
//! (`/lua/userInit.lua`, `/lua/maui/*`, `/lua/ui/**`). It lives on the
//! render thread beside the renderer, reads the sim only through snapshots,
//! and gives orders only as player commands (`sim::input`), so it can never
//! touch sim state.
//!
//! Stage 1: the VM boots FAF's user init, and the core `maui` controls
//! (frame, group, bitmap, text) are native objects whose layout comes from
//! the LazyVars FAF's layout code sets ([`controls`]); each frame the visible
//! controls become a draw list the renderer batches into one draw call,
//! as SupCom does (one texture atlas for the whole interface).

pub mod clipboard;
pub mod controls;
pub mod cursor;
pub mod fonts;
pub mod movie;
pub mod net_lobby;
pub mod prefs;
pub mod replay;
pub mod shapes;
pub mod signatures;
pub mod sound;
pub mod team_color;
pub mod user_api;
pub mod world_draw;
pub mod world_mesh;
pub mod world_pick;

use std::sync::{Arc, RwLock};

use mlua::prelude::*;

use crate::engine::api;
use crate::lua_api::lua_fn;
use crate::engine::lua::script_cache::ScriptCache;
use crate::engine::vfs::Vfs;

/// User-layer moho classes and their bases (FAF's `/engine/User/*.lua`).
pub const USER_CLASSES: &[(&str, &[&str])] = &[
    ("control_methods", &[]),
    ("group_methods", &["control_methods"]),
    ("bitmap_methods", &["control_methods"]),
    ("text_methods", &["control_methods"]),
    ("frame_methods", &["control_methods"]),
    ("border_methods", &["control_methods"]),
    ("edit_methods", &["control_methods"]),
    ("item_list_methods", &["control_methods"]),
    ("histogram_methods", &["control_methods"]),
    ("scrollbar_methods", &["control_methods"]),
    ("dragger_methods", &[]),
    ("movie_methods", &["control_methods"]),
    ("mesh_methods", &["control_methods"]),
    ("cursor_methods", &[]),
    ("discovery_service_methods", &[]),
    ("lobby_methods", &[]),
    ("UIWorldView", &["control_methods"]),
    ("world_mesh_methods", &[]),
    ("WldUIProvider_methods", &[]),
    ("userunit_methods", &[]),
    ("ui_map_preview_methods", &["control_methods"]),
    ("PathDebugger_methods", &["control_methods"]),
    ("userDecal_methods", &[]),
    ("CameraImpl_methods", &[]),
    // core (shared with the sim state)
    ("sound_methods", &[]),
    ("EntityCategory", &[]),
    ("CPrefetchSet", &[]),
];

/// A unit as the interface sees it (from a snapshot).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitInfo {
    pub id: u32,
    /// Shared with the renderer's table of names: a beat hands over every
    /// unit on screen (48k in a big battle), which must not copy a string
    /// each.
    pub blueprint: std::sync::Arc<str>,
    pub army: u32,
    /// 0..1.
    pub health: f32,
    /// Its maximum health now (the blueprint's unless changed).
    pub max_health: f32,
    /// Its build rate now (`GetBuildRate`).
    pub build_rate: f32,
    /// 0..1 (1 = finished).
    pub build: f32,
    pub position: [f32; 3],
    /// Its job's progress, 0..1 (an enhancement, upgrade, build, ...):
    /// `GetWorkProgress`. Every unit's, as moho replicates it per unit
    /// (FAF's ACU upgrade ETA watches an unselected commander).
    pub work: f32,
    /// Its shield's health, 0..1 (0: none): `GetShieldRatio`.
    pub shield: f32,
    /// Selected units' details (`None`: not known).
    pub detail: Option<crate::engine::snapshot::UnitDetail>,
}

impl UnitInfo {
    /// A unit of a snapshot as the interface sees it (no details).
    pub fn from_view(e: &crate::engine::snapshot::EntityView, blueprint: std::sync::Arc<str>) -> Self {
        UnitInfo {
            id: e.id,
            blueprint,
            army: e.army,
            health: e.health,
            max_health: e.max_health,
            build_rate: e.build_rate,
            build: e.build,
            position: e.position,
            work: e.work,
            // (The view's -1 for none: the getter's 0.)
            shield: e.shield.max(0.0),
            detail: None,
        }
    }
}

/// An order given from the interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiOrder {
    /// `UNITCOMMAND_<name>` (or a bare name such as `Stop`).
    pub command: String,
    /// Its units, else the selection.
    pub units: Option<Vec<u32>>,
    pub blueprint: Option<String>,
    pub count: u32,
    /// Replace the units' orders rather than queue.
    pub clear: bool,
    /// A setting rather than an order.
    pub toggle: Option<crate::engine::sim::input::Toggle>,
    /// Script orders: the order table's string fields, by key.
    pub script: Vec<(String, String)>,
    /// A `SimCallback` (and the selection it carries in `units`).
    pub callback: Option<crate::engine::sim::input::SimCallback>,
}

/// The command kind of a `UNITCOMMAND_*` / `RULEUCC_*` / bare order name.
pub fn command_kind(name: &str) -> Option<crate::engine::sim::command::CommandKind> {
    use crate::engine::sim::command::CommandKind as K;
    let n = name.trim_start_matches("UNITCOMMAND_").trim_start_matches("RULEUCC_");
    Some(match n {
        "Stop" => K::Stop,
        "Move" => K::Move,
        "Attack" => K::Attack,
        "AggressiveMove" => K::AggressiveMove,
        "Patrol" => K::Patrol,
        "Guard" | "AssistCommander" => K::Guard,
        "Reclaim" => K::Reclaim,
        "Repair" => K::Repair,
        "Capture" => K::Capture,
        "Nuke" => K::Nuke,
        "Tactical" => K::Tactical,
        "Teleport" => K::Teleport,
        "Ferry" => K::Ferry,
        "Dive" => K::Dive,
        "Overcharge" | "OverCharge" => K::OverCharge,
        "BuildFactory" => K::BuildFactory,
        "BuildMobile" => K::BuildMobile,
        "Upgrade" => K::Upgrade,
        // The Unload button (`RULEUCC_Transport`, no `UNITCOMMAND_` twin: FAF
        // only ever starts a command mode with it) arms a mode whose ground
        // click unloads — so the bare cap resolves to the unload. A load
        // needs an entity target anyway, which `issue_mode` never produces
        // for this path.
        "Transport" => K::TransportUnloadUnits,
        "TransportLoadUnits" => K::TransportLoadUnits,
        "TransportReverseLoadUnits" => K::TransportReverseLoadUnits,
        "TransportUnloadUnits" => K::TransportUnloadUnits,
        "TransportUnloadSpecificUnits" => K::TransportUnloadSpecificUnits,
        "DetachFromTransport" => K::DetachFromTransport,
        "SiloBuildTactical" | "BuildSiloTactical" => K::BuildSiloTactical,
        "SiloBuildNuke" | "BuildSiloNuke" => K::BuildSiloNuke,
        "Script" => K::Script,
        "SpecialAction" => K::SpecialAction,
        _ => return None,
    })
}

/// The units the interface can look up but hasn't been sent (moho's
/// `GetUnitById` answers for any unit the player knows): the latest frame's,
/// sorted by id, refreshed each beat.
#[derive(Default)]
struct UiWorld(Vec<UnitInfo>);

/// The map's deposits, for `GetDepositsAroundPoint`.
struct UiDeposits(Arc<Vec<(u8, f64, f64, f64)>>);

/// Units with orders (sorted), for `GetIdleEngineers` / `GetIdleFactories`.
#[derive(Default)]
struct UiBusy(Vec<u32>);

/// Focus-army units hurt since the last beat (`OnFocusArmyUnitDamaged`).
#[derive(Default)]
struct DamagedUnits(Vec<u32>);

/// `OnFocusArmyUnitDamaged` calls per beat at most (FAF only counts them).
const MAX_DAMAGE_NOTICES: usize = 32;

/// Each blueprint's `General.QuickSelectPriority` (`None`: not an avatar),
/// read once.
#[derive(Default)]
struct AvatarPriorities(std::collections::BTreeMap<String, Option<i64>>);

fn avatar_priority(lua: &Lua, blueprint: &str) -> LuaResult<Option<i64>> {
    if let Some(p) = lua.app_data_ref::<AvatarPriorities>().and_then(|a| a.0.get(blueprint).copied()) {
        return Ok(p);
    }
    let priority = lua
        .globals()
        .raw_get::<Option<LuaTable>>("__blueprints")?
        .and_then(|b| b.raw_get::<Option<LuaTable>>(blueprint).ok().flatten())
        .and_then(|bp| bp.raw_get::<Option<LuaTable>>("General").ok().flatten())
        .and_then(|g| g.raw_get::<Option<i64>>("QuickSelectPriority").ok().flatten());
    if let Some(mut a) = lua.app_data_mut::<AvatarPriorities>() {
        a.0.insert(blueprint.to_string(), priority);
    }
    Ok(priority)
}

/// A unit's basic data for the interface, `__engine_unit_data[id]` (read by
/// `units.lua`'s `UserUnit` methods).
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "EngineUnitData", write_only)]
struct UnitBasics<'a> {
    bp: &'a str,
    army: u32,
    /// 0..1.
    health: f32,
    /// Its maximum health now.
    max_health: f32,
    /// Its build rate now.
    build_rate: f32,
    /// 0..1 (1 = finished).
    build: f32,
    x: f32,
    y: f32,
    z: f32,
    /// Its job's progress, 0..1 (`GetWorkProgress`).
    work: f32,
    /// Its shield's health, 0..1 (`GetShieldRatio`).
    shield: f32,
}

/// Writes a unit's basic data (blueprint, army, health, build, position,
/// work progress, shield) into `__engine_unit_data[id]`; and whether its
/// table was made now.
fn write_unit_basics(lua: &Lua, data: &LuaTable, u: &UnitInfo) -> LuaResult<(LuaTable, bool)> {
    let (t, fresh) = match data.raw_get::<Option<LuaTable>>(u.id)? {
        Some(t) => (t, false),
        None => {
            let t = lua.create_table_with_capacity(0, 11)?;
            data.raw_set(u.id, t.clone())?;
            (t, true)
        }
    };
    let [x, y, z] = u.position;
    UnitBasics { bp: &u.blueprint, army: u.army, health: u.health, max_health: u.max_health, build_rate: u.build_rate, build: u.build, x, y, z, work: u.work, shield: u.shield }.write_into(lua, &t)?;
    Ok((t, fresh))
}

/// The details last written into each unit's data table
/// ([`UserVm::update_units`]): a part's Lua tables (its queue, economy,
/// stats, ...) are only made again when that part changed. A big selection
/// is thousands of units whose details mostly stay put from beat to beat.
#[derive(Default)]
struct WrittenDetails(std::collections::BTreeMap<u32, crate::engine::snapshot::UnitDetail>);

/// Units whose queue holds an order predicted at issue
/// ([`UserVm::command_issued`]), by when (interface seconds): the sim's
/// queue replaces it when it changes, or after 2 s if it never does.
#[derive(Default)]
struct PredictedOrders(std::collections::BTreeMap<u32, f64>);

/// A camera move the interface asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraRequest {
    /// The camera it is for (`None`: the main one, `WorldCamera`).
    pub camera: Option<String>,
    /// Point to look at (world).
    pub focus: Option<[f32; 3]>,
    /// Zoom: the ground across the view's longer side.
    pub zoom: Option<f32>,
    /// Back to the default view.
    pub reset: bool,
    /// Wheel notches to zoom by, toward the cursor (`ZoomScale`).
    pub wheel: Option<f32>,
    /// Seconds the move takes (0: at once), e.g. `UIZoomTo(units, 1)`.
    pub seconds: f32,
    /// `SetMaxZoomMult`: how far past the whole-map view it may zoom out.
    pub max_zoom_mult: Option<f32>,
    /// `TrackEntities`: the entities to follow (their middle).
    pub track: Option<Vec<u32>>,
    /// `HoldRotation`: the camera keeps its heading and pitch as it zooms.
    pub hold_rotation: bool,
    /// `RevertRotation`: a held rotation eases back to the usual view.
    pub revert_rotation: bool,
    /// `SnapTo` / `MoveTo` / `RestoreSettings`' orientation: heading and
    /// pitch, radians (moho's camera holds them from then on).
    pub rotation: Option<(f32, f32)>,
}

/// A world camera as the interface reads it (`GetCamera(name)`'s zoom,
/// focus, `SaveSettings`).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraState {
    pub name: String,
    pub focus: [f32; 3],
    pub zoom: f32,
    pub min: f32,
    pub max: f32,
    /// Radians down from level (moho's `Pitch`).
    pub pitch: f32,
    /// Radians about the vertical: pi faces north (moho's `Heading`).
    pub heading: f32,
}

/// An active range overlay filter (`SetOverlayFilter` / `SetOverlayFilters`,
/// FAF's `rangeoverlayparams.lua`).
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayFilter {
    pub name: String,
    /// Normal, selected and rollover colours: RGB and glow (`AARRGGBB`, AA
    /// the glow).
    pub colors: [[f32; 4]; 3],
    /// Line thickness of the minimum / maximum range rings, zoomed in and
    /// zoomed out.
    pub inner: [f32; 2],
    pub outer: [f32; 2],
    /// Range kinds it draws (bits: `snapshot::RANGE_KINDS`).
    pub kinds: u16,
    /// Which blueprints (by the renderer's index) it covers.
    pub members: Vec<bool>,
}

/// How one range kind draws on its own (the placement ghost's rings, and
/// selected or hovered units' with no overlay on): its overlay's normal,
/// selected and rollover colours (RGB, glow) and line widths, zoomed in and
/// out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeStyle {
    pub colors: [[f32; 4]; 3],
    pub inner: [f32; 2],
    pub outer: [f32; 2],
}

/// `AARRGGBB` (AA the glow) or `RRGGBB` -> RGB and glow.
fn overlay_color(s: &str) -> [f32; 4] {
    let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0xffffff);
    let c = |shift: u32| ((n >> shift) & 0xff) as f32 / 255.0;
    if s.len() >= 8 { [c(16), c(8), c(0), c(24)] } else { [c(16), c(8), c(0), 0.0] }
}

/// App data: this interface is a game's (its session's mods are active).
struct GameSession;

/// App data: the game's `SessionInit.lua` ran (it runs `UserSync.lua`).
struct SessionInitRan;

/// The user-layer VM.
pub struct UserVm {
    lua: Lua,
}

impl UserVm {
    /// A user VM over the game's file system (the same VFS the sim uses),
    /// with its own script cache.
    pub fn new(vfs: Arc<RwLock<Vfs>>) -> LuaResult<Self> {
        let cache = Arc::new(ScriptCache::new(vfs.clone()));
        Self::with_cache(vfs, cache)
    }

    /// A user VM sharing an existing compiled-script cache (the sim's,
    /// prewarmed): bytecode isn't tied to a VM, so the interface's scripts
    /// load without compiling again.
    pub fn with_cache(vfs: Arc<RwLock<Vfs>>, cache: Arc<ScriptCache>) -> LuaResult<Self> {
        let lua = Lua::new();
        lua.set_compiler(crate::engine::lua::script_cache::compiler());
        lua.set_app_data(fonts::Fonts::new(vfs.clone()));
        // The process's audio (windowed runs install one), else silence.
        let audio = crate::audio::Audio::installed().unwrap_or_else(crate::audio::Audio::disabled);
        audio.use_vfs(vfs.clone());
        lua.set_app_data(audio);
        lua.set_app_data(vfs);
        lua.set_app_data(cache);
        crate::engine::lua::debugger::attach(&lua, "UI");
        api::pending::register_core(&lua)?;
        user_api::register_stubs(&lua)?;
        api::globals::register(&lua)?;
        api::log::register(&lua)?;
        api::dofile_or_script::register(&lua)?;
        api::repr::register_safe(&lua)?;
        api::shims::register(&lua)?;
        api::thread_metatable::register(&lua)?;
        api::math::register(&lua)?;
        api::colors::register(&lua)?;
        api::disk_find_files::register(&lua)?;
        api::debug::register(&lua)?;
        api::loader::register(&lua)?;
        api::disk_to_local::register(&lua)?;
        api::sound::register(&lua)?;
        api::nil_metatable::register(&lua)?;
        api::nil_metatable::register_numbers(&lua)?;
        api::nil_metatable::register_booleans(&lua)?;
        api::nil_metatable::register_functions(&lua)?;
        api::disk_get_file_info::register(&lua)?;
        api::threads::register(&lua)?;
        api::random::register(&lua)?;
        api::remove_disk_access(&lua)?;
        api::file_collapse_path::register(&lua)?;
        api::audio::register(&lua)?;
        api::exists::register(&lua)?;
        api::path_utils::register(&lua)?;
        api::moho::register_classes(&lua, USER_CLASSES)?;
        api::blueprints::register(&lua)?;
        api::spec_footprints::register_safe(&lua)?;
        api::categories::register(&lua)?;
        user_api::register(&lua)?;
        api::core::register(&lua)?;
        world_pick::register(&lua)?;
        world_draw::register(&lua)?;
        world_pick::register_view_methods(&lua)?;
        sound::register(&lua)?;
        team_color::register(&lua)?;
        prefs::register(&lua)?;
        controls::register(&lua)?;
        cursor::register(&lua)?;
        world_mesh::register(&lua)?;
        crate::lua_api::script::exec(&lua, include_str!("units.lua"), "=[engine user units]")?;
        crate::lua_api::script::exec(&lua, include_str!("camera.lua"), "=[engine user camera]")?;
        net_lobby::register(&lua)?;
        crate::lua_api::script::exec(&lua, include_str!("lobby.lua"), "=[engine user lobby]")?;
        lua.load(include_str!("ping_grid.lua")).set_name("=[engine ping grid]").exec()?;
        movie::register(&lua)?;
        replay::register(&lua)?;
        user_api::register_method_stubs(&lua)?;
        lua.set_app_data(UiWorld::default());
        lua.set_app_data(DamagedUnits::default());
        lua_fn!(&lua, global_raw,
            /// `GetUnitById`'s fallback (units.lua): a unit of the latest frame
            /// the interface has no data for yet.
            #[lua(internal)]
            __engine_find_unit = |lua, id: u32| {
                let found = lua.app_data_ref::<UiWorld>().and_then(|w| w.0.binary_search_by_key(&id, |u| u.id).ok().map(|i| w.0[i].clone()));
                let Some(u) = found else { return Ok(false) };
                let data: LuaTable = lua.globals().raw_get("__engine_unit_data")?;
                write_unit_basics(lua, &data, &u)?;
                Ok(true)
            })?;
        lua.set_app_data(AvatarPriorities::default());
        lua_fn!(&lua, global_raw,
            /// `GetArmyAvatars`' units (units.lua): the focus army's units whose
            /// blueprint has `General.QuickSelectPriority` (FAF's ACUs), by that
            /// priority, then id.
            #[lua(internal)]
            __engine_avatars = |lua, ()| {
                let focus = lua.named_registry_value::<Option<i64>>("user.focus_army")?.unwrap_or(-1);
                let units: Vec<(u32, std::sync::Arc<str>)> = match lua.app_data_ref::<UiWorld>() {
                    Some(w) if focus >= 1 => w.0.iter().filter(|u| i64::from(u.army) == focus).map(|u| (u.id, u.blueprint.clone())).collect(),
                    _ => Vec::new(),
                };
                let mut avatars = Vec::new();
                for (id, blueprint) in units {
                    if let Some(priority) = avatar_priority(lua, &blueprint)? {
                        avatars.push((priority, id));
                    }
                }
                avatars.sort();
                Ok(avatars.into_iter().map(|(_, id)| id).collect::<Vec<u32>>())
            })?;
        lua_fn!(&lua, global,
            /// As the sim's (`api::deposits::around`), over the deposits the
            /// engine handed over.
            GetDepositsAroundPoint = |lua, (x, z, radius, kind): (f64, f64, f64, Option<u8>)| {
                let all = lua.app_data_ref::<UiDeposits>().map(|d| d.0.clone()).unwrap_or_default();
                crate::engine::api::deposits::around(lua, &all, x, z, radius, kind.unwrap_or(0))
            })?;
        lua_fn!(&lua, global_raw,
            /// The focus army's idle, finished units (no orders) whose blueprint
            /// is in `category` (`GetIdleEngineers` / `GetIdleFactories`, user_api.rs).
            #[lua(internal)]
            __engine_idle_units = |lua, category: String| {
                let focus = lua.named_registry_value::<Option<i64>>("user.focus_army")?.unwrap_or(-1);
                let units: Vec<(u32, std::sync::Arc<str>)> = match lua.app_data_ref::<UiWorld>() {
                    Some(w) if focus >= 1 => {
                        let busy = lua.app_data_ref::<UiBusy>();
                        w.0.iter()
                            .filter(|u| i64::from(u.army) == focus && u.build >= 1.0)
                            .filter(|u| busy.as_ref().is_none_or(|b| b.0.binary_search(&u.id).is_err()))
                            .map(|u| (u.id, u.blueprint.clone()))
                            .collect()
                    }
                    _ => Vec::new(),
                };
                let list = lua.create_table_with_capacity(units.len() * 2, 0)?;
                for (id, bp) in units {
                    list.push(id)?;
                    list.push(&*bp)?;
                }
                let f: LuaFunction = lua.globals().get("__engine_filter_category")?;
                f.call::<Vec<u32>>((list, category))
            })?;
        Ok(Self { lua })
    }

    /// A game's interface starts as moho's user state: `__active_mods` is
    /// the session's sim mods (and UI-only `--mod`s, the engine's own), and
    /// [`Self::init`] runs the game's `/lua/SessionInit.lua`, which adds the
    /// player's UI mods (its `mods.lua` `GetUiMods`: FAF's leaves out
    /// `Hotstats`, LOUD's sorts by name). They hook the scripts loaded
    /// after that (`mods::sync_hooks`). Before [`Self::init`]; the front
    /// end starts with none.
    pub fn activate_session_mods(&self, session: &crate::engine::mods::SessionMods) -> LuaResult<()> {
        use crate::engine::mods;
        let uids: Vec<String> = session.ui.iter().filter(|u| session.all.get(*u).is_some_and(|m| m.ui_only)).cloned().collect();
        let ui = mods::resolve(&session.all, &uids);
        mods::log_active("user", &ui);
        let all: Vec<_> = session.sim.iter().cloned().chain(ui).collect();
        let vfs = self.lua.app_data_ref::<Arc<RwLock<Vfs>>>().map(|v| v.clone()).ok_or_else(|| LuaError::runtime("no VFS"))?;
        let vfs = vfs.read().map_err(|_| LuaError::runtime("VFS poisoned"))?;
        mods::activate(&self.lua, &vfs, &all)?;
        self.lua.set_app_data(GameSession);
        Ok(())
    }

    /// Runs `/lua/userInit.lua` (FAF's user-layer init), after loading the
    /// blueprints and categories the interface reads, as moho's user state
    /// has them. A game's interface ([`Self::activate_session_mods`]) runs
    /// the game's `/lua/SessionInit.lua` instead, as moho's session does:
    /// it runs `userInit.lua`, adds the UI mods, then `UserSync.lua`.
    pub fn init(&self) -> LuaResult<()> {
        let started = std::time::Instant::now();
        let (code, session_init) = {
            let vfs = self.lua.app_data_ref::<Arc<RwLock<Vfs>>>().map(|v| v.clone()).ok_or_else(|| LuaError::runtime("no VFS"))?;
            let vfs = vfs.read().map_err(|_| LuaError::runtime("VFS poisoned"))?;
            let session_init = self.lua.app_data_ref::<GameSession>().is_some() && vfs.contains("/lua/SessionInit.lua");
            (crate::engine::mods::script_source(&self.lua, &vfs, "lua/RuleInit.lua").map_err(LuaError::external)?, session_init)
        };
        crate::engine::lua::execute::exec_lua_string(&self.lua, &String::from_utf8_lossy(&code), "lua/RuleInit.lua")?;
        api::blueprints::apply_defaults(&self.lua, true)?;
        api::categories::build(&self.lua)?;
        tracing::info!(ms = started.elapsed().as_millis() as u64, "user layer blueprints loaded");
        let doscript: LuaFunction = self.lua.globals().get("doscript")?;
        if session_init {
            doscript.call::<()>("/lua/SessionInit.lua")?;
            self.lua.set_app_data(SessionInitRan);
            self.log_ui_mods()?;
        } else {
            doscript.call::<()>("/lua/userInit.lua")?;
        }
        // The front end (profile creation, the options dialog) fills the
        // profile's options with their defaults, which the game reads
        // unguarded (worldview's selection thresholds); we start without it.
        let import: LuaFunction = self.lua.globals().get("import")?;
        let options: LuaTable = import.call("/lua/options/optionslogic.lua")?;
        options.get::<LuaFunction>("GetCurrent")?.call::<()>(())?;
        self.apply_startup_options()
    }

    /// The profile's options, as moho's start-up `optionslogic.Apply(true)`
    /// sets them: every setter, or on FAF's code only the sound and video
    /// sections' (`GameCode::runs_every_option_setter`; the video ones send
    /// console commands, `render/console.rs`; those that skip themselves at
    /// start are sent from the profile there).
    pub fn apply_startup_options(&self) -> LuaResult<()> {
        use crate::engine::game_code::GameCode;
        let all = self.lua.app_data_ref::<Arc<RwLock<Vfs>>>().and_then(|v| v.read().ok().map(|v| GameCode::detect(&v).runs_every_option_setter())).unwrap_or(false);
        self.lua.globals().raw_set("__engine_apply_all_options", all)?;
        self.lua
            .load(
                r#"
                local logic = import('/lua/options/optionslogic.lua')
                local current = logic.GetCurrent()
                local function apply(item)
                    if item.set and current[item.key] ~= nil then
                        local ok, e = pcall(item.set, item.key, current[item.key], true)
                        if not ok then WARN('option ', item.key, ': ', e) end
                    end
                end
                for name, section in logic.GetOptionsData() do
                    for _, item in section.items or {} do
                        if __engine_apply_all_options or name == 'sound' or name == 'video' then apply(item) end
                    end
                end
                rawset(_G, '__engine_apply_all_options', nil)
                "#,
            )
            .set_name("=[engine startup options]")
            .exec()
    }

    /// The UI-only mods in `__active_mods` once `SessionInit.lua` has run
    /// (the game's choice), logged as the sim's are.
    fn log_ui_mods(&self) -> LuaResult<()> {
        let Some(list) = self.lua.globals().raw_get::<Option<LuaTable>>("__active_mods")? else { return Ok(()) };
        for m in list.sequence_values::<LuaValue>() {
            let LuaValue::Table(m) = m? else { continue };
            if !m.raw_get::<Option<bool>>("ui_only").ok().flatten().unwrap_or(false) {
                continue;
            }
            let text = |k: &str| m.raw_get::<LuaValue>(k).ok().and_then(|v| v.to_string().ok()).unwrap_or_default();
            tracing::info!(vm = "user", uid = %text("uid"), name = %text("name"), version = %text("version"), ui_only = true, "active mod");
        }
        Ok(())
    }

    /// Builds the in-game interface: FAF's own panels, unchanged (stage 1:
    /// the economy bar). The session's sync handling first, unless
    /// `SessionInit.lua` ran it.
    pub fn start_hud(&self) -> LuaResult<()> {
        if self.lua.app_data_ref::<SessionInitRan>().is_none() {
            self.lua.globals().get::<LuaFunction>("doscript")?.call::<()>("/lua/UserSync.lua")?;
        }
        self.lua.load(include_str!("hud.lua")).set_name("=[engine hud]").exec()
    }

    /// Starts the main menu, as moho's front end does: `uimain.SetupUI()`
    /// (skin, layout, cursor), then `uimain.StartFrontEndUI()` (FAF's
    /// `menus/main.lua`).
    pub fn start_front_end(&self) -> LuaResult<()> {
        self.lua.load(include_str!("front_end.lua")).set_name("=[engine front end]").exec()
    }

    /// A sim beat: the UI's clock moves to `tick`, its economy view is
    /// `economy` (for the focus army), then FAF's beat functions run
    /// (`gamemain.OnBeat`, which updates the economy bar and other panels).
    pub fn beat(&self, tick: u64, economy: &[crate::engine::snapshot::ArmyEconomy]) -> LuaResult<()> {
        let g = self.lua.globals();
        g.raw_set("__engine_ui_tick", tick)?;
        let focus: i64 = self.lua.named_registry_value("user.focus_army")?;
        let shown = if focus >= 1 { focus as u32 } else { 1 };
        // Each army's synced overflow flag, for `GetResourceSharing` (which
        // reads the focus army's when called).
        let sharing = match g.raw_get::<Option<LuaTable>>("__engine_resource_sharing")? {
            Some(t) => t,
            None => {
                let t = self.lua.create_table_with_capacity(economy.len(), 0)?;
                g.raw_set("__engine_resource_sharing", &t)?;
                t
            }
        };
        for e in economy {
            sharing.raw_set(e.army, e.resource_sharing)?;
        }
        if let Some(e) = economy.iter().find(|e| e.army == shown) {
            let t = self.lua.create_table()?;
            for (key, i) in [("stored", 0), ("maxStorage", 1), ("income", 2), ("lastUseRequested", 3), ("lastUseActual", 4)] {
                let v = self.lua.create_table()?;
                v.raw_set("MASS", e.mass[i])?;
                v.raw_set("ENERGY", e.energy[i])?;
                t.raw_set(key, v)?;
            }
            // Reclaim: FAF's panel shows the running total and its rate,
            // and takes the previous beat's rate back out of the income
            // ("generated" income): moho's income, like the sim's, includes
            // what was reclaimed, a tick late.
            let reclaimed = self.lua.create_table()?;
            reclaimed.raw_set("MASS", e.mass[5])?;
            reclaimed.raw_set("ENERGY", e.energy[5])?;
            t.raw_set("reclaimed", reclaimed)?;
            g.raw_set("__engine_economy", t)?;
        }
        let import: LuaFunction = g.get("import")?;
        let gamemain: LuaTable = import.call("/lua/ui/game/gamemain.lua")?;
        if let Some(on_beat) = gamemain.get::<Option<LuaFunction>>("OnBeat")? {
            on_beat.call::<()>(())?;
        }
        let damaged = self.lua.app_data_mut::<DamagedUnits>().map(|mut d| std::mem::take(&mut d.0)).unwrap_or_default();
        if !damaged.is_empty()
            && let Some(on_damaged) = gamemain.get::<Option<LuaFunction>>("OnFocusArmyUnitDamaged")? {
            let get_unit: LuaFunction = g.get("GetUnitById")?;
            for id in damaged {
                let unit: LuaValue = get_unit.call(id.to_string())?;
                if !unit.is_nil() {
                    on_damaged.call::<()>(unit)?;
                }
            }
        }
        Ok(())
    }

    /// The session's scenario (`SessionGetScenarioInfo`).
    pub fn set_scenario(&self, info: &crate::engine::sync::SyncValue) -> LuaResult<()> {
        self.lua.globals().raw_set("__engine_session", info.to_lua(&self.lua)?)
    }

    /// The session's armies (`GetArmiesTable`, `GetSessionClients`).
    pub fn set_armies(&self, armies: &[crate::engine::snapshot::ArmyInfo]) -> LuaResult<()> {
        let list = self.lua.create_table()?;
        // Each human army's player is a command source, in army order
        // (1-based here, as FAF's chat matches them against clients').
        let mut source = 0;
        for a in armies {
            let t = self.lua.create_table()?;
            let sources = self.lua.create_table()?;
            if a.human {
                source += 1;
                sources.raw_set(1, source)?;
            }
            t.raw_set("authorizedCommandSources", sources)?;
            t.raw_set("name", a.name.as_str())?;
            t.raw_set("nickname", a.nickname.as_str())?;
            t.raw_set("faction", a.faction as i64 - 1)?;
            t.raw_set("human", a.human)?;
            t.raw_set("civilian", a.civilian)?;
            t.raw_set("outOfGame", false)?;
            t.raw_set("showScore", !a.civilian)?;
            // Until the sim's colours arrive (`set_army_display`).
            let color = format!("{:08x}", crate::engine::api::colors::fallback_army_color(a.index));
            t.raw_set("color", color.as_str())?;
            t.raw_set("iconColor", color.as_str())?;
            t.raw_set("armyIndex", a.index)?;
            list.raw_set(a.index, t)?;
        }
        self.lua.globals().raw_set("__engine_armies", list)
    }

    /// How the sim shows each army: `GetArmiesTable`'s `iconColor` (army
    /// colour) and `color` (player colour; moho's user army keeps them in
    /// that order after the sim's, docs/research/verifying-against-the-binary.md) and
    /// `showScore`.
    /// The interface's `TeamColorMode` state: its version, and the army
    /// colours to draw instead of the session's while the mode is on.
    pub fn team_colors(&self) -> (u64, Option<Vec<u32>>) {
        team_color::current(&self.lua)
    }

    pub fn set_army_display(&self, armies: &[crate::engine::snapshot::ArmyDisplay]) -> LuaResult<()> {
        let Some(list) = self.lua.globals().raw_get::<Option<LuaTable>>("__engine_armies")? else { return Ok(()) };
        for d in armies {
            if let Some(t) = list.raw_get::<Option<LuaTable>>(d.army)? {
                t.raw_set("color", format!("{:08x}", d.player_color))?;
                t.raw_set("iconColor", format!("{:08x}", d.color))?;
                t.raw_set("showScore", d.show_score)?;
            }
        }
        Ok(())
    }

    /// Units' names given by the sim (`SetCustomName`), replacing the last
    /// set: `__engine_custom_names[id]`, read by `UserUnit:GetCustomName`.
    pub fn set_custom_names(&self, names: &[(u32, String)]) -> LuaResult<()> {
        let t = self.lua.create_table_with_capacity(0, names.len())?;
        for (id, name) in names {
            t.raw_set(*id, name.as_str())?;
        }
        self.lua.globals().raw_set("__engine_custom_names", t)
    }

    /// The sim's explicit one-way alliances (`IsAlly` / `IsEnemy` /
    /// `IsNeutral`; unset pairs are enemies, an army is its own ally).
    pub fn set_alliances(&self, alliances: &[(u32, u32, crate::engine::sim::army::Alliance)]) -> LuaResult<()> {
        use crate::engine::sim::army::Alliance;
        let all = self.lua.create_table()?;
        for &(from, to, a) in alliances {
            let row = match all.raw_get::<Option<LuaTable>>(from)? {
                Some(r) => r,
                None => {
                    let r = self.lua.create_table()?;
                    all.raw_set(from, r.clone())?;
                    r
                }
            };
            row.raw_set(to, match a {
                Alliance::Ally => "Ally",
                Alliance::Neutral => "Neutral",
                Alliance::Enemy => "Enemy",
            })?;
        }
        self.lua.globals().raw_set("__engine_alliances", all)
    }

    /// The latest data of units the interface knows (selected ones). True
    /// when what a unit may build changed (its build restrictions arrived or
    /// changed, or an upgrade was queued or done): moho then re-runs the
    /// selection callbacks so the construction panel's tabs follow.
    pub fn update_units(&self, units: &[UnitInfo]) -> LuaResult<bool> {
        let data: LuaTable = self.lua.globals().raw_get("__engine_unit_data")?;
        let mut command_data_changed = false;
        let mut rally_changed = false;
        // (Taken out while Lua runs below; an error loses it, which only
        // means the next beat writes everything.)
        let mut written = self.lua.remove_app_data::<WrittenDetails>().unwrap_or_default();
        // The fields every selected unit writes each beat, as Lua strings
        // made once (a `&str` key is made into one per write).
        let key = |k: &str| self.lua.create_string(k);
        let (k_focus, k_fuel, k_fire_state, k_script_bits, k_paused, k_overcharge_paused, k_creator, k_auto_mode, k_auto_surface, k_submerged, k_stunned, k_guarding) = (key("focus")?, key("fuel")?, key("fireState")?, key("scriptBits")?, key("paused")?, key("overchargePaused")?, key("creator")?, key("autoMode")?, key("autoSurface")?, key("submerged")?, key("stunned")?, key("guarding")?);
        let now = self.time();
        // (Moved out and back, as no Lua runs in between: no allocation.)
        let mut predicted = self.lua.app_data_mut::<PredictedOrders>().map(|mut p| std::mem::take(&mut p.0)).unwrap_or_default();
        for u in units {
            let (t, fresh) = write_unit_basics(&self.lua, &data, u)?;
            if let Some(d) = &u.detail {
                // What this table already holds (nothing if it was just made).
                let p = if fresh { None } else { written.0.get(&u.id) };
                let changed = |part: &dyn Fn(&crate::engine::snapshot::UnitDetail) -> bool| p.is_none_or(|p| !part(p));
                // An order predicted at issue (`command_issued`) the sim
                // hasn't confirmed in 2 s (refused, or never sent): back to
                // the sim's queue.
                let stale_prediction = predicted.get(&u.id).is_some_and(|&at| now - at > 2.0);
                if changed(&|p| p.commands == d.commands) || stale_prediction {
                    predicted.remove(&u.id);
                    let q = self.lua.create_table_with_capacity(d.commands.len(), 0)?;
                    for (i, (kind, bp, count, at)) in d.commands.iter().enumerate() {
                        let c = self.lua.create_table_with_capacity(0, 6)?;
                        let name = crate::engine::sim::command::CommandKind::from_u8(*kind).map_or("None".to_string(), |k| format!("{k:?}"));
                        c.raw_set("type", name)?;
                        c.raw_set("ID", i + 1)?;
                        if let Some(bp) = bp {
                            c.raw_set("blueprintId", bp.as_str())?;
                        }
                        c.raw_set("count", *count)?;
                        c.raw_set("x", at[0])?;
                        c.raw_set("y", at[1])?;
                        c.raw_set("z", at[2])?;
                        q.raw_set(i + 1, c)?;
                    }
                    t.raw_set("queue", q)?;
                    // Its queued upgrades: what it may build includes theirs.
                    let upgrade = crate::engine::sim::command::CommandKind::Upgrade as u8;
                    let upgrades: Vec<String> = d.commands.iter().filter(|(k, ..)| *k == upgrade).filter_map(|(_, bp, ..)| bp.clone()).collect();
                    let old: Option<Vec<String>> = t.raw_get("upgrades")?;
                    if old.unwrap_or_default() != upgrades {
                        command_data_changed = true;
                        t.raw_set("upgrades", upgrades)?;
                    }
                }
                // A factory's command queue (its rally orders): what moho's
                // `UserUnit:GetCommandQueue` gives for a FACTORY (units.lua),
                // FAF's `rallypoint.lua` flagging its last order.
                if changed(&|p| p.factory_commands == d.factory_commands) {
                    let had = t.raw_get::<Option<LuaTable>>("factoryQueue")?.map_or(0, |q| q.raw_len());
                    if had > 0 || !d.factory_commands.is_empty() {
                        rally_changed = true;
                    }
                    let q = self.lua.create_table_with_capacity(d.factory_commands.len(), 0)?;
                    for (i, (kind, at)) in d.factory_commands.iter().enumerate() {
                        let c = self.lua.create_table_with_capacity(0, 5)?;
                        let name = crate::engine::sim::command::CommandKind::from_u8(*kind).map_or("None".to_string(), |k| format!("{k:?}"));
                        c.raw_set("type", name)?;
                        c.raw_set("ID", i + 1)?;
                        c.raw_set("x", at[0])?;
                        c.raw_set("y", at[1])?;
                        c.raw_set("z", at[2])?;
                        q.raw_set(i + 1, c)?;
                    }
                    t.raw_set("factoryQueue", q)?;
                }
                t.raw_set(&k_focus, d.focus)?;
                if changed(&|p| p.econ == d.econ) {
                    let e = self.lua.create_table_with_capacity(0, 6)?;
                    for (k, v) in ["massConsumed", "energyConsumed", "massProduced", "energyProduced", "massRequested", "energyRequested"].iter().zip(d.econ) {
                        e.raw_set(*k, v)?;
                    }
                    t.raw_set("econ", e)?;
                }
                t.raw_set(&k_fuel, d.fuel)?;
                t.raw_set(&k_fire_state, d.fire_state)?;
                t.raw_set(&k_script_bits, d.script_bits)?;
                t.raw_set(&k_paused, d.paused)?;
                t.raw_set(&k_overcharge_paused, d.overcharge_paused)?;
                if changed(&|p| p.missiles == d.missiles) {
                    t.raw_set("missiles", d.missiles.to_vec())?;
                }
                t.raw_set(&k_creator, d.creator)?;
                t.raw_set(&k_auto_mode, d.auto_mode)?;
                t.raw_set(&k_auto_surface, d.auto_surface)?;
                t.raw_set(&k_submerged, d.submerged)?;
                if changed(&|p| p.cargo == d.cargo) {
                    t.raw_set("cargo", d.cargo.clone())?;
                }
                if changed(&|p| p.assisters == d.assisters) {
                    t.raw_set("assisters", d.assisters.clone())?;
                }
                t.raw_set(&k_stunned, d.stunned)?;
                t.raw_set(&k_guarding, d.guarding)?;
                // Unit statistics (veterancy level / experience, mass
                // reclaimed, ...): FAF's `UserUnit:GetStat(name, default)`.
                if changed(&|p| p.stats == d.stats) {
                    let s = self.lua.create_table_with_capacity(0, d.stats.len())?;
                    for (k, v) in &d.stats {
                        s.raw_set(k.as_str(), *v)?;
                    }
                    t.raw_set("stats", s)?;
                }
                if changed(&|p| p.restricted == d.restricted) {
                    let old: Option<Vec<String>> = t.raw_get("restricted")?;
                    if old.as_ref() != Some(&d.restricted) {
                        command_data_changed = true;
                        t.raw_set("restricted", d.restricted.clone())?;
                    }
                }
                written.0.insert(u.id, d.clone());
            }
        }
        self.lua.set_app_data(written);
        match self.lua.app_data_mut::<PredictedOrders>() {
            Some(mut p) => p.0 = predicted,
            None if !predicted.is_empty() => drop(self.lua.set_app_data(PredictedOrders(predicted))),
            None => {}
        }
        // A selected factory's rally orders arrived (details come a snapshot
        // after the selection; moho has them at once) or moved: FAF shows
        // rally flags from the selection callback, so run that part again.
        if rally_changed {
            let refresh = self.lua.load("import('/lua/ui/game/rallypoint.lua').OnSelectionChanged(GetSelectedUnits() or {})").set_name("=[engine rally refresh]");
            if let Err(e) = refresh.exec() {
                tracing::warn!("UI rally points: {e}");
            }
        }
        // The shown factory's queue changed: FAF's OnQueueChanged.
        if let Some(f) = self.lua.globals().raw_get::<Option<LuaFunction>>("__engine_check_queue")?
            && let Err(e) = f.call::<()>(()) {
            tracing::warn!("UI OnQueueChanged: {e}");
        }
        Ok(command_data_changed)
    }

    /// The unit whose build queue the construction panel shows
    /// (`SetCurrentFactoryForQueueDisplay`), if any. A mobile factory's is
    /// its external factory, which isn't selected: the engine sends its
    /// queue too ([`crate::engine::snapshot::DebugRequest::queue_factory`]).
    pub fn queue_factory(&self) -> Option<u32> {
        self.lua.globals().raw_get::<Option<u32>>("__engine_queue_factory").ok().flatten()
    }

    /// Units gone from the game (their UserUnits read as dead).
    /// The latest frame's units, for `GetUnitById` on units the interface
    /// hasn't been sent (moho answers for any unit the player knows).
    pub fn set_world(&self, mut units: Vec<UnitInfo>) {
        units.sort_unstable_by_key(|u| u.id);
        // The focus army's units that lost health since the last frame:
        // the next beat tells `gamemain.OnFocusArmyUnitDamaged` (FAF's
        // battle music counts them).
        let focus = self.lua.named_registry_value::<Option<i64>>("user.focus_army").ok().flatten().unwrap_or(-1);
        if focus >= 1
            && let Some(old) = self.lua.app_data_ref::<UiWorld>() {
            let mut damaged = self.lua.app_data_mut::<DamagedUnits>();
            // Both lists are in id order: one walk pairs them up.
            let mut j = 0;
            for u in units.iter().filter(|u| i64::from(u.army) == focus) {
                while j < old.0.len() && old.0[j].id < u.id {
                    j += 1;
                }
                let hurt = j < old.0.len() && old.0[j].id == u.id && u.health < old.0[j].health;
                if let (true, Some(d)) = (hurt, damaged.as_mut())
                    && d.0.len() < MAX_DAMAGE_NOTICES {
                    d.0.push(u.id);
                }
            }
        }
        self.lua.set_app_data(UiWorld(units));
    }

    /// The active build template's structures (blueprint, x / z offset from
    /// the cursor), if one is set (`SetActiveBuildTemplate`).
    pub fn build_template(&self) -> LuaResult<Option<Vec<(String, f32, f32)>>> {
        let Some(t) = self.lua.globals().raw_get::<Option<LuaTable>>("__engine_build_template")? else { return Ok(None) };
        let mut out = Vec::new();
        for i in 3..=t.raw_len() as i64 {
            let Some(e) = t.raw_get::<Option<LuaTable>>(i)? else { continue };
            let (Some(bp), Some(x), Some(z)) = (e.raw_get::<Option<String>>(1)?, e.raw_get::<Option<f32>>(3)?, e.raw_get::<Option<f32>>(4)?) else { continue };
            out.push((bp.to_lowercase(), x, z));
        }
        Ok((!out.is_empty()).then_some(out))
    }

    /// How this client takes part: watching a recording (`SessionIsReplay`),
    /// and having no army of its own (`IsObserver`).
    pub fn set_session_flags(&self, replay: bool, observer: bool) -> LuaResult<()> {
        let g = self.lua.globals();
        g.raw_set("__engine_replay", replay)?;
        g.raw_set("__engine_observer", observer)
    }

    /// The interface's ground decals when they changed since the last call
    /// (`None`: unchanged): map decals, corner-placed, albedo.
    pub fn user_decals(&self) -> LuaResult<Option<Vec<crate::engine::assets::scmap::Decal>>> {
        let f: LuaFunction = self.lua.globals().get("__engine_user_decals")?;
        let (version, list): (i64, LuaTable) = f.call(())?;
        let seen: Option<i64> = self.lua.named_registry_value("ui.decals_seen")?;
        if seen == Some(version) {
            return Ok(None);
        }
        self.lua.set_named_registry_value("ui.decals_seen", version)?;
        let values: Vec<LuaValue> = list.sequence_values::<LuaValue>().collect::<LuaResult<_>>()?;
        let num = |v: &LuaValue| match v {
            LuaValue::Integer(i) => *i as f32,
            LuaValue::Number(n) => *n as f32,
            _ => 0.0,
        };
        let mut out = Vec::new();
        for c in values.chunks_exact(7) {
            let LuaValue::String(tex) = &c[0] else { continue };
            let (x, y, z, sx, sy, sz) = (num(&c[1]), num(&c[2]), num(&c[3]), num(&c[4]), num(&c[5]), num(&c[6]));
            out.push(crate::engine::assets::scmap::Decal {
                kind: crate::engine::assets::scmap::decal_kind::ALBEDO,
                textures: vec![tex.to_str()?.to_lowercase()],
                scale: [sx, sy, sz],
                position: [x - sx / 2.0, y, z - sz / 2.0],
                rotation: [0.0; 3],
                cutoff: 0.0,
                cutoff_min: 0.0,
                army: -1,
            });
        }
        Ok(Some(out))
    }

    /// The map's resource deposits (`GetDepositsAroundPoint`).
    pub fn set_deposits(&self, deposits: Arc<Vec<(u8, f64, f64, f64)>>) {
        self.lua.set_app_data(UiDeposits(deposits));
    }

    /// The order waypoint under the mouse this frame (`GetHighlightCommand`):
    /// where, its `commandType` number, its command id and a build's
    /// blueprint.
    pub fn set_highlight_command(&self, command: Option<([f32; 3], u8, u32, Option<String>)>) -> LuaResult<()> {
        let value = match command {
            Some((at, kind, id, blueprint)) => {
                let t = self.lua.create_table_with_capacity(0, 6)?;
                t.raw_set("x", at[0])?;
                t.raw_set("y", at[1])?;
                t.raw_set("z", at[2])?;
                t.raw_set("commandType", kind)?;
                t.raw_set("commandId", id)?;
                t.raw_set("blueprintId", blueprint)?;
                LuaValue::Table(t)
            }
            None => LuaNil,
        };
        self.lua.globals().raw_set("__engine_highlight_command", value)
    }

    /// The unit under the mouse this frame (`GetRolloverInfo`), if any.
    pub fn set_rollover(&self, unit: Option<u32>) -> LuaResult<()> {
        self.lua.globals().raw_set("__engine_rollover", unit)
    }

    /// Is interface point (x, y) over a control other than a world view?
    pub fn over_interface(&self, x: f32, y: f32) -> LuaResult<bool> {
        controls::over_interface(&self.lua, x, y)
    }

    /// The units with orders this tick (sorted): the rest are idle.
    pub fn set_busy(&self, busy: &[u32]) {
        self.lua.set_app_data(UiBusy(busy.to_vec()));
    }

    /// Units the interface holds data for (selected, or looked up), sorted.
    pub fn known_units(&self) -> LuaResult<Vec<u32>> {
        let data: LuaTable = self.lua.globals().raw_get("__engine_unit_data")?;
        let mut ids: Vec<u32> = data.pairs::<u32, LuaValue>().filter_map(|p| p.ok().map(|(k, _)| k)).collect();
        ids.sort_unstable();
        Ok(ids)
    }

    /// The beat's refresh of units the interface holds beyond `selection`
    /// (sorted): `present` gives the ones still interface units (see
    /// [`crate::engine::snapshot::EntityView::is_interface_unit`]), which are
    /// updated; the rest (destroyed, killed and dying, out of sight) are
    /// dropped, so `UserUnit:IsDead()` is true for them.
    pub fn refresh_held_units(&self, selection: &[u32], present: impl FnOnce(&[u32]) -> Vec<UnitInfo>) -> LuaResult<()> {
        let others: Vec<u32> = self.known_units()?.into_iter().filter(|id| selection.binary_search(id).is_err()).collect();
        let present = present(&others);
        let gone: Vec<u32> = others.iter().copied().filter(|id| present.binary_search_by_key(id, |u| u.id).is_err()).collect();
        self.update_units(&present)?;
        self.remove_units(&gone)
    }

    pub fn remove_units(&self, ids: &[u32]) -> LuaResult<()> {
        let data: LuaTable = self.lua.globals().raw_get("__engine_unit_data")?;
        let mut written = self.lua.app_data_mut::<WrittenDetails>();
        for &id in ids {
            data.raw_set(id, LuaNil)?;
            if let Some(w) = written.as_mut() {
                w.0.remove(&id);
            }
        }
        drop(written);
        if let Some(mut p) = self.lua.app_data_mut::<PredictedOrders>() {
            for id in ids {
                p.0.remove(id);
            }
        }
        Ok(())
    }

    /// The player's selection changed to `ids` (sorted): FAF's
    /// `gamemain.OnSelectionChanged(old, new, added, removed)` runs, and
    /// `GetSelectedUnits` returns them.
    pub fn set_selection(&self, ids: &[u32]) -> LuaResult<()> {
        self.selection_changed(ids, false)
    }

    /// The selection's command data changed (see [`Self::update_units`]):
    /// `OnSelectionChanged(sel, sel, {}, {})`, which FAF reads as the same
    /// selection refreshed.
    pub fn refresh_selection(&self) -> LuaResult<()> {
        let ids: Vec<u32> = self.lua.named_registry_value::<Option<Vec<u32>>>("user.selection")?.unwrap_or_default();
        if ids.is_empty() {
            return Ok(());
        }
        self.selection_changed(&ids, true)
    }

    fn selection_changed(&self, ids: &[u32], refresh: bool) -> LuaResult<()> {
        let unit: LuaFunction = self.lua.globals().get("__engine_unit")?;
        let old: Vec<u32> = self.lua.named_registry_value::<Option<Vec<u32>>>("user.selection")?.unwrap_or_default();
        if old == ids && !refresh {
            return Ok(());
        }
        let list = |ids: &mut dyn Iterator<Item = u32>| -> LuaResult<LuaTable> {
            let t = self.lua.create_table()?;
            for (i, id) in ids.enumerate() {
                t.raw_set(i + 1, unit.call::<LuaTable>(id)?)?;
            }
            Ok(t)
        };
        let new_t = list(&mut ids.iter().copied())?;
        let old_t = list(&mut old.iter().copied())?;
        let added = list(&mut ids.iter().copied().filter(|i| old.binary_search(i).is_err()))?;
        let removed = list(&mut old.iter().copied().filter(|i| ids.binary_search(i).is_err()))?;
        self.lua.set_named_registry_value("user.selection", ids.to_vec())?;
        self.lua.globals().raw_set("__engine_selection", if ids.is_empty() { LuaValue::Nil } else { LuaValue::Table(new_t.clone()) })?;
        let import: LuaFunction = self.lua.globals().get("import")?;
        let gamemain: LuaTable = import.call("/lua/ui/game/gamemain.lua")?;
        if let Some(f) = gamemain.get::<Option<LuaFunction>>("OnSelectionChanged")? {
            f.call::<()>((old_t, new_t, added, removed))?;
        }
        Ok(())
    }

    /// Orders the interface gave since the last call (its buttons and
    /// panels: stop, factory builds, upgrades, ...), oldest first.
    /// Order feedback and world meshes (rally flags) to draw this frame.
    pub fn interface_meshes(&self) -> Vec<world_mesh::InterfaceMesh> {
        world_mesh::meshes(&self.lua)
    }

    pub fn take_orders(&self) -> LuaResult<Vec<UiOrder>> {
        let list: LuaTable = self.lua.globals().raw_get("__engine_ui_orders")?;
        let mut callbacks: Vec<Option<_>> = std::mem::take(&mut self.lua.app_data_mut::<user_api::PendingSimCallbacks>().expect("PendingSimCallbacks").0).into_iter().map(Some).collect();
        let mut out = Vec::new();
        for o in list.clone().sequence_values::<LuaTable>() {
            let o = o?;
            if let Some(index) = o.raw_get::<Option<usize>>("callback")? {
                if let Some((callback, units)) = index.checked_sub(1).and_then(|i| callbacks.get_mut(i)).and_then(Option::take) {
                    out.push(UiOrder { command: String::new(), units: Some(units), blueprint: None, count: 0, clear: false, toggle: None, script: Vec::new(), callback: Some(callback) });
                }
                continue;
            }
            let units = o.raw_get::<Option<LuaTable>>("units")?.map(|t| t.sequence_values::<LuaTable>().filter_map(|u| u.ok()?.raw_get::<Option<u32>>("_id").ok().flatten()).collect());
            out.push(UiOrder {
                command: o.raw_get::<Option<String>>("command")?.unwrap_or_default(),
                units,
                blueprint: o.raw_get::<Option<String>>("bp")?,
                count: o.raw_get::<Option<f64>>("count")?.map_or(1, |c| c.max(1.0) as u32),
                clear: o.raw_get::<Option<bool>>("clear")?.unwrap_or(false),
                script: match o.raw_get::<LuaValue>("arg")? {
                    LuaValue::Table(t) => {
                        let mut kv: Vec<(String, String)> = t
                            .pairs::<String, LuaValue>()
                            .filter_map(|p| p.ok())
                            .filter_map(|(k, v)| match v {
                                LuaValue::String(s) => Some((k, s.to_str().ok()?.to_string())),
                                _ => None,
                            })
                            .collect();
                        kv.sort();
                        kv
                    }
                    _ => Vec::new(),
                },
                toggle: {
                    use crate::engine::sim::input::Toggle;
                    let v = o.raw_get::<Option<f64>>("value")?.unwrap_or(0.0) as i32;
                    match o.raw_get::<Option<String>>("toggle")?.as_deref() {
                        Some("fire") => Some(Toggle::FireState(v)),
                        Some("pause") => Some(Toggle::Paused(v != 0)),
                        Some("repeat") => Some(Toggle::RepeatQueue(v != 0)),
                        Some("vo") => Some(Toggle::StagingVo(v != 0)),
                        Some("bit") => Some(Toggle::ScriptBit(o.raw_get::<Option<f64>>("bit")?.unwrap_or(0.0) as u8, v != 0)),
                        Some("count") => Some(Toggle::BuildCount(o.raw_get::<Option<f64>>("index")?.unwrap_or(0.0) as u16, v)),
                        Some("auto") => Some(Toggle::AutoMode(v != 0)),
                        Some("autosurface") => Some(Toggle::AutoSurfaceMode(v != 0)),
                        _ => None,
                    }
                },
                callback: None,
            });
        }
        list.clear()?;
        Ok(out)
    }

    /// FAF's current command mode (`commandmode.GetCommandMode`): `("order",
    /// "RULEUCC_Attack")`, `("build", "ueb0101")`, ... or `None`.
    pub fn command_mode(&self) -> LuaResult<Option<(String, String)>> {
        let import: LuaFunction = self.lua.globals().get("import")?;
        let cm: LuaTable = import.call("/lua/ui/game/commandmode.lua")?;
        let (mode, data): (LuaValue, LuaValue) = match cm.get::<Option<LuaFunction>>("GetCommandMode")? {
            Some(f) => {
                let t: LuaTable = f.call(())?;
                (t.raw_get(1)?, t.raw_get(2)?)
            }
            None => return Ok(None),
        };
        let LuaValue::String(mode) = mode else { return Ok(None) };
        let name = match data {
            LuaValue::Table(d) => d.raw_get::<Option<String>>("name")?.unwrap_or_default(),
            _ => String::new(),
        };
        Ok(Some((mode.to_str()?.to_string(), name)))
    }

    /// A blueprint's footprint (`Footprint.SizeX`, `SizeZ`; 1 when absent).
    pub fn footprint(&self, bp: &str) -> LuaResult<(f32, f32)> {
        let bps: LuaTable = self.lua.globals().raw_get("__blueprints")?;
        let fp = bps.raw_get::<Option<LuaTable>>(bp.to_lowercase())?.and_then(|b| b.raw_get::<Option<LuaTable>>("Footprint").ok().flatten());
        let size = |k: &str| fp.as_ref().and_then(|f| f.raw_get::<Option<f64>>(k).ok().flatten()).unwrap_or(1.0) as f32;
        Ok((size("SizeX"), size("SizeZ")))
    }

    /// How far apart drag building lays a blueprint's structures: its skirt
    /// (`Physics.SkirtSizeX/Z`: structures' skirts may not overlap), at least
    /// its footprint.
    pub fn drag_spacing(&self, bp: &str) -> LuaResult<(f32, f32)> {
        let (fx, fz) = self.footprint(bp)?;
        let bps: LuaTable = self.lua.globals().raw_get("__blueprints")?;
        let physics = bps.raw_get::<Option<LuaTable>>(bp.to_lowercase())?.and_then(|b| b.raw_get::<Option<LuaTable>>("Physics").ok().flatten());
        let skirt = |k: &str| physics.as_ref().and_then(|p| p.raw_get::<Option<f64>>(k).ok().flatten()).unwrap_or(0.0) as f32;
        Ok((skirt("SkirtSizeX").max(fx), skirt("SkirtSizeZ").max(fz)))
    }

    /// The deposit a blueprint must sit on (`Physics.BuildRestriction`): 1
    /// mass, 2 hydrocarbon (the snapshot's deposit types).
    pub fn deposit_restriction(&self, bp: &str) -> LuaResult<Option<u8>> {
        let bps: LuaTable = self.lua.globals().raw_get("__blueprints")?;
        let rule = bps
            .raw_get::<Option<LuaTable>>(bp.to_lowercase())?
            .and_then(|b| b.raw_get::<Option<LuaTable>>("Physics").ok().flatten())
            .and_then(|p| p.raw_get::<Option<String>>("BuildRestriction").ok().flatten());
        Ok(match rule.as_deref() {
            Some("RULEUBR_OnMassDeposit") => Some(1),
            Some("RULEUBR_OnHydrocarbonDeposit") => Some(2),
            _ => None,
        })
    }

    /// Whether a blueprint can be drag-built (its `DRAGBUILD` category).
    pub fn drag_buildable(&self, bp: &str) -> LuaResult<bool> {
        let bps: LuaTable = self.lua.globals().raw_get("__blueprints")?;
        let Some(categories) = bps.raw_get::<Option<LuaTable>>(bp.to_lowercase())?.and_then(|b| b.raw_get::<Option<LuaTable>>("Categories").ok().flatten()) else { return Ok(false) };
        for c in categories.sequence_values::<LuaValue>() {
            if matches!(c?, LuaValue::String(s) if s.to_str().is_ok_and(|s| s == "DRAGBUILD")) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Loads a FAF `Game.prefs` file (options, key bindings) over the
    /// defaults; call before the interface starts.
    pub fn load_prefs(&self, path: &std::path::Path) -> LuaResult<()> {
        let text = std::fs::read_to_string(path).map_err(|e| LuaError::runtime(format!("{}: {e}", path.display())))?;
        prefs::load_file(&self.lua, &text)
    }

    /// The renderer's world views this frame (their cameras and where they
    /// are), for the views' `Project` and for `UnProject` /
    /// `GetMouseWorldPos`.
    pub fn set_pick_views(&self, views: Vec<world_pick::PickView>) {
        self.lua.set_app_data(world_pick::PickViews(views));
    }

    /// The renderer's cameras this frame, by name, for FAF's camera API
    /// (`GetCamera(name)`: zoom, focus, `SaveSettings`).
    pub fn set_camera_states(&self, cameras: &[CameraState]) -> LuaResult<()> {
        let all = self.lua.create_table_with_capacity(0, cameras.len())?;
        for c in cameras {
            let t = self.lua.create_table_with_capacity(0, 6)?;
            t.raw_set("focus", self.lua.create_sequence_from(c.focus)?)?;
            t.raw_set("zoom", c.zoom)?;
            t.raw_set("min", c.min)?;
            t.raw_set("max", c.max)?;
            t.raw_set("pitch", c.pitch)?;
            t.raw_set("heading", c.heading)?;
            all.raw_set(c.name.as_str(), t)?;
        }
        self.lua.globals().raw_set("__engine_cameras", all)
    }

    /// Camera moves the interface asked for since the last call (FAF's
    /// `SetZoom`, `MoveTo`, `RestoreSettings`, `UIZoomTo`, ...), in order.
    pub fn take_camera_requests(&self) -> LuaResult<Vec<CameraRequest>> {
        let g = self.lua.globals();
        let Some(q) = g.raw_get::<Option<LuaTable>>("__engine_camera_requests")? else { return Ok(Vec::new()) };
        g.raw_set("__engine_camera_requests", LuaNil)?;
        let mut out = Vec::new();
        for r in q.sequence_values::<LuaTable>() {
            let r = r?;
            let focus = match r.raw_get::<Option<LuaTable>>("focus")? {
                Some(f) => Some([f.raw_get::<Option<f32>>(1)?.unwrap_or(0.0), f.raw_get::<Option<f32>>(2)?.unwrap_or(0.0), f.raw_get::<Option<f32>>(3)?.unwrap_or(0.0)]),
                None => None,
            };
            out.push(CameraRequest {
                camera: r.raw_get("camera")?,
                focus,
                zoom: r.raw_get("zoom")?,
                reset: r.raw_get::<Option<bool>>("reset")?.unwrap_or(false),
                wheel: r.raw_get("wheel")?,
                seconds: r.raw_get::<Option<f32>>("seconds")?.unwrap_or(0.0).max(0.0),
                max_zoom_mult: r.raw_get("max_zoom_mult")?,
                track: r.raw_get::<Option<Vec<u32>>>("track")?,
                hold_rotation: r.raw_get::<Option<bool>>("hold_rotation")?.unwrap_or(false),
                revert_rotation: r.raw_get::<Option<bool>>("revert_rotation")?.unwrap_or(false),
                rotation: match (r.raw_get::<Option<f32>>("heading")?, r.raw_get::<Option<f32>>("pitch")?) {
                    (Some(h), Some(p)) => Some((h, p)),
                    _ => None,
                },
            });
        }
        Ok(out)
    }

    /// A game the lobby launched (`CLobby:LaunchGame`): `{Config =
    /// gameConfig, LocalID = peer id}` as it crosses to the sim, taken once.
    pub fn take_launch_request(&self) -> LuaResult<Option<crate::engine::sim::lua_data::LuaData>> {
        let g = self.lua.globals();
        let request: LuaValue = g.raw_get("__engine_launch_request")?;
        if request.is_nil() {
            return Ok(None);
        }
        g.raw_set("__engine_launch_request", LuaNil)?;
        Ok(Some(crate::engine::sim::lua_data::LuaData::from_lua(&request)))
    }

    /// A recording the Replay dialog launched (`LaunchReplaySession`): its
    /// file, taken once.
    pub fn take_replay_request(&self) -> LuaResult<Option<std::path::PathBuf>> {
        let g = self.lua.globals();
        let request: Option<String> = g.raw_get("__engine_launch_replay")?;
        g.raw_set("__engine_launch_replay", LuaNil)?;
        Ok(request.map(std::path::PathBuf::from))
    }

    /// The current profile's name (FAF's `Prefs.GetCurrentProfile().Name`),
    /// if there is one.
    pub fn profile_name(&self) -> LuaResult<Option<String>> {
        self.lua
            .load("local current = GetPreference('profile.current') return current and GetPreference('profile.profiles.' .. current .. '.Name')")
            .set_name("=[engine profile name]")
            .eval()
    }

    /// The player's preferences (moho keeps them across its Lua states: the
    /// front end's go on to the game's interface).
    pub fn prefs(&self) -> LuaResult<crate::engine::sim::lua_data::LuaData> {
        Ok(crate::engine::sim::lua_data::LuaData::from_lua(&LuaValue::Table(self.lua.named_registry_value("user.prefs")?)))
    }

    /// Sets the preferences [`Self::prefs`] took, key by key.
    pub fn set_prefs(&self, prefs: &crate::engine::sim::lua_data::LuaData) -> LuaResult<()> {
        let LuaValue::Table(t) = prefs.to_lua(&self.lua)? else { return Ok(()) };
        let set: LuaFunction = self.lua.globals().get("SetPreference")?;
        for pair in t.pairs::<String, LuaValue>() {
            let (k, v) = pair?;
            set.call::<()>((k, v))?;
        }
        Ok(())
    }

    /// The interface asked to quit (FAF's `ExitApplication`: the main
    /// menu's Exit).
    pub fn exit_requested(&self) -> LuaResult<bool> {
        Ok(self.lua.globals().raw_get::<Option<bool>>("__engine_exit_request")?.unwrap_or(false))
    }

    /// A pause / resume the interface asked for (FAF's pause button:
    /// `SessionRequestPause` / `SessionResume`), taken once.
    /// The game speed asked for and the actual one (`GetSimRate`), both moho
    /// steps: a changed step
    /// tells FAF (`uimain.NoteGameSpeedChanged(client, step)`, `by`: the
    /// session client who changed it, 1-based).
    pub fn set_game_speed(&self, step: i32, rate: f32, by: u32) -> LuaResult<()> {
        let g = self.lua.globals();
        let old: Option<i32> = g.raw_get("__engine_game_speed")?;
        g.raw_set("__engine_game_speed", step)?;
        g.raw_set("__engine_sim_rate", rate)?;
        // A change, or a game that starts away from normal speed (FAF's
        // clock starts at +0 and only hears changes).
        if old.map_or(step != 0, |o| o != step) {
            let import: LuaFunction = g.get("import")?;
            let uimain: LuaTable = import.call("/lua/ui/uimain.lua")?;
            if let Some(f) = uimain.get::<Option<LuaFunction>>("NoteGameSpeedChanged")?
                && let Err(e) = f.call::<()>((by, step)) {
                tracing::warn!(target: "lua", "NoteGameSpeedChanged failed: {e}");
            }
        }
        Ok(())
    }

    /// A game speed the interface asked for (`SetGameSpeed`), as a step.
    pub fn take_speed_request(&self) -> LuaResult<Option<i32>> {
        let g = self.lua.globals();
        let v: Option<i32> = g.raw_get("__engine_speed_request")?;
        if v.is_some() {
            g.raw_set("__engine_speed_request", LuaNil)?;
        }
        Ok(v)
    }

    pub fn take_pause_request(&self) -> LuaResult<Option<bool>> {
        let g = self.lua.globals();
        let v: Option<bool> = g.raw_get("__engine_pause_request")?;
        if v.is_some() {
            g.raw_set("__engine_pause_request", LuaNil)?;
        }
        Ok(v)
    }

    /// The sim ended the game (`EndGame`): `SessionIsGameOver` turns true
    /// and FAF's `uimain.NoteGameOver` runs, once (MohoEngine.dll
    /// `CWldSession::DoBeat` 0x10457bda, after the beat's Sync).
    pub fn set_game_over(&self) -> LuaResult<()> {
        let g = self.lua.globals();
        if g.raw_get::<Option<bool>>("__engine_game_over_noted")?.unwrap_or(false) {
            return Ok(());
        }
        g.raw_set("__engine_game_over_noted", true)?;
        g.raw_set("__engine_game_over", true)?;
        let import: LuaFunction = g.get("import")?;
        let uimain: LuaTable = import.call("/lua/ui/uimain.lua")?;
        if let Some(f) = uimain.get::<Option<LuaFunction>>("NoteGameOver")?
            && let Err(e) = f.call::<()>(()) {
            tracing::error!(target: "lua", "Error running '/lua/ui/uimain.lua:NoteGameOver': {e}");
        }
        Ok(())
    }

    /// The player left the session (`SessionEndGame`), taken once.
    pub fn take_session_left(&self) -> LuaResult<bool> {
        let g = self.lua.globals();
        let left = g.raw_get::<Option<bool>>("__engine_session_left")?.unwrap_or(false);
        if left {
            g.raw_set("__engine_session_left", LuaNil)?;
        }
        Ok(left)
    }

    /// The interface asked for its preferences to be written
    /// (`SavePreferences`: the options dialog's OK, a key binding change),
    /// taken once.
    pub fn take_save_request(&self) -> LuaResult<bool> {
        let g = self.lua.globals();
        let asked = g.raw_get::<Option<bool>>("__engine_save_prefs")?.unwrap_or(false);
        if asked {
            g.raw_set("__engine_save_prefs", LuaNil)?;
        }
        Ok(asked)
    }

    /// The recording being watched, for the replay timeline
    /// (`Replay.GetEvents`); `None`: not a replay.
    pub fn set_replay_timeline(&self, timeline: Option<std::sync::Arc<crate::engine::replay::Timeline>>) -> LuaResult<()> {
        self.lua.set_app_data(replay::ReplayTimeline(timeline));
        Ok(())
    }

    /// The watched replay's keyframes (`Replay.GetKeyframes`,
    /// `GetBuffered`, `GetPrecompute`).
    pub fn set_replay_keyframes(&self, keyframes: replay::ReplayKeyframes) {
        self.lua.set_app_data(keyframes);
    }

    /// The player's keyframe options (the Options screen's Interface tab:
    /// `replay_play_ahead`, `replay_keyframe_every` seconds,
    /// `replay_keyframe_budget` MB), defaults for any not set.
    pub fn keyframe_settings(&self) -> LuaResult<crate::engine::keyframe::recorder::Settings> {
        let (ahead, every, budget): (Option<LuaValue>, Option<f64>, Option<f64>) = self
            .lua
            .load("local ok, prefs = pcall(import, '/lua/user/prefs.lua') local o = ok and prefs.GetFromCurrentProfile('options') or {} return o.replay_play_ahead, o.replay_keyframe_every, o.replay_keyframe_budget")
            .set_name("=[keyframe options]")
            .eval()?;
        let d = crate::engine::keyframe::recorder::Settings::default();
        Ok(crate::engine::keyframe::recorder::Settings {
            precompute: match ahead {
                Some(LuaValue::Integer(0)) | Some(LuaValue::Boolean(false)) => false,
                Some(LuaValue::Number(0.0)) => false,
                _ => d.precompute,
            },
            every: every.map_or(d.every, |s| (s * 10.0) as u64),
            budget_mb: budget.map_or(d.budget_mb, |m| m as u64),
        })
    }

    /// A seek the interface asked for (`Replay.Seek`), taken once.
    pub fn take_seek_request(&self) -> Option<replay::SeekRequest> {
        self.lua.remove_app_data::<replay::SeekRequest>()
    }

    /// The tick a seek under way goes to (`Replay.GetSeekTarget`).
    pub fn set_seeking(&self, target: Option<u64>) {
        self.lua.set_app_data(replay::Seeking(target));
    }

    /// The interface asked to start the session again (`RestartSession`),
    /// taken once.
    pub fn take_restart_request(&self) -> LuaResult<bool> {
        let g = self.lua.globals();
        let asked = g.raw_get::<Option<bool>>("__engine_restart_request")?.unwrap_or(false);
        if asked {
            g.raw_set("__engine_restart_request", LuaNil)?;
        }
        Ok(asked)
    }

    /// The interface asked to return to the main menu (FAF's `ExitGame`:
    /// the score screen's Continue), taken once.
    pub fn take_menu_request(&self) -> LuaResult<bool> {
        let g = self.lua.globals();
        let asked = g.raw_get::<Option<bool>>("__engine_menu_request")?.unwrap_or(false);
        if asked {
            g.raw_set("__engine_menu_request", LuaNil)?;
        }
        Ok(asked)
    }

    /// The engine paused or resumed (whoever asked): FAF's `OnPause(by,
    /// timeoutsRemaining)` / `OnResume()` (pause overlay, sounds) and
    /// `OnUserPause(pause)` (`/lua/ui/game/gamemain.lua`).
    pub fn set_paused(&self, paused: bool) -> LuaResult<()> {
        let g = self.lua.globals();
        if g.raw_get::<Option<bool>>("__engine_paused")? == Some(paused) {
            return Ok(());
        }
        g.raw_set("__engine_paused", paused)?;
        let import: LuaFunction = g.get("import")?;
        let main: LuaTable = import.call("/lua/ui/game/gamemain.lua")?;
        if paused {
            if let Some(f) = main.get::<Option<LuaFunction>>("OnPause")? {
                f.call::<()>((0, 3))?;
            }
        } else if let Some(f) = main.get::<Option<LuaFunction>>("OnResume")? {
            f.call::<()>(())?;
        }
        if let Some(f) = main.get::<Option<LuaFunction>>("OnUserPause")? {
            f.call::<()>(paused)?;
        }
        Ok(())
    }

    /// Shift went down / up: FAF's command graph shows queued orders,
    /// reclaim labels and pings while it's held (`OnCommandGraphShow`).
    pub fn command_graph_show(&self, show: bool) -> LuaResult<()> {
        let import: LuaFunction = self.lua.globals().get("import")?;
        let graph: LuaTable = import.call("/lua/ui/game/commandgraph.lua")?;
        if let Some(f) = graph.get::<Option<LuaFunction>>("OnCommandGraphShow")? {
            f.call::<()>(show)?;
        }
        Ok(())
    }

    /// A waypoint drag began in the world view: FAF's
    /// `commandgraph.OnCommandDragBegin()`.
    pub fn command_drag_begin(&self) -> LuaResult<()> {
        let import: LuaFunction = self.lua.globals().get("import")?;
        let graph: LuaTable = import.call("/lua/ui/game/commandgraph.lua")?;
        if let Some(f) = graph.get::<Option<LuaFunction>>("OnCommandDragBegin")? {
            f.call::<()>(())?;
        }
        Ok(())
    }

    /// The drag ended with the button let go at interface point `(x, y)`:
    /// `commandgraph.OnCommandDragEnd(event, cmdId)` (the event's
    /// `MouseX` / `MouseY`, which its disabled trash button reads).
    pub fn command_drag_end(&self, x: f32, y: f32, command: u32) -> LuaResult<()> {
        let import: LuaFunction = self.lua.globals().get("import")?;
        let graph: LuaTable = import.call("/lua/ui/game/commandgraph.lua")?;
        if let Some(f) = graph.get::<Option<LuaFunction>>("OnCommandDragEnd")? {
            let event = self.lua.create_table_with_capacity(0, 3)?;
            event.raw_set("Type", "ButtonRelease")?;
            event.raw_set("MouseX", x)?;
            event.raw_set("MouseY", y)?;
            f.call::<()>((event, command))?;
        }
        Ok(())
    }

    /// The window changed size: FAF's `OnApplicationResize(head, width,
    /// height)` (`/lua/ui/uimain.lua`: deselects, for the construction
    /// panel's sake).
    pub fn application_resized(&self, width: f32, height: f32) -> LuaResult<()> {
        let import: LuaFunction = self.lua.globals().get("import")?;
        let main: LuaTable = import.call("/lua/ui/uimain.lua")?;
        if let Some(f) = main.get::<Option<LuaFunction>>("OnApplicationResize")? {
            f.call::<()>((1, width, height))?;
        }
        Ok(())
    }

    /// FAF's `CommandType` name for an order kind (`UserCommand`).
    pub fn command_type_name(kind: crate::engine::sim::command::CommandKind) -> &'static str {
        use crate::engine::sim::command::CommandKind as K;
        match kind {
            K::Stop => "Stop",
            K::Move => "Move",
            K::FormMove => "FormMove",
            K::Attack => "Attack",
            K::FormAttack => "FormAttack",
            K::AggressiveMove => "AggressiveMove",
            K::FormAggressiveMove => "FormAggressiveMove",
            K::Patrol => "Patrol",
            K::FormPatrol => "FormPatrol",
            K::Guard => "Guard",
            K::Reclaim => "Reclaim",
            K::Repair => "Repair",
            K::Capture => "Capture",
            K::Nuke => "Nuke",
            K::Tactical => "Tactical",
            K::Teleport => "Teleport",
            K::Ferry => "Ferry",
            K::Dive => "Dive",
            K::OverCharge => "OverCharge",
            K::BuildFactory => "BuildFactory",
            K::BuildMobile => "BuildMobile",
            K::Upgrade => "Upgrade",
            K::TransportLoadUnits => "TransportLoadUnits",
            K::TransportReverseLoadUnits => "TransportReverseLoadUnits",
            K::TransportUnloadUnits => "TransportUnloadUnits",
            K::TransportUnloadSpecificUnits => "TransportUnloadSpecificUnits",
            K::DetachFromTransport => "DetachFromTransport",
            K::BuildSiloTactical => "BuildSiloTactical",
            K::BuildSiloNuke => "BuildSiloNuke",
            K::Script => "Script",
            K::SpecialAction => "SpecialAction",
            _ => "None",
        }
    }

    /// Tells the interface about an order the player gave (moho's
    /// `OnCommandIssued(command)`, FAF's `commandmode.lua`: ends command
    /// mode unless shift-queued, per-type follow-ups, command feedback).
    pub fn command_issued(&self, kind: crate::engine::sim::command::CommandKind, target: crate::engine::sim::command::CommandTarget, queue: bool, blueprint: Option<&str>, units: &[u32], script: &[(String, String)]) -> LuaResult<()> {
        use crate::engine::sim::command::CommandTarget as T;
        let cmd = self.lua.create_table()?;
        cmd.set("CommandType", Self::command_type_name(kind))?;
        cmd.set("Clear", !queue)?;
        let get: Option<LuaFunction> = self.lua.globals().get("GetUnitById")?;
        let target_unit = match (target, &get) {
            (T::Entity(id), Some(get)) => get.call::<Option<LuaTable>>(id.to_string())?,
            _ => None,
        };
        // "" when the order has none, as moho sends it (0x10404850). A
        // guard names the guarded unit's blueprint (FAF probe
        // probe_guardcmd): FAF's OnGuardIssued reads it to find factories
        // (ValidateAssist) and structures to cap (hotkeys/capping.lua:
        // engineers assisting a T2/T3 extractor ring it with storages).
        match (blueprint, kind, &target_unit) {
            (Some(bp), _, _) => cmd.set("Blueprint", bp)?,
            (None, crate::engine::sim::command::CommandKind::Guard, Some(unit)) => {
                let bp: LuaTable = unit.call_method("GetBlueprint", ())?;
                cmd.set("Blueprint", bp.get::<LuaValue>("BlueprintId")?)?;
            }
            _ => cmd.set("Blueprint", "")?,
        }
        if !script.is_empty() {
            let params = self.lua.create_table_with_capacity(0, script.len())?;
            for (k, v) in script {
                params.set(k.as_str(), v.as_str())?;
            }
            cmd.set("LuaParams", params)?;
        }
        let t = self.lua.create_table()?;
        match target {
            T::Ground(p) => {
                let (x, y, z) = p.to_f64();
                t.set("Type", "Position")?;
                t.set("Position", self.lua.create_sequence_from([x, y, z])?)?;
            }
            T::Entity(id) => {
                t.set("Type", "Entity")?;
                t.set("EntityId", id.to_string())?;
                // With the entity's position, as moho gives it.
                if let Some(unit) = &target_unit {
                    t.set("Position", unit.call_method::<LuaValue>("GetPosition", ())?)?;
                }
            }
            T::None => t.set("Type", "None")?,
        }
        cmd.set("Target", t)?;
        let list = self.lua.create_table_with_capacity(units.len(), 0)?;
        for &u in units {
            if let Some(get) = &get
                && let Some(obj) = get.call::<Option<LuaTable>>(u.to_string())? {
                list.push(obj)?;
            }
        }
        cmd.set("Units", list)?;
        // A script order (an enhancement) is in its units' queues before
        // FAF hears of it, as moho's are (units.lua).
        if kind == crate::engine::sim::command::CommandKind::Script {
            if let Some(f) = self.lua.globals().raw_get::<Option<LuaFunction>>("__engine_predict_command")? {
                f.call::<()>((units, "Script", !queue))?;
            }
            let now = self.time();
            let mut predicted = self.lua.remove_app_data::<PredictedOrders>().unwrap_or_default();
            predicted.0.extend(units.iter().map(|&u| (u, now)));
            self.lua.set_app_data(predicted);
        }
        let import: LuaFunction = self.lua.globals().get("import")?;
        let cm: LuaTable = import.call("/lua/ui/game/commandmode.lua")?;
        if let Some(f) = cm.get::<Option<LuaFunction>>("OnCommandIssued")? {
            f.call::<()>(cmd)?;
        }
        Ok(())
    }

    /// Ends FAF's command mode (`isCancel`: the player cancelled it).
    pub fn end_command_mode(&self, cancel: bool) -> LuaResult<()> {
        let import: LuaFunction = self.lua.globals().get("import")?;
        let cm: LuaTable = import.call("/lua/ui/game/commandmode.lua")?;
        if let Some(f) = cm.get::<Option<LuaFunction>>("EndCommandMode")? {
            f.call::<()>(cancel)?;
        }
        Ok(())
    }

    /// One beat's `Sync` from the sim: `PreviousSync` becomes the old one,
    /// `Sync` this copy, then FAF's `OnSync` (`/lua/UserSync.lua`) runs.
    pub fn sync(&self, value: &crate::engine::sync::SyncValue) -> LuaResult<()> {
        let g = self.lua.globals();
        let old: LuaValue = g.raw_get("Sync")?;
        g.raw_set("PreviousSync", old)?;
        g.raw_set("Sync", value.to_lua(&self.lua)?)?;
        if let Some(f) = g.raw_get::<Option<LuaFunction>>("OnSync")? {
            f.call::<()>(())?;
        }
        Ok(())
    }

    /// The army the UI shows as (`--player`; observers: -1).
    pub fn set_focus_army(&self, army: i64) -> LuaResult<()> {
        self.lua.set_named_registry_value("user.focus_army", army)
    }

    /// The army the UI shows as, if any (observers: `None`): moho's sound
    /// listener army.
    pub fn focus_army(&self) -> Option<u32> {
        let army = self.lua.named_registry_value::<Option<i64>>("user.focus_army").ok().flatten()?;
        u32::try_from(army).ok().filter(|&a| a > 0)
    }

    /// Head 0's screen size in pixels (its root frame spans it).
    pub fn set_screen(&self, w: f32, h: f32) -> LuaResult<()> {
        controls::set_screen(&self.lua, 0, w, h)
    }

    /// Head `head`'s screen size in pixels.
    pub fn set_head_screen(&self, head: usize, w: f32, h: f32) -> LuaResult<()> {
        controls::set_screen(&self.lua, head, w, h)
    }

    /// The number of heads (`GetNumRootFrames`), set before the interface
    /// starts (FAF builds the second head's view as it starts).
    pub fn set_heads(&self, n: usize) {
        controls::set_heads(&self.lua, n);
    }

    /// The head the mouse is on: mouse events and hit tests from here on
    /// are in its screen's pixels, over its root frame's controls.
    pub fn set_mouse_head(&self, head: usize) {
        controls::set_mouse_head(&self.lua, head);
    }

    /// One UI frame: user threads, then `OnFrame` of controls that want it.
    pub fn frame(&self, dt: f32) -> LuaResult<()> {
        let t: f64 = self.lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0);
        self.lua.set_named_registry_value("user.time", t + dt as f64)?;
        crate::engine::disk_watch::deliver(&self.lua, |lua| {
            api::blueprints::apply_defaults(lua, true)?;
            api::categories::build(lua)
        })?;
        api::threads::run_tick(&self.lua)?;
        self.lua.set_app_data(world_draw::FrameDelta(dt));
        controls::on_frame(&self.lua, dt)?;
        movie::tick(&self.lua, dt)?;
        cursor::update(&self.lua)
    }

    /// The cursor FAF wants (the renderer makes it the window's cursor).
    pub fn cursor(&self) -> cursor::Wanted {
        cursor::wanted(&self.lua)
    }

    /// The order a right click would give at the mouse (`RULEUCC_Attack`,
    /// ...; `None` over nothing to act on), for FAF's cursor.
    /// Whether a right click on the hovered waypoint would turn the
    /// selection's moves into a patrol (`ShowConvertToPatrolCursor`).
    pub fn set_convert_to_patrol(&self, on: bool) -> LuaResult<()> {
        self.lua.globals().raw_set("__engine_convert_to_patrol", on)
    }

    pub fn set_mouse_order(&self, kind: Option<crate::engine::sim::command::CommandKind>) -> LuaResult<()> {
        self.lua.globals().raw_set("__engine_mouse_order", Self::mouse_order_name(kind))
    }

    /// The `__engine_mouse_order` for a right-click order: FAF's cursor
    /// follows moho's `GetRightMouseButtonOrder`, which answers the caps,
    /// not the orders (`engine/User/CUIWorldView.lua`: only the 8 contract
    /// names plus nil, e.g. `RULEUCC_CallTransport` hovering a transport
    /// with loadable units selected, `RULEUCC_Transport` hovering a drop
    /// target with transports selected). Anything outside the contract
    /// reads as nil, so the cursor falls back to default. Pure, so the
    /// contract is unit-tested.
    pub fn mouse_order_name(kind: Option<crate::engine::sim::command::CommandKind>) -> Option<String> {
        use crate::engine::sim::command::CommandKind as K;
        match kind {
            Some(K::TransportLoadUnits | K::TransportReverseLoadUnits) => Some("RULEUCC_CallTransport".to_string()),
            Some(K::TransportUnloadUnits | K::TransportUnloadSpecificUnits) => Some("RULEUCC_Transport".to_string()),
            Some(K::Move) => Some("RULEUCC_Move".to_string()),
            Some(K::Attack) => Some("RULEUCC_Attack".to_string()),
            Some(K::Guard) => Some("RULEUCC_Guard".to_string()),
            Some(K::Reclaim) => Some("RULEUCC_Reclaim".to_string()),
            Some(K::Repair) => Some("RULEUCC_Repair".to_string()),
            Some(K::Capture) => Some("RULEUCC_Capture".to_string()),
            _ => None,
        }
    }

    /// The active range overlay filters when they've changed since the last
    /// call (`None`: unchanged), for the renderer's `blueprints` order.
    pub fn take_overlay_filters(&self, blueprints: &[String]) -> LuaResult<Option<Vec<OverlayFilter>>> {
        let g = self.lua.globals();
        let version: i64 = g.get::<LuaFunction>("__engine_overlay_version")?.call(())?;
        let key = (version, blueprints.len() as i64);
        let last: Option<(i64, i64)> = self.lua.named_registry_value::<Option<LuaTable>>("ui.overlay_seen")?.map(|t| (t.raw_get(1).unwrap_or(-1), t.raw_get(2).unwrap_or(-1)));
        if last == Some(key) {
            return Ok(None);
        }
        self.lua.set_named_registry_value("ui.overlay_seen", self.lua.create_sequence_from([key.0, key.1])?)?;
        let ids = self.lua.create_sequence_from(blueprints.iter().map(String::as_str))?;
        let kinds = self.lua.create_sequence_from(crate::engine::snapshot::RANGE_KINDS)?;
        let list: LuaTable = g.get::<LuaFunction>("__engine_overlay_filters")?.call((ids, kinds))?;
        let color = |v: Option<String>| overlay_color(&v.unwrap_or_default());
        let pair = |t: Option<LuaTable>| -> [f32; 2] { t.map_or([0.0; 2], |t| [t.raw_get::<Option<f32>>(1).ok().flatten().unwrap_or(0.0), t.raw_get::<Option<f32>>(2).ok().flatten().unwrap_or(0.0)]) };
        let mut out = Vec::new();
        for f in list.sequence_values::<LuaTable>() {
            let f = f?;
            let members: LuaTable = f.raw_get("members")?;
            out.push(OverlayFilter {
                name: f.raw_get("name")?,
                colors: [color(f.raw_get("normal")?), color(f.raw_get("select")?), color(f.raw_get("rollover")?)],
                inner: pair(f.raw_get("inner")?),
                outer: pair(f.raw_get("outer")?),
                kinds: f.raw_get::<f64>("kinds")? as u16,
                members: (1..=blueprints.len()).map(|i| members.raw_get::<Option<bool>>(i).ok().flatten().unwrap_or(false)).collect(),
            });
        }
        Ok(Some(out))
    }

    /// Each range kind's style (by `snapshot::RANGE_KINDS`), from FAF's
    /// `rangeoverlayparams.lua`: the overlay that shows that kind.
    pub fn range_styles(&self) -> LuaResult<Vec<Option<RangeStyle>>> {
        // Kind -> FAF's overlay for it.
        const OVERLAYS: [&str; 11] = ["AntiAir", "AntiNavy", "DirectFire", "IndirectFire", "Defense", "Defense", "Miscellaneous", "Radar", "Sonar", "Omni", "CounterIntel"];
        let import: LuaFunction = self.lua.globals().get("import")?;
        let params: LuaTable = import.call::<LuaTable>("/lua/ui/game/rangeoverlayparams.lua")?.get("RangeOverlayParams")?;
        let pair = |t: Option<LuaTable>| -> [f32; 2] { t.map_or([0.0; 2], |t| [t.raw_get::<Option<f32>>(1).ok().flatten().unwrap_or(0.0), t.raw_get::<Option<f32>>(2).ok().flatten().unwrap_or(0.0)]) };
        OVERLAYS
            .iter()
            .map(|name| {
                let Some(p) = params.get::<Option<LuaTable>>(*name)? else { return Ok(None) };
                let color = |k: &str| -> LuaResult<[f32; 4]> { Ok(overlay_color(&p.get::<Option<String>>(k)?.unwrap_or_default())) };
                Ok(Some(RangeStyle { colors: [color("NormalColor")?, color("SelectColor")?, color("RolloverColor")?], inner: pair(p.get("Inner")?), outer: pair(p.get("Outer")?) }))
            })
            .collect()
    }

    /// Modifier keys held (FAF's `IsKeyDown('Shift')`, its Alt / Ctrl
    /// cursor and order variants).
    pub fn set_modifiers(&self, shift: bool, ctrl: bool, alt: bool) -> LuaResult<()> {
        let g = self.lua.globals();
        g.raw_set("__engine_shift", shift)?;
        g.raw_set("__engine_ctrl", ctrl)?;
        g.raw_set("__engine_alt", alt)
    }

    /// A mouse event; true if the interface took it (else it's the world's).
    pub fn mouse(&self, ev: controls::MouseEvent, x: f32, y: f32, mods: controls::Modifiers) -> LuaResult<bool> {
        controls::mouse(&self.lua, ev, x, y, mods)
    }

    /// A key press named FAF's way (`"S"`, `"F1"`, `"Esc"`, `"UpArrow"`,
    /// ...) with its modifiers: a `KeyDown` for the control with the
    /// keyboard focus, then (unless that blocks) the action FAF's key map binds to it
    /// (`UI_Lua <code>` runs in this VM; console commands are logged, not
    /// run). True if something was bound.
    pub fn key(&self, key: &str, ctrl: bool, alt: bool, shift: bool) -> LuaResult<bool> {
        // Engine hotkeys come before everything (an edit with the focus
        // included): Ctrl+Shift+P (Cmd on the Mac) toggles the ping grid.
        if ctrl && shift && !alt && key.eq_ignore_ascii_case("p") {
            let toggle: LuaFunction = self.lua.globals().get("__engine_toggle_ping_grid")?;
            if let Err(e) = toggle.call::<()>(()) {
                tracing::warn!(target: "lua", "ping grid: {e}");
            }
            return Ok(true);
        }
        // The control with the keyboard focus sees it first (the key
        // bindings dialog's "press a key"); one that blocks keeps it.
        let held = controls::Modifiers { ctrl, alt, shift, ..Default::default() };
        if controls::key_down(&self.lua, key, held, false)? == Some(true) {
            return Ok(true);
        }
        // Enter opens the chat (moho handles it specially, not through the
        // key map: FAF's `keynames.lua` says so, and nothing in its Lua
        // calls `ActivateChat`), once the game interface has built it.
        if key == "Enter" && !ctrl && !alt {
            let opened: bool = self
                .lua
                .load("local chat = import('/lua/ui/game/chat.lua') if chat.GUI and chat.GUI.bg then chat.ActivateChat({ Shift = ... }) return true end return false")
                .set_name("=[engine chat key]")
                .call(shift)?;
            if opened {
                return Ok(true);
            }
        }
        let mods: Vec<&str> = [("Ctrl", ctrl), ("Alt", alt), ("Shift", shift)].iter().filter(|m| m.1).map(|m| m.0).collect();
        // FAF's maps aren't consistent about modifier order: try them all.
        let mut names = Vec::new();
        let mut perm = mods.clone();
        let mut push = |p: &[&str]| names.push(p.iter().map(|m| format!("{m}-")).collect::<String>() + key);
        match perm.len() {
            0 | 1 => push(&perm),
            2 => {
                push(&perm);
                perm.swap(0, 1);
                push(&perm);
            }
            _ => {
                for p in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
                    push(&[perm[p[0]], perm[p[1]], perm[p[2]]]);
                }
            }
        }
        let find: LuaFunction = self.lua.globals().get("__engine_key_action")?;
        let Some(action) = find.call::<Option<String>>(names.clone())? else { return Ok(false) };
        if let Err(e) = self.console(&action) {
            tracing::warn!(target: "lua", key = %names[0], action, "key action failed: {e}");
        }
        Ok(true)
    }

    /// Does an edit have the keyboard focus (keys type instead of
    /// panning the camera or changing game speed)?
    pub fn typing(&self) -> bool {
        controls::typing(&self.lua)
    }

    /// A held key repeating: only an edit with the focus takes it (held
    /// Backspace, arrows). True if one did.
    pub fn key_repeat(&self, key: &str, ctrl: bool, alt: bool, shift: bool) -> LuaResult<bool> {
        let held = controls::Modifiers { ctrl, alt, shift, ..Default::default() };
        Ok(controls::key_down(&self.lua, key, held, true)?.is_some())
    }

    /// Characters typed (the window's text for a key press), for the edit
    /// with the focus. True if one took them.
    pub fn text(&self, text: &str) -> LuaResult<bool> {
        let mut took = false;
        for ch in text.chars() {
            took |= controls::edit_char(&self.lua, ch)?;
        }
        Ok(took)
    }

    /// Runs a console command (a key map action, `ConExecute`): the
    /// interface's own here; the engine's (`UI_SelectByCategory`,
    /// `UI_TrackUnit`, sim rate, ...) are queued for the renderer
    /// ([`UserVm::take_console`]).
    pub fn console(&self, line: &str) -> LuaResult<()> {
        console(&self.lua, line)
    }

    /// Console commands for the engine (renderer) since the last call.
    pub fn take_console(&self) -> LuaResult<Vec<String>> {
        let g = self.lua.globals();
        let Some(q) = g.raw_get::<Option<LuaTable>>("__engine_console")? else { return Ok(Vec::new()) };
        g.raw_set("__engine_console", LuaNil)?;
        q.sequence_values::<String>().collect()
    }

    /// Of `units` (entity id, blueprint id), those in the category
    /// expression (space-separated names, intersected; `ALLUNITS` any).
    pub fn filter_category(&self, units: &[(u32, &str)], expr: &str) -> LuaResult<Vec<u32>> {
        let list = self.lua.create_table_with_capacity(units.len() * 2, 0)?;
        for &(id, bp) in units {
            list.push(id)?;
            list.push(bp)?;
        }
        let f: LuaFunction = self.lua.globals().get("__engine_filter_category")?;
        f.call::<Vec<u32>>((list, expr))
    }

    /// Units the interface selected (`SelectUnits`) since the last call.
    pub fn take_selection_request(&self) -> LuaResult<Option<Vec<u32>>> {
        let g = self.lua.globals();
        let Some(t) = g.raw_get::<Option<LuaTable>>("__engine_select_request")? else { return Ok(None) };
        g.raw_set("__engine_select_request", LuaNil)?;
        let mut ids: Vec<u32> = t.sequence_values::<LuaTable>().filter_map(|u| u.ok()?.raw_get::<Option<u32>>("_id").ok().flatten()).collect();
        ids.sort_unstable();
        ids.dedup();
        Ok(Some(ids))
    }

    /// Is (x, y) over the interface (a control other than a world view)?
    pub fn hit_interface(&self, x: f32, y: f32) -> LuaResult<bool> {
        controls::over_interface(&self.lua, x, y)
    }

    /// Whether the minimap is the topmost control at (x, y).
    pub fn hit_minimap(&self, x: f32, y: f32) -> LuaResult<bool> {
        controls::over_minimap(&self.lua, x, y)
    }

    /// Seconds of UI frames run so far (the user layer's clock).
    pub fn time(&self) -> f64 {
        self.lua.named_registry_value::<Option<f64>>("user.time").ok().flatten().unwrap_or(0.0)
    }

    /// What to draw this frame, back to front.
    pub fn draw_list(&self) -> LuaResult<Vec<controls::DrawItem>> {
        controls::draw_list(&self.lua, 0)
    }

    /// What head `head` draws this frame, back to front.
    pub fn head_draw_list(&self, head: usize) -> LuaResult<Vec<controls::DrawItem>> {
        controls::draw_list(&self.lua, head)
    }

    /// Controls in the tree, and how many of them are hidden (tests,
    /// profiling).
    pub fn control_count(&self) -> (usize, usize) {
        let c = controls::controls(&self.lua);
        (c.map.len(), c.map.values().filter(|c| c.hidden).count())
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}

/// Runs a console command line in the user layer (see [`UserVm::console`]);
/// FAF's `ConExecute` comes here too.
pub fn console(lua: &Lua, line: &str) -> LuaResult<()> {
    let line = line.trim();
    let (cmd, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, r)| (c, r.trim()));
    let import = || -> LuaResult<LuaFunction> { lua.globals().get("import") };
    match cmd.to_ascii_lowercase().as_str() {
        "ui_lua" => {
            let code = crate::engine::lua_translation::translate(rest);
            lua.load(code).set_name("=[console]").exec()?;
        }
        // Alt-F2 (`debug_create_unit`): FAF's spawn menu, opened or closed
        // (MohoEngine.dll imports `/lua/ui/dialogs/createunit.lua`; the sim
        // checks cheats when a spawn arrives).
        "popupcreateunitmenu" => {
            let dialog: LuaTable = import()?.call("/lua/ui/dialogs/createunit.lua")?;
            dialog.get::<LuaFunction>("CreateDialog")?.call::<()>(())?;
        }
        // `StartCommandMode order RULEUCC_Attack` / `build ueb0101`.
        "startcommandmode" => {
            let mut words = rest.split_whitespace();
            let (mode, name) = (words.next().unwrap_or("order"), words.next().unwrap_or(""));
            let cm: LuaTable = import()?.call("/lua/ui/game/commandmode.lua")?;
            let data = lua.create_table()?;
            data.set("name", name)?;
            cm.get::<LuaFunction>("StartCommandMode")?.call::<()>((mode, data))?;
        }
        // Control groups live in FAF's selection.lua.
        "ui_makeselectionset" => {
            let sel: LuaTable = import()?.call("/lua/ui/game/selection.lua")?;
            sel.get::<LuaFunction>("AddCurrentSelectionSet")?.call::<()>(rest)?;
        }
        "ui_applyselectionset" => {
            let sel: LuaTable = import()?.call("/lua/ui/game/selection.lua")?;
            sel.get::<LuaFunction>("ApplySelectionSet")?.call::<()>(rest)?;
        }
        _ => {
            let queue: LuaTable = match lua.globals().raw_get::<Option<LuaTable>>("__engine_console")? {
                Some(q) => q,
                None => {
                    let q = lua.create_table()?;
                    lua.globals().raw_set("__engine_console", q.clone())?;
                    q
                }
            };
            queue.push(line)?;
        }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::sim::command::CommandKind as K;

    #[test]
    fn transport_order_names_round_trip() {
        for (name, kind) in [
            ("UNITCOMMAND_TransportLoadUnits", K::TransportLoadUnits),
            ("UNITCOMMAND_TransportReverseLoadUnits", K::TransportReverseLoadUnits),
            ("UNITCOMMAND_TransportUnloadUnits", K::TransportUnloadUnits),
            ("UNITCOMMAND_TransportUnloadSpecificUnits", K::TransportUnloadSpecificUnits),
            ("UNITCOMMAND_DetachFromTransport", K::DetachFromTransport),
            ("RULEUCC_Ferry", K::Ferry),
            // The Unload button arms `RULEUCC_Transport`; its click unloads.
            ("RULEUCC_Transport", K::TransportUnloadUnits),
            ("UNITCOMMAND_Transport", K::TransportUnloadUnits),
        ] {
            assert_eq!(command_kind(name), Some(kind), "{name}");
        }
        assert_eq!(command_kind("UNITCOMMAND_Ferry"), Some(K::Ferry));
        assert!(command_kind("UNITCOMMAND_Dock").is_none());
    }

    /// The hover cursor only ever names caps moho answers with
    /// (`engine/User/CUIWorldView.lua` `GetRightMouseButtonOrder`, 8 names
    /// plus nil): load orders read as `RULEUCC_CallTransport`, unload
    /// orders as `RULEUCC_Transport`, anything else falls back to nil.
    #[test]
    fn mouse_order_names_stay_inside_the_cursor_contract() {
        use std::collections::BTreeSet;
        let contract: BTreeSet<&str> = [
            "RULEUCC_Reclaim", "RULEUCC_Attack", "RULEUCC_Move", "RULEUCC_Guard", "RULEUCC_Repair", "RULEUCC_Capture", "RULEUCC_CallTransport",
            "RULEUCC_Transport",
        ]
        .into_iter()
        .collect();
        for kind in [K::Move, K::Attack, K::Guard, K::Reclaim, K::Repair, K::Capture, K::TransportLoadUnits, K::TransportReverseLoadUnits, K::TransportUnloadUnits, K::TransportUnloadSpecificUnits] {
            let name = UserVm::mouse_order_name(Some(kind)).expect("a cursor");
            assert!(contract.contains(name.as_str()), "{kind:?} -> {name}");
        }
        assert_eq!(UserVm::mouse_order_name(Some(K::TransportLoadUnits)), UserVm::mouse_order_name(Some(K::TransportReverseLoadUnits)));
        assert_eq!(UserVm::mouse_order_name(Some(K::TransportUnloadUnits)), UserVm::mouse_order_name(Some(K::TransportUnloadSpecificUnits)));
        for kind in K::ALL {
            if !matches!(kind, K::Move | K::Attack | K::Guard | K::Reclaim | K::Repair | K::Capture | K::TransportLoadUnits | K::TransportReverseLoadUnits | K::TransportUnloadUnits | K::TransportUnloadSpecificUnits) {
                assert_eq!(UserVm::mouse_order_name(Some(kind)), None, "{kind:?} leaves the contract");
            }
        }
        assert_eq!(UserVm::mouse_order_name(None), None);
    }
}
