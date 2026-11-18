-- Differential probe: reclaim returns with room in storage (stored change
-- per tick), and SetReclaimable(false) once the reclaim is under way.
-- Header (hooks, storage, helpers) copied from probe_unit_build; storage is
-- filled, then half of it taken (TakeResource).
--   real game: run_faf.ps1 probe_unit_reclaim2 150
--   ours: --headless --no-ai --ticks 900 --script dev/probes/maps/probe_unit_reclaim2/probe.lua
-- Lines: PROBE <group> <tick> <tag> <event> ...; ticks count from the group's start.
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
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
local group = 'G0'
local tagOf = {}
local nextAnon = 0
local function tag(u)
    if u == nil then return 'nil' end
    if type(u) ~= 'table' then return tostring(u) end
    if tagOf[u] then return tagOf[u] end
    local ok, id = pcall(function() return u:GetBlueprint().BlueprintId end)
    if ok and id then
        nextAnon = nextAnon + 1
        tagOf[u] = string.gsub(tostring(id), '.*/', '') .. '#' .. nextAnon
        return tagOf[u]
    end
    return 'table'
end
local function S(x)
    local t = type(x)
    if t == 'number' then return fmt('%.3f', x) end
    if t == 'table' then return tag(x) end
    return tostring(x)
end
local function P(who, text) LOG(fmt('PROBE %s %d %s %s', group, GetGameTick() - t0, who, text)) end
local function make(bp, army, x, z)
    return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
end
local function hookClass(path, names)
    local ok, mod = pcall(import, path)
    local cls = ok and mod and mod.TypeClass
    if not cls then LOG('PROBE hook failed ' .. path) return end
    for _, name in names do
        local orig = cls[name]
        local n = name
        cls[n] = function(self, a, b, c, d)
            P(tag(self), n .. ' ' .. S(a) .. ' ' .. S(b) .. ' ' .. S(c) .. ' ' .. S(d))
            if orig then return orig(self, a, b, c, d) end
        end
    end
end
local BUILDER = { 'OnStartBuild', 'OnStopBuild', 'OnFailedToBuild', 'OnStartReclaim', 'OnStopReclaim', 'OnStartRepair', 'OnStopRepair',
    'OnStartCapture', 'OnStopCapture', 'OnFailedCapture', 'OnStartSacrifice', 'OnStopSacrifice', 'OnWorkBegin', 'OnWorkEnd', 'OnWorkFail',
    'OnKilled', 'OnDestroy', 'OnPaused', 'OnUnpaused', 'OnProductionPaused', 'OnProductionUnpaused' }
local BUILT = { 'OnCreate', 'OnStartBeingBuilt', 'OnStopBeingBuilt', 'OnFailedToBeBuilt', 'OnAdjacentTo', 'OnNotAdjacentTo', 'OnKilled', 'OnDestroy',
    'OnReclaimed', 'OnCaptured', 'OnDecayed', 'OnStartBeingCaptured', 'OnStopBeingCaptured', 'OnFailedBeingCaptured', 'OnGiven', 'OnStartBuild',
    'OnStopBuild', 'OnFailedToBuild', 'OnStartReclaim' }
hookClass('/units/uel0105/uel0105_script.lua', BUILDER)
hookClass('/units/ual0105/ual0105_script.lua', BUILDER)
hookClass('/units/ueb0101/ueb0101_script.lua', BUILDER)
hookClass('/units/ueb1101/ueb1101_script.lua', BUILT)
hookClass('/units/ueb3101/ueb3101_script.lua', BUILT)
hookClass('/units/ueb3201/ueb3201_script.lua', BUILT)
hookClass('/units/uel0201/uel0201_script.lua', { 'OnReclaimed', 'OnKilled', 'OnDestroy', 'OnStartBeingBuilt', 'OnStopBeingBuilt' })

local STATES = { 'Immobile', 'Moving', 'Attacking', 'Guarding', 'Building', 'Upgrading', 'Busy', 'BeingReclaimed', 'Repairing', 'Capturing',
    'BeingCaptured', 'Reclaiming', 'BeingUpgraded', 'Enhancing', 'BeingBuilt', 'NoReclaim', 'NoCost', 'BlockCommandQueue', 'PathFinding',
    'AssistMoving', 'GuardBusy', 'Patrolling' }
local function states(u)
    local on = {}
    for _, s in STATES do
        local ok, v = pcall(function() return u:IsUnitState(s) end)
        if ok and v then table.insert(on, s) end
    end
    return table.concat(on, '+')
end
local watched = {}
local function watch(u, name)
    tagOf[u] = name
    table.insert(watched, { u = u, last = '' })
    return u
end
local function poll()
    for _, w in watched do
        if not w.gone then
            local u = w.u
            local s
            if u:BeenDestroyed() then
                s = 'gone'
                w.gone = true
            else
                local f = u:GetFocusUnit()
                s = fmt('[%s] q %d focus %s idle %s paused %s', states(u), (function() local ok, q = pcall(u.GetCommandQueue, u) return ok and table.getn(q or {}) or -1 end)(), f and tag(f) or '-', tostring(u:IsIdleState()), tostring(u:IsPaused()))
            end
            if s ~= w.last then
                w.last = s
                P(tag(u), 'state ' .. s)
            end
        end
    end
end
local function wait(n, each)
    for i = 1, n do
        WaitTicks(1)
        poll()
        if each and each(i) then return i end
    end
    return n
end
local markM, markE = 0, 0
local function mark() markM, markE = brain:GetEconomyStored('MASS'), brain:GetEconomyStored('ENERGY') end
local function spent() return fmt('mass %.2f energy %.2f', markM - brain:GetEconomyStored('MASS'), markE - brain:GetEconomyStored('ENERGY')) end
local function info(u)
    if not u or u:BeenDestroyed() then return 'gone' end
    return fmt('frac %.4f hp %.2f', u:GetFractionComplete(), u:GetHealth())
end
-- Trees on a build site are reclaimed first (and take a different time in
-- each engine): every group's ground is cleared of props.
local function clearProps(site)
    local ok, list = pcall(GetReclaimablesInRect, Rect(site[1] - 16, site[2] - 16, site[1] + 60, site[2] + 16))
    if not ok or not list then return end
    for _, r in list do
        if IsProp(r) and not r:BeenDestroyed() then r:Destroy() end
    end
end
local function begin(name)
    watched = {}
    brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)
    WaitTicks(3)
    t0 = GetGameTick()
    group = name
    mark()
    P('-', fmt('begin stored %.1f %.1f', markM, markE))
end
local SITES = { G1 = { 784, 144 }, G2 = { 656, 160 }, G3 = { 736, 224 }, G4 = { 816, 224 }, G5 = { 912, 240 }, G6 = { 672, 304 }, G7 = { 800, 336 },
    G8 = { 960, 336 }, G9 = { 656, 384 }, STORE = { 608, 80 } }
for _, site in SITES do clearProps(site) end
for i = 0, 3 do
    make('ueb1105', A1, SITES.STORE[1] + 4 * i, SITES.STORE[2])
    make('ueb1106', A1, SITES.STORE[1] + 4 * i, SITES.STORE[2] + 6)
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function engineer(name, x, z, rate, bp)
    local e = watch(make(bp or 'uel0105', A1, x, z), name)
    if rate then e:SetBuildRate(rate) end
    return e
end
local function waitState(u, s, n)
    for i = 1, n do
        if u:BeenDestroyed() or u:IsUnitState(s) then return i end
        WaitTicks(1)
        poll()
    end
    return n
end
local function drain()
    local m, en = brain:GetEconomyStored('MASS'), brain:GetEconomyStored('ENERGY')
    local tm = brain:TakeResource('MASS', m / 2)
    local te = brain:TakeResource('ENERGY', en / 2)
    WaitTicks(2)
    P('-', fmt('drain had %.1f %.1f took %s %s now %.1f %.1f ratio %.3f %.3f', m, en, S(tm), S(te), brain:GetEconomyStored('MASS'),
        brain:GetEconomyStored('ENERGY'), brain:GetEconomyStoredRatio('MASS'), brain:GetEconomyStoredRatio('ENERGY')))
    mark()
end
local function gained()
    return fmt('mass %.3f energy %.3f', brain:GetEconomyStored('MASS') - markM, brain:GetEconomyStored('ENERGY') - markE)
end

ForkThread(function()
    WaitTicks(5)
    begin('R')
    local x, z = SITES.G5[1], SITES.G5[2]
    local e = engineer('eng5', x, z, 10)
    WaitTicks(5)
    drain()
    local okp, prop = pcall(CreateProp, pos(x + 3, z), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
    if okp and prop then
        tagOf[prop] = 'prop'
        pcall(function() prop:SetMaxReclaimValues(10, 40, 100) end)
        P('prop', fmt('values mass %s energy %s time %s frac %s', S(prop.MaxMassReclaim), S(prop.MaxEnergyReclaim), S(prop.TimeReclaim), S(prop.ReclaimLeft)))
        local on = prop.OnReclaimed
        prop.OnReclaimed = function(self, a, b) P('prop', 'OnReclaimed ' .. S(a) .. ' ' .. S(b) .. ' ' .. gained()) if on then return on(self, a, b) end end
        IssueClearCommands({ e })
        IssueReclaim({ e }, prop)
        wait(200, function(i)
            P('prop', fmt('+%d gained %s work %.3f left %s', i, gained(), e:GetWorkProgress(), S(not prop:BeenDestroyed() and prop.ReclaimLeft)))
            return prop:BeenDestroyed() and e:IsIdleState()
        end)
        WaitTicks(2)
        P('prop', 'end gained ' .. gained())
    else
        P('prop', 'CreateProp failed ' .. string.sub(tostring(prop), 1, 80))
    end
    for _, case in { { 'victim', 1 }, { 'victimhurt', 0.5 } } do
        drain()
        local v = watch(make('uel0201', A1, x + 3, z + 3), case[1])
        v:SetHealth(nil, v:GetMaxHealth() * case[2])
        IssueClearCommands({ e })
        IssueReclaim({ e }, v)
        local listed = false
        wait(100, function(i)
            P(case[1], fmt('+%d %s gained %s work %.3f', i, info(v), gained(), e:GetWorkProgress()))
            if v:BeenDestroyed() and not listed then
                listed = true
                local ok, list = pcall(GetReclaimablesInRect, Rect(x - 5, z - 5, x + 11, z + 11))
                local out = {}
                for _, r in (ok and list) or {} do
                    if not r:BeenDestroyed() then
                        local bp = r:GetBlueprint()
                        table.insert(out, fmt('%s:%s:m%s:e%s:t%s', IsProp(r) and 'prop' or 'unit', tostring(bp and bp.BlueprintId), S(r.MaxMassReclaim), S(r.MaxEnergyReclaim), S(r.TimeReclaim)))
                    end
                end
                P(case[1], 'reclaimables after ' .. table.concat(out, ' '))
            end
            return v:BeenDestroyed() and e:IsIdleState()
        end)
        WaitTicks(2)
        P(case[1], 'end gained ' .. gained())
    end
    -- reclaim refused: SetReclaimable(false) 5 ticks into the reclaim
    drain()
    local nv = watch(make('uel0201', A1, x + 3, z + 3), 'noreclaim')
    IssueClearCommands({ e })
    IssueReclaim({ e }, nv)
    local n = waitState(e, 'Reclaiming', 40)
    P('noreclaim', 'reclaiming after ' .. n)
    wait(5)
    P('noreclaim', 'before ' .. info(nv) .. ' gained ' .. gained())
    nv:SetReclaimable(false)
    P('noreclaim', 'SetReclaimable(false) ' .. info(nv))
    for i = 1, 10 do
        wait(1)
        P('noreclaim', fmt('+%d %s eng [%s] q %d gained %s', i, info(nv), states(e), table.getn(e:GetCommandQueue()), gained()))
    end
    wait(30)
    P('noreclaim', 'after ' .. info(nv) .. ' gained ' .. gained())
    if not nv:BeenDestroyed() then nv:Destroy() end
    WaitTicks(30)
    LOG('PROBE done')
end)
