-- Scenario test (tests/scenarios.rs): brain:RemoveArmyStatsTrigger removes a
-- trigger by name, as the AI energy manager does
-- (EnergyManagerBrainComponent.lua:278): a removed trigger never fires.
-- Triggers are first checked at tick 11, as in the real game (dev/probes
-- probe_re2_misc E). A trigger on a stat that doesn't exist yet never
-- fires, a stat SetArmyStat makes with a number is an integer, and
-- same-tick triggers fire in registration order (dev/probes probe_re3_ai
-- ST1).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local fired = {}
    -- (FAF's own EnergyDepleted fires too: this army has no energy.)
    brain.OnStatsTrigger = function(self, name) if name ~= 'EnergyDepleted' then table.insert(fired, name) end end
    brain:SetArmyStatsTrigger('Economy_Stored_Energy', 'Kept', 'GreaterThanOrEqual', 0)
    brain:SetArmyStatsTrigger('Economy_Ratio_Energy', 'Removed', 'GreaterThanOrEqual', 0)
    -- Removed under another stat name: the name is what counts.
    local ok, err = pcall(brain.RemoveArmyStatsTrigger, brain, 'Economy_Stored_Energy', 'Removed')
    check(ok, 'RemoveArmyStatsTrigger runs: ' .. tostring(err))
    if not ok then WARN('stats trigger scenario: ' .. tostring(err)) end
    while GetGameTick() < 10 do WaitTicks(1) end
    check(table.getn(fired) == 0, 'nothing fires before tick 11: ' .. table.concat(fired, ' '))
    WaitTicks(3)
    check(table.concat(fired, ' ') == 'Kept', 'only the kept trigger fired: ' .. table.concat(fired, ' '))
    fired = {}
    brain:SetArmyStat('Scenario_Z', 10.5)
    check(brain:GetArmyStat('Scenario_Z', 0).Value == 10, 'a stat SetArmyStat makes is an integer')
    brain:SetArmyStat('Scenario_A', 3)
    brain:SetArmyStatsTrigger('Scenario_Z', 'Zed', 'GreaterThan', 5)
    brain:SetArmyStatsTrigger('Scenario_Missing', 'Dead', 'GreaterThanOrEqual', 5)
    brain:SetArmyStatsTrigger('Scenario_A', 'Ay', 'LessThan', 5)
    WaitTicks(2)
    brain:SetArmyStat('Scenario_Missing', 10)
    WaitTicks(3)
    check(table.concat(fired, ' ') == 'Zed Ay', 'registration order, the dead trigger never fires: ' .. table.concat(fired, ' '))
    ScenarioResult.done = true
end)
