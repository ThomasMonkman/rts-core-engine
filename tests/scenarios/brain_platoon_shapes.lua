-- Scenario test (tests/scenarios.rs): brain / platoon call shapes the real
-- game shows (dev/probes probe_ai_brain, probe_ai_platoon, probe_ai_misc):
-- an empty squad's GetSquadUnits is nil, MakePlatoon without a plan is nil,
-- PlatoonExists on a non-object and FormPlatoon with no unit lines raise,
-- DecideWhatToBuild takes a nil template and matches the type in any case,
-- GetListOfUnits' requireBuilt still lists unfinished units.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local t = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 405, GetTerrainHeight(405, 600), 600, 0, 0, 0)
    local p = brain:MakePlatoon('Shapes', '')
    brain:AssignUnitsToPlatoon(p, { t }, 'Attack', 'None')
    check(p:GetSquadUnits('Attack')[1] == t, 'the Attack squad')
    check(p:GetSquadUnits('Guard') == nil, 'an empty squad is nil')
    p:MoveToLocation({ 420, 0, 600 }, false, 'Guard') -- an order to an empty squad is fine
    check(brain:MakePlatoon('NoPlan') == nil, 'no plan: nil')
    check(not pcall(brain.MakePlatoon, brain), 'no arguments raise')
    check(not pcall(brain.PlatoonExists, brain, nil), 'PlatoonExists(nil) raises')
    check(not pcall(brain.PlatoonExists, brain, {}), 'PlatoonExists({}) raises')
    check(brain:PlatoonExists(p), 'a live platoon exists')
    local pool = brain:GetPlatoonUniquelyNamed('ArmyPool')
    check(not pcall(pool.FormPlatoon, pool, { 'X' }, 1, { 400, 0, 600 }, 10), 'FormPlatoon with no unit lines raises')
    check(brain:DecideWhatToBuild(eng, 'T1LandFactory', nil) == nil, 'nil template: nothing')
    check(brain:DecideWhatToBuild(eng, 't1landfactory', { { 'T1LandFactory', 'ueb0101' } }) == 'ueb0101', 'type name in any case')
    local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 420, GetTerrainHeight(420, 620), 620, 0, 0, 0)
    IssueBuildFactory({ fac }, 'uel0201', 1)
    WaitTicks(15)
    local building = nil
    for _, u in brain:GetListOfUnits(categories.uel0201, false, true) do
        if u:GetFractionComplete() < 1 then building = u end
    end
    check(building ~= nil, 'requireBuilt lists the tank being built')
    ScenarioResult.done = true
end)
