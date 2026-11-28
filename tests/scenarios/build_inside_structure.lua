-- Scenario test (tests/scenarios.rs): a builder standing inside a structure
-- (a factory's new engineer still on the pad) walks out before it starts a
-- build it could reach from where it stands; one outside builds on the
-- spot. As in the real game (dev/probes probe_padbuild2 `placed`, `rolled`):
-- otherwise the factory's roll-off waits for the whole build.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 20000) b:GiveResource('ENERGY', 400000)
    local x, z = 400, 600
    CreateUnitHPR('ueb0101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local inside = CreateUnitHPR('uel0105', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local outside = CreateUnitHPR('uel0105', 'ARMY_1', x + 30, GetTerrainHeight(x + 30, z - 4), z - 4, 0, 0, 0)
    WaitTicks(3)
    -- Both sites are 7.2 away: inside a T1 engineer's reach of 8.
    IssueBuildMobile({ inside }, { x - 6, GetTerrainHeight(x - 6, z + 4), z + 4 }, 'ueb1101', {})
    IssueBuildMobile({ outside }, { x + 24, GetTerrainHeight(x + 24, z), z }, 'ueb1101', {})
    WaitTicks(3)
    check(inside:GetFocusUnit() == nil and inside:IsUnitState('Moving'), 'the builder inside the factory walks first')
    -- (The one outside turns to face its site, then builds.)
    for _ = 1, 30 do
        WaitTicks(1)
        if outside:GetFocusUnit() then break end
    end
    local o = outside:GetPosition()
    check(outside:GetFocusUnit() ~= nil and VDist2(o[1], o[3], x + 30, z - 4) < 0.05, 'the builder outside builds where it stands: focus ' .. tostring(outside:GetFocusUnit() ~= nil) .. ' moved ' .. VDist2(o[1], o[3], x + 30, z - 4))
    for _ = 1, 150 do
        WaitTicks(1)
        if inside:GetFocusUnit() then break end
    end
    local p = inside:GetPosition()
    check(inside:GetFocusUnit() ~= nil, 'and then builds')
    check(VDist2(p[1], p[3], x, z) > 2, 'from outside the factory: ' .. VDist2(p[1], p[3], x, z))
    ScenarioResult.done = true
end)
