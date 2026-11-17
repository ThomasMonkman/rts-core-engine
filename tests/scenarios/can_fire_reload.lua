-- Scenario test (tests/scenarios.rs): Weapon:CanFire() ignores the reload
-- clock: once the weapon is aimed on its target it stays true through every
-- reload (probe_wpn_ctrl `fireclock`: true each tick while GetFireClockPct runs
-- 0.0..0.9). FAF's counted-projectile weapons read it mid-reload.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG('SCN FAIL ', msg) end end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local tank = make('uel0201', 'ARMY_1', 400.5, 600.5)
    local wall = make('ueb5101', 'ARMY_2', 400.5, 612.5)
    wall:SetMaxHealth(100000) wall:SetHealth(nil, 100000)
    IssueAttack({tank}, wall)
    local w = tank:GetWeapon(1)
    local fired = false
    local reloading, can = 0, 0
    for t = 1, 60 do
        WaitTicks(1)
        local pct = w:GetFireClockPct()
        if wall:GetHealth() < 100000 then fired = true end
        if fired and pct < 1 then
            reloading = reloading + 1
            if w:CanFire() then can = can + 1 end
        end
    end
    check(fired, 'the tank fired')
    check(reloading > 5, 'it was seen reloading: ' .. reloading)
    check(can == reloading, string.format('CanFire through the reload: %d of %d ticks', can, reloading))
    ScenarioResult.done = true
end)
