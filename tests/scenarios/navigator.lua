-- Scenario test (tests/scenarios.rs): the navigator's SetGoal sends an idle
-- unit somewhere (FAF's load-into-transport gathers units with it) and
-- GetCurrentTargetPos reports where it is steering (AirTransportUnit reads it
-- before dropping units).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    WaitTicks(2)
    local nav = tank:GetNavigator()
    -- Not going anywhere, the target is the centre of the unit's cell at the
    -- ground's height (moho always gives a point; real game, probe_navtarget).
    local function idle(what)
        local t, at = nav:GetCurrentTargetPos(), tank:GetPosition()
        local x, z = math.floor(at[1]) + 0.5, math.floor(at[3]) + 0.5
        check(t ~= nil and math.abs(t[1] - x) < 0.01 and math.abs(t[3] - z) < 0.01 and math.abs(t[2] - GetTerrainHeight(x, z)) < 0.01,
            what .. ': ' .. repr(t) .. ' at ' .. repr(at))
    end
    idle('an idle unit\'s target is its cell')
    nav:SetGoal({ 420, 0, 600 })
    local target = nav:GetCurrentTargetPos()
    check(target ~= nil and math.abs(target[1] - 420) < 1 and math.abs(target[3] - 600) < 1, 'the target is the goal')
    WaitTicks(150)
    local p = tank:GetPosition()
    check(VDist2(p[1], p[3], 420, 600) < 1.5, string.format('it went to the goal: %.1f, %.1f', p[1], p[3]))
    idle('arrived: the target is its cell')
    -- An order takes over from a goal, and the goal doesn't come back after.
    nav:SetGoal({ 440, 0, 600 })
    IssueMove({ tank }, { 420, GetTerrainHeight(420, 620), 620 })
    WaitTicks(150)
    p = tank:GetPosition()
    check(VDist2(p[1], p[3], 420, 620) < 1.5, string.format('the order won: %.1f, %.1f', p[1], p[3]))
    -- AbortMove drops a goal.
    nav:SetGoal({ 400, 0, 620 })
    WaitTicks(3)
    nav:AbortMove()
    idle('AbortMove clears the goal')
    -- FAF's load-into-transport gathers tech 2 units with SetGoal while a
    -- tech 3 unit boards first (load-in-transport.lua:352).
    local transport = CreateUnitHPR('uea0104', 'ARMY_1', 430, GetTerrainHeight(430, 640) + 10, 640, 0, 0, 0)
    local units = { CreateUnitHPR('uel0303', 'ARMY_1', 380, GetTerrainHeight(380, 640), 640, 0, 0, 0) }
    for i, bp in { 'uel0202', 'uel0205', 'uel0203' } do
        table.insert(units, CreateUnitHPR(bp, 'ARMY_1', 380 + i * 4, GetTerrainHeight(380 + i * 4, 630), 630, 0, 0, 0))
    end
    WaitTicks(2)
    local goals = 0
    local setGoal = moho.navigator_methods.SetGoal
    moho.navigator_methods.SetGoal = function(self, p) goals = goals + 1 return setGoal(self, p) end
    local loaded, why = pcall(import('/lua/sim/commands/load-in-transport.lua').LoadIntoTransports, units, { transport }, true, false)
    moho.navigator_methods.SetGoal = setGoal
    check(loaded, 'LoadIntoTransports runs: ' .. tostring(why))
    check(goals == 3, 'it gathered the tech 2 units with SetGoal: ' .. goals)
    -- Moho fills Transport.TransportClass = 1 into blueprints that leave it
    -- out (uel0106 has none), so FAF's slot maths works for tech 1 units.
    check(__blueprints['uel0106'].Transport.TransportClass == 1, 'uel0106 transport class defaults to 1')
    local okT1, slots = pcall(import('/lua/sim/commands/load-in-transport.lua').GetTransportSlotRequirements, units[2])
    check(okT1 and slots ~= nil, 'slot requirements for any unit: ' .. tostring(slots))
    ScenarioResult.done = true
end)
