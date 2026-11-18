-- Differential probe: intel (vision, radar, sonar, omni) against stealth and
-- cloak, in a format both engines print identically ("PROBE ..." lines).
--   real game (Windows): dev/probes/run_faf.ps1 probe_unit_intel 90
--   ours: cargo run -- --headless --no-ai --ticks 140 --script dev/probes/maps/probe_unit_intel/probe.lua
-- Lanes 70+ apart. Each has an army 1 observer (hold fire) whose intel is
-- set by hand (see the lanes) and NEUTRAL_CIVILIAN
-- targets (enemy, hold fire, their own intel off) at given distances east.
-- Samples at ticks 12, 22 (settled), 37 (the sense disabled at 25), 52
-- (enabled again at 40), 67 (observer destroyed at 55).
-- Per target: its blip for army 1 ('noblip' or the flags S seen now, R on
-- radar, s on sonar, O on omni, E seen ever, F known fake, M maybe dead)
-- and 'A' when brain:GetUnitsAroundPoint(..., 'Enemy') lists it.
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
LOG('PROBE armynames ' .. table.concat(armies, ','))
local fmt = string.format
local function indexOf(name)
    for i, a in armies do
        if a == name then return i end
    end
end
local A1, CIV = 1, indexOf('NEUTRAL_CIVILIAN')
SetAlliance(A1, CIV, 'Enemy') SetAlliance(CIV, A1, 'Enemy')
local brain = ArmyBrains[A1]
local t0 = GetGameTick()
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick() - t0, text)) end
local function make(bp, army, x, z, y)
    return CreateUnitHPR(bp, army, x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
end
local TYPES = { 'Vision', 'WaterVision', 'Radar', 'Sonar', 'Omni', 'RadarStealth', 'SonarStealth', 'Cloak', 'RadarStealthField', 'SonarStealthField', 'CloakField', 'Jammer', 'Spoof' }
local function intel(u)
    local parts = {}
    for _, t in TYPES do
        local ok, r = pcall(function() return u:GetIntelRadius(t) end)
        local ok2, e = pcall(function() return u:IsIntelEnabled(t) end)
        table.insert(parts, t .. '=' .. (ok and fmt('%.0f', r or -1) or 'ERR') .. (ok2 and (e and '+' or '-') or '?'))
    end
    return table.concat(parts, ' ')
end
local function off(u)
    for _, t in { 'Vision', 'WaterVision', 'Radar', 'Sonar', 'Omni' } do
        pcall(function() u:SetIntelRadius(t, 0) end)
        pcall(function() u:DisableIntel(t) end)
    end
end
local function sense(u, army, t, r)
    local ok, err = pcall(function()
        u:InitIntel(army, t, r)
        u:SetIntelRadius(t, r)
        u:EnableIntel(t)
    end)
    if not ok then P('sense ' .. t .. ' ERR | ' .. string.sub(tostring(err), 1, 80)) end
end
local FLAGS = { { 'IsSeenNow', 'S' }, { 'IsOnRadar', 'R' }, { 'IsOnSonar', 's' }, { 'IsOnOmni', 'O' }, { 'IsSeenEver', 'E' }, { 'IsKnownFake', 'F' }, { 'IsMaybeDead', 'M' } }
local function flags(target, around)
    if target:BeenDestroyed() then return 'gone' end
    local ok, b = pcall(function() return target:GetBlip(A1) end)
    local s
    if not ok then
        s = 'ERR'
    elseif not b then
        s = 'noblip'
    else
        s = ''
        for _, f in FLAGS do
            local ok2, v = pcall(function() return b[f[1]](b, A1) end)
            s = s .. (ok2 and (v and f[2] or '.') or '?')
        end
    end
    for _, u in around do
        if u == target then s = s .. ' A' end
    end
    return s
end
local lanes = {}
-- targets: { dist, bp, setup, name }
local function lane(name, site, obsBp, primary, targets, water, radius)
    local x, z = site[1], site[2]
    local y = water and GetSurfaceHeight(x, z) or nil
    local obs = make(obsBp, A1, x, z, y)
    obs:SetFireState(1)
    if water or radius == 4 then
        off(obs)
        sense(obs, A1, 'Vision', 4)
    end
    if water then sense(obs, A1, 'WaterVision', 4) end
    if primary then sense(obs, A1, primary, water and radius or 24) end
    local l = { name = name, obs = obs, primary = primary, targets = {}, x = x, z = z }
    for i, t in targets do
        local u = make(t[2], CIV, x + t[1], z, t.deep and GetTerrainHeight(x + t[1], z) or y)
        u:SetFireState(1)
        off(u)
        if t[3] then
            local ok, err = pcall(t[3], u)
            if not ok then P(name .. ' setup ' .. i .. ' ERR | ' .. string.sub(tostring(err), 1, 80)) end
        end
        table.insert(l.targets, { u = u, name = (t[4] or 'plain') .. '@' .. t[1] })
    end
    table.insert(lanes, l)
    P(fmt('%s observer %s layer %s', name, intel(obs), obs:GetCurrentLayer()))
    return l
end
local function stealth(kind)
    return function(u)
        u:InitIntel(CIV, kind, 1)
        u:EnableIntel(kind)
    end
end
local function field(kind, r)
    return function(u)
        u:InitIntel(CIV, kind, r)
        u:SetIntelRadius(kind, r)
        u:EnableIntel(kind)
    end
end
-- (Set up at tick 20: FAF's RevealCivilians gives every army vision of the
-- civilian units that exist at tick 10.)
local function setup()
local T, G = 'uel0201', 'ueb1101'
-- Land observers are T1 generators (vision 10 from the blueprint, left as
-- it is) with a 24-radius sense; the 'vision' lane's is a tank whose vision
-- is set to 4 by hand.
local plain = { { 5, T }, { 16, T }, { 23, T }, { 25, T }, { 28, T } }
lane('vision', { 384, 560 }, T, nil, { { 3, T }, { 3.9, T }, { 4.5, T }, { 12, T }, { 22, T }, { -3, T, stealth('Cloak'), 'cloak' }, { -3.5, T, stealth('RadarStealth'), 'rstealth' } }, false, 4)
lane('novision', { 432, 912 }, G, nil, { { 5, T }, { 9, T }, { 11, T }, { 14, T }, { -5, T, stealth('Cloak'), 'cloak' } })
lane('radar', { 96, 608 }, G, 'Radar', plain)
lane('omni', { 352, 640 }, G, 'Omni', plain)
lane('sonar-land', { 64, 688 }, G, 'Sonar', plain)
lane('radar-stealth', { 176, 688 }, G, 'Radar', { { 16, T, stealth('RadarStealth'), 'rstealth' }, { -16, T, stealth('SonarStealth'), 'sstealth' },
    { 5, T, stealth('RadarStealth'), 'rstealth' }, { -5, T, stealth('Cloak'), 'cloak' }, { 20, T, stealth('Cloak'), 'cloak' } })
lane('radar-field', { 320, 720 }, G, 'Radar', { { 16, T, field('RadarStealthField', 3), 'rfield' }, { 18, T, nil, 'infield' }, { -16, T, nil, 'outfield' } })
lane('omni-stealth', { 224, 768 }, G, 'Omni', { { 16, T, stealth('RadarStealth'), 'rstealth' }, { -16, T, stealth('Cloak'), 'cloak' },
    { 20, T, field('CloakField', 3), 'cfield' }, { 18, T, nil, 'incfield' } })
lane('cloak-field', { 112, 784 }, G, nil, { { 5, T, field('CloakField', 2), 'cfield' }, { 4, T, nil, 'incfield' }, { -5, T, nil, 'outfield' } })
lane('air-target', { 304, 800 }, G, 'Radar', { { 16, 'uea0102', nil, 'air' }, { 5, 'uea0102', nil, 'air' }, { -16, 'ueb1101', nil, 'structure' }, { -5, 'ueb1101', nil, 'structure' } })
local defaults = { 'ueb3101', 'ueb3102', 'uel0101', 'uel0201', 'uea0101', 'ueb1101', 'url0306', 'ues0103', 'xrl0302', 'urb4203', 'uel0001' }
for i, bp in defaults do
    local ok, u = pcall(make, bp, A1, 240 + 6 * i, 880)
    if ok and u then
        P('default ' .. bp .. ' ' .. intel(u))
        u:Destroy()
    else
        P('default ' .. bp .. ' create failed')
    end
end
-- Water observers are frigates (vision 32 from the blueprint; set to 4 by
-- hand, which may not take) with a 44-radius sense.
local W = 'urs0203'
lane('sonar-water', { 208, 128 }, 'ues0103', 'Sonar', { { 38, W, nil, 'sub' }, { -38, 'ues0103', nil, 'ship' }, { 3, W, nil, 'sub' }, { -3, 'ues0103', nil, 'ship' },
    { 36, W, stealth('SonarStealth'), 'sub-sstealth' }, { 48, W, nil, 'sub' }, { 40, W, nil, 'seabed-sub', deep = true }, { 10, W, nil, 'sub' }, { -10, 'ues0103', nil, 'ship' } }, true, 44)
lane('radar-water', { 448, 320 }, 'ues0103', 'Radar', { { 38, W, nil, 'sub' }, { -38, 'ues0103', nil, 'ship' }, { 3, W, nil, 'sub' }, { -3, 'ues0103', nil, 'ship' }, { 10, W, nil, 'sub' } }, true, 44)
lane('omni-water', { 768, 656 }, 'ues0103', 'Omni', { { 38, W, nil, 'sub' }, { -38, 'ues0103', nil, 'ship' }, { 36, W, stealth('SonarStealth'), 'sub-sstealth' } }, true, 44)

end

local function sample(label)
    for _, l in lanes do
        local around = {}
        pcall(function() around = brain:GetUnitsAroundPoint(categories.ALLUNITS, { l.x, 0, l.z }, 50, 'Enemy') or {} end)
        local s = label .. ' ' .. l.name .. ':'
        for _, t in l.targets do
            s = s .. ' ' .. t.name .. '=' .. flags(t.u, around)
        end
        P(s)
    end
end
ForkThread(function()
    WaitTicks(20)
    t0 = GetGameTick()
    setup()
    WaitTicks(12)
    sample('settled')
    for _, l in lanes do
        local s = l.name .. ' layers:'
        for _, t in l.targets do s = s .. ' ' .. t.name .. '=' .. tostring(t.u:GetCurrentLayer()) end
        P(s)
    end
    WaitTicks(10)
    sample('settled2')
    WaitTicks(3)
    for _, l in lanes do
        local ok, err = pcall(function() l.obs:DisableIntel(l.primary or 'Vision') end)
        if not ok then P(l.name .. ' DisableIntel ERR') end
    end
    P('disabled: radar lane observer ' .. intel(lanes[2].obs))
    WaitTicks(12)
    sample('disabled')
    WaitTicks(3)
    for _, l in lanes do
        pcall(function() l.obs:EnableIntel(l.primary or 'Vision') end)
    end
    WaitTicks(12)
    sample('enabled')
    WaitTicks(3)
    for _, l in lanes do l.obs:Destroy() end
    WaitTicks(12)
    sample('observer-gone')
    -- allied intel: army 1 has no units near; a civilian-allied check isn't possible, so army 1 allies the targets' army
    SetAlliance(A1, CIV, 'Ally') SetAlliance(CIV, A1, 'Ally')
    for _, t in lanes[1].targets do pcall(function() t.u:SetIntelRadius('Vision', 6) t.u:EnableIntel('Vision') end) end
    WaitTicks(12)
    sample('allied')
    WaitTicks(30)
    LOG('PROBE done')
end)
