-- Scenario test (tests/scenarios.rs): projectile lifetimes (probe_wpn_proj,
-- probe_wpn_misc2). A lifetime of L seconds ends with OnImpact('Air') after
-- ceil(10 L + 0.5) ticks (SetLifetime(1.0) at tick 11, 0.25 at 3, 0 at 1), and
-- a blueprint without Lifetime lives 15 s (151 ticks), not for ever.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG('SCN FAIL ', msg) end end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    local l = CreateUnitHPR('ueb5101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local ends = {}
    local t0 = GetGameTick()
    local function shoot(tag, life, dz)
        local p = l:CreateProjectile(LOS, 0, 2, dz, 0, 1, 0)
        p:SetVelocity(0, 2, 0)
        if life then p:SetLifetime(life) end
        p.OnImpact = function(self, kind)
            ends[tag] = string.format('%s %d', kind, GetGameTick() - t0)
            self:Destroy()
        end
    end
    shoot('1.0', 1.0, 0)
    shoot('0.25', 0.25, 4)
    shoot('0', 0, 8)
    shoot('default', nil, 12)
    WaitTicks(160)
    check(ends['1.0'] == 'Air 11', 'SetLifetime(1.0) ends at tick 11: ' .. tostring(ends['1.0']))
    check(ends['0.25'] == 'Air 3', 'SetLifetime(0.25) ends at tick 3: ' .. tostring(ends['0.25']))
    check(ends['0'] == 'Air 1', 'SetLifetime(0) ends at tick 1: ' .. tostring(ends['0']))
    check(ends.default == 'Air 151', 'no Lifetime: 15 s, tick 151: ' .. tostring(ends.default))
    ScenarioResult.done = true
end)
