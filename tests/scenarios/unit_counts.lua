-- Scenario test (tests/scenarios.rs): `GetCurrentUnits` counts finished
-- units only (dev/probes probe_unit_build G3), and `GetResourceConsumed` is
-- 0 for a unit consuming nothing (probe_unit_econ D, K).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 620), 620, 0, 0, 0)
    local idle = CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    WaitTicks(3)
    check(idle:GetResourceConsumed() == 0, 'an idle unit consumes nothing: ' .. idle:GetResourceConsumed())
    IssueBuildFactory({fac}, 'uel0201', 1)
    local building
    for _ = 1, 50 do
        WaitTicks(2)
        building = fac:GetFocusUnit()
        if building then break end
    end
    check(building ~= nil, 'the factory started a tank')
    check(b:GetCurrentUnits(categories.uel0201) == 0, 'a tank under construction is not counted: ' .. b:GetCurrentUnits(categories.uel0201))
    for _ = 1, 200 do
        WaitTicks(3)
        if building and not building:BeenDestroyed() and not building:IsBeingBuilt() then break end
    end
    check(b:GetCurrentUnits(categories.uel0201) == 1, 'the finished tank is: ' .. b:GetCurrentUnits(categories.uel0201))
    ScenarioResult.done = true
end)
