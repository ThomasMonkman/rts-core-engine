-- Differential probe: capture interrupted by IssueClearCommands (is the
-- progress kept when the capture is re-issued?) and SetCapturable(false)
-- mid-capture. Header (hooks, storage, helpers) copied from probe_unit_build.
--   real game: run_faf.ps1 probe_unit_capture2 120
--   ours: --headless --no-ai --ticks 700 --script dev/probes/maps/probe_unit_capture2/probe.lua
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
local function line(name, e, prey)
    P(name, fmt('work %.3f eng [%s] q %d prey %s [%s] spent %s', e:GetWorkProgress(), states(e), table.getn(e:GetCommandQueue()),
        prey:BeenDestroyed() and 'gone' or 'alive', prey:BeenDestroyed() and '-' or states(prey), spent()))
end

ForkThread(function()
    WaitTicks(5)
    -- CI: capture, IssueClearCommands 10 ticks into it, re-issue after 5
    begin('CI')
    local x, z = SITES.G7[1], SITES.G7[2]
    local e = engineer('engI', x, z + 4, 20)
    local prey = watch(make('ueb1101', CIV, x, z), 'preyI')
    WaitTicks(2)
    mark()
    IssueCapture({ e }, prey)
    local n = waitState(e, 'Capturing', 40)
    P('preyI', 'capturing after ' .. n)
    wait(10)
    line('preyI', e, prey)
    IssueClearCommands({ e })
    P('preyI', 'cleared')
    for i = 1, 5 do
        wait(1)
        line('preyI', e, prey)
    end
    IssueCapture({ e }, prey)
    P('preyI', 'reissued')
    local t1 = GetGameTick()
    wait(120, function(i)
        if prey:BeenDestroyed() then return true end
        if i <= 12 or math.mod(i, 5) == 0 then line('preyI', e, prey) end
    end)
    P('preyI', fmt('reissue to done %d destroyed %s spent %s', GetGameTick() - t1, tostring(prey:BeenDestroyed()), spent()))
    wait(5)

    -- CU: SetCapturable(false) 10 ticks into a capture
    begin('CU')
    x = x + 40
    e = engineer('engU', x, z + 4, 20)
    prey = watch(make('ueb1101', CIV, x, z), 'preyU')
    WaitTicks(2)
    mark()
    IssueCapture({ e }, prey)
    n = waitState(e, 'Capturing', 40)
    P('preyU', 'capturing after ' .. n)
    wait(10)
    line('preyU', e, prey)
    local ok, err = pcall(function() prey:SetCapturable(false) end)
    P('preyU', 'SetCapturable(false) ' .. tostring(ok) .. ' ' .. tostring(err))
    for i = 1, 10 do
        wait(1)
        line('preyU', e, prey)
    end
    wait(40, function(i)
        if math.mod(i, 10) == 0 then line('preyU', e, prey) end
        return prey:BeenDestroyed()
    end)
    P('preyU', fmt('end destroyed %s spent %s', tostring(prey:BeenDestroyed()), spent()))
    WaitTicks(30)
    LOG('PROBE done')
end)
