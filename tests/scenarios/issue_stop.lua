-- Scenario test (tests/scenarios.rs): the sim's `IssueStop` appends a Stop
-- behind the queue and clears nothing, as in the real game (dev/probes
-- probe_mov_misc ST, probe_unit_build G3): a unit with moves queued drives
-- every leg, and a factory's current build carries on.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0)
    local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 620), 620, 0, 0, 0)
    WaitTicks(2)
    IssueMove({tank}, { 410, 0, 580 })
    IssueMove({tank}, { 410, 0, 590 })
    IssueBuildFactory({fac}, 'uel0201', 1)
    WaitTicks(5)
    IssueStop({tank, fac})
    local q = tank:GetCommandQueue()
    check(table.getn(q) == 3, 'Stop is appended behind the two moves: queue ' .. table.getn(q))
    check(q[3] and q[3].commandType == 1, 'the last entry is the Stop')
    check(table.getn(fac:GetCommandQueue()) == 2, 'the factory keeps its build, Stop behind it')
    WaitTicks(3)
    check(fac:IsUnitState('Building'), 'the factory is still building')
    for _ = 1, 100 do
        WaitTicks(3)
        if tank:IsIdleState() and table.getn(tank:GetCommandQueue()) == 0 then break end
    end
    local p = tank:GetPosition()
    note(string.format('tank ended at %.1f, %.1f', p[1], p[3]))
    check(VDist2(p[1], p[3], 410, 590) < 2, 'the tank drove both legs')
    ScenarioResult.done = true
end)
