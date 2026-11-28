-- Differential probe: radar jamming (Jammer intel, blueprint Intel.JammerBlips /
-- Intel.JamRadius) as an enemy radar sees it. Second version: where the fake
-- blips go, whether weapons shoot them, what happens when a jammer dies.
--   real game (Windows): dev/probes/run_faf.ps1 probe_intel_jam 150
--   ours: --headless --no-ai --ticks 420 --script dev/probes/maps/probe_intel_jam/probe.lua
-- (The first version's findings: each jammer makes JammerBlips blips, real
-- ReconBlip objects heard through OnIntelChange, at fixed offsets that move
-- with it, made again only when its jammer is switched back on.)
-- Army 1 observes, army 2 jams (all on hold fire but army 1's artillery).
-- Seton's Clutch:
--   F  UEF frigate ues0103 (5 blips, JamRadius 70/70) of army 2 at (500, 200)
--   S  UEF Sparky xel0209 (8 blips, JamRadius 18/18) of army 2 at (800, 330),
--      still
--   P  UEF spy plane uea0302 (4 blips, JamRadius Min 10 Max 25) of army 2
--      over (650, 340)
--   R  army 1's T3 radar ueb3104 at (800, 100): radar 600 covers all, its
--      omni 200 none of them
--   A  army 1's T2 artillery ueb2303 (range 115, vision 28) at (690, 375),
--      118.8 from S: S out of range, some of its fakes in range
-- t=40..240: F's and P's jammers off for 5 ticks every 25 (new fakes each
-- time: their offsets give the placement); t=260 F warped 40 east;
-- t=300 S and P destroyed; observed to t=340.
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local OBS, OWN = 1, 2
SetAlliance(armies[OBS], armies[OWN], 'Enemy')
SetAlliance(armies[OWN], armies[OBS], 'Enemy')
local brain = ArmyBrains[OBS]
for _, i in { OBS, OWN } do
    ArmyBrains[i]:GiveStorage('MASS', 50000) ArmyBrains[i]:GiveStorage('ENERGY', 500000)
end
ForkThread(function()
    while true do
        for _, i in { OBS, OWN } do
            ArmyBrains[i]:GiveResource('MASS', 50000) ArmyBrains[i]:GiveResource('ENERGY', 500000)
        end
        WaitTicks(5)
    end
end)
local t0 = GetGameTick()
local function T() return GetGameTick() - t0 end
local function P(s) LOG('PROBE ' .. T() .. ' ' .. s) end
local function N(v)
    if type(v) ~= 'number' then return tostring(v) end
    if math.abs(v) < 0.005 then v = 0 end
    return fmt('%.2f', v)
end
local function hold(u) if u and u.SetFireState then pcall(u.SetFireState, u, 1) end end
local function make(bp, army, x, z, y, nohold)
    local u = CreateUnitHPR(bp, armies[army], x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
    if not nohold then hold(u) end
    return u
end
local function B(f) local ok, v = pcall(f) if not ok then return 'E' end return v end
local function flag(v, c) if v == 'E' then return '?' end return v and c or '.' end

local frig = make('ues0103', OWN, 500, 200, GetSurfaceHeight(500, 200))
local spark = make('xel0209', OWN, 800, 330)
local plane = make('uea0302', OWN, 650, 340, GetTerrainHeight(650, 340) + 22)
local radar = make('ueb3104', OBS, 800, 100)
local arty = make('ueb2303', OBS, 690, 375, nil, true)
local JAM = { [frig:GetEntityId()] = 'F', [spark:GetEntityId()] = 'S', [plane:GetEntityId()] = 'P' }
local JU = { F = frig, S = spark, P = plane }
P('ids frig=' .. frig:GetEntityId() .. ' spark=' .. spark:GetEntityId() .. ' plane=' .. plane:GetEntityId())

local blips = {}   -- blip object -> { site, n }
local order = {}
local seq = 0
local function rel(site, p)
    local j = JU[site]
    local jp = B(function() return j:GetPosition() end)
    if type(jp) ~= 'table' and type(jp) ~= 'userdata' then return 'jammer gone' end
    local dx, dz = p[1] - jp[1], p[3] - jp[3]
    return 'dx=' .. N(dx) .. ' dy=' .. N(p[2] - jp[2]) .. ' dz=' .. N(dz) .. ' r=' .. N(math.sqrt(dx * dx + dz * dz))
end
local orig = brain.OnIntelChange
brain.OnIntelChange = function(self, blip, reconType, val)
    local okS, src = pcall(function() return blip:GetSource() end)
    local sid = okS and src and B(function() return src:GetEntityId() end) or '-'
    local site = JAM[sid]
    if site then
        local b = blips[blip]
        if not b then
            seq = seq + 1
            b = { site = site, n = seq }
            blips[blip] = b
            table.insert(order, blip)
        end
        local okp, p = pcall(function() return blip:GetPosition() end)
        P('ic ' .. site .. ' #' .. b.n .. ' ' .. tostring(reconType) .. '=' .. tostring(val) .. ' ' .. (okp and p and rel(site, p) or 'nopos')
            .. ' srcIsUnit=' .. tostring(okS and rawequal(src, JU[site])))
    end
    if orig then return orig(self, blip, reconType, val) end
end

local function flags(b)
    if B(function() return b:BeenDestroyed() end) == true then return 'D' end
    return flag(B(function() return b:IsOnRadar(OBS) end), 'R') .. flag(B(function() return b:IsSeenNow(OBS) end), 'S')
        .. flag(B(function() return b:IsOnOmni(OBS) end), 'O') .. flag(B(function() return b:IsKnownFake(OBS) end), 'F')
end

local function sample()
    for _, site in { 'F', 'S', 'P' } do
        local line = site
        local n = 0
        for _, b in order do
            local info = blips[b]
            if info.site == site then
                local fl = flags(b)
                if fl ~= 'D' then
                    n = n + 1
                    local okp, p = pcall(function() return b:GetPosition() end)
                    line = line .. ' [#' .. info.n .. ' ' .. fl .. ' ' .. (okp and p and rel(site, p) or 'nopos') .. ']'
                end
            end
        end
        P('s ' .. site .. ' live=' .. n .. ' ' .. line)
    end
end

local fired = 0
local w = B(function() return arty:GetWeapon(1) end)
if type(w) == 'table' or type(w) == 'userdata' then
    local of = w.OnWeaponFired
    w.OnWeaponFired = function(self, a, b, c)
        fired = fired + 1
        local tp = B(function() return self:GetCurrentTargetPos() end)
        P('fired ' .. fired .. ' at=' .. ((type(tp) == 'table' or type(tp) == 'userdata') and (N(tp[1]) .. ',' .. N(tp[3]) .. ' ' .. rel('S', tp)) or tostring(tp)))
        if of then return of(self, a, b, c) end
    end
end
local function weapon()
    if type(w) ~= 'table' and type(w) ~= 'userdata' then P('w none') return end
    local tp = B(function() return w:GetCurrentTargetPos() end)
    local tps = (type(tp) == 'table' or type(tp) == 'userdata') and (N(tp[1]) .. ',' .. N(tp[3]) .. ' ' .. rel('S', tp)) or tostring(tp)
    local tgt = B(function() return w:GetCurrentTarget() end)
    local ts = tostring(tgt)
    if type(tgt) == 'table' or type(tgt) == 'userdata' then
        ts = (blips[tgt] and ('#' .. blips[tgt].n) or 'other') .. ' src=' .. tostring(B(function() return tgt:GetSource():GetEntityId() end))
            .. ' id=' .. tostring(B(function() return tgt:GetEntityId() end))
    end
    P('w pos=' .. tps .. ' tgt=' .. ts .. ' fired=' .. fired)
end

ForkThread(function()
    WaitTicks(1)
    hold(frig) hold(spark) hold(radar) hold(plane)
    P('start jam=' .. tostring(B(function() return frig:IsIntelEnabled('Jammer') end)) .. ',' .. tostring(B(function() return spark:IsIntelEnabled('Jammer') end))
        .. ',' .. tostring(B(function() return plane:IsIntelEnabled('Jammer') end)))
    while T() <= 340 do
        local t = T()
        local m25 = t - math.floor(t / 25) * 25
        if t >= 40 and t <= 240 and m25 == 15 then
            P('do off ' .. tostring(B(function() frig:DisableIntel('Jammer') plane:DisableIntel('Jammer') return true end)))
        elseif t >= 40 and t <= 245 and m25 == 20 then
            P('do on ' .. tostring(B(function() frig:EnableIntel('Jammer') plane:EnableIntel('Jammer') return true end)))
        end
        if t == 260 then
            local p = frig:GetPosition()
            Warp(frig, { p[1] + 40, p[2], p[3] })
            P('do warp')
        elseif t == 300 then
            spark:Destroy() plane:Destroy()
            P('do destroyed S P')
        end
        local m10 = t - math.floor(t / 10) * 10
        if m10 == 0 or (t >= 258 and t <= 266) or (t >= 298 and t <= 306) then sample() end
        if t - math.floor(t / 5) * 5 == 0 then weapon() end
        WaitTicks(1)
    end
    WaitTicks(30)
    P('done-data')
    WaitTicks(30)
    LOG('PROBE done')
end)
