-- Differential probe: attack vectors, a platoon's transport calls, and the
-- brain / platoon calls given bad arguments (error text, nil or crash; each
-- announced by a 'risky' line first).
--   real game: dev/probes/run_faf.ps1 probe_ai_misc 120
--   ours: --headless --no-ai --ticks 600 --script dev/probes/maps/probe_ai_misc/probe.lua
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

local C = categories
local QX, QZ = 656, 160
local Q = pos(QX, QZ)
local t1 = make('t1', 'uel0201', TESTER, QX, QZ)
local t2 = make('t2', 'uel0201', TESTER, QX + 3, QZ)
local a1 = make('a1', 'uel0103', TESTER, QX + 6, QZ)
local tr = make('tr', 'uea0107', TESTER, QX, QZ + 8)
-- enemy base for attack vectors
local EX, EZ = 800, 336
local es = {
    make('es1', 'ueb1101', ENEMY, EX, EZ),
    make('es2', 'ueb1101', ENEMY, EX + 40, EZ),
    make('es3', 'ueb2101', ENEMY, EX, EZ + 40),
    make('em1', 'uel0201', ENEMY, EX - 40, EZ),
}
local function vecs(tag)
    local ok, v = pcall(brain.GetAttackVectors, brain)
    if not ok then LOG('PROBE ' .. T() .. ' vectors ' .. tag .. ' ERR ' .. tostring(v)); return end
    if type(v) ~= 'table' then LOG('PROBE ' .. T() .. ' vectors ' .. tag .. ' ' .. tostring(v)); return end
    local n = table.getn(v)
    local line = 'PROBE ' .. T() .. ' vectors ' .. tag .. ' n=' .. n
    for i = 1, math.min(n, 40) do line = line .. ' ' .. S(v[i]) end
    LOG(line)
end

ForkThread(function()
    LOG('PROBE 0 setup')
    wait_to(3)
    local pers = brain:GetPersonality()
    for _, ab in { {10, 0}, {10, 0.25}, {10, 0.5}, {10, 1}, {10, 1.5}, {10, 3}, {10, -1}, {100, 0.25}, {100, 0.9}, {100, 2}, {1, 2}, {-10, 2} } do
        R('AdjustDelay ' .. ab[1] .. ' ' .. ab[2], function() return pers:AdjustDelay(ab[1], ab[2]) end)
    end
    vecs('initial')
    R('SetUpAttackVectorsToArmy no enemy', function() return brain:SetUpAttackVectorsToArmy(C.STRUCTURE) end)
    vecs('no enemy')
    brain:SetCurrentEnemy(ebrain)
    R('SetUpAttackVectorsToArmy STRUCTURE', function() return brain:SetUpAttackVectorsToArmy(C.STRUCTURE) end)
    vecs('STRUCTURE')
    R('SetUpAttackVectorsToArmy nocat', function() return brain:SetUpAttackVectorsToArmy() end)
    vecs('nocat')
    R('SetUpAttackVectorsToArmy MOBILE', function() return brain:SetUpAttackVectorsToArmy(C.MOBILE) end)
    vecs('MOBILE')
    R('SetUpAttackVectorsToArmy DEFENSE', function() return brain:SetUpAttackVectorsToArmy(C.DEFENSE) end)
    vecs('DEFENSE')
    R('SetUpAttackVectorsToArmy NAVAL (none)', function() return brain:SetUpAttackVectorsToArmy(C.NAVAL) end)
    vecs('NAVAL')
    brain:SetUpAttackVectorsToArmy(C.STRUCTURE)
    local p = brain:MakePlatoon('ProbeM', '')
    tags[p] = 'P'
    brain:AssignUnitsToPlatoon(p, { t1, t2 }, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(p, { a1 }, 'Artillery', 'None')
    brain:AssignUnitsToPlatoon(p, { tr }, 'Scout', 'None')
    for _, cmp in { 'Closest', 'Furthest', 'LeastDefended', 'HighestValue', 'Bogus' } do
        R('PickBestAttackVector ' .. cmp .. ' STRUCTURE', function() return brain:PickBestAttackVector(p, 'Attack', 'Enemy', cmp, C.STRUCTURE) end)
        R('PickBestAttackVector ' .. cmp .. ' ALLUNITS', function() return brain:PickBestAttackVector(p, 'Attack', 'Enemy', cmp, C.ALLUNITS) end)
    end
    R('PickBestAttackVector FAF order (cat, cmp)', function() return brain:PickBestAttackVector(p, 'Attack', 'Enemy', C.STRUCTURE, 'Closest') end)
    R('PickBestAttackVector Ally', function() return brain:PickBestAttackVector(p, 'Attack', 'Ally', 'Closest', C.STRUCTURE) end)
    R('PickBestAttackVector empty squad', function() return brain:PickBestAttackVector(p, 'Guard', 'Enemy', 'Closest', C.STRUCTURE) end)
    R('GetPlatoonLifetimeStats', function() return p:GetPlatoonLifetimeStats() end)
    R('GetFerryBeacons', function() return p:GetFerryBeacons() end)
    R('CanConsiderFormingPlatoon', function() return brain:GetPlatoonUniquelyNamed('ArmyPool'):CanConsiderFormingPlatoon({ 'X', '', { 'uel0201', 1, 1, 'Attack', 'None' } }, 'X') end)
    R('DisbandOnIdle', function() return p:DisbandOnIdle() end)

    -- transports: load the platoon's land units into its transport, drop them 40 away
    wait_to(10)
    local function tstate(tag)
        local line = 'PROBE ' .. T() .. ' transport ' .. tag
        for _, u in { t1, t2, a1, tr } do
            local x, y, z = u:GetPositionXYZ()
            line = line .. ' ' .. tags[u] .. '=' .. N(x) .. ',' .. N(z) .. (u:IsUnitState('Attached') and '/att' or '') .. '/q' .. table.getn(u:GetCommandQueue())
        end
        local ok, v = pcall(p.IsFerrying, p, 'Attack')
        line = line .. ' ferrying=' .. (ok and tostring(v) or 'ERR')
        ok, v = pcall(p.IsMoving, p, 'Attack')
        line = line .. ' moving=' .. (ok and tostring(v) or 'ERR')
        ok, v = pcall(function() return table.getn(tr:GetCargo()) end)
        line = line .. ' cargo=' .. (ok and tostring(v) or 'ERR')
        LOG(line)
    end
    LOG('PROBE ' .. T() .. ' before LoadUnits')
    local lc = R('LoadUnits LAND', function() return type(p:LoadUnits(C.LAND)) end)
    for i = 1, 12 do
        tstate('load+' .. (i - 1) * 10)
        WaitTicks(10)
    end
    LOG('PROBE ' .. T() .. ' before UnloadAllAtLocation')
    R('UnloadAllAtLocation', function() return type(p:UnloadAllAtLocation(pos(QX + 40, QZ + 20))) end)
    for i = 1, 20 do
        tstate('unload+' .. (i - 1) * 10)
        WaitTicks(10)
    end
    R('P exists after DisbandOnIdle', function() return brain:PlatoonExists(p) end)
    LOG('PROBE ' .. T() .. ' before MoveToLocation useTransports')
    R('MoveToLocation useTransports', function() return type(p:MoveToLocation(pos(QX, QZ + 10), true)) end)
    for i = 1, 8 do
        tstate('movetr+' .. (i - 1) * 10)
        WaitTicks(10)
    end
    R('Stop', function() return p:Stop() end)

    -- Calls with bad arguments, each announced first (the real game may crash on one).
    local function risky(tag, f)
        LOG('PROBE ' .. T() .. ' risky ' .. tag)
        R('risky ' .. tag, f)
        WaitTicks(2)
    end
    risky('GetThreatAtPosition bogus type', function() return brain:GetThreatAtPosition(Q, 0, true, 'Bogus') end)
    risky('GetThreatAtPosition no radius', function() return brain:GetThreatAtPosition(Q) end)
    risky('GetThreatAtPosition 2-vector', function() return brain:GetThreatAtPosition({ QX, QZ }, 0, true, 'Overall') end)
    risky('GetHighestThreatPosition bogus type', function() return brain:GetHighestThreatPosition(0, true, 'Bogus') end)
    risky('AssignThreatAtPosition bogus type', function() return brain:AssignThreatAtPosition(Q, 5, 0.1, 'Bogus') end)
    risky('GetThreatAtPosition army 99', function() return brain:GetThreatAtPosition(Q, 0, true, 'Overall', 99) end)
    risky('GetThreatAtPosition army 0', function() return brain:GetThreatAtPosition(Q, 0, true, 'Overall', 0) end)
    risky('GetUnitBlueprint bogus', function() return brain:GetUnitBlueprint('zzz9999') end)
    risky('GetPlatoonUnits squad bogus', function() return p:GetSquadUnits('Bogus') end)
    risky('AssignUnitsToPlatoon bogus squad', function() return brain:AssignUnitsToPlatoon(p, { t1 }, 'Bogus', 'None') end)
    risky('AssignUnitsToPlatoon missing name', function() return brain:AssignUnitsToPlatoon('NoSuchPlatoon', { t1 }, 'Attack', 'None') end)
    risky('AssignUnitsToPlatoon enemy unit', function() return brain:AssignUnitsToPlatoon(p, { es[4] }, 'Attack', 'None') end)
    risky('P units after enemy assign', function() return p:GetPlatoonUnits() end)
    risky('CalculatePlatoonThreat bogus', function() return p:CalculatePlatoonThreat('Bogus', C.ALLUNITS) end)
    risky('GetArmyStat no default', function() return brain:GetArmyStat('Probe_NoDefault') end)
    risky('GetBlueprintStat no category', function() return brain:GetBlueprintStat('Units_Active') end)
    risky('GetEconomyStored no arg', function() return brain:GetEconomyStored() end)
    risky('GetListOfUnits nil category', function() return brain:GetListOfUnits(nil, false) end)
    risky('GetUnitsAroundPoint nil pos', function() return brain:GetUnitsAroundPoint(C.ALLUNITS, nil, 10, 'Ally') end)
    -- takes the real game down
    -- risky('CanBuildStructureAt bogus bp', function() return brain:CanBuildStructureAt('zzz9999', Q) end)
    risky('DecideWhatToBuild nil template', function() return brain:DecideWhatToBuild(t1, 'T1LandFactory', nil) end)
    risky('BuildStructure bogus bp', function() return brain:BuildStructure(t1, 'zzz9999', { QX, QZ, 0 }, false) end)
    risky('FormPlatoon bogus template', function() return brain:GetPlatoonUniquelyNamed('ArmyPool'):FormPlatoon({ 'X' }, 1, Q, 10) end)
    risky('PlatoonExists nil', function() return brain:PlatoonExists(nil) end)
    risky('PlatoonExists table', function() return brain:PlatoonExists({}) end)
    risky('MakePlatoon no plan', function() return type(brain:MakePlatoon('ProbeNoPlan')) end)
    risky('MakePlatoon no args', function() return type(brain:MakePlatoon()) end)
    risky('DisbandPlatoon ArmyPool', function() return brain:DisbandPlatoon(brain:GetPlatoonUniquelyNamed('ArmyPool')) end)
    risky('ArmyPool exists after', function() return brain:PlatoonExists(brain:GetPlatoonUniquelyNamed('ArmyPool')) end)
    risky('DisbandPlatoon twice', function() brain:DisbandPlatoon(p); return brain:DisbandPlatoon(p) end)
    risky('GetPlatoonUnits on disbanded', function() return p:GetPlatoonUnits() end)
    risky('GetPlatoonPosition on disbanded', function() return p:GetPlatoonPosition() end)
    risky('MoveToLocation on disbanded', function() return p:MoveToLocation(Q, false) end)
    risky('SetCurrentEnemy table', function() return brain:SetCurrentEnemy({}) end)
    risky('GiveResource bogus', function() return brain:GiveResource('BOGUS', 5) end)
    risky('CheckBlockingTerrain nil', function() return brain:CheckBlockingTerrain(nil, Q, 'low') end)
    WaitTicks(30)
    LOG('PROBE done')
end)
