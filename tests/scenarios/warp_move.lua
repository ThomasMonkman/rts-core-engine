-- Scenario test (tests/scenarios.rs): `Warp` on a unit. A tank warped in
-- the middle of a move gives the move up: on the next tick its queue is empty
-- and it's idle, and it brakes to a halt from the new spot (the real game's
-- stops 2.2 on and stays). A tank warped 10 up is on the ground at once.
-- (dev/probes probe_mov_api W2 / W3, probe_mov_mult WP)
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local function make(x, z) return CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, math.pi / 2, 0) end
    local mover = make(596.5, 124.5)
    mover:SetFireState(1)
    local idle = make(650.5, 118.5)
    IssueMove({ mover }, { 626.5, 0, 124.5 })
    WaitTicks(20)
    local x, y, z = mover:GetPositionXYZ()
    Warp(mover, { x, y, z + 4 })
    local ix, iy, iz = idle:GetPositionXYZ()
    Warp(idle, { ix, iy + 10, iz })
    local _, wy = idle:GetPositionXYZ()
    check(math.abs(wy - GetTerrainHeight(ix, iz)) < 0.01, 'a tank warped 10 up is on the ground at once: ' .. (wy - GetTerrainHeight(ix, iz)))
    WaitTicks(1)
    check(table.getn(mover:GetCommandQueue()) == 0, 'the warped mover has no orders left')
    check(mover:IsIdleState(), 'the warped mover is idle')
    WaitTicks(40)
    local ex, _, ez = mover:GetPositionXYZ()
    check(ex - x < 4 and math.abs(ez - (z + 4)) < 0.5, string.format('the warped mover halted near the new spot: %.2f on', ex - x))
    local _, ly = idle:GetPositionXYZ()
    check(math.abs(ly - GetTerrainHeight(ix, iz)) < 0.01, 'the warped idle tank stays on the ground')
    note(string.format('mover went %.2f on from the warp', ex - x))
    ScenarioResult.done = true
end)
