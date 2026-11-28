-- Differential probe: platoons (moho.platoon_methods and the brain's platoon
-- calls): the ArmyPool, squads, moving units between platoons, threat and
-- counts, target finders, orders and their state flags, FormPlatoon, and
-- when an emptied platoon stops existing.
--   real game: dev/probes/run_faf.ps1 probe_ai_platoon 90
--   ours: --headless --no-ai --ticks 500 --script dev/probes/maps/probe_ai_platoon/probe.lua
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
local t = {}
for i = 1, 4 do t[i] = make('t' .. i, 'uel0201', TESTER, QX - 6 + i * 3, QZ) end
local a1 = make('a1', 'uel0103', TESTER, QX - 3, QZ + 4)
local a2 = make('a2', 'uel0103', TESTER, QX + 3, QZ + 4)
local sc = make('sc', 'uel0101', TESTER, QX, QZ + 8)
local eng = make('eng', 'uel0105', TESTER, QX + 4, QZ + 8)
local aa = make('aa', 'uel0104', TESTER, QX - 4, QZ + 8)
local x1 = make('x1', 'uel0201', TESTER, QX - 8, QZ + 8)
local x2 = make('x2', 'uel0201', TESTER, QX + 8, QZ + 8)
-- pool units for FormPlatoon, at a second site
local FX, FZ = 800, 336
local F = pos(FX, FZ)
local f = {}
for i = 1, 5 do f[i] = make('f' .. i, 'uel0201', TESTER, FX + i * 4, FZ) end
local fa1 = make('fa1', 'uel0103', TESTER, FX + 2, FZ + 6)
local fa2 = make('fa2', 'uel0103', TESTER, FX + 12, FZ + 6)
local ffar = make('ffar', 'uel0201', TESTER, FX + 60, FZ)
-- enemies near Q
local e1 = make('e1', 'uel0201', ENEMY, QX + 16, QZ)
local e2 = make('e2', 'uel0201', ENEMY, QX + 22, QZ + 6)
local epg = make('epg', 'ueb1101', ENEMY, QX + 30, QZ - 6)
local eair = make('eair', 'uea0102', ENEMY, QX + 12, QZ - 14)
local eacu = make('eacu', 'uel0001', ENEMY, QX + 40, QZ + 10)
local efar = make('efar', 'uel0303', ENEMY, QX + 120, QZ)
SetAlliance(TESTER, armies[2], 'Ally')
SetAlliance(armies[2], TESTER, 'Ally')
local al = make('al', 'uel0201', armies[2], QX, QZ - 12)
for _, u in { e1, e2, epg, eacu, efar } do u:SetMaxHealth(50000); u:SetHealth(u, 50000) end

local SQUADS = { 'Attack', 'Artillery', 'Guard', 'Scout', 'Support', 'Unassigned' }
local function dump(tag, p)
    R(tag .. ' units', function() return p:GetPlatoonUnits() end)
    for _, s in SQUADS do
        R(tag .. ' squad ' .. s, function() return p:GetSquadUnits(s) end)
    end
end
local function state(tag, p, cmd)
    local line = 'PROBE ' .. T() .. ' state ' .. tag
    for _, fn in { 'IsMoving', 'IsAttacking', 'IsPatrolling', 'IsFerrying' } do
        local ok, v = pcall(p[fn], p, 'Attack')
        line = line .. ' ' .. fn .. '=' .. (ok and tostring(v) or 'ERR')
    end
    if cmd ~= nil then
        local ok, v = pcall(p.IsCommandsActive, p, cmd)
        line = line .. ' active=' .. (ok and tostring(v) or 'ERR')
    end
    local ok, pp = pcall(p.GetPlatoonPosition, p)
    line = line .. ' pos=' .. (ok and S(pp) or 'ERR')
    LOG(line)
end
local function unitpos(tag, list)
    local line = 'PROBE ' .. T() .. ' unitpos ' .. tag
    for _, u in list do
        if not u.Dead and not u:BeenDestroyed() then
            local x, y, z = u:GetPositionXYZ()
            line = line .. ' ' .. tags[u] .. '=' .. N(x) .. ',' .. N(z) .. '/q' .. table.getn(u:GetCommandQueue())
        end
    end
    LOG(line)
end
local function hook(p, name)
    tags[p] = name
    local od, oa = p.OnDestroy, p.OnUnitsAddedToPlatoon
    p.OnDestroy = function(self) LOG('PROBE ' .. T() .. ' OnDestroy ' .. name); if od then od(self) end end
    p.OnUnitsAddedToPlatoon = function(self) LOG('PROBE ' .. T() .. ' OnUnitsAddedToPlatoon ' .. name); if oa then oa(self) end end
end

ForkThread(function()
    LOG('PROBE 0 setup')
    wait_to(3)
    local pool = R('GetPlatoonUniquelyNamed ArmyPool', function() return brain:GetPlatoonUniquelyNamed('ArmyPool') end)
    if pool then tags[pool] = 'pool' end
    R('pool type', function() return type(pool), pool == brain:GetPlatoonUniquelyNamed('ArmyPool') end)
    R('pool units', function() return pool:GetPlatoonUnits() end)
    R('pool squad Unassigned', function() return pool:GetSquadUnits('Unassigned') end)
    R('pool name', function() return pool:GetPlatoonUniqueName() end)
    R('pool plan', function() return pool:GetAIPlan() end)
    R('pool pos', function() return pool:GetPlatoonPosition() end)
    R('GetPlatoonsList initial', function() return brain:GetPlatoonsList() end)
    R('GetPlatoonUniquelyNamed missing', function() return brain:GetPlatoonUniquelyNamed('NoSuchPlatoon') end)

    local p1 = R('MakePlatoon P1', function() return brain:MakePlatoon('ProbeP1', '') end)
    hook(p1, 'P1')
    R('P1 exists', function() return brain:PlatoonExists(p1) end)
    R('GetPlatoonsList after make', function() return brain:GetPlatoonsList() end)
    R('P1 empty units', function() return p1:GetPlatoonUnits() end)
    R('P1 empty pos', function() return p1:GetPlatoonPosition() end)
    R('P1 empty squad Attack', function() return p1:GetSquadUnits('Attack') end)
    R('P1 empty squadpos Attack', function() return p1:GetSquadPosition('Attack') end)
    R('P1 name', function() return p1:GetPlatoonUniqueName() end)
    R('P1 plan', function() return p1:GetAIPlan() end)
    R('P1 brain', function() return p1:GetBrain() end)
    R('P1 faction', function() return p1:GetFactionIndex() end)
    R('P1 personality', function() return type(p1:GetPersonality()) end)
    R('P1 opponentAI', function() return p1:IsOpponentAIRunning() end)
    R('P1 lifetime', function() return p1:GetPlatoonLifetimeStats() end)
    R('P1 fields', function() return p1.PlanName, p1.BuilderName, p1.Trash ~= nil end)
    R('P1 empty CalculatePlatoonThreat', function() return p1:CalculatePlatoonThreat('Overall', C.ALLUNITS) end)
    R('P1 empty PlatoonCategoryCount', function() return p1:PlatoonCategoryCount(C.ALLUNITS) end)
    R('P1 empty IsMoving', function() return p1:IsMoving('Attack') end)
    R('P1 empty MoveToLocation', function() return type(p1:MoveToLocation(pos(QX, QZ + 20), false)) end)

    R('assign attack', function() return brain:AssignUnitsToPlatoon(p1, { t[1], t[2], t[3], t[4] }, 'Attack', 'GrowthFormation') end)
    R('assign artillery', function() return brain:AssignUnitsToPlatoon(p1, { a1, a2 }, 'Artillery', 'GrowthFormation') end)
    R('assign scout', function() return brain:AssignUnitsToPlatoon(p1, { sc }, 'Scout', 'AttackFormation') end)
    R('assign support', function() return brain:AssignUnitsToPlatoon(p1, { eng }, 'Support', 'None') end)
    R('assign guard', function() return brain:AssignUnitsToPlatoon(p1, { aa }, 'Guard', 'NoFormation') end)
    R('assign unassigned', function() return brain:AssignUnitsToPlatoon(p1, { x1 }, 'Unassigned', 'BogusFormation') end)
    R('assign lowercase squad', function() return brain:AssignUnitsToPlatoon(p1, { x2 }, 'attack', 'GrowthFormation') end)
    dump('P1', p1)
    R('pool units after assign', function() return pool:GetPlatoonUnits() end)
    R('P1 pos', function() return p1:GetPlatoonPosition() end)
    for _, s in SQUADS do R('P1 squadpos ' .. s, function() return p1:GetSquadPosition(s) end) end
    R('P1 squad bogus', function() return p1:GetSquadUnits('Bogus') end)
    R('unit PlatoonHandle', function() return t[1].PlatoonHandle, x1.PlatoonHandle end)
    R('assign empty list', function() return brain:AssignUnitsToPlatoon(p1, {}, 'Attack', 'GrowthFormation') end)
    R('assign same again other squad', function() return brain:AssignUnitsToPlatoon(p1, { t[4] }, 'Artillery', 'GrowthFormation') end)
    dump('P1b', p1)
    R('assign back', function() return brain:AssignUnitsToPlatoon(p1, { t[4] }, 'Attack', 'GrowthFormation') end)
    R('P1 units after back', function() return p1:GetPlatoonUnits() end)

    -- counts and threat
    for _, ty in { 'Overall', 'AntiSurface', 'AntiAir', 'AntiSub', 'Economy', 'Land', 'Artillery', 'Air', 'Structures', 'Unknown' } do
        R('P1 threat ' .. ty, function() return p1:CalculatePlatoonThreat(ty, C.ALLUNITS) end)
    end
    R('P1 threat Overall INDIRECTFIRE', function() return p1:CalculatePlatoonThreat('Overall', C.INDIRECTFIRE) end)
    R('P1 threat Surface (FAF name)', function() return p1:CalculatePlatoonThreat('Surface', C.ALLUNITS) end)
    R('P1 threat Air (FAF name) ANTIAIR', function() return p1:CalculatePlatoonThreat('Air', C.ANTIAIR) end)
    R('P1 threat notype', function() return p1:CalculatePlatoonThreat(nil, C.ALLUNITS) end)
    R('P1 threataround Overall r5', function() return p1:CalculatePlatoonThreatAroundPosition('Overall', C.ALLUNITS, Q, 5) end)
    R('P1 threataround Overall r4', function() return p1:CalculatePlatoonThreatAroundPosition('Overall', C.ALLUNITS, Q, 4) end)
    R('P1 threataround Overall r3', function() return p1:CalculatePlatoonThreatAroundPosition('Overall', C.ALLUNITS, Q, 3) end)
    R('P1 threataround AntiSurface r50', function() return p1:CalculatePlatoonThreatAroundPosition('AntiSurface', C.ALLUNITS, Q, 50) end)
    R('P1 threataround Surface r50', function() return p1:CalculatePlatoonThreatAroundPosition('Surface', C.ALLUNITS, Q, 50) end)
    R('P1 count ALL', function() return p1:PlatoonCategoryCount(C.ALLUNITS) end)
    R('P1 count DIRECTFIRE', function() return p1:PlatoonCategoryCount(C.DIRECTFIRE) end)
    R('P1 count NAVAL', function() return p1:PlatoonCategoryCount(C.NAVAL) end)
    R('P1 countaround ALL r5', function() return p1:PlatoonCategoryCountAroundPosition(C.ALLUNITS, Q, 5) end)
    R('P1 countaround ALL r4', function() return p1:PlatoonCategoryCountAroundPosition(C.ALLUNITS, Q, 4) end)
    R('P1 countaround ALL r3', function() return p1:PlatoonCategoryCountAroundPosition(C.ALLUNITS, Q, 3) end)
    R('P1 countaround ALL r50', function() return p1:PlatoonCategoryCountAroundPosition(C.ALLUNITS, Q, 50) end)

    -- targets (the platoon sees e1, e2, epg, eair, eacu at 16..42; efar at 120 is out of sight)
    wait_to(8)
    for _, s in { 'Attack', 'Artillery', 'Guard', 'Scout', 'Support' } do
        R('canattack ' .. s .. ' e1', function() return p1:CanAttackTarget(s, e1) end)
        R('canattack ' .. s .. ' eair', function() return p1:CanAttackTarget(s, eair) end)
    end
    R('canattack Attack ally', function() return p1:CanAttackTarget('Attack', al) end)
    R('canattack Attack own', function() return p1:CanAttackTarget('Attack', a1) end)
    R('canattack Attack efar', function() return p1:CanAttackTarget('Attack', efar) end)
    R('canattack empty squad', function() return p1:CanAttackTarget('Unassigned', e1) end)
    for _, fn in { 'FindClosestUnit', 'FindFurthestUnit', 'FindHighestValueUnit', 'FindClosestUnitToBase' } do
        R(fn .. ' Attack Enemy true ALL', function() return p1[fn](p1, 'Attack', 'Enemy', true, C.ALLUNITS) end)
        R(fn .. ' Attack Enemy false ALL', function() return p1[fn](p1, 'Attack', 'Enemy', false, C.ALLUNITS) end)
        R(fn .. ' Attack Enemy true STRUCTURE', function() return p1[fn](p1, 'Attack', 'Enemy', true, C.STRUCTURE) end)
        R(fn .. ' Attack Enemy true AIR', function() return p1[fn](p1, 'Attack', 'Enemy', true, C.AIR) end)
        R(fn .. ' Attack Enemy false AIR', function() return p1[fn](p1, 'Attack', 'Enemy', false, C.AIR) end)
        R(fn .. ' Guard Enemy true ALL', function() return p1[fn](p1, 'Guard', 'Enemy', true, C.ALLUNITS) end)
        R(fn .. ' Attack Enemy true TECH3', function() return p1[fn](p1, 'Attack', 'Enemy', true, C.TECH3) end)
        R(fn .. ' Attack Ally false ALL', function() return p1[fn](p1, 'Attack', 'Ally', false, C.ALLUNITS) end)
        R(fn .. ' Attack Enemy true NAVAL', function() return p1[fn](p1, 'Attack', 'Enemy', true, C.NAVAL) end)
    end
    R('FindPrioritizedUnit no list', function() return p1:FindPrioritizedUnit('Attack', 'Enemy', true, Q, 60) end)
    R('SetPrioritizedTargetList', function() return p1:SetPrioritizedTargetList('Attack', { C.STRUCTURE, C.COMMAND, C.ALLUNITS }) end)
    R('FindPrioritizedUnit r60', function() return p1:FindPrioritizedUnit('Attack', 'Enemy', true, Q, 60) end)
    R('FindPrioritizedUnit r25', function() return p1:FindPrioritizedUnit('Attack', 'Enemy', true, Q, 25) end)
    R('FindPrioritizedUnit r5', function() return p1:FindPrioritizedUnit('Attack', 'Enemy', true, Q, 5) end)
    R('SetPrioritizedTargetList 2', function() return p1:SetPrioritizedTargetList('Attack', { C.AIR, C.COMMAND }) end)
    R('FindPrioritizedUnit AIR first canattack', function() return p1:FindPrioritizedUnit('Attack', 'Enemy', true, Q, 60) end)
    R('FindPrioritizedUnit AIR first nocanattack', function() return p1:FindPrioritizedUnit('Attack', 'Enemy', false, Q, 60) end)

    -- naming, plans
    R('UniquelyNamePlatoon', function() return p1:UniquelyNamePlatoon('Uniq1') end)
    R('GetPlatoonUniquelyNamed Uniq1', function() return brain:GetPlatoonUniquelyNamed('Uniq1') end)
    R('P1 name', function() return p1:GetPlatoonUniqueName() end)
    local p3 = R('MakePlatoon P3 plan', function() return brain:MakePlatoon('ProbeP3', 'ProbePlanMissing') end)
    if p3 then hook(p3, 'P3') end
    R('P3 plan', function() return p3:GetAIPlan(), p3.PlanName end)
    R('P3 name collision', function() return p3:UniquelyNamePlatoon('Uniq1') end)
    R('GetPlatoonUniquelyNamed Uniq1 after collision', function() return brain:GetPlatoonUniquelyNamed('Uniq1') end)
    R('P3 SwitchAIPlan', function() return p3:SwitchAIPlan('OtherPlan') end)
    R('P3 plan after switch', function() return p3:GetAIPlan(), p3.PlanName end)
    R('GetNumPlatoonsTemplateNamed ProbeP1', function() return brain:GetNumPlatoonsTemplateNamed('ProbeP1') end)
    R('GetNumPlatoonsTemplateNamed ProbeP3', function() return brain:GetNumPlatoonsTemplateNamed('ProbeP3') end)
    R('GetNumPlatoonsTemplateNamed none', function() return brain:GetNumPlatoonsTemplateNamed('Nope') end)
    R('GetNumPlatoonsWithAI OtherPlan', function() return brain:GetNumPlatoonsWithAI('OtherPlan') end)
    R('GetNumPlatoonsWithAI ProbePlanMissing', function() return brain:GetNumPlatoonsWithAI('ProbePlanMissing') end)
    R('GetNumPlatoonsWithAI empty', function() return brain:GetNumPlatoonsWithAI('') end)
    R('GetPlatoonsList 3', function() return brain:GetPlatoonsList() end)
    R('P3 exists (empty) now', function() return brain:PlatoonExists(p3) end)
    WaitTicks(2)
    R('P3 exists (empty) +2', function() return brain:PlatoonExists(p3) end)

    -- moving units between platoons
    local p2 = R('MakePlatoon P2', function() return brain:MakePlatoon('ProbeP2', 'None') end)
    hook(p2, 'P2')
    R('P2 plan', function() return p2:GetAIPlan(), p2.PlanName end)
    R('move t4 to P2', function() return brain:AssignUnitsToPlatoon(p2, { t[4] }, 'Attack', 'AttackFormation') end)
    R('P1 units', function() return p1:GetPlatoonUnits() end)
    R('P2 units', function() return p2:GetPlatoonUnits() end)
    R('t4 to pool by name', function() return brain:AssignUnitsToPlatoon('ArmyPool', { t[4] }, 'Unassigned', 'None') end)
    R('P2 units emptied', function() return p2:GetPlatoonUnits() end)
    R('P2 exists emptied now', function() return brain:PlatoonExists(p2) end)
    R('pool units', function() return pool:GetPlatoonUnits() end)
    WaitTicks(1)
    R('P2 exists emptied +1', function() return brain:PlatoonExists(p2) end)
    WaitTicks(5)
    R('P2 exists emptied +6', function() return brain:PlatoonExists(p2) end)
    R('GetPlatoonsList 4', function() return brain:GetPlatoonsList() end)
    R('t4 back to P1 by name', function() return brain:AssignUnitsToPlatoon('Uniq1', { t[4] }, 'Attack', 'GrowthFormation') end)
    R('P1 units', function() return p1:GetPlatoonUnits() end)

    -- orders: each observed at +1, +3, +10, then stopped
    wait_to(30)
    local all = { t[1], t[2], t[3], t[4], x2, a1, a2, sc, eng, aa, x1 }
    local function order(tag, f, n)
        LOG('PROBE ' .. T() .. ' order ' .. tag)
        local ok, cmd = pcall(f)
        LOG('PROBE ' .. T() .. ' order ' .. tag .. ' -> ' .. (ok and type(cmd) or ('ERR ' .. tostring(cmd))))
        if not ok then cmd = nil end
        state(tag .. ' +0', p1, cmd)
        unitpos(tag .. ' +0', all)
        WaitTicks(1)
        state(tag .. ' +1', p1, cmd)
        WaitTicks(2)
        state(tag .. ' +3', p1, cmd)
        WaitTicks(n or 17)
        state(tag .. ' +end', p1, cmd)
        unitpos(tag .. ' +end', all)
        R(tag .. ' Stop', function() return p1:Stop() end)
        state(tag .. ' stopped+0', p1, cmd)
        WaitTicks(3)
        state(tag .. ' stopped+3', p1, cmd)
        return cmd
    end
    order('MoveToLocation noformation', function() return p1:MoveToLocation(pos(QX, QZ + 20), false) end, 47)
    order('MoveToLocation squad Artillery', function() return p1:MoveToLocation(pos(QX - 10, QZ + 30), false, 'Artillery') end)
    R('SetPlatoonFormationOverride AttackFormation', function() return p1:SetPlatoonFormationOverride('AttackFormation') end)
    order('MoveToLocation AttackFormation', function() return p1:MoveToLocation(pos(QX, QZ), false) end, 47)
    R('SetPlatoonFormationOverride NoFormation', function() return p1:SetPlatoonFormationOverride('NoFormation') end)
    order('AggressiveMoveToLocation', function() return p1:AggressiveMoveToLocation(pos(QX, QZ + 20)) end)
    order('AttackTarget e1', function() return p1:AttackTarget(e1) end)
    order('Patrol', function() p1:Patrol(pos(QX - 10, QZ + 20)); return p1:Patrol(pos(QX + 10, QZ + 20)) end)
    order('GuardTarget a1', function() return p1:GuardTarget(a1) end)
    order('MoveToTarget e1', function() return p1:MoveToTarget(e1, false) end)
    order('MoveToLocation transports=true', function() return p1:MoveToLocation(pos(QX, QZ + 20), true) end)
    R('Stop squad', function() return p1:Stop('Attack') end)
    R('MoveToLocation offmap', function() return type(p1:MoveToLocation({ -50, 0, -50 }, false)) end)
    R('Stop', function() return p1:Stop() end)
    R('IsCommandsActive nil', function() return p1:IsCommandsActive(nil) end)

    -- FormPlatoon from the pool at the second site
    local T1 = { 'ProbeTemplate', 'None', { 'uel0201', 2, 3, 'Attack', 'GrowthFormation' }, { 'uel0103', 1, 1, 'Artillery', 'AttackFormation' } }
    local T2 = { 'ProbeTemplate2', '', { 'uel0201', 9, 9, 'Attack', 'GrowthFormation' } }
    local T3 = { 'ProbeTemplate3', '', { 'uel0201', 1, 2, 'Attack', 'None' } }
    R('pool units before form', function() return pool:GetPlatoonUnits() end)
    R('CanFormPlatoon T1 r30', function() return pool:CanFormPlatoon(T1, 1, F, 30) end)
    R('CanFormPlatoon T1 r1', function() return pool:CanFormPlatoon(T1, 1, F, 1) end)
    R('CanFormPlatoon T2 r30', function() return pool:CanFormPlatoon(T2, 1, F, 30) end)
    R('CanFormPlatoon T3 x2 r30', function() return pool:CanFormPlatoon(T3, 2, F, 30) end)
    R('CanFormPlatoon T3 x4 r30', function() return pool:CanFormPlatoon(T3, 4, F, 30) end)
    R('CanFormPlatoon T1 r1000', function() return pool:CanFormPlatoon(T1, 1, F, 1000) end)
    R('P1 CanFormPlatoon T1 (non-pool)', function() return p1:CanFormPlatoon(T1, 1, F, 30) end)
    R('CanConsiderFormingPlatoon exists', function() return type(pool.CanConsiderFormingPlatoon) end)
    local fp = R('FormPlatoon T1 r30', function() return pool:FormPlatoon(T1, 1, F, 30) end)
    if type(fp) == 'table' then
        hook(fp, 'FP1')
        dump('FP1', fp)
        R('FP1 plan/name', function() return fp:GetAIPlan(), fp:GetPlatoonUniqueName(), fp.PlanName end)
        R('GetNumPlatoonsTemplateNamed ProbeTemplate', function() return brain:GetNumPlatoonsTemplateNamed('ProbeTemplate') end)
    end
    local fp2 = R('FormPlatoon T2 (too few)', function() return pool:FormPlatoon(T2, 1, F, 30) end)
    R('FormPlatoon T2 exists', function() return fp2 and brain:PlatoonExists(fp2) end)
    local fp3 = R('FormPlatoon T3 x2 r1000', function() return pool:FormPlatoon(T3, 2, F, 1000) end)
    if type(fp3) == 'table' then
        hook(fp3, 'FP3')
        dump('FP3', fp3)
    end
    R('pool units after form', function() return pool:GetPlatoonUnits() end)
    R('GetPlatoonsList 5', function() return brain:GetPlatoonsList() end)

    -- disband, death
    R('DisbandPlatoon FP1', function() return brain:DisbandPlatoon(fp) end)
    R('FP1 exists after disband', function() return brain:PlatoonExists(fp) end)
    R('pool units after disband', function() return pool:GetPlatoonUnits() end)
    R('FP1 units after disband', function() return fp:GetPlatoonUnits() end)
    WaitTicks(2)
    R('FP1 exists after disband +2', function() return brain:PlatoonExists(fp) end)
    R('DisbandPlatoonUniquelyNamed Uniq1', function() return brain:DisbandPlatoonUniquelyNamed('Uniq1') end)
    R('P1 exists after', function() return brain:PlatoonExists(p1) end)
    R('GetPlatoonUniquelyNamed Uniq1 after disband', function() return brain:GetPlatoonUniquelyNamed('Uniq1') end)
    local pk = R('MakePlatoon PK', function() return brain:MakePlatoon('ProbePK', '') end)
    hook(pk, 'PK')
    brain:AssignUnitsToPlatoon(pk, { t[1], t[2] }, 'Attack', 'None')
    LOG('PROBE ' .. T() .. ' kill t1')
    t[1]:Kill()
    R('PK units after kill 1 (same tick)', function() return pk:GetPlatoonUnits() end)
    WaitTicks(1)
    R('PK units after kill 1 (+1)', function() return pk:GetPlatoonUnits() end)
    R('PK exists', function() return brain:PlatoonExists(pk) end)
    LOG('PROBE ' .. T() .. ' kill t2')
    t[2]:Kill()
    R('PK exists (same tick)', function() return brain:PlatoonExists(pk) end)
    R('PK units (same tick)', function() return pk:GetPlatoonUnits() end)
    WaitTicks(1)
    R('PK exists (+1)', function() return brain:PlatoonExists(pk) end)
    WaitTicks(1)
    R('PK exists (+2)', function() return brain:PlatoonExists(pk) end)
    WaitTicks(10)
    R('PK exists (+12)', function() return brain:PlatoonExists(pk) end)
    R('PK units (+12)', function() return pk:GetPlatoonUnits() end)
    local pd = R('MakePlatoon PD', function() return brain:MakePlatoon('ProbePD', '') end)
    hook(pd, 'PD')
    brain:AssignUnitsToPlatoon(pd, { t[3] }, 'Attack', 'None')
    LOG('PROBE ' .. T() .. ' destroy t3')
    t[3]:Destroy()
    R('PD exists (same tick)', function() return brain:PlatoonExists(pd) end)
    WaitTicks(2)
    R('PD exists (+2)', function() return brain:PlatoonExists(pd) end)
    R('GetPlatoonsList end', function() return brain:GetPlatoonsList() end)
    -- a new unit joins the pool?
    local nu = make('nu', 'uel0201', TESTER, QX - 20, QZ - 20)
    R('new unit in pool (same tick)', function() return pool:PlatoonCategoryCount(C.ALLUNITS), nu.PlatoonHandle end)
    WaitTicks(2)
    R('pool units end', function() return pool:GetPlatoonUnits() end)
    WaitTicks(30)
    LOG('PROBE done')
end)
