//! Declared Lua signatures for the user (UI) VM's registrations without
//! types: the stubs (`USER_GLOBALS`, `CORE_GLOBALS`) and functions taking
//! `LuaMultiValue`. See `src/engine/api/signatures.rs` (which also has the
//! globals both VMs share). Nothing here runs.

#![allow(unused_imports)]

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::categories::EntityCategory;
use crate::engine::api::moho;
use crate::engine::sim::layer::Layer;
use crate::lua_api::shapes::{Army, Bone, Color, FireState, List, Quaternion, Rect, Shaped, Vector, Vector2};
use crate::lua_api::{Obj, signatures};
use crate::ui::net_lobby::SessionClientData;

/// A resource pair: mass and energy.
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "ResourcePair", rename_all = "UPPERCASE", write_only)]
struct ResourcePair {
    mass: f64,
    energy: f64,
}

/// The focus army's economy (`GetEconomyTotals`).
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "EconomyTotals", rename_all = "camelCase", write_only)]
struct EconomyTotals {
    stored: ResourcePair,
    max_storage: ResourcePair,
    income: ResourcePair,
    last_use_requested: ResourcePair,
    last_use_actual: ResourcePair,
    reclaimed: ResourcePair,
}

/// One army of `GetArmiesTable().armiesTable` (indexed by army, 1-based).
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "UIArmy", rename_all = "camelCase", write_only)]
struct UiArmy {
    /// Command sources (1-based) of the army's player.
    authorized_command_sources: Vec<u32>,
    name: String,
    nickname: String,
    /// 0-based faction index.
    faction: i64,
    human: bool,
    civilian: bool,
    out_of_game: bool,
    show_score: bool,
    /// The player colour, hex `aarrggbb`.
    color: String,
    /// The army colour, hex `aarrggbb`.
    icon_color: String,
    army_index: u32,
}

/// The session's armies (`GetArmiesTable`).
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "UIArmiesTable", rename_all = "camelCase", write_only)]
struct ArmiesTable {
    num_armies: u32,
    focus_army: i64,
    armies_table: Vec<UiArmy>,
}

/// The order waypoint under the mouse (`GetHighlightCommand`).
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "UIHighlightedCommand", rename_all = "camelCase", write_only)]
struct HighlightCommand {
    x: f64,
    y: f64,
    z: f64,
    command_type: u32,
    command_id: u32,
    blueprint_id: Option<String>,
}

/// One factory queue entry (`SetCurrentFactoryForQueueDisplay`): the
/// blueprint and how many in a row.
#[derive(crate::lua_api::LuaTable)]
#[allow(dead_code)]
#[lua(name = "UIQueueEntry", write_only)]
struct QueueEntry {
    id: String,
    count: u32,
}

signatures! {
    global {
        AddBlinkyBox(entity_id: String, on_time: f64, off_time: f64, total_time: f64);
        AddInputCapture(control: Obj<moho::Control>);
        AddSelectUnits(selection: Vec<Obj<moho::UserUnit>>);
        AddToSessionExtraSelectList(unit: Obj<moho::UserUnit>);
        AnyInputCapture() -> bool;
        ClearBuildTemplates();
        ClearCurrentFactoryForQueueDisplay();
        ClearSessionExtraSelectList();
        ConExecute(command: String);
        ConExecuteSave(command: String);
        ConTextMatches(text: String) -> Vec<String>;
        CopyCurrentReplay(profile: String, new_filename: String);
        CreateUnitAtMouse(blueprint_id: String, owner_army_index: f64, offset_mouse_world_pos_x: f64, offset_mouse_world_pos_z: f64, rotation: f64) -> Obj<moho::UserUnit>;
        DecreaseBuildCountInQueue(queue_index: f64, count: f64);
        DeleteCommand(command_id: f64);
        EjectSessionClient(client_index: f64);
        EngineStartFrontEndUI();
        EngineStartSplashScreens();
        ExecLuaInSim(func: String, arg: LuaValue);
        ExitApplication();
        ExitGame();
        FlushEvents();
        FormatTime(seconds: f64) -> String;
        GameTick() -> f64;
        GameTime() -> f64;
        GenerateBuildTemplateFromSelection();
        /// A `UIBuildTemplate`: `{ width, height, { blueprint, id, x, z }, ... }`, x / z offsets from the cursor.
        GetActiveBuildTemplate() -> Vec<LuaValue>;
        GetAntiAliasingOptions() -> Vec<f64>;
        GetArmiesTable() -> Shaped<ArmiesTable>;
        GetArmyAvatars() -> Vec<Obj<moho::UserUnit>>;
        GetArmyScore(army_index: f64) -> f64;
        GetAssistingUnitsList(units: Vec<Obj<moho::UserUnit>>) -> Vec<Obj<moho::UserUnit>>;
        GetAttachedUnitsList(units: Vec<Obj<moho::UserUnit>>) -> Vec<Obj<moho::UserUnit>>;
        GetCurrentUIState() -> String;
        GetCursor() -> Option<Obj<moho::Cursor>>;
        GetEconomyTotals() -> Shaped<EconomyTotals>;
        GetFireState(units: Vec<Obj<moho::UserUnit>>) -> FireState;
        GetFrontEndData(key: String) -> LuaValue;
        GetGameSpeed() -> f64;
        GetGameTime() -> String;
        GetGameTimeSeconds() -> f64;
        GetHighlightCommand() -> Option<Shaped<HighlightCommand>>;
        GetIdleEngineers() -> Vec<Obj<moho::UserUnit>>;
        GetIdleFactories() -> Vec<Obj<moho::UserUnit>>;
        /// The control that takes all input (the last one added), nil when none.
        GetInputCapture() -> Option<Obj<moho::Control>>;
        GetIsAutoMode(units: Vec<Obj<moho::UserUnit>>) -> bool;
        GetIsAutoSurfaceMode(units: Vec<Obj<moho::UserUnit>>) -> bool;
        GetIsPaused(units: Vec<Obj<moho::UserUnit>>) -> bool;
        GetIsPausedOfUnit(unit: Vec<Obj<moho::UserUnit>>) -> bool;
        /// -1 all submerged, 1 none, 0 a mix.
        GetIsSubmerged(units: Vec<Obj<moho::UserUnit>>) -> i64;
        GetMouseScreenPos() -> Vector2;
        GetOptions(key: String) -> LuaValue;
        GetPreference(string: String, default: Option<LuaValue>) -> LuaValue;
        GetResourceSharing() -> bool;
        /// The unit under the mouse, as moho builds it (blueprint, health, economy, silos, army, the unit it works on or guards); nil over the interface or empty ground.
        GetRolloverInfo() -> Option<LuaTable>;
        GetScriptBit(unit: Obj<moho::UserUnit>, bit: f64) -> bool;
        GetSelectedUnits() -> Vec<Obj<moho::UserUnit>>;
        GetSessionClients() -> List<Shaped<SessionClientData>>;
        GetSimRate() -> f64;
        GetSimTicksPerSecond() -> f64;
        GetUnitById(id: String) -> Option<Obj<moho::UserUnit>>;
        GetUnitCommandDataOfUnit(unit: Obj<moho::UserUnit>) -> (Vec<String>, Vec<String>, EntityCategory);
        GetUnitCommandFromCommandCap(rule: String) -> String;
        GetValidAttackingUnits() -> Vec<Obj<moho::UserUnit>>;
        GpgNetSend(command: String, rest: mlua::Variadic<LuaValue>);
        /// Key name to action (a name, or a table with an `action`).
        IN_AddKeyMapTable(key_map_table: BTreeMap<String, mlua::Either<String, LuaTable>>);
        IN_ClearKeyMap();
        /// The table given to `IN_AddKeyMapTable`.
        IN_RemoveKeyMapTable(key_map_table: BTreeMap<String, mlua::Either<String, LuaTable>>);
        IncreaseBuildCountInQueue(queue_index: f64, count: f64);
        InternalCreateDiscoveryService(service_class: LuaTable) -> Obj<moho::DiscoveryService>;
        /// nil when the lobby can't be created (the network port is taken).
        InternalCreateLobby(lobby_com_class: LuaTable, protocol: String, local_port: f64, max_connections: f64, player_name: String, player_uid: Option<mlua::Either<String, f64>>, nat_traversal_provider: Option<LuaValue>) -> Option<Obj<moho::Lobby>>;
        InternalSaveGame(filename: String, friendlyname: String, oncompletion: LuaFunction);
        IssueBlueprintCommand(command: String, blueprintid: String, count: f64, clear: Option<bool>);
        IssueBlueprintCommandToUnit(unit: Obj<moho::UserUnit>, command: String, blueprintid: String, count: f64, clear: Option<bool>);
        IssueBlueprintCommandToUnits(units: Vec<Obj<moho::UserUnit>>, command: String, blueprintid: String, count: f64, clear: Option<bool>);
        /// `lua_params` is any serialisable value, sent with the command.
        IssueCommand(command: String, lua_params: Option<LuaValue>, clear: Option<bool>);
        IssueDockCommand(clear: bool);
        /// `lua_params` is any serialisable value, sent with the command.
        IssueUnitCommand(unit_list: Vec<Obj<moho::UserUnit>>, command: String, lua_params: Option<LuaValue>, clear: Option<bool>);
        KeycodeMSWToMaui(charcode: f64) -> f64;
        KeycodeMauiToMSW(charcode: f64) -> f64;
        LaunchGPGNet();
        LaunchSinglePlayerSession(session_info: LuaTable);
        LoadSavedGame(filename: String) -> bool;
        OpenURL(url: String);
        OrientFromDir(vector: Vector) -> Quaternion;
        PlayTutorialVO(params: LuaValue);
        PrefetchSession(mapname: String, mods: Vec<LuaTable>, hipri: Option<bool>);
        RemoveFromSessionExtraSelectList(unit: Obj<moho::UserUnit>);
        RemoveInputCapture(control: Obj<moho::Control>);
        RemoveProfileDirectories(profile: String);
        RenderOverlayEconomy(render: bool);
        RenderOverlayIntel(render: bool);
        RenderOverlayMilitary(render: bool);
        SavePreferences();
        SecondsPerTick() -> f64;
        SelectUnits(units: Option<Vec<Obj<moho::UserUnit>>>);
        SessionEndGame();
        SessionGetCommandSourceNames() -> Vec<String>;
        SessionGetLocalCommandSource() -> f64;
        SessionGetScenarioInfo() -> LuaTable;
        SessionIsBeingRecorded() -> bool;
        SessionIsObservingAllowed() -> bool;
        SessionIsPaused() -> bool;
        SessionRequestPause();
        SessionResume();
        /// To everyone.
        SessionSendChatMessage(message: mlua::Either<LuaTable, mlua::Either<f64, String>>);
        /// To the given session client index (1-based) or indices.
        SessionSendChatMessage(client: mlua::Either<f64, Vec<f64>>, message: mlua::Either<LuaTable, mlua::Either<f64, String>>);
        /// A `UIBuildTemplate`: `{ width, height, { blueprint, id, x, z }, ... }`.
        SetActiveBuildTemplate(template: Vec<LuaValue>);
        SetAutoMode(units: Vec<Obj<moho::UserUnit>>, mode: bool);
        SetAutoSurfaceMode(units: Vec<Obj<moho::UserUnit>>, mode: bool);
        /// The factory's queue, front first.
        SetCurrentFactoryForQueueDisplay(unit: Obj<moho::UserUnit>) -> Vec<Shaped<QueueEntry>>;
        SetCursor(cursor: Obj<moho::Cursor>);
        SetFireState(units: Vec<Obj<moho::UserUnit>>, fire_state: FireState);
        SetFrontEndData(key: String, data: LuaValue);
        SetGameSpeed(speed: f64);
        SetInvertMidMouseButton(flag: bool);
        SetOverlayFilter(overlay: String, categories: EntityCategory, normal_color: Color, select_color: Color, rollover_color: Color, inner1: f64, inner2: f64, outer1: f64, outer2: f64);
        /// Range overlay names; an empty list turns them off.
        SetOverlayFilters(list: Vec<String>);
        SetPaused(units: Vec<Obj<moho::UserUnit>>, pause: bool);
        SetPausedOfUnit(unit: Obj<moho::UserUnit>, pause: bool);
        SetPreference(key: String, obj: LuaValue);
        SyncPlayableRect(region: Rect);
        ToggleFireState(units: Vec<Obj<moho::UserUnit>>, fire_state: mlua::Either<String, f64>);
        ToggleScriptBit(units: Vec<Obj<moho::UserUnit>>, bit: f64, state: bool);
        UISelectAndZoomTo(userunit: Obj<moho::UserUnit>, seconds: Option<f64>);
        UISelectionByCategory(expression: String, add_to_cur_sel: bool, in_view_frustum: bool, nearest_to_mouse: bool, must_be_idle: bool);
        UIZoomTo(units: Vec<Obj<moho::UserUnit>>, seconds: Option<f64>);
        UI_DrawCircle(pos: Vector, size: f64, color: Color, thickness: Option<f64>);
        UI_DrawRect(pos: Vector, size: f64, color: Color, thickness: Option<f64>);
        ValidateUnitsList(units: Vec<Obj<moho::UserUnit>>) -> Vec<Obj<moho::UserUnit>>;
        WorldIsPlaying() -> bool;
        _c_CreateDecal(decal: Obj<moho::UserDecal>, spec: Option<LuaTable>);
        _c_CreatePathDebugger(path_debugger: Obj<moho::PathDebugger>, spec: LuaTable);
    }
    moho::Bitmap {
        UseAlphaHitTest(do_hit: bool);
    }
    moho::Control {
        Dump();
        GetCurrentFocusControl() -> Option<Obj<moho::Control>>;
    }
    moho::Edit {
        SetNewHighlightBackgroundColor(color: Color);
        SetNewHighlightForegroundColor(color: Color);
    }
    moho::Frame {
        GetTargetHead() -> LuaValue;
        SetTargetHead(head: f64);
    }
    moho::ItemList {
        /// A nil colour keeps the current one.
        SetNewColors(foreground: Option<Color>, background: Option<Color>, selected_foreground: Option<Color>, selected_background: Option<Color>, mouseover_foreground: Option<Color>, mouseover_background: Option<Color>);
    }
    moho::WorldMesh {
        SetAuxiliaryParameter(param: f64);
        /// The team colour (moho's own notes say `SetColor(bool hidden)`, a slip).
        SetColor(color: Color);
        SetFractionCompleteParameter(param: f64);
        SetFractionHealthParameter(param: f64);
    }
}
