-- Scenario test (tests/scenarios.rs): a uniquely named platoon is a pool
-- that outlives being emptied. FAF's AI makes its `TransportPool` once
-- (`transportutilities.lua` `CreateTransportPool`: `MakePlatoon` then
-- `UniquelyNamePlatoon('TransportPool')`) and keeps assigning transports
-- back to it (`AssignTransportToPool`), though it is empty whenever every
-- transport is out or dead. An unnamed platoon emptied the same way ends the
-- beat after (dev/probes probe_ai_platoon `P2`).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    local function unit(bp, x)
        return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, 600), 600, 0, 0, 0)
    end
    local pool = brain:MakePlatoon('TransportPool', 'none')
    pool:UniquelyNamePlatoon('TransportPool')
    local plain = brain:MakePlatoon('plain', 'none')
    local work = brain:MakePlatoon('work', 'none')
    local tr, tank = unit('uea0107', 400), unit('uel0201', 410)
    brain:AssignUnitsToPlatoon(pool, {tr}, 'Support', 'None')
    brain:AssignUnitsToPlatoon(plain, {tank}, 'Attack', 'None')
    WaitTicks(1)
    -- Both emptied by moving their only unit out.
    brain:AssignUnitsToPlatoon(work, {tr, tank}, 'Attack', 'None')
    WaitTicks(5)
    check(brain:PlatoonExists(pool), 'the emptied transport pool still exists')
    check(not brain:PlatoonExists(plain), 'an unnamed platoon emptied the same way has ended')
    local ok, err = pcall(function() brain:AssignUnitToPlatoon(pool, tr, 'Support', '') end)
    check(ok, 'a transport goes back into the pool (' .. tostring(err) .. ')')
    check(table.getn(pool:GetPlatoonUnits()) == 1, 'and is in it')
    -- Its only unit dies: still the pool.
    tr:Kill()
    WaitTicks(5)
    local tr2 = unit('uea0107', 420)
    ok, err = pcall(function() brain:AssignUnitToPlatoon(pool, tr2, 'Support', '') end)
    check(ok and brain:PlatoonExists(pool), 'the pool outlives its last transport dying (' .. tostring(err) .. ')')
    check(brain:GetPlatoonUniquelyNamed('TransportPool') == pool, 'and keeps its name')
    ScenarioResult.done = true
end)
