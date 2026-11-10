//! Session launch: what moho does between `LaunchGame` and the first tick.

use mlua::prelude::*;
use tracing::info;

use crate::engine::sim::rng::SimRng;

#[derive(Clone, Debug, PartialEq)]
pub struct SessionConfig {
    /// VFS path of the map's `_scenario.lua`.
    pub scenario: String,
    /// AI armies to fill from the start spots (0 = every start spot).
    pub ai_armies: u32,
    /// Teams by start spot: spot `i` joins team `(i - 1) % teams` (FAF's
    /// alternating spots); 0 or 1 = no teams (everyone against everyone).
    pub teams: u32,
    /// Give those armies FAF's AI. False makes them human slots nobody
    /// plays (scenario tests: the AI never touches their units).
    pub ai: bool,
    /// Seed for the sim RNG; part of the lockstep game setup.
    pub seed: u64,
    /// Spawn each army's initial unit (ACU). False when resuming a save.
    pub create_initial_units: bool,
    /// Ticks to run after `BeginSession`; `None` = run in real time forever.
    pub ticks: Option<u64>,
    /// Disk path of a Lua file run in the sim VM after `BeginSession`
    /// (development scenarios, e.g. `dev/scenes/fight.lua`).
    pub script: Option<String>,
    /// Army (1-based) played by a person: a human slot even when the
    /// others get the AI. `None` = every army is the AI (or nobody).
    pub human_army: Option<u32>,
    /// FAF's `PrebuiltUnits` lobby option: each army starts with a basic
    /// base (factory, generators, extractors).
    pub prebuilt: bool,
    /// A lobby's launch (`CLobby:LaunchGame`): `{Config = gameConfig, LocalID
    /// = peer id}`. The scenario, options and armies are then the lobby's
    /// (`scenario`, `ai_armies`, `ai`, `teams` and `prebuilt` are unused) and
    /// the local player's army is the human one it owns.
    pub launch: Option<crate::engine::sim::lua_data::LuaData>,
    /// Teams by where the armies start (FAF's lobby option `AutoTeams`),
    /// replacing `teams`' alternating spots. Unused with a lobby launch.
    pub auto_teams: Option<AutoTeams>,
    /// A networked game: the local player is a bot giving random orders
    /// (`net::bot`, headless network tests).
    pub net_bot: bool,
    /// The game's `FogOfWar` option (`'none'`, `'explored'`, `'unexplored'`),
    /// replacing the launch's (FAF's lobby offers only the first two; moho
    /// also takes `'unexplored'`). `None` keeps the launch's ('explored'
    /// without a lobby).
    pub fog_of_war: Option<String>,
    /// The game's `Victory` option, replacing the launch's (`'sandbox'`
    /// without a lobby). `None` keeps the launch's.
    pub victory: Option<String>,
    /// Mods (uids) to run besides a launch's `GameMods` (`--mod`): sim mods
    /// in the sim, UI-only ones in the interface (`engine::mods`).
    pub mods: Vec<String>,
    /// The AI the armies get (`--ai`), instead of the game code's default.
    /// Unused with a lobby launch (its slots name their own AI).
    pub ai_choice: AiChoice,
}

/// Which AI (a lobby AI key, e.g. `adaptive`, `RNGStandard`) the armies
/// get: one for all, and any by army number. Checked against the game's
/// lobby AI list (with the session's mods) when the session starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiChoice {
    /// For every AI army without its own; `None` = the game code's default.
    pub default: Option<String>,
    /// `(army, key)`: army numbers are 1-based, in start-spot order.
    pub armies: Vec<(u32, String)>,
}

impl AiChoice {
    /// The `--ai` values: `KEY` for every army, `N=KEY` for army N.
    pub fn parse(values: &[String]) -> Result<Self, String> {
        let mut choice = AiChoice::default();
        for v in values {
            match v.split_once('=') {
                Some((army, key)) => {
                    let army: u32 = army.trim().parse().ok().filter(|&a| a > 0).ok_or_else(|| format!("--ai {v}: expected ARMY=KEY with an army number from 1"))?;
                    let key = key.trim();
                    if key.is_empty() {
                        return Err(format!("--ai {v}: no AI key"));
                    }
                    if choice.armies.iter().any(|(a, _)| *a == army) {
                        return Err(format!("--ai: army {army} given twice"));
                    }
                    choice.armies.push((army, key.to_string()));
                }
                None if v.trim().is_empty() => return Err("--ai: no AI key".into()),
                None if choice.default.is_some() => return Err(format!("--ai {v}: a default AI is already given (use ARMY=KEY for one army)")),
                None => choice.default = Some(v.trim().to_string()),
            }
        }
        Ok(choice)
    }

    /// Back to the `--ai` values (the replay header keeps them so).
    pub fn values(&self) -> Vec<String> {
        self.default.iter().cloned().chain(self.armies.iter().map(|(a, k)| format!("{a}={k}"))).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.default.is_none() && self.armies.is_empty()
    }

    /// As `session_launch.lua` takes it: `{default = key, [army] = key}`,
    /// or nil when nothing was chosen.
    fn to_lua(&self, lua: &Lua) -> LuaResult<LuaValue> {
        if self.is_empty() {
            return Ok(LuaNil);
        }
        let t = lua.create_table()?;
        t.set("default", self.default.as_deref())?;
        for (army, key) in &self.armies {
            t.set(*army, key.as_str())?;
        }
        Ok(LuaValue::Table(t))
    }
}

/// FAF's lobby `AutoTeams` layouts: two teams split by the map's middle
/// line (start marker in the top / left half: team 2, else team 3), or by
/// start spot (odd: team 2, even: team 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoTeams {
    /// `tvsb`: north against south.
    TopBottom,
    /// `lvsr`: west against east.
    LeftRight,
    /// `pvsi`: odd spots against even.
    OddEven,
}

impl AutoTeams {
    /// FAF's option key.
    pub fn key(self) -> &'static str {
        match self {
            AutoTeams::TopBottom => "tvsb",
            AutoTeams::LeftRight => "lvsr",
            AutoTeams::OddEven => "pvsi",
        }
    }

    /// The layout for FAF's option key.
    pub fn from_key(key: &str) -> Option<Self> {
        [AutoTeams::TopBottom, AutoTeams::LeftRight, AutoTeams::OddEven].into_iter().find(|a| a.key() == key)
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            scenario: "/maps/scmp_009/scmp_009_scenario.lua".into(),
            ai_armies: 2,
            teams: 0,
            ai: true,
            seed: 0,
            create_initial_units: true,
            ticks: None,
            script: None,
            human_army: None,
            prebuilt: false,
            launch: None,
            auto_teams: None,
            net_bot: false,
            fog_of_war: None,
            victory: None,
            mods: Vec::new(),
            ai_choice: AiChoice::default(),
        }
    }
}

/// Sets up `ScenarioInfo`, runs `SetupSession()`, creates the armies/brains
/// (`OnCreateArmyBrain`) and runs `BeginSession()`.
/// The game code the sim's files are (FAF's without a VFS: tests).
fn game_code(lua: &Lua) -> crate::engine::game_code::GameCode {
    use crate::engine::game_code::GameCode;
    lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>()
        .and_then(|v| v.read().ok().map(|v| GameCode::detect(&v)))
        .unwrap_or(GameCode::Faf)
}

pub fn setup(lua: &Lua, cfg: &SessionConfig) -> LuaResult<()> {
    // The lobby's configuration is logged by the launch, readably.
    let lobby = cfg.launch.is_some();
    info!(cfg = ?SessionConfig { launch: None, ..cfg.clone() }, lobby, "launching session");
    // A networked launch carries the host's seed (as a string: a u64 doesn't
    // fit a Lua number); every peer must use it.
    let seed = match &cfg.launch {
        Some(crate::engine::sim::lua_data::LuaData::Table(fields)) => fields
            .iter()
            .find(|(k, _)| *k == crate::engine::sim::lua_data::LuaData::string("Seed"))
            .and_then(|(_, v)| match v {
                crate::engine::sim::lua_data::LuaData::String(s) => std::str::from_utf8(s).ok().and_then(|s| s.parse().ok()),
                _ => None,
            })
            .unwrap_or(cfg.seed),
        _ => cfg.seed,
    };
    lua.set_app_data(SimRng::new(seed));
    lua.set_named_registry_value("sim.create_initial_units", cfg.create_initial_units)?;
    let launch = cfg.launch.as_ref().map(|l| l.to_lua(lua)).transpose()?;
    let scenario = scenario_file(lua, launch.as_ref(), cfg)?;
    let human_army = {
        let _s = crate::prof_span!("session launch (scenario, armies)");
        let lobby_army: Option<u32> = lua
            .load(include_str!("lua/session_launch.lua"))
            .set_name("=[engine session_launch]")
            .call((scenario.as_str(), cfg.ai_armies, cfg.ai, cfg.human_army.unwrap_or(0), cfg.teams, cfg.prebuilt, launch, game_code(lua).name(), cfg.ai_choice.to_lua(lua)?))?;
        cfg.human_army.or(lobby_army)
    };
    if let Some(fog) = &cfg.fog_of_war {
        let options: LuaTable = lua.globals().get::<LuaTable>("ScenarioInfo")?.get("Options")?;
        options.set("FogOfWar", fog.as_str())?;
    }
    if let Some(victory) = &cfg.victory {
        let options: LuaTable = lua.globals().get::<LuaTable>("ScenarioInfo")?.get("Options")?;
        options.set("Victory", victory.as_str())?;
    }
    // Moho takes CheatsEnabled from the launch options ('true' or true).
    let cheats = lua
        .globals()
        .get::<Option<LuaTable>>("ScenarioInfo")?
        .and_then(|s| s.get::<Option<LuaTable>>("Options").ok().flatten())
        .map(|o| o.get::<LuaValue>("CheatsEnabled"))
        .transpose()?
        .is_some_and(|v| matches!(&v, LuaValue::Boolean(true)) || v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("true")));
    lua.set_app_data(crate::engine::api::command_source::Cheats { enabled: cheats, cheaters: Vec::new() });
    {
        let _s = crate::prof_span!("SetupSession");
        let setup: LuaFunction = lua.globals().get("SetupSession")?;
        setup.call::<()>(())?;
    }
    info!("SetupSession complete");
    // After the map's script (which may add start markers), before the
    // brains: teams by start position.
    if let (Some(layout), None) = (cfg.auto_teams, &cfg.launch) {
        lua.load(include_str!("lua/auto_teams.lua")).set_name("=[engine auto_teams]").call::<()>(layout.key())?;
    }
    {
        let _s = crate::prof_span!("map props");
        crate::engine::api::terrain::spawn_map_props(lua)?;
    }
    {
        let _s = crate::prof_span!("create armies (brains)");
        crate::engine::api::army::create_armies(lua)?;
    }
    crate::engine::api::command_source::set_local_source(lua, human_army);
    #[cfg(feature = "tracy")]
    profile_begin_session(lua)?;
    {
        let _s = crate::prof_span!("BeginSession");
        let begin: LuaFunction = lua.globals().get("BeginSession")?;
        begin.call::<()>(())?;
    }
    info!("BeginSession complete");
    {
        let _s = crate::prof_span!("prebuilt units");
        initialize_prebuilt_units(lua)?;
    }
    if let Some(path) = &cfg.script {
        let _s = crate::prof_span!("session script");
        let code = std::fs::read_to_string(path).map_err(|e| LuaError::runtime(format!("--script {path}: {e}")))?;
        lua.load(code).set_name(format!("@{path}")).exec()?;
        info!(path, "session script complete");
    }
    Ok(())
}

/// With the `PrebuiltUnits` option on, simInit's
/// `InitializePrebuiltUnits(name)` for each army that isn't civilian, in
/// army order, after `BeginSession` (inferred: moho's exact point in the
/// start-up and whether civilians are skipped).
fn initialize_prebuilt_units(lua: &Lua) -> LuaResult<()> {
    let on = lua
        .globals()
        .get::<Option<LuaTable>>("ScenarioInfo")?
        .and_then(|s| s.get::<Option<LuaTable>>("Options").ok().flatten())
        .and_then(|o| o.get::<Option<String>>("PrebuiltUnits").ok().flatten())
        .is_some_and(|v| v == "On");
    if !on {
        return Ok(());
    }
    let init: LuaFunction = lua.globals().get("InitializePrebuiltUnits")?;
    let names: Vec<(String, bool)> = lua
        .app_data_ref::<crate::engine::sim::army::Armies>()
        .map(|a| a.list.iter().map(|army| (army.name.clone(), army.civilian)).collect())
        .unwrap_or_default();
    for (name, civilian) in names {
        if !civilian {
            init.call::<()>(name)?;
        }
    }
    Ok(())
}

/// The session's scenario file as a VFS path: the lobby launch's
/// `ScenarioFile`, else the configured one.
fn scenario_file(lua: &Lua, launch: Option<&LuaValue>, cfg: &SessionConfig) -> LuaResult<String> {
    let scenario = match launch {
        Some(LuaValue::Table(l)) => {
            let options: LuaTable = l.get::<LuaTable>("Config")?.get("GameOptions")?;
            options.get::<String>("ScenarioFile")?
        }
        _ => cfg.scenario.clone(),
    };
    scenario_vfs_path(lua, &scenario)
}

/// Loads the session's map (the scenario's `.scmap`) into the sim, before
/// `/lua/simInit.lua` runs: moho has the terrain before any sim Lua, and
/// mods call `GetMapSize()` and the like when their modules are imported
/// (RNGAI's `unitcountbuildconditions.lua` hook, from simInit's imports).
pub fn load_map(lua: &Lua, cfg: &SessionConfig) -> LuaResult<()> {
    let launch = cfg.launch.as_ref().map(|l| l.to_lua(lua)).transpose()?;
    let scenario = scenario_file(lua, launch.as_ref(), cfg)?;
    let data = lua.create_table()?;
    let doscript: LuaFunction = lua.globals().get("doscript")?;
    doscript.call::<()>(("/lua/dataInit.lua", data.clone()))?;
    doscript.call::<()>((scenario.as_str(), data.clone()))?;
    let map: String = data
        .get::<Option<LuaTable>>("ScenarioInfo")?
        .ok_or_else(|| LuaError::runtime(format!("session launch: no ScenarioInfo in {scenario}")))?
        .get("map")?;
    crate::engine::api::terrain::load_map(lua, &map)?;
    if let Some(mut sink) = lua.app_data_mut::<crate::engine::snapshot::SnapshotSink>() {
        sink.send_terrain(lua);
    }
    Ok(())
}

/// The scenario as a VFS path: a disk path inside a mounted folder (e.g. a
/// `--vault` map) is translated (`DiskToLocal`); anything not in the VFS is
/// an error that says what to pass.
fn scenario_vfs_path(lua: &Lua, path: &str) -> LuaResult<String> {
    let Some(vfs) = lua.app_data_ref::<std::sync::Arc<std::sync::RwLock<crate::engine::vfs::Vfs>>>().map(|v| v.clone()) else {
        return Ok(path.to_owned());
    };
    let vfs = vfs.read().map_err(|_| LuaError::runtime("VFS poisoned"))?;
    let local = if vfs.read(path).is_ok() { path.to_owned() } else { vfs.disk_to_local(path) };
    if vfs.read(&local).is_err() {
        return Err(LuaError::runtime(format!(
            "scenario {path} is not in the VFS: pass a VFS path like /maps/<folder>/<name>_scenario.lua (vault maps need --vault <dir>)"
        )));
    }
    Ok(local)
}

/// Profiling builds: FAF's `BeginSession` steps (simInit's `BeginSession*`
/// functions, the modules it calls, each brain's `OnBeginSession`) wrapped
/// in named spans, so the trace shows which step the time goes to.
#[cfg(feature = "tracy")]
fn profile_begin_session(lua: &Lua) -> LuaResult<()> {
    lua.load(
        r#"
        local enter, leave = __prof_enter, __prof_leave
        local function wrapped(f, label)
            return function(...)
                enter(label)
                local r = table.pack(pcall(f, ...))
                leave()
                if not r[1] then error(r[2], 0) end
                return unpack(r, 2, r.n)
            end
        end
        local function wrap(t, k, label)
            if t and type(t[k]) == 'function' then t[k] = wrapped(t[k], label) end
        end
        for _, n in {'BeginSessionAI', 'BeginSessionMapSetup', 'BeginSessionEffects', 'BeginSessionTeams', 'OnStartOffMapPreventionThread'} do
            wrap(_G, n, n)
        end
        wrap(import('/lua/sim/markerutilities.lua'), 'Setup', 'markerutilities.Setup')
        local su = import('/lua/sim/scenarioutilities.lua')
        wrap(su, 'CreateProps', 'scenarioutilities.CreateProps')
        wrap(su, 'CreateResources', 'scenarioutilities.CreateResources')
        wrap(import('/lua/sim/score.lua'), 'init', 'score.init')
        wrap(import('/lua/sim/recall.lua'), 'init', 'recall.init')
        if ScenarioInfo.Env then wrap(ScenarioInfo.Env, 'OnPopulate', 'Env.OnPopulate') end
        -- The AI brains' OnBeginSession steps.
        wrap(import('/lua/sim/navutils.lua'), 'Generate', 'navutils.Generate')
        local mu = import('/lua/sim/markerutilities.lua')
        for _, n in {'GenerateExpansionMarkers', 'GenerateRallyPointMarkers', 'GenerateNavalAreaMarkers'} do wrap(mu, n, 'markerutilities.' .. n) end
        for _, m in {'gridreclaim', 'gridbrain', 'gridrecon', 'griddeposits', 'gridpresence'} do
            wrap(import('/lua/ai/' .. m .. '.lua'), 'Setup', m .. '.Setup')
        end
        -- The generator's stages it exports (the rest are locals).
        local ng = import('/lua/sim/navgenerator.lua')
        for _, k in {'Generate', 'InitCaches', 'PopulateCaches', 'ComputeLandPathingMatrix', 'ComputeHoverPathingMatrix', 'ComputeNavalPathingMatrix', 'ComputeAmphPathingMatrix'} do
            wrap(ng, k, 'navgenerator.' .. k)
        end
        for _, brain in ArmyBrains do
            if type(brain.OnBeginSession) == 'function' then
                brain.OnBeginSession = wrapped(brain.OnBeginSession, 'brain:OnBeginSession ' .. tostring(brain.Nickname))
            end
        end
        "#,
    )
    .set_name("=[engine profile BeginSession]")
    .exec()
}


/// What the front end starts (the renderer sends it to the engine thread).
#[derive(Clone, Debug)]
pub enum Launch {
    /// A lobby's game (`CLobby:LaunchGame`, see [`SessionConfig::launch`]),
    /// recorded for the player's profile (its name).
    Game { config: crate::engine::sim::lua_data::LuaData, profile: Option<String> },
    /// A recorded game (`LaunchReplaySession`): its file.
    Replay(std::path::PathBuf),
    /// The last session again, from its start (`RestartSession`): the same
    /// game, or the same recording.
    Restart,
    /// The last session (a replay) again, carried on from its latest
    /// keyframe at or before this tick (a replay seek; from the start
    /// without one).
    Seek(u64),
}
