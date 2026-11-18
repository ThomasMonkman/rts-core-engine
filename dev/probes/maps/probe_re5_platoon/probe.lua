-- Differential probe (sweep 5): platoon lifetime around emptying, as RNGAI's air platoons use it.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re5_platoon 90
--   ours: --headless --no-ai --ticks 500 --script dev/probes/maps/probe_re5_platoon/probe.lua
-- MG  merge: P's units moved into Q from a thread, then a thread forked on P in that beat calls
--     GetBrain / CalculatePlatoonThreat / GetPlatoonPosition on it. Is P alive for calls that beat, does
--     PlatoonExists / IsDestroyed say so, and on which beat do calls start to raise and OnDestroy run?
-- RF  P emptied and given a unit back in the same beat: does P live on (OnDestroy, exists +3)?
-- AK  air kill: a platoon whose only unit, a flying bomber, is Kill()ed mid-air. Per beat until it has
--     ended: unit Dead / BeenDestroyed, GetPlatoonUnits count, PlatoonExists, IsDestroyed, whether
--     CanAttackTarget / GetPlatoonPosition raise, and when OnDestroy runs (crash vs kill).
-- LK  the same with a T1 tank (baseline against probe_ai_platoon's PK).
-- PL  which emptied platoons survive (FAF's TransportPool is made empty, named, and refilled for the
--     whole game). Every 5 beats: PlatoonExists, OnDestroy, and whether AssignUnitsToPlatoon into it raises.
--     PL1 made empty, left 50 beats, then given a unit. PL2 named ('ProbePool'), its unit moved out,
--     refilled 20 beats later. PL3 unnamed, same as PL2. PL4 named, its only unit killed, refilled 30
--     beats later. PL5 unnamed, only unit killed, refilled 30 beats later.
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function A() return GetGameTick() end
local function P(text) LOG(fmt('PROBE %d %s', A(), text)) end
local brain = ArmyBrains[1]
local function make(bp, x, z)
    local u = CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetFireState(1)
    return u
end
local function plat(name)
    local p = brain:MakePlatoon(name, 'none')
    p:AddDestroyCallback(function() P(name .. ' OnDestroy') end)
    return p
end
local function calls(tag, p, target)
    local okb, b = pcall(function() return p:GetBrain() end)
    local okt, t = pcall(function() return p:CalculatePlatoonThreat('Surface', categories.ALLUNITS) end)
    local okp, pp = pcall(function() return p:GetPlatoonPosition() end)
    local oka, a = true, 'n/a'
    if target then oka, a = pcall(function() return p:CanAttackTarget('attack', target) end) end
    local n = table.getn(p:GetPlatoonUnits() or {})
    P(fmt('%s exists=%s destroyed=%s units=%d brain=%s threat=%s pos=%s canattack=%s', tag,
        tostring(brain:PlatoonExists(p)), tostring(IsDestroyed(p)), n,
        okb and tostring(b == brain) or 'ERR', okt and tostring(t) or 'ERR',
        okp and (pp and 'vec' or 'nil') or 'ERR', oka and tostring(a) or 'ERR'))
end
local function watch(tag, p, beats, target, unit)
    for i = 0, beats do
        if unit then
            P(fmt('%s +%d unit dead=%s destroyed=%s', tag, i, tostring(unit.Dead), tostring(unit:BeenDestroyed())))
        end
        calls(fmt('%s +%d', tag, i), p, target)
        WaitTicks(1)
    end
end

ForkThread(function()
    WaitTicks(5)
    -- MG
    local q, p = plat('MGQ'), plat('MGP')
    local t1, t2 = make('uel0201', 300, 300), make('uel0201', 305, 300)
    brain:AssignUnitsToPlatoon(q, {t1}, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(p, {t2}, 'Attack', 'None')
    WaitTicks(1)
    P('MG merge')
    brain:AssignUnitsToPlatoon(q, p:GetPlatoonUnits(), 'Attack', 'None')
    calls('MG same thread', p)
    ForkThread(function() calls('MG forked', p) end)
    WaitTicks(1)
    watch('MG', p, 3)

    -- RF
    local r = plat('RFP')
    local t3 = make('uel0201', 310, 300)
    brain:AssignUnitsToPlatoon(r, {t3}, 'Attack', 'None')
    WaitTicks(1)
    brain:AssignUnitsToPlatoon(q, {t3}, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(r, {t3}, 'Attack', 'None')
    P('RF refilled')
    watch('RF', r, 3)

    -- AK: a bomber flying toward a far point, killed mid-air
    local enemyTarget = CreateUnitHPR('uel0201', 'ARMY_2', 600, GetTerrainHeight(600, 600), 600, 0, 0, 0)
    local ak = plat('AKP')
    local bomber = make('uea0103', 320, 300)
    brain:AssignUnitsToPlatoon(ak, {bomber}, 'Attack', 'None')
    IssueMove({bomber}, {700, GetTerrainHeight(700, 700), 700})
    WaitTicks(40)
    P(fmt('AK bomber height %.1f', bomber:GetPosition()[2] - GetTerrainHeight(bomber:GetPosition()[1], bomber:GetPosition()[3])))
    bomber:Kill()
    watch('AK', ak, 150, enemyTarget, bomber)

    -- LK
    local lk = plat('LKP')
    local t4 = make('uel0201', 330, 300)
    brain:AssignUnitsToPlatoon(lk, {t4}, 'Attack', 'None')
    WaitTicks(2)
    t4:Kill()
    watch('LK', lk, 12, enemyTarget, t4)

    -- PL
    local hold = plat('PLHOLD')
    local function try_assign(tag, p, u)
        local ok, e = pcall(function() return brain:AssignUnitsToPlatoon(p, {u}, 'Support', 'None') end)
        P(fmt('%s assign ok=%s %s', tag, tostring(ok), ok and '' or tostring(e)))
    end
    local function state(tag, p)
        P(fmt('%s exists=%s destroyed=%s units=%d', tag, tostring(brain:PlatoonExists(p)), tostring(IsDestroyed(p)), table.getn(p:GetPlatoonUnits() or {})))
    end
    local pl1, pl2, pl3, pl4, pl5 = plat('PL1'), plat('PL2'), plat('PL3'), plat('PL4'), plat('PL5')
    pl2:UniquelyNamePlatoon('ProbePool')
    pl4:UniquelyNamePlatoon('ProbePool4')
    local u2, u3, u4, u5 = make('uel0201', 340, 300), make('uel0201', 345, 300), make('uel0201', 350, 300), make('uel0201', 355, 300)
    brain:AssignUnitsToPlatoon(pl2, {u2}, 'Support', 'None')
    brain:AssignUnitsToPlatoon(pl3, {u3}, 'Support', 'None')
    brain:AssignUnitsToPlatoon(pl4, {u4}, 'Support', 'None')
    brain:AssignUnitsToPlatoon(pl5, {u5}, 'Support', 'None')
    WaitTicks(1)
    P('PL empty')
    brain:AssignUnitsToPlatoon(hold, {u2, u3}, 'Attack', 'None')
    u4:Kill()
    u5:Kill()
    for i = 0, 50, 5 do
        for k, p in {PL1 = pl1, PL2 = pl2, PL3 = pl3, PL4 = pl4, PL5 = pl5} do state(fmt('%s +%d', k, i), p) end
        if i == 20 then
            try_assign('PL2 refill', pl2, u2)
            try_assign('PL3 refill', pl3, u3)
        end
        if i == 30 then
            try_assign('PL4 refill', pl4, make('uel0201', 360, 300))
            try_assign('PL5 refill', pl5, make('uel0201', 365, 300))
        end
        WaitTicks(5)
    end
    try_assign('PL1 fill', pl1, make('uel0201', 370, 300))
    WaitTicks(5)
    for k, p in {PL1 = pl1, PL2 = pl2, PL3 = pl3, PL4 = pl4, PL5 = pl5} do state(k .. ' end', p) end
    P(fmt('PL names ProbePool=%s ProbePool4=%s', tostring(brain:GetPlatoonUniquelyNamed('ProbePool') == pl2), tostring(brain:GetPlatoonUniquelyNamed('ProbePool4') == pl4)))

    WaitTicks(30)
    LOG('PROBE done')
end)
