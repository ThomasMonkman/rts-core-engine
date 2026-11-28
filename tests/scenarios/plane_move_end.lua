-- Scenario test (tests/scenarios.rs): a winged plane's move order ends only
-- once it's over the point, after circling back onto it. The real game's
-- interceptor at SetSpeedMult(2) (30 a second) passes the goal, circles in
-- and goes idle 0.24 from it at v 0.26, 17 s later; ours used to end the
-- order 15 short at full speed (dev/probes probe_mov_mult I2).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local x, z = 100, 370
    local u = CreateUnitHPR('uea0102', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, math.pi / 2, 0)
    u:SetFireState(1)
    u:SetSpeedMult(2)
    local gx, gz = x + 300, z
    IssueMove({ u }, { gx, 0, gz })
    local passed = false
    for t = 1, 500 do
        WaitTicks(1)
        local px, _, pz = u:GetPositionXYZ()
        if px > gx then passed = true end
        if table.getn(u:GetCommandQueue()) == 0 then
            local d = math.sqrt((px - gx) * (px - gx) + (pz - gz) * (pz - gz))
            local vx, _, vz = u:GetVelocity()
            local v = 10 * math.sqrt(vx * vx + vz * vz)
            check(passed, 'the move ended only after the plane went past the point')
            check(d < 2, string.format('the move ended over the point: %.2f from it', d))
            check(v < 15, string.format('the move ended slowed down: v %.2f', v))
            note(string.format('done at tick %d, %.2f from the goal, v %.2f', t, d, v))
            ScenarioResult.done = true
            return
        end
    end
    check(false, 'the move never ended')
    ScenarioResult.done = true
end)
