-- Differential probe: the brain's build calls: CanBuildStructureAt (other
-- structures, deposits, water, the map border, slopes), DecideWhatToBuild,
-- FindPlaceToBuild (result shape, relative / override / resource types),
-- BuildStructure, factories (GetAvailableFactories, CanBuildPlatoon,
-- BuildPlatoon, BuildUnit).
--   real game: dev/probes/run_faf.ps1 probe_ai_build 120
--   ours: --headless --no-ai --ticks 560 --script dev/probes/maps/probe_ai_build/probe.lua
-- Lines: PROBE <tick> <case> ok n=<results> <values> | ERR <message>.
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local TESTER = armies[1]
local ENEMY = 'NEUTRAL_CIVILIAN'
local enemyIndex
for i, a in armies do
    if a == ENEMY then enemyIndex = i end
end
SetAlliance(TESTER, ENEMY, 'Enemy')
SetAlliance(ENEMY, TESTER, 'Enemy')
local brain = ArmyBrains[1]
local ebrain = ArmyBrains[enemyIndex]

-- Names for the objects the probe made (units, platoons, brains), so both
-- engines print them the same.
local tags = {}
for i, b in ArmyBrains do tags[b] = 'brain' .. i end
local function N(v)
    if type(v) ~= 'number' then return tostring(v) end
    if math.abs(v) < 0.005 then v = 0 end
    return fmt('%.2f', v)
end
local S
local function S0(v, d)
    d = d or 0
    local t = type(v)
    if t == 'number' then return N(v) end
    if t == 'string' then return '"' .. v .. '"' end
    if t ~= 'table' then
        if t == 'userdata' or t == 'function' or t == 'thread' then return t end
        return tostring(v)
    end
    if tags[v] then return tags[v] end
    local ok, id = pcall(function() return v.GetUnitId and v:GetUnitId() end)
    if ok and id then return 'unit:' .. id end
    if rawget(v, '_c_object') ~= nil then return 'cobj' end
    if d > 3 then return '{..}' end
    local out = {}
    local n = table.getn(v)
    for i = 1, n do table.insert(out, S(v[i], d + 1)) end
    local keys = {}
    for k in v do
        if not (type(k) == 'number' and k >= 1 and k <= n) then table.insert(keys, tostring(k)) end
    end
    table.sort(keys)
    for _, k in keys do
        local val = v[k]
        if val == nil then val = v[tonumber(k)] end
        table.insert(out, k .. '=' .. S(val, d + 1))
    end
    return '{' .. table.concat(out, ',') .. '}'
end
S = function(v, d)
    local ok, r = pcall(S0, v, d)
    if ok then return r end
    return '<unprintable ' .. type(v) .. '>'
end
local start = GetGameTick()
local function T() return GetGameTick() - start end
-- Calls f, logs its results (up to the last non-nil of 6) or its error.
local function R(tag, f)
    local ok, a, b, c, d, e, g = pcall(f)
    if ok then
        local r = { a, b, c, d, e, g }
        local n = 0
        for i = 1, 6 do if r[i] ~= nil then n = i end end
        local line = 'PROBE ' .. T() .. ' ' .. tag .. ' ok n=' .. n
        for i = 1, n do line = line .. ' ' .. S(r[i]) end
        LOG(line)
    else
        LOG('PROBE ' .. T() .. ' ' .. tag .. ' ERR ' .. tostring(a))
    end
    return a, b
end
local function make(name, bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    pcall(function() u:SetFireState(1) end)
    if name then tags[u] = name end
    return u
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function wait_to(t)
    while T() < t do WaitTicks(1) end
end

local C = categories
local QX, QZ = 656, 160
local Q = pos(QX, QZ)
local pg = make('pgen', 'ueb1101', TESTER, QX + 10, QZ)       -- 2x2 structure
local tank = make('tank', 'uel0201', TESTER, QX + 10, QZ + 10)
local eng1 = make('eng1', 'uel0105', TESTER, QX - 6, QZ)
local eng2 = make('eng2', 'uel0105', TESTER, QX - 6, QZ + 6)
local eng3 = make('eng3', 'uel0105', TESTER, QX - 6, QZ - 6)
local eng4 = make('eng4', 'uel0105', TESTER, QX - 6, QZ + 12)
local fac1 = make('fac1', 'ueb0101', TESTER, QX - 30, QZ - 10)
local fac2 = make('fac2', 'ueb0101', TESTER, QX - 30, QZ + 10)
local epg = make('epg', 'ueb1101', ENEMY, 352, 880)            -- enemy structure, never seen
SetAlliance(TESTER, armies[2], 'Ally')
SetAlliance(armies[2], TESTER, 'Ally')
local apg = make('apg', 'ueb1101', armies[2], QX + 10, QZ - 10) -- allied structure

-- Mass / hydrocarbon markers (the scenario's data), nearest to the site first.
local mass, hydro = {}, {}
pcall(function()
    for name, m in Scenario.MasterChain._MASTERCHAIN_.Markers do
        if m.type == 'Mass' then table.insert(mass, m.position) end
        if m.type == 'Hydrocarbon' then table.insert(hydro, m.position) end
    end
end)
local function d2(p) return (p[1] - QX) * (p[1] - QX) + (p[3] - QZ) * (p[3] - QZ) end
local function near(a, b)
    local da, db = d2(a), d2(b)
    if da ~= db then return da < db end
    if a[1] ~= b[1] then return a[1] < b[1] end
    return a[3] < b[3]
end
table.sort(mass, near)
table.sort(hydro, near)

local function can(tag, bp, p)
    R('can ' .. tag, function() return brain:CanBuildStructureAt(bp, p) end)
end
local function structures_near(tag, p, r)
    local list = brain:GetUnitsAroundPoint(C.STRUCTURE, p, r, 'Ally') or {}
    local line = 'PROBE ' .. T() .. ' structures ' .. tag
    for _, u in list do
        local x, y, z = u:GetPositionXYZ()
        line = line .. ' ' .. (tags[u] or u:GetUnitId()) .. '@' .. N(x) .. ',' .. N(z) .. '/' .. N(u:GetFractionComplete())
    end
    LOG(line)
end
local function queue(tag, u)
    local q = u:GetCommandQueue()
    local line = 'PROBE ' .. T() .. ' queue ' .. tag .. ' n=' .. table.getn(q)
    for i, c in q do
        if i <= 4 then line = line .. ' [' .. tostring(c.commandType) .. ' ' .. N(c.x) .. ',' .. N(c.z) .. ']' end
    end
    LOG(line)
end

ForkThread(function()
    LOG('PROBE 0 setup mass=' .. table.getn(mass) .. ' hydro=' .. table.getn(hydro))
    wait_to(3)
    brain:GiveStorage('MASS', 100000)
    brain:GiveStorage('ENERGY', 100000)
    brain:GiveResource('MASS', 100000)
    brain:GiveResource('ENERGY', 100000)
    -- CanBuildStructureAt
    can('flat empty pgen', 'ueb1101', pos(QX, QZ + 30))
    can('flat empty factory', 'ueb0101', pos(QX, QZ + 30))
    can('uppercase id', 'UEB1101', pos(QX, QZ + 30))
    can('y=0', 'ueb1101', { QX, 0, QZ + 30 })
    can('fractional pos', 'ueb1101', pos(QX + 0.5, QZ + 30.5))
    for dx = 0, 4 do can('pgen over pgen dx=' .. dx, 'ueb1101', pos(QX + 10 + dx, QZ)) end
    for dx = 0, 8 do can('factory by pgen dx=' .. dx, 'ueb0101', pos(QX + 10 + dx, QZ)) end
    for dx = 0, 3 do can('wall by pgen dx=' .. dx, 'ueb5101', pos(QX + 10 + dx, QZ)) end
    for dx = 4, 12, 1 do can('factory by factory dx=' .. dx, 'ueb0101', pos(QX - 30 + dx, QZ - 10)) end
    for dx = 4, 10, 1 do can('pgen by factory dx=' .. dx, 'ueb1101', pos(QX - 30 + dx, QZ - 10)) end
    can('on mobile tank', 'ueb1101', pos(QX + 10, QZ + 10))
    can('on engineer', 'ueb1101', pos(QX - 6, QZ))
    can('on allied pgen', 'ueb1101', pos(QX + 10, QZ - 10))
    can('on unseen enemy pgen', 'ueb1101', pos(352, 880))
    can('land pgen in water', 'ueb1101', pos(208, 128))
    can('naval factory in water', 'ueb0103', pos(208, 128))
    can('naval factory on land', 'ueb0103', pos(QX, QZ + 30))
    can('sonar in water', 'ueb3102', pos(208, 128))
    can('mex off deposit', 'ueb1103', pos(QX, QZ + 30))
    can('hydro off deposit', 'ueb1102', pos(QX, QZ + 30))
    can('mobile unit bp', 'uel0201', pos(QX, QZ + 30))
    can('mobile unit bp on pgen', 'uel0201', pos(QX + 10, QZ))
    can('mobile land unit in water', 'uel0201', pos(208, 128))
    for i = 1, 3 do
        if mass[i] then
            LOG('PROBE ' .. T() .. ' mass' .. i .. ' ' .. S(mass[i]))
            can('mex on mass' .. i, 'ueb1103', mass[i])
            can('mex 1 off mass' .. i, 'ueb1103', { mass[i][1] + 1, mass[i][2], mass[i][3] })
            can('mex 0.4 off mass' .. i, 'ueb1103', { mass[i][1] + 0.4, mass[i][2], mass[i][3] + 0.4 })
            can('pgen on mass' .. i, 'ueb1101', mass[i])
            can('pgen 1 off mass' .. i, 'ueb1101', { mass[i][1] + 1, mass[i][2], mass[i][3] })
            can('pgen 2 off mass' .. i, 'ueb1101', { mass[i][1] + 2, mass[i][2], mass[i][3] })
            can('hydro on mass' .. i, 'ueb1102', mass[i])
        end
    end
    if hydro[1] then
        LOG('PROBE ' .. T() .. ' hydro1 ' .. S(hydro[1]))
        can('hydro on hydro1', 'ueb1102', hydro[1])
        can('mex on hydro1', 'ueb1103', hydro[1])
        can('pgen on hydro1', 'ueb1101', hydro[1])
    end
    -- map border
    for _, d in { 1, 2, 4, 6, 7, 8, 9, 10, 12 } do
        can('pgen edge x=' .. d, 'ueb1101', pos(d, QZ))
        can('sonar edge x=' .. d, 'ueb3102', pos(d, 128))
        can('sonar edge z=' .. d, 'ueb3102', pos(208, d))
        can('sonar far edge x=1024-' .. d, 'ueb3102', pos(1024 - d, 128))
    end
    can('offmap', 'ueb1101', { -20, 0, -20 })
    -- slope: the first point (scan order) on land whose height range over a
    -- factory-sized square reaches each threshold
    local found = {}
    local TH = { 0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 2.5, 3, 4, 6 }
    for x = 524, 800, 4 do
        for z = 44, 300, 4 do
            local lo, hi = 1000, -1000
            for dx = -4, 4, 2 do
                for dz = -4, 4, 2 do
                    local h = GetTerrainHeight(x + dx, z + dz)
                    if h < lo then lo = h end
                    if h > hi then hi = h end
                end
            end
            if lo > GetSurfaceHeight(x, z) - 0.01 and lo > 17.6 then
                for i, th in TH do
                    if not found[i] and hi - lo >= th and (TH[i + 1] == nil or hi - lo < TH[i + 1]) then found[i] = { x, z, hi - lo } end
                end
            end
        end
    end
    for i, th in TH do
        local f = found[i]
        if f then
            LOG('PROBE ' .. T() .. ' slope th=' .. th .. ' at ' .. f[1] .. ',' .. f[2] .. ' range=' .. N(f[3]))
            can('slope th=' .. th .. ' factory', 'ueb0101', pos(f[1], f[2]))
            can('slope th=' .. th .. ' pgen', 'ueb1101', pos(f[1], f[2]))
            can('slope th=' .. th .. ' wall', 'ueb5101', pos(f[1], f[2]))
            can('slope th=' .. th .. ' t3pgen', 'ueb1301', pos(f[1], f[2]))
        end
    end

    -- DecideWhatToBuild
    local BT = { { 'T1LandFactory', 'ueb0101' }, { 'T1Resource', 'ueb1103' }, { 'T1EnergyProduction', 'ueb1101' }, { 'T3EnergyProduction', 'ueb1301' }, { 'T1Sonar', 'ueb3102' }, { 'Cybran', 'urb1101' }, { 'Mobile', 'uel0201' } }
    for _, ty in { 'T1LandFactory', 'T1Resource', 'T1EnergyProduction', 'T3EnergyProduction', 'T1Sonar', 'Cybran', 'Mobile', 'Missing', 't1landfactory' } do
        R('decide eng ' .. ty, function() return brain:DecideWhatToBuild(eng1, ty, BT) end)
    end
    R('decide tank T1LandFactory', function() return brain:DecideWhatToBuild(tank, 'T1LandFactory', BT) end)
    R('decide factory Mobile', function() return brain:DecideWhatToBuild(fac1, 'Mobile', BT) end)
    R('decide dup first buildable', function() return brain:DecideWhatToBuild(eng1, 'X', { { 'X', 'ueb1301' }, { 'X', 'ueb1101' }, { 'X', 'ueb0101' } }) end)
    R('decide multi ids in row', function() return brain:DecideWhatToBuild(eng1, 'X', { { 'X', 'ueb1301', 'ueb1101' } }) end)
    AddBuildRestriction(1, C.ueb0101)
    R('decide restricted T1LandFactory', function() return brain:DecideWhatToBuild(eng1, 'T1LandFactory', BT) end)
    R('eng CanBuild restricted', function() return eng1:CanBuild('ueb0101') end)
    RemoveBuildRestriction(1, C.ueb0101)
    R('decide unrestricted T1LandFactory', function() return brain:DecideWhatToBuild(eng1, 'T1LandFactory', BT) end)

    -- FindPlaceToBuild
    local function tmpl(ty, pts)
        local row = { { ty } }
        for _, p in pts do table.insert(row, p) end
        return { row }
    end
    local absT = tmpl('T1EnergyProduction', { { QX + 10, QZ, 0 }, { QX + 40, QZ + 2, 0 }, { QX + 20, QZ + 30, 0 }, { QX, QZ + 30, 0 } })
    R('find abs eng1', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', absT, false, eng1) end)
    R('find abs nil builder', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', absT, false, nil) end)
    R('find abs override far', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', absT, false, eng1, nil, QX + 40, QZ) end)
    R('find abs type not in template', function() return brain:FindPlaceToBuild('T1LandFactory', 'ueb0101', absT, false, eng1) end)
    R('find abs all blocked', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', tmpl('T1EnergyProduction', { { QX + 10, QZ, 0 }, { 208, 128, 0 } }), false, eng1) end)
    local relT = tmpl('T1EnergyProduction', { { 0, 0, 0 }, { 6, 0, 0 }, { -8, 4, 0 }, { 0, 12, 0 }, { 40, 40, 0 } })
    R('find rel no override', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', relT, true, eng1) end)
    R('find rel override Q+10', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', relT, true, eng1, nil, QX + 10, QZ) end)
    R('find rel override Q+10 nil builder', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', relT, true, nil, nil, QX + 10, QZ) end)
    R('find rel override enemy alliance threat5', function() return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', relT, true, eng1, 'Enemy', QX + 10, QZ, 5) end)
    R('find two rows', function()
        return brain:FindPlaceToBuild('T1EnergyProduction', 'ueb1101', { { { 'T1LandFactory' }, { QX, QZ + 30, 0 } }, { { 'T1AirFactory', 'T1EnergyProduction' }, { QX + 20, QZ + 30, 0 } } }, false, eng1)
    end)
    R('find resource mex eng1', function() return brain:FindPlaceToBuild('T1Resource', 'ueb1103', absT, false, eng1) end)
    R('find resource mex rel', function() return brain:FindPlaceToBuild('T1Resource', 'ueb1103', relT, true, eng1) end)
    R('find resource mex override', function() return brain:FindPlaceToBuild('T1Resource', 'ueb1103', relT, true, eng1, nil, 352, 880) end)
    R('find resource mex abs override', function() return brain:FindPlaceToBuild('T1Resource', 'ueb1103', absT, false, eng1, nil, 352, 880) end)
    R('find resource hydro', function() return brain:FindPlaceToBuild('T1HydroCarbon', 'ueb1102', absT, false, eng1) end)
    R('find resource T2Resource', function() return brain:FindPlaceToBuild('T2Resource', 'ueb1202', absT, false, eng1) end)
    R('find resource type with pgen bp', function() return brain:FindPlaceToBuild('T1Resource', 'ueb1101', absT, false, eng1) end)

    -- other creators
    local cu = R('CreateUnitNearSpot pgen', function() return brain:CreateUnitNearSpot('ueb1101', QX + 60, QZ + 40) end)
    if type(cu) == 'table' then R('CreateUnitNearSpot pos', function() return cu:GetPosition(), cu:GetFractionComplete() end) end
    local cu2 = R('CreateUnitNearSpot same spot', function() return brain:CreateUnitNearSpot('ueb1101', QX + 60, QZ + 40) end)
    if type(cu2) == 'table' then R('CreateUnitNearSpot same spot pos', function() return cu2:GetPosition() end) end
    local cr = R('CreateResourceBuildingNearest mex', function() return brain:CreateResourceBuildingNearest('ueb1103', QX, QZ) end)
    if type(cr) == 'table' then R('CreateResourceBuildingNearest pos', function() return cr:GetPosition() end) end
    local cr2 = R('CreateResourceBuildingNearest mex again', function() return brain:CreateResourceBuildingNearest('ueb1103', QX, QZ) end)
    if type(cr2) == 'table' then R('CreateResourceBuildingNearest again pos', function() return cr2:GetPosition() end) end
    R('FindUpgradeBP', function() return brain:FindUpgradeBP('ueb0101', { { 'ueb0101', 'ueb0201' }, { 'ueb1103', 'ueb1202' } }) end)
    R('FindUpgradeBP missing', function() return brain:FindUpgradeBP('ueb1101', { { 'ueb0101', 'ueb0201' } }) end)

    -- BuildStructure
    wait_to(6)
    R('BuildStructure eng1 abs {x,z,0}', function() return brain:BuildStructure(eng1, 'ueb1101', { QX + 1, QZ + 31, 0 }, false) end)
    R('BuildStructure eng2 relative {4,6,0}', function() return brain:BuildStructure(eng2, 'ueb1101', { 4, 6, 0 }, true) end)
    R('BuildStructure eng3 blocked', function() return brain:BuildStructure(eng3, 'ueb1101', { QX + 10, QZ, 0 }, false) end)
    R('BuildStructure eng4 {x,y,z}', function() return brain:BuildStructure(eng4, 'ueb1101', pos(QX + 4.5, QZ + 44.5), false) end)
    R('BuildStructure tank', function() return brain:BuildStructure(tank, 'ueb1101', { QX + 20, QZ + 40, 0 }, false) end)
    for i, e in { eng1, eng2, eng3, eng4 } do queue('eng' .. i .. ' +0', e) end
    WaitTicks(2)
    for i, e in { eng1, eng2, eng3, eng4 } do queue('eng' .. i .. ' +1', e) end
    R('BuildStructure eng1 second (queued?)', function() return brain:BuildStructure(eng1, 'ueb1101', { QX + 1, QZ + 37, 0 }, false) end)
    queue('eng1 after second', eng1)
    can('queued site of eng1 second', 'ueb1101', pos(QX + 1, QZ + 37))
    can('active site of eng1 first', 'ueb1101', pos(QX + 1, QZ + 31))

    -- factories
    R('GetAvailableFactories', function() return brain:GetAvailableFactories() end)
    R('GetAvailableFactories pos r25', function() return brain:GetAvailableFactories(pos(QX - 30, QZ - 10), 25) end)
    R('GetAvailableFactories pos r5', function() return brain:GetAvailableFactories(pos(QX - 30, QZ - 10), 5) end)
    R('GetAvailableFactories far r5', function() return brain:GetAvailableFactories(pos(352, 880), 5) end)
    local PT = { 'ProbeBuild', '', { 'uel0201', 1, 2, 'Attack', 'GrowthFormation' }, { 'uel0101', 1, 1, 'Scout', 'None' } }
    local PT3 = { 'ProbeBuild3', '', { 'uel0303', 1, 1, 'Attack', 'None' } }
    R('CanBuildPlatoon PT', function() return brain:CanBuildPlatoon(PT, { fac1, fac2 }) end)
    R('CanBuildPlatoon PT3 (T3 unit)', function() return brain:CanBuildPlatoon(PT3, { fac1, fac2 }) end)
    R('CanBuildPlatoon no factories', function() return brain:CanBuildPlatoon(PT, {}) end)
    R('CanBuildPlatoon eng as factory', function() return brain:CanBuildPlatoon(PT, { eng1 }) end)
    R('BuildPlatoon PT x2', function() return brain:BuildPlatoon(PT, { fac1, fac2 }, 2) end)
    queue('fac1 after BuildPlatoon', fac1)
    queue('fac2 after BuildPlatoon', fac2)
    R('GetAvailableFactories after BuildPlatoon', function() return brain:GetAvailableFactories() end)
    R('BuildUnit fac2 uel0201 x3', function() return brain:BuildUnit(fac2, 'uel0201', 3) end)
    queue('fac2 after BuildUnit', fac2)
    R('BuildUnit eng1 pgen', function() return brain:BuildUnit(eng1, 'ueb1101', 1) end)
    queue('eng1 after BuildUnit', eng1)
    R('BuildUnit fac2 T3 unit', function() return brain:BuildUnit(fac2, 'uel0303', 1) end)
    queue('fac2 after BuildUnit T3', fac2)
    WaitTicks(3)
    R('GetAvailableFactories busy', function() return brain:GetAvailableFactories() end)
    R('fac1 building', function() return fac1:IsUnitState('Building'), fac1.UnitBeingBuilt and fac1.UnitBeingBuilt:GetUnitId() end)
    R('fac2 building', function() return fac2:IsUnitState('Building'), fac2.UnitBeingBuilt and fac2.UnitBeingBuilt:GetUnitId() end)
    R('NumCurrentlyBuilding MOBILE by FACTORY', function() return brain:NumCurrentlyBuilding(C.MOBILE, C.FACTORY) end)
    R('IsAnyEngineerBuilding ENERGYPRODUCTION', function() return brain:IsAnyEngineerBuilding(C.ENERGYPRODUCTION) end)
    R('bpstat Units_BeingBuilt', function() return brain:GetBlueprintStat('Units_BeingBuilt', C.ALLUNITS) end)

    for _, t in { 40, 80, 120, 200, 300, 400, 500 } do
        wait_to(t)
        structures_near('t' .. t, Q, 80)
        for i, e in { eng1, eng2, eng3, eng4 } do queue('eng' .. i, e) end
        queue('fac1', fac1)
        queue('fac2', fac2)
        local line = 'PROBE ' .. T() .. ' produced'
        local counts = {}
        for _, u in brain:GetListOfUnits(C.MOBILE - C.ENGINEER, false) do
            local id = u:GetUnitId() .. (u:IsBeingBuilt() and '*' or '')
            counts[id] = (counts[id] or 0) + 1
        end
        LOG(line .. ' ' .. S(counts))
    end
    R('platoons at end', function() return table.getn(brain:GetPlatoonsList()) end)
    R('pool count at end', function() return brain:GetPlatoonUniquelyNamed('ArmyPool'):PlatoonCategoryCount(C.ALLUNITS) end)
    WaitTicks(30)
    LOG('PROBE done')
end)
