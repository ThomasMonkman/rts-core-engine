-- Scenario test (tests/scenarios.rs): a bubble hangs at the owner's origin +
-- SizeY/2 + ShieldVerticalOffset (FAF attaches it to bone -1). ueb4202
-- (SizeY 2.5, offset -3, radius 13): the bubble's top is 11.25 above the
-- generator, and a level shot at height 10 hits it (probe_wpn_shield `geo.*`:
-- `geo.top0 ... pos 0.00,11.24`, `geo.h10 ... type Shield`).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    brain:__engine_SetStorage('ENERGY', 500000) brain:GiveResource('ENERGY', 500000)
    local gen = make('ueb4202', 'ARMY_1', 400.5, 600.5)
    local enemy = make('ueb5101', 'ARMY_2', 375.5, 600.5)
    WaitTicks(60)
    check(gen.MyShield ~= nil and gen.MyShield:IsUp(), 'the shield is up')
    local gp = gen:GetPosition()
    local ep = enemy:GetPosition()
    local results = {}
    local function shoot(tag, ox, oy, oz, dx, dy, dz)
        local p = enemy:CreateProjectile(LOS, ox, oy, oz, dx, dy, dz)
        p.OnImpact = function(self, kind)
            local pos = self:GetPosition()
            results[tag] = { kind = kind, x = pos[1] - gp[1], y = pos[2] - gp[2] }
            self:Destroy()
        end
    end
    -- level at height 10 above the generator, toward it
    shoot('h10', 1.5, gp[2] + 10 - ep[2], 0, 1, 0, 0)
    -- straight down onto its top, from the generator's own spot
    shoot('top', gp[1] - ep[1], gp[2] + 20 - ep[2], 0, 0, -1, 0)
    WaitTicks(40)
    local h, t = results.h10, results.top
    check(h and h.kind == 'Shield', 'a shot at height 10 hits the bubble: ' .. tostring(h and h.kind))
    check(t and t.kind == 'Shield' and math.abs(t.y - 11.25) < 0.3, 'the top is 11.25 up: ' .. tostring(t and t.y))
    ScenarioResult.done = true
end)
