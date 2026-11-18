-- Probe hook, mounted at /schook by run_faf.ps1: an all-AI skirmish for a
-- probe map, when the launch has `/probeai <key>` (see run_faf.ps1's
-- `launch_args.txt`). Without it the stock `/map` launch runs (army 1 human,
-- the rest 'rush').
--   /probeai <key>[,<key>...]  the AI of every army (a list is cycled by army)
--   /probemod <uid>[,<uid>...] the sim mods the session runs (instead of the
--                              active mods in the game prefs)
-- The session is set up as ours sets up `--session --ai`: the same options
-- (and every other lobby option at its default, the mods' AI options
-- included), the factions spread UEF, Aeon, Cybran, Seraphim by army, the
-- AI's lobby list entry as `AILobbyProperties`, the map's extra armies as
-- civilians.
local ProbeStartCommandLineSession = StartCommandLineSession

local function ProbeSplit(s)
    local out = {}
    for part in string.gfind(s, '[^,]+') do
        table.insert(out, part)
    end
    return out
end

local function ProbeAllAISession(mapName, aiKeys)
    local MapUtil = import("/lua/ui/maputil.lua")
    local Mods = import("/lua/mods.lua")
    local scenario = MapUtil.LoadScenario(FixupMapName(mapName))
    if not scenario then
        error("probe launch: unable to load map " .. mapName)
    end
    VerifyScenarioConfiguration(scenario)

    local mods = Mods.GetCampaignMods(scenario)
    local modArg = GetCommandLineArg("/probemod", 1)
    if modArg then
        local selected = {}
        for _, uid in ProbeSplit(modArg[1]) do
            selected[uid] = true
        end
        mods = Mods.GetGameMods(selected)
    end

    scenario.Options = {
        FogOfWar = 'explored', NoRushOption = 'Off', PrebuiltUnits = 'Off', Difficulty = 2,
        ShareUnitCap = 'allies', Timeouts = -1, GameSpeed = 'adjustable', UnitCap = '500',
        Victory = 'sandbox', CheatsEnabled = 'true', CivilianAlliance = 'enemy',
        TeamShareOverflow = 'enabled',
    }
    local function addDefaults(list)
        if type(list) ~= 'table' then return end
        for _, option in list do
            if type(option) == 'table' and option.key and scenario.Options[option.key] == nil then
                local value = option.values and option.default and option.values[option.default]
                if type(value) == 'table' then value = value.key end
                if value ~= nil then scenario.Options[option.key] = value end
            end
        end
    end
    local lobbyOptions = import('/lua/ui/lobby/lobbyoptions.lua')
    for _, name in { 'globalOpts', 'teamOptions', 'AIOpts', 'advAIOptions', 'advGameOptions' } do
        addDefaults(rawget(lobbyOptions, name))
    end

    -- The AIs' lobby list entries: the game's, then the session mods'.
    local entries = {}
    local okTypes, aiTypes = pcall(import, '/lua/ui/lobby/aitypes.lua')
    if okTypes and type(aiTypes.aitypes) == 'table' then
        for _, ai in aiTypes.aitypes do
            if ai.key then entries[ai.key] = ai end
        end
    end
    for _, mod in mods do
        local file = mod.location .. '/lua/AI/LobbyOptions/lobbyoptions.lua'
        if DiskGetFileInfo(file) then
            addDefaults(import(file).AIOpts)
        end
        for _, f in DiskFindFiles(mod.location .. '/lua/AI/CustomAIs_v2', '*.lua') do
            local list = import(f).AI
            for _, group in { list and list.AIList, list and list.CheatAIList } do
                for _, ai in group or {} do
                    if ai.key then entries[ai.key] = ai end
                end
            end
        end
    end

    local sessionInfo = {}
    sessionInfo.playerName = 'Probe'
    sessionInfo.createReplay = false
    sessionInfo.scenarioInfo = scenario
    sessionInfo.scenarioMods = mods
    sessionInfo.teamInfo = {}
    local seed = GetCommandLineArg("/seed", 1)
    if seed then
        sessionInfo.RandomSeed = tonumber(seed[1])
    end

    local GetDefaultPlayerOptions = import("/lua/ui/lobby/lobbycomm.lua").GetDefaultPlayerOptions
    local armies = scenario.Configurations.standard.teams[1].armies
    local numColors = table.getn(import("/lua/gamecolors.lua").GameColors.PlayerColors)
    for index, name in armies do
        local key = aiKeys[math.mod(index - 1, table.getn(aiKeys)) + 1]
        local opts = GetDefaultPlayerOptions(name)
        opts.PlayerName = name
        opts.ArmyName = name
        opts.Human = false
        opts.AIPersonality = key
        opts.AILobbyProperties = entries[key]
        opts.Faction = math.mod(index - 1, 4) + 1
        opts.PlayerColor = math.mod(index, numColors)
        opts.ArmyColor = math.mod(index, numColors)
        opts.StartSpot = index
        sessionInfo.teamInfo[index] = opts
    end
    local extras = MapUtil.GetExtraArmies(scenario)
    if extras then
        for _, armyName in extras do
            local opts = GetDefaultPlayerOptions("civilian")
            opts.PlayerName = 'civilian'
            opts.Civilian = true
            opts.ArmyName = armyName
            opts.Human = false
            table.insert(sessionInfo.teamInfo, opts)
        end
    end
    return sessionInfo
end

function StartCommandLineSession(mapName, isPerfTest)
    local aiArg = GetCommandLineArg("/probeai", 1)
    if not aiArg then
        return ProbeStartCommandLineSession(mapName, isPerfTest)
    end
    LaunchSinglePlayerSession(ProbeAllAISession(mapName, ProbeSplit(aiArg[1])))
end
