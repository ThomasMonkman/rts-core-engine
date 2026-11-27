//! User-layer engine globals (FAF's `/engine/User.lua`). Every one exists
//! from the start so scripts that capture them at load time work; the ones
//! not implemented yet raise a clear error when called (like the sim's
//! `api::pending`). Real implementations are registered after the stubs
//! and replace them.

use mlua::prelude::*;

use crate::lua_api::shapes::Shaped;
use crate::lua_api::{Obj, Status, lua_fn};
use crate::ui::session::UiState;
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

/// Moho's front-end data (`GetFrontEndData` / `SetFrontEndData`): kept by
/// the engine, not a Lua state, so it outlives the interface that set it
/// (FAF's campaign briefing, the replay a game restarts from). Interface
/// only: the sim never sees it.
static FRONT_END_DATA: std::sync::Mutex<BTreeMap<String, crate::engine::sim::lua_data::LuaData>> = std::sync::Mutex::new(BTreeMap::new());

fn front_end_data() -> std::sync::MutexGuard<'static, BTreeMap<String, crate::engine::sim::lua_data::LuaData>> {
    FRONT_END_DATA.lock().unwrap_or_else(|e| e.into_inner())
}

/// moho `LaunchReplaySession(filename)`: from the main menu, leaves the
/// recording for the renderer to start (`__engine_launch_replay`, taken by
/// `UserVm::take_replay_request`) and returns true; false (FAF says it
/// can't launch it) for a file that isn't one of our recordings with a
/// header, or during a game (a session can't be swapped for another yet).
/// Moho keeps the recording it launched as front-end data
/// `replay_filename`, which FAF's restart reads (`tabs.lua`
/// `RestartReplay`, `misckeyactions.lua` `RestartSessionOrReplay`).
fn launch_replay_session(lua: &Lua, filename: &str) -> LuaResult<bool> {
    if crate::ui::session::state(lua).ui_state != UiState::Frontend {
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
    if let Some(why) = crate::engine::replay::unplayable(&text) {
        tracing::warn!(target: "lua", filename, "LaunchReplaySession: {why}");
        return Ok(false);
    }
    let header = crate::engine::replay::is_recording(&text).then(|| crate::engine::replay::read_header(&text));
    if !matches!(header, Some(Ok(crate::engine::replay::Header { session: Some(_), .. }))) {
        tracing::warn!(target: "lua", filename, "LaunchReplaySession: not a recording of this engine's with its session");
        return Ok(false);
    }
    lua.globals().raw_set("__engine_launch_replay", path.to_string_lossy())?;
    front_end_data().insert("replay_filename".into(), crate::engine::sim::lua_data::LuaData::string(filename));
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
/// `units` has (sorted), and what every one of them can build, or will once the
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
        let mut o = own.unwrap_or_else(|| CategorySet::empty(domain.entries.len()));
        for id in &restricted {
            if let Ok(cat) = domain.parse(id) {
                o = o.difference(&cat);
            }
        }
        own_sets.push(o);
    }
    // Orders are what any unit has; what can be built is what every unit can
    // build (the real game: engineer + factory, or engineer + tank, offer
    // nothing; `probe_ui_hover` build).
    let (mut orders, mut toggles) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    let empty = CategorySet::empty(domain.entries.len());
    let mut buildable: Option<CategorySet> = None;
    let mut meet = |set: &CategorySet| buildable = Some(buildable.as_ref().map_or_else(|| set.clone(), |b| b.intersection(set)));
    for e in used.values().filter(|e| e.any) {
        let c = e.commands.as_ref().expect("only blueprints that exist are used");
        orders.extend(c.orders.iter().cloned());
        toggles.extend(c.toggles.iter().cloned());
        if e.plain {
            meet(c.buildable.as_ref().unwrap_or(&empty));
        }
    }
    for o in &own_sets {
        meet(o);
    }
    Ok((orders.into_iter().collect(), toggles.into_iter().collect(), EntityCategory(buildable.unwrap_or(empty))))
}

/// `loadstring`, on its own so it can be tested without the whole layer.
fn register_loadstring(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// Compiles `code` (SupCom Lua, translated as every game script is)
        /// into a function, or returns nil and the compile error. Text only:
        /// moho also loads its own bytecode, which Luau can't. FAF removes
        /// it from the sim (`SimHooks.lua`); interface mods use it.
        loadstring = |lua, (code, name): (LuaString, Option<String>)| {
            let code = code.to_string_lossy();
            let translated = crate::engine::lua_translation::translate(&code);
            match lua.load(translated).set_name(name.unwrap_or(code)).set_mode(mlua::ChunkMode::Text).into_function() {
                Ok(f) => Ok((Some(f), None)),
                Err(e) => Ok((None, Some(e.to_string()))),
            }
        })
}

/// `SetFocusArmy(index)` (the user function and the console command, which
/// FAF's score board sends): view the game as army `index` (0-based; -1 every
/// army). As moho's `CWldSession::RequestFocusArmy` (MohoEngine.dll
/// 0x10458d20): an index outside -1 .. armies - 1 is refused; then
/// `ValidateFocusArmyRequest` (0x10458da0) allows it when the session is a
/// replay (no command source), cheats are on, the army is one this client
/// commands, the game is over, or this client commands no army still in the
/// game (an observer, or a defeated player; moho also wants the session to
/// allow observers, which ours always do). So a player in a live game keeps
/// their own army's view.
///
/// It's the view's alone: the sim, its Lua and the command stream don't
/// hear of it (moho tells its sim driver, whose `NoteFocusArmyChanged` only
/// fills the local `Sync`). The next `Sync` the interface gets carries
/// `FocusArmyChanged` (`UserVm::sync`), as moho's would, so FAF's panels
/// refresh. True if the focus is now `index`.
pub fn request_focus_army(lua: &Lua, index: i64) -> LuaResult<bool> {
    let g = lua.globals();
    let armies = g.raw_get::<Option<LuaTable>>("__engine_armies")?;
    let count = armies.as_ref().map_or(0, |a| a.raw_len() as i64);
    if index < -1 || index >= count {
        tracing::warn!(target: "lua", index, count, "SetFocusArmy: no such army");
        return Ok(false);
    }
    let new = if index < 0 { -1 } else { index + 1 };
    let old = lua.named_registry_value::<Option<i64>>("user.focus_army")?.unwrap_or(-1);
    let flag = |name: &str| -> LuaResult<bool> { Ok(g.raw_get::<Option<bool>>(name)?.unwrap_or(false)) };
    let cheats = || -> LuaResult<bool> {
        let Some(session) = g.raw_get::<Option<LuaTable>>("__engine_session")? else { return Ok(false) };
        let Some(options) = session.raw_get::<Option<LuaTable>>("Options")? else { return Ok(false) };
        Ok(match options.raw_get::<LuaValue>("CheatsEnabled")? {
            LuaValue::Boolean(b) => b,
            LuaValue::String(s) => s.to_str().is_ok_and(|s| s.eq_ignore_ascii_case("true")),
            _ => false,
        })
    };
    let own = lua.named_registry_value::<Option<i64>>("user.own_army")?.unwrap_or(-1);
    let own_in_game = || -> LuaResult<bool> {
        if own < 1 {
            return Ok(false);
        }
        let army = armies.as_ref().map(|a| a.raw_get::<Option<LuaTable>>(own)).transpose()?.flatten();
        Ok(army.is_some_and(|a| !a.raw_get::<Option<bool>>("outOfGame").ok().flatten().unwrap_or(false)))
    };
    let allowed = flag("__engine_replay")? || cheats()? || (new >= 1 && new == own) || crate::ui::session::state(lua).game_over || !own_in_game()?;
    if !allowed {
        tracing::info!(target: "lua", index, "SetFocusArmy: refused (a player in a live game keeps their army's view)");
        return Ok(false);
    }
    if new != old {
        lua.set_named_registry_value("user.focus_army", new)?;
        // The first change since the last Sync keeps its `old`.
        let pending = match lua.named_registry_value::<Option<LuaTable>>("user.focus_changed")? {
            Some(t) => t,
            None => {
                let t = lua.create_table()?;
                t.raw_set("old", old)?;
                lua.set_named_registry_value("user.focus_changed", &t)?;
                t
            }
        };
        pending.raw_set("new", new)?;
    }
    Ok(true)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// The selection's orders, toggles and buildable categories
        /// ([`unit_command_data`]).
        GetUnitCommandData = |lua, unit_set: Option<Vec<Obj<moho::UserUnit>>>| unit_command_data(lua, &unit_set.unwrap_or_default()))?;
    lua_fn!(lua, global,
        /// The user layer's clock: seconds of UI frames run (`UserVm::frame`),
        /// which is what animations and `WaitSeconds` count in.
        CurrentTime = |lua, ()| Ok(lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0)))?;
    lua_fn!(lua, global,
        /// Same clock as `CurrentTime`.
        GetSystemTimeSeconds = |lua, ()| Ok(lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0)))?;
    register_loadstring(lua)?;
    lua_fn!(lua, global,
        /// Seconds of the `CurrentTime` clock since `origin` (FAF's binary
        /// patch, for timings that would lose precision as one big float).
        GetTimeForProfile = |lua, origin: Option<f64>| Ok(lua.named_registry_value::<Option<f64>>("user.time")?.unwrap_or(0.0) - origin.unwrap_or(0.0)))?;
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
        /// A copy of what `SetFrontEndData` kept under `key` (nil if nothing).
        GetFrontEndData = |lua, key: String| match front_end_data().get(&key) {
            Some(data) => data.to_lua(lua),
            None => Ok(LuaNil),
        })?;
    lua_fn!(lua, global,
        /// Keeps a copy of `data` under `key` for the rest of the program,
        /// across the front end's and each game's interface (nil forgets it).
        SetFrontEndData = |_, (key, data): (String, LuaValue)| {
            let mut all = front_end_data();
            match data {
                LuaNil => all.remove(&key),
                data => all.insert(key, crate::engine::sim::lua_data::LuaData::from_lua(&data)),
            };
            Ok(())
        })?;
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
        SessionIsActive = |lua, ()| Ok(crate::ui::session::state(lua).ui_state == UiState::Game))?;
    lua_fn!(lua, global, SessionIsReplay = |lua, ()| Ok(lua.globals().raw_get::<Option<bool>>("__engine_replay")?.unwrap_or(false)))?;
    lua_fn!(lua, global, SessionIsMultiplayer = |_, ()| Ok(crate::net::core::current().is_some_and(|n| !n.session_clients().is_empty())))?;
    lua_fn!(lua, global,
        /// A game (not the main menu) that isn't networked: a skirmish or a
        /// replay starts again on this machine alone.
        SessionCanRestart = |lua, ()| Ok(crate::ui::session::state(lua).ui_state == UiState::Game
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
        SessionIsGameOver = |lua, ()| Ok(crate::ui::session::state(lua).game_over))?;
    lua_fn!(lua, global,
        /// This client has no army of its own (watching, or a replay).
        IsObserver = |lua, ()| Ok(lua.globals().raw_get::<Option<bool>>("__engine_observer")?.unwrap_or(false)))?;
    lua_fn!(lua, global, GetUIControlsAlpha = |_, ()| Ok(1.0f64))?;
    lua_fn!(lua, global, SetUIControlsAlpha = |_, _alpha: Loose<f64>| Ok(()))?;
    lua_fn!(lua, global, AddConsoleOutputReciever = |_, _receiver: Loose<LuaFunction>| Ok(()))?;
    lua_fn!(lua, global, RemoveConsoleOutputReciever = |_, _receiver: Loose<LuaFunction>| Ok(()))?;
    lua_fn!(lua, global,
        /// Moho runs the sim at 10 ticks a second, whatever the game speed.
        GetSimTicksPerSecond = |_, ()| Ok(10.0f64))?;
    lua_fn!(lua, global,
        /// The Anti-Aliasing choices besides Off (FAF's presets pick the middle
        /// one for High, the last for Ultra): 1 is SMAA (render/smaa.rs);
        /// MSAA counts (2, 4, 8) join it once MSAA is drawn.
        GetAntiAliasingOptions = |_, ()| Ok(vec![1.0f64]))?;
    lua_fn!(lua, global,
        /// Sound globals are ui/sound.rs; the tutorial's voice-overs aren't.
        #[lua(stub)]
        PlayTutorialVO = |_, _args: LuaMultiValue| Ok(()))?;
    lua_fn!(lua, global,
        #[lua(stub)]
        FlushEvents = |_, _args: LuaMultiValue| Ok(()))?;
    lua_fn!(lua, global,
        /// Console commands matching `text`: none.
        ConTextMatches = |_, _text: Loose<String>| Ok(Vec::<String>::new()))?;
    lua_fn!(lua, global,
        /// The units handed in, unfiltered.
        GetValidAttackingUnits = |_, units: Loose<Vec<Obj<crate::engine::api::moho::UserUnit>>>| Ok(units))?;
    lua_fn!(lua, global,
        /// The current profile's options, where FAF's options dialog keeps
        /// them (`Prefs.SetToCurrentProfile('options', ...)`), or just the
        /// option `key`. No profile: an empty table (nil for one option),
        /// and FAF falls back to each option's default from optionslogic.lua.
        GetOptions = |lua, key: Option<String>| {
            let preference: LuaFunction = lua.globals().get("GetPreference")?;
            let Some(current) = lua.coerce_string(preference.call::<LuaValue>("profile.current")?)? else {
                return Ok(if key.is_none() { LuaValue::Table(lua.create_table()?) } else { LuaNil });
            };
            let options = format!("profile.profiles.{}.options", current.to_string_lossy());
            // One option: read just it (a preference comes out as a copy).
            match key {
                Some(key) => preference.call(format!("{options}.{key}")),
                None => preference.call((options, lua.create_table()?)),
            }
        })?;
    lua.set_named_registry_value("user.focus_army", -1)?;
    lua_fn!(lua, global,
        /// The army the player sees as, 1-based (`--player`, else observer: -1).
        GetFocusArmy = |lua, ()| lua.named_registry_value::<i64>("user.focus_army"))?;
    lua_fn!(lua, global,
        /// `index` is 0-based (-1: observer); `GetFocusArmy` is 1-based. A
        /// request, allowed only as moho allows it ([`request_focus_army`]).
        SetFocusArmy = |lua, a: i64| request_focus_army(lua, a).map(|_| ()))?;
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
    lua_fn!(lua, global,
        #[lua(internal)]
        __engine_wld_provider = |lua, ()| lua.named_registry_value::<Option<Obj<crate::engine::api::moho::WldUIProvider>>>("user.wld_provider"))?;
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
            // The selection as the interface sees it now (`SelectUnits` is
            // synchronous).
            let units = if with_selection { crate::ui::orders::selection(lua)? } else { Vec::new() };
            let callback = crate::engine::sim::input::SimCallback {
                func: func.to_string_lossy(),
                args: crate::engine::sim::lua_data::LuaData::from_lua(&callback.raw_get::<LuaValue>("Args")?),
                with_selection,
            };
            crate::ui::orders::push(lua, crate::ui::UiOrder { units: Some(units), callback: Some(callback), ..Default::default() });
            Ok(())
        })?;
    lua_fn!(lua, global, GetTextureDimensions = |lua, (path, _scale): (String, Option<f64>)| Ok(crate::ui::controls::texture_dimensions(lua, &path)))?;
    Ok(())
}

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

        crate::ui::session::state(&lua).ui_state = UiState::Game;
        assert!(!launch_replay_session(&lua, &ours).unwrap(), "not mid-game");
        crate::ui::session::state(&lua).ui_state = UiState::Frontend;
        assert!(!launch_replay_session(&lua, &theirs).unwrap(), "no header");
        assert!(!launch_replay_session(&lua, "/no/such/file.rtsreplay").unwrap());
        assert!(lua.globals().raw_get::<LuaValue>("__engine_launch_replay").unwrap().is_nil());
        assert!(launch_replay_session(&lua, &ours).unwrap());
        assert_eq!(lua.globals().raw_get::<String>("__engine_launch_replay").unwrap(), ours);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod loadstring_tests {
    use super::*;

    /// The interface's `loadstring` takes SupCom Lua and returns nil and
    /// the message for code that doesn't compile, as in the real game.
    #[test]
    fn compiles_supcom_lua() {
        let lua = Lua::new();
        register_loadstring(&lua).unwrap();
        let r: (i64, bool, bool, i64) = lua
            .load("local f = loadstring('return probe_x, 1 != 2') setfenv(f, { probe_x = 7 }) local x, ne = f() local bad, err = loadstring('return +') return x, ne, bad == nil and type(err) == 'string', loadstring(5) == nil and 1 or 0")
            .eval()
            .unwrap();
        assert_eq!(r, (7, true, true, 1));
    }
}
