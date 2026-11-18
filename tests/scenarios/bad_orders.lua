-- Scenario test (tests/scenarios.rs): orders naming things that don't exist
-- fail without taking the game down, as in the real game (dev/probes
-- probe_ai_misc, probe_unit_orders): `BuildStructure` with an unknown
-- blueprint returns false and queues nothing; an `IssueScript` with an
-- unknown `TaskName` is queued, then dropped. The engineer then builds as
-- normal.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    WaitTicks(2)
    local r = b:BuildStructure(eng, 'zzz9999', { 424, 600, 0 }, false)
    check(r == false, 'BuildStructure with an unknown blueprint returns false: ' .. tostring(r))
    check(table.getn(eng:GetCommandQueue()) == 0, 'and queues nothing')
    -- A relative location is an offset from the army's start position, not
    -- from the builder.
    local sx, sz = b:GetArmyStartPos()
    b:BuildStructure(eng, 'ueb1101', { 4, 6, 0 }, true)
    local q = eng:GetCommandQueue()
    check(q[1] and math.abs(q[1].x - (sx + 4)) < 0.01 and math.abs(q[1].z - (sz + 6)) < 0.01,
        string.format('relative BuildStructure queues at the start + offset: %s,%s (start %s,%s)', tostring(q[1] and q[1].x), tostring(q[1] and q[1].z), sx, sz))
    IssueClearCommands({eng})
    -- Orders the unit has no command cap for are refused at issue: a tank
    -- can't dive or overcharge (dev/probes probe_unit_orders Q).
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 430, GetTerrainHeight(430, 610), 610, 0, 0, 0)
    IssueDive({tank})
    IssueOverCharge({tank}, tank)
    check(table.getn(tank:GetCommandQueue()) == 0, 'a tank takes no dive or overcharge order')
    check(select('#', IssueClearCommands({tank})) == 0, 'IssueClearCommands returns nothing')
    IssueBuildMobile({eng}, { 424, GetTerrainHeight(424, 600), 600 }, 'zzz9999', {})
    check(table.getn(eng:GetCommandQueue()) == 0, 'IssueBuildMobile with an unknown blueprint queues nothing')
    IssueScript({eng}, { TaskName = 'NoSuchScenarioTask' })
    check(table.getn(eng:GetCommandQueue()) == 1, 'IssueScript with an unknown task is queued')
    WaitTicks(5)
    check(table.getn(eng:GetCommandQueue()) == 0, 'and then dropped')
    -- The engineer still works.
    IssueBuildMobile({eng}, { 428, GetTerrainHeight(428, 600), 600 }, 'ueb1101', {})
    local built
    for _ = 1, 300 do
        WaitTicks(3)
        for _, u in b:GetListOfUnits(categories.ueb1101, false) do if not u:IsBeingBuilt() then built = u end end
        if built then break end
    end
    check(built ~= nil, 'a real build afterwards finishes')
    ScenarioResult.done = true
end)
