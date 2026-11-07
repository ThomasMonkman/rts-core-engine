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
-- Damage goes through armordefinition.lua: structures take 3.2% of Deathnuke.
ForkThread(function()
    WaitTicks(2)
    local pgen = CreateUnitHPR('ueb1101', 'ARMY_1', 400, ground(400, 600), 600, 0, 0, 0)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 420, ground(420, 600), 600, 0, 0, 0)
    check(pgen:GetArmorMult('Overcharge') == 0.25, 'structure Overcharge 0.25')
    check(pgen:GetArmorMult('Normal') == 1, 'structure Normal 1')
    check(tank:GetArmorMult('Normal') == 1, 'tank Normal 1')
    local h0 = pgen:GetHealth()
    Damage(nil, pgen:GetPosition(), pgen, 100, 'Deathnuke')
    WaitTicks(1)
    local lost = h0 - pgen:GetHealth()
    check(math.abs(lost - 3.2) < 0.01, '100 Deathnuke took 3.2, took ' .. lost)
    -- Armour names match without regard to case (moho's _stricmp):
    -- XAC2101's 'STRUCTURE' is the Structure armour.
    local civ = CreateUnitHPR('xac2101', 'ARMY_2', 440, ground(440, 600), 600, 0, 0, 0)
    check(civ:GetArmorMult('Overcharge') == 0.25, "'STRUCTURE' armour takes Structure's Overcharge 0.25")
    -- OnDamage only for damage above 0 after armour (DealDamage): a
    -- Monkeylord's Experimental armour takes ExperimentalFootfall x0, and
    -- negative damage doesn't heal.
    local ml = CreateUnitHPR('url0402', 'ARMY_1', 420, ground(420, 620), 620, 0, 0, 0)
    local calls = 0
    local onDamage = ml.OnDamage
    ml.OnDamage = function(self, ...) calls = calls + 1 return onDamage(self, ...) end
    Damage(nil, ml:GetPosition(), ml, 100, 'ExperimentalFootfall')
    check(calls == 0, 'no OnDamage for damage armour takes to 0, got ' .. calls)
    Damage(nil, ml:GetPosition(), ml, 100, 'Normal')
    check(calls == 1, 'OnDamage for 100 Normal, got ' .. calls)
    local h1 = tank:GetHealth()
    Damage(nil, tank:GetPosition(), tank, 50, 'Normal')
    Damage(nil, tank:GetPosition(), tank, -50, 'Normal')
    WaitTicks(1)
    check(math.abs(tank:GetHealth() - (h1 - 50)) < 0.01, 'negative damage does not heal: ' .. tank:GetHealth())
    -- 0 damage raises (SIM_Damage's callers).
    local ok, err = pcall(Damage, nil, tank:GetPosition(), tank, 0, 'Normal')
    check(not ok and string.find(tostring(err), '0 damage specified', 1, true), '0 damage raises: ' .. tostring(err))
    done()
end)
