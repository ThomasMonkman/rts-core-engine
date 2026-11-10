-- Scenario test (tests/scenarios.rs): AIBrain:IsAnyEngineerBuilding(category),
-- true only while an ENGINEER unit is building (or assisting) something in
-- the category; factories and queued orders don't count.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
    brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
    local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 340, GetTerrainHeight(340, 600), 600, 0, 0, 0)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local helper = CreateUnitHPR('uel0105', 'ARMY_1', 396, GetTerrainHeight(396, 600), 600, 0, 0, 0)
    WaitTicks(2)
    check(not brain:IsAnyEngineerBuilding(categories.ALLUNITS), 'nothing building yet')
    IssueBuildFactory({fac}, 'uel0105', 1)
    WaitTicks(10)
    check(not brain:IsAnyEngineerBuilding(categories.uel0105), 'a factory is not an engineer')
    IssueBuildMobile({eng}, {410.5, GetTerrainHeight(410.5, 600.5), 600.5}, 'ueb1101', {})
    IssueBuildMobile({eng}, {400.5, GetTerrainHeight(400.5, 614.5), 614.5}, 'ueb2101', {})
    local building = false
    for _ = 1, 200 do
        WaitTicks(1)
        if eng:IsUnitState('Building') then building = true break end
    end
    check(building, 'the engineer started building')
    check(brain:IsAnyEngineerBuilding(categories.ENERGYPRODUCTION), 'building a power generator')
    check(not brain:IsAnyEngineerBuilding(categories.DEFENSE), 'the queued point defence does not count')
    local target = eng:GetFocusUnit()
    if target then
        IssueGuard({helper}, target)
        local assisting = false
        for _ = 1, 200 do
            WaitTicks(1)
            if helper:IsUnitState('Building') then assisting = true break end
        end
        check(assisting, 'the helper started assisting')
        IssueClearCommands({eng})
        WaitTicks(2)
        check(not eng:IsUnitState('Building'), 'the builder stopped')
        check(helper:GetFocusUnit() == target, 'the helper is still on the build')
        check(brain:IsAnyEngineerBuilding(categories.ENERGYPRODUCTION), 'assisting counts')
    else
        check(false, 'no focus unit')
    end
    ScenarioResult.done = true
end)
