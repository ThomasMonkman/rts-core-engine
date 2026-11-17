-- Scenario test (tests/scenarios.rs): unit states an order gives, as the
-- real game reports them (dev/probes probe_unit_orders, probe_unit_build
-- G9): `Guarding` under a guard order, `Patrolling` under a patrol or an
-- aggressive move, and `NoCost` on a killed unit before it's destroyed.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local function make(x, z) return CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
    local a, b, c, d = make(400, 600), make(404, 600), make(408, 600), make(412, 600)
    check(not a:IsUnitState('Guarding') and not a:IsUnitState('Patrolling'), 'idle: neither')
    IssueGuard({ a }, b)
    IssuePatrol({ b }, { 420, GetTerrainHeight(420, 620), 620 })
    IssuePatrol({ b }, { 400, GetTerrainHeight(400, 620), 620 })
    IssueAggressiveMove({ c }, { 440, GetTerrainHeight(440, 600), 600 })
    WaitTicks(3)
    check(a:IsUnitState('Guarding') and not a:IsUnitState('Patrolling'), 'guard order: Guarding')
    check(b:IsUnitState('Patrolling'), 'patrol: Patrolling')
    check(c:IsUnitState('Patrolling'), 'aggressive move: Patrolling')
    IssueClearCommands({ a })
    WaitTicks(2)
    check(not a:IsUnitState('Guarding'), 'the order cleared: not Guarding')
    check(not d:IsUnitState('NoCost'), 'alive: no NoCost')
    IssueKillSelf({ d })
    WaitTicks(2)
    check(not d:BeenDestroyed() and d:IsUnitState('NoCost'), 'killed: NoCost through its death interval')
    ScenarioResult.done = true
end)
