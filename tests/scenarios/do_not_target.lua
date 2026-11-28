-- Scenario test (tests/scenarios.rs): `SetDoNotTarget(true)` keeps a unit out
-- of target searches, but an attack order on it keeps firing, and the unit
-- reads the `DoNotTarget` state (dev/probes probe_unit_orders O5).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function ground(x, z) return GetTerrainHeight(x, z) end
ForkThread(function()
    WaitTicks(2)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, ground(400, 600), 600, 0, 0, 0)
    local hidden = CreateUnitHPR('ueb1101', 'ARMY_2', 412, ground(412, 600), 600, 0, 0, 0)
    local ordered = CreateUnitHPR('ueb1101', 'ARMY_2', 400, ground(400, 612), 612, 0, 0, 0)
    hidden:SetDoNotTarget(true)
    ordered:SetDoNotTarget(true)
    check(hidden:IsUnitState('DoNotTarget'), 'SetDoNotTarget sets the DoNotTarget state')
    WaitTicks(60)
    check(hidden:GetHealth() == hidden:GetMaxHealth() and ordered:GetHealth() == ordered:GetMaxHealth(), 'neither is picked as a target')
    IssueAttack({tank}, ordered)
    WaitTicks(80)
    check(ordered:GetHealth() < ordered:GetMaxHealth(), 'an attack order on it fires')
    check(hidden:GetHealth() == hidden:GetMaxHealth(), 'the other stays untouched')
    ScenarioResult.done = true
end)
