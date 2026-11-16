-- Scenario test (tests/scenarios.rs): orders the real game refuses or keeps
-- out of the command queue (dev/probes probe_re2_orders SILO / FA / SAC).
-- A silo build order leaves the queue empty yet builds; only a factory
-- takes IssueFactoryAssist; a sacrifice on a damaged finished structure is
-- kept, on a full-health one dropped.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
    brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
    local silo = make('ueb2108', 400, 600)
    silo:SetAutoMode(false)
    WaitTicks(2)
    IssueSiloBuildTactical({ silo })
    check(table.getn(silo:GetCommandQueue()) == 0, 'the silo build order is not queued')
    WaitTicks(3)
    check(silo:IsUnitState('SiloBuildingAmmo'), 'but the silo builds a missile for it')

    local f0 = make('ueb0101', 440, 560)
    local fac = make('ueb0101', 470, 560)
    local eng = make('uel0105', 440, 575)
    local tank = make('uel0201', 445, 575)
    IssueFactoryAssist({ fac, eng, tank }, f0)
    check(table.getn(fac:GetCommandQueue()) == 1, 'a factory takes IssueFactoryAssist')
    check(table.getn(eng:GetCommandQueue()) == 0, 'an engineer does not')
    check(table.getn(tank:GetCommandQueue()) == 0, 'a tank does not')

    local full = make('ueb1101', 410, 640)
    local dmg = make('ueb1101', 410, 660)
    dmg:SetHealth(nil, 100)
    local s1 = make('ual0105', 400, 640)
    local s2 = make('ual0105', 400, 660)
    IssueSacrifice({ s1 }, full) IssueSacrifice({ s2 }, dmg)
    WaitTicks(2)
    check(table.getn(s1:GetCommandQueue()) == 0, 'a sacrifice on a full-health structure is dropped')
    check(table.getn(s2:GetCommandQueue()) == 1, 'one on a damaged structure is kept')
    for _ = 1, 100 do
        if s2:BeenDestroyed() then break end
        WaitTicks(1)
    end
    check(s2:BeenDestroyed(), 'and the engineer is consumed')
    check(dmg:GetHealth() > 100, 'repairing the structure: ' .. dmg:GetHealth())
    ScenarioResult.done = true
end)
