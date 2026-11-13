-- Scenario test (tests/scenarios.rs): the Absolver's anti-shield shots.
-- FAF's Projectile.DoDamage deals a weapon's DamageToShields first, as
-- separate 'FAF_AntiShield' damage that only shields take (Unit.OnDamage
-- ignores it), then its normal damage. The Aeon Absolver's gun does 5 and
-- 1295 to shields: against a UEF T2 shield (9000) each shot should take
-- about 1300, and the tank under the bubble nothing.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, army, x, z, h) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, h or 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    brain:__engine_SetStorage('ENERGY', 500000) brain:GiveResource('ENERGY', 500000)
    local gen = make('ueb4202', 'ARMY_1', 400.5, 580.5)
    local tank = make('uel0201', 'ARMY_1', 400.5, 585.5)
    tank:SetFireState(1)
    WaitTicks(60)
    local sh = gen.MyShield
    check(sh ~= nil, 'the generator made a shield')
    if not sh then ScenarioResult.done = true return end
    -- Within its vision (20) of the generator, 5 outside the bubble.
    local absolver = make('dal0310', 'ARMY_2', 400.5, 598.5, 3.14159)
    IssueAttack({ absolver }, gen)
    -- Each drop in shield health while it's up (one per shot).
    local last, drops, hurt = sh:GetHealth(), {}, false
    for _ = 1, 150 do
        WaitTicks(1)
        if sh:GetHealth() <= 0 or sh.DepletedByDamage then break end
        local h = sh:GetHealth()
        if h < last - 1 then table.insert(drops, last - h) end
        last = h
        hurt = hurt or tank:GetHealth() < tank:GetMaxHealth() or gen:GetHealth() < gen:GetMaxHealth()
    end
    local shown = {}
    for i, d in drops do shown[i] = string.format('%.0f', d) end
    note('shield drops per shot: ' .. table.concat(shown, ' '))
    check(table.getn(drops) >= 2, 'the Absolver hit the shield')
    for _, d in drops do
        check(d >= 1250 and d <= 1350, string.format('a shot took %.0f from the shield, not about 1300', d))
    end
    check(not hurt, 'nothing under the bubble was hurt')
    ScenarioResult.done = true
end)
