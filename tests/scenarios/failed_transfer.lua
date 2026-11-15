-- Scenario test (tests/scenarios.rs): ChangeUnitArmy into an army at its
-- unit cap is refused (nil, the unit stays) and the receiving army's brain
-- hears OnFailedUnitTransfer, as in the real game (dev/probes
-- probe_unit_api `army`); with room, the transfer goes through. A commander
-- is refused through FAF's hook unless `noRestrictions` is passed, then it
-- transfers (probe_unit_api `givecdr`).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    CreateUnitHPR('uel0201', 'ARMY_2', 410, GetTerrainHeight(410, 600), 600, 0, 0, 0)
    local failed = {0, 0}
    for i = 1, 2 do
        ArmyBrains[i].OnFailedUnitTransfer = function(self) failed[i] = failed[i] + 1 end
    end
    SetArmyUnitCap(2, math.ceil(GetArmyUnitCostTotal(2)))
    local moved = ChangeUnitArmy(tank, 2)
    check(moved == nil, 'refused at the cap')
    check(not tank.Dead and tank:GetArmy() == 1, 'the unit stays with its army')
    check(failed[1] == 0 and failed[2] == 1, 'the receiver hears it once: ' .. failed[1] .. ', ' .. failed[2])
    SetArmyUnitCap(2, GetArmyUnitCap(2) + 10)
    moved = ChangeUnitArmy(tank, 2)
    check(moved ~= nil and moved:GetArmy() == 2, 'goes through with room')
    check(failed[2] == 1, 'no failure with room')
    local cdr = CreateUnitHPR('uel0001', 'ARMY_1', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    check(ChangeUnitArmy(cdr, 2) == nil and not cdr:BeenDestroyed(), 'a commander stays without noRestrictions')
    local given = ChangeUnitArmy(cdr, 2, true)
    check(given ~= nil and given:GetArmy() == 2 and cdr:BeenDestroyed(), 'and transfers with it')
    ScenarioResult.done = true
end)
