-- Scenario test (tests/scenarios.rs): `OrientFromDir` takes a direction of
-- any length; yaw then pitch, no roll (dev/probes probe_api_spot
-- `core.OrientFromDir*`, the real game's values to 3 places).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function near(q, want, what)
    local ok = true
    for i = 1, 4 do if math.abs(q[i] - want[i]) > 0.001 then ok = false end end
    check(ok, string.format('%s: {%.3f %.3f %.3f %.3f}', what, q[1], q[2], q[3], q[4]))
end
ForkThread(function()
    near(OrientFromDir(Vector(1, 0, 0)), { 0, 0.7071, 0, 0.7071 }, '+X')
    near(OrientFromDir(Vector(0, 0, 1)), { 0, 0, 0, 1 }, '+Z')
    near(OrientFromDir(Vector(1, 2, 3)), { -0.2745, 0.1537, 0.0445, 0.9481 }, '(1, 2, 3)')
    near(OrientFromDir(Vector(0, 1, 0)), { -0.7071, 0, 0, 0.7071 }, '+Y')
    near(OrientFromDir(Vector(0, 0, 0)), { 0, 0, 0, 1 }, 'zero')
    near(OrientFromDir(Vector(5, 0, 0)), { 0, 0.7071, 0, 0.7071 }, '+X, long')
    ScenarioResult.done = true
end)
