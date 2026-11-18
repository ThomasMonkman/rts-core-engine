-- Differential probe: construction, upgrades, reclaim, repair, capture and
-- sacrifice, in a format both engines print identically ("PROBE ..." lines).
--   real game (Windows): dev/probes/run_faf.ps1 probe_unit_build 200
--   ours: cargo run -- --headless --no-ai --ticks 1500 --script dev/probes/maps/probe_unit_build/probe.lua
-- Groups run one after another in army 1 (no income: storage buildings
-- filled at the start of each group, so the stored change is the cost).
-- G1 build: an engineer (build rate 20) builds a T1 generator, then one
--    right beside it; two more are made side by side with CreateUnitHPR.
-- G2 cancel: the same, IssueClearCommands at +20; the half-built generator
--    is watched for 60 ticks, repaired for 10, then reclaimed.
-- G3 factory: IssueBuildFactory x3 engineers (rate 100), then a queue cleared.
-- G4 upgrade: radar T1 -> T2 (rate 100); one finished, one cancelled, one
--    whose old structure is destroyed.
-- G5 reclaim: a prop (SetMaxReclaimValues 10 s, 40 mass, 100 energy), a
--    finished tank, a damaged tank.
-- G6 repair: a generator at 100 hp.
-- G7 capture: a civilian generator, then one by two engineers.
-- G8 sacrifice: an Aeon engineer on a half-built generator.
-- G9 IssueKillSelf / IssueDestroySelf / IssuePause.
-- Lines: PROBE <tick> <tag> <event> ...; ticks count from the group's start.
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

ForkThread(function()
    WaitTicks(5)
    -- G1
    begin('G1')
    local x, z = SITES.G1[1], SITES.G1[2]
    local e = engineer('eng', x, z + 4, 20)
    P('eng', fmt('build rate %.2f', e:GetBuildRate()))
    IssueBuildMobile({ e }, pos(x, z), 'ueb1101', {})
    P('eng', 'ordered')
    local built
    wait(90, function(i)
        local f = e:GetFocusUnit()
        if f and not built then
            built = f
            tagOf[f] = 'built'
            P('built', fmt('first seen %s pos %.2f %.2f cons %.3f %.3f', info(f), f:GetPosition()[1], f:GetPosition()[3], e:GetConsumptionPerSecondMass(), e:GetConsumptionPerSecondEnergy()))
        end
        if built and (i <= 12 or math.mod(i, 10) == 0) then P('built', fmt('%s work %.4f spent %s', info(built), e:GetWorkProgress(), spent())) end
        return built and not built:BeenDestroyed() and built:GetFractionComplete() >= 1 and e:IsIdleState()
    end)
    P('built', fmt('end %s spent %s', info(built), spent()))
    -- a second one right beside the first (adjacency callbacks), then both destroyed
    if built and not built:BeenDestroyed() then
        IssueBuildMobile({ e }, pos(x + 2, z), 'ueb1101', {})
        local second
        local n = wait(90, function(i)
            local f = e:GetFocusUnit()
            if f and not second then second = f tagOf[f] = 'second' end
            return second and not second:BeenDestroyed() and second:GetFractionComplete() >= 1 and e:IsIdleState()
        end)
        P('second', fmt('end after %d %s', n, info(second)))
        if second and not second:BeenDestroyed() then
            second:Destroy()
            wait(2)
        end
        built:Destroy()
    end
    local c1, c2 = make('ueb1101', A1, x + 20, z), make('ueb1101', A1, x + 22, z)
    tagOf[c1] = 'made1' tagOf[c2] = 'made2'
    wait(3)
    c1:Destroy() c2:Destroy()
    wait(2)
    -- G2
    begin('G2')
    x, z = SITES.G2[1], SITES.G2[2]
    e = engineer('eng2', x, z + 4, 20)
    IssueBuildMobile({ e }, pos(x, z), 'ueb1101', {})
    local half
    wait(25, function(i)
        local f = e:GetFocusUnit()
        if f and not half then half = f tagOf[f] = 'half' end
        return false
    end)
    P('half', fmt('before clear %s spent %s', info(half), spent()))
    IssueClearCommands({ e })
    wait(60, function(i)
        if i <= 5 or math.mod(i, 10) == 0 then P('half', fmt('idle +%d %s spent %s', i, info(half), spent())) end
    end)
    if half and not half:BeenDestroyed() then
        IssueRepair({ e }, half)
        wait(12, function(i) if i > 2 then P('half', fmt('repair +%d %s spent %s', i, info(half), spent())) end end)
        IssueStop({ e })
        wait(3)
        P('half', fmt('stopped %s spent %s', info(half), spent()))
        mark()
        IssueReclaim({ e }, half)
        wait(60, function(i)
            if i <= 6 or math.mod(i, 5) == 0 then P('half', fmt('reclaim +%d %s gained %s work %.3f', i, info(half), spent(), e:GetWorkProgress())) end
            return half:BeenDestroyed()
        end)
        P('half', fmt('reclaim end gained %s', spent()))
    end
    -- G3
    begin('G3')
    x, z = SITES.G3[1], SITES.G3[2]
    local fac = watch(make('ueb0101', A1, x, z), 'fac')
    fac:SetBuildRate(100)
    WaitTicks(5)
    P('fac', 'rally ' .. S(fac:GetRallyPoint() and fac:GetRallyPoint()[1]) .. ' ' .. S(fac:GetRallyPoint() and fac:GetRallyPoint()[3]))
    IssueBuildFactory({ fac }, 'uel0201', 3)
    local q = fac:GetCommandQueue()
    P('fac', fmt('queued %d', table.getn(q)))
    for i, c in q do
        local keys = {}
        for k, v in c do table.insert(keys, tostring(k) .. '=' .. (type(v) == 'table' and 'table' or S(v))) end
        table.sort(keys)
        P('fac', 'cmd ' .. i .. ' ' .. table.concat(keys, ','))
    end
    local made = 0
    wait(80, function(i)
        local n = brain:GetCurrentUnits(categories.uel0201)
        if n ~= made then
            made = n
            P('fac', fmt('tanks %d spent %s', n, spent()))
        end
    end)
    for i, t in brain:GetListOfUnits(categories.uel0201, false) do
        local p = t:GetPosition()
        P('fac', fmt('tank %d at %.1f %.1f (factory %d %d) queue %d', i, p[1], p[3], x, z, table.getn(t:GetCommandQueue())))
    end
    IssueFactoryRallyPoint({ fac }, pos(x + 12, z + 12))
    P('fac', 'rally set ' .. S(fac:GetRallyPoint() and fac:GetRallyPoint()[1]) .. ' ' .. S(fac:GetRallyPoint() and fac:GetRallyPoint()[3]))
    IssueBuildFactory({ fac }, 'uel0201', 2)
    IssueBuildFactory({ fac }, 'uel0105', 1)
    P('fac', fmt('queued %d', table.getn(fac:GetCommandQueue())))
    wait(5)
    mark()
    IssueClearFactoryCommands({ fac })
    P('fac', fmt('after ClearFactoryCommands queue %d focus %s', table.getn(fac:GetCommandQueue()), tag(fac:GetFocusUnit())))
    wait(10)
    P('fac', fmt('refund %s tanks %d', spent(), brain:GetCurrentUnits(categories.uel0201)))
    IssueBuildFactory({ fac }, 'uel0201', 1)
    wait(5)
    mark()
    IssueClearCommands({ fac })
    P('fac', fmt('after ClearCommands queue %d focus %s', table.getn(fac:GetCommandQueue()), tag(fac:GetFocusUnit())))
    wait(10)
    P('fac', fmt('refund %s tanks %d', spent(), brain:GetCurrentUnits(categories.uel0201)))
    IssueBuildFactory({ fac }, 'uel0201', 1)
    wait(5)
    IssueStop({ fac })
    P('fac', fmt('after Stop queue %d focus %s', table.getn(fac:GetCommandQueue()), tag(fac:GetFocusUnit())))
    wait(10)
    P('fac', fmt('tanks %d', brain:GetCurrentUnits(categories.uel0201)))
    for _, t in brain:GetListOfUnits(categories.uel0201, false) do t:Destroy() end
    fac:Destroy()
    -- G4
    begin('G4')
    x, z = SITES.G4[1], SITES.G4[2]
    local function upgrade(name, px, pz, at, action)
        local r = watch(make('ueb3101', A1, px, pz), name)
        r:SetBuildRate(100)
        WaitTicks(2)
        mark()
        IssueUpgrade({ r }, 'ueb3201')
        P(name, 'ordered')
        local new
        wait(110, function(i)
            local f = not r:BeenDestroyed() and r:GetFocusUnit()
            if f and not new then
                new = f
                watch(f, name .. '-new')
                P(name, fmt('new %s same pos %s cons %.3f %.3f', info(f), tostring(VDist3(f:GetPosition(), r:GetPosition()) < 0.01), r:GetConsumptionPerSecondMass(), r:GetConsumptionPerSecondEnergy()))
            end
            if new and (i <= 6 or math.mod(i, 20) == 0) then P(name, fmt('+%d new %s old %s work %.3f', i, info(new), info(r), r:BeenDestroyed() and -1 or r:GetWorkProgress())) end
            if i == at then
                P(name, fmt('action at +%d new %s spent %s', i, info(new), spent()))
                action(r, new)
            end
            if at and i >= at + 15 then return true end
            return r:BeenDestroyed() and new and not new:BeenDestroyed() and new:GetFractionComplete() >= 1
        end)
        P(name, fmt('end old %s new %s spent %s', info(r), info(new), spent()))
        return r, new
    end
    upgrade('up', x, z)
    upgrade('upcancel', x + 10, z, 30, function(r) IssueClearCommands({ r }) end)
    upgrade('upstop', x + 20, z, 30, function(r) IssueStop({ r }) end)
    upgrade('upkill', x + 30, z, 30, function(r) r:Destroy() end)
    upgrade('uppause', x + 40, z, 10, function(r) r:SetPaused(true) end)
    -- G5
    begin('G5')
    x, z = SITES.G5[1], SITES.G5[2]
    e = engineer('eng5', x, z, 5)
    local okp, prop = pcall(CreateProp, pos(x + 3, z), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
    if okp and prop then
        tagOf[prop] = 'prop'
        pcall(function() prop:SetMaxReclaimValues(10, 40, 100) end)
        P('prop', fmt('values mass %s energy %s time %s frac %s', S(prop.MaxMassReclaim), S(prop.MaxEnergyReclaim), S(prop.TimeReclaim), S(prop.ReclaimLeft)))
        local on = prop.OnReclaimed
        prop.OnReclaimed = function(self, a, b) P('prop', 'OnReclaimed ' .. S(a) .. ' ' .. S(b)) if on then return on(self, a, b) end end
        local od = prop.OnDestroy
        prop.OnDestroy = function(self, a, b) P('prop', 'OnDestroy') if od then return od(self, a, b) end end
        IssueReclaim({ e }, prop)
        wait(140, function(i)
            if i <= 6 or math.mod(i, 10) == 0 then P('prop', fmt('+%d gained %s work %.3f left %s', i, spent(), e:GetWorkProgress(), S(not prop:BeenDestroyed() and prop.ReclaimLeft))) end
            return prop:BeenDestroyed() and e:IsIdleState()
        end)
        P('prop', 'end gained ' .. spent())
    else
        P('prop', 'CreateProp failed ' .. string.sub(tostring(prop), 1, 80))
    end
    for _, case in { { 'victim', 1 }, { 'victimhurt', 0.5 } } do
        mark()
        local v = watch(make('uel0201', A1, x + 3, z + 3), case[1])
        v:SetHealth(nil, v:GetMaxHealth() * case[2])
        IssueReclaim({ e }, v)
        wait(80, function(i)
            if i <= 6 or math.mod(i, 10) == 0 then P(case[1], fmt('+%d %s gained %s work %.3f', i, info(v), spent(), e:GetWorkProgress())) end
            return v:BeenDestroyed() and e:IsIdleState()
        end)
        P(case[1], 'end gained ' .. spent())
    end
    -- reclaim refused: SetReclaimable(false) mid-way
    local nv = watch(make('uel0201', A1, x + 3, z + 3), 'noreclaim')
    IssueReclaim({ e }, nv)
    wait(8)
    nv:SetReclaimable(false)
    P('noreclaim', 'SetReclaimable(false) ' .. info(nv))
    wait(8)
    P('noreclaim', 'after ' .. info(nv))
    nv:Destroy()
    -- G6
    begin('G6')
    x, z = SITES.G6[1], SITES.G6[2]
    e = engineer('eng6', x, z + 4, 5)
    local hurt = watch(make('ueb1101', A1, x, z), 'hurt')
    hurt:SetHealth(nil, 100)
    WaitTicks(2)
    mark()
    IssueRepair({ e }, hurt)
    wait(260, function(i)
        if i <= 8 or math.mod(i, 20) == 0 then P('hurt', fmt('+%d %s spent %s cons %.3f %.3f', i, info(hurt), spent(), e:GetConsumptionPerSecondMass(), e:GetConsumptionPerSecondEnergy())) end
        return hurt:GetHealth() >= hurt:GetMaxHealth() and e:IsIdleState()
    end)
    P('hurt', fmt('end %s spent %s', info(hurt), spent()))
    -- G7
    begin('G7')
    x, z = SITES.G7[1], SITES.G7[2]
    e = engineer('eng7', x, z + 4, 20)
    local prey = watch(make('ueb1101', CIV, x, z), 'prey')
    prey:SetHealth(nil, 300)
    local preyId = prey:GetEntityId()
    WaitTicks(2)
    mark()
    IssueCapture({ e }, prey)
    wait(200, function(i)
        if i <= 8 or math.mod(i, 10) == 0 then P('prey', fmt('+%d work %.3f spent %s cons %.3f %.3f', i, e:GetWorkProgress(), spent(), e:GetConsumptionPerSecondMass(), e:GetConsumptionPerSecondEnergy())) end
        return prey:BeenDestroyed() and e:IsIdleState()
    end)
    P('prey', fmt('end old destroyed %s spent %s', tostring(prey:BeenDestroyed()), spent()))
    for _, u in brain:GetUnitsAroundPoint(categories.ueb1101, pos(x, z), 3, 'Ally') or {} do
        P('prey', fmt('now army %d hp %.1f same id %s same table %s', u:GetArmy(), u:GetHealth(), tostring(u:GetEntityId() == preyId), tostring(u == prey)))
    end
    local ea, eb = engineer('eng7a', x + 20, z + 4, 20), engineer('eng7b', x + 24, z + 4, 20)
    local prey2 = watch(make('ueb1101', CIV, x + 22, z), 'prey2')
    WaitTicks(2)
    mark()
    IssueCapture({ ea, eb }, prey2)
    local n = wait(200, function(i) return prey2:BeenDestroyed() end)
    P('prey2', fmt('two captors done after %d spent %s', n, spent()))
    -- interrupted capture: progress kept?
    local ec = engineer('eng7c', x + 40, z + 4, 20)
    local prey3 = watch(make('ueb1101', CIV, x + 40, z), 'prey3')
    WaitTicks(2)
    IssueCapture({ ec }, prey3)
    wait(30)
    P('prey3', fmt('before stop work %.3f', ec:GetWorkProgress()))
    IssueStop({ ec })
    wait(5)
    if not prey3:BeenDestroyed() then IssueCapture({ ec }, prey3) end
    wait(6)
    P('prey3', fmt('resumed +6 work %.3f', ec:GetWorkProgress()))
    pcall(function() prey3:SetCapturable(false) end)
    wait(5)
    P('prey3', fmt('uncapturable +5 work %.3f destroyed %s', ec:GetWorkProgress(), tostring(prey3:BeenDestroyed())))
    -- G8
    begin('G8')
    x, z = SITES.G8[1], SITES.G8[2]
    e = engineer('eng8', x, z + 4, 20)
    IssueBuildMobile({ e }, pos(x, z), 'ueb1101', {})
    local target
    wait(20, function() target = target or e:GetFocusUnit() end)
    IssueClearCommands({ e })
    if target then
        tagOf[target] = 'sactarget'
        local aeon = engineer('aeon', x + 4, z + 4, nil, 'ual0105')
        P('sactarget', 'before ' .. info(target) .. ' aeon hp ' .. S(aeon:GetHealth()))
        mark()
        IssueSacrifice({ aeon }, target)
        wait(40, function(i)
            if i <= 10 then P('sactarget', fmt('+%d %s aeon %s', i, info(target), aeon:BeenDestroyed() and 'gone' or S(aeon:GetHealth()))) end
        end)
        P('sactarget', fmt('end %s spent %s', info(target), spent()))
    end
    -- G9
    begin('G9')
    x, z = SITES.G9[1], SITES.G9[2]
    local ks = watch(make('uel0105', A1, x, z), 'killself')
    local ds = watch(make('uel0105', A1, x + 6, z), 'destroyself')
    local ps = watch(make('uel0105', A1, x + 12, z), 'pauser')
    IssueKillSelf({ ks })
    IssueDestroySelf({ ds })
    IssueBuildMobile({ ps }, pos(x + 12, z - 4), 'ueb1101', {})
    wait(15)
    IssuePause({ ps })
    P('pauser', 'IssuePause')
    wait(10)
    local f = ps:GetFocusUnit()
    P('pauser', 'paused ' .. tostring(ps:IsPaused()) .. ' ' .. info(f))
    wait(10)
    P('pauser', 'still ' .. info(f))
    ps:SetPaused(false)
    wait(60)
    WaitTicks(30)
    LOG('PROBE done')
end)
