-- Scenario test (tests/scenarios.rs): a unit still under construction is in
-- its army's pool platoon but can't be formed into a platoon, as in the real
-- game (dev/probes probe_poolbuild). The stock game's AI forms a scout
-- platoon as soon as the pool can fill one and turns the scout's cloak on:
-- on an unfinished scout that raised.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 20000) b:GiveResource('ENERGY', 400000)
    local x, z = 400, 600
    local pool = b:GetPlatoonUniquelyNamed('ArmyPool')
    local template = { 'Scouts', '', { 'uel0101', 1, 1, 'Attack', 'None' } }
    local f = CreateUnitHPR('ueb0101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    WaitTicks(3)
    check(not pool:CanFormPlatoon(template, 1, { x, 0, z }, 40), 'no scout yet')
    IssueBuildFactory({ f }, 'uel0101', 1)
    local scout
    for _ = 1, 20 do
        WaitTicks(1)
        scout = f.UnitBeingBuilt
        if scout then break end
    end
    check(scout ~= nil and scout:IsBeingBuilt(), 'the factory starts a scout')
    local listed = false
    for _, u in pool:GetPlatoonUnits() do
        if u == scout then listed = true end
    end
    check(listed, 'the unfinished scout is in the pool')
    check(not pool:CanFormPlatoon(template, 1, { x, 0, z }, 40), 'but can not be formed into a platoon')
    for _ = 1, 100 do
        WaitTicks(1)
        if not scout:IsBeingBuilt() then break end
    end
    check(not scout:IsBeingBuilt(), 'the scout is finished')
    check(pool:CanFormPlatoon(template, 1, { x, 0, z }, 40), 'and then can be')
    local p = pool:FormPlatoon(template, 1, { x, 0, z }, 40)
    check(p ~= nil and p:GetPlatoonUnits()[1] == scout, 'FormPlatoon takes it')
    ScenarioResult.done = true
end)
