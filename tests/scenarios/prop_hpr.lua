-- Scenario test (tests/scenarios.rs): CreatePropHPR takes its heading in
-- degrees, as in the real game (dev/probes probe_unit_misc R: heading 1 gives
-- quaternion y 0.009).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local p = CreatePropHPR('/env/evergreen/props/rocks/fieldstone01_prop.bp', 400, GetTerrainHeight(400, 600), 600, 1, 0, 0)
    local o = p:GetOrientation()
    check(math.abs(o[2]) > 0.008 and math.abs(o[2]) < 0.0095, 'heading 1 is one degree: quaternion y ' .. tostring(o[2]))
    local q = CreatePropHPR('/env/evergreen/props/rocks/fieldstone01_prop.bp', 404, GetTerrainHeight(404, 600), 600, 180, 0, 0)
    local o2 = q:GetOrientation()
    check(math.abs(math.abs(o2[2]) - 1) < 0.001, 'heading 180 is a half turn: quaternion y ' .. tostring(o2[2]))
    ScenarioResult.done = true
end)
