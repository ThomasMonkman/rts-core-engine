-- Scenario test (tests/scenarios.rs): a platoon's transport calls (the
-- campaign's ScenarioPlatoonAI uses UnloadAllAtLocation): LoadUnits puts
-- the platoon's land units in its transport, UnloadAllAtLocation sets them
-- down at the point (dev/probes probe_ai_transport).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local function make(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
    local t1, t2 = make('uel0201', 400, 600), make('uel0201', 403, 600)
    local tr = make('uea0107', 400, 608)
    local p = brain:MakePlatoon('Lift', '')
    brain:AssignUnitsToPlatoon(p, { t1, t2 }, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(p, { tr }, 'Scout', 'None')
    local cmd = p:LoadUnits(categories.LAND)
    check(p:IsCommandsActive(cmd), 'loading')
    local n = 0
    while n < 300 and table.getn(tr:GetCargo()) < 2 do WaitTicks(1); n = n + 1 end
    check(table.getn(tr:GetCargo()) == 2, 'both tanks aboard: ' .. table.getn(tr:GetCargo()))
    cmd = p:UnloadAllAtLocation({ 430, GetTerrainHeight(430, 620), 620 })
    n = 0
    while n < 400 and table.getn(tr:GetCargo()) > 0 do WaitTicks(1); n = n + 1 end
    check(table.getn(tr:GetCargo()) == 0, 'unloaded')
    local x, _, z = t1:GetPositionXYZ()
    check(math.abs(x - 430) < 12 and math.abs(z - 620) < 12, string.format('set down near the point: %.1f, %.1f', x, z))
    ScenarioResult.done = true
end)
