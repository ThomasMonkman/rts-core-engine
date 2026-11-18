-- Differential probe (sweep 6): follow-up to probe_re5_platoon.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re6_platoon 90
--   ours: --headless --no-ai --ticks 500 --script dev/probes/maps/probe_re6_platoon/probe.lua
-- UD  every unit made here logs its death (OnKilled callback) and the beat it is destroyed: in re5 the
--     platoons of idle live tanks ended about 104 beats after they were made, and so did the enemy tank.
-- CE  which platoon calls raise, on three platoons: one whose only unit is dead but not yet destroyed
--     (a bomber falling), one that has ended (unit moved out), and a live one as the baseline.
-- EM  platoons made empty: unnamed and named, state each beat for 6 beats, then every 20 to +100,
--     then a unit assigned into each.
-- NM  a named platoon whose unit is moved out, and one whose unit is killed: state to +40, then refilled.
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end
local brain = ArmyBrains[1]
local watched = {}
local function make(tag, bp, x, z, army)
    local u = CreateUnitHPR(bp, army or 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetFireState(1)
    u:AddUnitCallback(function(self, instigator)
        P(fmt('UD %s OnKilled by=%s', tag, instigator and (instigator.GetUnitId and instigator:GetUnitId() or 'entity') or 'nil'))
    end, 'OnKilled')
    table.insert(watched, {tag = tag, u = u})
    return u
end
ForkThread(function()
    while true do
        for _, w in watched do
            if not w.gone and w.u:BeenDestroyed() then
                w.gone = true
                P(fmt('UD %s destroyed', w.tag))
            end
        end
        WaitTicks(1)
    end
end)
local function plat(name)
    local p = brain:MakePlatoon(name, 'none')
    p:AddDestroyCallback(function() P(name .. ' OnDestroy') end)
    return p
end
local function state(tag, p)
    local d = IsDestroyed(p)
    local ok, n = pcall(function() return table.getn(p:GetPlatoonUnits() or {}) end)
    P(fmt('%s exists=%s destroyed=%s units=%s', tag, tostring(brain:PlatoonExists(p)), tostring(d), ok and tostring(n) or 'ERR'))
end
local function try_assign(tag, p, u)
    local ok, e = pcall(function() return brain:AssignUnitsToPlatoon(p, {u}, 'Support', 'None') end)
    P(fmt('%s assign ok=%s', tag, tostring(ok)))
end
local function show(v)
    if type(v) == 'table' then return 'table#' .. table.getn(v) end
    if type(v) == 'number' then return fmt('%.2f', v) end
    return tostring(v)
end
local function census(tag, p, target, spot)
    local list = {
        {'GetBrain', function() return p:GetBrain() == brain end},
        {'GetPlatoonUnits', function() return p:GetPlatoonUnits() end},
        {'GetSquadUnits', function() return p:GetSquadUnits('Attack') end},
        {'GetPlatoonPosition', function() return p:GetPlatoonPosition() end},
        {'GetSquadPosition', function() return p:GetSquadPosition('Attack') end},
        {'CalculatePlatoonThreat', function() return p:CalculatePlatoonThreat('Surface', categories.ALLUNITS) end},
        {'CalculatePlatoonThreatAroundPosition', function() return p:CalculatePlatoonThreatAroundPosition('Surface', categories.ALLUNITS, spot, 45) end},
        {'CanAttackTarget', function() return p:CanAttackTarget('Attack', target) end},
        {'PlatoonCategoryCount', function() return p:PlatoonCategoryCount(categories.ALLUNITS) end},
        {'PlatoonCategoryCountAroundPosition', function() return p:PlatoonCategoryCountAroundPosition(categories.ALLUNITS, spot, 45) end},
        {'GetPlatoonUniqueName', function() return p:GetPlatoonUniqueName() end},
        {'FindClosestUnit', function() return p:FindClosestUnit('Attack', 'Enemy', true, categories.ALLUNITS) ~= nil end},
        {'SetPlatoonFormationOverride', function() return p:SetPlatoonFormationOverride('NoFormation') end},
        {'Stop', function() return p:Stop() end},
        {'MoveToLocation', function() return p:MoveToLocation(spot, false) ~= nil end},
        {'AggressiveMoveToLocation', function() return p:AggressiveMoveToLocation(spot) ~= nil end},
        {'AttackTarget', function() return p:AttackTarget(target) ~= nil end},
        {'MoveToTarget', function() return p:MoveToTarget(target, false) ~= nil end},
        {'Patrol', function() return p:Patrol(spot) ~= nil end},
        {'GuardTarget', function() return p:GuardTarget(target) ~= nil end},
        {'IssueAttack on its units', function() local us = p:GetPlatoonUnits() IssueAttack(us, target) return table.getn(us) end},
        {'UniquelyNamePlatoon', function() return p:UniquelyNamePlatoon(tag .. 'Name') end},
        {'SetAIPlan', function() return p:SetAIPlan('none') end},
        {'PlatoonDisband', function() return p:PlatoonDisband() end},
    }
    for _, c in list do
        local ok, v = pcall(c[2])
        P(fmt('CE %s %s -> %s', tag, c[1], ok and show(v) or 'ERR'))
    end
end

ForkThread(function()
    WaitTicks(5)
    -- UD: idle tanks in unnamed platoons, alone and in none, and an idle enemy tank far off
    local ud1, ud2 = make('ud1', 'uel0201', 300, 300), make('ud2', 'uel0201', 305, 300)
    local ud3 = make('ud3', 'uel0201', 300, 320)
    local enemy = make('enemy', 'uel0201', 600, 600, 'ARMY_2')
    local udp = plat('UDP')
    brain:AssignUnitsToPlatoon(udp, {ud1, ud2}, 'Attack', 'None')
    P(fmt('UD made, pool units=%d', table.getn(brain:GetPlatoonUniquelyNamed('ArmyPool') and brain:GetPlatoonUniquelyNamed('ArmyPool'):GetPlatoonUnits() or {})))
    ForkThread(function()
        for i = 0, 300, 10 do
            state(fmt('UDP +%d', i), udp)
            P(fmt('UD +%d dead ud1=%s ud2=%s ud3=%s enemy=%s', i, tostring(ud1.Dead), tostring(ud2.Dead), tostring(ud3.Dead), tostring(enemy.Dead)))
            WaitTicks(10)
        end
    end)

    -- EM
    local em1, em2 = plat('EM1'), plat('EM2')
    em2:UniquelyNamePlatoon('ProbeEmpty')
    ForkThread(function()
        for i = 0, 5 do
            state(fmt('EM1 +%d', i), em1) state(fmt('EM2 +%d', i), em2)
            WaitTicks(1)
        end
        for i = 20, 100, 20 do
            WaitTicks(20)
            state(fmt('EM1 +%d', i), em1) state(fmt('EM2 +%d', i), em2)
        end
        try_assign('EM1 fill', em1, make('emu1', 'uel0201', 340, 300))
        try_assign('EM2 fill', em2, make('emu2', 'uel0201', 345, 300))
        WaitTicks(3)
        state('EM1 end', em1) state('EM2 end', em2)
        P(fmt('EM name still=%s', tostring(brain:GetPlatoonUniquelyNamed('ProbeEmpty') == em2)))
    end)

    -- NM
    local nm1, nm2, nmhold = plat('NM1'), plat('NM2'), plat('NMHOLD')
    nm1:UniquelyNamePlatoon('ProbeNamed1')
    nm2:UniquelyNamePlatoon('ProbeNamed2')
    local n1, n2, keep = make('n1', 'uel0201', 360, 300), make('n2', 'uel0201', 365, 300), make('keep', 'uel0201', 370, 300)
    brain:AssignUnitsToPlatoon(nm1, {n1}, 'Support', 'None')
    brain:AssignUnitsToPlatoon(nm2, {n2}, 'Support', 'None')
    brain:AssignUnitsToPlatoon(nmhold, {keep}, 'Support', 'None')
    ForkThread(function()
        WaitTicks(2)
        P('NM empty')
        brain:AssignUnitsToPlatoon(nmhold, {n1}, 'Support', 'None')
        n2:Kill()
        for i = 0, 40, 5 do
            state(fmt('NM1 +%d', i), nm1) state(fmt('NM2 +%d', i), nm2)
            WaitTicks(5)
        end
        try_assign('NM1 refill', nm1, n1)
        try_assign('NM2 refill', nm2, make('n3', 'uel0201', 375, 300))
        WaitTicks(3)
        state('NM1 end', nm1) state('NM2 end', nm2)
        P(fmt('NM names 1=%s 2=%s', tostring(brain:GetPlatoonUniquelyNamed('ProbeNamed1') == nm1), tostring(brain:GetPlatoonUniquelyNamed('ProbeNamed2') == nm2)))
    end)

    -- CE
    local spot = {320, GetTerrainHeight(320, 300), 300}
    local live, ended, falling = plat('CELIVE'), plat('CEENDED'), plat('CEFALL')
    local c1, c2 = make('c1', 'uel0201', 320, 310), make('c2', 'uel0201', 325, 310)
    local bomber = make('bomber', 'uea0103', 320, 330)
    brain:AssignUnitsToPlatoon(live, {c1}, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(ended, {c2}, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(falling, {bomber}, 'Attack', 'None')
    IssueMove({bomber}, {700, GetTerrainHeight(700, 700), 700})
    WaitTicks(40)
    local spare = plat('CESPARE')
    brain:AssignUnitsToPlatoon(spare, {c2}, 'Attack', 'None')
    bomber:Kill()
    WaitTicks(3)
    state('CE live', live) state('CE ended', ended) state('CE falling', falling)
    P(fmt('CE bomber dead=%s destroyed=%s enemy dead=%s', tostring(bomber.Dead), tostring(bomber:BeenDestroyed()), tostring(enemy.Dead)))
    census('ended', ended, enemy, spot)
    census('falling', falling, enemy, spot)
    census('live', live, enemy, spot)
    WaitTicks(2)
    state('CE live after', live) state('CE falling after', falling)

    WaitTicks(170)
    LOG('PROBE done')
end)
