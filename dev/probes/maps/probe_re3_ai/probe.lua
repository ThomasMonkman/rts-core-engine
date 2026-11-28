-- Differential probe (sweep 3): AI brain calls.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re3_ai 90
--   ours: --headless --no-ai --ticks 200 --script dev/probes/maps/probe_re3_ai/probe.lua
-- ST  stat triggers. Every brain logs OnStatsTrigger (absolute tick).
--     ST0: at absolute tick 0, armies 2.. get always-true triggers (army 3 also a unit; armies 2.. have
--     none after the clean-up): is the first check at tick 11 with no units? Late registrations at
--     abs 3 / 12 / 20 / 33 (army 2) show the period.
--     ST1 (army 1, from abs 40): script-set stats, set before vs after the trigger, created by GetArmyStat
--     first, a float, and engine stats a script sets (Enemies_Killed, Units_Active).
-- NS  brain:CreateUnitNearSpot repeated on one spot off the 2-grid: offsets from the base point.
-- AV  SetUpAttackVectorsToArmy + PickBestAttackVector over four layouts, each compare name.
local fmt = string.format
local armies = ListArmies()
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function A() return GetGameTick() end
local function P(text) LOG(fmt('PROBE %d %s', A(), text)) end
local CIV
for i, a in armies do if a == 'NEUTRAL_CIVILIAN' then CIV = i end end
SetAlliance(1, CIV, 'Enemy') SetAlliance(CIV, 1, 'Enemy')
local brain = ArmyBrains[1]
local function try(tag, f)
    local ok, a, b = pcall(f)
    if not ok then P(tag .. ' ERR ' .. tostring(a)) end
    return ok, a, b
end
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    pcall(function() u:SetFireState(1) end)
    return u
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end

-- ST: hooks on every brain
for i, b in ArmyBrains do
    local orig = b.OnStatsTrigger
    local idx = i
    b.OnStatsTrigger = function(self, name)
        P(fmt('ST fire army %d %s', idx, tostring(name)))
        if orig then return orig(self, name) end
    end
end
local nb = table.getn(ArmyBrains)
P('ST0 brains ' .. nb .. ' civ ' .. tostring(CIV))
local function always(i, tag)
    try('ST0 ' .. tag, function()
        ArmyBrains[i]:SetArmyStatsTrigger('Units_Active', tag .. '_ua', 'LessThan', 100000)
        ArmyBrains[i]:SetArmyStatsTrigger('Economy_Stored_Mass', tag .. '_ms', 'GreaterThanOrEqual', 0)
    end)
    P(fmt('ST0 set army %d %s units %d', i, tag, table.getn(ArmyBrains[i]:GetListOfUnits(categories.ALLUNITS, false) or {})))
end
if nb >= 3 then make('uel0201', 3, 912, 240) end
for i = 2, nb do always(i, 'a' .. i .. 't0') end
ForkThread(function()
    for t = 1, 40 do
        WaitTicks(1)
        local a = A()
        if a == 3 or a == 12 or a == 20 or a == 33 then always(2, 'a2t' .. a) end
        if a == 21 and CIV then always(CIV, 'civt21') end
        if (a == 2 or a == 5) and nb >= 4 then always(4, 'a4t' .. a) end
        if a == 5 and nb >= 9 then always(9, 'a9t5') end
        if a == 30 and nb >= 9 then make('uel0201', 9, 912, 384) P('ST2 unit for army 9') end
        if a == 35 and nb >= 9 then always(9, 'a9t35') end
    end
end)

-- ST1
ForkThread(function()
    while A() < 40 do WaitTicks(1) end
    local function set(stat, v)
        try('ST1 set ' .. stat, function() brain:SetArmyStat(stat, v) end)
        local ok, s = pcall(function() return brain:GetArmyStat(stat, 0) end)
        local val = ok and type(s) == 'table' and s.Value or s
        P(fmt('ST1 set %s %s -> %s', stat, tostring(v), tostring(val)))
    end
    local function trig(stat, name, cmp, v)
        try('ST1 trig ' .. name, function() brain:SetArmyStatsTrigger(stat, name, cmp, v) end)
        P(fmt('ST1 trig %s %s %s %s', stat, name, cmp, tostring(v)))
    end
    -- before: stat set, trigger 2 ticks later
    set('Probe_Before', 10)
    set('Probe_FloatBefore', 10.5)
    set('Enemies_Killed', 5)
    set('Probe_LtBefore', 3)
    -- created by GetArmyStat before the trigger
    pcall(function() brain:GetArmyStat('Probe_Created', 0) end)
    WaitTicks(2)
    trig('Probe_Before', 'pB', 'GreaterThanOrEqual', 5)
    trig('Probe_FloatBefore', 'pFB', 'GreaterThan', 5)
    trig('Enemies_Killed', 'pEK', 'GreaterThanOrEqual', 3)
    trig('Probe_LtBefore', 'pLtB', 'LessThan', 5)
    trig('Probe_After', 'pA', 'GreaterThanOrEqual', 5)
    trig('Probe_Created', 'pC', 'GreaterThanOrEqual', 5)
    trig('Probe_Never', 'pN0', 'LessThan', 5)
    trig('Units_Active', 'pUA', 'GreaterThanOrEqual', 50)
    trig('Enemies_Killed', 'pEK2', 'GreaterThanOrEqual', 20)
    WaitTicks(3)
    set('Probe_After', 10)
    set('Probe_Created', 10)
    set('Units_Active', 60)
    set('Enemies_Killed', 25)
    WaitTicks(3)
    set('Probe_After', 12)
    WaitTicks(3)
    P('ST1 new unit')
    make('uel0201', 1, 656, 160)
    WaitTicks(5)
    P('ST1 end')
    trig('Probe_Late', 'pL1', 'GreaterThanOrEqual', 5)
    WaitTicks(2)
    set('Probe_Late', 10)
    WaitTicks(2)
    trig('Probe_Late', 'pL2', 'GreaterThanOrEqual', 5)
    WaitTicks(2)
    set('Probe_Late', 11)
    WaitTicks(2)
    set('Probe_Float2', 2.75)
    trig('Probe_Float2', 'pF2', 'GreaterThan', 2.5)
    WaitTicks(3)
    P('ST2 end')
end)

-- NS
ForkThread(function()
    while A() < 15 do WaitTicks(1) end
    -- flat land base points on a 32 grid (even coordinates)
    local bases = {}
    local function scan(x0, x1, z0, z1)
        for z = z0, z1, 32 do
            for x = x0, x1, 32 do
                local lo, hi, wet = 1e9, -1e9, false
                for dx = -14, 14, 7 do
                    for dz = -14, 14, 7 do
                        local h = GetTerrainHeight(x + dx, z + dz)
                        if GetSurfaceHeight(x + dx, z + dz) > h + 0.01 then wet = true end
                        if h < lo then lo = h end
                        if h > hi then hi = h end
                    end
                end
                local far = true
                for _, b in bases do
                    if math.abs(b[1] - x) < 48 and math.abs(b[2] - z) < 48 then far = false end
                end
                if far and not wet and hi - lo < 1.0 then table.insert(bases, { x, z }) end
            end
        end
    end
    scan(64, 464, 592, 992)
    scan(560, 992, 32, 448)
    local line = {}
    for _, b in bases do table.insert(line, b[1] .. ',' .. b[2]) end
    P('NS bases ' .. table.getn(bases) .. ' ' .. table.concat(line, ' '))
    local CASES = {
        { 'ueb1101', 0, 0, 6 }, { 'ueb1101', 0.3, 0.7, 6 }, { 'ueb1101', 1, 1, 6 }, { 'ueb1101', 1.5, 0.5, 6 },
        { 'ueb1101', 0.9, 0.1, 6 }, { 'ueb1101', 1, 0, 6 }, { 'ueb1101', -0.3, -0.7, 6 }, { 'ueb1101', 2, 2, 6 },
        { 'ueb5101', 0, 0, 6 }, { 'ueb5101', 0.3, 0.7, 6 }, { 'ueb5101', 1, 1, 6 }, { 'ueb5101', 1.5, 1.5, 6 },
        { 'ueb0101', 0, 0, 5 }, { 'ueb0101', 0.3, 0.7, 5 }, { 'ueb0101', 1, 1, 5 }, { 'ueb0101', 3, 3, 5 },
        { 'uel0201', 0.3, 0.7, 6 }, { 'uel0201', 1, 1, 6 },
        { 'ueb1301', 0, 0, 5 }, { 'ueb1301', 0.3, 0.7, 5 }, { 'ueb1301', 1, 1, 5 },
    }
    for i, c in CASES do
        local b = bases[i]
        if not b then P('NS out of bases') break end
        local x, z = b[1] + c[2], b[2] + c[3]
        local out = {}
        for k = 1, c[4] do
            local ok, u = pcall(function() return brain:CreateUnitNearSpot(c[1], x, z) end)
            if ok and u then
                local p = u:GetPosition()
                table.insert(out, fmt('%.1f,%.1f', p[1] - b[1], p[3] - b[2]))
            else
                table.insert(out, ok and 'nil' or ('ERR ' .. tostring(u)))
            end
        end
        P(fmt('NS %s off %.1f,%.1f base %d,%d : %s', c[1], c[2], c[3], b[1], b[2], table.concat(out, ' ')))
    end
    -- NS2: first spot of a single pgen / wall over a sweep of offsets, 3 points per spare base
    local SW = { { 'ueb1101', 0.25, 0 }, { 'ueb1101', 0.5, 0 }, { 'ueb1101', 0.75, 0 }, { 'ueb1101', 1.0, 0 }, { 'ueb1101', 1.25, 0 },
        { 'ueb1101', 1.5, 0 }, { 'ueb1101', 1.75, 0 }, { 'ueb1101', 2.5, 0 }, { 'ueb1101', 3.0, 0 }, { 'ueb1101', 0, 1.0 },
        { 'ueb1101', -1.0, 0 }, { 'ueb5101', 1.0, 0 }, { 'ueb5101', 1.5, 0 }, { 'ueb5101', 0.5, 0 }, { 'uel0201', 1.0, 0 } }
    for k, c in SW do
        local b = bases[22 + math.floor((k - 1) / 3)]
        if not b then break end
        local bx = b[1] + (math.mod(k - 1, 3) - 1) * 10
        local ok, u = pcall(function() return brain:CreateUnitNearSpot(c[1], bx + c[2], b[2] + c[3]) end)
        local r = 'fail'
        if ok and u then local p = u:GetPosition() r = fmt('%.2f,%.2f', p[1] - bx, p[3] - b[2]) end
        P(fmt('NS2 %s off %.2f,%.2f at %d,%d -> %s', c[1], c[2], c[3], bx, b[2], r))
    end
end)

-- AV
ForkThread(function()
    while A() < 60 do WaitTicks(1) end
    local C = categories
    local civ = ArmyBrains[CIV]
    brain:SetCurrentEnemy(civ)
    local squad = { make('uel0201', 1, 600, 100), make('uel0201', 1, 603, 100) }
    local p = brain:MakePlatoon('ProbeAV', '')
    brain:AssignUnitsToPlatoon(p, squad, 'Attack', 'None')
    local enemy = {}
    local function S(v)
        if type(v) ~= 'table' then return tostring(v) end
        return fmt('(%s,%s %s,%s)', tostring(v.px), tostring(v.pz), tostring(v.vx), tostring(v.vz))
    end
    local function layout(name, units, sx, sz)
        for _, u in enemy do if not u:BeenDestroyed() then u:Destroy() end end
        enemy = {}
        for _, d in units do table.insert(enemy, make(d[1], CIV, d[2], d[3])) end
        Warp(squad[1], pos(sx, sz)) Warp(squad[2], pos(sx + 3, sz))
    end
    local function analyse(name)
        for _, cat in { 'STRUCTURE', 'DEFENSE', 'ALLUNITS' } do
            try('AV setup', function() brain:SetUpAttackVectorsToArmy(C[cat]) end)
            local ok, v = pcall(function() return brain:GetAttackVectors() end)
            local list, idx = {}, {}
            if ok and type(v) == 'table' then
                for i, e in v do
                    table.insert(list, S(e))
                    idx[S(e)] = i
                end
            end
            P(fmt('AV %s vec %s n %d %s', name, cat, table.getn(list), table.concat(list, ' ')))
            for _, pcat in { 'STRUCTURE', 'DEFENSE', 'ALLUNITS' } do
                local res = {}
                for _, cmp in { 'Closest', 'Furthest', 'LeastDefended', 'HighestValue' } do
                    local ok2, r = pcall(function() return brain:PickBestAttackVector(p, 'Attack', 'Enemy', cmp, C[pcat]) end)
                    table.insert(res, cmp .. '=' .. (ok2 and (S(r) .. '#' .. tostring(idx[S(r)])) or ('ERR ' .. tostring(r))))
                end
                P(fmt('AV %s pick vec=%s cat=%s %s', name, cat, pcat, table.concat(res, ' ')))
            end
        end
    end
    local L12 = { { 'ueb1101', 720, 176 }, { 'ueb1101', 880, 368 } }
    local L34 = { { 'ueb1101', 690, 120 }, { 'ueb1101', 880, 240 }, { 'ueb2101', 872, 232 }, { 'ueb2101', 888, 232 }, { 'ueb2101', 880, 252 },
        { 'ueb1301', 700, 340 } }
    try('AV L1', function() layout('L1', L12, 600, 100) end)
    WaitTicks(2)
    try('AV L1 an', function() analyse('L1') end)
    try('AV L2', function() layout('L2', L12, 900, 400) end)
    WaitTicks(2)
    try('AV L2 an', function() analyse('L2') end)
    try('AV L3', function() layout('L3', L34, 780, 200) end)
    WaitTicks(2)
    try('AV L3 an', function() analyse('L3') end)
    try('AV L4', function() layout('L4', L34, 656, 250) end)
    WaitTicks(2)
    try('AV L4 an', function() analyse('L4') end)
    P('AV end')
end)

ForkThread(function()
    while A() < 110 do WaitTicks(1) end
    WaitTicks(30)
    LOG('PROBE done')
end)
