-- Scenario test (tests/scenarios.rs): a projectile crossing the water
-- surface gets OnEnterWater going down and OnExitWater coming up (over open
-- water), before any impact.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    -- Find open water at least 6 deep.
    local wx, wz
    for x = 16, 1008, 16 do
        for z = 16, 1008, 16 do
            if GetSurfaceHeight(x, z) - GetTerrainHeight(x, z) > 6 then wx, wz = x, z break end
        end
        if wx then break end
    end
    if not wx then
        table.insert(ScenarioResult.notes, 'no open water on this map: skipped')
        ScenarioResult.done = true
        return
    end
    local surface = GetSurfaceHeight(wx, wz)
    -- (A ship: a land unit made here stands on the seabed the next tick.)
    local launcher = CreateUnitHPR('ues0103', 'ARMY_1', wx, surface, wz, 0, 0, 0)
    WaitTicks(1)
    local log = {}
    local function watch(p, name)
        p.OnEnterWater = function(self) table.insert(log, name .. ':enter') end
        p.OnExitWater = function(self) table.insert(log, name .. ':exit') end
        local impact = p.OnImpact
        p.OnImpact = function(self, kind, target) table.insert(log, name .. ':impact:' .. kind); return impact(self, kind, target) end
    end
    local down = launcher:CreateProjectile('/projectiles/TDFGauss01/TDFGauss01_proj.bp', 0, 4, 0, 0, -1, 0)
    watch(down, 'down')
    down:SetVelocity(0, -2, 0)
    -- (Gauss shells fly under gravity, moho's default: hold them level.)
    down:SetBallisticAcceleration(0)
    -- (Made under water: a blueprint that doesn't die on water, as a
    -- `DestroyOnWater` one made there is destroyed at once, probe_re3_wpn_b.)
    local up = launcher:CreateProjectile('/projectiles/AANTorpedoCluster01/AANTorpedoCluster01_proj.bp', 0, -3, 0, 0, 1, 0)
    watch(up, 'up')
    up:SetVelocity(0, 2, 0)
    up:SetBallisticAcceleration(0)
    WaitTicks(40)
    local got = table.concat(log, ' ')
    check(string.find(got, 'down:enter', 1, true), 'falling shot entered the water: ' .. got)
    local e, i = string.find(got, 'down:enter', 1, true), string.find(got, 'down:impact', 1, true)
    check(not i or (e and e < i), 'OnEnterWater before its impact: ' .. got)
    check(string.find(got, 'up:exit', 1, true), 'rising shot left the water: ' .. got)
    ScenarioResult.done = true
end)
