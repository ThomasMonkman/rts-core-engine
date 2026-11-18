-- Differential probe: radar jamming (Jammer intel, blueprint Intel.JammerBlips /
-- Intel.JamRadius) as an enemy radar sees it. Third version: does artillery
-- shoot a jammer's fake blips; what omni does to them; the threat they add.
-- (probe_v1.lua: count, placement, movement, Lua objects, vision, disable;
-- probe_v2.lua: placement statistics, warp, the jammer dying.)
--   real game (Windows): dev/probes/run_faf.ps1 probe_intel_jam 150
--   ours: --headless --no-ai --ticks 600 --script dev/probes/maps/probe_intel_jam/probe.lua
-- Army 1 observes, army 2 jams (hold fire). Seton's Clutch:
--   F  UEF frigate ues0103 (5 blips, JamRadius 70) of army 2 at (530, 210)
--   R  army 1's T3 radar ueb3104 at (800, 100): radar 600 covers F and its
--      fakes, its omni 200 doesn't (291 - 70 away)
--   A  army 1's T2 artillery ueb2303 (range 50..115, vision 28) at
--      (656, 240): F 129.5 away (out of range), fakes up to 70 nearer
-- t=120, 220: F's jammer off for 5 ticks (new fakes); t=300 an army 1 T3
-- radar (omni 200) at (656, 180), 129.5 from F: omni over F and all its
-- fakes; destroyed at t=340; observed to t=540.
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
local function isobj(v) return type(v) == 'table' or type(v) == 'userdata' end

local frig = make('ues0103', OWN, 530, 210, GetSurfaceHeight(530, 210))
local radar = make('ueb3104', OBS, 800, 100)
local arty = make('ueb2303', OBS, 656, 240, nil, true)
local FID = frig:GetEntityId()

local blips, order, seq = {}, {}, 0
local function rel(p)
    local jp = B(function() return frig:GetPosition() end)
    if not isobj(jp) then return 'jammer gone' end
    local dx, dz = p[1] - jp[1], p[3] - jp[3]
    local ax, az = p[1] - 656, p[3] - 240
    return 'dx=' .. N(dx) .. ' dz=' .. N(dz) .. ' r=' .. N(math.sqrt(dx * dx + dz * dz)) .. ' fromArty=' .. N(math.sqrt(ax * ax + az * az))
end
local orig = brain.OnIntelChange
brain.OnIntelChange = function(self, blip, reconType, val)
    local okS, src = pcall(function() return blip:GetSource() end)
    if okS and src and B(function() return src:GetEntityId() end) == FID then
        local b = blips[blip]
        if not b then
            seq = seq + 1
            b = { n = seq }
            blips[blip] = b
            table.insert(order, blip)
        end
        local okp, p = pcall(function() return blip:GetPosition() end)
        P('ic #' .. b.n .. ' ' .. tostring(reconType) .. '=' .. tostring(val) .. ' ' .. (okp and p and rel(p) or 'nopos'))
    end
    if orig then return orig(self, blip, reconType, val) end
end

local function flags(b)
    if B(function() return b:BeenDestroyed() end) == true then return 'D' end
    return flag(B(function() return b:IsOnRadar(OBS) end), 'R') .. flag(B(function() return b:IsSeenNow(OBS) end), 'S')
        .. flag(B(function() return b:IsSeenEver(OBS) end), 'E') .. flag(B(function() return b:IsOnOmni(OBS) end), 'O')
        .. flag(B(function() return b:IsKnownFake(OBS) end), 'F')
end
local function thr(ring)
    local p = frig:GetPosition()
    local ok, v = pcall(brain.GetThreatAtPosition, brain, p, ring, true, 'Overall')
    return ok and N(v) or 'ERR'
end
local function sample(full)
    local line, n = '', 0
    for _, b in order do
        local fl = flags(b)
        if fl ~= 'D' then
            n = n + 1
            if full then
                local okp, p = pcall(function() return b:GetPosition() end)
                line = line .. ' [#' .. blips[b].n .. ' ' .. fl .. (full == 2 and (' ' .. (okp and p and rel(p) or 'nopos')) or '') .. ']'
            end
        end
    end
    P('s live=' .. n .. ' thr0=' .. thr(0) .. ' thr1=' .. thr(1) .. line)
end

local fired = 0
local w = B(function() return arty:GetWeapon(1) end)
local function target(tgt)
    if not isobj(tgt) then return tostring(tgt) end
    return (blips[tgt] and ('#' .. blips[tgt].n) or 'other') .. ' src=' .. tostring(B(function() return tgt:GetSource():GetEntityId() end))
end
if isobj(w) then
    local of = w.OnWeaponFired
    w.OnWeaponFired = function(self, a, b, c)
        fired = fired + 1
        local tp = B(function() return self:GetCurrentTargetPos() end)
        P('fired ' .. fired .. ' tgt=' .. target(B(function() return self:GetCurrentTarget() end)) .. ' at=' .. (isobj(tp) and rel(tp) or tostring(tp)))
        if of then return of(self, a, b, c) end
    end
end
local last = ''
local function weapon()
    if not isobj(w) then return end
    local tp = B(function() return w:GetCurrentTargetPos() end)
    local s = 'w tgt=' .. target(B(function() return w:GetCurrentTarget() end)) .. ' at=' .. (isobj(tp) and rel(tp) or tostring(tp))
    if s ~= last then P(s) last = s end
end

ForkThread(function()
    WaitTicks(1)
    hold(frig) hold(radar)
    local omni
    while T() <= 540 do
        local t = T()
        if t == 120 or t == 220 then
            P('do off ' .. tostring(B(function() frig:DisableIntel('Jammer') return true end)))
        elseif t == 125 or t == 225 then
            P('do on ' .. tostring(B(function() frig:EnableIntel('Jammer') return true end)))
        elseif t == 300 then
            omni = make('ueb3104', OBS, 656, 180)
            P('do omni')
        elseif t == 340 then
            if omni and not omni:BeenDestroyed() then omni:Destroy() end
            P('do omni gone')
        end
        local m10 = t - math.floor(t / 10) * 10
        if m10 == 0 then sample((t == 40 or t == 140 or t == 240 or t == 320) and 2 or 1) end
        weapon()
        WaitTicks(1)
    end
    WaitTicks(30)
    P('done-data')
    WaitTicks(30)
    LOG('PROBE done')
end)
