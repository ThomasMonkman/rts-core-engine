-- Scenario test (tests/scenarios.rs): bindings the real game accepts with
-- fewer arguments than ours used to (dev/probes probe_api_arity).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    local brain = ArmyBrains[1]
    local template = { 'T', '', { 'uel0201', 1, 1, 'Attack', 'None' } }
    local ok, r = pcall(brain.CanBuildPlatoon, brain, template)
    check(ok, 'CanBuildPlatoon(template) without factories: ' .. tostring(r))
    check(ok and r == nil, 'and no factory can build it')
    local ok2, r2 = pcall(brain.CanBuildPlatoon, brain, template, {})
    check(ok2 and r2 == nil, 'an empty factory list: nil')
    ScenarioResult.done = true
end)
