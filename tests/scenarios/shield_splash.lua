-- Scenario test (tests/scenarios.rs): area damage wears shield bubbles down.
-- FAF's beams and projectiles with a DamageRadius damage through DamageArea,
-- and moho's area damage hands each shield bubble it touches from outside
-- the share it absorbs (OnGetDamageAbsorption), the units under it the rest.
-- A Monkeylord's main laser (DamageRadius 0.5) and an Othuum's Tau cannon
-- (a projectile, DamageRadius 1.2), their other weapons off, both outside
-- the bubbles (radius 13), each attack a UEF T2 shield generator covering a
-- tank.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, army, x, z, h) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, h or 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    brain:__engine_SetStorage('ENERGY', 500000) brain:GiveResource('ENERGY', 500000)
    local genA = make('ueb4202', 'ARMY_1', 385.5, 570.5)
    local tankA = make('uel0201', 'ARMY_1', 385.5, 565.5)
    local genB = make('ueb4202', 'ARMY_1', 420.5, 632.5)
    local tankB = make('uel0201', 'ARMY_1', 420.5, 637.5)
    tankA:SetFireState(1) tankB:SetFireState(1)
    WaitTicks(60)
    local shA, shB = genA.MyShield, genB.MyShield
    check(shA ~= nil and shB ~= nil, 'the generators made shields')
    if not (shA and shB) then ScenarioResult.done = true return end
    local ml = make('url0402', 'ARMY_2', 385.5, 597.5, 3.14159)
    for _, label in { 'RightLaserTurret', 'LeftLaserTurret', 'RightAntiAirMissile', 'LeftAntiAirMissile' } do
        if ml:GetWeaponByLabel(label) then ml:SetWeaponEnabledByLabel(label, false) end
    end
    local tau = make('xsl0303', 'ARMY_2', 420.5, 612.5)
    for _, label in { 'RightTurret', 'LeftTurret', 'Torpedo01' } do
        if tau:GetWeaponByLabel(label) then tau:SetWeaponEnabledByLabel(label, false) end
    end
    IssueAttack({ ml }, genA)
    IssueAttack({ tau }, genB)
    -- Each tick while a bubble is up, the tank and generator under it take
    -- nothing. (The laser does 400 a tick: it breaks the bubble in a few
    -- seconds, and the tank under it dies after.)
    local function watch(name, gen, sh, tank)
        local s0, up, hurt = sh:GetHealth(), true, false
        return function()
            if not up then return end
            if sh:GetHealth() <= 0 or sh.DepletedByDamage then up = false return end
            hurt = hurt or tank:GetHealth() < tank:GetMaxHealth() or gen:GetHealth() < gen:GetMaxHealth()
        end, function()
            local s1 = sh:GetHealth()
            note(string.format('%s: shield %.0f -> %.0f%s', name, s0, s1, up and '' or ' (broken)'))
            check(s1 < s0 or not up, name .. ' damaged the shield')
            check(not hurt, name .. ': nothing under the bubble was hurt while it was up')
        end
    end
    local tickA, endA = watch('beam with a radius', genA, shA, tankA)
    local tickB, endB = watch('projectile with a radius', genB, shB, tankB)
    for _ = 1, 150 do
        WaitTicks(1)
        tickA() tickB()
    end
    endA() endB()
    ScenarioResult.done = true
end)
