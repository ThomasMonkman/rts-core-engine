-- Scenario test (tests/scenarios.rs): AIBrain:GetNumPlatoonsTemplateNamed(name)
-- counts the army's platoons with that name (MakePlatoon's first argument, a
-- formed platoon's template name), ignoring case, as moho's _stricmp does.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain, other = GetArmyBrain('ARMY_1'), GetArmyBrain('ARMY_2')
    local a = brain:MakePlatoon('EngineerReclaimT1', '')
    brain:MakePlatoon('engineerreclaimt1', '')
    brain:MakePlatoon('Other', '')
    other:MakePlatoon('EngineerReclaimT1', '')
    check(brain:GetNumPlatoonsTemplateNamed('EngineerReclaimT1') == 2, 'two platoons named so, ignoring case')
    check(brain:GetNumPlatoonsTemplateNamed('Other') == 1, 'one other')
    check(brain:GetNumPlatoonsTemplateNamed('Missing') == 0, 'no platoon named Missing')
    check(brain:GetNumPlatoonsTemplateNamed(5) == 0, 'a non-string name counts nothing')
    check(other:GetNumPlatoonsTemplateNamed('EngineerReclaimT1') == 1, 'only the brain\'s own army counts')
    brain:DisbandPlatoon(a)
    WaitTicks(1)
    check(brain:GetNumPlatoonsTemplateNamed('EngineerReclaimT1') == 1, 'a disbanded platoon no longer counts')
    ScenarioResult.done = true
end)
