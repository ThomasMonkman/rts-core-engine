-- Scenario test (tests/scenarios.rs): Entity:CreateProjectile destroys at
-- once, before OnCreate, a projectile whose blueprint tracks a target
-- (FAF probe probe_re3_wpn_b); one that doesn't track flies.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local x, z = 402.5, 600.5
    local wall = CreateUnitHPR('ueb5101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local created = {}
    for _, name in { 'TIFMissileCruise01', 'TDFGauss01' } do
        local cls = import('/projectiles/' .. name .. '/' .. name .. '_script.lua').TypeClass
        local oc = cls.OnCreate
        cls.OnCreate = function(self, inWater)
            created[name] = true
            return oc(self, inWater)
        end
    end
    local tracking = wall:CreateProjectile('/projectiles/TIFMissileCruise01/TIFMissileCruise01_proj.bp', 0, 8, 0, 1, 0, 0)
    local plain = wall:CreateProjectile('/projectiles/TDFGauss01/TDFGauss01_proj.bp', 0, 8, 0, 1, 0, 0)
    check(tracking and tracking:BeenDestroyed(), 'a TrackTarget projectile is destroyed at once')
    check(not created.TIFMissileCruise01, 'before its OnCreate')
    check(plain and not plain:BeenDestroyed() and created.TDFGauss01, 'one that does not track is made and flies')
    ScenarioResult.done = true
end)
