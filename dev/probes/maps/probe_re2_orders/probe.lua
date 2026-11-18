-- Differential probe (sweep 2 leftovers): orders.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re2_orders 110
--   ours: --headless --no-ai --ticks 600 --script dev/probes/maps/probe_re2_orders/probe.lua
-- SILO   IssueSiloBuild* on launchers: ammo over time (separate silo queue?).
-- FA     IssueFactoryAssist by an engineer, a factory, an ACU, a tank.
-- SAC    IssueSacrifice by an Aeon engineer on a finished own structure.
-- ST     unit states: fresh idle tank, guard variants, attack, patrol, transport.
-- ATK    attack-order stop distance: 3 weapons x 5 target sizes.
-- REP    IssueRepair on a damaged own tank, variants.
-- Lines: PROBE <tick> <tag> <event>; 'state' lines when states / queue / focus change.
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
brain:GiveStorage('MASS', 100000) brain:GiveResource('MASS', 100000)
brain:GiveStorage('ENERGY', 1000000) brain:GiveResource('ENERGY', 1000000)
local t0 = GetGameTick()
local tagOf = {}
local function tag(u)
    if u == nil then return 'nil' end
    if type(u) ~= 'table' then return tostring(u) end
    if tagOf[u] then return tagOf[u] end
    local ok, id = pcall(function() return u:GetBlueprint().BlueprintId end)
    return ok and id and string.gsub(tostring(id), '.*/', '') or 'table'
end
local function P(who, text) LOG(fmt('PROBE %d %s %s', GetGameTick() - t0, who, text)) end
local function make(bp, army, x, z)
    return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function dist(a, b)
    local dx, dz = a[1] - b[1], a[3] - b[3]
    return math.sqrt(dx * dx + dz * dz)
end
local function dummy(bp, x, z)
    local u = make(bp, CIV, x, z)
    u:SetFireState(1)
    u:SetMaxHealth(30000) u:SetHealth(nil, 30000)
    return u
end
local STATES = { 'Immobile', 'Moving', 'Attacking', 'Guarding', 'Building', 'Upgrading', 'WaitingForTransport', 'TransportLoading',
    'TransportUnloading', 'MovingDown', 'MovingUp', 'Patrolling', 'Busy', 'Attached', 'Repairing', 'Ferrying', 'WaitForFerry',
    'AssistMoving', 'PathFinding', 'Capturing', 'Reclaiming', 'GuardBusy', 'ForceSpeedThrough', 'NoCost', 'BlockCommandQueue',
    'HoldingPattern', 'SiloBuildingAmmo', 'BeingBuilt', 'AssistingCommander' }
local function states(u)
    local on = {}
    for _, s in STATES do
        local ok, v = pcall(function() return u:IsUnitState(s) end)
        if ok and v then table.insert(on, s) end
    end
    return table.concat(on, '+')
end
local function queue(u)
    local ok, q = pcall(function() return u:GetCommandQueue() end)
    if not ok then return 'ERR' end
    local parts = {}
    for _, c in q do
        local s = tostring(c.commandType)
        if c.x then s = s .. fmt('@%.1f,%.1f', c.x, c.z) end
        table.insert(parts, s)
    end
    return table.getn(q) .. ' [' .. table.concat(parts, ' ') .. ']'
end
local watched = {}
local function watch(u, name, extra)
    tagOf[u] = name
    table.insert(watched, { u = u, last = '', extra = extra })
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
                    s = fmt('[%s] q %s focus %s guard %s idle %s', states(u), queue(u), f and tag(f) or '-', g and tag(g) or '-', tostring(u:IsIdleState()))
                    if w.extra then s = s .. ' ' .. w.extra(u) end
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
local SITES = { { 608, 80 }, { 688, 80 }, { 912, 96 }, { 784, 144 }, { 656, 160 }, { 736, 224 }, { 816, 224 }, { 912, 240 }, { 672, 304 }, { 800, 336 },
    { 960, 336 }, { 656, 384 }, { 880, 400 }, { 576, 464 }, { 384, 560 }, { 96, 608 }, { 352, 640 }, { 64, 688 }, { 176, 688 }, { 320, 720 },
    { 224, 768 }, { 112, 784 }, { 304, 800 }, { 240, 880 }, { 352, 880 }, { 96, 896 }, { 432, 912 } }

-- ST: a tank made at the probe's start: states every tick for 12 ticks
ForkThread(function()
    local x, z = SITES[4][1], SITES[4][2]
    local u = make('uel0201', A1, x, z)
    tagOf[u] = 'fresh0'
    for i = 0, 12 do
        local p = u:GetPosition()
        local vx, vy, vz = u:GetVelocity()
        P('fresh0', fmt('[%s] idle %s y-ground %.3f v %.3f %.3f %.3f', states(u), tostring(u:IsIdleState()), p[2] - GetTerrainHeight(p[1], p[3]), vx, vy, vz))
        WaitTicks(1)
    end
end)

ForkThread(function()
    WaitTicks(15) -- (FAF reveals the civilian units that exist at tick 10)
    t0 = GetGameTick()
    P('t0', 'cases start')

    -- ST: fresh tank at t15 (tick 0 here), a second made on uneven ground
    do
        local x, z = SITES[4][1], SITES[4][2]
        local u = make('uel0201', A1, x + 20, z)
        tagOf[u] = 'fresh15'
        local ok, u2 = pcall(make, 'uel0201', A1, x + 20, z + 10)
        if ok then tagOf[u2] = 'fresh15b' u2:SetOrientation({ 0, 0, 0, 1 }, true) end
        ForkThread(function()
            for i = 0, 12 do
                for _, v in { u, u2 } do
                    local p = v:GetPosition()
                    local vx, vy, vz = v:GetVelocity()
                    P(tag(v), fmt('[%s] idle %s y-ground %.3f v %.3f %.3f %.3f', states(v), tostring(v:IsIdleState()), p[2] - GetTerrainHeight(p[1], p[3]), vx, vy, vz))
                end
                WaitTicks(1)
            end
        end)
    end
    -- ST guard variants
    do
        local x, z = SITES[5][1], SITES[5][2]
        local engIdle = watch(make('uel0105', A1, x, z), 'g-engidle')
        local engBuild = watch(make('uel0105', A1, x, z + 12), 'g-engbuild')
        local pg = make('ueb1101', A1, x + 20, z + 24)
        local tIdle = watch(make('uel0201', A1, x + 20, z), 'g-tankidle')
        local tMove = watch(make('uel0201', A1, x + 20, z + 36), 'g-tankmove')
        local a = watch(make('uel0201', A1, x - 8, z), 'guard-engidle')
        local b = watch(make('uel0201', A1, x - 8, z + 12), 'guard-engbuild')
        local c = watch(make('uel0105', A1, x - 8, z + 16), 'engguard-engbuild')
        local d = watch(make('uel0201', A1, x + 12, z + 24), 'guard-pgen')
        local e = watch(make('uel0201', A1, x + 12, z), 'guard-tankidle')
        local f = watch(make('uel0201', A1, x + 12, z + 36), 'guard-tankmove')
        IssueBuildMobile({ engBuild }, pos(x + 2, z + 22), 'ueb1101', {})
        IssueGuard({ a }, engIdle) IssueGuard({ b }, engBuild) IssueGuard({ c }, engBuild)
        IssueGuard({ d }, pg) IssueGuard({ e }, tIdle) IssueGuard({ f }, tMove)
        IssueMove({ tMove }, pos(x + 50, z + 36))
    end
    -- ST attack in range / out of range, patrol
    do
        local x, z = SITES[6][1], SITES[6][2]
        local t1 = dummy('ueb1101', x + 10, z)
        local t2 = dummy('ueb1101', x + 40, z + 15)
        local a = watch(make('uel0201', A1, x, z), 'atk-inrange')
        local b = watch(make('uel0201', A1, x, z + 15), 'atk-outrange')
        IssueAttack({ a }, t1) IssueAttack({ b }, t2)
        local x2, z2 = SITES[7][1], SITES[7][2]
        local p = watch(make('uel0201', A1, x2, z2), 'patrol2')
        IssuePatrol({ p }, pos(x2 + 20, z2)) IssuePatrol({ p }, pos(x2 + 20, z2 + 20))
    end
    -- ST transport
    ForkThread(function()
        local x, z = SITES[8][1], SITES[8][2]
        local tr = watch(make('uea0107', A1, x, z), 'transport')
        local a = watch(make('uel0201', A1, x + 6, z), 'cargo1')
        local b = watch(make('uel0201', A1, x + 6, z + 4), 'cargo2')
        WaitTicks(5)
        IssueTransportLoad({ a, b }, tr)
        local loaded
        for i = 1, 300 do
            WaitTicks(1)
            if tr:BeenDestroyed() then break end
            if not loaded and table.getn(tr:GetCargo()) == 2 then
                loaded = i
                P('transport', 'loaded both after ' .. i)
                IssueTransportUnload({ tr }, pos(x + 40, z))
            end
            if loaded and table.getn(tr:GetCargo()) == 0 then
                P('transport', 'unloaded after ' .. (i - loaded))
                break
            end
        end
    end)

    -- SILO
    do
        local x, z = SITES[1][1], SITES[1][2]
        local L = {}
        local function launcher(name, bp, dx, order)
            local u = make(bp, A1, x + dx, z)
            tagOf[u] = name
            local ok, err = pcall(order, u)
            if not ok then P('SILO ' .. name, 'ERR | ' .. string.sub(tostring(err), 1, 80)) end
            table.insert(L, u)
            P('SILO ' .. name, 'now q ' .. queue(u) .. ' [' .. states(u) .. ']')
        end
        launcher('tml', 'ueb2108', -30, function(u) IssueSiloBuildTactical({ u }) end)
        launcher('tml2x', 'ueb2108', -15, function(u) IssueSiloBuildTactical({ u }) IssueSiloBuildTactical({ u }) end)
        launcher('tmlnone', 'ueb2108', 0, function(u) end)
        launcher('sml', 'ueb2305', 15, function(u) IssueSiloBuildNuke({ u }) end)
        launcher('smd', 'ueb4302', 32, function(u) IssueSiloBuildNuke({ u }) end)
        ForkThread(function()
            local last = {}
            for i = 1, 560 do
                WaitTicks(1)
                for _, u in L do
                    local s = fmt('q %s [%s] tac %s nuke %s work %s', queue(u), states(u), tostring(u:GetTacticalSiloAmmoCount()), tostring(u:GetNukeSiloAmmoCount()),
                        fmt('%.2f', u:GetWorkProgress() or -1))
                    local key = string.gsub(s, 'work .*', '')
                    if key ~= last[u] or math.mod(i, 50) == 0 then P('SILO ' .. tag(u), s) end
                    last[u] = key
                end
            end
        end)
    end

    -- FA
    do
        local x, z = SITES[2][1], SITES[2][2]
        local f0 = make('ueb0101', A1, x, z)
        tagOf[f0] = 'f0'
        local fIdle = make('ueb0101', A1, x + 40, z + 20)
        tagOf[fIdle] = 'fidle'
        IssueBuildFactory({ f0 }, 'uel0201', 10)
        local function assist(name, bp, ax, az, target)
            local u = make(bp, A1, ax, az)
            local ok, err = pcall(function() IssueFactoryAssist({ u }, target) end)
            if not ok then P('FA ' .. name, 'ERR | ' .. string.sub(tostring(err), 1, 80)) end
            P('FA ' .. name, 'now q ' .. queue(u) .. ' [' .. states(u) .. ']')
            watch(u, 'FA ' .. name)
        end
        WaitTicks(3)
        assist('eng', 'uel0105', x - 10, z + 12, f0)
        assist('fac', 'ueb0101', x - 20, z, f0)
        assist('acu', 'uel0001', x + 10, z + 12, f0)
        assist('tank', 'uel0201', x + 14, z + 12, f0)
        assist('fac-idle', 'ueb0101', x + 20, z + 20, fIdle)
        assist('eng-idle', 'uel0105', x + 40, z + 30, fIdle)
        local g = make('uel0105', A1, x - 10, z + 16)
        IssueGuard({ g }, f0)
        watch(g, 'FA ref-guard')
    end

    -- SAC
    do
        local x, z = SITES[3][1], SITES[3][2]
        local full = make('ueb1101', A1, x + 10, z)
        tagOf[full] = 'sac-pgenfull'
        local dmg = make('ueb1101', A1, x + 10, z + 15)
        tagOf[dmg] = 'sac-pgendmg'
        dmg:SetHealth(nil, 100)
        local hp = function(t) return function() return t:BeenDestroyed() and 'tgt gone' or fmt('tgt hp %.0f frac %.2f', t:GetHealth(), t:GetFractionComplete()) end end
        local s1 = watch(make('ual0105', A1, x, z), 'sac-full', hp(full))
        local s2 = watch(make('ual0105', A1, x, z + 15), 'sac-dmg', hp(dmg))
        IssueSacrifice({ s1 }, full) IssueSacrifice({ s2 }, dmg)
        P('SAC', 'ordered full q ' .. queue(s1) .. ' dmg q ' .. queue(s2))
    end

    -- ATK: attacker 50 west of a target; 2 lanes per site (z -12, +12)
    do
        local ATT = { 'uel0201', 'uel0103', 'uel0202' }
        local TGT = { 'ueb5101', 'ueb1101', 'ueb0101', 'ueb1301', 'uel0201' }
        local lanes = {}
        local n = 0
        for ai, abp in ATT do
            for ti, tbp in TGT do
                local site = SITES[9 + math.floor(n / 2)]
                local dz = (math.mod(n, 2) == 0) and -12 or 12
                n = n + 1
                local x, z = site[1], site[2] + dz
                local t = dummy(tbp, x + 25, z)
                local a = make(abp, A1, x - 25, z)
                a:SetFireState(0)
                local name = 'ATK ' .. abp .. '>' .. tbp
                local bp = t:GetBlueprint()
                local wr = '?'
                pcall(function() wr = tostring(a:GetBlueprint().Weapon[1].MaxRadius) end)
                P(name, fmt('range %s footprint %s %s skirt %s %s start %.2f', wr, tostring(bp.Footprint.SizeX), tostring(bp.Footprint.SizeZ),
                    tostring(bp.Physics.SkirtSizeX), tostring(bp.Physics.SkirtSizeZ), dist(a:GetPosition(), t:GetPosition())))
                IssueAttack({ a }, t)
                table.insert(lanes, { name = name, a = a, t = t, moved = false, still = 0 })
            end
        end
        ForkThread(function()
            for i = 1, 200 do
                WaitTicks(1)
                for _, l in lanes do
                    if not l.a:BeenDestroyed() then
                        local vx, vy, vz = l.a:GetVelocity()
                        local sp = math.sqrt(vx * vx + vz * vz)
                        if sp > 0.01 then l.moved = true l.still = 0 else l.still = l.still + 1 end
                        if l.moved and not l.stopped and l.still >= 3 then
                            l.stopped = i
                            P(l.name, fmt('stopped at +%d dist %.2f hp %.0f', i - 3, dist(l.a:GetPosition(), l.t:GetPosition()), l.t:GetHealth()))
                        end
                        if i == 200 then
                            P(l.name, fmt('end dist %.2f hp %.0f moved %s q %s', dist(l.a:GetPosition(), l.t:GetPosition()), l.t:GetHealth(), tostring(l.moved), queue(l.a)))
                        end
                    end
                end
            end
        end)
    end

    -- REP
    do
        local function lane(name, site, dz, ebp, edist, setup, delay)
            ForkThread(function()
                local x, z = SITES[site][1], SITES[site][2] + dz
                local t = make(setup.bp or 'uel0201', A1, x + 15, z)
                local e = make(ebp, A1, x + 15 - edist, z)
                if setup.hp then t:SetHealth(nil, setup.hp) end
                if setup.adjust then pcall(function() t:AdjustHealth(nil, setup.adjust) end) end
                if delay then WaitTicks(delay) end
                P('REP ' .. name, fmt('target hp %.0f/%.0f', t:GetHealth(), t:GetMaxHealth()))
                IssueRepair({ e }, t)
                P('REP ' .. name, 'now q ' .. queue(e) .. ' [' .. states(e) .. ']')
                watch(e, 'REP ' .. name, function() return t:BeenDestroyed() and 'tgt gone' or fmt('tgt hp %.0f', t:GetHealth()) end)
            end)
        end
        lane('far-fresh', 17, -15, 'uel0105', 30, { hp = 50 })
        lane('far-wait', 17, 0, 'uel0105', 30, { hp = 50 }, 10)
        lane('near-fresh', 17, 15, 'uel0105', 5, { hp = 50 })
        lane('near-wait', 18, -15, 'uel0105', 5, { hp = 50 }, 10)
        lane('adjust', 18, 0, 'uel0105', 5, { adjust = -200 }, 10)
        lane('acu', 18, 15, 'uel0001', 8, { hp = 50 }, 10)
        lane('full', 19, -15, 'uel0105', 5, {}, 10)
        lane('pgen', 19, 0, 'uel0105', 6, { bp = 'ueb1101', hp = 100 }, 10)
        lane('engtarget', 19, 15, 'uel0105', 5, { bp = 'uel0105', hp = 50 }, 10)
    end
end)

ForkThread(function()
    WaitTicks(600)
    WaitTicks(30)
    LOG('PROBE done')
end)
