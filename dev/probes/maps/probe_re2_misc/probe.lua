-- Differential probe (sweep 2 leftovers): radar / sonar stealth against
-- IsOnRadar / IsOnSonar (ST*), when FAF's EnergyDepleted stat trigger first
-- fires for a fresh radar in an empty economy (E*), and whether an idle
-- gunship lands after a move (G*).
--   real game (Windows): dev/probes/run_faf.ps1 probe_re2_misc 130
--   ours: --headless --no-ai --ticks 460 --script dev/probes/maps/probe_re2_misc/probe.lua
-- Lines: PROBE <tick> <tag> <text>; tick counted from the probe's start.
local fmt = string.format
local armies = ListArmies()
local t0 = GetGameTick()
local function P(tag, text) LOG(fmt('PROBE %d %s %s', GetGameTick() - t0, tag, text)) end
local function stat(b, name)
    local ok, s = pcall(function() return b:GetArmyStat(name, 0) end)
    if not ok then return 'ERR' end
    if type(s) ~= 'table' then return tostring(s) end
    return fmt('%.3f', s.Value or -1)
end
local function econ(i)
    local b = ArmyBrains[i]
    local ok, st = pcall(function() return b:GetEconomyStored('ENERGY') end)
    return fmt('stored %s max %s ratio %s depleted %s', ok and fmt('%.2f', st) or 'ERR', stat(b, 'Economy_MaxStorage_Energy'), stat(b, 'Economy_Ratio_Energy'), tostring(b.EnergyDepleted))
end
P('start', 'abs tick ' .. GetGameTick() .. ' armies ' .. table.concat(armies, ','))
for i = 2, 6 do P('E' .. i, 'before destroy ' .. econ(i)) end
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function indexOf(name)
    for i, a in armies do
        if a == name then return i end
    end
end
local A1, CIV = 1, indexOf('NEUTRAL_CIVILIAN')
local function make(bp, army, x, z, y)
    return CreateUnitHPR(bp, army, x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
end

-- E: armies 2-6, a T1 radar each made at the probe's start, nothing else.
-- E2 nothing given; E3 GiveResource at t0 (after the radar); E4 at t1;
-- E5 at t2; E6 GiveStorage 1000 and GiveResource at t0.
for i, b in ArmyBrains do
    local orig = b.OnStatsTrigger
    local idx = i
    b.OnStatsTrigger = function(self, name)
        P('E' .. idx, 'OnStatsTrigger ' .. tostring(name) .. ' ' .. econ(idx))
        if orig then return orig(self, name) end
    end
end
local ESITES = { [2] = { 912, 240 }, [3] = { 960, 336 }, [4] = { 880, 400 }, [5] = { 656, 384 }, [6] = { 576, 464 } }
local radars = {}
for i = 2, 6 do
    P('E' .. i, 'after destroy ' .. econ(i))
    local r = make('ueb3101', i, ESITES[i][1], ESITES[i][2])
    radars[i] = r
    local idx = i
    local od, ov = r.OnEnergyDepleted, r.OnEnergyViable
    r.OnEnergyDepleted = function(self) P('E' .. idx, 'radar OnEnergyDepleted') if od then return od(self) end end
    r.OnEnergyViable = function(self) P('E' .. idx, 'radar OnEnergyViable') if ov then return ov(self) end end
    P('E' .. i, 'radar made ' .. econ(i) .. ' radar ' .. tostring(r:IsIntelEnabled('Radar')))
end
local function give(i)
    ArmyBrains[i]:GiveResource('ENERGY', 1000)
    P('E' .. i, 'GiveResource 1000 at abs ' .. GetGameTick() .. ' ' .. econ(i))
end
ArmyBrains[6]:GiveStorage('ENERGY', 1000)
give(3) give(6)
ForkThread(function()
    for t = 1, 100 do
        WaitTicks(1)
        if t == 1 then give(4) end
        if t == 2 then give(5) end
        if t <= 12 or math.mod(t, 5) == 0 then
            for i = 2, 6 do
                local r = radars[i]
                P('E' .. i, econ(i) .. ' radar ' .. (r:BeenDestroyed() and 'gone' or tostring(r:IsIntelEnabled('Radar'))))
            end
        end
    end
end)

-- G: army 8 gunships (hold fire), spawned in the air; 'mv' ordered 40 east
-- at t5, 'idle' never ordered. 'w' over water.
local G = {}
local GSPEC = { { 'uea0203', 'mv', 608, 80 }, { 'xra0105', 'mv', 688, 80 }, { 'uea0303', 'mv', 912, 96 }, { 'uea0203', 'idle', 784, 144 },
    { 'xra0105', 'idle', 736, 224 }, { 'uea0303', 'idle', 816, 224 }, { 'uea0203', 'mvw', 100, 300 } }
for _, s in GSPEC do
    local x, z = s[3], s[4]
    local u = CreateUnitHPR(s[1], 8, x, GetSurfaceHeight(x, z) + 20, z, 0, 0, 0)
    pcall(function() u:SetFireState(1) end)
    table.insert(G, { tag = 'G' .. s[1] .. s[2], u = u, x = x, z = z, mv = s[2] ~= 'idle', last = '' })
end
local function gs(g)
    local u = g.u
    if u:BeenDestroyed() then return 'gone', 'gone' end
    local x, y, z = u:GetPositionXYZ()
    local vx, vy, vz = u:GetVelocity()
    local q = table.getn(u:GetCommandQueue() or {})
    local layer = tostring(u:GetCurrentLayer())
    local mov = tostring(u:IsUnitState('Moving'))
    return fmt('rel %.2f %.2f h=%.2f v=%.2f vy=%.2f %s q%d moving %s', x - g.x, z - g.z, y - GetSurfaceHeight(x, z), 10 * math.sqrt(vx * vx + vz * vz), 10 * vy, layer, q, mov),
        layer .. q .. mov
end
ForkThread(function()
    for t = 0, 420 do
        if t == 5 then
            for _, g in G do
                if g.mv then IssueMove({ g.u }, { g.x + 40, GetSurfaceHeight(g.x + 40, g.z), g.z }) end
            end
        end
        for _, g in G do
            local s, key = gs(g)
            if math.mod(t, 5) == 0 or key ~= g.last then P(g.tag, s) end
            g.last = key
        end
        WaitTicks(1)
    end
end)

-- ST: radar / sonar stealth. Army 1 observers (hold fire), civilian targets
-- (enemy, hold fire, own intel off). Set up at t20 (FAF reveals civilians
-- that exist at tick 10).
SetAlliance(A1, CIV, 'Enemy') SetAlliance(CIV, A1, 'Enemy')
local FLAGS = { { 'IsSeenNow', 'S' }, { 'IsOnRadar', 'R' }, { 'IsOnSonar', 's' }, { 'IsOnOmni', 'O' }, { 'IsSeenEver', 'E' } }
local function flags(target)
    if target:BeenDestroyed() then return 'gone' end
    local ok, b = pcall(function() return target:GetBlip(A1) end)
    if not ok then return 'ERR' end
    if not b then return 'noblip' end
    local s = ''
    for _, f in FLAGS do
        local ok2, v = pcall(function() return b[f[1]](b, A1) end)
        s = s .. (ok2 and (v and f[2] or '.') or '?')
    end
    return s
end
local function off(u)
    for _, t in { 'Vision', 'WaterVision', 'Radar', 'Sonar', 'Omni' } do
        pcall(function() u:SetIntelRadius(t, 0) end)
        pcall(function() u:DisableIntel(t) end)
    end
end
local function sense(u, t, r)
    local ok, err = pcall(function()
        u:InitIntel(A1, t, r)
        u:SetIntelRadius(t, r)
        u:EnableIntel(t)
    end)
    if not ok then P('ST', 'sense ' .. t .. ' ERR | ' .. string.sub(tostring(err), 1, 80)) end
end
local function stealth(kind)
    return function(u)
        u:InitIntel(CIV, kind, 1)
        u:EnableIntel(kind)
    end
end
local lanes = {}
-- obs: { bp, water, { {sense, radius}, ... } }; targets { dist, bp, setup, name }
local function lane(name, site, obs, targets)
    local x, z = site[1], site[2]
    local y = obs.water and GetSurfaceHeight(x, z) or nil
    local o = make(obs.bp, A1, x, z, y)
    o:SetFireState(1)
    if obs.clear then off(o) end
    for _, s in obs.senses do sense(o, s[1], s[2]) end
    local l = { name = name, obs = o, targets = {} }
    for i, t in targets do
        local u = make(t[2], CIV, x + t[1], z, y)
        u:SetFireState(1)
        off(u)
        if t[3] then
            local ok, err = pcall(t[3], u)
            if not ok then P('ST', name .. ' setup ' .. i .. ' ERR | ' .. string.sub(tostring(err), 1, 80)) end
        end
        table.insert(l.targets, { u = u, name = (t[4] or 'plain') .. '@' .. t[1], x = x, z = z, y = y })
    end
    table.insert(lanes, l)
    return l
end
local function sample(label)
    for _, l in lanes do
        local s = label .. ' ' .. l.name .. ':'
        for _, t in l.targets do s = s .. ' ' .. t.name .. '=' .. flags(t.u) end
        P('ST', s)
    end
end
ForkThread(function()
    WaitTicks(20)
    local T, RS, SS = 'uel0201', stealth('RadarStealth'), stealth('SonarStealth')
    local W = 'urs0203'
    -- pgen observer: blueprint vision 10, radar 24
    lane('rs-radar', { 176, 688 }, { bp = 'ueb1101', senses = { { 'Radar', 24 } } },
        { { 5, T, RS, 'rs' }, { 9, T, RS, 'rs' }, { 11, T, RS, 'rs' }, { 16, T, RS, 'rs' }, { 23, T, RS, 'rs' }, { 25, T, RS, 'rs' }, { -23, T, nil, 'plain' }, { -25, T, nil, 'plain' } })
    -- pgen observer: vision 30, radar 24
    lane('rs-bigvis', { 320, 720 }, { bp = 'ueb1101', clear = true, senses = { { 'Vision', 30 }, { 'Radar', 24 } } },
        { { 16, T, RS, 'rs' }, { 23, T, RS, 'rs' }, { 25, T, RS, 'rs' }, { 28, T, RS, 'rs' }, { -28, T, nil, 'plain' } })
    -- pgen observer: vision 10, no radar: stealthed and plain in vision
    lane('rs-norad', { 432, 912 }, { bp = 'ueb1101', senses = {} }, { { 5, T, RS, 'rs' }, { -5, T, nil, 'plain' } })
    -- pgen observer: radar 24; targets at 5 warped to 16 at t+30 (seen, then radar only)
    local lose = lane('rs-lose', { 112, 784 }, { bp = 'ueb1101', senses = { { 'Radar', 24 } } }, { { 5, T, RS, 'rs' }, { -5, T, nil, 'plain' } })
    -- frigate observers: vision 4 + water vision 4 + sonar 44
    lane('ss-sonar', { 208, 128 }, { bp = 'ues0103', water = true, clear = true, senses = { { 'Vision', 4 }, { 'WaterVision', 4 }, { 'Sonar', 44 } } },
        { { 3, W, SS, 'ss' }, { 10, W, SS, 'ss' }, { 36, W, SS, 'ss' }, { 42, W, SS, 'ss' }, { 46, W, SS, 'ss' }, { -42, W, nil, 'plain' }, { -46, W, nil, 'plain' }, { -3, W, nil, 'plain' } })
    -- frigate observer: blueprint vision, water vision 30, sonar 44
    lane('ss-bigvis', { 448, 320 }, { bp = 'ues0103', water = true, senses = { { 'WaterVision', 30 }, { 'Sonar', 44 } } },
        { { 10, W, SS, 'ss' }, { 20, W, SS, 'ss' }, { 36, W, SS, 'ss' }, { -36, W, nil, 'plain' } })
    -- frigate observer: radar 44, sonar 0: sonar-stealthed ship and sub
    lane('ss-radar', { 768, 656 }, { bp = 'ues0103', water = true, senses = { { 'Radar', 44 } } },
        { { 10, 'ues0103', SS, 'ssship' }, { 36, 'ues0103', SS, 'ssship' }, { -36, 'ues0103', nil, 'ship' } })
    WaitTicks(12)
    sample('settled')
    for _, l in lanes do
        local s = l.name .. ' layers:'
        for _, t in l.targets do s = s .. ' ' .. t.name .. '=' .. tostring(t.u:GetCurrentLayer()) end
        P('ST', s)
    end
    WaitTicks(10)
    sample('settled2')
    for _, t in lose.targets do
        local dx = (string.sub(t.name, 1, 2) == 'rs') and 16 or -16
        pcall(function() Warp(t.u, { t.x + dx, GetTerrainHeight(t.x + dx, t.z), t.z }) end)
    end
    WaitTicks(1)
    sample('warped+1')
    WaitTicks(10)
    sample('warped+11')
    WaitTicks(30)
    sample('warped+41')
end)

ForkThread(function()
    WaitTicks(425)
    WaitTicks(30)
    LOG('PROBE done')
end)
