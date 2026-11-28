-- Scenario test (tests/scenarios.rs): the engine asks CheckCanBeKilled before
-- a kill (probe_wpn_misc2 `kill cantkill*`). With SetCanBeKilled(false), Kill()
-- does nothing and lethal damage leaves the unit alive at 0 health; after
-- SetCanBeKilled(true) the next lethal hit kills it.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local inst = CreateUnitHPR('uel0201', 'ARMY_2', 410, GetTerrainHeight(410, 600), 600, 0, 0, 0)
    local u = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local killed = 0
    local orig = u.OnKilled
    u.OnKilled = function(self, a, b, c)
        killed = killed + 1
        return orig(self, a, b, c)
    end
    WaitTicks(1)
    u:SetCanBeKilled(false)
    u:Kill(inst, 'Normal', 0)
    check(killed == 0, 'Kill() on an unkillable unit runs no OnKilled: ' .. killed)
    check(u:GetHealth() == u:GetMaxHealth(), 'Kill() on an unkillable unit leaves health: ' .. u:GetHealth())
    Damage(inst, u:GetPosition(), u, 5000, 'Normal')
    check(killed == 0, 'lethal damage on an unkillable unit runs no OnKilled: ' .. killed)
    check(u:GetHealth() == 0, 'lethal damage leaves it at 0 health: ' .. u:GetHealth())
    WaitTicks(2)
    check(not u:BeenDestroyed(), 'the unkillable unit stays')
    u:SetCanBeKilled(true)
    Damage(inst, u:GetPosition(), u, 1, 'Normal')
    check(killed == 1, 'after SetCanBeKilled(true) the next hit kills: ' .. killed)
    check(u.Dead == true, 'FAF marked it dead')
    ScenarioResult.done = true
end)
