-- Scenario test (tests/scenarios.rs): a Fatboy's external factory helper
-- (`uel0401ef`, EconomyThreatLevel 2000) adds no threat; the real game's
-- Fatboy cell reads 563 Economy (dev/probes probe_ai_threat).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local fatboy = CreateUnitHPR('uel0401', 'ARMY_1', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    WaitTicks(35) -- past a 30-tick threat map update
    local helper = fatboy.ExternalFactory
    check(helper and not helper:BeenDestroyed(), 'the Fatboy has its external factory')
    local p = fatboy:GetPosition()
    local eco = brain:GetThreatAtPosition(p, 0, true, 'Economy', 1)
    local structures = brain:GetThreatAtPosition(p, 0, true, 'Structures', 1)
    check(math.abs(eco - 563) < 0.01, 'economy threat is the Fatboy alone: ' .. eco)
    check(structures == 0, 'no structure threat: ' .. structures)
    ScenarioResult.done = true
end)
