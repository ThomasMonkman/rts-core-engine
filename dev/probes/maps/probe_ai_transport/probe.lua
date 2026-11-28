-- Differential probe: a platoon's transport calls on a live platoon
-- (LoadUnits, UnloadUnitsAtLocation, UnloadAllAtLocation,
-- MoveToLocation(useTransports), IsFerrying, GetFerryBeacons, UseFerryBeacon)
-- and AssignUnitsToPlatoon with an enemy unit.
--   real game: run_faf.ps1 probe_ai_transport 200
--   ours: --headless --no-ai --ticks 1300 --script dev/probes/maps/probe_ai_transport/probe.lua
-- Lines: PROBE <tick> <case> ok n=<results> <values> | ERR <message>;
-- PROBE <tick> st <phase> <unit>=x,z[/att]/q<queue> ... flags.
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
        return nil
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
-- ferry: a second transport and two tanks in their own platoon
local f1 = make('f1', 'uel0201', TESTER, QX - 20, QZ + 20)
local f2 = make('f2', 'uel0201', TESTER, QX - 17, QZ + 20)
local tr2 = make('tr2', 'uea0107', TESTER, QX - 30, QZ + 10)
local em1 = make('em1', 'uel0201', ENEMY, 800, 336)

local function cmdActive(p, cmd)
    if cmd == nil then return 'nil' end
    local ok, v = pcall(p.IsCommandsActive, p, cmd)
    return ok and tostring(v) or 'ERR'
end
local function flag(p, fn, squad)
    local ok, v = pcall(p[fn], p, squad)
    return ok and tostring(v) or 'ERR'
end
local function st(phase, p, cmd, units, trans)
    local line = 'PROBE ' .. T() .. ' st ' .. phase
    for _, u in units do
        if u:BeenDestroyed() then
            line = line .. ' ' .. tags[u] .. '=gone'
        else
            local x, y, z = u:GetPositionXYZ()
            local ok, q = pcall(function() return table.getn(u:GetCommandQueue()) end)
            line = line .. ' ' .. tags[u] .. '=' .. N(x) .. ',' .. N(z) .. (u:IsUnitState('Attached') and '/att' or '') .. '/q' .. (ok and q or 'E')
        end
    end
    line = line .. ' active=' .. cmdActive(p, cmd)
    line = line .. ' ferryA=' .. flag(p, 'IsFerrying', 'Attack') .. ' ferryS=' .. flag(p, 'IsFerrying', 'Scout') .. ' ferry=' .. flag(p, 'IsFerrying')
    line = line .. ' movA=' .. flag(p, 'IsMoving', 'Attack') .. ' movS=' .. flag(p, 'IsMoving', 'Scout')
    local ok, v = pcall(function() return table.getn(trans:GetCargo()) end)
    line = line .. ' cargo=' .. (ok and tostring(v) or 'ERR')
    LOG(line)
end
local function watch(phase, p, cmd, units, trans, n, every)
    for i = 0, n - 1, every do
        st(phase .. '+' .. i, p, cmd, units, trans)
        WaitTicks(every)
    end
end
local function allAttached(list)
    for _, u in list do
        if u:BeenDestroyed() or not u:IsUnitState('Attached') then return false end
    end
    return true
end

ForkThread(function()
    LOG('PROBE 0 setup')
    wait_to(3)
    local p = brain:MakePlatoon('ProbeT', '')
    tags[p] = 'P'
    brain:AssignUnitsToPlatoon(p, { t1, t2 }, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(p, { a1 }, 'Artillery', 'None')
    brain:AssignUnitsToPlatoon(p, { tr }, 'Scout', 'None')
    local U = { t1, t2, a1, tr }
    -- DisbandOnIdle on its own throw-away platoon (the real game disbands an idle one at once)
    local pz = brain:MakePlatoon('ProbeZ', '')
    R('throwaway DisbandOnIdle', function() return pz:DisbandOnIdle() end)
    R('throwaway exists', function() return brain:PlatoonExists(pz) end)
    R('P exists', function() return brain:PlatoonExists(p) end)
    R('GetFerryBeacons none', function() return p:GetFerryBeacons() end)
    wait_to(5)

    -- 1. LoadUnits
    local cmd = R('LoadUnits LAND', function() return p:LoadUnits(C.LAND) end)
    LOG('PROBE ' .. T() .. ' LoadUnits type ' .. type(cmd))
    watch('load', p, cmd, U, tr, 120, 10)
    if not allAttached({ t1, t2, a1 }) then
        LOG('PROBE ' .. T() .. ' fallback IssueTransportLoad')
        IssueClearCommands({ t1, t2, a1, tr })
        IssueTransportLoad({ t1, t2, a1 }, tr)
        for i = 1, 150 do
            if allAttached({ t1, t2, a1 }) then break end
            WaitTicks(1)
        end
        st('loaded', p, nil, U, tr)
    end

    -- 2. UnloadUnitsAtLocation (FAF's doc: carriers only)
    cmd = R('UnloadUnitsAtLocation LAND', function() return p:UnloadUnitsAtLocation(C.LAND, pos(QX + 30, QZ + 30)) end)
    LOG('PROBE ' .. T() .. ' UnloadUnitsAtLocation type ' .. type(cmd))
    watch('unloadspec', p, cmd, U, tr, 60, 10)

    -- 3. UnloadAllAtLocation
    R('Stop before unload', function() return p:Stop() end)
    cmd = R('UnloadAllAtLocation', function() return p:UnloadAllAtLocation(pos(QX + 40, QZ + 20)) end)
    LOG('PROBE ' .. T() .. ' UnloadAllAtLocation type ' .. type(cmd))
    watch('unload', p, cmd, U, tr, 200, 10)
    R('P units after unload', function() return p:GetPlatoonUnits() end)

    -- 4. MoveToLocation with transports
    R('Stop before movetr', function() return p:Stop() end)
    cmd = R('MoveToLocation useTransports', function() return p:MoveToLocation(pos(QX, QZ + 60), true) end)
    LOG('PROBE ' .. T() .. ' MoveToLocation type ' .. type(cmd))
    watch('movetr', p, cmd, U, tr, 160, 10)
    R('Stop after movetr', function() return p:Stop() end)

    -- 5. Ferry beacon: tr2 gets a ferry route A -> B, the tanks' platoon uses its beacon
    local p3 = brain:MakePlatoon('ProbeF', '')
    tags[p3] = 'P3'
    brain:AssignUnitsToPlatoon(p3, { f1, f2 }, 'Attack', 'None')
    local FA, FB = pos(QX - 20, QZ + 34), pos(QX - 20, QZ + 100)
    IssueFerry({ tr2 }, FA)
    IssueFerry({ tr2 }, FB)
    WaitTicks(3)
    local beacon = R('tr2 GetTransportFerryBeacon', function() return tr2:GetTransportFerryBeacon() end)
    if beacon then
        tags[beacon] = 'beacon'
        R('beacon id pos', function() local x, y, z = beacon:GetPositionXYZ() return beacon:GetUnitId(), N(x) .. ',' .. N(z), beacon:GetArmy() end)
    end
    R('tr2 queue', function() return table.getn(tr2:GetCommandQueue()) end)
    R('P3 GetFerryBeacons', function() return p3:GetFerryBeacons() end)
    if beacon then
        LOG('PROBE ' .. T() .. ' next UseFerryBeacon')
        WaitTicks(1)
        cmd = R('UseFerryBeacon LAND', function() return p3:UseFerryBeacon(C.LAND, beacon) end)
        LOG('PROBE ' .. T() .. ' UseFerryBeacon type ' .. type(cmd))
        watch('ferry', p3, cmd, { f1, f2, tr2 }, tr2, 320, 10)
    end

    -- 6. A live platoon given a bogus squad and an enemy unit
    LOG('PROBE ' .. T() .. ' next live bogus squad')
    WaitTicks(1)
    R('live GetSquadUnits Bogus', function() return p:GetSquadUnits('Bogus') end)
    LOG('PROBE ' .. T() .. ' next live enemy assign')
    WaitTicks(1)
    R('live AssignUnitsToPlatoon enemy unit', function() return brain:AssignUnitsToPlatoon(p, { em1 }, 'Attack', 'None') end)
    R('live P units after enemy assign', function() return p:GetPlatoonUnits() end)
    R('em1 army', function() return em1:GetArmy() end)
    R('enemy platoon of em1', function() return em1.PlatoonHandle == p end)
    WaitTicks(30)
    LOG('PROBE done')
end)
