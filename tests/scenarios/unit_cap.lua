-- Scenario test (tests/scenarios.rs): a factory at the army's unit cap
-- doesn't start its build; the brain hears OnUnitCapLimitReached once, and
-- the build starts when the cap is raised.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    local factory = CreateUnitHPR('ueb0101', 'ARMY_1', 360, GetTerrainHeight(360, 560), 560, 0, 0, 0)
    WaitTicks(2)
    local reached = 0
    b.OnUnitCapLimitReached = function(self) reached = reached + 1 end
    SetArmyUnitCap(1, math.ceil(GetArmyUnitCostTotal(1)))
    IssueBuildFactory({factory}, 'uel0201', 1)
    WaitTicks(20)
    check(factory:GetFocusUnit() == nil, 'no build at the cap')
    check(reached == 1, 'brain told once: ' .. reached)
    SetArmyUnitCap(1, GetArmyUnitCap(1) + 10)
    WaitTicks(10)
    check(factory:GetFocusUnit() ~= nil, 'build starts once the cap allows')
    -- Scripted spawns go through the same cap (moho's `Sim::CreateUnit`): at
    -- the cap, CreateUnitHPR raises and the brain hears it; a cap of 0
    -- refuses too; ignoring the cap lets it through.
    SetArmyUnitCap(1, math.ceil(GetArmyUnitCostTotal(1)))
    local before = reached
    local ok, err = pcall(CreateUnitHPR, 'uel0201', 'ARMY_1', 380, GetTerrainHeight(380, 560), 560, 0, 0, 0)
    check(not ok and string.find(tostring(err), 'CreateUnit%(uel0201%) failed'), 'CreateUnitHPR past the cap fails: ' .. tostring(err))
    check(reached == before + 1, 'the brain hears the scripted spawn hit the cap')
    SetArmyUnitCap(1, 0)
    check(not pcall(CreateUnitHPR, 'uel0201', 'ARMY_1', 380, GetTerrainHeight(380, 560), 560, 0, 0, 0), 'cap 0 refuses')
    SetIgnoreArmyUnitCap(1, true)
    local made = CreateUnitHPR('uel0201', 'ARMY_1', 380, GetTerrainHeight(380, 560), 560, 0, 0, 0)
    check(made ~= nil, 'ignoring the cap spawns')
    SetIgnoreArmyUnitCap(1, false)
    ScenarioResult.done = true
end)
