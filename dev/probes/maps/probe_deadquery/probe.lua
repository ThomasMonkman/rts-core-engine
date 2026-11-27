-- Probe: do the unit queries return a unit that was killed (dead, not yet
-- destroyed), and what do engine methods do on a dead / destroyed unit?
local armies = ListArmies()
local brain = GetArmyBrain(armies[1])
local w, h = GetMapSize()
local x, z = w / 2, h / 2
local function has(list, u)
    local n, found = 0, false
    for _, v in list or {} do n = n + 1 if v == u then found = true end end
    return tostring(found) .. '/' .. n
end
local function call(u, name)
    local ok, r = pcall(function() return u[name](u) end)
    if not ok then return 'ERR(' .. string.sub(tostring(r), 1, 60) .. ')' end
    if type(r) == 'table' then return 'table' end
    return tostring(r)
end
local function probe(tag, u, cat, t0)
    LOG('PROBE ', tag, ' dt=', GetGameTick() - t0,
        ' aroundAlly=', has(brain:GetUnitsAroundPoint(cat, {x, 0, z}, 50, 'Ally'), u),
        ' around=', has(brain:GetUnitsAroundPoint(cat, {x, 0, z}, 50), u),
        ' num=', brain:GetNumUnitsAroundPoint(cat, {x, 0, z}, 50, 'Ally'),
        ' list=', has(brain:GetListOfUnits(cat, false), u),
        ' rect=', has(GetUnitsInRect(Rect(x - 20, z - 20, x + 20, z + 20)), u),
        ' count=', brain:GetCurrentUnits(cat),
        ' Dead=', tostring(u.Dead), ' BeenDestroyed=', call(u, 'BeenDestroyed'), ' IsDead=', call(u, 'IsDead'),
        ' GetBlueprint=', call(u, 'GetBlueprint'), ' GetPosition=', call(u, 'GetPosition'),
        ' GetHealth=', call(u, 'GetHealth'), ' GetFractionComplete=', call(u, 'GetFractionComplete'),
        ' GetAIBrain=', call(u, 'GetAIBrain'), ' IsUnitState=', (pcall(function() return u:IsUnitState('Moving') end)) and 'ok' or 'ERR')
end
ForkThread(function()
    WaitTicks(10)
    for _, case in { { 'radar-kill', 'ueb3101', categories.RADAR, 'Kill' }, { 'radar-destroy', 'ueb3101', categories.RADAR, 'Destroy' }, { 'tank-kill', 'uel0201', categories.TANK, 'Kill' } } do
        local u = CreateUnitHPR(case[2], armies[1], x, GetTerrainHeight(x, z), z, 0, 0, 0)
        WaitTicks(5)
        local t0 = GetGameTick()
        probe(case[1] .. ' before', u, case[3], t0)
        if case[4] == 'Kill' then u:Kill() else u:Destroy() end
        probe(case[1] .. ' same-tick', u, case[3], t0)
        for i = 1, 8 do WaitTicks(1) probe(case[1], u, case[3], t0) end
        WaitTicks(150)
        probe(case[1] .. ' late', u, case[3], t0)
    end
    LOG('PROBE done')
end)
