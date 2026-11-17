-- Scenario test (tests/scenarios.rs): `SetImmobile(true)` on a tank at
-- speed halts it on the spot. The real game's stands at the same position
-- two ticks later; ours braked on 2.2 over 8 ticks (dev/probes
-- probe_mov_api I2).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local u = CreateUnitHPR('uel0201', 'ARMY_1', 596.5, GetTerrainHeight(596.5, 130.5), 130.5, 0, math.pi / 2, 0)
    u:SetFireState(1)
    IssueMove({ u }, { 636.5, 0, 130.5 })
    WaitTicks(25)
    local vx, _, vz = u:GetVelocity()
    check(10 * math.sqrt(vx * vx + vz * vz) > 3, 'the tank is at speed')
    u:SetImmobile(true)
    local x0 = u:GetPositionXYZ()
    WaitTicks(2)
    local x2 = u:GetPositionXYZ()
    WaitTicks(10)
    local x12 = u:GetPositionXYZ()
    check(x2 - x0 < 0.5, string.format('halted at once: %.2f on after 2 ticks', x2 - x0))
    check(math.abs(x12 - x2) < 0.001, string.format('and stays: %.3f more', x12 - x2))
    note(string.format('went %.2f on', x12 - x0))
    ScenarioResult.done = true
end)
