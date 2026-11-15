-- Scenario test (tests/scenarios.rs): `SetSpeedMult(m)` on a surface unit
-- takes m under 1 squared and m of 1 or more as it is; an aircraft takes it
-- as it is. The real game's T1 tank (top speed 3.40) runs 0.85 at 0.5, 2.75
-- at 0.9 and 5.10 at 1.5; the interceptor (15) 7.5 at 0.5 (dev/probes
-- probe_mov_mult).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local function make(bp, m, x, z, air)
        local y = GetTerrainHeight(x, z)
        local u = CreateUnitHPR(bp, 'ARMY_1', x, y, z, 0, math.pi / 2, 0)
        u:SetFireState(1)
        u:SetSpeedMult(m)
        IssueMove({ u }, { x + (air and 300 or 30), 0, z })
        return u
    end
    local tanks = { { 0.5, 0.85 }, { 0.9, 2.754 }, { 1.5, 5.10 } }
    for i, t in tanks do t.u = make('uel0201', t[1], 596.5, 70.5 + (i - 1) * 6) end
    local plane = make('uea0102', 0.5, 100, 340, true)
    -- Top speed reached well before the end of the move.
    WaitTicks(60)
    local function speed(u) local vx, _, vz = u:GetVelocity() return 10 * math.sqrt(vx * vx + vz * vz) end
    for _, t in tanks do
        local v = speed(t.u)
        check(math.abs(v - t[2]) < 0.05, string.format('tank at SetSpeedMult(%.1f) cruises %.2f, want %.2f', t[1], v, t[2]))
        note(string.format('tank %.1f: %.2f', t[1], v))
    end
    local pv = speed(plane)
    check(pv > 7 and pv < 8.5, string.format('interceptor at SetSpeedMult(0.5) cruises %.2f, want ~7.5 (linear)', pv))
    note(string.format('plane 0.5: %.2f', pv))
    ScenarioResult.done = true
end)
