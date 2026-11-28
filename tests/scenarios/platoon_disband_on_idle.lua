-- Scenario test (tests/scenarios.rs): `Platoon:DisbandOnIdle` ends the
-- platoon once none of its units has an order left, its units going back to
-- the army pool (FA exe 0x700820; dev/probes probe_ai_misc: an idle platoon
-- is gone after the call). Survival maps send every wave this way
-- (Survival Horde EE: `MakePlatoon`, `DisbandOnIdle`, then units and
-- attack-moves in the same beat, and `DisbandOnIdle` again).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    local pool = brain:GetPlatoonUniquelyNamed('ArmyPool')
    local function unit(x)
        return CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, 600), 600, 0, 0, 0)
    end
    local function inPool(u)
        for _, member in pool:GetPlatoonUnits() do
            if member == u then return true end
        end
        return false
    end

    -- No orders: it lasts the beat it was told in, and is gone the next.
    local idle, a = brain:MakePlatoon('', ''), unit(400)
    brain:AssignUnitsToPlatoon(idle, {a}, 'Attack', 'None')
    local destroyed = false
    idle:AddDestroyCallback(function() destroyed = true end)
    idle:DisbandOnIdle()
    check(brain:PlatoonExists(idle), 'an idle platoon lasts the beat DisbandOnIdle was called in')
    WaitTicks(2)
    check(not brain:PlatoonExists(idle), 'an idle platoon has ended the beat after')
    check(destroyed, 'its OnDestroy ran')
    check(inPool(a), 'its unit is back in the army pool')

    -- Told while empty, then given units and an order in the same beat: it
    -- lives until the order is done.
    local wave, b, c = brain:MakePlatoon('', ''), unit(420), unit(424)
    wave:DisbandOnIdle()
    brain:AssignUnitsToPlatoon(wave, {b, c}, 'Attack', 'None')
    wave:MoveToLocation({430, GetTerrainHeight(430, 612), 612}, false)
    wave:DisbandOnIdle()
    WaitTicks(5)
    check(brain:PlatoonExists(wave), 'a platoon with an order under way lives on')
    check(not inPool(b), 'and keeps its units')
    local waited = 0
    while brain:PlatoonExists(wave) and waited < 300 do
        WaitTicks(5)
        waited = waited + 5
    end
    check(not brain:PlatoonExists(wave), 'it ends once its units have no orders left')
    check(inPool(b) and inPool(c), 'and its units are back in the army pool')
    check(not b.Dead and not c.Dead, 'alive')

    -- Without DisbandOnIdle an idle platoon stays.
    local stays, d = brain:MakePlatoon('', ''), unit(440)
    brain:AssignUnitsToPlatoon(stays, {d}, 'Attack', 'None')
    WaitTicks(5)
    check(brain:PlatoonExists(stays), 'a platoon not told to disband stays while idle')
    ScenarioResult.done = true
end)
