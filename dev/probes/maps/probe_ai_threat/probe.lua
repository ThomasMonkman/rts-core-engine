-- Differential probe: the AI threat map (moho.aibrain_methods threat calls).
--   real game: dev/probes/run_faf.ps1 probe_ai_threat 150
--   ours: --headless --no-ai --ticks 800 --script dev/probes/maps/probe_ai_threat/probe.lua
-- Army 1 asks; NEUTRAL_CIVILIAN (made its enemy) owns the threat. One enemy
-- unit per site, sites in their own 64-unit cells. Phases:
--   A  no intel on the enemy (army 1 has no units near): t = 1, 5, 10, 20, 40
--   B  a land scout of army 1 by every site (t = 45), sampled at 50, 60, 80
--   C  scouts destroyed at 85: sampled at 90, 120, 160
--   D  enemy units at S1 (tank) and S3 (point defence) destroyed at 165:
--      sampled every 10 ticks to 300
--   E  AssignThreatAtPosition cases from 305, sampled every 10 ticks to 600
-- Lines: PROBE <tick> <tag> ...; numbers at 2 decimals.
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

local function N(v)
    if type(v) ~= 'number' then return tostring(v) end
    if math.abs(v) < 0.005 then v = 0 end
    return fmt('%.2f', v)
end
local function S(v, d)
    d = d or 0
    local t = type(v)
    if t == 'number' then return N(v) end
    if t == 'string' then return '"' .. v .. '"' end
    if t ~= 'table' then return tostring(v) end
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

local function make(bp, army, x, z, y)
    local u = CreateUnitHPR(bp, army, x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
    if u.SetFireState then pcall(u.SetFireState, u, 1) end
    return u
end

local TYPES = { 'Overall', 'OverallNotAssigned', 'StructuresNotMex', 'Structures', 'Naval', 'Air', 'Land',
    'Experimental', 'Commander', 'Artillery', 'AntiAir', 'AntiSurface', 'AntiSub', 'Economy', 'Unknown' }

-- name, blueprint, x, z
local SITES = {
    { 'S1tank', 'uel0201', 608, 80 },
    { 'S2aa', 'uel0104', 912, 96 },
    { 'S3pd', 'ueb2101', 656, 384 },
    { 'S4mex', 'ueb1103', 960, 336 },
    { 'S5acu', 'uel0001', 384, 560 },
    { 'S6arty', 'uel0103', 96, 608 },
    { 'S7exp', 'uel0401', 352, 880 },
    { 'S8air', 'uea0102', 96, 896 },
    { 'S9sub', 'ues0203', 48, 48 },
    { 'S10frig', 'ues0103', 768, 656 },
}
local EMPTY = { 208, 128 } -- nothing here (water)
local OWN = { 784, 144 }   -- army 1's own tank
local ALLY = { 816, 224 }  -- an allied army's tank (army 2)
local units = {}
for i, s in SITES do
    units[i] = make(s[2], ENEMY, s[3], s[4])
    local bp = units[i]:GetBlueprint().Defense
    LOG('PROBE 0 bp ' .. s[1] .. ' ' .. s[2] .. ' air=' .. N(bp.AirThreatLevel) .. ' surf=' .. N(bp.SurfaceThreatLevel)
        .. ' sub=' .. N(bp.SubThreatLevel) .. ' eco=' .. N(bp.EconomyThreatLevel))
end
local own = make('uel0201', TESTER, OWN[1], OWN[2])
SetAlliance(TESTER, armies[2], 'Ally')
SetAlliance(armies[2], TESTER, 'Ally')
local ally = make('uel0201', armies[2], ALLY[1], ALLY[2])

local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function thr(p, rings, ty, army, restriction)
    local ok, v = pcall(brain.GetThreatAtPosition, brain, p, rings, restriction == nil and true or restriction, ty, army)
    if ok then return N(v) end
    return 'ERR'
end

-- Every type at every site, ring 0, all enemies; then the enemy army only.
local function sample_types(phase)
    for i, s in SITES do
        local p = pos(s[3], s[4])
        local line = 'PROBE ' .. T() .. ' types ' .. phase .. ' ' .. s[1]
        for _, ty in TYPES do line = line .. ' ' .. ty .. '=' .. thr(p, 0, ty) end
        LOG(line)
    end
end
local function sample_short(phase)
    local line = 'PROBE ' .. T() .. ' overall ' .. phase
    for i, s in SITES do
        local p = pos(s[3], s[4])
        line = line .. ' ' .. s[1] .. '=' .. thr(p, 0, 'Overall') .. '/' .. thr(p, 0, 'Overall', enemyIndex) .. '/' .. thr(p, 0, 'Overall', nil, false)
    end
    LOG(line)
    line = 'PROBE ' .. T() .. ' own ' .. phase
    local po, pa, pe = pos(OWN[1], OWN[2]), pos(ALLY[1], ALLY[2]), pos(EMPTY[1], EMPTY[2])
    line = line .. ' own:noarmy=' .. thr(po, 0, 'Overall') .. ' own:self=' .. thr(po, 0, 'Overall', 1) .. ' own:enemyidx=' .. thr(po, 0, 'Overall', enemyIndex)
    line = line .. ' ally:noarmy=' .. thr(pa, 0, 'Overall') .. ' ally:idx=' .. thr(pa, 0, 'Overall', 2)
    line = line .. ' empty=' .. thr(pe, 0, 'Overall')
    LOG(line)
end
local function sample_rings(phase)
    for _, i in { 1, 3, 5 } do
        local s = SITES[i]
        local p = pos(s[3], s[4])
        local line = 'PROBE ' .. T() .. ' rings ' .. phase .. ' ' .. s[1]
        for r = 0, 4 do line = line .. ' r' .. r .. '=' .. thr(p, r, 'Overall') end
        line = line .. ' r0.5=' .. thr(p, 0.5, 'Overall') .. ' r1.9=' .. thr(p, 1.9, 'Overall') .. ' r-1=' .. thr(p, -1, 'Overall')
        LOG(line)
    end
end
-- Threat (ring 0) along a line through a site: where the cell edges are and
-- whether a unit's threat spreads to neighbouring cells.
local function scan(phase, i, ty)
    local s = SITES[i]
    local line = 'PROBE ' .. T() .. ' scanx ' .. phase .. ' ' .. s[1] .. ' ' .. ty
    local last
    for dx = -160, 160, 4 do
        local x = s[3] + dx
        if x > 2 and x < 1022 then
            local v = thr({ x, 0, s[4] }, 0, ty)
            if v ~= last then line = line .. ' x' .. x .. '=' .. v; last = v end
        end
    end
    LOG(line)
    line = 'PROBE ' .. T() .. ' scanz ' .. phase .. ' ' .. s[1] .. ' ' .. ty
    last = nil
    for dz = -160, 160, 4 do
        local z = s[4] + dz
        if z > 2 and z < 1022 then
            local v = thr({ s[3], 0, z }, 0, ty)
            if v ~= last then line = line .. ' z' .. z .. '=' .. v; last = v end
        end
    end
    LOG(line)
end
local function others(phase)
    local p1, p3, p5 = pos(SITES[1][3], SITES[1][4]), pos(SITES[3][3], SITES[3][4]), pos(SITES[5][3], SITES[5][4])
    for _, ty in { 'Overall', 'AntiSurface', 'AntiAir', 'Economy', 'Commander', 'Experimental', 'Naval' } do
        for _, ring in { 0, 1, 2 } do
            R('highest ' .. phase .. ' ' .. ty .. ' ring' .. ring, function() return brain:GetHighestThreatPosition(ring, true, ty) end)
        end
        R('highest ' .. phase .. ' ' .. ty .. ' enemyidx', function() return brain:GetHighestThreatPosition(0, true, ty, enemyIndex) end)
    end
    R('highest ' .. phase .. ' self', function() return brain:GetHighestThreatPosition(0, true, 'Overall', 1) end)
    R('highest ' .. phase .. ' ally', function() return brain:GetHighestThreatPosition(0, true, 'Overall', 2) end)
    R('highest ' .. phase .. ' norestrict', function() return brain:GetHighestThreatPosition(0, false, 'Overall') end)
    R('highest ' .. phase .. ' notype', function() return brain:GetHighestThreatPosition(0, true) end)
    R('highest ' .. phase .. ' ring16', function() return brain:GetHighestThreatPosition(16, true, 'Overall') end)
    for _, ring in { 0, 1, 2, 3 } do
        R('around ' .. phase .. ' S3 Overall ring' .. ring, function() return brain:GetThreatsAroundPosition(p3, ring, true, 'Overall') end)
    end
    R('around ' .. phase .. ' S3 AntiSurface ring16', function() return brain:GetThreatsAroundPosition(p3, 16, true, 'AntiSurface') end)
    R('around ' .. phase .. ' S3 Overall ring16 enemyidx', function() return brain:GetThreatsAroundPosition(p3, 16, true, 'Overall', enemyIndex) end)
    R('around ' .. phase .. ' S3 notype', function() return brain:GetThreatsAroundPosition(p3, 1, true) end)
    R('around ' .. phase .. ' empty', function() return brain:GetThreatsAroundPosition(pos(EMPTY[1], EMPTY[2]), 0, true, 'Overall') end)
    R('between ' .. phase .. ' S1-S3', function() return brain:GetThreatBetweenPositions(p1, p3, true, 'Overall') end)
    R('between ' .. phase .. ' S3-S1', function() return brain:GetThreatBetweenPositions(p3, p1, true, 'Overall') end)
    R('between ' .. phase .. ' S1-S1', function() return brain:GetThreatBetweenPositions(p1, p1, true, 'Overall') end)
    R('between ' .. phase .. ' S3-S5', function() return brain:GetThreatBetweenPositions(p3, p5, true, 'Overall') end)
    R('between ' .. phase .. ' S3-S5 AntiSurface', function() return brain:GetThreatBetweenPositions(p3, p5, true, 'AntiSurface') end)
    R('between ' .. phase .. ' S3-S5 enemyidx', function() return brain:GetThreatBetweenPositions(p3, p5, true, 'Overall', enemyIndex) end)
    R('between ' .. phase .. ' S3-near', function() return brain:GetThreatBetweenPositions(p3, { p3[1] + 100, p3[2], p3[3] }, true, 'Overall') end)
    R('at ' .. phase .. ' notype', function() return brain:GetThreatAtPosition(p3, 0, true) end)
    R('at ' .. phase .. ' lowercase', function() return brain:GetThreatAtPosition(p3, 0, true, 'overall') end)
    R('at ' .. phase .. ' enemybrain-own', function() return ebrain:GetThreatAtPosition(p3, 0, true, 'Overall') end)
    R('at ' .. phase .. ' enemybrain-self', function() return ebrain:GetThreatAtPosition(p3, 0, true, 'Overall', enemyIndex) end)
    R('at ' .. phase .. ' enemybrain-tester', function() return ebrain:GetThreatAtPosition(pos(OWN[1], OWN[2]), 0, true, 'Overall') end)
    R('at ' .. phase .. ' offmap', function() return brain:GetThreatAtPosition({ -50, 0, -50 }, 0, true, 'Overall') end)
    R('at ' .. phase .. ' offmap2', function() return brain:GetThreatAtPosition({ 5000, 0, 5000 }, 0, true, 'Overall') end)
    R('at ' .. phase .. ' corner r1', function() return brain:GetThreatAtPosition({ 48, 0, 48 }, 1, true, 'Overall') end)
end

local function wait_to(t)
    while T() < t do WaitTicks(1) end
end

ForkThread(function()
    LOG('PROBE 0 setup armies=' .. table.getn(armies) .. ' enemyIndex=' .. tostring(enemyIndex) .. ' map=' .. S({ GetMapSize() }))
    for _, t in { 1, 5, 10, 20, 40 } do
        wait_to(t)
        sample_short('A')
        if t == 1 or t == 20 or t == 40 then sample_types('A') end
    end
    sample_rings('A')
    scan('A', 1, 'Overall')
    scan('A', 3, 'Overall')
    scan('A', 5, 'Overall')
    scan('A', 7, 'Overall')
    others('A')

    wait_to(45)
    local scouts = {}
    for i, s in SITES do scouts[i] = make('uel0101', TESTER, s[3] + 6, s[4] + 6) end
    for _, t in { 46, 50, 60, 80 } do
        wait_to(t)
        sample_short('B')
        if t == 50 or t == 80 then sample_types('B') end
    end
    sample_rings('B')
    scan('B', 1, 'Overall')
    scan('B', 3, 'Overall')
    scan('B', 3, 'AntiSurface')
    scan('B', 7, 'Overall')
    scan('B', 7, 'AntiSurface')
    scan('B', 7, 'Economy')
    scan('B', 5, 'Commander')
    others('B')
    for i, s in SITES do
        local line = 'PROBE ' .. T() .. ' intel ' .. s[1]
        local ok, blip = pcall(units[i].GetBlip, units[i], 1)
        if ok and blip then
            line = line .. ' los=' .. tostring(blip:IsSeenNow(1)) .. ' radar=' .. tostring(blip:IsOnRadar(1)) .. ' sonar=' .. tostring(blip:IsOnSonar(1))
        else
            line = line .. ' noblip'
        end
        LOG(line)
    end

    wait_to(85)
    for _, u in scouts do u:Destroy() end
    for _, t in { 86, 90, 120, 160 } do
        wait_to(t)
        sample_short('C')
        if t == 160 then sample_types('C') end
    end

    wait_to(165)
    units[1]:Destroy()
    units[3]:Destroy()
    own:Destroy()
    for t = 166, 300, 10 do
        wait_to(t)
        sample_short('D')
    end
    scan('D', 3, 'Overall')

    -- E: assigned threat. Cells of their own (no units).
    wait_to(305)
    local A1, A2, A3, A4, A5, A6, A7 = pos(208, 128), pos(208, 320), pos(448, 320), pos(624, 608), pos(880, 720), pos(560, 848), pos(880, 912)
    R('assign A1 100 decay0.1 Overall', function() return brain:AssignThreatAtPosition(A1, 100, 0.1, 'Overall') end)
    R('assign A2 100 decay0.01 AntiSurface', function() return brain:AssignThreatAtPosition(A2, 100, 0.01, 'AntiSurface') end)
    R('assign A3 100 decay1 AntiAir', function() return brain:AssignThreatAtPosition(A3, 100, 1, 'AntiAir') end)
    R('assign A4 100 decay0 Economy', function() return brain:AssignThreatAtPosition(A4, 100, 0, 'Economy') end)
    R('assign A5 100 decay0.5 notype', function() return brain:AssignThreatAtPosition(A5, 100, 0.5) end)
    R('assign A6 -50 decay0.1 Overall', function() return brain:AssignThreatAtPosition(A6, -50, 0.1, 'Overall') end)
    R('assign A7 100 decay5 Land', function() return brain:AssignThreatAtPosition(A7, 100, 5, 'Land') end)
    R('assign S5 -20 decay0.1 AntiSurface', function() return brain:AssignThreatAtPosition(pos(SITES[5][3], SITES[5][4]), -20, 0.1, 'AntiSurface') end)
    local AS = { { 'A1', A1 }, { 'A2', A2 }, { 'A3', A3 }, { 'A4', A4 }, { 'A5', A5 }, { 'A6', A6 }, { 'A7', A7 }, { 'S5', pos(SITES[5][3], SITES[5][4]) } }
    local ATYPES = { 'Overall', 'OverallNotAssigned', 'AntiSurface', 'AntiAir', 'Economy', 'Land', 'Unknown' }
    local function sample_assigned()
        for _, a in AS do
            local line = 'PROBE ' .. T() .. ' assigned ' .. a[1]
            for _, ty in ATYPES do line = line .. ' ' .. ty .. '=' .. thr(a[2], 0, ty) end
            line = line .. ' enemyidx=' .. thr(a[2], 0, 'Overall', enemyIndex) .. ' self=' .. thr(a[2], 0, 'Overall', 1)
                .. ' r1=' .. thr(a[2], 1, 'Overall') .. ' ebrain=' .. N(ebrain:GetThreatAtPosition(a[2], 0, true, 'Overall'))
            LOG(line)
        end
    end
    sample_assigned()
    for _, t in { 306, 307, 310, 315, 320, 330, 340, 360, 380, 400 } do
        wait_to(t)
        sample_assigned()
    end
    scan('E', 5, 'AntiSurface')
    R('highest E Overall', function() return brain:GetHighestThreatPosition(0, true, 'Overall') end)
    R('highest E Economy', function() return brain:GetHighestThreatPosition(0, true, 'Economy') end)
    R('around E A4 Economy', function() return brain:GetThreatsAroundPosition(A4, 1, true, 'Economy') end)
    -- Assigning again at the same cell: adds or replaces?
    R('assign A4 again 30', function() return brain:AssignThreatAtPosition(A4, 30, 0, 'Economy') end)
    R('assign A1 again 30', function() return brain:AssignThreatAtPosition(A1, 30, 0.1, 'Overall') end)
    sample_assigned()
    for _, t in { 410, 450, 500, 560, 600 } do
        wait_to(t)
        sample_assigned()
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
