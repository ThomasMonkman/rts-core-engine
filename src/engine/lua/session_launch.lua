-- Engine-side session launch (what moho's C++ does on LaunchGame), mirroring
-- FAF's SetupBotSession in /lua/SinglePlayerLaunch.lua.
-- Args: scenario path, number of armies to fill (0 = all start spots), and
-- whether they get FAF's AI (false: human slots nobody plays), and the army
-- a person plays (0 = none; always a human slot), and the number of teams
-- (spot i joins team (i - 1) % teams, FAF's alternating spots; 0 or 1 = no
-- teams), and whether armies start with a prebuilt base, and a lobby's launch
-- ({Config = gameConfig, LocalID = peer id}, or nil): its options and
-- players replace the ones made up here. Returns the army the local player
-- has in the lobby's game (nil without a lobby, or as an observer). Last, the
-- game code (`GameCode::name`: 'faf', 'loud' or 'other'). Last, the AI chosen
-- with `--ai` (`{default = key, [army] = key}`, or nil: the default below).
local scenarioPath, aiArmies, withAI, humanArmy, teamCount, prebuilt, launch, gameCode, aiChoice = ...
local isFaf, isLoud = gameCode == 'faf', gameCode == 'loud'
teamCount = teamCount or 0

local data = {}
doscript('/lua/dataInit.lua', data)
doscript(scenarioPath, data)
local scenario = data.ScenarioInfo
if not scenario then
    error("session launch: no ScenarioInfo in " .. tostring(scenarioPath))
end

local teams = scenario.Configurations and scenario.Configurations.standard
    and scenario.Configurations.standard.teams
if not teams or not teams[1] or not teams[1].armies then
    error("session launch: scenario has no standard team configuration")
end

scenario.Options = {
    FogOfWar = 'explored',
    NoRushOption = 'Off',
    PrebuiltUnits = prebuilt and 'On' or 'Off',
    Difficulty = 2,
    ShareUnitCap = 'allies',
    Timeouts = -1,
    GameSpeed = 'normal',
    UnitCap = '500',
    Victory = 'sandbox',
    CheatsEnabled = 'true',
    CivilianAlliance = 'enemy',
    TeamShareOverflow = 'enabled',
}

-- The rest of the options take the lobby's defaults (an option's `default`
-- is an index into its `values`), as a game started from a lobby has them:
-- the base lists in lobbyoptions.lua after any `/lua/CustomOptions` files
-- (LOUD's lobby merges those), then each AI mod's
-- `/lua/AI/LobbyOptions/lobbyoptions.lua` `AIOpts` (FAF's lobby,
-- `ImportModAIOptions`). Scripts read them: RNGAI takes FAF's `TeamSpawn`
-- ('fixed') to mean its enemies' start spots are known, and has its own AI
-- options (real-game run with these defaults). The options above win.
do
    local function addDefaults(list)
        if type(list) ~= 'table' then return end
        for _, option in ipairs(list) do
            if type(option) == 'table' and option.key and scenario.Options[option.key] == nil then
                local value = option.values and option.default and option.values[option.default]
                if type(value) == 'table' then value = value.key end
                if value ~= nil then scenario.Options[option.key] = value end
            end
        end
    end
    local listNames = { 'LobbyGlobalOptions', 'globalOpts', 'teamOptions', 'AIOpts', 'advAIOptions', 'advGameOptions' }
    local files = DiskFindFiles('/lua/CustomOptions', '*.lua')
    table.insert(files, '/lua/ui/lobby/lobbyoptions.lua')
    for _, file in ipairs(files) do
        local ok, module = pcall(import, file)
        if ok then
            for _, name in ipairs(listNames) do addDefaults(rawget(module, name)) end
        end
    end
    for _, mod in ipairs(__active_mods or {}) do
        local file = mod.location and (mod.location .. '/lua/AI/LobbyOptions/lobbyoptions.lua')
        if file and DiskGetFileInfo(file) then
            local ok, module = pcall(import, file)
            if ok then addDefaults(rawget(module, 'AIOpts')) end
        end
    end
end

local function playerOptions(name)
    return {
        Team = 1, PlayerColor = 1, ArmyColor = 1, StartSpot = 1, Ready = false,
        Faction = 1, PlayerName = name, AIPersonality = '', Human = false, Civilian = false,
    }
end

-- FAF's matchmaker colours (other game code, e.g. LOUD, has none: palette order).
local gameColorsModule = import('/lua/gamecolors.lua')
local warmCold = rawget(gameColorsModule, 'MapToWarmCold') or function() return nil end
local gameColors = rawget(gameColorsModule, 'GameColors')

-- The AI the armies get: FAF's maintained 'adaptive' ('rush' is a legacy
-- brain whose plan loop no longer works without the deprecated skirmish
-- systems). Game code that isn't FAF's may not have
-- it (LOUD has only 'loud'): then the first of its lobby's AI list.
local defaultAI = 'adaptive'
-- The lobby names an AI player after its list entry ('AI: LOUD'); LOUD's
-- brain only starts its plan when a sim mod is active if the player's name
-- is one of its AIs' (`aibrain.lua` `IsNicknameALoudNickname`).
local defaultAIName
if not isFaf then
    local ok, lobbyTypes = pcall(import, '/lua/ui/lobby/aitypes.lua')
    local list = ok and rawget(lobbyTypes, 'aitypes')
    if type(list) == 'table' and list[1] then
        defaultAI, defaultAIName = list[1].key, list[1].name
        for _, ai in ipairs(list) do
            if ai.key == 'adaptive' then defaultAI, defaultAIName = 'adaptive', ai.name end
        end
    end
end

-- The game's lobby AI list, with the session's mods' AIs: FAF's
-- `GetAItypes` (it adds each sim mod's `/lua/AI/CustomAIs_v2`), else the
-- module's `aitypes` list. FAF's asks `/lua/mods.lua` for the mods, which
-- needs the interface (`GetPreference`): here it's given the session's
-- `__active_mods`, the mods actually running.
local function lobbyAITypes()
    local env = setmetatable({}, { __index = _G })
    env.import = function(path)
        if string.lower(path) == '/lua/mods.lua' then
            return { GetGameMods = function() return __active_mods or {} end }
        end
        return import(path)
    end
    local ok, err = pcall(doscript, '/lua/ui/lobby/aitypes.lua', env)
    if not ok then
        error("--ai: can't read the lobby's AI list (/lua/ui/lobby/aitypes.lua): " .. tostring(err), 0)
    end
    local list = type(rawget(env, 'GetAItypes')) == 'function' and env.GetAItypes() or rawget(env, 'aitypes')
    return type(list) == 'table' and list or {}
end

-- The `--ai` keys, checked against that list: `--ai list` or an unknown
-- key stops with the keys there are.
local chosenAI = {}
if aiChoice then
    local types = lobbyAITypes()
    local byKey = {}
    for _, ai in ipairs(types) do
        if ai.key then byKey[ai.key] = ai end
    end
    local function keys()
        local lines = {}
        for _, ai in ipairs(types) do
            if ai.key then table.insert(lines, '  ' .. ai.key .. '  ' .. tostring(LOC and LOC(ai.name) or ai.name)) end
        end
        return table.concat(lines, '\n')
    end
    local function check(key, what)
        if key == 'list' then
            error('AI keys (the game\'s, and the session\'s mods\'):\n' .. keys(), 0)
        end
        if not byKey[key] then
            error('--ai ' .. what .. ': no AI with the key ' .. key .. ' (an AI mod needs its --mod). The keys:\n' .. keys(), 0)
        end
        return byKey[key]
    end
    if aiChoice.default then
        chosenAI.default = check(aiChoice.default, aiChoice.default)
    end
    for army, key in pairs(aiChoice) do
        if type(army) == 'number' then
            chosenAI[army] = check(key, army .. '=' .. key)
        end
    end
end

local armySetup = {}
local index = 0
local starts = teams[1].armies
local localArmy

if launch then
    -- What moho's LaunchGame gives the sim: the lobby's options, and an army
    -- for each player at the start spot of their `PlayerOptions` slot
    -- (`ArmyName`), numbered in the scenario's army order; free spots get no
    -- army (inferred, see docs/research/verifying-against-the-binary.md).
    -- Observers have no army. The slot, not `StartSpot`: the exe never reads
    -- `StartSpot` (no such string), and the lobbies put each player at the
    -- slot of their spot (FAF's sets `StartSpot = slot`; the retail lobby's
    -- and LOUD's leave every player at the default 1).
    local config = launch.Config
    scenario.Options = config.GameOptions or {}
    local players = config.PlayerOptions or {}
    for spot = 1, table.getn(starts) do
        local name, player = starts[spot], players[spot]
        if player then
            index = index + 1
            local opts = table.copy(player)
            opts.ArmyName, opts.ArmyIndex = name, index
            opts.AIPersonality = opts.AIPersonality or ''
            opts.Civilian = opts.Civilian or false
            armySetup[name] = opts
            if opts.Human and opts.OwnerID == launch.LocalID then localArmy = index end
        end
    end
    starts = {}
end
local count = (aiArmies > 0 and aiArmies < table.getn(starts)) and aiArmies or table.getn(starts)
if aiChoice and launch then
    WARN('--ai is ignored: the lobby\'s launch names each slot\'s AI')
end
for army in pairs(chosenAI) do
    if type(army) == 'number' and not launch then
        if army > count then
            error('--ai ' .. army .. '=...: there are only ' .. count .. ' armies', 0)
        elseif army == humanArmy or not withAI then
            error('--ai ' .. army .. '=...: army ' .. army .. ' is not an AI', 0)
        end
    end
end
for i = 1, count do
    local name = starts[i]
    index = index + 1
    local opts = playerOptions(name)
    opts.ArmyName = name
    if withAI and i ~= humanArmy then
        local chosen = chosenAI[i] or chosenAI.default
        local aiKey, aiName = defaultAI, defaultAIName
        if chosen then
            -- As the lobby's `GetAIPlayerData`: the list entry goes along.
            aiKey, opts.AILobbyProperties = chosen.key, chosen
            if not isFaf then aiName = chosen.name end
        end
        opts.AIPersonality = aiKey
        -- Unique, as the lobby's `MakeValidPlayerName` keeps them (its
        -- suffix format is a guess).
        if aiName then
            opts.PlayerName = (i == 1) and aiName or (aiName .. ' (' .. i .. ')')
        end
    else
        opts.Human = true
    end
    -- FAF's team numbers: 1 = no team, 2.. = teams.
    local teamed = teamCount > 1
    if teamed then
        opts.Team = math.mod(i - 1, teamCount) + 2
    end
    -- deterministic faction spread (UEF, Aeon, Cybran, Seraphim), each team
    -- getting one of each in turn
    local k = teamed and math.floor((i - 1) / teamCount) or (i - 1)
    opts.Faction = math.mod(k, 4) + 1
    -- the matchmaker's colours: warm against cold by start spot (FAF's
    -- `MapToWarmCold` covers 16 spots; the rest in palette order)
    local color = warmCold(i) or i
    opts.PlayerColor, opts.ArmyColor = color, color
    -- LOUD's lobby gives each player an RGB colour its sim sets
    -- (`SetArmyColor` in `InitializeTeams`): the palette's; and an AI
    -- cheat multiplier and adaptive-cheat mode (its `HostTryAddPlayer`:
    -- `Mult = '1.0'`, `ACT = 1`, no adaptive cheat).
    if isLoud then
        opts.Mult, opts.ACT = '1.0', 1
        local hex = gameColors and gameColors.PlayerColors and gameColors.PlayerColors[color]
        if type(hex) == 'string' then
            hex = string.sub(hex, -6)
            opts.WheelColor = { tonumber(string.sub(hex, 1, 2), 16), tonumber(string.sub(hex, 3, 4), 16), tonumber(string.sub(hex, 5, 6), 16) }
        end
    end
    opts.StartSpot = i
    opts.ArmyIndex = index
    armySetup[name] = opts
end

-- LOUD's lobby counts its players (people and AIs) into the options.
if isLoud and scenario.Options.PlayerCount == nil then
    scenario.Options.PlayerCount = count
end

local extras = scenario.Configurations.standard.customprops
    and scenario.Configurations.standard.customprops.ExtraArmies
if extras then
    for _, name in STR_GetTokens(extras, ' ') do
        index = index + 1
        local opts = playerOptions('civilian')
        opts.ArmyName, opts.Civilian, opts.ArmyIndex = name, true, index
        armySetup[name] = opts
    end
end

scenario.ArmySetup = armySetup
ScenarioInfo = scenario
return localArmy
