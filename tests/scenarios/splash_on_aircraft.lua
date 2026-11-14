-- Scenario test (tests/scenarios.rs): a small blast on a flying unit.
-- `OnImpact` runs the tick after the collision, as moho's (FAF probe
-- probe_3841b): the unit hit has moved a step, and FAF's
-- `Projectile.OnImpact` moves the blast one step of its velocity on to
-- meet it. Run in the collision's tick, a 0.3 blast (FAF 3841's cruiser
-- SAMs) missed a scout on most direct hits.
-- A UEF SAM launcher, its 1.5 blast cut to 0.3, at a scout patrolling past:
-- every impact on the scout damages it.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    local ok, err = pcall(function()
        WaitTicks(2)
        SetAlliance('ARMY_1', 'ARMY_2', 'Enemy')
        SetAlliance('ARMY_2', 'ARMY_1', 'Enemy')
        -- A far-off unit keeps ARMY_2 in the game.
        CreateUnitHPR('ueb3101', 'ARMY_2', 600, GetTerrainHeight(600, 300), 300, 0, 0, 0)
        local sam = CreateUnitHPR('ueb2304', 'ARMY_1', 400, GetTerrainHeight(400, 900), 900, 0, 0, 0)
        local scout = CreateUnitHPR('uea0101', 'ARMY_2', 350, GetTerrainHeight(350, 880) + 18, 880, 0, 0, 0)
        scout:SetMaxHealth(100000)
        scout:SetHealth(nil, 100000)
        IssuePatrol({ scout }, { 450, 0, 880 })
        IssuePatrol({ scout }, { 350, 0, 880 })
        local w = sam:GetWeaponByLabel('MissileRack01')
        w:AddDamageRadiusMod(0.3 - w:GetBlueprint().DamageRadius)
        local hits, missed, radius = 0, 0, nil
        local create = w.CreateProjectileAtMuzzle
        w.CreateProjectileAtMuzzle = function(self, muzzle)
            local p = create(self, muzzle)
            if p then
                radius = p.DamageData.DamageRadius
                local impact = p.OnImpact
                p.OnImpact = function(self, kind, other)
                    local before = other == scout and scout:GetHealth()
                    local r = impact(self, kind, other)
                    if before then
                        hits = hits + 1
                        if scout:GetHealth() >= before then missed = missed + 1 end
                    end
                    return r
                end
            end
            return p
        end
        WaitTicks(150)
        check(radius and math.abs(radius - 0.3) < 0.001, 'the missiles carry a 0.3 blast: ' .. tostring(radius))
        check(hits >= 4, 'the SAM hit the scout: ' .. hits)
        check(missed == 0, 'every hit damaged it: ' .. missed .. ' of ' .. hits .. ' did not')
    end)
    if not ok then check(false, 'error: ' .. tostring(err)) end
    ScenarioResult.done = true
end)
