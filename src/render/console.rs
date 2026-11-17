//! Console commands the interface hands the engine (key map actions that
//! aren't `UI_Lua`): unit selection by category, camera tracking, sim rate.
//! The interface's own commands run in the user layer (`UserVm::console`).

use rts_core_engine::engine::snapshot::ViewKind;

use crate::render::camera::SCALE_LOD;
use crate::render::world::World;
use crate::render::state::RenderState;

/// Runs one console command line.
pub fn run(s: &mut RenderState, line: &str) {
    let (cmd, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, r)| (c, r.trim()));
    match cmd.to_ascii_lowercase().as_str() {
        "ui_selectbycategory" => select_by_category(s, rest),
        "ui_expandcurrentselection" => expand_selection(s),
        // Follow the (first) selected unit until the camera is moved.
        "ui_trackunit" => {
            if let Some(&id) = s.player.selection.first() {
                crate::render::frame::track(s, crate::render::camera::MAIN, vec![id]);
            }
        }
        // Moho's steps (rate 10^(step/10); no upper limit, unlike moho).
        "wld_resetsimrate" => s.control.set_speed(100),
        "wld_increasesimrate" => s.control.step_moho(true),
        "wld_decreasesimrate" => s.control.step_moho(false),
        "wld_gamespeed" => {
            use rts_core_engine::engine::snapshot::SimControl;
            if let Ok(step) = rest.parse::<i32>() {
                s.control.set_speed(SimControl::percent_of(step));
            }
        }
        // Camera settings (FAF's options set the zoom speed this way), for
        // every world camera (new ones copy the main one's).
        "cam_zoomamount" | "cam_nearpitch" | "cam_farpitch" | "cam_nearzoom" => {
            let Ok(v) = rest.parse::<f32>() else { return };
            let cmd = cmd.to_ascii_lowercase();
            for (_, cam) in s.world.cameras.iter_mut() {
                match cmd.as_str() {
                    "cam_zoomamount" => cam.zoom_amount = v,
                    "cam_nearpitch" => cam.near_pitch = v,
                    "cam_farpitch" => cam.far_pitch = v,
                    _ => cam.min_zoom = v.max(1.0),
                }
            }
        }
        "cam_setlod" | "cam_defaultlod" | "sc_camerascalelod" => lod(&mut s.world, &cmd.to_ascii_lowercase(), rest),
        // The Video options' Anti-Aliasing (MSAA sample counts in FA): 0
        // off, 1 SMAA. MSAA isn't drawn yet, so a profile's 2 / 4 / 8 (from
        // FA) means SMAA too.
        "sc_antialiasingsamples" => {
            if let Ok(n) = rest.parse::<f32>() {
                s.settings.smaa = n >= 1.0 && !s.no_aa;
            }
        }
        // The Video options' Vertical Sync.
        "sc_verticalsync" => {
            if let Some(on) = console_bool(rest) {
                s.set_vsync(on);
            }
        }
        // The Video options' Frametime: milliseconds a frame takes at least.
        "sc_frametimeclamp" => {
            if let Ok(ms) = rest.parse::<f64>() {
                s.min_frame = std::time::Duration::from_secs_f64(ms.clamp(0.0, 1000.0) / 1000.0);
                let min = s.min_frame;
                s.command_heads(|| crate::render::present::Command::MinFrame(min));
                tracing::info!(ms, "frame time clamp");
            }
        }
        // `cam_Free` ("Allow the camera to remain rotated"; FAF's Ctrl-V
        // sends it bare: a toggle): a Space spin stays when Space is let go,
        // and works at any zoom.
        "cam_free" => {
            s.cam_free = set_or_toggle(s.cam_free, rest);
            tracing::info!(on = s.cam_free, "cam_Free");
        }
        // The Video options' Bloom Render.
        "ren_bloom" => {
            if let Some(on) = console_bool(rest) {
                s.settings.bloom = on;
            }
        }
        // The Video options' Bloom Intensity (its slider / 10).
        "ren_bloomblurkernelscale" => {
            if let Some(v) = rest.parse::<f32>().ok().filter(|v| v.is_finite()) {
                s.settings.bloom_blur = v.clamp(0.0, 10.0);
            }
        }
        // The Video options' Lock Fullscreen Cursor To Window.
        "sc_togglecursorclip" => {
            if let Some(on) = console_bool(rest) {
                s.cursor_clip = on;
                s.apply_cursor_clip();
            }
        }
        // The Video options' Shadow Resolution.
        "ren_shadowsize" => {
            if let Ok(size) = rest.parse::<f32>() {
                s.settings.shadow_size = size.max(0.0) as u32;
            }
        }
        // The Video options' Shadow render distance.
        "ren_shadowlod" => {
            if let Some(v) = rest.parse::<f32>().ok().filter(|v| *v > 0.0) {
                s.settings.shadow_lod = shadow_lod(v);
            }
        }
        // The Video options' Shadow Fidelity (0 off .. 3 high; moho clamps
        // to `shadow_FidelitySupported`, 3).
        "shadow_fidelity" => {
            if let Ok(level) = rest.parse::<f32>() {
                s.settings.shadow_fidelity = level.clamp(0.0, 3.0) as u8;
            }
        }
        // The Video options' Texture Detail (mips skipped: 2 low .. 0 high).
        "ren_mipskiplevels" => {
            if let Ok(n) = rest.parse::<f32>() {
                s.settings.mip_skip = n.clamp(0.0, 4.0) as u32;
            }
        }
        // The engine's own: the Video options' Texture Cache Disk Space (MB)
        // and Texture Cache: Keep Unused (days), for generated mip levels.
        "eng_texturecachemb" => {
            if let Some(mb) = rest.parse::<f32>().ok().filter(|v| *v >= 0.0) {
                super::texture_load::set_mip_cache_budget(mb as u64);
            }
        }
        "eng_texturecachedays" => {
            if let Some(days) = rest.parse::<f32>().ok().filter(|v| *v >= 0.0) {
                super::texture_load::set_mip_cache_max_age(days as u64);
            }
        }
        // The Video options' Primary Adapter (window or fullscreen mode).
        "sc_primaryadapter" => s.set_display_mode(rest),
        // The engine's own: the Primary Display option's choice by number
        // (as its "Display n" labels; 0 or `default`: the system's main
        // display) or key, for scripts and `--ui-lua` (a `--screenshot` run
        // doesn't apply the profile's).
        "eng_primarydisplay" => {
            let arg = rest.trim();
            let key = match arg.parse::<usize>() {
                Ok(0) => crate::render::state::DEFAULT_DISPLAY.to_owned(),
                Ok(n) => match crate::render::state::monitors(&s.window).into_iter().nth(n - 1) {
                    Some((k, _)) => k,
                    None => {
                        tracing::warn!(n, "eng_PrimaryDisplay: no such display");
                        return;
                    }
                },
                Err(_) => arg.to_owned(),
            };
            s.set_primary_display(&key);
        }
        // Its Secondary Adapter (a second world view on another monitor):
        // not supported, so its only choice is Disabled
        // (`list_display_modes`); FAF sends this to refill that list.
        "sc_secondaryadapter" => {}
        // Lifebars: the `Render_Unit_Bars` key action (bare: a toggle), and
        // Interface > Always Show Enemy Lifebars (`0` / `1`; FAF also sends
        // it at game start, `launchconsolecommands.lua`).
        "ui_renderunitbars" => s.settings.render_bars = set_or_toggle(s.settings.render_bars, rest),
        "ui_forcelifbarsonenemy" => s.settings.force_enemy_bars = set_or_toggle(s.settings.force_enemy_bars, rest),
        // Interface > Always Show Strategic Icons (`0` / `1`); the key
        // action `Always_Render_Strategic_Icons` sends it bare, a toggle.
        "ui_alwaysrenderstrategicicons" => {
            s.settings.always_icons = set_or_toggle(s.settings.always_icons, rest);
        }
        // Interface > Scale strategic icons (a FAF binary patch's variable).
        "ui_strategiciconscale" => {
            if let Some(v) = rest.parse::<f32>().ok().filter(|v| v.is_finite()) {
                s.settings.icon_scale = v.clamp(0.25, 4.0);
            }
        }
        // The Video options' Render Sky.
        "ren_skydome" => {
            if let Some(on) = console_bool(rest) {
                s.settings.sky = on;
            }
        }
        // The Video options' Fidelity (FAF's shader tiers).
        "graphics_fidelity" => {
            if let Ok(level) = rest.parse::<f32>() {
                s.settings.fidelity = level.clamp(0.0, 2.0) as u8;
            }
        }
        // FAF's overlay menu (Rollover / Selection): rings of selected and
        // hovered units with no overlay on.
        "range_renderselected" | "range_renderhighlighted" => {
            let Some(on) = console_bool(rest) else { return };
            if cmd.eq_ignore_ascii_case("range_renderselected") {
                s.settings.range_selected = on;
            } else {
                s.settings.range_highlighted = on;
            }
        }
        _ => tracing::debug!(command = line, "console command not supported yet"),
    }
}

/// Options applied at start as the command their setter sends on a change:
/// the Video ones FAF's setters skip at start (moho reads them from the
/// profile when it makes the device), and Interface ones moho's start-up
/// `optionslogic.Apply(true)` sends (strategic icons); the profile key and
/// the command.
const PROFILE_AT_START: &[(&str, &str)] = &[
    ("antialiasing", "SC_AntiAliasingSamples"),
    ("vsync", "SC_VerticalSync"),
    ("frametime", "sc_FrameTimeClamp"),
    ("strat_icons_always_on", "ui_AlwaysRenderStrategicIcons"),
    ("strat_icon_scale", "ui_StrategicIconScale"),
];

/// Interface options moho reads from the profile itself: their FAF setters
/// send no command (SupremeCommander.exe has the key names as strings), so
/// they are read when an interface starts and whenever it saves its
/// preferences (the options dialog's OK).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileOptions {
    /// Show Waypoint ETAs (`display_eta`, default on).
    pub display_eta: bool,
    /// Show Lifebars of Attached Units (`show_attached_unit_lifebars`,
    /// default on; moho reads it as it draws: MohoEngine.dll 0x1042006a).
    pub show_attached_bars: bool,
    /// `switch_right_click_behavior` (default off; moho reads it as it
    /// picks the right-click order: MohoEngine.dll 0x103e8072): a damaged
    /// friendly unit is repaired before it's guarded.
    pub switch_right_click: bool,
}

impl Default for ProfileOptions {
    fn default() -> Self {
        Self { display_eta: true, show_attached_bars: true, switch_right_click: false }
    }
}

/// Reads [`ProfileOptions`] from the interface's profile (unset ones keep
/// their defaults).
pub fn read_profile_options(vm: &rts_core_engine::ui::UserVm) -> ProfileOptions {
    let flag = |key: &str| vm.lua().load(format!("return GetOptions('{key}')")).eval::<Option<bool>>().ok().flatten();
    let d = ProfileOptions::default();
    ProfileOptions {
        display_eta: flag("display_eta").unwrap_or(d.display_eta),
        show_attached_bars: flag("show_attached_unit_lifebars").unwrap_or(d.show_attached_bars),
        switch_right_click: flag("switch_right_click_behavior").unwrap_or(d.switch_right_click),
    }
}

/// FAF's Primary Adapter default, which it writes into every new profile:
/// not a mode the player chose, so it doesn't make the window fullscreen
/// at start.
const DEFAULT_ADAPTER: &str = "1024,768,60";

/// Applies the profile's [`PROFILE_AT_START`] options, as the commands they
/// send, when an interface (the front end's or a game's) starts. Unset ones
/// keep the engine's defaults. Primary Adapter too, unless it's FAF's
/// default or this is a `--screenshot` run (always windowed). The window's
/// fullscreen modes become the option's choices first.
pub fn apply_profile(s: &mut RenderState) {
    apply_displays(s);
    let modes = s.display_modes();
    let second = s.second_display_modes();
    let monitors: Vec<(String, String)> = crate::render::state::monitors(&s.window)
        .into_iter()
        .enumerate()
        .map(|(i, (key, m))| {
            // (winit's names are model numbers on macOS: no help to a player.)
            let size = m.size();
            let main = s.window.primary_monitor().as_ref() == Some(&m);
            (key, format!("Display {} ({}x{}{})", i + 1, size.width, size.height, if main { ", main" } else { "" }))
        })
        .collect();
    let Some(vm) = &s.ui else { return };
    s.profile = read_profile_options(vm);
    s.player.switch_right_click = s.profile.switch_right_click;
    if let Err(e) = list_display_modes(vm.lua(), &modes, &second, &monitors) {
        tracing::warn!("display modes for the options: {e}");
    }
    let read = |key: &str| vm.lua().load(format!("local v = GetOptions('{key}') return v ~= nil and tostring(v) or nil")).eval::<Option<String>>().ok().flatten();
    let mut lines: Vec<String> = PROFILE_AT_START.iter().filter_map(|(key, cmd)| Some(format!("{cmd} {}", read(key)?))).collect();
    if let Some(mode) = read("primary_adapter").filter(|m| m != DEFAULT_ADAPTER && s.screenshot.is_none()) {
        lines.push(format!("SC_PrimaryAdapter {mode}"));
    }
    for line in lines {
        run(s, &line);
    }
}

/// The profile's Primary Display (the main window moves to it) and
/// Secondary Display (the second head's monitor, read as it is made): the
/// engine's own options, beside FAF's adapters, since a Mac's (or any
/// desktop's) windows open where the system puts them.
pub fn apply_displays(s: &mut RenderState) {
    let Some(vm) = s.ui.as_ref().or(s.ui_booting.as_ref()) else { return };
    let read = |key: &str| vm.lua().load(format!("local v = GetOptions('{key}') return v ~= nil and tostring(v) or nil")).eval::<Option<String>>().ok().flatten();
    let (primary, secondary) = (read("primary_display"), read("secondary_display"));
    s.secondary_display = secondary.filter(|k| k != crate::render::state::DEFAULT_DISPLAY).unwrap_or_default();
    if s.screenshot.is_none() {
        s.set_primary_display(primary.as_deref().unwrap_or(crate::render::state::DEFAULT_DISPLAY));
    }
}

/// Puts `modes` (width, height, Hz) in FAF's Primary Adapter option as its
/// choices after Windowed (moho fills them from the device's caps; FAF's
/// list is an example), Windowed and `second` (another monitor's) in
/// Secondary Adapter after Disabled, and names Anti-Aliasing's choices (Off, SMAA). Adds the
/// engine's Primary / Secondary Display options after each adapter, with
/// `monitors` (key, label) after the default.
fn list_display_modes(lua: &mlua::Lua, modes: &[(u32, u32, u32)], second: &[(u32, u32, u32)], monitors: &[(String, String)]) -> mlua::Result<()> {
    let table = |modes: &[(u32, u32, u32)]| -> mlua::Result<mlua::Table> {
        let list = lua.create_table_with_capacity(modes.len(), 0)?;
        for &(w, h, hz) in modes {
            list.push([w, h, hz])?;
        }
        Ok(list)
    };
    lua.load(
        r#"
        local modes, second, monitors = ...
        local function states(first, list)
            local out = { first }
            for _, m in list do
                table.insert(out, { text = string.format('%dx%d(%d)', m[1], m[2], m[3]), key = string.format('%d,%d,%d', m[1], m[2], m[3]) })
            end
            return out
        end
        for _, item in import('/lua/options/options.lua').options.video.items do
            if item.key == 'primary_adapter' then
                item.custom.states = states({ text = "<LOC OPTIONS_0070>Windowed", key = 'windowed' }, modes)
            elseif item.key == 'secondary_adapter' then
                item.custom.states = states({ text = "<LOC _Disabled>", key = 'disabled' }, second)
                -- The engine's own: a second window on any display (moho's
                -- list is fullscreen modes only).
                table.insert(item.custom.states, 2, { text = "<LOC OPTIONS_0070>Windowed", key = 'windowed' })
            elseif item.key == 'antialiasing' then
                -- Keys as FA's sample counts: 0 off, 1 SMAA (render/smaa.rs).
                item.custom.states = { { text = "<LOC OPTIONS_0029>Off", key = 0 }, { text = "SMAA", key = 1 } }
            end
        end
        local function displays(default)
            local out = { { text = default, key = 'default' } }
            for _, m in monitors do
                table.insert(out, { text = m[2], key = m[1] })
            end
            return out
        end
        local items = import('/lua/options/options.lua').options.video.items
        local function place(after, item)
            for i, it in items do
                if it.key == item.key then
                    it.custom.states = item.custom.states
                    return
                end
            end
            for i, it in items do
                if it.key == after then
                    table.insert(items, i + 1, item)
                    return
                end
            end
        end
        place('primary_adapter', {
            title = "Primary Display",
            key = 'primary_display',
            type = 'toggle',
            default = 'default',
            custom = { states = displays("System main display") },
        })
        place('secondary_adapter', {
            title = "Secondary Display",
            key = 'secondary_display',
            type = 'toggle',
            default = 'default',
            restart = true,
            custom = { states = displays("Next display") },
        })
        "#,
    )
    .set_name("=[engine display modes]")
    .call::<()>((table(modes)?, table(second)?, monitors.iter().map(|(k, l)| [k.clone(), l.clone()]).collect::<Vec<_>>()))
}

/// FAF's Level Of Detail option (moho's LOD scale: every LOD cutoff is
/// compared with view depth times it). `cam_SetLOD <camera> <s>`
/// (MohoEngine.dll 0x10376b80: the named camera's `SetLODScale`; FAF's
/// options name `WorldCamera`, and a name with no world camera, such as the
/// minimap's, finds none); `cam_DefaultLOD <s>`,
/// what a new camera takes; and `SC_CameraScaleLOD n` (SupremeCommander.exe
/// 0x912500: Low / Medium / High by `n`, read as an integer, so FAF's
/// Extreme 2.1 is High; it runs the other two).
pub fn lod(scene: &mut World, cmd: &str, rest: &str) {
    tracing::debug!(cmd, rest, "LOD console command");
    let set = |scene: &mut World, camera: &str, scale: f32| {
        if let Some(cam) = scene.cameras.get_mut(camera) {
            cam.lod_scale = scale;
        }
    };
    let number = |v: &str| v.parse::<f32>().ok().filter(|v| v.is_finite());
    match cmd {
        "cam_setlod" => {
            let mut words = rest.split_whitespace();
            if let (Some(camera), Some(scale)) = (words.next(), words.next().and_then(number)) {
                set(scene, camera, scale);
            }
        }
        "cam_defaultlod" => {
            if let Some(scale) = number(rest) {
                scene.default_lod = scale;
            }
        }
        _ => {
            let level = rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse::<usize>().ok());
            if let Some(&scale) = level.and_then(|n| SCALE_LOD.get(n)) {
                scene.default_lod = scale;
                set(scene, "WorldCamera", scale);
            }
        }
    }
}

/// The Shadow render distance slider's top (FAF's `shadow_render_distance`
/// max): no limit, shadows at every zoom.
const SHADOW_LOD_UNLIMITED: f32 = 440.0;

/// `ren_ShadowLOD`'s LOD metric cut-off (unlimited at the slider's top).
fn shadow_lod(v: f32) -> f32 {
    if v >= SHADOW_LOD_UNLIMITED { f32::INFINITY } else { v }
}

/// A console variable's boolean value (`true` / `false`, `1` / `0`).
/// A boolean console variable's new value: the argument, or with none the
/// other value (moho's bare variable toggles; key actions send it so); an
/// unreadable argument changes nothing.
fn set_or_toggle(current: bool, rest: &str) -> bool {
    if rest.is_empty() {
        !current
    } else {
        console_bool(rest).unwrap_or(current)
    }
}

fn console_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "on" => Some(true),
        "false" | "0" | "off" => Some(false),
        _ => None,
    }
}

/// The player's units (any army's when observing), with their blueprints.
/// As the real game's `UISelectionByCategory('ALLUNITS')` (`probe_ui_units`):
/// structures still being built count, cargo in a transport and units still
/// inside their factory don't.
fn own_units(s: &RenderState) -> Vec<(u32, String, [f32; 3])> {
    let Some(blueprints) = s.world.blueprints.as_ref() else { return Vec::new() };
    s.world
        .entities()
        .iter()
        .filter(|e| matches!(e.kind, ViewKind::Unit | ViewKind::Structure) && !e.attached && !e.in_builder)
        .filter(|e| s.player.player.is_none_or(|a| a == e.army))
        .filter_map(|e| Some((e.id, blueprints.get(e.blueprint as usize)?.clone(), e.position)))
        .collect()
}

/// `UI_SelectByCategory [+add] [+inview] [+idle] [+nearest]
/// [+excludeengineers] [+goto] NAMES...`: the player's units in all the
/// named categories, on screen / idle / the one nearest the cursor / without
/// engineers; `+add` adds them to the selection; `+goto` centres the camera
/// on the result. (FAF's `UISelectionByCategory` comes here too.)
fn select_by_category(s: &mut RenderState, args: &str) {
    let (flags, names): (Vec<&str>, Vec<&str>) = args.split_whitespace().partition(|w| w.starts_with('+'));
    let has = |f: &str| flags.iter().any(|g| g.eq_ignore_ascii_case(f));
    let Some(vm) = &s.ui else { return };
    let units = own_units(s);
    let pairs: Vec<(u32, &str)> = units.iter().map(|(id, bp, _)| (*id, bp.as_str())).collect();
    let mut ids = vm.filter_category(&pairs, &names.join(" ")).unwrap_or_default();
    if has("+excludeengineers") {
        let engineers = vm.filter_category(&pairs, "ENGINEER").unwrap_or_default();
        ids.retain(|id| engineers.binary_search(id).is_err());
    }
    if has("+idle") {
        let busy = s.world.latest.as_ref().map(|l| l.busy.clone()).unwrap_or_default();
        ids.retain(|id| busy.binary_search(id).is_err());
    }
    let position = |id: u32| units.iter().find(|u| u.0 == id).map(|u| u.2);
    if has("+inview") {
        ids.retain(|&id| position(id).is_some_and(|p| s.world.in_view(p.into())));
    }
    if has("+nearest") {
        // Nearest the ground under the cursor (else the camera's focus).
        let from = s
            .input
            .cursor
            .and_then(|(x, y)| s.world.ground_at(x as f32, y as f32))
            .or_else(|| s.world.camera().map(|c| c.target));
        if let Some(from) = from {
            let d2 = |id: u32| position(id).map_or(f32::MAX, |p| (p[0] - from.x).powi(2) + (p[2] - from.z).powi(2));
            ids = ids.into_iter().min_by(|a, b| d2(*a).total_cmp(&d2(*b)).then(a.cmp(b))).into_iter().collect();
        }
    }
    if has("+add") {
        ids.extend(s.player.selection.iter().copied());
        ids.sort_unstable();
        ids.dedup();
    }
    if ids.is_empty() {
        return;
    }
    ids.sort_unstable();
    if has("+goto")
        && let (Some(p), Some(cam)) = (position(ids[0]), s.world.camera_mut()) {
        cam.target.x = p[0];
        cam.target.z = p[2];
    }
    s.player.selection = ids;
}

/// `UI_ExpandCurrentSelection`: every unit of the player's of the
/// selected units' blueprints (on screen or not).
fn expand_selection(s: &mut RenderState) {
    let units = own_units(s);
    let kinds: Vec<&str> = units.iter().filter(|u| s.player.selection.binary_search(&u.0).is_ok()).map(|u| u.1.as_str()).collect();
    if kinds.is_empty() {
        return;
    }
    let mut ids: Vec<u32> = units.iter().filter(|u| kinds.contains(&u.1.as_str())).map(|u| u.0).collect();
    ids.sort_unstable();
    s.player.selection = ids;
}

#[cfg(test)]
mod tests {
    #[test]
    fn boolean_variables_set_or_toggle() {
        assert!(super::set_or_toggle(false, "1"));
        assert!(!super::set_or_toggle(true, "0"));
        assert!(super::set_or_toggle(false, ""), "bare: toggles on");
        assert!(!super::set_or_toggle(true, ""), "bare: toggles off");
        assert!(super::set_or_toggle(true, "maybe"), "unreadable: unchanged");
    }

    /// Shadow render distance: the slider's values cut shadows off there,
    /// its top (and beyond) leaves them unlimited.
    #[test]
    fn shadow_lod_top_is_unlimited() {
        assert_eq!(super::shadow_lod(260.0), 260.0);
        assert_eq!(super::shadow_lod(420.0), 420.0);
        assert!(super::shadow_lod(440.0).is_infinite());
        assert!(super::shadow_lod(1000.0).is_infinite());
    }
}
