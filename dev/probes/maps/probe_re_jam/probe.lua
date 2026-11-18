-- Differential probe (re-check of intel J9): why the previous generation of
-- a jammer's fakes keeps adding threat after a reset. Army 1 observes with a
-- T3 radar at (800,100); army 2 has a jamming frigate F at (530,210) and a
-- radar-only tank K at (664,320).
-- t=100,160,220: F's jammer off for 5 ticks (a new generation each time).
-- t=260: K destroyed (does a dead radar contact keep its threat?).
-- t=300: F warped 40 along +z (do stale threats follow the fakes?).
-- t=360: F destroyed.
-- Every 5 ticks: live fake count, threat at F's start (ring 0, 1), at K,
-- and at each generation's fake spots (count of spots with threat > 0).
--   real game: dev/probes/run_faf.ps1 probe_re_jam 90
--   ours: --headless --no-ai --ticks 700 --script dev/probes/maps/probe_re_jam/probe.lua
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
local function B(f) local ok, v = pcall(f) if not ok then return 'E' end return v end
local function make(bp, army, x, z, y)
    local u = CreateUnitHPR(bp, armies[army], x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
    if u.SetFireState then pcall(u.SetFireState, u, 1) end
    return u
end
local FX, FZ = 530, 210
local frig = make('ues0103', OWN, FX, FZ, GetSurfaceHeight(FX, FZ))
local radar = make('ueb3104', OBS, 800, 100)
local KX, KZ = 664, 320
local tank = make('uel0201', OWN, KX, KZ)
local FID = frig:GetEntityId()

local gens, cur = {}, nil
local known = {}
local blist = {}
local function newgen() cur = { spots = {}, n = table.getn(gens) + 1 } table.insert(gens, cur) end
newgen()
local orig = brain.OnIntelChange
brain.OnIntelChange = function(self, blip, reconType, val)
    local okS, src = pcall(function() return blip:GetSource() end)
    if okS and src and B(function() return src:GetEntityId() end) == FID and reconType == 'Radar' and val and not known[blip] then
        known[blip] = true
        table.insert(blist, blip)
        local okp, p = pcall(function() return blip:GetPosition() end)
        if okp and p then
            local dx, dz = p[1] - FX, p[3] - FZ
            if math.abs(dx) > 0.01 or math.abs(dz) > 0.01 then
                table.insert(cur.spots, { p[1], p[2], p[3] })
            end
        end
    end
    if orig then return orig(self, blip, reconType, val) end
end
local function thr(x, z, ring)
    local ok, v = pcall(brain.GetThreatAtPosition, brain, { x, 0, z }, ring, true, 'Overall')
    return ok and v or -1
end
local function live()
    local n = 0
    for _, b in blist do if B(function() return b:BeenDestroyed() end) == false then n = n + 1 end end
    return n
end
local function sample()
    local line = fmt('s live=%d F0=%s F1=%s F3=%s F5=%s K0=%s', live(), N(thr(FX, FZ, 0)), N(thr(FX, FZ, 1)), N(thr(FX, FZ, 3)), N(thr(FX, FZ + 20, 5)), N(thr(KX, KZ, 0)))
    for _, g in gens do
        local hot = 0
        for _, s in g.spots do if thr(s[1], s[3], 0) > 0 then hot = hot + 1 end end
        line = line .. fmt(' g%d=%d/%d', g.n, hot, table.getn(g.spots))
    end
    P(line)
end

ForkThread(function()
    WaitTicks(1)
    while T() <= 440 do
        local t = T()
        if t == 100 or t == 160 or t == 220 then
            P('do off')
            pcall(function() frig:DisableIntel('Jammer') end)
        elseif t == 105 or t == 165 or t == 225 then
            newgen()
            P('do on gen ' .. cur.n)
            pcall(function() frig:EnableIntel('Jammer') end)
        elseif t == 260 then
            P('do kill tank') tank:Destroy()
        elseif t == 300 then
            P('do warp frig')
            if not frig:BeenDestroyed() then Warp(frig, { FX, GetSurfaceHeight(FX, FZ + 40), FZ + 40 }) end
        elseif t == 360 then
            P('do kill frig')
            if not frig:BeenDestroyed() then frig:Destroy() end
        end
        if math.mod(t, 5) == 0 then sample() end
        WaitTicks(1)
    end
    for _, g in gens do
        local line = 'gen ' .. g.n
        for _, s in g.spots do line = line .. fmt(' %.0f,%.0f', s[1] - FX, s[3] - FZ) end
        P(line)
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
