-- Scenario test (tests/scenarios.rs): two engineers capturing one unit share
-- the capture, as in the real game (dev/probes probe_unit_build G7): about
-- half the time of one (a third engineer captures a second generator alone
-- alongside), about one capture's cost, and both get `OnStopCapture` (none
-- `OnFailedCapture`).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function ground(x, z) return GetTerrainHeight(x, z) end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    local e1 = CreateUnitHPR('uel0105', 'ARMY_1', 400, ground(400, 598), 598, 0, 0, 0)
    local e2 = CreateUnitHPR('uel0105', 'ARMY_1', 400, ground(400, 602), 602, 0, 0, 0)
    -- T1 power generators: BuildCostEnergy 750.
    local prey = CreateUnitHPR('ueb1101', 'ARMY_2', 405, ground(405, 600), 600, 0, 0, 0)
    local e3 = CreateUnitHPR('uel0105', 'ARMY_1', 420, ground(420, 620), 620, 0, 0, 0)
    local prey2 = CreateUnitHPR('ueb1101', 'ARMY_2', 425, ground(425, 620), 620, 0, 0, 0)
    local calls = {}
    for _, e in { e1, e2 } do
        local eng = e
        for _, name in { 'OnStopCapture', 'OnFailedCapture' } do
            local n, orig = name, eng[name]
            eng[n] = function(self, t) table.insert(calls, (eng == e1 and 'e1 ' or 'e2 ') .. n) return orig(self, t) end
        end
    end
    WaitTicks(2)
    local energy = b:GetEconomyStored('ENERGY')
    IssueCapture({e1, e2}, prey)
    local started, finished
    -- (`WaitTicks(2)` waits one tick.)
    for t = 1, 400 do
        WaitTicks(2)
        if not started and e1:IsUnitState('Capturing') and e2:IsUnitState('Capturing') then started = t end
        if prey:BeenDestroyed() then finished = t break end
    end
    WaitTicks(2)
    local spent = energy - b:GetEconomyStored('ENERGY')
    check(table.getn(b:GetListOfUnits(categories.ueb1101, false) or {}) == 1, 'army 1 owns the generator once')
    -- The lone capture, timed alone.
    IssueCapture({e3}, prey2)
    local s3, f3
    for t = 1, 400 do
        WaitTicks(2)
        if not s3 and e3:IsUnitState('Capturing') then s3 = t end
        if prey2:BeenDestroyed() then f3 = t break end
    end
    local two, one = finished and started and finished - started, f3 and s3 and f3 - s3
    note(string.format('two captors %s ticks, one %s; cost %.1f energy; calls: %s', tostring(two), tostring(one), spent, table.concat(calls, ', ')))
    check(two and one and math.abs(2 * two - one) <= 3, 'two captors take half the time of one')
    check(math.abs(spent - 750) < 60, 'and about one capture\'s cost: ' .. spent)
    check(table.concat(calls, ', ') == 'e1 OnStopCapture, e2 OnStopCapture', 'both finish it together')
    ScenarioResult.done = true
end)
