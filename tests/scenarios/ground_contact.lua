-- Scenario test (tests/scenarios.rs): a projectile hitting the ground gets
-- OnImpact('Terrain') at the contact point on its path, not at the end of the
-- step past the surface (probe_wpn_proj `imp.terrain ... pos 6.02,-0.02,6.02`:
-- FAF centres the burst's DamageArea on GetPosition()).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG("SCN FAIL ", msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) LOG("SCN ", msg) end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    local l = CreateUnitHPR('ueb5101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local hit
    local k = 1 / math.sqrt(3)
    local p = l:CreateProjectile(LOS, 3, 3, 3, k, -k, k)
    p.OnImpact = function(self, kind)
        local pos = self:GetPosition()
        hit = { kind = kind, above = pos[2] - GetTerrainHeight(pos[1], pos[3]), d = pos[1] - x }
        self:Destroy()
    end
    WaitTicks(10)
    check(hit and hit.kind == 'Terrain', 'the shot hit the ground: ' .. tostring(hit and hit.kind))
    check(hit and math.abs(hit.above) < 0.05, 'at the surface: ' .. tostring(hit and hit.above))
    check(hit and math.abs(hit.d - 6.0) < 0.2, 'where the path meets it, ~6 out: ' .. tostring(hit and hit.d))
    if hit then note(string.format('%s %.3f above, %.2f out', hit.kind, hit.above, hit.d)) end
    ScenarioResult.done = true
end)
