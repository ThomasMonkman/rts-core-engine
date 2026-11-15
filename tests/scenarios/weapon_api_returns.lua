-- Scenario test (tests/scenarios.rs): weapon methods as the real game has
-- them (probe_wpn_ctrl `dmg`, `manual`): ChangeDamageRadius and
-- ChangeDamageType exist and succeed; FireWeapon fires and returns nothing.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG('SCN FAIL ', msg) end end
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    local u = CreateUnitHPR('ueb2101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local w = u:GetWeapon(1)
    local ok1 = pcall(moho.weapon_methods.ChangeDamageRadius, w, 4)
    local ok2 = pcall(moho.weapon_methods.ChangeDamageType, w, 'Fire')
    check(ok1, 'ChangeDamageRadius exists and succeeds')
    check(ok2, 'ChangeDamageType exists and succeeds')
    local fired = 0
    local onFire = w.OnFire
    w.OnFire = function(self) fired = fired + 1 return onFire(self) end
    local n = select('#', w:FireWeapon())
    check(n == 0, 'FireWeapon returns nothing: ' .. n .. ' values')
    check(fired == 1, 'FireWeapon fired: ' .. fired)
    ScenarioResult.done = true
end)
