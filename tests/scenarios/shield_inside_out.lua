-- Scenario test (tests/scenarios.rs): a shot launched inside an enemy bubble
-- hits it from the inside on its way out (probe_wpn_shield `geo.insideout`:
-- `type Shield`, 12.84 from the generator), while one launched inside its
-- own army's bubble gets out.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    brain:__engine_SetStorage('ENERGY', 500000) brain:GiveResource('ENERGY', 500000)
    local gen = make('ueb4202', 'ARMY_1', 400.5, 600.5)
    local enemy = make('ueb5101', 'ARMY_2', 394.5, 600.5)
    local own = make('ueb5101', 'ARMY_1', 400.5, 606.5)
    WaitTicks(60)
    check(gen.MyShield ~= nil and gen.MyShield:IsUp(), 'the shield is up')
    local results = {}
    local function shoot(tag, launcher, dx, dz)
        local lp = launcher:GetPosition()
        local p = launcher:CreateProjectile(LOS, dx * 1.5, 0.4, dz * 1.5, dx, 0, dz)
        p.OnImpact = function(self, kind, target)
            local pos = self:GetPosition()
            local gp = gen:GetPosition()
            local x, z = pos[1] - gp[1], pos[3] - gp[3]
            results[tag] = { kind = kind, dist = math.sqrt(x * x + z * z) }
            self:Destroy()
        end
    end
    -- the enemy shoots away from the generator (-x), its own army's wall
    -- away from it (+z)
    shoot('enemy', enemy, -1, 0)
    shoot('own', own, 0, 1)
    WaitTicks(40)
    local e, o = results.enemy, results.own
    check(e and e.kind == 'Shield', 'the enemy shot hit the bubble on its way out: ' .. tostring(e and e.kind))
    check(e and e.dist > 12 and e.dist < 13.5, 'at the bubble: ' .. tostring(e and e.dist))
    check(not o or o.kind ~= 'Shield', 'the own shot got out: ' .. tostring(o and o.kind))
    if e then note(string.format('enemy %s at %.2f', e.kind, e.dist)) end
    ScenarioResult.done = true
end)
