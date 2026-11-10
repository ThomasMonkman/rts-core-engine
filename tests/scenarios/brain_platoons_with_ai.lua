-- Scenario test (tests/scenarios.rs): AIBrain:GetNumPlatoonsWithAI(plan) counts
-- the army's platoons running that AI plan (MakePlatoon's second argument),
-- ignoring case, as moho's _stricmp does.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain, other = GetArmyBrain('ARMY_1'), GetArmyBrain('ARMY_2')
    local a = brain:MakePlatoon('A', 'ReclaimAIProbe')
    brain:MakePlatoon('B', 'reclaimaiprobe')
    brain:MakePlatoon('ReclaimAIProbe', 'OtherAIProbe')
    other:MakePlatoon('C', 'ReclaimAIProbe')
    check(brain:GetNumPlatoonsWithAI('ReclaimAIProbe') == 2, 'two platoons run the plan, ignoring case')
    check(brain:GetNumPlatoonsWithAI('OtherAIProbe') == 1, 'the name is not the plan')
    check(brain:GetNumPlatoonsWithAI('Missing') == 0, 'no platoon runs Missing')
    check(brain:GetNumPlatoonsWithAI(5) == 0, 'a non-string plan counts nothing')
    check(other:GetNumPlatoonsWithAI('ReclaimAIProbe') == 1, 'only the brain\'s own army counts')
    brain:DisbandPlatoon(a)
    WaitTicks(1)
    check(brain:GetNumPlatoonsWithAI('ReclaimAIProbe') == 1, 'a disbanded platoon no longer counts')
    ScenarioResult.done = true
end)
