-- Scenario test (tests/scenarios.rs): `GetUnitsAroundPoint(..., 'Enemy')`
-- keeps listing an enemy structure the brain has seen once sight of it is
-- lost (it keeps a blip); a mobile unit drops out (dev/probes
-- probe_unit_intel observer-gone).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local pgen = CreateUnitHPR('ueb1101', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 402, GetTerrainHeight(402, 604), 604, 0, 0, 0)
    local watcher = CreateUnitHPR('uel0201', 'ARMY_2', 406, GetTerrainHeight(406, 600), 600, 0, 0, 0)
    for _, u in { pgen, tank, watcher } do u:SetFireState(1) end
    local brain = ArmyBrains[2]
    local function listed()
        local seen = {}
        for _, u in brain:GetUnitsAroundPoint(categories.ALLUNITS, { 400, 0, 600 }, 15, 'Enemy') or {} do seen[u] = true end
        return seen
    end
    WaitTicks(5)
    local before = listed()
    check(before[pgen] and before[tank], 'both listed while seen')
    watcher:Destroy()
    WaitTicks(5)
    local after = listed()
    check(after[pgen], 'the seen structure is still listed after sight is lost')
    check(not after[tank], 'the tank is not')
    ScenarioResult.done = true
end)
