-- Scenario test (tests/scenarios.rs): an attack order closes to the same
-- distance on a structure as on a unit, about 1.8 inside weapon range (the
-- real game's T1 tank, range 18, stops 16.05 from both: dev/probes
-- probe_re2_orders ATK).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local t1 = make('ueb1101', 'ARMY_2', 425, 600) t1:SetMaxHealth(30000) t1:SetHealth(nil, 30000)
    local t2 = make('uel0201', 'ARMY_2', 425, 640) t2:SetMaxHealth(30000) t2:SetHealth(nil, 30000) t2:SetFireState(1)
    local a1 = make('uel0201', 'ARMY_1', 375, 600) local a2 = make('uel0201', 'ARMY_1', 375, 640)
    a1:SetFireState(0) a2:SetFireState(0)
    IssueAttack({ a1 }, t1) IssueAttack({ a2 }, t2)
    WaitTicks(160)
    local function d(a, t) local p, q = a:GetPosition(), t:GetPosition() return VDist2(p[1], p[3], q[1], q[3]) end
    local ds, du = d(a1, t1), d(a2, t2)
    check(ds > 15.7 and ds < 16.4, 'structure target: ' .. ds)
    check(du > 15.7 and du < 16.4, 'unit target: ' .. du)
    ScenarioResult.done = true
end)
