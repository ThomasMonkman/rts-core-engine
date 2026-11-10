//! User-layer engine globals (FAF's `/engine/User.lua`). Every one exists
//! from the start so scripts that capture them at load time work; the ones
//! not implemented yet raise a clear error when called (like the sim's
//! `api::pending`). Real implementations are registered after the stubs
//! and replace them.

use mlua::prelude::*;

use crate::lua_api::shapes::Shaped;
use crate::lua_api::{Obj, Status, lua_fn};
use crate::ui::shapes::Loose;
use std::collections::BTreeMap;

use crate::engine::api::categories::EntityCategory;
use crate::engine::api::moho;
use crate::engine::sim::category::{CategoryDomain, CategorySet};

use mlua::Either;

/// The files of a kind (`GetSpecialFiles`), for the file pickers.
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "SpecialFiles", write_only)]
struct SpecialFiles {
    /// Prepend to `profile .. '/' .. name .. '.' .. extension` for a path.
    directory: String,
    extension: String,
    /// Basenames by profile.
    files: BTreeMap<String, Vec<String>>,
}

/// A file's details (`GetSpecialFileInfo`).
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "SpecialFileInfo", write_only)]
struct SpecialFileInfo {
    #[lua(rename = "IsFolder")]
    is_folder: bool,
    #[lua(rename = "IsReadOnly")]
    is_read_only: bool,
    #[lua(rename = "SizeBytes")]
    size_bytes: u64,
    /// The write time as 16 hex digits; sorts as the time does.
    #[lua(rename = "TimeStamp")]
    time_stamp: String,
    /// UTC.
    #[lua(rename = "WriteTime")]
    write_time: WriteTime,
}

/// A write time, broken down.
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "SpecialFileWriteTime", write_only)]
struct WriteTime {
    year: u32,
    month: u32,
    mday: u32,
    wday: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

/// What `SimCallback` takes.
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "SimCallbackRequest", rename_all = "PascalCase", write_only)]
struct SimCallbackRequest {
    /// The sim's global function to call.
    func: String,
    /// Any serialisable value, passed to it.
    args: Option<LuaValue>,
}

pub const USER_GLOBALS: &[&str] = &[
    "AddBlinkyBox",
    "AddCommandFeedbackBlip",
    "AddConsoleOutputReciever",
    "AddInputCapture",
    "AddSelectUnits",
    "AddToSessionExtraSelectList",
    "AnyInputCapture",
    "ClearBuildTemplates",
    "ClearCurrentFactoryForQueueDisplay",
    "ClearFrame",
    "ClearSessionExtraSelectList",
    "ConExecute",
    "ConExecuteSave",
    "ConTextMatches",
    "CopyCurrentReplay",
    "CopyToClipboard",
    "CreateUnitAtMouse",
    "CurrentTime",
    "DebugFacilitiesEnabled",
    "DecreaseBuildCountInQueue",
    "DeleteCommand",
    "DisableWorldSounds",
    "EjectSessionClient",
    "EnableWorldSounds",
    "EngineStartFrontEndUI",
    "EngineStartSplashScreens",
    "EntityCategoryFilterOut",
    "ExecLuaInSim",
    "ExitApplication",
    "ExitGame",
    "FlushEvents",
    "FormatTime",
    "GameTick",
    "GameTime",
    "GenerateBuildTemplateFromSelection",
    "GetActiveBuildTemplate",
    "GetAntiAliasingOptions",
    "GetArmiesTable",
    "GetArmyAvatars",
    "GetArmyScore",
    "GetAssistingUnitsList",
    "GetAttachedUnitsList",
    "GetCamera",
    "GetCommandLineArg",
    "GetCurrentUIState",
    "GetCursor",
    "GetEconomyTotals",
    "GetFireState",
    "GetFrame",
    "GetFrontEndData",
    "GetGameSpeed",
    "GetGameTime",
    "GetHighlightCommand",
    "GetIdleEngineers",
    "GetIdleFactories",
    "GetInputCapture",
    "GetIsAutoMode",
    "GetIsAutoSurfaceMode",
    "GetIsPaused",
    "GetIsPausedOfUnit",
    "GetIsSubmerged",
    "GetMouseScreenPos",
    "GetMouseWorldPos",
    "GetMovieVolume",
    "GetNumRootFrames",
    "GetOptions",
    "GetPreference",
    "GetResourceSharing",
    "GetRolloverInfo",
    "GetScriptBit",
    "GetSelectedUnits",
    "GetSessionClients",
    "GetSimRate",
    "GetSimTicksPerSecond",
    "GetSpecialFileInfo",
    "GetSpecialFilePath",
    "GetSpecialFiles",
    "GetSpecialFolder",
    "GetSystemTime",
    "GetSystemTimeSeconds",
    "GetTextureDimensions",
    "GetUIControlsAlpha",
    "GetUnitCommandData",
    "GetUnitCommandDataOfUnit",
    "GetUnitCommandFromCommandCap",
    "GetValidAttackingUnits",
    "GetVolume",
    "GpgNetActive",
    "GpgNetSend",
    "HasCommandLineArg",
    "IN_AddKeyMapTable",
    "IN_ClearKeyMap",
    "IN_RemoveKeyMapTable",
    "IncreaseBuildCountInQueue",
    "InternalCreateBitmap",
    "InternalCreateBorder",
    "InternalCreateDiscoveryService",
    "InternalCreateDragger",
    "InternalCreateEdit",
    "InternalCreateFrame",
    "InternalCreateGroup",
    "InternalCreateHistogram",
    "InternalCreateItemList",
    "InternalCreateLobby",
    "InternalCreateMapPreview",
    "InternalCreateMesh",
    "InternalCreateMovie",
    "InternalCreateScrollbar",
    "InternalCreateText",
    "InternalCreateWldUIProvider",
    "InternalCreateWorldMesh",
    "InternalSaveGame",
    "IsKeyDown",
    "IsObserver",
    "IssueBlueprintCommand",
    "IssueBlueprintCommandToUnit",
    "IssueBlueprintCommandToUnits",
    "IssueCommand",
    "IssueDockCommand",
    "IssueUnitCommand",
    "KeycodeMSWToMaui",
    "KeycodeMauiToMSW",
    "LaunchGPGNet",
    "LaunchReplaySession",
    "LaunchSinglePlayerSession",
    "LoadSavedGame",
    "MapBorderAdd",
    "MapBorderClear",
    "OpenURL",
    "PauseSound",
    "PauseVoice",
    "PlaySound",
    "PlayTutorialVO",
    "PlayVoice",
    "PostDragger",
    "PrefetchSession",
    "RemoveConsoleOutputReciever",
    "RemoveFromSessionExtraSelectList",
    "RemoveInputCapture",
    "RemoveProfileDirectories",
    "RemoveSpecialFile",
    "RenderOverlayEconomy",
    "RenderOverlayIntel",
    "RenderOverlayMilitary",
    "RestartSession",
    "SavePreferences",
    "SelectUnits",
    "SessionCanRestart",
    "SessionEndGame",
    "SessionGetCommandSourceNames",
    "SessionGetLocalCommandSource",
    "SessionGetScenarioInfo",
    "SessionIsActive",
    "SessionIsBeingRecorded",
    "SessionIsGameOver",
    "SessionIsMultiplayer",
    "SessionIsObservingAllowed",
    "SessionIsPaused",
    "SessionRequestPause",
    "SessionResume",
    "SessionSendChatMessage",
    "SetActiveBuildTemplate",
    "SetAutoMode",
    "SetAutoSurfaceMode",
    "SetCurrentFactoryForQueueDisplay",
    "SetCursor",
    "SetFireState",
    "SetFrontEndData",
    "SetGameSpeed",
    "SetInvertMidMouseButton",
    "SetMovieVolume",
    "SetOverlayFilter",
    "SetOverlayFilters",
    "SetPaused",
    "SetPausedOfUnit",
    "SetPreference",
    "SetUIControlsAlpha",
    "SetVolume",
    "SimCallback",
    "SoundIsPrepared",
    "StartSound",
    "StopAllSounds",
    "StopSound",
    "SyncPlayableRect",
    "ToggleFireState",
    "ToggleScriptBit",
    "UISelectAndZoomTo",
    "UISelectionByCategory",
    "UIZoomTo",
    "UnProject",
    "ValidateIPAddress",
    "ValidateUnitsList",
    "WorldIsLoading",
    "WorldIsPlaying",
    "_c_CreateCursor",
    "_c_CreateDecal",
    "_c_CreatePathDebugger",
];

/// Core globals (FAF's `/engine/Core.lua`, shared with the sim state):
/// stubbed only where nothing registered them already.
pub const CORE_GLOBALS: &[&str] = &[
    "AITarget",
    "AudioSetLanguage",
    "BeginLoggingStats",
    "BlueprintLoaderUpdateProgress",
    "CreatePrefetchSet",
    "CurrentThread",
    "Dirname",
    "DiskFindFiles",
    "DiskGetFileInfo",
    "DiskToLocal",
    "EndLoggingStats",
    "EntityCategoryContains",
    "EntityCategoryEmpty",
    "EntityCategoryFilterDown",
    "EntityCategoryGetUnitList",
    "EnumColorNames",
    "EulerToQuaternion",
    "FileCollapsePath",
    "ForkThread",
    "GetBlueprint",
    "GetCueBank",
    "GetDepositsAroundPoint",
    "GetFocusArmy",
    "GetGameTimeSeconds",
    "GetMovieDuration",
    "GetUnitById",
    "GetVersion",
    "HasLocalizedVO",
    "IsDestroyed",
    "KillThread",
    "MATH_IRound",
    "MATH_Lerp",
    "MinLerp",
    "MinSlerp",
    "OrientFromDir",
    "ParseEntityCategory",
    "PointVector",
    "RPCSound",
    "Random",
    "Rect",
    "RegisterBeamBlueprint",
    "RegisterEmitterBlueprint",
    "RegisterMeshBlueprint",
    "RegisterProjectileBlueprint",
    "RegisterPropBlueprint",
    "RegisterTrailEmitterBlueprint",
    "RegisterUnitBlueprint",
    "ResumeThread",
    "SPEW",
    "STR_GetTokens",
    "STR_Utf8Len",
    "STR_Utf8SubString",
    "STR_itox",
    "STR_xtoi",
    "SecondsPerTick",
    "SessionIsReplay",
    "SetFocusArmy",
    "Sound",
    "SpecFootprints",
    "SuspendCurrentThread",
    "Trace",
    "VAdd",
    "VDiff",
    "VDist2",
    "VDist2Sq",
    "VDist3",
    "VDist3Sq",
    "VDot",
    "VMult",
    "VPerpDot",
    "Vector",
    "Vector2",
    "WARN",
    "WaitFor",
    "doscript",
    "exists",
];

/// Where moho keeps its saves: the game's folder under Documents (the
/// layout is inferred; docs/research/verifying-against-the-binary.md). Saves aren't
/// supported yet, so this is only a name.
fn saves_dir() -> String {
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
    format!("{home}/Documents/My Games/Gas Powered Games/Supreme Commander Forged Alliance/savegames/").replace('\\', "/")
}

/// moho's special file types: their folder (`/`-terminated, with a folder
/// per profile in it) and extension. Replays are our recordings
/// (`engine::replay`: our own format, in the engine's replays folder), not
/// moho's `.SCFAReplay`s. `None` for an unknown type.
fn special_folder(kind: &str) -> Option<(String, &'static str)> {
    match kind {
        "SaveGame" | "CampaignSave" => Some((saves_dir(), "SCFAsave")),
        "Replay" => {
            let root = crate::install::replays_dir()?;
            Some((format!("{}/", root.to_string_lossy().replace('\\', "/").trim_end_matches('/')), crate::engine::replay::EXTENSION))
        }
        _ => None,
    }
}

/// moho `GetSpecialFilePath(profile, basename, type)`: where a save or
/// replay of that name lives.
fn special_file_path(profile: &str, name: &str, kind: &str) -> Option<String> {
    let (dir, ext) = special_folder(kind)?;
    let profile = crate::engine::replay::profile_dir(std::path::Path::new(""), profile);
    Some(format!("{dir}{}/{name}.{ext}", profile.to_string_lossy()))
}

/// moho `GetSpecialFiles(type)`: `{directory, extension, files = {[profile]
/// = {basename, ...}}}` (FAF's file picker builds a file's path as
/// `directory .. profile .. '/' .. name .. '.' .. extension`). Profiles and
/// names in sorted order. No saves are listed (not supported).
fn special_files(lua: &Lua, kind: &str) -> LuaResult<Option<Shaped<SpecialFiles>>> {
    let Some((dir, ext)) = special_folder(kind) else { return Ok(None) };
    let listed = match kind {
        "Replay" => crate::install::replays_dir().map(|root| crate::engine::replay::list(&root)).unwrap_or_default(),
        _ => Default::default(),
    };
    let files = lua.create_table_with_capacity(0, listed.len())?;
    for (profile, names) in listed {
        files.raw_set(profile, lua.create_sequence_from(names)?)?;
    }
    let t = lua.create_table_with_capacity(0, 3)?;
    t.raw_set("directory", dir)?;
    t.raw_set("extension", ext)?;
    t.raw_set("files", files)?;
    Ok(Some(Shaped::new(t)))
}

/// moho `GetSpecialFileInfo(profile, basename, type)`: the file's
/// `IsFolder`, `IsReadOnly`, `SizeBytes`, `TimeStamp` (its write time as 16
/// hex digits: sorts as the time does) and `WriteTime` (`year`, `month`,
/// `mday`, `wday`, `hour`, `minute`, `second`; UTC, as our recordings'
/// names), or nil when it isn't there. The interface only: the sim never
/// sees a file's time.
fn special_file_info(lua: &Lua, profile: &str, name: &str, kind: &str) -> LuaResult<Option<Shaped<SpecialFileInfo>>> {
    let Some(meta) = special_file_path(profile, name, kind).and_then(|p| std::fs::metadata(crate::engine::replay::find_file(p.as_ref())).ok()) else { return Ok(None) };
    let secs = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let at = crate::engine::replay::utc_time(secs);
    let write = lua.create_table_with_capacity(0, 7)?;
    write.raw_set("year", at.year)?;
    write.raw_set("month", at.month)?;
    write.raw_set("mday", at.day)?;
    write.raw_set("wday", at.weekday)?;
    write.raw_set("hour", at.hour)?;
    write.raw_set("minute", at.minute)?;
    write.raw_set("second", at.second)?;
    let t = lua.create_table_with_capacity(0, 5)?;
    t.raw_set("IsFolder", meta.is_dir())?;
    t.raw_set("IsReadOnly", meta.permissions().readonly())?;
    t.raw_set("SizeBytes", meta.len())?;
    t.raw_set("TimeStamp", format!("{secs:016x}"))?;
    t.raw_set("WriteTime", write)?;
    Ok(Some(Shaped::new(t)))
}

/// moho `LaunchReplaySession(filename)`: from the main menu, leaves the
/// recording for the renderer to start (`__engine_launch_replay`, taken by
/// `UserVm::take_replay_request`) and returns true; false (FAF says it
/// can't launch it) for a file that isn't one of our recordings with a
/// header, or during a game (a session can't be swapped for another yet).
fn launch_replay_session(lua: &Lua, filename: &str) -> LuaResult<bool> {
    if lua.globals().raw_get::<Option<String>>("__engine_ui_state")?.as_deref() != Some("frontend") {
        tracing::warn!(target: "lua", filename, "LaunchReplaySession: only from the main menu for now");
        return Ok(false);
    }
    let path = crate::engine::replay::find_file(filename.as_ref());
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(target: "lua", filename, "LaunchReplaySession: {e}");
            return Ok(false);
        }
    };
    let header = crate::engine::replay::is_recording(&text).then(|| crate::engine::replay::read_header(&text));
    if !matches!(header, Some(Ok(crate::engine::replay::Header { session: Some(_), .. }))) {
        tracing::warn!(target: "lua", filename, "LaunchReplaySession: not a recording of this engine's with its session");
        return Ok(false);
    }
    lua.globals().raw_set("__engine_launch_replay", path.to_string_lossy())?;
    Ok(true)
}

pub fn register_stubs(lua: &Lua) -> LuaResult<()> {
    let g = lua.globals();
    // User-layer globals not implemented yet warn once and return nil (the
    // interface keeps building round them; the log lists them). Core ones
    // the sim also lacks raise, as there.
    for &name in USER_GLOBALS {
        if g.raw_get::<LuaValue>(name)? != LuaNil {
            continue;
        }
        let warned = std::cell::Cell::new(false);
        crate::lua_api::register_fn(
            lua,
            &g,
            "",
            name,
            None,
            "",
            Status::Stub,
            false,
            move |_, _: LuaMultiValue| {
                if !warned.replace(true) {
                    tracing::warn!(target: "lua", "user engine API '{name}' is not implemented yet (returns nil)");
                }
                Ok(())
            },
        )?;
    }
    for &name in CORE_GLOBALS {
        if g.raw_get::<LuaValue>(name)? != LuaNil {
            continue;
        }
        crate::lua_api::register_fn(
            lua,
            &g,
            "",
            name,
            None,
            "",
            Status::Stub,
            false,
            move |_, _: LuaMultiValue| -> LuaResult<()> { Err(LuaError::runtime(format!("engine API '{name}' is not implemented yet"))) },
        )?;
    }
    Ok(())
}

/// The ones implemented (simple ones here; controls, prefs in their modules).
/// The values after option `name` (`/nomusic`, also `--nomusic`) on the
/// process command line, up to the next option; `None` when it isn't there.
fn command_line_arg(name: &str) -> Option<Vec<String>> {
    command_line_arg_in(&std::env::args().skip(1).collect::<Vec<_>>(), name)
}

fn command_line_arg_in(args: &[String], name: &str) -> Option<Vec<String>> {
    let bare = name.trim_start_matches(['/', '-']).to_lowercase();
    let is_option = |a: &str| a.starts_with('/') || a.starts_with('-');
    let at = args.iter().position(|a| is_option(a) && a.trim_start_matches(['/', '-']).eq_ignore_ascii_case(&bare))?;
    Some(args[at + 1..].iter().take_while(|a| !is_option(a)).cloned().collect())
}

/// One blueprint's part of [`unit_command_data`]: its command and toggle
/// caps, and what its `Economy.BuildableCategory` expressions name.
struct BlueprintCommands {
    orders: Vec<String>,
    toggles: Vec<String>,
    buildable: Option<CategorySet>,
}

/// `General.CommandCaps` / `ToggleCaps` that are on, and the union of the
/// `Economy.BuildableCategory` expressions that parse (moho skips bad ones).
fn blueprint_commands(domain: &CategoryDomain, bp: &LuaTable) -> LuaResult<BlueprintCommands> {
    let caps = |field: &str, names: &[&str]| -> LuaResult<Vec<String>> {
        let Some(general) = bp.raw_get::<Option<LuaTable>>("General")? else { return Ok(Vec::new()) };
        let bits = crate::engine::sim::unit_state::bitfield_bits(names, "", &general.raw_get::<LuaValue>(field)?)?;
        Ok(names.iter().enumerate().filter(|(i, _)| bits & 1 << i != 0).map(|(_, n)| n.to_string()).collect())
    };
    let mut buildable: Option<CategorySet> = None;
    let exprs = match bp.raw_get::<Option<LuaTable>>("Economy")? {
        Some(economy) => economy.raw_get::<Option<LuaTable>>("BuildableCategory")?,
        None => None,
    };
    if let Some(exprs) = exprs {
        for expr in exprs.sequence_values::<LuaValue>() {
            if let LuaValue::String(expr) = expr?
                && let Ok(cat) = domain.parse(&expr.to_str()?) {
                buildable = Some(buildable.map_or_else(|| cat.clone(), |b| b.union(&cat)));
            }
        }
    }
    Ok(BlueprintCommands { orders: caps("CommandCaps", &crate::engine::sim::unit_state::COMMAND_CAPS)?, toggles: caps("ToggleCaps", &crate::engine::sim::unit_state::TOGGLE_CAPS)?, buildable })
}

/// moho's `GetUnitCommandData`: the command caps and toggle caps any of
/// `units` has (sorted), and what any of them can build, or will once the
/// upgrades it has queued are done (a factory upgrading to T2 offers T2
/// units to queue behind the upgrade), less what the sim's build
/// restrictions forbid that unit now (the ACU's T2/T3 until its engineering
/// upgrade, HQ-gated units). Each blueprint is read once: FAF calls this
/// twice per selection change, and a selection is thousands of units of a
/// few blueprints.
fn unit_command_data(lua: &Lua, units: &[Obj<moho::UserUnit>]) -> LuaResult<(Vec<String>, Vec<String>, EntityCategory)> {
    /// A blueprint the selection uses: its part, whether a unit uses it as
    /// it is (no upgrade queued, no restriction), and whether any does.
    struct Used {
        commands: Option<BlueprintCommands>,
        plain: bool,
        any: bool,
    }
    let domain = crate::engine::api::categories::domain(lua)?;
    let data: LuaTable = lua.globals().raw_get("__engine_unit_data")?;
    let blueprints: LuaTable = lua.globals().get("__blueprints")?;
    let mut used: BTreeMap<String, Used> = BTreeMap::new();
    let part = |used: &mut BTreeMap<String, Used>, id: &str| -> LuaResult<()> {
        if !used.contains_key(id) {
            let commands = blueprints.get::<Option<LuaTable>>(id)?.map(|bp| blueprint_commands(&domain, &bp)).transpose()?;
            used.insert(id.to_string(), Used { commands, plain: false, any: false });
        }
        Ok(())
    };
    let key = |k: &str| lua.create_string(k);
    let (id_key, bp_key, queue_key, restricted_key) = (key("_id")?, key("bp")?, key("queue")?, key("restricted")?);
    // Units with an upgrade queued or a restriction: their own sets.
    let mut own_sets: Vec<CategorySet> = Vec::new();
    for u in units {
        // A dead unit (its data gone) has no blueprint.
        let Some(d) = data.raw_get::<Option<LuaTable>>(u.raw_get::<LuaValue>(&id_key)?)? else { continue };
        let Some(bp) = d.raw_get::<Option<LuaString>>(&bp_key)? else { continue };
        let bp = bp.to_str()?;
        part(&mut used, &bp)?;
        let Some(entry) = used.get_mut(&*bp).filter(|e| e.commands.is_some()) else { continue };
        entry.any = true;
        let mut upgrades: Vec<String> = Vec::new();
        if let Some(queue) = d.raw_get::<Option<LuaTable>>(&queue_key)? {
            for c in queue.sequence_values::<LuaTable>() {
                let c = c?;
                if c.raw_get::<Option<LuaString>>("type")?.is_some_and(|k| k.as_bytes() == b"Upgrade") {
                    upgrades.extend(c.raw_get::<Option<String>>("blueprintId")?);
                }
            }
        }
        let restricted = d.raw_get::<Option<Vec<String>>>(&restricted_key)?.unwrap_or_default();
        if upgrades.is_empty() && restricted.is_empty() {
            used.get_mut(&*bp).expect("just read").plain = true;
            continue;
        }
        let mut own = used[&*bp].commands.as_ref().and_then(|c| c.buildable.clone());
        for up in &upgrades {
            part(&mut used, up)?;
            if let Some(cat) = used[up.as_str()].commands.as_ref().and_then(|c| c.buildable.as_ref()) {
                own = Some(own.map_or_else(|| cat.clone(), |o| o.union(cat)));
            }
        }
        if let Some(mut o) = own {
            for id in &restricted {
                if let Ok(cat) = domain.parse(id) {
                    o = o.difference(&cat);
                }
            }
            own_sets.push(o);
        }
    }
    let (mut orders, mut toggles) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    let mut buildable = CategorySet::empty(domain.entries.len());
    for e in used.values().filter(|e| e.any) {
        let c = e.commands.as_ref().expect("only blueprints that exist are used");
        orders.extend(c.orders.iter().cloned());
        toggles.extend(c.toggles.iter().cloned());
        if let (true, Some(cat)) = (e.plain, &c.buildable) {
            buildable = buildable.union(cat);
        }
    }
    for o in &own_sets {
        buildable = buildable.union(o);
    }
    Ok((orders.into_iter().collect(), toggles.into_iter().collect(), EntityCategory(buildable)))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// The selection's orders, toggles and buildable categories
        /// ([`unit_command_data`]).
        GetUnitCommandData = |lua, unit_set: Option<Vec<Obj<moho::UserUnit>>>| unit_command_data(lua, &unit_set.unwrap_or_default()))?;
    lua_fn!(lua, global,
        #[lua(internal)]
        __engine_con_execute = |lua, line: String| {
            if let Err(e) = crate::ui::console(lua, &line) {
                tracing::warn!(target: "lua", line, "ConExecute failed: {e}");
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The user layer's clock: seconds of UI frames run (`UserVm::frame`),
        /// which is what animations and `WaitSeconds` count in.
        CurrentTime = |lua, ()| Ok(lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0)))?;
    lua_fn!(lua, global,
        /// Same clock as `CurrentTime`.
        GetSystemTimeSeconds = |lua, ()| Ok(lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0)))?;
    lua_fn!(lua, global,
        /// Wall-clock seconds since the Unix epoch.
        GetSystemTime = |_, ()| Ok(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())))?;
    lua_fn!(lua, global,
        /// Direct Connect: `host:port` (a name or an address) to `ip:port`, or
        /// false (moho resolves the name; FAF joins the address it returns). An
        /// internet join code in the address box (FAF appends the port box) gives
        /// `iroh:<code>`.
        ValidateIPAddress = |lua, addr: String| {
            use std::net::ToSocketAddrs;
            let code_part = addr.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(addr.as_str(), |(code, _)| code);
            if let Some(id) = crate::net::transport::parse_code(code_part) {
                let code = format!("{}{}", crate::net::transport::IROH_PREFIX, crate::net::transport::format_code(id));
                return Ok(Loose::<Either<String, bool>>::new(LuaValue::String(lua.create_string(code)?)));
            }
            let found = addr.to_socket_addrs().ok().and_then(|mut a| {
                let all: Vec<_> = a.by_ref().collect();
                all.iter().find(|a| a.is_ipv4()).or(all.first()).copied()
            });
            Ok(Loose::new(match found {
                Some(a) => LuaValue::String(lua.create_string(a.to_string())?),
                None => LuaValue::Boolean(false),
            }))
        })?;
    lua_fn!(lua, global, CopyToClipboard = |_, text: LuaString| Ok(crate::ui::clipboard::set(&text.to_string_lossy())))?;
    lua_fn!(lua, global, GpgNetActive = |_, ()| Ok(false))?;
    lua_fn!(lua, global, GpgNetSend = |_, _args: LuaMultiValue| Ok(()))?;
    lua_fn!(lua, global,
        /// Whether moho's command line (`/nomusic`, `/faction 2`, ...: the
        /// process arguments, `/name` or `--name`) has the option.
        HasCommandLineArg = |_, name: String| Ok(command_line_arg(&name).is_some()))?;
    lua_fn!(lua, global,
        /// The (up to `max`) values after `/name`, until the next option; false
        /// when it isn't there.
        GetCommandLineArg = |lua, (name, max): (String, Option<usize>)| {
            let Some(values) = command_line_arg(&name) else { return Ok(Loose::<Either<Vec<String>, bool>>::new(LuaValue::Boolean(false))) };
            let t = lua.create_table()?;
            for v in values.into_iter().take(max.unwrap_or(usize::MAX)) {
                t.raw_push(v)?;
            }
            Ok(Loose::new(LuaValue::Table(t)))
        })?;
    lua_fn!(lua, global, DebugFacilitiesEnabled = |_, ()| Ok(false))?;
    lua_fn!(lua, global,
        GetSpecialFilePath = |_, (profile, name, kind): (String, String, String)| Ok(special_file_path(&profile, &name, &kind)))?;
    lua_fn!(lua, global, GetSpecialFolder = |_, kind: String| Ok(special_folder(&kind).map(|(dir, _)| dir)))?;
    lua_fn!(lua, global, GetSpecialFiles = |lua, kind: String| special_files(lua, &kind))?;
    lua_fn!(lua, global,
        GetSpecialFileInfo = |lua, (profile, name, kind): (String, String, String)| special_file_info(lua, &profile, &name, &kind))?;
    lua_fn!(lua, global,
        /// Deletes it (moho moves it to the recycle bin).
        RemoveSpecialFile = |_, (profile, name, kind): (String, String, String)| {
            if let Some(path) = special_file_path(&profile, &name, &kind)
                && let Err(e) = std::fs::remove_file(crate::engine::replay::find_file(path.as_ref())) {
                tracing::warn!(target: "lua", path, "RemoveSpecialFile: {e}");
            }
            Ok(())
        })?;
    lua_fn!(lua, global, LaunchReplaySession = |lua, filename: String| launch_replay_session(lua, &filename))?;
    lua_fn!(lua, global,
        /// Saved games aren't supported: every load fails as moho's does for a
        /// file it can't open, which FAF reports in its own dialog.
        LoadSavedGame = |_, _args: LuaMultiValue| Ok((false, "CantOpen")))?;
    lua_fn!(lua, global, WorldIsLoading = |_, ()| Ok(false))?;
    lua_fn!(lua, global,
        /// The modifier keys, by name or Windows virtual key (the renderer sets
        /// them each frame, `UserVm::set_modifiers`); other keys read as up.
        IsKeyDown = |lua, key: Loose<Either<String, f64>>| {
            let name = match &*key {
                LuaValue::Integer(16) => "shift".to_string(),
                LuaValue::Integer(17) => "ctrl".to_string(),
                LuaValue::Integer(18) => "alt".to_string(),
                LuaValue::Number(n) if *n == 16.0 => "shift".to_string(),
                LuaValue::Number(n) if *n == 17.0 => "ctrl".to_string(),
                LuaValue::Number(n) if *n == 18.0 => "alt".to_string(),
                LuaValue::String(s) => s.to_string_lossy().to_lowercase(),
                _ => return Ok(false),
            };
            let global = match name.as_str() {
                "shift" => "__engine_shift",
                "ctrl" | "control" => "__engine_ctrl",
                "alt" | "menu" => "__engine_alt",
                _ => return Ok(false),
            };
            Ok(lua.globals().raw_get::<Option<bool>>(global)?.unwrap_or(false))
        })?;
    lua_fn!(lua, global,
        /// A game is running (not the main menu).
        SessionIsActive = |lua, ()| Ok(lua.globals().raw_get::<Option<String>>("__engine_ui_state")?.as_deref() == Some("game")))?;
    lua_fn!(lua, global, SessionIsReplay = |lua, ()| Ok(lua.globals().raw_get::<Option<bool>>("__engine_replay")?.unwrap_or(false)))?;
    lua_fn!(lua, global, SessionIsMultiplayer = |_, ()| Ok(crate::net::core::current().is_some_and(|n| !n.session_clients().is_empty())))?;
    lua_fn!(lua, global,
        /// A game (not the main menu) that isn't networked: a skirmish or a
        /// replay starts again on this machine alone.
        SessionCanRestart = |lua, ()| Ok(lua.globals().raw_get::<Option<String>>("__engine_ui_state")?.as_deref() == Some("game")
            && !crate::net::core::current().is_some_and(|n| !n.session_clients().is_empty())))?;
    lua_fn!(lua, global,
        /// Starts the session again from its first tick (`SessionCanRestart`):
        /// the renderer closes this one and launches it anew
        /// (`UserVm::take_restart_request`). Ignored where it can't.
        RestartSession = |lua, ()| {
            let can: bool = lua.globals().get::<LuaFunction>("SessionCanRestart")?.call(())?;
            if can {
                lua.globals().raw_set("__engine_restart_request", true)?;
            } else {
                tracing::warn!(target: "lua", "RestartSession: not in a game this machine can restart alone");
            }
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// The game ended (the sim's `EndGame`, `UserVm::set_game_over`) or this
        /// player left it (`SessionEndGame`).
        SessionIsGameOver = |lua, ()| Ok(lua.globals().raw_get::<Option<bool>>("__engine_game_over")?.unwrap_or(false)))?;
    lua_fn!(lua, global,
        /// This client has no army of its own (watching, or a replay).
        IsObserver = |lua, ()| Ok(lua.globals().raw_get::<Option<bool>>("__engine_observer")?.unwrap_or(false)))?;
    lua_fn!(lua, global, GetUIControlsAlpha = |_, ()| Ok(1.0f64))?;
    lua_fn!(lua, global, SetUIControlsAlpha = |_, _alpha: Loose<f64>| Ok(()))?;
    lua_fn!(lua, global, AddConsoleOutputReciever = |_, _receiver: Loose<LuaFunction>| Ok(()))?;
    lua_fn!(lua, global, RemoveConsoleOutputReciever = |_, _receiver: Loose<LuaFunction>| Ok(()))?;
    lua.set_named_registry_value("user.focus_army", -1)?;
    lua_fn!(lua, global,
        /// The army the player sees as, 1-based (`--player`, else observer: -1).
        GetFocusArmy = |lua, ()| lua.named_registry_value::<i64>("user.focus_army"))?;
    lua_fn!(lua, global,
        /// `index` is 0-based (-1: observer); `GetFocusArmy` is 1-based.
        SetFocusArmy = |lua, a: i64| lua.set_named_registry_value("user.focus_army", if a < 0 { -1 } else { a + 1 }))?;
    crate::lua_api::script::exec(
        lua,
        r#"
        -- The current profile's options, where FAF's options dialog keeps
        -- them (`Prefs.SetToCurrentProfile('options', ...)`; none set: FAF
        -- falls back to each option's default from optionslogic.lua).
        function GetOptions(key)
            local current = GetPreference('profile.current')
            if not current then return key == nil and {} or nil end
            local opts = 'profile.profiles.' .. current .. '.options'
            -- One option: read just it (a preference comes out as a copy).
            if key then return GetPreference(opts .. '.' .. key) end
            return GetPreference(opts, {})
        end
        -- Sound globals are ui/sound.rs; the tutorial's voice-overs aren't.
        function PlayTutorialVO(...) end
        -- The Anti-Aliasing choices besides Off (FAF's presets pick the middle
        -- one for High, the last for Ultra): 1 is SMAA (render/smaa.rs);
        -- MSAA counts (2, 4, 8) join it once MSAA is drawn.
        function GetAntiAliasingOptions() return { 1 } end
        -- Range overlays (the military / intel rings): filters defined by
        -- name, then the active set chosen. Kept, not drawn yet.
        local overlayFilters, activeOverlays, overlayVersion = {}, {}, 0
        function SetOverlayFilter(name, categories, normal, select, rollover, innerMin, innerMax, outerMin, outerMax)
            overlayFilters[name] = { Categories = categories, NormalColor = normal, SelectColor = select, RolloverColor = rollover,
                Inner = { innerMin, innerMax }, Outer = { outerMin, outerMax } }
            overlayVersion = overlayVersion + 1
        end
        function SetOverlayFilters(names) activeOverlays = names or {} overlayVersion = overlayVersion + 1 end
        function __engine_overlay_version() return overlayVersion end
        -- The active filters for the renderer: colours, thicknesses, which
        -- of `ids` (blueprints) each covers, and which range kinds (OVERLAY<kind>
        -- categories) it takes in: those whose every blueprint it covers, so
        -- the anti-air filter doesn't also draw a destroyer's guns.
        function __engine_overlay_filters(ids, kinds)
            local out = {}
            for _, name in activeOverlays do
                local f = overlayFilters[name]
                if f and f.Categories then
                    local members = {}
                    for i, id in ids do members[i] = EntityCategoryContains(f.Categories, id) end
                    local mask = 0
                    for k, kind in kinds do
                        local cat = categories['OVERLAY' .. kind]
                        local any, all = false, true
                        if cat then
                            for i, id in ids do
                                if EntityCategoryContains(cat, id) then
                                    any = true
                                    if not members[i] then all = false break end
                                end
                            end
                        end
                        if any and all then mask = mask + 2 ^ (k - 1) end
                    end
                    table.insert(out, { name = name, normal = f.NormalColor, select = f.SelectColor, rollover = f.RolloverColor,
                        inner = f.Inner, outer = f.Outer, members = members, kinds = mask })
                end
            end
            return out
        end
        -- Units added to the selection from the interface (a transport's
        -- carried units in the construction panel), by entity id. Kept;
        -- orders don't go to them yet.
        local extraSelect = {}
        function AddToSessionExtraSelectList(unit) if unit then extraSelect[unit:GetEntityId()] = unit end end
        function RemoveFromSessionExtraSelectList(unit) if unit then extraSelect[unit:GetEntityId()] = nil end end
        function ClearSessionExtraSelectList() extraSelect = {} end
        -- Console commands: the interface's own run here, the engine's go to
        -- the renderer (`ui::console`).
        function ConExecute(command) __engine_con_execute(command) end
        function ConExecuteSave(command) __engine_con_execute(command) end
        function ConTextMatches(text) return {} end
        function GetSimTicksPerSecond() return 10 end
        -- Game speed as moho's step (-10..10, 0 normal; rate 10^(step/10)):
        -- the engine keeps `__engine_game_speed` current and takes requests
        -- (`UserVm::set_game_speed` / `take_speed_request`).
        function GetGameSpeed() return rawget(_G, '__engine_game_speed') or 0 end
        function SetGameSpeed(speed) rawset(_G, '__engine_speed_request', math.max(-10, math.min(10, math.floor(speed or 0)))) end
        function GameTick() return rawget(_G, '__engine_ui_tick') or 0 end
        function GameTime() return (rawget(_G, '__engine_ui_tick') or 0) / 10 end
        function GetGameTime() return FormatTime((rawget(_G, '__engine_ui_tick') or 0) / 10) end
        function GetGameTimeSeconds() return (rawget(_G, '__engine_ui_tick') or 0) / 10 end
        function FormatTime(seconds)
            local s = math.floor(seconds or 0)
            return string.format('%02d:%02d:%02d', math.floor(s / 3600), math.floor(s / 60) % 60, s % 60)
        end
        -- The rate the sim actually runs at (1 = normal).
        function GetSimRate() return rawget(_G, '__engine_sim_rate') or 0 end
        -- The player's selection, set by the engine from the renderer's
        -- selection (`UserVm::set_selection`): a new table each call, nil
        -- when nothing is selected, as moho's. FAF adds to the table it gets
        -- (`AppendSetToSelection`) and tells calls apart by table identity
        -- (selection sets' double tap).
        -- Interface ground decals (`UserDecal`: FAF's range rings under the
        -- cursor): a texture, a centre and a size, drawn on the terrain.
        -- The engine reads them when `version` moves (`UserVm::user_decals`).
        do
            local decals, nextId, version = {}, 0, 0
            local function bump() version = version + 1 end
            function _c_CreateDecal(self)
                nextId = nextId + 1
                rawset(self, '_decal', nextId)
                decals[nextId] = { x = 0, y = 0, z = 0, sx = 1, sy = 1, sz = 1 }
                bump()
            end
            local m = moho.userDecal_methods
            local function of(self) return decals[rawget(self, '_decal') or 0] end
            m.SetTexture = function(self, tex) local d = of(self) if d then d.texture = tex bump() end end
            m.SetPosition = function(self, v) local d = of(self) if d and v then d.x, d.y, d.z = v[1] or v.x, v[2] or v.y, v[3] or v.z bump() end end
            m.SetPositionByScreen = function(self, v)
                local p = v and UnProject(nil, { v[1] or v.x, v[2] or v.y })
                if p then m.SetPosition(self, p) end
            end
            m.SetScale = function(self, v) local d = of(self) if d and v then d.sx, d.sy, d.sz = v[1] or v.x, v[2] or v.y, v[3] or v.z bump() end end
            m.Destroy = function(self) local id = rawget(self, '_decal') if id and decals[id] then decals[id] = nil bump() end end
            -- (version, { texture, x, y, z, sx, sy, sz, ... }) of the textured ones, id order.
            function __engine_user_decals()
                local out = {}
                for id = 1, nextId do
                    local d = decals[id]
                    if d and d.texture then
                        for _, v in { d.texture, d.x, d.y, d.z, d.sx, d.sy, d.sz } do table.insert(out, v) end
                    end
                end
                return version, out
            end
        end
        -- The command-graph waypoint under the mouse (the engine sets it each
        -- frame), nil when none.
        function GetHighlightCommand() return rawget(_G, '__engine_highlight_command') end
        -- Build templates (`UIBuildTemplate`: { width, height, { bp, id, x, z },
        -- ... }, x / z offsets from the cursor). The active one is placed in
        -- build mode, every structure at once (render/frame.rs); FAF mutates
        -- what it gets (rotating), so copies go both ways.
        local function copy_template(t)
            if type(t) ~= 'table' then return {} end
            local out = { t[1], t[2] }
            for i = 3, table.getn(t) do
                local e = t[i]
                if type(e) == 'table' then out[i] = { e[1], e[2], e[3], e[4] } end
            end
            return out
        end
        function SetActiveBuildTemplate(t) rawset(_G, '__engine_build_template', copy_template(t)) end
        function GetActiveBuildTemplate() return copy_template(rawget(_G, '__engine_build_template')) end
        function ClearBuildTemplates() rawset(_G, '__engine_build_template', nil) end
        -- The selected structures as a template, offsets from the first one
        -- (inferred: FAF re-centres on the first entry's footprint itself).
        function GenerateBuildTemplateFromSelection()
            local units = EntityCategoryFilterDown(categories.STRUCTURE, GetSelectedUnits() or {})
            if not units[1] then
                ClearBuildTemplates()
                return
            end
            local origin = units[1]:GetPosition()
            local t = { 0, 0 }
            local minX, maxX, minZ, maxZ = 0, 0, 0, 0
            for i, u in units do
                local p = u:GetPosition()
                local x, z = p[1] - origin[1], p[3] - origin[3]
                minX, maxX = math.min(minX, x), math.max(maxX, x)
                minZ, maxZ = math.min(minZ, z), math.max(maxZ, z)
                table.insert(t, { u:GetUnitId(), i, x, z })
            end
            t[1], t[2] = math.ceil(maxX - minX), math.ceil(maxZ - minZ)
            SetActiveBuildTemplate(t)
        end
        -- The focus army's idle engineers (not commanders) and factories (gates
        -- too: FAF filters those out itself), nil when none (avatars.lua).
        local function idle(category)
            local units = {}
            for _, id in __engine_idle_units(category) do
                local u = GetUnitById(id)
                if u then table.insert(units, u) end
            end
            if units[1] then return units end
        end
        function GetIdleEngineers() return idle('ENGINEER - COMMAND') end
        function GetIdleFactories() return idle('FACTORY STRUCTURE') end
        -- The game world is running (the interface exists only then).
        function WorldIsPlaying() return true end
        -- Selects the player's units in a category expression, as the
        -- `UI_SelectByCategory` console command does (render/console.rs).
        function UISelectionByCategory(expression, addToCurSel, inViewFrustum, nearestToMouse, mustBeIdle)
            local flags = ''
            if addToCurSel then flags = flags .. '+add ' end
            if inViewFrustum then flags = flags .. '+inview ' end
            if nearestToMouse then flags = flags .. '+nearest ' end
            if mustBeIdle then flags = flags .. '+idle ' end
            ConExecute('UI_SelectByCategory ' .. flags .. expression)
        end
        function GetSelectedUnits()
            local sel = rawget(_G, '__engine_selection')
            if not sel or not sel[1] then return nil end
            local copy = {}
            for i, u in ipairs(sel) do copy[i] = u end
            return copy
        end
        -- Selecting from the interface: the engine adopts it as the
        -- player's selection (`UserVm::take_selection_request`).
        function SelectUnits(units) rawset(_G, '__engine_select_request', units or {}) end
        function AddSelectUnits(units)
            local all = {}
            for _, u in rawget(_G, '__engine_selection') or {} do table.insert(all, u) end
            for _, u in units or {} do table.insert(all, u) end
            rawset(_G, '__engine_select_request', all)
        end
        -- The session's scenario (the engine fills __engine_session when it
        -- knows the map; until then a minimal one). `Share` is the lobby's
        -- default: FAF's score panel looks it up and fails without it.
        function SessionGetScenarioInfo()
            return rawget(_G, '__engine_session') or { Options = { Share = 'ShareUntilDeath' }, size = {1024, 1024}, name = '', type = 'skirmish' }
        end
        function GetEconomyTotals()
            return rawget(_G, '__engine_economy') or {
                stored = {MASS = 0, ENERGY = 0}, maxStorage = {MASS = 0, ENERGY = 0}, income = {MASS = 0, ENERGY = 0},
                lastUseRequested = {MASS = 0, ENERGY = 0}, lastUseActual = {MASS = 0, ENERGY = 0}, reclaimed = {MASS = 0, ENERGY = 0},
            }
        end
        -- Alliances between army indices, from the sim's (`UserVm::set_alliances`).
        local function alliance(a, b)
            if a == b then return 'Ally' end
            local row = (rawget(_G, '__engine_alliances') or {})[a]
            return row and row[b] or 'Enemy'
        end
        function IsAlly(a, b) return alliance(a, b) == 'Ally' end
        function IsEnemy(a, b) return alliance(a, b) == 'Enemy' end
        function IsNeutral(a, b) return alliance(a, b) == 'Neutral' end
        function GetArmiesTable()
            local armies = rawget(_G, '__engine_armies') or {}
            return { numArmies = table.getn(armies), focusArmy = GetFocusArmy(), armiesTable = armies }
        end
        function GetSessionClients()
            local net = __net_session_clients()
            if net then return net end
            local out = {}
            for i, a in rawget(_G, '__engine_armies') or {} do
                if a.human then
                    table.insert(out, { name = a.nickname, uid = tostring(i), connected = true, ['local'] = true, ejectedBy = {}, authorizedCommandSources = { table.getn(out) + 1 } })
                end
            end
            -- Watching (no human army): moho still lists the local client,
            -- an observer with no command source.
            if table.getn(out) == 0 then
                table.insert(out, { name = 'Observer', uid = '0', connected = true, ['local'] = true, ejectedBy = {}, authorizedCommandSources = {} })
            end
            return out
        end
        -- Command sources (1-based): each human army's player in army
        -- order, then observers (the sim's numbering, `command_source.rs`).
        function SessionGetCommandSourceNames()
            local names = {}
            for _, a in rawget(_G, '__engine_armies') or {} do
                if a.human then table.insert(names, a.nickname) end
            end
            return names
        end
        function SessionGetLocalCommandSource()
            local source, focus = 0, GetFocusArmy()
            for i, a in rawget(_G, '__engine_armies') or {} do
                if a.human then
                    source = source + 1
                    if i == focus then return source end
                end
            end
            return source + 1
        end
        -- Engine-populated globals (/engine/Library.lua).
        rawset(_G, '__EngineStats', rawget(_G, '__EngineStats') or { Children = {} })
        rawset(_G, 'LaunchDir', rawget(_G, 'LaunchDir') or '')
        -- Input capture: controls that take all input (modal dialogs), last
        -- added first.
        local captures = {}
        function AddInputCapture(control) table.insert(captures, control) end
        function RemoveInputCapture(control)
            for i = table.getn(captures), 1, -1 do
                if captures[i] == control then table.remove(captures, i) end
            end
        end
        function GetInputCapture() return captures[table.getn(captures)] end
        function AnyInputCapture() return table.getn(captures) > 0 end
        function FlushEvents() end
        function GetMouseScreenPos() return { rawget(_G, '__engine_mouse_x') or 0, rawget(_G, '__engine_mouse_y') or 0 } end
        -- A unit's rollover table, as moho builds it (FA's exe 0x847d40):
        -- the economy as whole numbers per second (the per-tick rate times
        -- 10, truncated), the rest as the unit's getters give them. Not
        -- copied: moho's 'unknown' blueprint and hidden health for units
        -- the player can't identify (the interface only has units it sees).
        local rolloverEcon = { 'energyConsumed', 'massConsumed', 'energyRequested', 'massRequested', 'energyProduced', 'massProduced' }
        local function rolloverInfo(unit)
            local info = { blueprintId = unit:GetUnitId(), health = unit:GetHealth(), maxHealth = unit:GetMaxHealth() }
            info.kills = unit:GetStat('KILLS', 0).Value
            local econ = unit:GetEconData()
            for _, k in rolloverEcon do
                info[k] = math.floor(econ[k] or 0)
            end
            for k, v in unit:GetMissileInfo() do
                info[k] = v
            end
            info.fuelRatio = unit:GetFuelRatio()
            info.shieldRatio = unit:GetShieldRatio()
            info.workProgress = unit:GetWorkProgress()
            info.userUnit = unit
            info.entityId = unit:GetEntityId()
            local army = unit:GetArmy()
            info.armyIndex = army - 1
            local a = (rawget(_G, '__engine_armies') or {})[army]
            info.teamColor = a and a.color
            local name = unit:GetCustomName()
            if name and name ~= '' then info.customName = name end
            return info
        end
        -- The unit under the mouse (the engine sets `__engine_rollover` each
        -- frame, nil over the interface or empty ground), as moho's binding
        -- builds it (FA's exe 0x848470): the unit it works on as `focus`
        -- (`focusUpgrade` while upgrading), else the unit it guards as
        -- `guarded`, one level deep. Not copied: moho's brief `focus` for a
        -- unit it targets (blueprint and health) and `focusProp` for a prop
        -- it works on; the interface has neither.
        function GetRolloverInfo()
            local id = rawget(_G, '__engine_rollover')
            local unit = id and GetUnitById(id)
            if not unit or unit:IsDead() then return nil end
            local info = rolloverInfo(unit)
            local focus = unit:GetFocus()
            if focus then
                local queue = unit:GetCommandQueue()
                local upgrading = queue[1] and queue[1].type == 'Upgrade'
                info[upgrading and 'focusUpgrade' or 'focus'] = rolloverInfo(focus)
            else
                local guarded = unit:GetGuardedEntity()
                if guarded then info.guarded = rolloverInfo(guarded) end
            end
            return info
        end
        -- Moho's interface states: 'frontend' (the main menu) or 'game'.
        rawset(_G, '__engine_ui_state', 'game')
        function GetCurrentUIState() return rawget(_G, '__engine_ui_state') end
        -- The renderer closes the window (`UserVm::exit_requested`).
        function ExitApplication() rawset(_G, '__engine_exit_request', true) end
        -- Session control: the engine keeps `__engine_paused` current; pause
        -- requests are taken by the renderer (`UserVm::take_pause_request`).
        rawset(_G, '__engine_paused', false)
        function SessionIsPaused() return rawget(_G, '__engine_paused') end
        function SessionRequestPause() rawset(_G, '__engine_pause_request', true) end
        function SessionResume() rawset(_G, '__engine_pause_request', false) end
        function SessionIsBeingRecorded() return false end
        function SessionIsObservingAllowed() return true end
        -- In-game chat (`net::core`): `SessionSendChatMessage([clients,]
        -- message)`; received messages go to FAF's gamemain.ReceiveChat.
        -- FAF's disconnect dialog's Eject button (a vote; `net::lockstep`).
        function EjectSessionClient(index) __net_eject(index) end
        function SessionSendChatMessage(client, message)
            if message == nil then client, message = nil, client end
            if type(client) == 'number' then client = { client } end
            __net_chat_send(client, message)
        end
        -- This player leaves the session (FAF's score screen, quitting):
        -- the sim stops here and a networked game is told we left
        -- (`UserVm::take_session_left`). A session's end is when moho hands
        -- the interface its score data, which FAF's score screen waits for
        -- (`hotstats.scoreData`); ours fills it in here at `SessionEndGame`,
        -- whichever end took it, in the shape of FAF's sim score table.
        function SessionEndGame()
            rawset(_G, '__engine_game_over', true)
            rawset(_G, '__engine_session_left', true)
            local scoreData = import('/lua/ui/dialogs/hotstats.lua').scoreData
            if scoreData.interval and scoreData.current and scoreData.history then return end
            local current = {}
            for index, info in GetArmiesTable().armiesTable do
                if not info.civilian then
                    current[index] = {
                        name = info.nickname,
                        general = { score = 0, kills = { count = 0, mass = 0, energy = 0 },
                            built = { count = 0, mass = 0, energy = 0 },
                            lost = { count = 0, mass = 0, energy = 0 },
                            currentunits = 0, currentcap = 0 },
                        units = { cdr = { kills = 0, built = 0, lost = 0 }, land = { kills = 0, built = 0, lost = 0 },
                            naval = { kills = 0, built = 0, lost = 0 }, air = { kills = 0, built = 0, lost = 0 },
                            experimental = { kills = 0, built = 0, lost = 0 }, structures = { kills = 0, built = 0, lost = 0 } },
                        resources = {
                            massin = { total = 0, rate = 0, reclaimed = 0, reclaimRate = 0 },
                            massout = { total = 0, rate = 0, excess = 0 },
                            energyin = { total = 0, rate = 0, reclaimed = 0, reclaimRate = 0 },
                            energyout = { total = 0, rate = 0, excess = 0 },
                            storage = { storedMass = 0, storedEnergy = 0, maxMass = 0, maxEnergy = 0 } },
                    }
                end
            end
            scoreData.interval = 30
            scoreData.current = current
            scoreData.history = {}
        end
        -- Moho returns to the main menu (the score screen's Continue): the
        -- renderer tears the game's interface down and starts the front end
        -- (`UserVm::take_menu_request`).
        function ExitGame()
            SessionEndGame()
            rawset(_G, '__engine_menu_request', true)
        end
        -- The focus army's synced overflow flag (moho: false for an
        -- observer; on until the first beat, as a new army's is).
        function GetResourceSharing()
            local focus = GetFocusArmy()
            if focus < 1 then return false end
            local flags = rawget(_G, '__engine_resource_sharing')
            local on = flags and flags[focus]
            if on == nil then return true end
            return on
        end
        function GetFrontEndData(key) return nil end
        function SetFrontEndData(key, data) end
        -- Unit toggles as the panels query them (defaults until they're
        -- bridged to the sim: return fire, not paused, script bits off).
        -- Unit toggles and lists over the units' details (selected units).
        local function detail(u) return rawget(_G, '__engine_unit_data')[u._id] end
        -- Any of the units in auto mode / auto-surfacing.
        function GetIsAutoMode(units)
            for _, u in units or {} do local d = detail(u) if d and d.autoMode then return true end end
            return false
        end
        function GetIsAutoSurfaceMode(units)
            for _, u in units or {} do local d = detail(u) if d and d.autoSurface then return true end end
            return false
        end
        -- -1 all submerged, 1 none, 0 a mix (or no units).
        function GetIsSubmerged(units)
            local sub, surf = false, false
            for _, u in units or {} do
                local d = detail(u)
                if d and d.submerged then sub = true else surf = true end
            end
            if sub and not surf then return -1 elseif surf and not sub then return 1 end
            return 0
        end
        -- Settings for the sim, like orders (`__engine_ui_orders`).
        function SetAutoMode(units, on)
            table.insert(rawget(_G, '__engine_ui_orders'), { units = units, toggle = 'auto', value = on and 1 or 0 })
        end
        function SetAutoSurfaceMode(units, on)
            table.insert(rawget(_G, '__engine_ui_orders'), { units = units, toggle = 'autosurface', value = on and 1 or 0 })
        end
        -- Units carried by / assisting any of `units`, each once.
        local function gather(units, field)
            local out, seen = {}, {}
            for _, u in units or {} do
                local d = detail(u)
                for _, id in (d and d[field]) or {} do
                    if not seen[id] then
                        seen[id] = true
                        local o = GetUnitById(id)
                        if o then table.insert(out, o) end
                    end
                end
            end
            return out
        end
        function GetAttachedUnitsList(units) return gather(units, 'cargo') end
        function GetAssistingUnitsList(units) return gather(units, 'assisters') end
        function GetValidAttackingUnits(units) return units end
        -- Orders from the interface: queued for the engine, which turns them
        -- into player commands for the selection (`UserVm::take_orders`).
        local orders = {}
        rawset(_G, '__engine_ui_orders', orders)
        function GetUnitCommandFromCommandCap(cap) return 'UNITCOMMAND_' .. string.gsub(cap, '^RULEUCC_', '') end
        function IssueCommand(command, a, b) table.insert(orders, { command = command, arg = a, clear = b }) end
        function IssueUnitCommand(units, command, a, b) table.insert(orders, { units = units, command = command, arg = a, clear = b }) end
        function IssueUnitCommandToUnit(unit, command, a, b) table.insert(orders, { units = { unit }, command = command, arg = a, clear = b }) end
        function IssueBlueprintCommand(command, bp, count, clear) table.insert(orders, { command = command, bp = bp, count = count, clear = clear }) end
        function IssueBlueprintCommandToUnits(units, command, bp, count, clear) table.insert(orders, { units = units, command = command, bp = bp, count = count, clear = clear }) end
        function IssueBlueprintCommandToUnit(unit, command, bp, count, clear) table.insert(orders, { units = { unit }, command = command, bp = bp, count = count, clear = clear }) end
        -- Unit settings (the order panel's toggles), sent like orders.
        local fireStates = { ReturnFire = 0, HoldFire = 1, HoldGround = 2 }
        function SetFireState(units, state) table.insert(orders, { units = units, toggle = 'fire', value = fireStates[state] or tonumber(state) or 0 }) end
        ToggleFireState = SetFireState
        function SetPaused(units, paused) table.insert(orders, { units = units, toggle = 'pause', value = paused and 1 or 0 }) end
        function SetPausedOfUnit(unit, paused) SetPaused({ unit }, paused) end
        -- `state` is the bit's current state: it flips.
        function ToggleScriptBit(units, bit, state) table.insert(orders, { units = units, toggle = 'bit', bit = bit, value = (not state) and 1 or 0 }) end
        function IssueDockCommand(clear) table.insert(orders, { command = 'UNITCOMMAND_Dock', clear = clear }) end
        -- Key maps (key name -> action), the most recently added first.
        local keymaps = {}
        function IN_AddKeyMapTable(map) table.insert(keymaps, map) end
        function IN_RemoveKeyMapTable(map)
            for i = table.getn(keymaps), 1, -1 do
                if keymaps[i] == map then table.remove(keymaps, i) end
            end
        end
        function IN_ClearKeyMap() keymaps = {} end
        --- The action bound to the first of `names` found (newest map first).
        function __engine_key_action(names)
            for i = table.getn(keymaps), 1, -1 do
                for _, name in names do
                    local entry = keymaps[i][name]
                    if entry then
                        if type(entry) == 'table' then return entry.action end
                        return entry
                    end
                end
            end
            return nil
        end
        "#,
        "=[engine user strings]",
    )?;
    // Engine objects that only need to exist as handles for now: the world
    // UI provider (gamemain drives the game interface through it) and the
    // cursor (its textures are kept by `ui::cursor`).
    fn bind_handle<C: crate::engine::api::moho::MohoClass + 'static>(lua: &Lua, name: &'static str) -> LuaResult<()> {
        crate::lua_api::register_fn(
            lua,
            &lua.globals(),
            "",
            name,
            None,
            "Binds a new script object to an engine handle. The world UI provider is also kept by the engine: it drives the game interface through it (CreateGameInterface, loading dialog).",
            Status::Implemented,
            false,
            move |lua, (this, _rest): (Obj<C>, LuaMultiValue)| {
                this.raw_set(crate::engine::api::script_object::C_OBJECT, lua.create_userdata(crate::ui::controls::ControlHandle(u32::MAX))?)?;
                if C::CLASS == "WldUIProvider_methods" {
                    lua.set_named_registry_value("user.wld_provider", this.into_table())?;
                }
                Ok(())
            },
        )
    }
    bind_handle::<crate::engine::api::moho::WldUIProvider>(lua, "InternalCreateWldUIProvider")?;
    bind_handle::<crate::engine::api::moho::Cursor>(lua, "_c_CreateCursor")?;
    crate::lua_api::script::exec(
        lua,
        r#"
        local cursor = nil
        function SetCursor(c) cursor = c end
        function GetCursor() return cursor end
        "#,
        "=[engine cursor]",
    )?;
    lua_fn!(lua, global,
        #[lua(internal)]
        __engine_wld_provider = |lua, ()| lua.named_registry_value::<Option<Obj<crate::engine::api::moho::WldUIProvider>>>("user.wld_provider"))?;
    lua.set_app_data(PendingSimCallbacks::default());
    lua_fn!(lua, global,
        /// `SimCallback{Func, Args}`: converted now (FAF changes some Args tables
        /// after sending them) with the selection as it is now (when
        /// `with_selection`), and queued with the other orders so they reach the
        /// sim in the order given.
        SimCallback = |lua, (callback, with_selection): (Loose<SimCallbackRequest>, Option<bool>)| {
            let LuaValue::Table(callback) = callback.into_inner() else {
                tracing::warn!(target: "lua", "SimCallback: expected a {{Func, Args}} table");
                return Ok(());
            };
            let Some(func) = callback.raw_get::<Option<LuaString>>("Func")? else {
                tracing::warn!(target: "lua", "SimCallback: Func is not a string");
                return Ok(());
            };
            let with_selection = with_selection.unwrap_or(false);
            let units = if with_selection { lua.named_registry_value::<Option<Vec<u32>>>("user.selection")?.unwrap_or_default() } else { Vec::new() };
            let callback = crate::engine::sim::input::SimCallback {
                func: func.to_string_lossy(),
                args: crate::engine::sim::lua_data::LuaData::from_lua(&callback.raw_get::<LuaValue>("Args")?),
                with_selection,
            };
            let index = {
                let mut pending = lua.app_data_mut::<PendingSimCallbacks>().expect("PendingSimCallbacks");
                pending.0.push((callback, units));
                pending.0.len()
            };
            let orders: LuaTable = lua.globals().raw_get("__engine_ui_orders")?;
            let entry = lua.create_table_with_capacity(0, 1)?;
            entry.raw_set("callback", index)?;
            orders.raw_push(entry)
        })?;
    lua_fn!(lua, global, GetTextureDimensions = |lua, (path, _scale): (String, Option<f64>)| Ok(crate::ui::controls::texture_dimensions(lua, &path)))?;
    Ok(())
}

/// `SimCallback`s given since the orders were last taken, with the
/// selection each carries (its entry in `__engine_ui_orders` holds the
/// 1-based index here).
#[derive(Default)]
pub struct PendingSimCallbacks(pub Vec<(crate::engine::sim::input::SimCallback, Vec<u32>)>);


/// Every method FAF documents for a user class (`/engine/User/*.lua`
/// annotations, from the VFS) that isn't implemented gets a stub that warns
/// once and returns nil, so FAF's interface keeps building around what's
/// missing (the warnings list it). Run after the real methods exist.
pub fn register_method_stubs(lua: &Lua) -> LuaResult<()> {
    let Some(vfs) = lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().map(|v| v.clone()) else { return Ok(()) };
    let files: Vec<String> = {
        let Ok(v) = vfs.read() else { return Ok(()) };
        v.sorted_paths().iter().filter(|p| p.starts_with("/engine/user/") && p.ends_with(".lua")).cloned().collect()
    };
    for path in files {
        let Ok(bytes) = vfs.read().map_err(|_| ()).and_then(|v| v.read(&path).map_err(|_| ())) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let class = crate::lua_api::compare::UNTAGGED
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, c)| c.to_string())
            .or_else(|| text.lines().find_map(|l| l.trim().strip_prefix("---@class moho.").map(|c| c.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')).next().unwrap_or("").to_string())));
        let Some(class) = class.filter(|c| !c.is_empty()) else { continue };
        let Ok(table) = crate::engine::api::moho::class_table(lua, &class) else { continue };
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("function ") else { continue };
            let Some((_, method)) = rest.split_once(':') else { continue };
            let method: String = method.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if method.is_empty() || has_method(&table, &method)? {
                continue;
            }
            let (c, m) = (class.clone(), method.clone());
            let warned = std::cell::Cell::new(false);
            // Names from files at run time: recorded as owned names, and the
            // function named through the process-wide interned names.
            let func = lua.create_function(move |_, _: LuaMultiValue| {
                if !warned.replace(true) {
                    tracing::warn!(target: "lua", "user engine method {c}:{m} is not implemented yet (returns nil)");
                }
                Ok(())
            })?;
            let luau = crate::lua_api::class::luau_name(&class).unwrap_or(&class);
            crate::lua_api::name_function(lua, &func, &format!("{luau}:{method}"))?;
            table.raw_set(method.as_str(), func)?;
            crate::lua_api::record_stub(lua, crate::lua_api::Scope::Class(class.clone().into()), method);
        }
    }
    Ok(())
}

/// `class` or one of its bases (nested at integer keys) has `name`.
fn has_method(class: &LuaTable, name: &str) -> LuaResult<bool> {
    if class.raw_get::<LuaValue>(name)? != LuaNil {
        return Ok(true);
    }
    for base in class.clone().sequence_values::<LuaTable>() {
        if has_method(&base?, name)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod command_line_tests {
    use super::command_line_arg_in;

    #[test]
    fn options_and_their_values() {
        let args: Vec<String> = ["--session", "/faction", "2", "--victory", "demoralization", "/nomusic"].iter().map(|s| s.to_string()).collect();
        assert_eq!(command_line_arg_in(&args, "/faction"), Some(vec!["2".to_string()]));
        assert_eq!(command_line_arg_in(&args, "/victory"), Some(vec!["demoralization".to_string()]));
        assert_eq!(command_line_arg_in(&args, "/nomusic"), Some(vec![]));
        assert_eq!(command_line_arg_in(&args, "/nofog"), None);
    }
}

#[cfg(test)]
mod special_file_tests {
    use super::*;

    #[test]
    fn replays_list_in_the_pickers_layout() {
        let lua = Lua::new();
        let t = special_files(&lua, "Replay").unwrap().expect("a table").into_inner();
        let dir: String = t.get("directory").unwrap();
        assert!(dir.ends_with("replays/"), "{dir}");
        assert_eq!(t.get::<String>("extension").unwrap(), "rtsreplay");
        assert!(t.get::<LuaTable>("files").is_ok());
        assert_eq!(special_file_path("Me", "game", "Replay").unwrap(), format!("{dir}Me/game.rtsreplay"), "the picker's fspec");
        assert!(special_files(&lua, "Nonsense").unwrap().is_none());
        assert!(special_file_info(&lua, "nobody here", "nothing", "Replay").unwrap().is_none());
    }

    #[test]
    fn launch_replay_session_takes_only_our_recordings_from_the_menu() {
        let lua = Lua::new();
        let dir = std::env::temp_dir().join(format!("rts-launch-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("ours.rtsreplay");
        let session = crate::engine::session::SessionConfig { scenario: "/maps/scmp_009/scmp_009_scenario.lua".into(), ..Default::default() };
        std::fs::write(&ours, crate::engine::replay::header("test", &session)).unwrap();
        let theirs = dir.join("old.rtsreplay");
        std::fs::write(&theirs, "12 0 0 1 2 0 - - - 5 - 0 - - -\n").unwrap();
        let (ours, theirs) = (ours.to_string_lossy().into_owned(), theirs.to_string_lossy().into_owned());

        lua.globals().raw_set("__engine_ui_state", "game").unwrap();
        assert!(!launch_replay_session(&lua, &ours).unwrap(), "not mid-game");
        lua.globals().raw_set("__engine_ui_state", "frontend").unwrap();
        assert!(!launch_replay_session(&lua, &theirs).unwrap(), "no header");
        assert!(!launch_replay_session(&lua, "/no/such/file.rtsreplay").unwrap());
        assert!(lua.globals().raw_get::<LuaValue>("__engine_launch_replay").unwrap().is_nil());
        assert!(launch_replay_session(&lua, &ours).unwrap());
        assert_eq!(lua.globals().raw_get::<String>("__engine_launch_replay").unwrap(), ours);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
