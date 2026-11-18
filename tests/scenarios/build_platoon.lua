-- Scenario test (tests/scenarios.rs): the brain's platoon build calls, as in
-- the real game (dev/probes probe_ai_build). `CanBuildPlatoon` answers one
-- factory per template line (the first that can build it); `BuildPlatoon`
-- puts line i on factory i.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    local fac1 = CreateUnitHPR('ueb0101', 'ARMY_1', 400.5, GetTerrainHeight(400.5, 600.5), 600.5, 0, 0, 0)
    local fac2 = CreateUnitHPR('ueb0101', 'ARMY_1', 420.5, GetTerrainHeight(420.5, 600.5), 600.5, 0, 0, 0)
    local PT = { 'P', '', { 'uel0201', 1, 2, 'Attack', 'None' }, { 'uel0101', 1, 1, 'Scout', 'None' } }
    local can = b:CanBuildPlatoon(PT, { fac1, fac2 })
    check(can and table.getn(can) == 2 and can[1] == fac1 and can[2] == fac1, 'CanBuildPlatoon gives the first able factory per line')
    check(b:CanBuildPlatoon({ 'P3', '', { 'uel0303', 1, 1, 'Attack', 'None' } }, { fac1, fac2 }) == nil, 'nil when a line has no factory')
    b:BuildPlatoon(PT, { fac1, fac2 }, 2)
    local n1, n2 = table.getn(fac1:GetCommandQueue()), table.getn(fac2:GetCommandQueue())
    check(n1 == 4 and n2 == 2, string.format('line 1 (2 x 2) on factory 1, line 2 (1 x 2) on factory 2: %d, %d', n1, n2))
    check(b:BuildUnit(fac2, 'uel0201', 3) == 3, 'BuildUnit returns the count')
    ScenarioResult.done = true
end)
