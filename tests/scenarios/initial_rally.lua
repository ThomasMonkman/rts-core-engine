-- Scenario test (tests/scenarios.rs): a naval factory (InitialRallyZ 10)
-- starts with a rally point 10 in front of it, turned with the factory, and
-- its first unit walks there.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
    -- Facing +x (a quarter turn): its z axis points along +x.
    local f = CreateUnitHPR('ueb0103', 'ARMY_1', 480, GetSurfaceHeight(480, 625), 625, 0, math.pi / 2, 0)
    WaitTicks(1)
    local r = f:GetRallyPoint()
    note(string.format('rally %.1f, %.1f', r[1], r[3]))
    check(math.abs(r[1] - 490) < 0.5 and math.abs(r[3] - 625) < 0.5, string.format('rally 10 ahead, turned with the factory: %.1f, %.1f', r[1], r[3]))
    IssueBuildFactory({f}, 'ues0103', 1)
    local unit
    -- Up to 800 ticks (`WaitTicks(n)` waits n - 1).
    for _ = 1, 400 do
        WaitTicks(3)
        for _, u in b:GetListOfUnits(categories.ues0103, false) do if not u:IsBeingBuilt() then unit = u end end
        if unit and unit:IsIdleState() and VDist2(unit:GetPosition()[1], unit:GetPosition()[3], r[1], r[3]) < 3 then break end
    end
    local p = unit and unit:GetPosition()
    if p then note(string.format('unit ended at %.1f, %.1f', p[1], p[3])) end
    check(p and VDist2(p[1], p[3], r[1], r[3]) < 3, 'the first unit walked to the rally point')
    -- A factory whose blueprint gives no InitialRallyX/Z (FAF's land
    -- factories) rallies 5 in front of itself, the engine's default
    -- `InitialRallyZ` (dev/probes probe_unit_build G3: factory at 736,224,
    -- rally 736,229), from its creation: moho's constructor issues it for
    -- every FACTORY.
    local land = CreateUnitHPR('ueb0101', 'ARMY_1', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    local lr = land:GetRallyPoint()
    note(string.format('land factory rally %.1f, %.1f', lr[1], lr[3]))
    check(lr and math.abs(lr[1] - 420) < 0.5 and math.abs(lr[3] - 605) < 0.5, 'a land factory rallies 5 in front of itself')
    IssueBuildFactory({land}, 'uel0105', 1)
    local eng
    for _ = 1, 200 do
        WaitTicks(3)
        for _, u in b:GetListOfUnits(categories.uel0105, false) do if not u:IsBeingBuilt() then eng = u end end
        if eng and eng:IsIdleState() then break end
    end
    check(eng and eng:IsIdleState(), 'its first unit rolls off and goes idle')
    local ep = eng and eng:GetPosition()
    if ep then note(string.format('engineer ended at %.1f, %.1f', ep[1], ep[3])) end
    ScenarioResult.done = true
end)
