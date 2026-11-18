-- Differential probe: the sim unit API, one line per call, in a format both
-- engines print identically ("PROBE ..." lines).
--   real game (Windows): dev/probes/run_faf.ps1 probe_unit_api 120
--   ours: cargo run -- --headless --no-ai --ticks 260 --script dev/probes/maps/probe_unit_api/probe.lua
-- Sections: 'bp' blueprint fields after registration; 'api' getters and
-- setters on idle units; 'state' IsUnitState over the state list; 'rect'
-- GetUnitsInRect edges; 'layer' the layer a unit starts in; 'life' the
-- lifecycle callbacks of a T2 tank (create, Destroy, Kill, ChangeUnitArmy);
-- 'army' alliances and the unit cap; 'attach' AttachBoneTo / GetParent.
-- Tested units are army 1's; ARMY_9 and NEUTRAL_CIVILIAN (no AI) receive.
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
local A1, A9, CIV = 1, indexOf('ARMY_9') or 9, indexOf('NEUTRAL_CIVILIAN')
LOG('PROBE armynames ' .. table.concat(armies, ','))
LOG('PROBE armies ', table.getn(armies), ' a9 ', tostring(A9), ' civ ', tostring(CIV))
local start = GetGameTick()
local tagOf = {}
local function S(x, depth)
    depth = depth or 0
    local t = type(x)
    if t == 'number' then return fmt('%.3f', x) end
    if t == 'nil' or t == 'boolean' then return tostring(x) end
    if t == 'string' then return '"' .. x .. '"' end
    if t == 'table' then
        if tagOf[x] then return '<' .. tagOf[x] .. '>' end
        local isEnt = false
        pcall(function() isEnt = x.GetEntityId ~= nil or x.GetArmyIndex ~= nil end)
        if isEnt then return '<ent>' end
        if depth > 2 then return '{..}' end
        local keys = {}
        for k in x do table.insert(keys, k) end
        table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
        local parts = {}
        for _, k in keys do
            table.insert(parts, tostring(k) .. '=' .. S(x[k], depth + 1))
        end
        return '{' .. table.concat(parts, ',') .. '}'
    end
    return t
end
local function P(section, text) LOG('PROBE ' .. section .. ' ' .. text) end
local section = 'api'
local function tryNow(section, label, f)
    local ok, a, b, c = pcall(f)
    if ok then
        local s = S(a)
        if b ~= nil or c ~= nil then s = s .. ' , ' .. S(b) end
        if c ~= nil then s = s .. ' , ' .. S(c) end
        P(section, label .. ' = ' .. s)
    else
        P(section, label .. ' ERR | ' .. string.sub(tostring(a), 1, 90))
    end
end
-- Each call runs in its own thread: a call that throws a C++ exception in
-- the real game then ends that thread only (at the top level it ends the
-- whole script).
local function try(label, f)
    local sec = section
    ForkThread(function() tryNow(sec, label, f) end)
end
local function make(bp, army, x, z, y)
    return CreateUnitHPR(bp, army, x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
end

-- bp: registered blueprint fields.
section = 'bp'
local function dumpBp(id, tables)
    local bp = __blueprints[id]
    if not bp then P('bp', id .. ' missing') return end
    for _, name in tables do
        local sub = bp[name]
        if type(sub) == 'table' then
            local keys = {}
            for k, v in sub do
                if type(v) ~= 'table' then table.insert(keys, k) end
            end
            table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
            for _, k in keys do
                P('bp', id .. ' ' .. name .. '.' .. tostring(k) .. ' = ' .. S(sub[k]))
            end
        else
            P('bp', id .. ' ' .. name .. ' = ' .. S(sub))
        end
    end
end
local BPS = { 'uel0201', 'ueb0101', 'uaa0107', 'ueb1101', 'uel0001', 'urs0203', 'uel0105', 'uea0107' }
for _, id in BPS do
    dumpBp(id, { 'General', 'Intel', 'Physics', 'Defense', 'Economy', 'Transport', 'AI', 'Air' })
    local bp = __blueprints[id]
    try(id .. ' top', function()
        return { bp.BlueprintId, bp.Description, bp.SizeX, bp.SizeY, bp.SizeZ, bp.SelectionSizeX, bp.LifeBarSize, bp.LifeBarHeight, bp.LifeBarOffset,
            bp.CollisionOffsetX, bp.CollisionOffsetY, bp.CollisionOffsetZ, bp.StrategicIconName, bp.StrategicIconSortPriority, bp.BuildIconSortPriority,
            bp.Display.IconName, bp.Display.UniformScale, bp.Footprint and bp.Footprint.SizeX, bp.Footprint and bp.Footprint.SizeZ, bp.Source, bp.ScriptClass, bp.ScriptModule }
    end)
    try(id .. ' jam', function() return { bp.Intel.JamRadius, bp.Intel.SpoofRadius } end)
    if bp and bp.Weapon then
        for i, w in bp.Weapon do
            local keys = {}
            for k, v in w do
                if type(v) ~= 'table' then table.insert(keys, k) end
            end
            table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
            for _, k in keys do
                P('bp', id .. ' Weapon' .. i .. '.' .. tostring(k) .. ' = ' .. S(w[k]))
            end
        end
    end
end
for _, id in { '/props/defaultwreckage/defaultwreckage_prop.bp', '/env/evergreen/props/trees/pine06_prop.bp', '/env/evergreen/props/rocks/rock01_prop.bp' } do
    dumpBp(id, { 'Economy', 'Defense', 'Physics' })
end

ForkThread(function()
-- api: getters and setters.
section = 'api'
local X, Z = 608, 80
local tank = make('uel0201', A1, X, Z) tagOf[tank] = 'tank'
local eng = make('uel0105', A1, X + 6, Z) tagOf[eng] = 'eng'
local pgen = make('ueb1101', A1, X + 12, Z) tagOf[pgen] = 'pgen'
local air = make('uea0102', A1, X + 18, Z) tagOf[air] = 'air'
local trans = make('uea0107', A1, X + 26, Z) tagOf[trans] = 'trans'
local fac = make('ueb0101', A1, X + 40, Z) tagOf[fac] = 'fac'
local tml = make('ueb2108', A1, X + 52, Z) tagOf[tml] = 'tml'
local nuke = make('ueb2305', A1, X + 64, Z) tagOf[nuke] = 'nuke'
local brain = ArmyBrains[A1]

local function battery(u, name, noPath)
    local function T(label, f) try(name .. ' ' .. label, f) end
    T('GetEntityId type', function() return type(u:GetEntityId()) end)
    T('GetArmy', function() return u:GetArmy() end)
    T('GetUnitId', function() return u:GetUnitId() end)
    T('GetAIBrain', function() return u:GetAIBrain() == brain end)
    T('GetHealth', function() return u:GetHealth(), u:GetMaxHealth() end)
    T('GetFractionComplete', function() return u:GetFractionComplete() end)
    T('GetWorkProgress', function() return u:GetWorkProgress() end)
    T('GetBuildRate', function() return u:GetBuildRate() end)
    T('GetFocusUnit', function() return u:GetFocusUnit() end)
    T('GetParent', function() return u:GetParent() end)
    T('GetParent==self', function() return u:GetParent() == u end)
    T('GetGuardedUnit', function() return u:GetGuardedUnit() end)
    T('GetTargetEntity', function() return u:GetTargetEntity() end)
    T('GetCargo', function() return u:GetCargo() end)
    T('GetFuelRatio', function() return u:GetFuelRatio() end)
    T('GetFuelUseTime', function() return u:GetFuelUseTime() end)
    T('GetCurrentLayer', function() return u:GetCurrentLayer() end)
    T('GetPosition', function() return u:GetPosition() end)
    T('GetOrientation', function() return u:GetOrientation() end)
    T('GetHeading', function() return u:GetHeading() end)
    T('GetVelocity', function() return u:GetVelocity() end)
    T('GetBoneCount', function() return u:GetBoneCount() end)
    T('GetBoneName0', function() return u:GetBoneName(0) end)
    T('GetBoneName1', function() return u:GetBoneName(1) end)
    T('GetBoneName-1', function() return u:GetBoneName(-1) end)
    T('GetBoneName999', function() return u:GetBoneName(999) end)
    T('IsValidBone bogus', function() return u:IsValidBone('NoSuchBone') end)
    T('IsValidBone 0', function() return u:IsValidBone(0) end)
    T('GetBoneDirection0', function() return u:GetBoneDirection(0) end)
    T('GetCommandQueue', function() return table.getn(u:GetCommandQueue()) end)
    T('GetCurrentMoveLocation', function() return u:GetCurrentMoveLocation() end)
    T('GetNavigator', function() return u:GetNavigator() ~= nil end)
    T('nav GetGoalPos', function() return u:GetNavigator():GetGoalPos() end)
    T('nav GetCurrentTargetPos', function() return u:GetNavigator():GetCurrentTargetPos() end)
    T('nav GetStatus', function() return u:GetNavigator():GetStatus() end)
    T('nav HasGoodPath', function() return u:GetNavigator():HasGoodPath() end)
    T('GetWeaponCount', function() return u:GetWeaponCount() end)
    T('GetArmorMult Normal', function() return u:GetArmorMult('Normal') end)
    T('GetArmorMult bogus', function() return u:GetArmorMult('Bogus') end)
    T('GetShieldRatio', function() return u:GetShieldRatio() end)
    T('GetScriptBit 0', function() return u:GetScriptBit(0) end)
    T('GetScriptBit name', function() return u:GetScriptBit('RULEUTC_ShieldToggle') end)
    T('IsCapturable', function() return u:IsCapturable() end)
    T('IsPaused', function() return u:IsPaused() end)
    T('IsIdleState', function() return u:IsIdleState() end)
    T('IsBeingBuilt', function() return u:IsBeingBuilt() end)
    T('IsMobile', function() return u:IsMobile() end)
    T('IsMoving', function() return u:IsMoving() end)
    T('IsStunned', function() return u:IsStunned() end)
    T('IsOverchargePaused', function() return u:IsOverchargePaused() end)
    T('IsDead', function() return u:IsDead() end)
    T('BeenDestroyed', function() return u:BeenDestroyed() end)
    T('GetConsumptionPerSecondMass', function() return u:GetConsumptionPerSecondMass(), u:GetConsumptionPerSecondEnergy() end)
    T('GetProductionPerSecondMass', function() return u:GetProductionPerSecondMass(), u:GetProductionPerSecondEnergy() end)
    T('GetResourceConsumed', function() return u:GetResourceConsumed() end)
    T('GetTransportFerryBeacon', function() return u:GetTransportFerryBeacon() end)
    T('GetRallyPoint', function() return u:GetRallyPoint() end)
    T('GetCreator', function() return u:GetCreator() end)
    T('GetNukeSiloAmmoCount', function() return u:GetNukeSiloAmmoCount(), u:GetTacticalSiloAmmoCount() end)
    T('GetFireState', function() return u:GetFireState() end)
    T('GetStat new', function() return u:GetStat('PROBESTAT', 5) end)
    T('GetStat new nodefault', function() return u:GetStat('PROBESTAT2') end)
    T('GetCollisionExtents', function() return u:GetCollisionExtents() end)
    T('GetFootPrintSize', function() return u:GetFootPrintSize() end)
    T('GetElevation', function() return u:GetElevation() end)
    T('GetUnitSizes', function() return u:GetUnitSizes() end)
    T('GetScale', function() return u:GetScale() end)
    T('GetMeshBlueprint? GetMesh', function() return u:GetMesh() end)
    T('CanBuild pgen', function() return u:CanBuild('ueb1101') end)
    T('CanBuild tank', function() return u:CanBuild('uel0201') end)
    if not noPath then
        T('CanPathTo', function() return u:CanPathTo({ X + 100, GetTerrainHeight(X + 100, Z), Z }) end)
        T('CanPathTo near', function() return u:CanPathTo({ X + 10, GetTerrainHeight(X + 10, Z), Z + 20 }) end)
        T('CanPathTo sea', function() return u:CanPathTo({ 48, GetTerrainHeight(48, 48), 48 }) end)
    end
    T('IsValidTarget', function() return u:IsValidTarget() end)
    T('TestCommandCaps move', function() return u:TestCommandCaps('RULEUCC_Move') end)
    T('TestCommandCaps reclaim', function() return u:TestCommandCaps('RULEUCC_Reclaim') end)
    T('TestToggleCaps prod', function() return u:TestToggleCaps('RULEUTC_ProductionToggle') end)
    T('GetBlip own', function() return u:GetBlip(A1) end)
    T('GetBlip civ', function() return u:GetBlip(CIV) end)
    T('GetStrategicIcon? Tractored', function() return u:IsUnitState('Bogus') end)
end
battery(tank, 'tank')
battery(eng, 'eng')

WaitTicks(2)
-- setters on the tank
local function T(label, f) try('set ' .. label, f) end
T('SetMaxHealth 500', function() tank:SetMaxHealth(500) return tank:GetHealth(), tank:GetMaxHealth() end)
T('SetHealth 9999', function() tank:SetHealth(nil, 9999) return tank:GetHealth(), tank:GetMaxHealth() end)
T('AdjustHealth -100', function() tank:AdjustHealth(tank, -100) return tank:GetHealth() end)
T('AdjustHealth +1000', function() tank:AdjustHealth(tank, 1000) return tank:GetHealth() end)
T('SetMaxHealth 200', function() tank:SetMaxHealth(200) return tank:GetHealth(), tank:GetMaxHealth() end)
T('SetHealth 150.7', function() tank:SetHealth(nil, 150.7) return tank:GetHealth() end)
T('SetWorkProgress .5', function() tank:SetWorkProgress(0.5) return tank:GetWorkProgress() end)
T('SetWorkProgress 2', function() tank:SetWorkProgress(2) return tank:GetWorkProgress() end)
T('SetBuildRate 33', function() eng:SetBuildRate(33) return eng:GetBuildRate() end)
T('SetBuildRate tank', function() tank:SetBuildRate(7) return tank:GetBuildRate() end)
T('SetStat', function() tank:SetStat('PROBESTAT', 7) return tank:GetStat('PROBESTAT', 1) end)
T('SetStat str', function() tank:SetStat('PROBESTR', 'abc') return tank:GetStat('PROBESTR', 'd') end)
T('GetStat KILLS', function() return tank:GetStat('KILLS', 0) end)
T('SetCustomName', function() tank:SetCustomName('bob') return tank.GetCustomName and tank:GetCustomName() end)
T('SetCapturable', function() tank:SetCapturable(false) return tank:IsCapturable() end)
T('SetReclaimable', function() tank:SetReclaimable(false) return tank:IsUnitState('NoReclaim') end)
T('SetDoNotTarget', function() tank:SetDoNotTarget(true) return tank:IsUnitState('DoNotTarget') end)
T('SetUnSelectable', function() tank:SetUnSelectable(true) return tank:IsUnitState('UnSelectable') end)
T('SetPaused', function() tank:SetPaused(true) return tank:IsPaused() end)
T('SetPaused off', function() tank:SetPaused(false) return tank:IsPaused() end)
T('SetBusy', function() tank:SetBusy(true) return tank:IsUnitState('Busy'), tank:IsIdleState() end)
T('SetBusy off', function() tank:SetBusy(false) return tank:IsUnitState('Busy') end)
T('SetBlockCommandQueue', function() tank:SetBlockCommandQueue(true) return tank:IsUnitState('BlockCommandQueue') end)
T('SetBlockCommandQueue off', function() tank:SetBlockCommandQueue(false) return tank:IsUnitState('BlockCommandQueue') end)
T('SetImmobile', function() tank:SetImmobile(true) return tank:IsUnitState('Immobile'), tank:IsMobile() end)
T('SetImmobile off', function() tank:SetImmobile(false) return tank:IsUnitState('Immobile') end)
T('SetUnitState', function() tank:SetUnitState('Busy', true) return tank:IsUnitState('Busy') end)
T('SetUnitState off', function() tank:SetUnitState('Busy', false) return tank:IsUnitState('Busy') end)
T('SetFireState 1', function() tank:SetFireState(1) return tank:GetFireState() end)
T('SetFireState name', function() tank:SetFireState('GroundFire') return tank:GetFireState() end)
T('SetScriptBit', function() tank:SetScriptBit('RULEUTC_WeaponToggle', true) return tank:GetScriptBit('RULEUTC_WeaponToggle'), tank:GetScriptBit(1) end)
T('ToggleScriptBit 3', function() tank:ToggleScriptBit(3) return tank:GetScriptBit(3) end)
T('SetFuelRatio air', function() air:SetFuelRatio(0.5) return air:GetFuelRatio() end)
T('SetFuelRatio tank', function() tank:SetFuelRatio(0.5) return tank:GetFuelRatio() end)
T('SetFuelUseTime air', function() air:SetFuelUseTime(50) return air:GetFuelUseTime() end)
T('SetConsumptionPerSecondEnergy', function() tank:SetConsumptionPerSecondEnergy(12) return tank:GetConsumptionPerSecondEnergy() end)
T('SetProductionPerSecondMass', function() tank:SetProductionPerSecondMass(3) return tank:GetProductionPerSecondMass() end)
T('SetElevation', function() tank:SetElevation(2) return tank:GetElevation() end)
T('SetSpeedMult', function() tank:SetSpeedMult(2) return tank:GetBlueprint().Physics.MaxSpeed end)
T('SetCollisionShape', function() tank:SetCollisionShape('Sphere', 0, 0, 0, 3) return tank:GetCollisionExtents() end)
T('SetCollisionShape box', function() tank:SetCollisionShape('Box', 0, 1, 0, 1, 2, 3) return tank:GetCollisionExtents() end)
T('SetCollisionShape none', function() tank:SetCollisionShape('None') return tank:GetCollisionExtents() end)
T('HideBone', function() tank:HideBone(0, true) return tank:IsValidBone('Turret') end)
T('ShowBone', function() tank:ShowBone(0, true) return 1 end)
T('HideBone bogus', function() tank:HideBone('NoSuchBone', true) return 1 end)
T('SetStrategicUnderlay', function() tank:SetStrategicUnderlay('icon_objective_primary') return 1 end)
T('SetScale? SetDrawScale', function() tank:SetDrawScale(2) return tank:GetScale() end)
T('SetMesh bogus', function() tank:SetMesh('/nomesh', true) return 1 end)
T('GiveTacticalSiloAmmo', function() tml:GiveTacticalSiloAmmo(2) return tml:GetTacticalSiloAmmoCount() end)
T('RemoveTacticalSiloAmmo', function() tml:RemoveTacticalSiloAmmo(1) return tml:GetTacticalSiloAmmoCount() end)
T('RemoveTacticalSiloAmmo 5', function() tml:RemoveTacticalSiloAmmo(5) return tml:GetTacticalSiloAmmoCount() end)
T('GiveTacticalSiloAmmo 50', function() tml:GiveTacticalSiloAmmo(50) return tml:GetTacticalSiloAmmoCount() end)
T('GiveNukeSiloAmmo', function() nuke:GiveNukeSiloAmmo(1) return nuke:GetNukeSiloAmmoCount() end)
T('GiveNukeSiloAmmo blocks', function() nuke:GiveNukeSiloAmmo(1, true) return nuke:GetNukeSiloAmmoCount() end)
T('GiveNukeSiloAmmo 20', function() nuke:GiveNukeSiloAmmo(20) return nuke:GetNukeSiloAmmoCount() end)
T('RemoveNukeSiloAmmo', function() nuke:RemoveNukeSiloAmmo(2) return nuke:GetNukeSiloAmmoCount() end)
T('AddBuildRestriction', function() eng:AddBuildRestriction(categories.ueb1101) return eng:CanBuild('ueb1101'), eng:CanBuild('ueb1103') end)
T('RemoveBuildRestriction', function() eng:RemoveBuildRestriction(categories.ueb1101) return eng:CanBuild('ueb1101') end)
T('RestoreBuildRestrictions', function() eng:AddBuildRestriction(categories.ALLUNITS) eng:RestoreBuildRestrictions() return eng:CanBuild('ueb1101') end)
T('AddBuildRestriction army', function() AddBuildRestriction(A1, categories.ueb1103) return eng:CanBuild('ueb1103') end)
T('RemoveBuildRestriction army', function() RemoveBuildRestriction(A1, categories.ueb1103) return eng:CanBuild('ueb1103') end)
T('GetEntityById', function() return GetEntityById(tank:GetEntityId()) == tank, GetUnitById(tank:GetEntityId()) == tank end)
T('GetEntityById str', function() return GetEntityById(tostring(tank:GetEntityId())) == tank end)
T('GetEntityById bogus', function() return GetEntityById(987654) end)
T('GetUnitById bogus', function() return GetUnitById(987654) end)
T('id army bits', function() return math.floor(tonumber(tank:GetEntityId()) / 1048576), math.floor(tonumber(make('uel0201', CIV, X, Z + 300):GetEntityId()) / 1048576) end)
T('EntityCategoryContains', function() return EntityCategoryContains(categories.LAND, tank), EntityCategoryContains(categories.AIR, tank) end)
T('IsUnit/IsEntity', function() return IsUnit(tank), IsEntity(tank), IsUnit(nil) end)
T('GetUnitBlueprintByName', function() return GetUnitBlueprintByName('uel0201').BlueprintId end)
T('GetBlueprint same table', function() return tank:GetBlueprint() == __blueprints['uel0201'] end)

WaitTicks(2)
-- state: every state on idle units.
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
        if not ok then table.insert(on, s .. '!ERR') elseif v then table.insert(on, s) end
    end
    return table.concat(on, '+')
end
for _, u in { tank, eng, pgen, air, trans, fac, tml, nuke } do P('state', tagOf[u] .. ' ' .. states(u)) end

-- rect: GetUnitsInRect edges. Units at x 700, 704, 708 (z 80).
section = 'rect'
local r1, r2, r3 = make('uel0201', A1, 700, 80), make('uel0201', A1, 704, 80), make('uel0201', A9, 708, 80)
local function count(t) return t and table.getn(t) or 'nil' end
try('all', function() return count(GetUnitsInRect(Rect(690, 70, 720, 90))) end)
try('4 numbers', function() return count(GetUnitsInRect(690, 70, 720, 90)) end)
try('edge 704..708', function() return count(GetUnitsInRect(Rect(704, 70, 708, 90))) end)
try('edge 704.5..707.5', function() return count(GetUnitsInRect(Rect(704.5, 70, 707.5, 90))) end)
try('edge 703.6..704.4', function() return count(GetUnitsInRect(Rect(703.6, 79.6, 704.4, 80.4))) end)
try('point 704', function() return count(GetUnitsInRect(Rect(704, 80, 704, 80))) end)
try('just off 705', function() return count(GetUnitsInRect(Rect(705, 70, 707, 90))) end)
try('off 704.6', function() return count(GetUnitsInRect(Rect(704.6, 70, 707.4, 90))) end)
try('off 705.1', function() return count(GetUnitsInRect(Rect(705.1, 70, 706.9, 90))) end)
try('empty', function() return count(GetUnitsInRect(Rect(730, 70, 740, 90))) end)
try('reclaimables', function() return count(GetReclaimablesInRect(Rect(690, 70, 720, 90))) end)
try('reclaimables empty', function() return count(GetReclaimablesInRect(Rect(730, 70, 731, 71))) end)
try('brain GetUnitsAroundPoint', function() return count(brain:GetUnitsAroundPoint(categories.ALLUNITS, { 704, 0, 80 }, 4, 'Ally')) end)
try('brain GetUnitsAroundPoint r3.9', function() return count(brain:GetUnitsAroundPoint(categories.ALLUNITS, { 704, 0, 80 }, 3.9, 'Ally')) end)
try('brain GetUnitsAroundPoint noalliance', function() return count(brain:GetUnitsAroundPoint(categories.ALLUNITS, { 704, 0, 80 }, 6)) end)
try('brain GetUnitsAroundPoint Enemy', function() return count(brain:GetUnitsAroundPoint(categories.ALLUNITS, { 704, 0, 80 }, 6, 'Enemy')) end)
try('brain GetNumUnitsAroundPoint', function() return brain:GetNumUnitsAroundPoint(categories.ALLUNITS, { 704, 0, 80 }, 6, 'Ally') end)
try('GetCurrentUnits', function() return brain:GetCurrentUnits(categories.uel0201), brain:GetCurrentUnits(categories.STRUCTURE) end)
try('GetListOfUnits', function() return count(brain:GetListOfUnits(categories.uel0201, false)), count(brain:GetListOfUnits(categories.uel0201, true)) end)

WaitTicks(2)
-- army: alliances, unit cap, brain values.
section = 'army'
try('IsAlly 1,9', function() return IsAlly(A1, A9), IsEnemy(A1, A9), IsNeutral(A1, A9) end)
try('IsAlly 1,civ', function() return IsAlly(A1, CIV), IsEnemy(A1, CIV), IsNeutral(A1, CIV) end)
try('IsAlly 1,1', function() return IsAlly(A1, A1), IsEnemy(A1, A1), IsNeutral(A1, A1) end)
try('IsAlly 1,2', function() return IsAlly(A1, 2), IsEnemy(A1, 2), IsNeutral(A1, 2) end)
try('SetAlliance ally', function() SetAlliance(A1, A9, 'Ally') return IsAlly(A1, A9), IsAlly(A9, A1), IsEnemy(A9, A1) end)
try('SetAlliance names', function() SetAlliance(armies[A1], 'NEUTRAL_CIVILIAN', 'Enemy') return IsEnemy(A1, CIV), IsEnemy(CIV, A1) end)
try('SetAlliance neutral', function() SetAlliance(A1, CIV, 'Neutral') return IsNeutral(A1, CIV), IsNeutral(CIV, A1) end)
try('SetAlliance bogus', function() SetAlliance(A1, CIV, 'Bogus') return IsNeutral(A1, CIV), IsAlly(A1, CIV), IsEnemy(A1, CIV) end)
try('ArmyIsCivilian', function() return ArmyIsCivilian(A1), ArmyIsCivilian(CIV), ArmyIsCivilian(A9) end)
try('ArmyIsOutOfGame', function() return ArmyIsOutOfGame(A1), ArmyIsOutOfGame(CIV) end)
try('GetArmyUnitCap', function() return GetArmyUnitCap(A1), GetArmyUnitCap(A9), GetArmyUnitCap(CIV) end)
try('GetArmyUnitCostTotal', function() return GetArmyUnitCostTotal(A1), GetArmyUnitCostTotal(A9) end)
try('ArmyGetHandicap', function() return ArmyGetHandicap(A1) end)
try('GetArmyBrain', function() return GetArmyBrain(A1) == brain, GetArmyBrain(armies[A1]) == brain end)
try('brain basics', function() return { brain:GetArmyIndex(), brain:GetFactionIndex(), brain.Name, brain.Nickname, brain.BrainType } end)
try('brain GetArmyStartPos', function() return brain:GetArmyStartPos() end)
try('brain stat UnitCap', function() return brain:GetArmyStat('UnitCap_Current', 0).Value, brain:GetArmyStat('UnitCap_MaxCap', 0).Value end)
try('brain stat bogus', function() return brain:GetArmyStat('ProbeBogus', 3) end)
try('brain SetArmyStat', function() brain:SetArmyStat('ProbeStat', 4) return brain:GetArmyStat('ProbeStat', 0).Value end)
try('brain AddArmyStat', function() brain:AddArmyStat('ProbeStat', 2) return brain:GetArmyStat('ProbeStat', 0).Value end)
try('brain GetBlueprintStat', function() return brain:GetBlueprintStat('Units_History', categories.uel0201), brain:GetBlueprintStat('Units_Active', categories.uel0201) end)
try('brain GetUnitBlueprint', function() return brain:GetUnitBlueprint('uel0201').BlueprintId end)
try('brain IsDefeated', function() return brain:IsDefeated() end)
try('brain GetCurrentEnemy', function() return brain:GetCurrentEnemy() end)
try('brain GetEconomyStored', function() return brain:GetEconomyStored('MASS'), brain:GetEconomyStored('ENERGY') end)
try('brain IsAnyEngineerBuilding', function() return brain:IsAnyEngineerBuilding(categories.ALLUNITS) end)
try('brain GetHighestThreatPosition', function() return brain:GetHighestThreatPosition(1, true, 'Overall') end)
try('brain GetNoRushTicks', function() return brain:GetNoRushTicks() end)
try('GetFocusArmy', function() return GetFocusArmy() end)
try('GetGameTimeSeconds type', function() return type(GetGameTimeSeconds()), GetGameTick() - GetGameTimeSeconds() * 10 end)
WaitTicks(2)
-- unit cap: A9 capped at its count + 2, then 4 creations and a transfer.
try('SetArmyUnitCap', function()
    local n = ArmyBrains[A9]:GetArmyStat('UnitCap_Current', 0).Value
    SetArmyUnitCap(A9, n + 2)
    return n, GetArmyUnitCap(A9)
end)
ArmyBrains[A1].OnFailedUnitTransfer = function() P('army', 'OnFailedUnitTransfer on giver') end
ArmyBrains[A9].OnFailedUnitTransfer = function() P('army', 'OnFailedUnitTransfer on receiver') end
ArmyBrains[A9].OnUnitCapLimitReached = function() P('army', 'OnUnitCapLimitReached A9') end
for i = 1, 4 do
    local i = i
    try('capped create ' .. i, function()
        local u = make('uel0201', A9, 760 + 4 * i, 80)
        return u ~= nil, u and u:BeenDestroyed(), ArmyBrains[A9]:GetArmyStat('UnitCap_Current', 0).Value
    end)
end
try('capped transfer', function() return ChangeUnitArmy(r1, A9) end)
try('capped transfer after', function() return r1:BeenDestroyed(), r1:GetArmy() end)
try('uncap', function() SetArmyUnitCap(A9, 500) return GetArmyUnitCap(A9) end)
try('IgnoreArmyUnitCap', function() SetIgnoreArmyUnitCap(A9, true) return GetArmyUnitCap(A9) end)

WaitTicks(2)
-- attach.
section = 'attach'
local rider = make('uel0201', A1, X + 26, Z + 6) tagOf[rider] = 'rider'
local bone
for i = 0, trans:GetBoneCount() - 1 do
    local n = trans:GetBoneName(i)
    if n and string.find(n, 'ttach') and not bone then bone = n end
end
P('attach', 'bone ' .. tostring(bone))
try('AttachBoneTo', function() rider:AttachBoneTo(-1, trans, bone) return rider:GetParent(), rider:IsUnitState('Attached') end)
try('cargo', function() return trans:GetCargo() end)
try('rider pos = bone', function()
    local a, b = rider:GetPosition(), trans:GetPosition(bone)
    return VDist3(a, b) < 0.01
end)
try('DetachFrom', function() rider:DetachFrom(false) return rider:GetParent() == rider, rider:IsUnitState('Attached') end)
try('cargo after', function() return trans:GetCargo() end)
try('AttachTo? rider2', function() rider:AttachTo(trans, bone) return rider:GetParent() end)
try('DetachAll', function() trans:DetachAll(bone, false) return rider:GetParent() == rider end)

WaitTicks(2)
-- layer: where units start.
section = 'layer'
local WX, WZ = 48, 48
P('layer', fmt('site terrain %.2f surface %.2f', GetTerrainHeight(WX, WZ), GetSurfaceHeight(WX, WZ)))
local layerUnits = {}
local function layerCase(tag, bp, x, z, y)
    local ok, u = pcall(make, bp, A1, x, z, y)
    if not ok or not u then P('layer', tag .. ' create failed') return end
    tagOf[u] = tag
    table.insert(layerUnits, u)
    local _, py = u:GetPositionXYZ()
    P('layer', fmt('%s t0 %s y %.2f', tag, tostring(u:GetCurrentLayer()), py))
end
layerCase('tank@seabed', 'uel0201', WX, WZ)
layerCase('tank@surface', 'uel0201', WX + 8, WZ, GetSurfaceHeight(WX, WZ))
layerCase('pgen@seabed', 'ueb1101', WX + 16, WZ)
layerCase('pgen@surface', 'ueb1101', WX + 24, WZ, GetSurfaceHeight(WX, WZ))
layerCase('sub@surface', 'urs0203', WX + 32, WZ, GetSurfaceHeight(WX, WZ))
layerCase('sub@seabed', 'urs0203', WX + 40, WZ)
layerCase('amph@seabed', 'url0203', WX + 48, WZ)
layerCase('amph@surface', 'url0203', WX + 56, WZ, GetSurfaceHeight(WX, WZ))
layerCase('hover@seabed', 'ual0201', WX + 64, WZ)
layerCase('eng@seabed', 'uel0105', WX + 72, WZ)
layerCase('frigate@seabed', 'ues0103', WX, WZ + 16)
layerCase('frigate@land', 'ues0103', X, Z + 40)
layerCase('air@ground', 'uea0102', X + 8, Z + 40)
layerCase('air@high', 'uea0102', X + 16, Z + 40, GetTerrainHeight(X + 16, Z + 40) + 30)
layerCase('tank@high', 'uel0201', X + 24, Z + 40, GetTerrainHeight(X + 24, Z + 40) + 30)
layerCase('sonar@surface', 'ueb3102', WX + 8, WZ + 16, GetSurfaceHeight(WX, WZ))
layerCase('torp@seabed', 'ueb2109', WX + 16, WZ + 16)

-- life: lifecycle callbacks of uel0202.
local lifeCtx = 'idle'
local function L(text) P('life', fmt('%d [%s] %s', GetGameTick() - start, lifeCtx, text)) end
local cls = import('/units/uel0202/uel0202_script.lua').TypeClass
local HOOKS = { 'OnPreCreate', 'OnCreate', 'OnLayerChange', 'OnStartBeingBuilt', 'OnStopBeingBuilt', 'OnKilled', 'OnDestroy', 'OnReclaimed',
    'OnCaptured', 'OnGiven', 'OnDamage', 'OnAnimCollision', 'OnMotionHorzEventChange', 'OnStartBuild', 'OnStopBuild', 'OnCollisionCheck',
    'OnHealthChanged', 'OnDetachedFromTransport', 'OnAttachedToTransport', 'OnScriptBitSet', 'OnPaused', 'OnUnpaused', 'OnGotTarget', 'OnLostTarget' }
for _, name in HOOKS do
    local orig = cls[name]
    if orig then
        local n = name
        cls[n] = function(self, a, b, c, d)
            L(n .. ' ' .. S(a) .. ' ' .. S(b) .. ' ' .. S(c) .. ' ' .. S(d))
            return orig(self, a, b, c, d)
        end
    else
        P('life', 'no class method ' .. name)
    end
end
if cls.Weapons then
    for label, wcls in cls.Weapons do
        for _, name in { 'OnCreate', 'OnDestroy' } do
            local orig = wcls[name]
            if orig then
                local n, lab = name, label
                wcls[n] = function(self, a, b)
                    L('weapon ' .. lab .. ' ' .. n)
                    return orig(self, a, b)
                end
            end
        end
    end
end

do
    local LX, LZ = 688, 80
    WaitTicks(12)
    for _, u in layerUnits do
        if u:BeenDestroyed() then
            P('layer', tagOf[u] .. ' t12 gone')
        else
            local _, py = u:GetPositionXYZ()
            P('layer', fmt('%s t12 %s y %.2f', tagOf[u], tostring(u:GetCurrentLayer()), py))
        end
    end
    -- regen
    tank:SetMaxHealth(300) tank:SetHealth(nil, 100)
    tryNow('regen', 'SetRegenRate', function() tank:SetRegenRate(7) return tank:GetHealth() end)
    for i = 1, 12 do
        WaitTicks(1)
        P('regen', fmt('+%d hp %.2f', i, tank:GetHealth()))
    end
    tank:SetRegenRate(0)
    tryNow('regen', 'RevertRegenRate', function() tank:RevertRegenRate() return 1 end)
    -- fuel: air unit idle on the ground and flying
    tryNow('fuel', 'air fuel before', function() return air:GetFuelRatio(), air:GetFuelUseTime() end)
    IssueMove({ air }, { X + 100, GetTerrainHeight(X + 100, Z), Z + 60 })
    WaitTicks(30)
    tryNow('fuel', 'air fuel +30 flying', function() return air:GetFuelRatio(), air:GetCurrentLayer() end)

    lifeCtx = 'create'
    L('begin')
    local u = make('uel0202', A1, LX, LZ) tagOf[u] = 'u1'
    L('end frac ' .. S(u:GetFractionComplete()))
    WaitTicks(5)
    lifeCtx = 'destroy'
    L('begin')
    u:Destroy()
    L('end destroyed ' .. tostring(u:BeenDestroyed()) .. ' dead ' .. tostring(u.Dead))
    WaitTicks(5)
    lifeCtx = 'kill'
    local k = make('uel0202', A1, LX, LZ) tagOf[k] = 'u2'
    WaitTicks(3)
    L('begin')
    k:Kill()
    L('end destroyed ' .. tostring(k:BeenDestroyed()) .. ' dead ' .. tostring(k.Dead) .. ' IsDead ' .. tostring(k:IsDead()))
    local was = false
    for i = 1, 80 do
        WaitTicks(1)
        if k:BeenDestroyed() and not was then
            was = true
            L('destroyed after ' .. i)
        end
    end
    lifeCtx = 'killinst'
    local k2 = make('uel0202', A1, LX, LZ) tagOf[k2] = 'u3'
    WaitTicks(3)
    L('begin')
    local okk, err = pcall(function() k2:Kill(tank, 'Normal', 2) end)
    L('end ' .. tostring(okk) .. ' dead ' .. tostring(k2.Dead))
    WaitTicks(60)
    lifeCtx = 'dmgkill'
    local k3 = make('uel0202', A1, LX, LZ) tagOf[k3] = 'u4'
    WaitTicks(3)
    L('begin')
    Damage(tank, k3:GetPosition(), k3, 100, 'Normal')
    L('after 100 hp ' .. S(k3:GetHealth()))
    Damage(tank, k3:GetPosition(), k3, 99999, 'Normal')
    L('end dead ' .. tostring(k3.Dead) .. ' hp ' .. S(k3:GetHealth()) .. ' destroyed ' .. tostring(k3:BeenDestroyed()))
    WaitTicks(60)
    lifeCtx = 'give'
    local g = make('uel0202', A1, LX, LZ) tagOf[g] = 'g1'
    g:SetHealth(nil, 123)
    g:SetCustomName('given')
    g:SetStat('PROBESTAT', 9)
    g.ProbeField = 'kept'
    IssueMove({ g }, { LX + 30, GetTerrainHeight(LX + 30, LZ), LZ })
    WaitTicks(3)
    L('begin')
    local oldId = g:GetEntityId()
    local ok, n = pcall(ChangeUnitArmy, g, A9)
    L('returned ' .. tostring(ok) .. ' ' .. S(n ~= nil) .. ' same table ' .. tostring(n == g))
    if ok and n then
        tagOf[n] = 'g1new'
        L('new id same ' .. tostring(n:GetEntityId() == oldId) .. ' army ' .. S(n:GetArmy()) .. ' hp ' .. S(n:GetHealth()) .. ' old destroyed ' .. tostring(g:BeenDestroyed())
            .. ' field ' .. tostring(n.ProbeField) .. ' queue ' .. S(table.getn(n:GetCommandQueue())) .. ' stat ' .. S(n:GetStat('PROBESTAT', 0).Value))
        local ok2, n2 = pcall(ChangeUnitArmy, n, A9)
        L('same army returned ' .. tostring(ok2) .. ' ' .. S(n2))
    end
    WaitTicks(5)
    lifeCtx = 'givecdr'
    local cdr = make('uel0001', A1, LX, LZ + 20) tagOf[cdr] = 'cdr'
    WaitTicks(3)
    local ok3, c2 = pcall(ChangeUnitArmy, cdr, A9)
    L('cdr no flag ' .. tostring(ok3) .. ' ' .. S(c2 ~= nil) .. ' old destroyed ' .. tostring(cdr:BeenDestroyed()))
    if not cdr:BeenDestroyed() then
        local ok4, c3 = pcall(ChangeUnitArmy, cdr, A9, true)
        L('cdr allow ' .. tostring(ok4) .. ' ' .. S(c3 ~= nil) .. ' old destroyed ' .. tostring(cdr:BeenDestroyed()))
    end
    -- a half-built unit given
    lifeCtx = 'givehalf'
    local okh, h = pcall(CreateUnit, 'uel0202', A1, LX + 10, GetTerrainHeight(LX + 10, LZ + 20), LZ + 20, 0, 0, 0, 1, 'Land')
    if okh and h then
        tagOf[h] = 'half'
        L('CreateUnit frac ' .. S(h:GetFractionComplete()) .. ' hp ' .. S(h:GetHealth()) .. ' beingbuilt ' .. tostring(h:IsBeingBuilt()))
    else
        L('CreateUnit failed ' .. string.sub(tostring(h), 1, 80))
    end
    -- Last, the calls that can take the real game down (CanPathTo on a
    -- structure did): the battery on the other kinds of unit, one at a time.
    section = 'api'
    for _, u in { trans, fac, tml, nuke, pgen } do
        if not u:BeenDestroyed() then battery(u, tagOf[u], true) end
        WaitTicks(2)
    end
    local air2 = make('uea0102', A1, X + 18, Z + 10) tagOf[air2] = 'air'
    battery(air2, 'air', true)
    WaitTicks(3)
    section = 'risky'
    tryNow('risky', 'air CanPathTo', function() return air2:CanPathTo({ X + 100, GetTerrainHeight(X + 100, Z), Z }) end)
    tryNow('risky', 'tank CanPathTo later', function() return tank:CanPathTo({ X + 100, GetTerrainHeight(X + 100, Z), Z }) end)
    tryNow('risky', 'tank CanPathTo near later', function() return tank:CanPathTo({ X + 10, GetTerrainHeight(X + 10, Z), Z + 20 }) end)
    WaitTicks(3)
    P('risky', 'next: tank GetAttacker')
    WaitTicks(3)
    -- takes the real game down
    -- try('tank GetAttacker', function() return tank:GetAttacker() end)
    WaitTicks(3)
    P('risky', 'next: pgen CanPathTo')
    WaitTicks(3)
    try('pgen CanPathTo', function() return pgen:CanPathTo({ X + 100, GetTerrainHeight(X + 100, Z), Z }) end)
    WaitTicks(3)
    P('risky', 'survived')
    WaitTicks(30)
    LOG('PROBE done')
end
end)
