-- Scenario test (tests/scenarios.rs): ARMY_1 / ARMY_2 have no AI; checks go
-- into ScenarioResult, which the test reads once `done` is set.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function done() ScenarioResult.done = true end
local function ground(x, z) return GetTerrainHeight(x, z) end
local function rich(army)
    local b = ArmyBrains[army]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    return b
end
-- An engineer captures an enemy tank: it ends up in the engineer's army.
ForkThread(function()
    WaitTicks(2)
    local brain = rich(1)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, ground(400, 600), 600, 0, 0, 0)
    local tank = CreateUnitHPR('uel0201', 'ARMY_2', 406, ground(406, 600), 600, 0, 0, 0)
    IssueStop({tank})
    -- Hold fire: point blank, the tank would kill the engineer first.
    tank:SetFireState(1)
    WaitTicks(2)
    local energy = brain:GetEconomyStored('ENERGY')
    IssueCapture({eng}, tank)
    WaitTicks(20)
    check(eng:IsUnitState('Capturing'), 'capturing once in range')
    for _ = 1, 100 do
        WaitTicks(5)
        if tank:BeenDestroyed() then break end
    end
    WaitTicks(2)
    check(tank:BeenDestroyed(), 'the enemy tank was transferred (old unit gone)')
    -- Moho's capture task draws the target's capture energy (FAF's
    -- `GetBlueprintCaptureCost`: its BuildCostEnergy, 266) over the capture.
    local spent = energy - brain:GetEconomyStored('ENERGY')
    check(math.abs(spent - 266) < 1, 'the capture cost its energy: ' .. tostring(spent))
    check(table.getn(brain:GetListOfUnits(categories.uel0201, false) or {}) == 1, 'army 3 now owns a tank')
    done()
end)
