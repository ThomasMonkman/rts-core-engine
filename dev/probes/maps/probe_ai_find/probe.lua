-- Differential probe: the platoon target finders (FindClosestUnit,
-- FindFurthestUnit, FindHighestValueUnit, FindClosestUnitToBase,
-- FindPrioritizedUnit) asked repeatedly as intel builds up, and per-type
-- threat queries without an army index (no explicit trailing nils: the real
-- game rejects them), ring sums 0..4, cell edges, own / ally cells, and the
-- decay start after sight is lost.
--   real game: run_faf.ps1 probe_ai_find 150
--   ours: --headless --no-ai --ticks 800 --script dev/probes/maps/probe_ai_find/probe.lua
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
-- the platoon
local t = {}
for i = 1, 4 do t[i] = make('t' .. i, 'uel0201', TESTER, QX - 6 + i * 3, QZ) end
local a1 = make('a1', 'uel0103', TESTER, QX - 3, QZ + 4)
local aa = make('aa', 'uel0104', TESTER, QX - 4, QZ + 8)
-- enemies near Q (in sight), and one out of sight
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

-- threat sites (as probe_ai_threat), each with an army-1 scout beside it
local TYPES = { 'Overall', 'OverallNotAssigned', 'StructuresNotMex', 'Structures', 'Naval', 'Air', 'Land',
    'Experimental', 'Commander', 'Artillery', 'AntiAir', 'AntiSurface', 'AntiSub', 'Economy', 'Unknown' }
local SITES = {
    { 'S3pd', 'ueb2101', 656, 384 },
    { 'S4mex', 'ueb1103', 960, 336 },
    { 'S5acu', 'uel0001', 384, 560 },
    { 'S6arty', 'uel0103', 96, 608 },
    { 'S7exp', 'uel0401', 352, 880 },
    { 'S8air', 'uea0102', 96, 896 },
    { 'S10frig', 'ues0103', 768, 656 },
}
local OWN = { 784, 144 }
local ALLY = { 816, 224 }
local su, scouts = {}, {}
for i, s in SITES do
    su[i] = make(s[1], s[2], ENEMY, s[3], s[4])
    scouts[i] = make('sc' .. i, 'uel0101', TESTER, s[3] + 6, s[4] + 6)
end
local own = make('own', 'uel0201', TESTER, OWN[1], OWN[2])
local ally = make('ally', 'uel0201', armies[2], ALLY[1], ALLY[2])

-- GetThreatAtPosition with no trailing nils
local function thr(p, rings, ty)
    local ok, v
    if ty then ok, v = pcall(brain.GetThreatAtPosition, brain, p, rings, true, ty)
    else ok, v = pcall(brain.GetThreatAtPosition, brain, p, rings, true) end
    if ok then return N(v) end
    return 'ERR'
end

local FINDERS = { 'FindClosestUnit', 'FindFurthestUnit', 'FindHighestValueUnit', 'FindClosestUnitToBase' }
local CASES = {
    { 'Attack Enemy true ALL', 'Attack', 'Enemy', true, C.ALLUNITS },
    { 'Attack Enemy false ALL', 'Attack', 'Enemy', false, C.ALLUNITS },
    { 'Attack Enemy true STRUCTURE', 'Attack', 'Enemy', true, C.STRUCTURE },
    { 'Attack Enemy true AIR', 'Attack', 'Enemy', true, C.AIR },
    { 'Attack Enemy false AIR', 'Attack', 'Enemy', false, C.AIR },
    { 'Attack Enemy true TECH3', 'Attack', 'Enemy', true, C.TECH3 },
    { 'Attack Enemy true COMMAND', 'Attack', 'Enemy', true, C.COMMAND },
    { 'Guard Enemy true ALL', 'Guard', 'Enemy', true, C.ALLUNITS },
    { 'Attack Ally false ALL', 'Attack', 'Ally', false, C.ALLUNITS },
    { 'Attack Ally true ALL', 'Attack', 'Ally', true, C.ALLUNITS },
}
local function finders(p, phase)
    for _, fn in FINDERS do
        local line = 'PROBE ' .. T() .. ' find ' .. phase .. ' ' .. fn
        for _, c in CASES do
            local ok, v = pcall(p[fn], p, c[2], c[3], c[4], c[5])
            line = line .. ' | ' .. c[1] .. '=' .. (ok and S(v) or ('ERR ' .. string.sub(tostring(v), 1, 60)))
        end
        LOG(line)
    end
    local line = 'PROBE ' .. T() .. ' prio ' .. phase
    for _, r in { 60, 25, 5, 200 } do
        for _, ca in { true, false } do
            local ok, v = pcall(p.FindPrioritizedUnit, p, 'Attack', 'Enemy', ca, Q, r)
            line = line .. ' | r' .. r .. ' ' .. tostring(ca) .. '=' .. (ok and S(v) or 'ERR')
        end
    end
    local ok, v = pcall(p.FindPrioritizedUnit, p, 'Attack', 'Ally', false, Q, 60)
    line = line .. ' | ally r60=' .. (ok and S(v) or 'ERR')
    LOG(line)
end

local function threats(phase)
    for _, s in SITES do
        local p = pos(s[3], s[4])
        local line = 'PROBE ' .. T() .. ' types ' .. phase .. ' ' .. s[1]
        for _, ty in TYPES do line = line .. ' ' .. ty .. '=' .. thr(p, 0, ty) end
        line = line .. ' notype=' .. thr(p, 0)
        LOG(line)
    end
    for _, i in { 1, 3, 5 } do
        local s = SITES[i]
        local p = pos(s[3], s[4])
        local line = 'PROBE ' .. T() .. ' rings ' .. phase .. ' ' .. s[1]
        for r = 0, 4 do line = line .. ' r' .. r .. '=' .. thr(p, r, 'Overall') end
        line = line .. ' r0.5=' .. thr(p, 0.5, 'Overall') .. ' r1.9=' .. thr(p, 1.9, 'Overall')
        LOG(line)
    end
    local line = 'PROBE ' .. T() .. ' own ' .. phase
    for _, ty in { 'Overall', 'Land', 'AntiSurface' } do
        line = line .. ' own:' .. ty .. '=' .. thr(pos(OWN[1], OWN[2]), 0, ty) .. ' ally:' .. ty .. '=' .. thr(pos(ALLY[1], ALLY[2]), 0, ty)
            .. ' Q:' .. ty .. '=' .. thr(Q, 0, ty)
    end
    LOG(line)
end
local function scan(phase, i, ty)
    local s = SITES[i]
    for _, axis in { 'x', 'z' } do
        local line = 'PROBE ' .. T() .. ' scan' .. axis .. ' ' .. phase .. ' ' .. s[1] .. ' ' .. ty
        local last
        for d = -96, 96, 2 do
            local x, z = s[3], s[4]
            if axis == 'x' then x = x + d else z = z + d end
            if x > 2 and x < 1022 and z > 2 and z < 1022 then
                local v = thr({ x, 0, z }, 0, ty)
                if v ~= last then line = line .. ' ' .. axis .. (axis == 'x' and x or z) .. '=' .. v; last = v end
            end
        end
        LOG(line)
    end
end

ForkThread(function()
    LOG('PROBE 0 setup')
    wait_to(2)
    local p1 = brain:MakePlatoon('ProbeFind', '')
    tags[p1] = 'P1'
    brain:AssignUnitsToPlatoon(p1, { t[1], t[2], t[3], t[4] }, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(p1, { a1 }, 'Artillery', 'None')
    brain:AssignUnitsToPlatoon(p1, { aa }, 'Guard', 'None')
    R('SetPrioritizedTargetList', function() return p1:SetPrioritizedTargetList('Attack', { C.STRUCTURE, C.COMMAND, C.ALLUNITS }) end)
    for _, tt in { 3, 8, 20, 35, 60 } do
        wait_to(tt)
        finders(p1, 't' .. tt)
    end
    -- threat after two 30-tick map updates with the scouts in place
    wait_to(70)
    threats('B')
    scan('B', 1, 'Overall')
    scan('B', 1, 'AntiSurface')
    scan('B', 5, 'Overall')
    scan('B', 3, 'Commander')
    -- sight lost: when does remembered threat start to decay?
    wait_to(75)
    for _, sc in scouts do sc:Destroy() end
    for tt = 80, 520, 10 do
        wait_to(tt)
        local line = 'PROBE ' .. T() .. ' decay'
        for _, i in { 1, 3, 5 } do
            local s = SITES[i]
            line = line .. ' ' .. s[1] .. '=' .. thr(pos(s[3], s[4]), 0, 'Overall')
        end
        LOG(line)
        if tt == 100 then finders(p1, 't100') end
    end
    threats('C')
    finders(p1, 'end')
    WaitTicks(30)
    LOG('PROBE done')
end)
