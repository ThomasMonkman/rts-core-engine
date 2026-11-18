-- Scenario test (tests/scenarios.rs): a platoon emptied by moving its units
-- out lasts to the next beat, as in the real game (dev/probes
-- probe_ai_platoon: `P2 exists emptied now` true, `OnDestroy P2` the beat
-- after). RNGAI's air platoons merge by moving one platoon's units into
-- another and restarting the emptied platoon's state machine, whose first
-- lines (forked, so later in the same beat) call `GetBrain` and
-- `CalculatePlatoonThreat` on it: those must work that beat, and calls raise
-- only once it has ended. A platoon given units again before then lives on.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    local function tank(x)
        return CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, 600), 600, 0, 0, 0)
    end
    local a, b = brain:MakePlatoon('a', 'none'), brain:MakePlatoon('b', 'none')
    local t1, t2 = tank(400), tank(404)
    brain:AssignUnitsToPlatoon(a, {t1}, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(b, {t2}, 'Attack', 'None')
    local destroyedAt
    b:AddDestroyCallback(function() destroyedAt = GetGameTick() end)
    WaitTicks(1)
    -- The merge: a takes b's units, then b's machine restarts.
    local merged = GetGameTick()
    brain:AssignUnitsToPlatoon(a, b:GetPlatoonUnits(), 'Attack', 'None')
    local started = {}
    ForkThread(function(p)
        started.tick = GetGameTick()
        started.brain = p:GetBrain()
        started.threat = p:CalculatePlatoonThreat('Surface', categories.ALLUNITS)
    end, b)
    WaitTicks(1)
    check(started.tick == merged, 'the restarted machine runs in the merge beat (ran at ' .. tostring(started.tick) .. ')')
    check(started.brain == brain, 'the emptied platoon still has its brain that beat')
    check(started.threat == 0, 'and its threat is 0 (got ' .. tostring(started.threat) .. ')')
    check(destroyedAt == merged + 1, 'it ends the beat after (OnDestroy at ' .. tostring(destroyedAt) .. ', merged ' .. merged .. ')')
    check(not brain:PlatoonExists(b), 'and is gone then')
    local ok = pcall(function() return b:GetSquadUnits('Attack') end)
    check(not ok, 'calls on it raise once it has ended')

    -- Emptied and given a unit back in the same beat: it lives on.
    local c = brain:MakePlatoon('c', 'none')
    local t3 = tank(408)
    brain:AssignUnitsToPlatoon(c, {t3}, 'Attack', 'None')
    WaitTicks(1)
    brain:AssignUnitsToPlatoon(a, {t3}, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(c, {t3}, 'Attack', 'None')
    WaitTicks(2)
    check(brain:PlatoonExists(c), 'a platoon refilled in the beat it emptied lives on')
    check(table.getn(c:GetPlatoonUnits()) == 1, 'with its unit')
    ScenarioResult.done = true
end)
