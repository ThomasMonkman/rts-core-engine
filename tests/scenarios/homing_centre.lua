-- Scenario test (tests/scenarios.rs): a homing projectile aims at its target's
-- centre (the middle of its collision box), not its origin (probe_wpn_proj
-- `trk.*`: a wall's GetCurrentTargetPosition is 0.25 above its origin, and the
-- hits land there).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG('SCN FAIL ', msg) end end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    local l = CreateUnitHPR('ueb5101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local wall = CreateUnitHPR('ueb5101', 'ARMY_2', x + 15, GetTerrainHeight(x + 15, z), z, 0, 0, 0)
    wall:SetMaxHealth(1e6) wall:SetHealth(nil, 1e6)
    local p = l:CreateProjectile(LOS, 0, 6, 0, 1, 0, 0)
    p:SetNewTarget(wall)
    p:TrackTarget(true)
    p:SetTurnRate(360)
    local tp = p:GetCurrentTargetPosition()
    local wp = wall:GetPosition()
    check(math.abs(tp[2] - wp[2] - 0.25) < 0.01, 'the target point is the wall\'s centre, 0.25 up: ' .. (tp[2] - wp[2]))
    local hit
    p.OnImpact = function(self, kind)
        local pos = self:GetPosition()
        hit = { kind = kind, y = pos[2] - wp[2] }
        self:Destroy()
    end
    WaitTicks(30)
    check(hit and hit.kind == 'Unit', 'it hit the wall: ' .. tostring(hit and hit.kind))
    check(hit and hit.y > 0.15, 'above the wall\'s feet: ' .. tostring(hit and hit.y))
    ScenarioResult.done = true
end)
