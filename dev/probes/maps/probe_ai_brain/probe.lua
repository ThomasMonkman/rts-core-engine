-- Differential probe: army brain queries (moho.aibrain_methods): units around
-- a point, unit lists, stats and stat triggers, economy, personality,
-- terrain helpers. One site, army 1 asks.
--   real game: dev/probes/run_faf.ps1 probe_ai_brain 60
--   ours: --headless --no-ai --ticks 160 --script dev/probes/maps/probe_ai_brain/probe.lua
-- Lines: PROBE <tick> <case> ok n=<results> <values> | ERR <message>.
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local TESTER = armies[1]
local ENEMY = 'NEUTRAL_CIVILIAN'
local enemyIndex
for i, a in armies do
    if a == ENEMY then enemyIndex = i end
end
SetAlliance(TESTER, ENEMY, 'Enemy')
SetAlliance(ENEMY, TESTER, 'Enemy')
local brain = ArmyBrains[1]
local ebrain = ArmyBrains[enemyIndex]

-- Names for the objects the probe made (units, platoons, brains), so both
-- engines print them the same.
local tags = {}
for i, b in ArmyBrains do tags[b] = 'brain' .. i end
local function N(v)
    if type(v) ~= 'number' then return tostring(v) end
    if math.abs(v) < 0.005 then v = 0 end
    return fmt('%.2f', v)
end
local S
local function S0(v, d)
    d = d or 0
    local t = type(v)
    if t == 'number' then return N(v) end
    if t == 'string' then return '"' .. v .. '"' end
    if t ~= 'table' then
        if t == 'userdata' or t == 'function' or t == 'thread' then return t end
        return tostring(v)
    end
    if tags[v] then return tags[v] end
    local ok, id = pcall(function() return v.GetUnitId and v:GetUnitId() end)
    if ok and id then return 'unit:' .. id end
    if rawget(v, '_c_object') ~= nil then return 'cobj' end
    if d > 3 then return '{..}' end
    local out = {}
    local n = table.getn(v)
    for i = 1, n do table.insert(out, S(v[i], d + 1)) end
    local keys = {}
    for k in v do
        if not (type(k) == 'number' and k >= 1 and k <= n) then table.insert(keys, tostring(k)) end
    end
    table.sort(keys)
    for _, k in keys do
        local val = v[k]
        if val == nil then val = v[tonumber(k)] end
        table.insert(out, k .. '=' .. S(val, d + 1))
    end
    return '{' .. table.concat(out, ',') .. '}'
end
S = function(v, d)
    local ok, r = pcall(S0, v, d)
    if ok then return r end
    return '<unprintable ' .. type(v) .. '>'
end
local start = GetGameTick()
local function T() return GetGameTick() - start end
-- Calls f, logs its results (up to the last non-nil of 6) or its error.
local function R(tag, f)
    local ok, a, b, c, d, e, g = pcall(f)
    if ok then
        local r = { a, b, c, d, e, g }
        local n = 0
        for i = 1, 6 do if r[i] ~= nil then n = i end end
        local line = 'PROBE ' .. T() .. ' ' .. tag .. ' ok n=' .. n
        for i = 1, n do line = line .. ' ' .. S(r[i]) end
        LOG(line)
    else
        LOG('PROBE ' .. T() .. ' ' .. tag .. ' ERR ' .. tostring(a))
    end
    return a, b
end
local function make(name, bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    pcall(function() u:SetFireState(1) end)
    if name then tags[u] = name end
    return u
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function wait_to(t)
    while T() < t do WaitTicks(1) end
end

local QX, QZ = 656, 160
local Q = pos(QX, QZ)
local tA = make('tA', 'uel0201', TESTER, QX, QZ)
local tB = make('tB', 'uel0201', TESTER, QX + 10, QZ)
local tC = make('tC', 'uel0201', TESTER, QX + 20, QZ)
local pg = make('pgen', 'ueb1101', TESTER, QX, QZ + 12)
local mex = make('mex', 'ueb1103', TESTER, QX + 8, QZ - 16)
local eng = make('eng', 'uel0105', TESTER, QX - 4, QZ + 6)
local eT = make('eT', 'uel0201', ENEMY, QX - 10, QZ)
SetAlliance(TESTER, armies[2], 'Ally')
SetAlliance(armies[2], TESTER, 'Ally')
SetAlliance(TESTER, armies[3], 'Neutral')
SetAlliance(armies[3], TESTER, 'Neutral')
local aT = make('aT', 'uel0201', armies[2], QX, QZ - 10)
local nT = make('nT', 'uel0201', armies[3], QX - 10, QZ - 10)
local eFar = make('eFar', 'uel0201', ENEMY, 352, 880)
local eFarB = make('eFarB', 'ueb1101', ENEMY, 352, 890)
local FAR = pos(352, 880)

local C = categories
local function around(phase)
    for _, al in { 'Ally', 'Enemy', 'Neutral', 'Self', 'ally', 'Bogus' } do
        R('around ' .. phase .. ' ' .. al, function() return brain:GetUnitsAroundPoint(C.ALLUNITS, Q, 30, al) end)
        R('numaround ' .. phase .. ' ' .. al, function() return brain:GetNumUnitsAroundPoint(C.ALLUNITS, Q, 30, al) end)
    end
    R('around ' .. phase .. ' noalliance', function() return brain:GetUnitsAroundPoint(C.ALLUNITS, Q, 30) end)
    R('numaround ' .. phase .. ' noalliance', function() return brain:GetNumUnitsAroundPoint(C.ALLUNITS, Q, 30) end)
    R('around ' .. phase .. ' STRUCTURE Ally', function() return brain:GetUnitsAroundPoint(C.STRUCTURE, Q, 30, 'Ally') end)
    R('around ' .. phase .. ' MOBILE-ENGINEER Ally', function() return brain:GetUnitsAroundPoint(C.MOBILE - C.ENGINEER, Q, 30, 'Ally') end)
    R('around ' .. phase .. ' none r1', function() return brain:GetUnitsAroundPoint(C.ALLUNITS, pos(208, 128), 1, 'Ally') end)
    R('numaround ' .. phase .. ' none r1', function() return brain:GetNumUnitsAroundPoint(C.ALLUNITS, pos(208, 128), 1, 'Ally') end)
    for _, r in { 9.5, 10, 10.5, 11, 19.5, 20, 20.5, 21, 0, -5 } do
        R('around ' .. phase .. ' radius ' .. r, function() return brain:GetUnitsAroundPoint(C.DIRECTFIRE, Q, r, 'Ally') end)
    end
    R('around ' .. phase .. ' y+50 r12', function() return brain:GetUnitsAroundPoint(C.DIRECTFIRE, { Q[1], Q[2] + 50, Q[3] }, 12, 'Ally') end)
    R('around ' .. phase .. ' far Enemy (no intel)', function() return brain:GetUnitsAroundPoint(C.ALLUNITS, FAR, 20, 'Enemy') end)
    R('numaround ' .. phase .. ' far Enemy (no intel)', function() return brain:GetNumUnitsAroundPoint(C.ALLUNITS, FAR, 20, 'Enemy') end)
    R('ebrain around ' .. phase .. ' Enemy', function() return ebrain:GetUnitsAroundPoint(C.ALLUNITS, Q, 30, 'Enemy') end)
    R('ebrain around ' .. phase .. ' Ally', function() return ebrain:GetUnitsAroundPoint(C.ALLUNITS, Q, 30, 'Ally') end)
end
local function lists(phase)
    R('list ' .. phase .. ' ALL false', function() return brain:GetListOfUnits(C.ALLUNITS, false) end)
    R('list ' .. phase .. ' ALL true', function() return brain:GetListOfUnits(C.ALLUNITS, true) end)
    R('list ' .. phase .. ' ALL false true', function() return brain:GetListOfUnits(C.ALLUNITS, false, true) end)
    R('list ' .. phase .. ' ALL true true', function() return brain:GetListOfUnits(C.ALLUNITS, true, true) end)
    R('list ' .. phase .. ' ALL false false', function() return brain:GetListOfUnits(C.ALLUNITS, false, false) end)
    R('list ' .. phase .. ' ALL noflag', function() return brain:GetListOfUnits(C.ALLUNITS) end)
    R('list ' .. phase .. ' FACTORY false', function() return brain:GetListOfUnits(C.FACTORY, false) end)
    R('list ' .. phase .. ' NAVAL false', function() return brain:GetListOfUnits(C.NAVAL, false) end)
    R('current ' .. phase .. ' ALL', function() return brain:GetCurrentUnits(C.ALLUNITS) end)
    R('current ' .. phase .. ' STRUCTURE', function() return brain:GetCurrentUnits(C.STRUCTURE) end)
    R('current ' .. phase .. ' FACTORY', function() return brain:GetCurrentUnits(C.FACTORY) end)
    R('current ' .. phase .. ' NAVAL', function() return brain:GetCurrentUnits(C.NAVAL) end)
    R('findunit ' .. phase .. ' DIRECTFIRE false', function() return brain:FindUnit(C.DIRECTFIRE, false) end)
    R('findunit ' .. phase .. ' DIRECTFIRE true', function() return brain:FindUnit(C.DIRECTFIRE, true) end)
    R('findunit ' .. phase .. ' ENGINEER true', function() return brain:FindUnit(C.ENGINEER, true) end)
    R('findunit ' .. phase .. ' NAVAL false', function() return brain:FindUnit(C.NAVAL, false) end)
    R('anyengbuilding ' .. phase .. ' ALL', function() return brain:IsAnyEngineerBuilding(C.ALLUNITS) end)
    R('anyengbuilding ' .. phase .. ' FACTORY', function() return brain:IsAnyEngineerBuilding(C.FACTORY) end)
    R('anyengbuilding ' .. phase .. ' ENERGYPRODUCTION', function() return brain:IsAnyEngineerBuilding(C.ENERGYPRODUCTION) end)
    R('numbuilding ' .. phase .. ' FACTORY by ENGINEER', function() return brain:NumCurrentlyBuilding(C.FACTORY, C.ENGINEER) end)
    R('numbuilding ' .. phase .. ' ALL by ALL', function() return brain:NumCurrentlyBuilding(C.ALLUNITS, C.ALLUNITS) end)
    R('numbuilding ' .. phase .. ' FACTORY by FACTORY', function() return brain:NumCurrentlyBuilding(C.FACTORY, C.FACTORY) end)
    for _, st in { 'Units_Active', 'Units_History', 'Units_BeingBuilt', 'Units_Killed', 'Enemies_Killed', 'Units_MassValue_Built', 'Units_MassValue_Lost', 'Enemies_MassValue_Killed', 'Units_TotalDamageDealt', 'Units_TotalDamageReceived', 'Bogus_Stat' } do
        R('bpstat ' .. phase .. ' ' .. st .. ' ALL', function() return brain:GetBlueprintStat(st, C.ALLUNITS) end)
        R('bpstat ' .. phase .. ' ' .. st .. ' STRUCTURE', function() return brain:GetBlueprintStat(st, C.STRUCTURE) end)
    end
    for _, st in { 'UnitCap_Current', 'UnitCap_MaxCap', 'Units_Active', 'Units_History', 'Units_Killed', 'Enemies_Killed', 'Economy_Income_Mass', 'Economy_Income_Energy',
        'Economy_Stored_Mass', 'Economy_MaxStorage_Mass', 'Economy_MaxStorage_Energy', 'Economy_Ratio_Mass', 'Economy_Trend_Energy', 'Economy_TotalProduced_Energy', 'Economy_TotalConsumed_Energy',
        'Economy_Output_Mass', 'Economy_Reclaimed_Mass', 'Economy_PeakStorage_Mass' } do
        R('armystat ' .. phase .. ' ' .. st, function() return brain:GetArmyStat(st, 0) end)
    end
end
local function econ(phase)
    for _, res in { 'MASS', 'ENERGY' } do
        local line = 'PROBE ' .. T() .. ' econ ' .. phase .. ' ' .. res
        for _, fn in { 'GetEconomyIncome', 'GetEconomyRequested', 'GetEconomyStored', 'GetEconomyStoredRatio', 'GetEconomyTrend', 'GetEconomyUsage' } do
            local ok, v = pcall(brain[fn], brain, res)
            line = line .. ' ' .. fn .. '=' .. (ok and S(v) or 'ERR')
        end
        LOG(line)
    end
end

ForkThread(function()
    LOG('PROBE 0 setup armies=' .. table.getn(armies) .. ' enemyIndex=' .. tostring(enemyIndex) .. ' brainType=' .. tostring(brain.BrainType))
    wait_to(3)
    around('A')
    lists('A')
    -- identity and simple getters
    R('GetArmyIndex', function() return brain:GetArmyIndex() end)
    R('GetFactionIndex', function() return brain:GetFactionIndex() end)
    R('ebrain GetFactionIndex', function() return ebrain:GetFactionIndex() end)
    R('GetArmyStartPos', function() return brain:GetArmyStartPos() end)
    R('ebrain GetArmyStartPos', function() return ebrain:GetArmyStartPos() end)
    R('GetMapWaterRatio', function() return brain:GetMapWaterRatio() end)
    R('GetNoRushTicks', function() return brain:GetNoRushTicks() end)
    R('IsOpponentAIRunning', function() return brain:IsOpponentAIRunning() end)
    R('GetCurrentEnemy initial', function() return brain:GetCurrentEnemy() end)
    R('SetCurrentEnemy ebrain', function() return brain:SetCurrentEnemy(ebrain) end)
    R('GetCurrentEnemy after', function() return brain:GetCurrentEnemy() end)
    R('SetCurrentEnemy ally', function() return brain:SetCurrentEnemy(ArmyBrains[2]) end)
    R('GetCurrentEnemy ally', function() return brain:GetCurrentEnemy() end)
    R('SetCurrentEnemy self', function() return brain:SetCurrentEnemy(brain) end)
    R('GetCurrentEnemy self', function() return brain:GetCurrentEnemy() end)
    R('SetCurrentEnemy nil', function() return brain:SetCurrentEnemy(nil) end)
    R('GetCurrentEnemy nil', function() return brain:GetCurrentEnemy() end)
    R('FindClosestArmyWithBase', function() return brain:FindClosestArmyWithBase() end)
    R('GetUnitBlueprint uel0201', function() local bp = brain:GetUnitBlueprint('uel0201'); return type(bp), bp and bp.BlueprintId, bp and bp == tA:GetBlueprint() end)
    R('GetUnitBlueprint UEL0201', function() local bp = brain:GetUnitBlueprint('UEL0201'); return type(bp), bp and bp.BlueprintId end)
    R('GetAvailableFactories none', function() return brain:GetAvailableFactories() end)
    R('GetArmyUnitCap', function() return GetArmyUnitCap(1) end)
    R('GetArmyUnitCostTotal', function() return GetArmyUnitCostTotal(1) end)
    R('ArmyIsCivilian enemy', function() return ArmyIsCivilian(enemyIndex) end)
    R('ArmyIsCivilian 1', function() return ArmyIsCivilian(1) end)
    R('ArmyIsOutOfGame 1', function() return ArmyIsOutOfGame(1) end)
    R('IsAlly 1 2', function() return IsAlly(1, 2) end)
    R('IsEnemy 1 enemy', function() return IsEnemy(1, enemyIndex) end)
    R('IsNeutral 1 3', function() return IsNeutral(1, 3) end)
    R('IsAlly 1 1', function() return IsAlly(1, 1) end)
    R('IsEnemy 1 1', function() return IsEnemy(1, 1) end)

    -- personality
    local okp, pers = pcall(brain.GetPersonality, brain)
    LOG('PROBE ' .. T() .. ' personality type=' .. type(pers) .. ' ok=' .. tostring(okp))
    if okp and pers then
        for _, fn in { 'GetPersonalityName', 'GetChatPersonality', 'GetDifficulty', 'GetArmySize', 'GetPlatoonSize', 'GetAttackFrequency', 'GetRepeatAttackFrequency', 'GetCounterForces',
            'GetIntelGathering', 'GetCoordinatedAttacks', 'GetExpansionDriven', 'GetTechAdvancement', 'GetUpgradesDriven', 'GetDefenseDriven', 'GetEconomyDriven',
            'GetFactoryTycoon', 'GetIntelBuildingTycoon', 'GetSuperWeaponTendency', 'GetFavouriteStructures', 'GetFavouriteUnits', 'GetAirUnitsEmphasis', 'GetTankUnitsEmphasis',
            'GetBotUnitsEmphasis', 'GetSeaUnitsEmphasis', 'GetSpecialtyForcesEmphasis', 'GetSupportUnitsEmphasis', 'GetDirectDamageEmphasis', 'GetInDirectDamageEmphasis',
            'GetSurvivalEmphasis', 'GetTeamSupport', 'GetFormationUse', 'GetTargetSpread', 'GetQuittingTendency', 'GetChatFrequency' } do
            R('pers ' .. fn, function() return pers[fn](pers) end)
        end
        R('pers AdjustDelay 10 2', function() return pers:AdjustDelay(10, 2) end)
        R('pers AdjustDelay 100 0.5', function() return pers:AdjustDelay(100, 0.5) end)
        R('pers AdjustDelay 0 1', function() return pers:AdjustDelay(0, 1) end)
        R('pers AdjustDelay 10 2 again', function() return pers:AdjustDelay(10, 2) end)
    end
    local oke, epers = pcall(ArmyBrains[2].GetPersonality, ArmyBrains[2])
    if oke and epers then
        for _, fn in { 'GetPersonalityName', 'GetDifficulty', 'GetArmySize', 'GetPlatoonSize', 'GetAttackFrequency' } do
            R('pers2 ' .. fn, function() return epers[fn](epers) end)
        end
        R('pers2 AdjustDelay 10 2', function() return epers:AdjustDelay(10, 2) end)
    end

    -- terrain helpers
    for _, p in { { 656, 160 }, { 208, 128 }, { 512, 512 }, { 0, 0 }, { 1024, 1024 } } do
        R('terrain ' .. p[1] .. ',' .. p[2], function()
            local tt = GetTerrainType(p[1], p[2])
            return GetTerrainHeight(p[1], p[2]), GetSurfaceHeight(p[1], p[2]), type(tt), tt and tt.Name, tt and tt.Description
        end)
    end
    R('terrain offmap', function() return GetTerrainHeight(-10, -10), GetSurfaceHeight(2000, 2000) end)
    R('terraintype -1,-1', function() local tt = GetTerrainType(-1, -1); return type(tt), tt and tt.Name end)
    -- blocking terrain: flat, across the site, and long lines over the map's hills
    local P2 = pos(QX + 20, QZ)
    for _, arc in { 'none', 'low', 'high', 'None', 'bogus' } do
        R('blocking flat ' .. arc, function() return brain:CheckBlockingTerrain(Q, P2, arc) end)
        R('blocking long1 ' .. arc, function() return brain:CheckBlockingTerrain(Q, pos(352, 880), arc) end)
        R('blocking long2 ' .. arc, function() return brain:CheckBlockingTerrain(pos(96, 608), pos(960, 336), arc) end)
        R('blocking long3 ' .. arc, function() return brain:CheckBlockingTerrain(pos(384, 560), pos(656, 384), arc) end)
        R('blocking long4 ' .. arc, function() return brain:CheckBlockingTerrain(pos(96, 896), pos(352, 880), arc) end)
        R('blocking long5 ' .. arc, function() return brain:CheckBlockingTerrain(pos(912, 96), pos(608, 80), arc) end)
    end
    R('blocking noarc', function() return brain:CheckBlockingTerrain(Q, P2) end)
    -- CanPathTo (unit method the AI uses)
    R('tA CanPathTo near', function() return tA:CanPathTo(pos(QX + 30, QZ)) end)
    R('tA CanPathTo water', function() return tA:CanPathTo(pos(208, 128)) end)
    R('tA CanPathTo far land', function() return tA:CanPathTo(pos(352, 880)) end)
    R('tA CanPathTo offmap', function() return tA:CanPathTo({ -100, 0, -100 }) end)

    -- economy
    wait_to(10)
    econ('t10')
    wait_to(15)
    around('A2')
    wait_to(20)
    econ('t20')
    R('GiveResource MASS 100', function() return brain:GiveResource('MASS', 100) end)
    R('GiveResource ENERGY 250', function() return brain:GiveResource('ENERGY', 250) end)
    econ('given')
    R('TakeResource MASS 30', function() return brain:TakeResource('MASS', 30) end)
    R('TakeResource ENERGY 1e7', function() return brain:TakeResource('ENERGY', 10000000) end)
    R('TakeResource MASS -5', function() return brain:TakeResource('MASS', -5) end)
    econ('taken')
    R('GiveStorage MASS 500', function() return brain:GiveStorage('MASS', 500) end)
    econ('storage')
    R('GiveResource MASS 1e6', function() return brain:GiveResource('MASS', 1000000) end)
    econ('overfull')
    R('GiveResource mass lowercase', function() return brain:GiveResource('mass', 10) end)
    R('GetEconomyStored mass lowercase', function() return brain:GetEconomyStored('mass') end)
    R('GetEconomyStored bogus', function() return brain:GetEconomyStored('BOGUS') end)
    R('TakeResource MASS all', function() return brain:TakeResource('MASS', 10000000) end)
    R('GiveResource MASS 200', function() return brain:GiveResource('MASS', 200) end)
    wait_to(21)
    econ('t21')
    wait_to(25)
    econ('t25')

    -- army stats: set/get/greater-of and triggers
    brain.OnStatsTrigger = function(self, name) LOG('PROBE ' .. T() .. ' OnStatsTrigger ' .. tostring(name)) end
    R('armystat Probe_Foo default 7', function() return brain:GetArmyStat('Probe_Foo', 7) end)
    R('armystat Probe_Foo default 9', function() return brain:GetArmyStat('Probe_Foo', 9) end)
    R('SetArmyStat Probe_Foo 5', function() return brain:SetArmyStat('Probe_Foo', 5) end)
    R('armystat Probe_Foo', function() return brain:GetArmyStat('Probe_Foo', 0) end)
    R('SetGreaterOf Probe_Foo 3', function() return brain:SetGreaterOf('Probe_Foo', 3) end)
    R('armystat Probe_Foo', function() return brain:GetArmyStat('Probe_Foo', 0) end)
    R('SetGreaterOf Probe_Foo 9', function() return brain:SetGreaterOf('Probe_Foo', 9) end)
    R('armystat Probe_Foo', function() return brain:GetArmyStat('Probe_Foo', 0) end)
    R('SetArmyStat Probe_Float 2.5', function() return brain:SetArmyStat('Probe_Float', 2.5) end)
    R('armystat Probe_Float', function() return brain:GetArmyStat('Probe_Float', 0) end)
    R('armystat Probe_Def 2.5', function() return brain:GetArmyStat('Probe_Def', 2.5) end)
    R('AddArmyStat exists', function() return type(brain.AddArmyStat) end)
    for _, cmp in { 'GreaterThanOrEqual', 'GreaterThan', 'LessThan', 'LessThanOrEqual', 'Equal' } do
        R('SetArmyStatsTrigger ' .. cmp, function() return brain:SetArmyStatsTrigger('Probe_Trig', 'trig_' .. cmp, cmp, 10) end)
    end
    R('SetArmyStatsTrigger units', function() return brain:SetArmyStatsTrigger('Units_Active', 'trig_units', 'GreaterThanOrEqual', 7, C.ALLUNITS) end)
    R('SetArmyStatsTrigger removed', function() return brain:SetArmyStatsTrigger('Probe_Trig', 'trig_removed', 'GreaterThanOrEqual', 10) end)
    R('RemoveArmyStatsTrigger', function() return brain:RemoveArmyStatsTrigger('Probe_Trig', 'trig_removed') end)
    LOG('PROBE ' .. T() .. ' triggers set')
    WaitTicks(2)
    LOG('PROBE ' .. T() .. ' set 10')
    brain:SetArmyStat('Probe_Trig', 10)
    WaitTicks(2)
    LOG('PROBE ' .. T() .. ' set 12')
    brain:SetArmyStat('Probe_Trig', 12)
    WaitTicks(2)
    LOG('PROBE ' .. T() .. ' set 12 again')
    brain:SetArmyStat('Probe_Trig', 12)
    WaitTicks(2)
    LOG('PROBE ' .. T() .. ' set 3')
    brain:SetArmyStat('Probe_Trig', 3)
    WaitTicks(2)
    LOG('PROBE ' .. T() .. ' set 12 third')
    brain:SetArmyStat('Probe_Trig', 12)
    WaitTicks(2)
    LOG('PROBE ' .. T() .. ' new unit for trig_units')
    local tD = make('tD', 'uel0201', TESTER, QX + 10, QZ + 10)
    WaitTicks(3)

    -- being built, moving
    wait_to(45)
    LOG('PROBE ' .. T() .. ' build start')
    IssueBuildMobile({ eng }, pos(QX - 14, QZ + 14), 'ueb0101', {})
    IssueMove({ tB }, pos(QX + 10, QZ + 60))
    wait_to(75)
    for _, u in brain:GetListOfUnits(C.FACTORY, false) or {} do tags[u] = 'factory' end
    for _, u in brain:GetUnitsAroundPoint(C.FACTORY, Q, 40, 'Ally') or {} do
        tags[u] = 'factory'
        LOG('PROBE ' .. T() .. ' factory fraction=' .. N(u:GetFractionComplete()) .. ' beingbuilt=' .. tostring(u:IsBeingBuilt()))
    end
    LOG('PROBE ' .. T() .. ' eng building=' .. tostring(eng:IsUnitState('Building')) .. ' tB moving=' .. tostring(tB:IsUnitState('Moving')) .. ' tA idle=' .. tostring(tA:IsIdleState()))
    around('B')
    lists('B')
    econ('t75')

    -- kills
    LOG('PROBE ' .. T() .. ' kills')
    Damage(tA, eT:GetPosition(), eT, 100000, 'Normal')
    Damage(eFar, tC:GetPosition(), tC, 100000, 'Normal')
    WaitTicks(5)
    lists('C')
    WaitTicks(30)
    LOG('PROBE done')
end)
