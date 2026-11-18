-- Scenario test (tests/scenarios.rs): FAF's `StructureUnit.OnCreate` tilts a
-- `FlattenSkirt` structure to the ground's gradient on top of the
-- orientation it is made with. An upgrade starts from the old structure's
-- heading, not its lean, so it leans as much as the old one, not twice
-- as much (a Cybran shield generator on the slope at 392.5, 568.5).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
-- The angle between the unit's up axis and the world's.
local function lean(u)
    local o = u:GetOrientation()
    local up_y = 1 - 2 * (o[1] * o[1] + o[3] * o[3])
    return math.acos(math.min(1, up_y))
end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
    local x, z = 392.5, 568.5
    local old = CreateUnitHPR('urb4202', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    WaitTicks(5)
    local before = lean(old)
    check(before > 0.05, string.format('the T2 shield leans on the slope (%.3f rad)', before))
    IssueUpgrade({ old }, 'urb4204')
    WaitTicks(10)
    local new = old.UnitBeingBuilt
    check(new ~= nil and new.UnitId == 'urb4204', 'the upgrade started')
    if new then
        local after = lean(new)
        note(string.format('lean T2 %.3f rad, T3 %.3f rad', before, after))
        check(math.abs(after - before) < 0.02, string.format('the upgrade leans as the old one (%.3f), not more: %.3f', before, after))
        check(math.abs(new:GetHeading() - old:GetHeading()) < 0.01, 'the upgrade keeps the heading')
    end
    ScenarioResult.done = true
end)
