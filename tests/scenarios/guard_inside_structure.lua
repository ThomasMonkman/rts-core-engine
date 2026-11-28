-- Scenario test (tests/scenarios.rs): an engineer standing inside a
-- structure (a factory's new engineer still on the pad) that is told to
-- guard walks out before anything else, as in the real game (dev/probes
-- probe_padassist): guarding its own factory it ends outside it (ours stayed
-- on the pad and the factory's roll-off never ended), and it helps a build
-- in its reach only once outside.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function outside(u, x, z)
    local p = u:GetPosition()
    return math.max(math.abs(p[1] - x), math.abs(p[3] - z)) > 2.5
end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 20000) b:GiveResource('ENERGY', 400000)
    local x, z = 400, 600
    -- Its own factory.
    local f1 = CreateUnitHPR('ueb0101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local own = CreateUnitHPR('uel0105', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    -- A build 7.2 from a second factory's centre, inside a T1 engineer's reach.
    local x2 = x + 40
    CreateUnitHPR('ueb0101', 'ARMY_1', x2, GetTerrainHeight(x2, z), z, 0, 0, 0)
    local helper = CreateUnitHPR('uel0105', 'ARMY_1', x2, GetTerrainHeight(x2, z), z, 0, 0, 0)
    local builder = CreateUnitHPR('uel0105', 'ARMY_1', x2 - 10, GetTerrainHeight(x2 - 10, z + 4), z + 4, 0, 0, 0)
    WaitTicks(3)
    IssueBuildMobile({ builder }, { x2 - 6, GetTerrainHeight(x2 - 6, z + 4), z + 4 }, 'ueb1105', {})
    for _ = 1, 40 do
        WaitTicks(1)
        if builder:GetFocusUnit() then break end
    end
    check(builder:GetFocusUnit() ~= nil, 'the builder starts the storage')
    IssueGuard({ own }, f1)
    IssueGuard({ helper }, builder)
    local early = false
    for _ = 1, 150 do
        WaitTicks(1)
        if helper:GetFocusUnit() and not outside(helper, x2, z) then early = true end
        if outside(own, x, z) and helper:GetFocusUnit() then break end
    end
    local p = own:GetPosition()
    check(outside(own, x, z), 'the engineer guarding its own factory leaves it: ' .. (p[1] - x) .. ',' .. (p[3] - z))
    check(not early, 'the helper does not work from inside the factory')
    check(helper:GetFocusUnit() ~= nil and outside(helper, x2, z), 'and helps from outside it')
    ScenarioResult.done = true
end)
