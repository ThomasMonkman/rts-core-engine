-- Scenario test (tests/scenarios.rs): AI brain and platoon queries that the
-- real game answers in a particular way (dev/probes probe_ai_brain,
-- probe_ai_platoon, probe_ai_misc).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local a = CreateUnitHPR('uel0201', 'ARMY_1', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    local b = CreateUnitHPR('uel0201', 'ARMY_1', 430, GetTerrainHeight(430, 600), 600, 0, 0, 0)
    WaitTicks(2)

    -- GetUnitsAroundPoint / GetNumUnitsAroundPoint: the radius is strict.
    local c = {420, GetTerrainHeight(420, 600), 600}
    local n = function(r) return brain:GetNumUnitsAroundPoint(categories.MOBILE, c, r) end
    check(n(10) == 1, 'a unit exactly 10 away is out at r=10: ' .. n(10))
    check(n(10.5) == 2, 'and in at r=10.5: ' .. n(10.5))
    check(n(0) == 0, 'r=0 finds nothing: ' .. n(0))
    check(table.getn(brain:GetUnitsAroundPoint(categories.MOBILE, c, 0)) == 0, 'GetUnitsAroundPoint r=0 is empty')

    ScenarioResult.done = true
end)
