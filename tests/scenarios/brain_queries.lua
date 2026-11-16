-- Scenario test (tests/scenarios.rs): AI brain and platoon queries that the
-- real game answers in a particular way (dev/probes probe_ai_brain,
-- probe_ai_platoon, probe_ai_misc).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local a = CreateUnitHPR('uel0201', 'ARMY_1', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    local b = CreateUnitHPR('uel0201', 'ARMY_1', 430, GetTerrainHeight(430, 600), 600, 0, 0, 0)
    WaitTicks(2)

    -- GetUnitsAroundPoint / GetNumUnitsAroundPoint: the radius is strict.
    local c = {420, GetTerrainHeight(420, 600), 600}
    local n = function(r) return brain:GetNumUnitsAroundPoint(categories.MOBILE, c, r) end
    check(n(10) == 1, 'a unit exactly 10 away is out at r=10: ' .. n(10))
    check(n(10.5) == 2, 'and in at r=10.5: ' .. n(10.5))
    check(n(0) == 0, 'r=0 finds nothing: ' .. n(0))
    check(table.getn(brain:GetUnitsAroundPoint(categories.MOBILE, c, 0)) == 0, 'GetUnitsAroundPoint r=0 is empty')

    -- GetPlatoonsList leaves out platoons with no units.
    local empty = brain:MakePlatoon('empty', 'none')
    local listed = function(x) for _, q in brain:GetPlatoonsList() do if q == x then return true end end return false end
    check(not listed(empty), 'an empty platoon is not listed')
    brain:AssignUnitsToPlatoon(empty, {b}, 'Attack', 'None')
    check(listed(empty), 'listed once it holds a unit')
    brain:AssignUnitsToPlatoon('ArmyPool', {b}, 'Unassigned', 'None')

    -- Squad names: moho's enum, any case; others raise.
    local p = brain:MakePlatoon('q', 'none')
    brain:AssignUnitsToPlatoon(p, {a}, 'attack', 'None')
    check(table.getn(p:GetSquadUnits('Attack') or {}) == 1, 'a lower-case squad name is the enum value')
    check(p:GetSquadPosition('ATTACK') ~= nil, 'GetSquadPosition ignores case')
    local ok, err = pcall(function() return p:GetSquadUnits('Bogus') end)
    check(not ok and string.find(tostring(err), 'Invalid enum value Bogus'), 'GetSquadUnits with a bad squad raises: ' .. tostring(err))
    ok = pcall(function() brain:AssignUnitsToPlatoon(p, {b}, 'Bogus', 'None') end)
    check(not ok, 'AssignUnitsToPlatoon with a bad squad raises')

    -- Calls on a disbanded platoon raise, except GetPlatoonUnits ({}).
    local d = brain:MakePlatoon('d', 'none')
    brain:AssignUnitsToPlatoon(d, {b}, 'Attack', 'None')
    brain:DisbandPlatoon(d)
    check(table.getn(d:GetPlatoonUnits()) == 0, 'GetPlatoonUnits of a disbanded platoon is {}')
    for name, f in {
        GetSquadUnits = function() return d:GetSquadUnits('Attack') end,
        GetPlatoonPosition = function() return d:GetPlatoonPosition() end,
        MoveToLocation = function() return d:MoveToLocation({430, 0, 610}, false) end,
        AssignUnitsToPlatoon = function() brain:AssignUnitsToPlatoon(d, {b}, 'Attack', 'None') end,
        DisbandPlatoon = function() brain:DisbandPlatoon(d) end,
    } do
        local ok2, e = pcall(f)
        check(not ok2 and string.find(tostring(e), 'Game object has been destroyed'), name .. ' on a disbanded platoon raises: ' .. tostring(e))
    end
    local inPool = false
    for _, u in brain:GetPlatoonUniquelyNamed('ArmyPool'):GetPlatoonUnits() do if u == b then inPool = true end end
    check(inPool, 'the unit stays in the army pool')

    -- The default personality's values.
    local pers = brain:GetPersonality()
    check(pers:GetPersonalityName() == 'AverageJoe' and pers:GetChatPersonality() == 'None', 'personality name and chat')
    check(pers:GetDifficulty() == 0.5 and pers:GetArmySize() == 75 and pers:GetPlatoonSize() == 1, 'personality difficulty and sizes')

    ScenarioResult.done = true
end)
