-- Differential probe: radar jamming (Jammer intel, blueprint Intel.JammerBlips /
-- Intel.JamRadius) as an enemy radar sees it.
--   real game (Windows): dev/probes/run_faf.ps1 probe_intel_jam 150
--   ours: --headless --no-ai --ticks 560 --script dev/probes/maps/probe_intel_jam/probe.lua
-- Army 1 observes, army 2 jams (everything on hold fire except army 1's
-- artillery). Seton's Clutch:
--   F  a UEF frigate ues0103 (5 blips, JamRadius 70) of army 2 on water at
--      (500, 200);
--   S  a UEF Sparky xel0209 (8 blips, JamRadius 18) of army 2 on land at
--      (800, 330); moves 30 east at t=90; Jammer disabled at t=150 and
--      enabled again at t=200;
--   R  army 1's T3 radar ueb3104 at (800, 100): radar 600 covers both, its
--      omni (200) neither (F and its fakes 316 +- 70 away, S 230 - 18);
--   A  army 1's T2 artillery ueb2303 (range 115, vision 28) at (656, 272):
--      the frigate 172 away (out of range), its fakes 102..242;
--   t=250 an army 1 air scout parks over the frigate (vision), destroyed at
--      270; an army 1 T3 radar (omni 200) at (850, 420) next to S, destroyed
--      at 300. Observed to t=520 (FAF resets a jammer 15 s after it was last
--      seen: JammerManagerBrainComponent).
-- Army 1's brain OnIntelChange is wrapped (then calls FAF's) to log every
-- intel change; blips whose source is a jammer are kept and sampled.
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
-- Intel costs energy: keep both armies topped up.
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
local radar = make('ueb3104', OBS, 800, 100)
local arty = make('ueb2303', OBS, 656, 272, nil, true)
local JAM = { [frig:GetEntityId()] = 'F', [spark:GetEntityId()] = 'S' }
local JU = { F = frig, S = spark }
P('ids frig=' .. frig:GetEntityId() .. ' spark=' .. spark:GetEntityId() .. ' radar=' .. radar:GetEntityId() .. ' arty=' .. arty:GetEntityId())

-- Intel changes army 1 hears.
local fakes = {}      -- key -> { blip, site, first tick, key }
local fakeOrder = {}
local calls, logged = 0, 0
local orig = brain.OnIntelChange
local function keyOf(blip)
    local ok, id = pcall(function() return blip:GetEntityId() end)
    return ok and tostring(id) or tostring(blip)
end
brain.OnIntelChange = function(self, blip, reconType, val)
    calls = calls + 1
    local okS, src = pcall(function() return blip:GetSource() end)
    local sid = okS and src and B(function() return src:GetEntityId() end) or '-'
    local site = JAM[sid] or '-'
    local key = keyOf(blip)
    if logged < 300 then
        logged = logged + 1
        local okp, p = pcall(function() return blip:GetPosition() end)
        local okb, bp = pcall(function() return blip:GetBlueprint().BlueprintId end)
        local pos = okp and p and (N(p[1]) .. ',' .. N(p[3])) or '?'
        local rel = ''
        if JU[site] and okp and p then
            local jp = JU[site]:GetPosition()
            rel = ' d=' .. N(math.sqrt((p[1] - jp[1]) * (p[1] - jp[1]) + (p[3] - jp[3]) * (p[3] - jp[3])))
        end
        P('ic ' .. site .. ' key=' .. key .. ' src=' .. tostring(sid) .. ' bp=' .. tostring(okb and bp) .. ' ' .. tostring(reconType) .. '=' .. tostring(val) .. ' pos=' .. pos .. rel
            .. ' same=' .. tostring(okS and src ~= nil and rawequal(blip, src)))
    end
    if site ~= '-' and not fakes[key] then
        fakes[key] = { blip = blip, site = site, t = T(), key = key }
        table.insert(fakeOrder, key)
    end
    if orig then return orig(self, blip, reconType, val) end
end

-- A blip's flags for army 1: R radar, S seen now, E seen ever, O omni, F known fake, M maybe dead, D destroyed.
local function flags(b)
    if B(function() return b:BeenDestroyed() end) == true then return 'D' end
    return flag(B(function() return b:IsOnRadar(OBS) end), 'R') .. flag(B(function() return b:IsSeenNow(OBS) end), 'S')
        .. flag(B(function() return b:IsSeenEver(OBS) end), 'E') .. flag(B(function() return b:IsOnOmni(OBS) end), 'O')
        .. flag(B(function() return b:IsKnownFake(OBS) end), 'F') .. flag(B(function() return b:IsMaybeDead(OBS) end), 'M')
end

local function thr(x, z, ring)
    local ok, v = pcall(brain.GetThreatAtPosition, brain, { x, GetSurfaceHeight(x, z), z }, ring or 0, true, 'Overall')
    return ok and N(v) or 'ERR'
end

local function sample(full)
    for _, site in { 'F', 'S' } do
        local j = JU[site]
        local jp = j:GetPosition()
        local line = site .. ' at=' .. N(jp[1]) .. ',' .. N(jp[3]) .. ' jam=' .. tostring(B(function() return j:IsIntelEnabled('Jammer') end))
        local real = B(function() return j:GetBlip(OBS) end)
        if type(real) == 'table' or type(real) == 'userdata' then
            line = line .. ' own=' .. keyOf(real) .. ':' .. flags(real)
        else
            line = line .. ' own=' .. tostring(real)
        end
        line = line .. ' thr0=' .. thr(jp[1], jp[3], 0) .. ' thr1=' .. thr(jp[1], jp[3], 1)
        -- Fakes known from OnIntelChange.
        local n, s = 0, ''
        for _, k in fakeOrder do
            local f = fakes[k]
            if f.site == site then
                local fl = flags(f.blip)
                if fl ~= 'D' then n = n + 1 end
                if full then
                    local okp, p = pcall(function() return f.blip:GetPosition() end)
                    local where = '?'
                    if okp and p then
                        where = N(p[1] - jp[1]) .. ',' .. N(p[3] - jp[3]) .. ' r' .. N(math.sqrt((p[1] - jp[1]) * (p[1] - jp[1]) + (p[3] - jp[3]) * (p[3] - jp[3])))
                    end
                    s = s .. ' [' .. k .. ' ' .. fl .. ' ' .. where .. ']'
                end
            end
        end
        line = line .. ' live=' .. n .. s
        -- What GetUnitsAroundPoint lists for the enemy around the jammer.
        local ok, us = pcall(brain.GetUnitsAroundPoint, brain, categories.ALLUNITS, jp, 100, 'Enemy')
        if ok then
            local l = ''
            for _, u in us or {} do
                local okp, p = pcall(function() return u:GetPosition() end)
                l = l .. ' ' .. tostring(B(function() return u:GetEntityId() end)) .. '@' .. (okp and p and (N(p[1] - jp[1]) .. ',' .. N(p[3] - jp[3])) or '?')
            end
            line = line .. ' gua=' .. table.getn(us or {}) .. l
        else
            line = line .. ' gua=ERR'
        end
        P('s ' .. line)
    end
end

local function weapon()
    local w = B(function() return arty:GetWeapon(1) end)
    if w == 'E' or not w then P('w none') return end
    local tp = B(function() return w:GetCurrentTargetPos() end)
    local tps = (type(tp) == 'table' or type(tp) == 'userdata') and (N(tp[1]) .. ',' .. N(tp[3])) or tostring(tp)
    local tgt = B(function() return w:GetCurrentTarget() end)
    local ts = tostring(tgt)
    if type(tgt) == 'table' or type(tgt) == 'userdata' then
        ts = keyOf(tgt) .. ' src=' .. tostring(B(function() return tgt:GetSource():GetEntityId() end))
    end
    local fp = frig:GetPosition()
    P('w pos=' .. tps .. ' tgt=' .. ts .. ' frig=' .. N(fp[1]) .. ',' .. N(fp[3]))
end

ForkThread(function()
    WaitTicks(1)
    hold(frig) hold(spark) hold(radar)
    P('start jamF=' .. tostring(B(function() return frig:IsIntelEnabled('Jammer') end)) .. ' jamS=' .. tostring(B(function() return spark:IsIntelEnabled('Jammer') end))
        .. ' radar=' .. tostring(B(function() return radar:IsIntelEnabled('Radar') end)) .. ' omni=' .. tostring(B(function() return radar:IsIntelEnabled('Omni') end)))
    local scout, omni
    while T() <= 520 do
        local t = T()
        if t == 90 then
            IssueMove({ spark }, { 830, GetTerrainHeight(830, 330), 330 })
        elseif t == 150 then
            P('do disable ' .. tostring(B(function() spark:DisableIntel('Jammer') return true end)))
        elseif t == 200 then
            P('do enable ' .. tostring(B(function() spark:EnableIntel('Jammer') return true end)))
        elseif t == 250 then
            scout = make('uea0101', OBS, 500, 200, GetSurfaceHeight(500, 200) + 20)
            IssueMove({ scout }, { 500, 0, 200 })
            omni = make('ueb3104', OBS, 850, 420)
            P('do vision+omni')
        elseif t == 270 then
            if scout and not scout:BeenDestroyed() then scout:Destroy() end
            P('do scout gone')
        elseif t == 300 then
            if omni and not omni:BeenDestroyed() then omni:Destroy() end
            P('do omni gone')
        end
        local m = t - math.floor(t / 10) * 10
        if t <= 120 or m == 0 then
            sample(t <= 20 or m == 0)
        end
        if m == 0 then weapon() P('calls ' .. calls) end
        WaitTicks(1)
    end
    WaitTicks(30)
    P('done-data')
    WaitTicks(30)
    LOG('PROBE done')
end)
