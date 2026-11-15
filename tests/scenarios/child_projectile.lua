-- Scenario test (tests/scenarios.rs): CreateChildProjectile makes a child that
-- flies along the parent's course at its own blueprint's InitialSpeed, with
-- the parent's launcher as its launcher (probe_wpn_proj2, probe_wpn_misc2
-- `child *`: under a parent slowed to 6/s, a 20/s child steps 2.0 a tick and
-- GetLauncher is the unit).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG('SCN FAIL ', msg) end end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    local l = CreateUnitHPR('ueb5101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local parent = l:CreateProjectile(LOS, 0, 10, 0, 1, 0, 0)
    parent:SetVelocity(6)
    WaitTicks(1)
    local child = parent:CreateChildProjectile(LOS)
    local p0 = child:GetPosition()
    WaitTicks(1)
    local p1 = child:GetPosition()
    local step = p1[1] - p0[1]
    check(math.abs(step - 2.0) < 0.01, 'the child steps its own 20/s: ' .. step)
    check(child:GetLauncher() == l, 'its launcher is the parent\'s launcher')
    ScenarioResult.done = true
end)
