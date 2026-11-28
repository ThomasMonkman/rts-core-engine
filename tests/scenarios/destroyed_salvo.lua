-- Scenario test (tests/scenarios.rs): a unit destroyed by a script mid-salvo
-- still exists until the end of the beat, so its weapon's CreateProjectile
-- still makes a projectile that beat (FAF's salvo thread indexes it at once:
-- weapon.lua:551 raised on a nil in ours); the weapon's OnDestroy at the end
-- of the beat is what stops the salvo.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG('SCN FAIL ', msg) end end
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    local u = CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    WaitTicks(2)
    local w = u:GetWeapon(1)
    u:Destroy()
    local p = w:CreateProjectile()
    check(p ~= nil, 'a unit destroyed this beat still makes its projectile')
    if p then p:Destroy() end
    ScenarioResult.done = true
end)
