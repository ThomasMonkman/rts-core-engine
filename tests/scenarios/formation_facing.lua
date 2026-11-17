-- Scenario test (tests/scenarios.rs): a formation move ends with every
-- member facing the order's heading. The real game's six tanks, form-moved
-- 40 along +x with heading 0, all finish facing +z (yaw 0.00); ours kept the
-- headings they arrived with (dev/probes probe_mov_misc FM1..6).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function yaw(u)
    local q = u:GetOrientation()
    local qx, qy, qz, qw = q[1], q[2], q[3], q[4]
    return math.atan2(2 * (qx * qz + qw * qy), 1 - 2 * (qx * qx + qy * qy))
end
ForkThread(function()
    WaitTicks(2)
    local units = {}
    for i = 0, 5 do
        local x, z = 330.5 + math.mod(i, 3) * 3, 920.5 + math.floor(i / 3) * 3
        local u = CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, math.pi / 2, 0)
        u:SetFireState(1)
        table.insert(units, u)
    end
    WaitTicks(2)
    IssueFormMove(units, { 372.5, 0, 922.5 }, 'AttackFormation', 0)
    for t = 1, 400 do
        WaitTicks(1)
        local busy = false
        for _, u in units do if table.getn(u:GetCommandQueue()) > 0 then busy = true end end
        if not busy then break end
    end
    local s = ''
    for i, u in units do
        check(table.getn(u:GetCommandQueue()) == 0, 'member ' .. i .. ' finished')
        check(math.abs(yaw(u)) < 0.05, string.format('member %d faces the formation heading: yaw %.2f', i, yaw(u)))
        s = s .. string.format('%.2f ', yaw(u))
    end
    note('yaws ' .. s)
    ScenarioResult.done = true
end)
