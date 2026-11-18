-- Differential probe: where engineers stop to work, engine unit placement,
-- wrecks, props and patrol reclaim, in a format both engines print
-- identically ("PROBE ..." lines).
--   real game (Windows): dev/probes/run_faf.ps1 probe_unit_misc 120
--   ours: cargo run -- --headless --no-ai --ticks 420 --script dev/probes/maps/probe_unit_misc/probe.lua
-- Everything starts at tick 15 (FAF reveals the civilian units that exist
-- at tick 10). Army 1; targets to capture are NEUTRAL_CIVILIAN (enemy).
-- W  work distance: a T1 engineer (build range 5) 30 west of its target is
--    ordered to reclaim / repair / capture / build; logged: the tick its
--    work state comes on and its distance to the target's centre then, 10
--    and 40 ticks later. Targets: a tank, a generator (2x2), a land factory (8x8).
-- S  brain:CreateUnitNearSpot and CreateResourceBuildingNearest: where the
--    units end up.
-- K  a tank killed: what GetReclaimablesInRect finds there 80 ticks later.
-- R  props: CreateProp / CreatePropHPR, their values, the reclaimables list.
-- A  patrol and aggressive move through three props worth reclaiming.
-- C  GetCargo order: three tanks attached in reverse order of creation.
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
local function P(who, text) LOG(fmt('PROBE %d %s %s', GetGameTick() - t0, who, text)) end
local function make(bp, army, x, z)
    return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function dist(a, b)
    local dx, dz = a[1] - b[1], a[3] - b[3]
    return math.sqrt(dx * dx + dz * dz)
end
local function try(who, label, f)
    local ok, a, b = pcall(f)
    if ok then
        P(who, label .. ' = ' .. tostring(a) .. (b ~= nil and (' , ' .. tostring(b)) or ''))
    else
        P(who, label .. ' ERR | ' .. string.sub(tostring(a), 1, 80))
    end
end
local SITES = { { 608, 80 }, { 688, 80 }, { 912, 96 }, { 784, 144 }, { 656, 160 }, { 736, 224 }, { 816, 224 }, { 912, 240 }, { 672, 304 }, { 800, 336 },
    { 960, 336 }, { 656, 384 }, { 880, 400 }, { 576, 464 }, { 384, 560 }, { 96, 608 }, { 352, 640 }, { 64, 688 }, { 176, 688 }, { 320, 720 },
    { 224, 768 }, { 112, 784 }, { 304, 800 }, { 240, 880 }, { 352, 880 }, { 96, 896 }, { 432, 912 } }

-- W
local function workLane(name, site, state, setup)
    ForkThread(function()
        WaitTicks(15)
        local x, z = SITES[site][1], SITES[site][2]
        local e = make('uel0105', A1, x - 15, z)
        local centre = setup(e, x + 15, z)
        local started
        for i = 1, 260 do
            WaitTicks(1)
            if e:BeenDestroyed() then break end
            if not started and e:IsUnitState(state) then
                started = i
                local f = e:GetFocusUnit()
                P('W ' .. name, fmt('%s on: dist %.2f moving %s focus %s', state, dist(e:GetPosition(), centre), tostring(e:IsUnitState('Moving')), tostring(f ~= nil)))
            end
            if started and (i == started + 10 or i == started + 40) then
                P('W ' .. name, fmt('+%d: dist %.2f moving %s %s', i - started, dist(e:GetPosition(), centre), tostring(e:IsUnitState('Moving')), tostring(e:IsUnitState(state))))
                if i == started + 40 then break end
            end
        end
        if not started then P('W ' .. name, fmt('never %s: dist %.2f queue %d', state, dist(e:GetPosition(), centre), table.getn(e:GetCommandQueue()))) end
    end)
end
local function target(bp, army, hp)
    return function(x, z)
        local u = make(bp, army, x, z)
        if hp then u:SetHealth(nil, hp) end
        return u
    end
end
local function unitLane(name, site, state, order, mk)
    workLane(name, site, state, function(e, x, z)
        local u = mk(x, z)
        order({ e }, u)
        return u:GetPosition()
    end)
end
unitLane('reclaim-tank', 1, 'Reclaiming', IssueReclaim, target('uel0201', A1))
unitLane('reclaim-pgen', 2, 'Reclaiming', IssueReclaim, target('ueb1101', A1))
unitLane('reclaim-factory', 3, 'Reclaiming', IssueReclaim, target('ueb0101', A1))
unitLane('repair-tank', 4, 'Repairing', IssueRepair, target('uel0201', A1, 50))
unitLane('repair-pgen', 5, 'Repairing', IssueRepair, target('ueb1101', A1, 50))
unitLane('repair-factory', 6, 'Repairing', IssueRepair, target('ueb0101', A1, 50))
unitLane('capture-tank', 7, 'Capturing', IssueCapture, function(x, z)
    local u = make('uel0201', CIV, x, z)
    u:SetFireState(1)
    return u
end)
unitLane('capture-pgen', 8, 'Capturing', IssueCapture, target('ueb1101', CIV))
unitLane('capture-factory', 9, 'Capturing', IssueCapture, target('ueb0101', CIV))
workLane('build-pgen', 10, 'Building', function(e, x, z)
    IssueBuildMobile({ e }, pos(x, z), 'ueb1101', {})
    return pos(x, z)
end)
workLane('build-factory', 11, 'Building', function(e, x, z)
    IssueBuildMobile({ e }, pos(x, z), 'ueb0101', {})
    return pos(x, z)
end)
workLane('reclaim-prop', 12, 'Reclaiming', function(e, x, z)
    local prop = CreateProp(pos(x, z), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
    prop:SetMaxReclaimValues(30, 40, 0)
    IssueReclaim({ e }, prop)
    return prop:GetPosition()
end)
workLane('assist-builder', 13, 'Building', function(e, x, z)
    local b = make('uel0105', A1, x - 3, z)
    IssueBuildMobile({ b }, pos(x, z), 'ueb0101', {})
    IssueGuard({ e }, b)
    return pos(x, z)
end)

-- S
ForkThread(function()
    WaitTicks(15)
    local x, z = SITES[14][1], SITES[14][2]
    local function spot(label, bp, sx, sz)
        local ok, u = pcall(function() return brain:CreateUnitNearSpot(bp, sx, sz) end)
        if ok and u then
            local p = u:GetPosition()
            P('S', fmt('%s %s at %.2f %.2f -> %.2f %.2f heading %.2f', label, bp, sx - x, sz - z, p[1] - x, p[3] - z, u:GetHeading()))
        else
            P('S', fmt('%s %s failed %s', label, bp, string.sub(tostring(u), 1, 60)))
        end
        return ok and u
    end
    spot('free', 'ueb1101', x, z)
    spot('taken', 'ueb1101', x, z)
    spot('taken2', 'ueb1101', x, z)
    spot('taken3', 'ueb1101', x, z)
    spot('offgrid', 'ueb1101', x + 20.3, z + 0.7)
    spot('big', 'ueb0101', x, z + 20)
    spot('big-taken', 'ueb0101', x, z + 20)
    local t = make('uel0201', A1, x + 40, z)
    spot('on-tank', 'ueb1101', x + 40, z)
    spot('mobile', 'uel0201', x + 40, z + 10)
    spot('mobile2', 'uel0201', x + 40, z + 10)
    for i = 1, 3 do
        local ok, u = pcall(function() return brain:CreateResourceBuildingNearest('ueb1103', x, z) end)
        P('S', 'mex ' .. i .. ' ' .. (ok and u and fmt('%.1f %.1f', u:GetPosition()[1], u:GetPosition()[3]) or 'failed ' .. string.sub(tostring(u), 1, 60)))
    end
    local ok, u = pcall(function() return brain:CreateResourceBuildingNearest('ueb1102', x, z) end)
    P('S', 'hydro ' .. (ok and u and fmt('%.1f %.1f', u:GetPosition()[1], u:GetPosition()[3]) or 'failed ' .. string.sub(tostring(u), 1, 60)))
    try('S', 'CanBuildStructureAt free', function() return brain:CanBuildStructureAt('ueb1101', pos(x + 60, z)) end)
    try('S', 'CanBuildStructureAt taken', function() return brain:CanBuildStructureAt('ueb1101', pos(x, z)) end)
    try('S', 'CanBuildStructureAt mex off deposit', function() return brain:CanBuildStructureAt('ueb1103', pos(x + 60, z)) end)
    try('S', 'CanBuildStructureAt on tank', function() return brain:CanBuildStructureAt('ueb1101', t:GetPosition()) end)
end)

-- K, R, C
ForkThread(function()
    WaitTicks(15)
    local x, z = SITES[15][1], SITES[15][2]
    local function listing(label, x0, z0, x1, z1)
        local ok, list = pcall(GetReclaimablesInRect, Rect(x0, z0, x1, z1))
        if not ok then P('R', label .. ' ERR') return end
        local parts = {}
        for _, r in list or {} do
            local id = '?'
            pcall(function() id = string.gsub(tostring(r:GetBlueprint().BlueprintId), '.*/', '') end)
            local p = r:GetPosition()
            table.insert(parts, fmt('%s@%.1f,%.1f mass %s energy %s time %s isprop %s', id, p[1] - x, p[3] - z, tostring(r.MaxMassReclaim), tostring(r.MaxEnergyReclaim),
                tostring(r.TimeReclaim), tostring(IsProp and IsProp(r))))
        end
        table.sort(parts)
        P('R', label .. ' ' .. (list and table.getn(list) or 'nil') .. ': ' .. table.concat(parts, ' ; '))
    end
    listing('empty before', x - 6, z - 6, x + 6, z + 6)
    local victim = make('uel0201', A1, x, z)
    local own = make('uel0201', A1, x + 3, z)
    listing('with two tanks', x - 6, z - 6, x + 6, z + 6)
    WaitTicks(3)
    victim:Kill()
    WaitTicks(80)
    listing('after kill', x - 6, z - 6, x + 6, z + 6)
    own:Destroy()
    -- props
    local px, pz = x + 30, z
    local ok, prop = pcall(CreateProp, pos(px, pz), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
    P('R', 'CreateProp ' .. tostring(ok and prop ~= nil))
    if ok and prop then
        try('R', 'prop bp id', function() return prop:GetBlueprint().BlueprintId end)
        try('R', 'prop pos', function() local p = prop:GetPosition() return fmt('%.2f %.2f %.2f', p[1] - px, p[2] - GetTerrainHeight(px, pz), p[3] - pz) end)
        try('R', 'prop values', function() return fmt('%s %s %s %s', tostring(prop.MaxMassReclaim), tostring(prop.MaxEnergyReclaim), tostring(prop.TimeReclaim), tostring(prop.ReclaimLeft)) end)
        try('R', 'prop id type', function() return type(prop:GetEntityId()) end)
        try('R', 'prop health', function() return prop:GetHealth(), prop:GetMaxHealth() end)
        try('R', 'prop IsProp IsUnit IsEntity', function() return fmt('%s %s %s', tostring(IsProp(prop)), tostring(IsUnit(prop)), tostring(IsEntity(prop))) end)
        try('R', 'prop army', function() return prop:GetArmy() end)
        try('R', 'prop GetEntityById', function() return GetEntityById(prop:GetEntityId()) == prop end)
        try('R', 'prop SetMaxReclaimValues', function() prop:SetMaxReclaimValues(7, 33, 44) return fmt('%s %s %s', tostring(prop.MaxMassReclaim), tostring(prop.MaxEnergyReclaim), tostring(prop.TimeReclaim)) end)
        try('R', 'prop SetReclaimable? AdjustHealth', function() prop:AdjustHealth(nil, -10) return prop:GetHealth() end)
        try('R', 'prop BeenDestroyed', function() return prop:BeenDestroyed() end)
    end
    local ok2, prop2 = pcall(CreatePropHPR, '/env/evergreen/props/rocks/fieldstone01_prop.bp', px + 4.3, GetTerrainHeight(px + 4.3, pz) + 2, pz + 0.6, 1, 0, 0)
    if ok2 and prop2 then
        try('R', 'propHPR pos', function() local p = prop2:GetPosition() return fmt('%.2f %.2f %.2f', p[1] - px, p[2] - GetTerrainHeight(px + 4.3, pz), p[3] - pz) end)
        try('R', 'propHPR orientation', function() local o = prop2:GetOrientation() return fmt('%.3f %.3f %.3f %.3f', o[1], o[2], o[3], o[4]) end)
    else
        P('R', 'CreatePropHPR failed')
    end
    local ok3, tree = pcall(CreateProp, pos(px + 8, pz), '/env/evergreen/props/trees/pine06_prop.bp')
    if ok3 and tree then
        try('R', 'tree values', function() return fmt('%s %s %s', tostring(tree.MaxMassReclaim), tostring(tree.MaxEnergyReclaim), tostring(tree.TimeReclaim)) end)
    end
    try('R', 'bogus prop', function() return CreateProp(pos(px + 12, pz), '/env/nosuch_prop.bp') ~= nil end)
    listing('props', px - 3, pz - 3, px + 10, pz + 3)
    if ok and prop then
        prop:Destroy()
        try('R', 'prop destroyed', function() return prop:BeenDestroyed() end)
    end
    listing('props after destroy', px - 3, pz - 3, px + 10, pz + 3)
    -- C
    local tr = make('uea0107', A1, x, z + 30)
    local riders = {}
    for i = 1, 3 do riders[i] = make('uel0201', A1, x + 4 * i, z + 36) end
    local bones = {}
    for i = 0, tr:GetBoneCount() - 1 do
        local n = tr:GetBoneName(i)
        if n and string.find(n, 'ttach') then table.insert(bones, n) end
    end
    P('C', 'bones ' .. table.concat(bones, ','))
    for i = 3, 1, -1 do
        pcall(function() riders[i]:AttachBoneTo(-1, tr, bones[4 - i]) end)
    end
    try('C', 'cargo order', function()
        local s = ''
        for _, c in tr:GetCargo() do
            for i = 1, 3 do
                if riders[i] == c then s = s .. i end
            end
        end
        return s
    end)
end)

-- A
ForkThread(function()
    WaitTicks(15)
    for lane, order in { IssuePatrol, IssueAggressiveMove, IssueMove } do
        local x, z = SITES[15 + lane][1], SITES[15 + lane][2]
        local name = ({ 'patrol', 'aggressive', 'move' })[lane]
        local e = make('uel0105', A1, x - 15, z)
        local props = {}
        for i = 1, 3 do
            local ok, p = pcall(CreateProp, pos(x - 15 + 8 * i, z + 2), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
            if ok and p then
                p:SetMaxReclaimValues(2, 20, 0)
                props[i] = p
            end
        end
        order({ e }, pos(x + 15, z))
        ForkThread(function(name, e, props)
            local gone = {}
            for t = 1, 300 do
                WaitTicks(1)
                for i = 1, 3 do
                    if props[i] and not gone[i] and props[i]:BeenDestroyed() then
                        gone[i] = true
                        P('A ' .. name, fmt('prop %d reclaimed, engineer at %.1f', i, e:GetPosition()[1] - props[i]:GetPosition()[1]))
                    end
                end
                if t == 150 or t == 300 then
                    P('A ' .. name, fmt('+%d engineer x %.1f queue %d reclaiming %s', t, e:GetPosition()[1], table.getn(e:GetCommandQueue()), tostring(e:IsUnitState('Reclaiming'))))
                end
            end
        end, name, e, props)
    end
end)

ForkThread(function()
    WaitTicks(340)
    WaitTicks(30)
    LOG('PROBE done')
end)
