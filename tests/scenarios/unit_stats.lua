-- Scenario test (tests/scenarios.rs): the unit stats moho keeps per army,
-- which FAF's score thread reads for the end-of-game screen:
-- units built (`Units_History`, mass / energy value), lost (`Units_Killed`)
-- and killed (`Enemies_Killed`, commanders), per army and per blueprint.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function stat(brain, name) return brain:GetArmyStat(name, 0).Value end
ForkThread(function()
    WaitTicks(2)
    local b1, b2 = ArmyBrains[1], ArmyBrains[2]
    b1:__engine_SetStorage('MASS', 50000) b1:__engine_SetStorage('ENERGY', 500000)
    b1:GiveResource('MASS', 50000) b1:GiveResource('ENERGY', 500000)
    local engineerCost = __blueprints.uel0105.Economy
    local e = CreateUnitHPR('uel0105', 'ARMY_1', 420, GetTerrainHeight(420, 590), 590, 0, 0, 0)
    check(stat(b1, 'Units_History') == 1, 'a unit made finished counts as built: ' .. stat(b1, 'Units_History'))
    check(stat(b1, 'Units_Active') == 1, 'and active')
    check(stat(b1, 'Units_MassValue_Built') == engineerCost.BuildCostMass, 'with its mass value: ' .. stat(b1, 'Units_MassValue_Built'))
    check(stat(b1, 'Units_EnergyValue_Built') == engineerCost.BuildCostEnergy, 'and its energy value')
    check(b1:GetBlueprintStat('Units_History', categories.uel0105) == 1, 'per blueprint too')

    IssueBuildMobile({ e }, Vector(426, 0, 590), 'ueb1101', {})
    local started = false
    for _ = 1, 300 do
        WaitTicks(1)
        if b1:GetBlueprintStat('Units_BeingBuilt', categories.ueb1101) == 1 then started = true break end
    end
    check(started, 'a started construction counts as being built')
    check(stat(b1, 'Units_History') == 1, 'but not as built until it finishes')
    local finished = false
    for _ = 1, 600 do
        WaitTicks(1)
        if stat(b1, 'Units_History') == 2 then finished = true break end
    end
    check(finished, 'the finished power generator counts as built')
    check(b1:GetBlueprintStat('Units_BeingBuilt', categories.ueb1101) == 0, 'and no longer as being built')
    check(b1:GetBlueprintStat('Units_History', categories.STRUCTURE) == 1, 'one structure built')

    -- Kills: army 1's tank kills army 2's tank and commander.
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 430, GetTerrainHeight(430, 600), 600, 0, 0, 0)
    -- Far from army 1's units: the commander's death explosion kills nearby units.
    local victim = CreateUnitHPR('uel0201', 'ARMY_2', 300, GetTerrainHeight(300, 400), 400, 0, 0, 0)
    local acu = CreateUnitHPR('ual0001', 'ARMY_2', 200, GetTerrainHeight(200, 400), 400, 0, 0, 0)
    victim:Kill(tank, 'Normal', 0)
    acu:Kill(tank, 'Normal', 0)
    local tankMass = __blueprints.uel0201.Economy.BuildCostMass
    local acuMass = __blueprints.ual0001.Economy.BuildCostMass
    check(stat(b2, 'Units_Killed') == 2, 'army 2 lost two units: ' .. stat(b2, 'Units_Killed'))
    check(stat(b2, 'Units_MassValue_Lost') == tankMass + acuMass, 'worth their mass')
    check(stat(b1, 'Enemies_Killed') == 2, 'army 1 killed two: ' .. stat(b1, 'Enemies_Killed'))
    check(stat(b1, 'Enemies_MassValue_Destroyed') == tankMass + acuMass, 'worth their mass')
    check(stat(b1, 'Enemies_Commanders_Destroyed') == 1, 'one of them a commander')
    check(b1:GetBlueprintStat('Enemies_Killed', categories.uel0201) == 1, 'kills by the victim\'s blueprint')
    check(b2:GetBlueprintStat('Units_Killed', categories.COMMAND) == 1, 'losses by blueprint')

    -- Damage totals: dealt by the instigator's army and blueprint, received
    -- by the target's, after armour.
    local target = CreateUnitHPR('uel0201', 'ARMY_2', 360, GetTerrainHeight(360, 560), 560, 0, 0, 0)
    local dealtBefore = stat(b1, 'DamageStats_TotalDamageDealt')
    local receivedBefore = stat(b2, 'DamageStats_TotalDamageReceived')
    Damage(tank, target:GetPosition(), target, 100, 'Normal')
    local dealt = stat(b1, 'DamageStats_TotalDamageDealt') - dealtBefore
    check(dealt > 0, 'damage dealt counted: ' .. dealt)
    check(stat(b2, 'DamageStats_TotalDamageReceived') - receivedBefore == dealt, 'and received the same')
    check(b1:GetBlueprintStat('Units_TotalDamageDealt', categories.uel0201) >= dealt, 'per blueprint dealt')
    check(b2:GetBlueprintStat('Units_TotalDamageReceive', categories.uel0201) >= dealt, 'per blueprint received')
    local noInstigator = stat(b2, 'DamageStats_TotalDamageReceived')
    Damage(nil, target:GetPosition(), target, 10, 'Normal')
    check(stat(b2, 'DamageStats_TotalDamageReceived') == noInstigator, 'damage with no instigator isn\'t counted')

    -- A unit killed by its own army isn't an enemy kill.
    local own = CreateUnitHPR('uel0201', 'ARMY_1', 430, GetTerrainHeight(430, 610), 610, 0, 0, 0)
    own:Kill(tank, 'Normal', 0)
    check(stat(b1, 'Enemies_Killed') == 2, 'killing your own unit is no enemy kill')
    check(stat(b1, 'Units_Killed') == 1, 'but a loss: ' .. stat(b1, 'Units_Killed'))

    -- Destroyed units leave the active count.
    WaitTicks(50)
    local active = table.getn(b1:GetListOfUnits(categories.ALLUNITS, false))
    note(string.format('army 1: %d active by stat, %d listed, history %d, killed %d', stat(b1, 'Units_Active'), active, stat(b1, 'Units_History'), stat(b1, 'Units_Killed')))
    check(stat(b1, 'Units_Active') == active, 'Units_Active follows the live units')

    -- The score thread's view (the end-of-game screen).
    WaitSeconds(4)
    local s = import('/lua/sim/score.lua').scoreData.current
    check(s[1].general.built.count == 4, 'score: army 1 built 4: ' .. tostring(s[1].general.built.count))
    check(s[1].general.kills.count == 2, 'score: army 1 killed 2')
    check(s[2].general.lost.count == 2, 'score: army 2 lost 2')

    -- Script stats: a default only types a new stat, its value starts at 0;
    -- a unit stat without a default is nil until made (dev/probes
    -- probe_unit_api).
    local bogus = b1:GetArmyStat('ScenarioBogus', 3)
    check(bogus.Value == 0 and bogus.Name == 'ScenarioBogus' and bogus.Type == 'Integer', 'a new army stat starts at 0: ' .. repr(bogus))
    local u = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 640), 640, 0, 0, 0)
    check(u:GetStat('ScenarioStat') == nil, 'a missing unit stat without a default is nil')
    check(u:GetStat('ScenarioStat', 5).Value == 0, 'a default only types it')
    u:SetStat('ScenarioStat', 7)
    check(u:GetStat('ScenarioStat', 1).Value == 7, 'SetStat then sets it')
    ScenarioResult.done = true
end)
