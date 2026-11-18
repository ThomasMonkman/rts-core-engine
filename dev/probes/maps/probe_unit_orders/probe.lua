-- Differential probe: orders, the command queue and unit states, in a
-- format both engines print identically ("PROBE ..." lines).
--   real game (Windows): dev/probes/run_faf.ps1 probe_unit_orders 150
--   ours: cargo run -- --headless --no-ai --ticks 700 --script dev/probes/maps/probe_unit_orders/probe.lua
-- Cases run side by side at their own sites (army 1; targets are
-- NEUTRAL_CIVILIAN, enemy, hold fire, 30000 hp):
-- Q  every Issue* order given to a fresh unit: the queue right after and a
--    tick later (each entry's commandType and fields).
-- O1 move 20: states, navigator goal during, after arrival and after a clear.
-- O2 three queued moves, IssueClearCommands on the second leg; then IssueStop.
-- O3 patrol: one point, then two points: queue lengths over 250 ticks.
-- O4 guard: a tank guards a tank that moves, then the guarded tank dies.
-- O5 attack a structure 30 away: where it stops; SetDoNotTarget(true) on
--    the target at +120.
-- O6 teleport orders (a tank, a commander).
-- O7 commander enhancement by IssueScript (build rate 100).
-- O8 blockers: SetBlockCommandQueue / SetBusy / SetPaused / SetImmobile, then
--    IssueMove.
-- O9 transport: two tanks load onto a T1 transport and unload 40 away.
-- Lines: PROBE <tick> <tag> <event>; 'state' lines when the states, queue
-- length, focus or idle flag change.
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
local tagOf = {}
local function tag(u)
    if u == nil then return 'nil' end
    if type(u) ~= 'table' then return tostring(u) end
    if tagOf[u] then return tagOf[u] end
    local ok, id = pcall(function() return u:GetBlueprint().BlueprintId end)
    return ok and id and string.gsub(tostring(id), '.*/', '') or 'table'
end
local function S(x)
    local t = type(x)
    if t == 'number' then return fmt('%.2f', x) end
    if t == 'table' then return tag(x) end
    return tostring(x)
end
local function P(who, text) LOG(fmt('PROBE %d %s %s', GetGameTick() - t0, who, text)) end
local function make(bp, army, x, z, y)
    return CreateUnitHPR(bp, army, x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function dummy(bp, x, z)
    local u = make(bp, CIV, x, z)
    u:SetFireState(1)
    u:SetMaxHealth(30000) u:SetHealth(nil, 30000)
    return u
end
local STATES = { 'Immobile', 'Moving', 'Attacking', 'Guarding', 'Building', 'Upgrading', 'WaitingForTransport', 'TransportLoading',
    'TransportUnloading', 'MovingDown', 'MovingUp', 'Patrolling', 'Busy', 'Attached', 'BeingReclaimed', 'Repairing', 'Diving',
    'Surfacing', 'Teleporting', 'Ferrying', 'WaitForFerry', 'AssistMoving', 'PathFinding', 'ProblemGettingToGoal',
    'NeedToTerminateTask', 'Capturing', 'BeingCaptured', 'Reclaiming', 'AssistingCommander', 'Refueling', 'GuardBusy',
    'ForceSpeedThrough', 'UnSelectable', 'DoNotTarget', 'LandingOnPlatform', 'CannotFindPlaceToLand', 'BeingUpgraded',
    'Enhancing', 'BeingBuilt', 'NoReclaim', 'NoCost', 'BlockCommandQueue', 'MakingAttackRun', 'HoldingPattern', 'SiloBuildingAmmo' }
local function states(u)
    local on = {}
    for _, s in STATES do
        local ok, v = pcall(function() return u:IsUnitState(s) end)
        if ok and v then table.insert(on, s) end
    end
    return table.concat(on, '+')
end
local function cmd(c)
    local keys = {}
    for k, v in c do
        local s
        if type(v) == 'table' then
            s = (v[1] and v[3]) and fmt('(%.1f,%.1f)', v[1], v[3]) or 'table'
        elseif k == 'targetId' or k == 'commandId' or k == 'ID' then
            s = v and 'set' or 'nil'
        else
            s = S(v)
        end
        table.insert(keys, tostring(k) .. '=' .. s)
    end
    table.sort(keys)
    return table.concat(keys, ',')
end
local function queue(u)
    local ok, q = pcall(function() return u:GetCommandQueue() end)
    if not ok then return 'ERR' end
    local parts = {}
    for i, c in q do table.insert(parts, '{' .. cmd(c) .. '}') end
    return table.getn(q) .. ' ' .. table.concat(parts, ' ')
end
local watched = {}
local function watch(u, name, brief)
    tagOf[u] = name
    table.insert(watched, { u = u, last = '', brief = brief })
    return u
end
ForkThread(function()
    while true do
        for _, w in watched do
            if not w.gone then
                local u = w.u
                local s
                if u:BeenDestroyed() then
                    s = 'gone'
                    w.gone = true
                else
                    local f = u:GetFocusUnit()
                    local g = u:GetGuardedUnit()
                    s = fmt('[%s] q %d focus %s guard %s idle %s', states(u), table.getn(u:GetCommandQueue() or {}), f and tag(f) or '-', g and tag(g) or '-', tostring(u:IsIdleState()))
                end
                if s ~= w.last then
                    w.last = s
                    local p = not w.gone and u:GetPosition()
                    P(tag(u), 'state ' .. s .. (p and fmt(' at %.1f %.1f', p[1], p[3]) or ''))
                end
            end
        end
        WaitTicks(1)
    end
end)
local function goal(u)
    local ok, g = pcall(function() return u:GetNavigator():GetGoalPos() end)
    local ok2, t = pcall(function() return u:GetNavigator():GetCurrentTargetPos() end)
    return (ok and g and fmt('goal %.1f %.1f', g[1], g[3]) or 'goal ' .. tostring(ok and g))
        .. (ok2 and t and fmt(' target %.1f %.1f', t[1], t[3]) or ' target ' .. tostring(ok2 and t))
end

-- Q: the queue each order makes.
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 384, 560
    local n = 0
    local far = dummy('ueb1101', x + 60, z)
    local farTank = dummy('uel0201', x + 60, z + 6)
    local own = make('ueb1101', A1, x + 60, z + 12)
    own:SetHealth(nil, 100)
    local ownFac = make('ueb0101', A1, x + 60, z + 24)
    local okp, prop = pcall(CreateProp, pos(x + 60, z - 8), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
    local function case(name, bp, order)
        n = n + 1
        local u = make(bp, A1, x + math.mod(n, 8) * 5, z + math.floor(n / 8) * 5)
        tagOf[u] = name
        local ok, err = pcall(order, u)
        if not ok then P('Q ' .. name, 'ERR | ' .. string.sub(tostring(err), 1, 80)) end
        P('Q ' .. name, 'now ' .. queue(u) .. ' [' .. states(u) .. ']')
        return u
    end
    local cases = {}
    local function add(name, bp, order) cases[table.getn(cases) + 1] = { name, case(name, bp, order) } end
    local T, E = 'uel0201', 'uel0105'
    add('Move', T, function(u) IssueMove({ u }, pos(x + 40, z)) end)
    add('AggressiveMove', T, function(u) IssueAggressiveMove({ u }, pos(x + 40, z)) end)
    add('AttackUnit', T, function(u) IssueAttack({ u }, far) end)
    add('AttackPos', T, function(u) IssueAttack({ u }, pos(x + 60, z)) end)
    add('Patrol', T, function(u) IssuePatrol({ u }, pos(x + 40, z)) end)
    add('GuardUnit', T, function(u) IssueGuard({ u }, own) end)
    add('FormMove', T, function(u) IssueFormMove({ u }, pos(x + 40, z), 'AttackFormation', 0) end)
    add('FormAggressiveMove', T, function(u) IssueFormAggressiveMove({ u }, pos(x + 40, z), 'AttackFormation', 0) end)
    add('FormPatrol', T, function(u) IssueFormPatrol({ u }, pos(x + 40, z), 'AttackFormation', 0) end)
    add('FormAttack', T, function(u) IssueFormAttack({ u }, far, 'AttackFormation', 0) end)
    add('Reclaim', E, function(u) IssueReclaim({ u }, far) end)
    add('ReclaimProp', E, function(u) IssueReclaim({ u }, prop) end)
    add('Repair', E, function(u) IssueRepair({ u }, own) end)
    add('Capture', E, function(u) IssueCapture({ u }, far) end)
    add('BuildMobile', E, function(u) IssueBuildMobile({ u }, pos(x + 40, z + 40), 'ueb1101', {}) end)
    add('GuardEng', E, function(u) IssueGuard({ u }, ownFac) end)
    add('FactoryAssist', E, function(u) IssueFactoryAssist({ u }, ownFac) end)
    add('Sacrifice', 'ual0105', function(u) IssueSacrifice({ u }, own) end)
    add('Teleport', T, function(u) IssueTeleport({ u }, pos(x + 40, z + 20)) end)
    add('Ferry', 'uea0107', function(u) IssueFerry({ u }, pos(x + 40, z + 20)) end)
    add('TransportUnload', 'uea0107', function(u) IssueTransportUnload({ u }, pos(x + 40, z + 20)) end)
    add('Stop', T, function(u) IssueMove({ u }, pos(x + 40, z)) IssueStop({ u }) end)
    add('Pause', T, function(u) IssueMove({ u }, pos(x + 40, z)) IssuePause({ u }) end)
    add('Tactical', 'ueb2108', function(u) u:GiveTacticalSiloAmmo(1) IssueTactical({ u }, pos(x + 100, z)) end)
    add('TacticalUnit', 'ueb2108', function(u) IssueTactical({ u }, far) end)
    add('SiloBuildTactical', 'ueb2108', function(u) IssueSiloBuildTactical({ u }) end)
    add('Nuke', 'ueb2305', function(u) IssueNuke({ u }, pos(x + 100, z)) end)
    add('SiloBuildNuke', 'ueb2305', function(u) IssueSiloBuildNuke({ u }) end)
    add('BuildFactory', 'ueb0101', function(u) IssueBuildFactory({ u }, 'uel0201', 2) end)
    add('Upgrade', 'ueb3101', function(u) IssueUpgrade({ u }, 'ueb3201') end)
    add('Dive', 'uel0201', function(u) IssueDive({ u }) end)
    add('OverCharge', 'uel0201', function(u) IssueOverCharge({ u }, far) end)
    add('MoveOffFactory', T, function(u) IssueMoveOffFactory({ u }, pos(x + 40, z)) end)
    add('TwoMoves', T, function(u) IssueMove({ u }, pos(x + 40, z)) IssueMove({ u }, pos(x + 40, z + 10)) end)
    add('MoveThenClear', T, function(u) IssueMove({ u }, pos(x + 40, z)) IssueClearCommands({ u }) end)
    add('KillSelf', T, function(u) IssueKillSelf({ u }) end)
    add('DestroySelf', T, function(u) IssueDestroySelf({ u }) end)
    for _, wait in { 1, 1, 8 } do
        WaitTicks(wait)
        for _, c in cases do
            local u = c[2]
            if u:BeenDestroyed() then P('Q ' .. c[1], 'gone') else P('Q ' .. c[1], 'later ' .. queue(u) .. ' [' .. states(u) .. ']') end
        end
    end
    for _, c in cases do
        if not c[2]:BeenDestroyed() then c[2]:Destroy() end
    end
end)

-- O1 / O2
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 96, 608
    local u = watch(make('uel0201', A1, x, z), 'o1')
    P('o1', 'idle ' .. goal(u))
    WaitTicks(5)
    IssueMove({ u }, pos(x + 20, z))
    for i = 1, 100 do
        WaitTicks(1)
        if i <= 3 or i == 20 or i == 70 or i == 99 then P('o1', fmt('+%d %s moving %s', i, goal(u), tostring(u:IsMoving()))) end
    end
    IssueMove({ u }, pos(x, z))
    WaitTicks(20)
    IssueClearCommands({ u })
    P('o1', 'cleared ' .. goal(u))
    WaitTicks(30)
    P('o1', 'cleared +30 ' .. goal(u) .. fmt(' at %.1f', u:GetPosition()[1]))
    local v = watch(make('uel0201', A1, x, z + 20), 'o2')
    IssueMove({ v }, pos(x + 10, z + 20)) IssueMove({ v }, pos(x + 10, z + 30)) IssueMove({ v }, pos(x, z + 30))
    P('o2', 'queued ' .. queue(v))
    local cleared = false
    for i = 1, 120 do
        WaitTicks(1)
        if not cleared and table.getn(v:GetCommandQueue()) == 2 then
            cleared = true
            WaitTicks(10)
            IssueClearCommands({ v })
            P('o2', 'clear ' .. queue(v))
        end
    end
    IssueMove({ v }, pos(x + 20, z + 20))
    WaitTicks(15)
    IssueStop({ v })
    P('o2', 'stop ' .. queue(v))
    WaitTicks(20)
    P('o2', fmt('stopped at %.1f %.1f', v:GetPosition()[1], v:GetPosition()[3]))
end)

-- O3 patrol
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 352, 640
    local a = watch(make('uel0201', A1, x, z), 'pat1')
    local b = watch(make('uel0201', A1, x, z + 20), 'pat2')
    IssuePatrol({ a }, pos(x + 15, z))
    IssuePatrol({ b }, pos(x + 15, z + 20)) IssuePatrol({ b }, pos(x + 15, z + 30))
    P('pat1', 'queued ' .. queue(a))
    P('pat2', 'queued ' .. queue(b))
    for i = 1, 250 do
        WaitTicks(1)
        if i == 2 or math.mod(i, 50) == 0 then
            P('pat1', fmt('+%d %s', i, queue(a)))
            P('pat2', fmt('+%d %s', i, queue(b)))
        end
    end
    IssueMove({ b }, pos(x, z + 20))
    P('pat2', 'move queued after patrol ' .. queue(b))
end)

-- O4 guard
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 64, 688
    local lead = watch(make('uel0201', A1, x, z), 'lead')
    local g = watch(make('uel0201', A1, x - 6, z), 'guard')
    IssueGuard({ g }, lead)
    WaitTicks(20)
    IssueMove({ lead }, pos(x + 30, z))
    WaitTicks(100)
    P('guard', fmt('dist to lead %.1f', VDist3(g:GetPosition(), lead:GetPosition())))
    lead:Destroy()
    WaitTicks(20)
    P('guard', 'after lead gone ' .. queue(g))
    -- engineers: one builds, one guards it; then the guarded one is idle and hurt
    local b = watch(make('uel0105', A1, x, z + 30), 'builder')
    local h = watch(make('uel0105', A1, x - 4, z + 30), 'helper')
    IssueGuard({ h }, b)
    WaitTicks(10)
    IssueBuildMobile({ b }, pos(x + 4, z + 34), 'ueb1101', {})
    WaitTicks(40)
    P('helper', 'assisting focus ' .. tag(h:GetFocusUnit()) .. ' builder focus ' .. tag(b:GetFocusUnit()))
    IssueClearCommands({ b })
    WaitTicks(10)
    b:SetHealth(nil, 50)
    WaitTicks(30)
    P('helper', fmt('builder hp %.1f', b:GetHealth()))
end)

-- O5 attack
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 176, 688
    local target = dummy('ueb1101', x + 30, z)
    tagOf[target] = 'o5target'
    local u = watch(make('uel0201', A1, x, z), 'atk')
    IssueAttack({ u }, target)
    for i = 1, 180 do
        WaitTicks(1)
        if i == 110 then P('atk', fmt('dist %.2f target hp %.0f targetEntity %s', VDist3(u:GetPosition(), target:GetPosition()), target:GetHealth(), tag(u:GetTargetEntity()))) end
        if i == 120 then
            target:SetDoNotTarget(true)
            P('atk', 'SetDoNotTarget(true)')
        end
        if i == 150 then P('atk', fmt('+30 target hp %.0f targetEntity %s %s', target:GetHealth(), tag(u:GetTargetEntity()), queue(u))) end
        if i == 179 then P('atk', fmt('+59 target hp %.0f', target:GetHealth())) end
    end
    local u2 = watch(make('uel0201', A1, x, z + 20), 'atkground')
    IssueAttack({ u2 }, pos(x + 30, z + 20))
    WaitTicks(120)
    P('atkground', fmt('dist %.2f %s', VDist3(u2:GetPosition(), pos(x + 30, z + 20)), queue(u2)))
end)

-- O6 teleport, O7 enhancement
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 320, 720
    local t = watch(make('uel0201', A1, x, z), 'tele')
    local c = watch(make('uel0001', A1, x, z + 10), 'cdr')
    WaitTicks(5)
    IssueTeleport({ t }, pos(x + 20, z))
    IssueTeleport({ c }, pos(x + 20, z + 10))
    WaitTicks(40)
    P('tele', fmt('at %.1f %.1f', t:GetPosition()[1], t:GetPosition()[3]))
    P('cdr', fmt('at %.1f %.1f', c:GetPosition()[1], c:GetPosition()[3]))
    IssueClearCommands({ c })
    WaitTicks(3)
    for _, name in { 'OnWorkBegin', 'OnWorkEnd', 'OnWorkFail', 'CreateEnhancement', 'OnPaused', 'OnUnpaused' } do
        local orig, n = c[name], name
        c[n] = function(self, a, b)
            P('cdr', n .. ' ' .. S(a) .. ' ' .. S(b))
            return orig(self, a, b)
        end
    end
    c:SetBuildRate(100)
    brain:GiveStorage('ENERGY', 500000)
    brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
    WaitTicks(2)
    local m0, e0 = brain:GetEconomyStored('MASS'), brain:GetEconomyStored('ENERGY')
    IssueScript({ c }, { TaskName = 'EnhanceTask', Enhancement = 'AdvancedEngineering' })
    P('cdr', 'enhance ordered ' .. queue(c))
    for i = 1, 150 do
        WaitTicks(1)
        if i <= 5 or math.mod(i, 20) == 0 then
            P('cdr', fmt('+%d work %.3f has %s spent %.1f %.1f cons %.2f %.2f', i, c:GetWorkProgress(), tostring(c:HasEnhancement('AdvancedEngineering')),
                m0 - brain:GetEconomyStored('MASS'), e0 - brain:GetEconomyStored('ENERGY'), c:GetConsumptionPerSecondMass(), c:GetConsumptionPerSecondEnergy()))
        end
        if i == 40 then c:SetPaused(true) end
        if i == 50 then c:SetPaused(false) end
    end
    IssueScript({ c }, { TaskName = 'EnhanceTask', Enhancement = 'ResourceAllocation' })
    WaitTicks(20)
    P('cdr', fmt('second work %.3f', c:GetWorkProgress()))
    IssueClearCommands({ c })
    WaitTicks(5)
    P('cdr', fmt('cancelled work %.3f has %s %s', c:GetWorkProgress(), tostring(c:HasEnhancement('ResourceAllocation')), queue(c)))
end)

-- O8 blockers
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 224, 768
    local function blocker(name, dz, set)
        local u = watch(make('uel0201', A1, x, z + dz), name)
        local ok, err = pcall(set, u)
        if not ok then P(name, 'ERR | ' .. string.sub(tostring(err), 1, 80)) end
        IssueMove({ u }, pos(x + 15, z + dz))
        P(name, 'ordered ' .. queue(u))
        return u
    end
    local us = {
        blocker('blockq', 0, function(u) u:SetBlockCommandQueue(true) end),
        blocker('busy', 6, function(u) u:SetBusy(true) end),
        blocker('paused', 12, function(u) u:SetPaused(true) end),
        blocker('immobile', 18, function(u) u:SetImmobile(true) end),
        blocker('stunned', 24, function(u) u:SetStunned(3) end),
        blocker('plain', 30, function(u) end),
    }
    WaitTicks(60)
    for _, u in us do P(tag(u), fmt('+60 at %.1f %s', u:GetPosition()[1], queue(u))) end
    us[1]:SetBlockCommandQueue(false) us[2]:SetBusy(false) us[3]:SetPaused(false) us[4]:SetImmobile(false)
    WaitTicks(60)
    for _, u in us do P(tag(u), fmt('+120 at %.1f %s', u:GetPosition()[1], queue(u))) end
end)

-- O9 transport
ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    local x, z = 112, 784
    local tr = watch(make('uea0107', A1, x, z), 'transport')
    local a = watch(make('uel0201', A1, x + 6, z), 'cargo1')
    local b = watch(make('uel0201', A1, x + 6, z + 4), 'cargo2')
    WaitTicks(5)
    IssueTransportLoad({ a, b }, tr)
    P('transport', 'load ordered ' .. queue(tr))
    P('cargo1', 'load ordered ' .. queue(a))
    local loaded
    for i = 1, 200 do
        WaitTicks(1)
        if not loaded and table.getn(tr:GetCargo()) == 2 then
            loaded = i
            P('transport', fmt('loaded both after %d parent %s', i, tag(a:GetParent())))
            IssueTransportUnload({ tr }, pos(x + 40, z))
            P('transport', 'unload ordered ' .. queue(tr))
        end
        if loaded and table.getn(tr:GetCargo()) == 0 then
            P('transport', fmt('unloaded after %d cargo1 at %.1f %.1f', i - loaded, a:GetPosition()[1], a:GetPosition()[3]))
            break
        end
    end
end)

ForkThread(function()
    WaitTicks(560)
    -- last: a script order naming a task that doesn't exist
    local u = make('uel0201', A1, 432, 912)
    tagOf[u] = 'badscript'
    P('badscript', 'ordering')
    local ok, err = pcall(function() IssueScript({ u }, { TaskName = 'ProbeNoSuchTask' }) end)
    P('badscript', 'ordered ' .. tostring(ok) .. ' ' .. queue(u))
    WaitTicks(5)
    P('badscript', 'survived ' .. queue(u))
    WaitTicks(25)
    LOG('PROBE done')
end)
