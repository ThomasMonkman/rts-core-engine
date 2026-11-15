-- Scenario test (tests/scenarios.rs): the platoon target finders go by
-- intel (dev/probes probe_ai_find): FindClosestUnit / FindPrioritizedUnit
-- return enemy units the army detects now and structures it has seen, never
-- an unseen one; FindClosestUnit honours canAttack and its 'Ally' leaves out
-- the platoon's own army.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetFireState(1)
    return u
end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local t1 = make('uel0201', 'ARMY_1', 400, 600)
    local t2 = make('uel0201', 'ARMY_1', 403, 600)
    local near = make('uel0201', 'ARMY_2', 412, 600)
    local farpg = make('ueb1101', 'ARMY_2', 400, 660)
    local faracu = make('uel0001', 'ARMY_2', 460, 600)
    for _, u in { near, farpg, faracu, t1, t2 } do u:SetMaxHealth(50000); u:SetHealth(u, 50000) end
    local p = brain:MakePlatoon('FindIntel', '')
    brain:AssignUnitsToPlatoon(p, { t1, t2 }, 'Attack', 'None')
    p:SetPrioritizedTargetList('Attack', { categories.STRUCTURE, categories.COMMAND, categories.ALLUNITS })
    WaitTicks(10)
    check(p:FindClosestUnit('Attack', 'Enemy', true, categories.ALLUNITS) == near, 'the seen tank is closest')
    check(p:FindClosestUnit('Attack', 'Enemy', true, categories.STRUCTURE) == nil, 'the unseen generator is not found')
    check(p:FindClosestUnit('Attack', 'Enemy', true, categories.COMMAND) == nil, 'the unseen commander is not found')
    check(p:FindClosestUnit('Attack', 'Ally', false, categories.ALLUNITS) == nil, "'Ally' is not the own army")
    check(p:FindPrioritizedUnit('Attack', 'Enemy', true, { 400, 0, 600 }, 100) == near, 'prioritized: only the seen tank')
    -- canAttack: tanks can't hit a flying interceptor.
    local air = make('uea0102', 'ARMY_2', 405, 595)
    WaitTicks(3)
    check(p:FindClosestUnit('Attack', 'Enemy', false, categories.AIR) == air, 'the seen interceptor without canAttack')
    check(p:FindClosestUnit('Attack', 'Enemy', true, categories.AIR) == nil, 'not with canAttack')
    ScenarioResult.done = true
end)
