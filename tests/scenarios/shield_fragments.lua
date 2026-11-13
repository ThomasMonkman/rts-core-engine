-- Scenario test (tests/scenarios.rs): Lobo shells that burst on a shield
-- bubble don't hurt what's under it. The Lobo's shell splits into five
-- fragments on any impact, the bubble included (FAF's
-- TIFFragmentationSensorShell01.OnImpact); the fragments start on the
-- bubble's surface. Three Lobos shell a UEF mobile shield (bubble radius
-- 8.5) with a tank beside it under the bubble: while the bubble is up,
-- neither loses health.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, army, x, z, h) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, h or 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    brain:__engine_SetStorage('ENERGY', 500000) brain:GiveResource('ENERGY', 500000)
    local carrier = make('uel0307', 'ARMY_1', 400.5, 585.5)
    local tank = make('uel0201', 'ARMY_1', 403.5, 585.5)
    carrier:SetFireState(1) tank:SetFireState(1)
    WaitTicks(60)
    local sh = carrier.MyShield
    check(sh ~= nil and sh:IsUp(), 'the mobile shield is up')
    if not sh then ScenarioResult.done = true return end
    local s0 = sh:GetHealth()
    -- Within their vision (18) of the tank, outside the bubble.
    local lobos = {
        make('uel0103', 'ARMY_2', 401.5, 601.5, 3.14159),
        make('uel0103', 'ARMY_2', 404.5, 601.5, 3.14159),
        make('uel0103', 'ARMY_2', 407.5, 601.5, 3.14159),
    }
    IssueAttack(lobos, tank)
    local up, hurtTank, hurtCarrier, ticks = true, 0, 0, 0
    for _ = 1, 240 do
        WaitTicks(1)
        if sh:GetHealth() <= 0 or sh.DepletedByDamage or not sh:IsUp() then up = false break end
        ticks = ticks + 1
        hurtTank = tank:GetMaxHealth() - tank:GetHealth()
        hurtCarrier = carrier:GetMaxHealth() - carrier:GetHealth()
    end
    note(string.format('%d ticks with the bubble up: shield %.0f -> %.0f, tank lost %.0f, carrier lost %.0f',
        ticks, s0, sh:GetHealth(), hurtTank, hurtCarrier))
    check(sh:GetHealth() < s0 or not up, 'the shells hit the shield')
    check(hurtTank == 0, 'the tank under the bubble took nothing while it was up')
    check(hurtCarrier == 0, 'the shield carrier took nothing while its bubble was up')
    ScenarioResult.done = true
end)
