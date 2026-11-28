-- Scenario test (tests/scenarios.rs): `EntityCategoryFilterDown` leaves out
-- a destroyed unit at once, while `EntityCategoryContains` still answers
-- from its blueprint (dev/probes probe_api_spot `misc.after_destroy5` / `8`).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local a = CreateUnitHPR('uel0201', 'ARMY_1', 500, GetTerrainHeight(500, 500), 500, 0, 0, 0)
    local b = CreateUnitHPR('uel0201', 'ARMY_1', 510, GetTerrainHeight(510, 500), 500, 0, 0, 0)
    check(table.getn(EntityCategoryFilterDown(categories.LAND, { a, b })) == 2, 'both alive: both kept')
    a:Destroy()
    local out = EntityCategoryFilterDown(categories.LAND, { a, b })
    check(table.getn(out) == 1 and out[1] == b, 'the destroyed unit is left out in the same tick')
    check(EntityCategoryContains(categories.LAND, a), 'Contains still answers for it')
    WaitTicks(2)
    out = EntityCategoryFilterDown(categories.LAND, { a, b })
    check(table.getn(out) == 1 and out[1] == b, 'and after its OnDestroy')
    check(table.getn(EntityCategoryFilterDown(categories.LAND, { 'uel0201' })) == 1, 'blueprint ids still filter')
    ScenarioResult.done = true
end)
