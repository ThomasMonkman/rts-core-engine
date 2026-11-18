-- Scenario test (tests/scenarios.rs): which platoon calls raise or count
-- what, as measured in the real game (dev/probes probe_re5_platoon,
-- probe_re6_platoon).
-- Ended (its unit moved out): GetBrain and CanAttackTarget raise; the threat
-- and count calls give 0. Only unit dead but not yet destroyed (a bomber
-- falling): threat, counts and CanAttackTarget leave it out, GetPlatoonUnits
-- still lists it. Made empty and never filled: an unnamed platoon ends the
-- beat after, a named one lives on. GetPlatoonUniqueName is '' when unnamed,
-- UniquelyNamePlatoon returns the name, Patrol returns nil.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    local function unit(bp, x, army)
        return CreateUnitHPR(bp, army or 'ARMY_1', x, GetTerrainHeight(x, 600), 600, 0, 0, 0)
    end
    local enemy = unit('uel0201', 700, 'ARMY_2')
    local spot = {400, GetTerrainHeight(400, 600), 600}
    local raises = function(f) return not pcall(f) end

    -- Made empty: unnamed ends the beat after, named lives on.
    local made = GetGameTick()
    local unnamed, named = brain:MakePlatoon('unnamed', 'none'), brain:MakePlatoon('named', 'none')
    local unnamedEnded
    unnamed:AddDestroyCallback(function() unnamedEnded = GetGameTick() end)
    check(named:UniquelyNamePlatoon('ProbeNamed') == 'ProbeNamed', 'UniquelyNamePlatoon returns the name')
    check(unnamed:GetPlatoonUniqueName() == '', "an unnamed platoon's unique name is ''")
    check(brain:PlatoonExists(unnamed), 'a platoon made empty exists in the beat it was made')

    -- Ended, and falling.
    local ended = brain:MakePlatoon('ended', 'none')
    local falling = brain:MakePlatoon('falling', 'none')
    local tank = unit('uel0201', 400)
    local bomber = unit('uea0103', 420)
    brain:AssignUnitsToPlatoon(ended, {tank}, 'Attack', 'None')
    brain:AssignUnitsToPlatoon(falling, {bomber}, 'Attack', 'None')
    IssueMove({bomber}, {900, GetTerrainHeight(900, 900), 900})
    WaitTicks(1)
    check(unnamedEnded == made + 1, 'an unnamed platoon made empty ends the beat after (OnDestroy at ' .. tostring(unnamedEnded) .. ', made ' .. made .. ')')
    check(not brain:PlatoonExists(unnamed), 'and is gone then')
    check(brain:PlatoonExists(named), 'a named one lives on')
    check(falling:Patrol(spot) == nil, 'Patrol returns nil')
    check(falling:CanAttackTarget('Attack', enemy), 'the bomber can attack the tank while alive')
    WaitTicks(40)
    brain:AssignUnitsToPlatoon(brain:MakePlatoon('spare', 'none'), {tank}, 'Attack', 'None')
    bomber:Kill()
    WaitTicks(3)
    check(bomber.Dead and not bomber:BeenDestroyed(), 'the bomber is falling (dead, not destroyed)')
    check(not brain:PlatoonExists(ended), 'the platoon its tank left has ended')

    check(raises(function() return ended:GetBrain() end), 'GetBrain on an ended platoon raises')
    check(raises(function() return ended:CanAttackTarget('Attack', enemy) end), 'CanAttackTarget on an ended platoon raises')
    check(ended:CalculatePlatoonThreat('Surface', categories.ALLUNITS) == 0, 'CalculatePlatoonThreat on an ended platoon is 0')
    check(ended:CalculatePlatoonThreatAroundPosition('Surface', categories.ALLUNITS, spot, 45) == 0, 'CalculatePlatoonThreatAroundPosition on an ended platoon is 0')
    check(ended:PlatoonCategoryCount(categories.ALLUNITS) == 0, 'PlatoonCategoryCount on an ended platoon is 0')
    check(ended:PlatoonCategoryCountAroundPosition(categories.ALLUNITS, spot, 45) == 0, 'PlatoonCategoryCountAroundPosition on an ended platoon is 0')
    check(table.getn(ended:GetPlatoonUnits()) == 0, 'GetPlatoonUnits on an ended platoon is {}')

    local bpos = bomber:GetPosition()
    check(falling:GetBrain() == brain, 'the falling platoon has its brain')
    check(table.getn(falling:GetPlatoonUnits()) == 1, 'GetPlatoonUnits still lists the falling bomber')
    check(falling:CalculatePlatoonThreat('Surface', categories.ALLUNITS) == 0, 'CalculatePlatoonThreat leaves out the falling bomber')
    check(falling:CalculatePlatoonThreatAroundPosition('Surface', categories.ALLUNITS, bpos, 45) == 0, 'CalculatePlatoonThreatAroundPosition leaves it out')
    check(falling:PlatoonCategoryCount(categories.ALLUNITS) == 0, 'PlatoonCategoryCount leaves it out')
    check(falling:PlatoonCategoryCountAroundPosition(categories.ALLUNITS, bpos, 45) == 0, 'PlatoonCategoryCountAroundPosition leaves it out')
    check(not falling:CanAttackTarget('Attack', enemy), 'CanAttackTarget leaves it out')

    -- Filling the made-empty ones later.
    local u1, u2 = unit('uel0201', 440), unit('uel0201', 444)
    check(raises(function() brain:AssignUnitsToPlatoon(unnamed, {u1}, 'Support', 'None') end), 'assigning into the ended unnamed platoon raises')
    check(not raises(function() brain:AssignUnitsToPlatoon(named, {u2}, 'Support', 'None') end), 'assigning into the named one works')
    WaitTicks(2)
    check(brain:PlatoonExists(named) and table.getn(named:GetPlatoonUnits()) == 1, 'the named one holds its unit')
    ScenarioResult.done = true
end)
