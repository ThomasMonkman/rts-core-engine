-- Scenario test (tests/scenarios.rs): a Fatboy's external factory helper
-- (`uel0401ef`, EconomyThreatLevel 2000) adds no threat; the real game's
-- Fatboy cell reads 563 Economy (dev/probes probe_ai_threat). Asking with the
-- brain's own army index gives no unit threat (`own:self=0.00` there).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local owner = GetArmyBrain('ARMY_2')
    local fatboy = CreateUnitHPR('uel0401', 'ARMY_2', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    local watcher = CreateUnitHPR('uel0201', 'ARMY_1', 428, GetTerrainHeight(428, 600), 600, 0, 0, 0) -- sees it
    fatboy:SetFireState(1) -- hold fire: the watcher has to live
    watcher:SetFireState(1)
    WaitTicks(35) -- past a 30-tick threat map update
    local helper = fatboy.ExternalFactory
    check(helper and not helper:BeenDestroyed(), 'the Fatboy has its external factory')
    local p = fatboy:GetPosition()
    local eco = brain:GetThreatAtPosition(p, 0, true, 'Economy', 2)
    local structures = brain:GetThreatAtPosition(p, 0, true, 'Structures', 2)
    check(math.abs(eco - 563) < 0.01, 'economy threat is the Fatboy alone: ' .. eco)
    check(structures == 0, 'no structure threat: ' .. structures)
    local own = owner:GetThreatAtPosition(p, 0, true, 'Economy', 2)
    check(own == 0, 'own army index gives no unit threat: ' .. own)
    ScenarioResult.done = true
end)
