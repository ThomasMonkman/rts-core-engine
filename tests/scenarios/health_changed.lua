-- Scenario test (tests/scenarios.rs): OnHealthChanged(new, old) when health
-- crosses a quarter step (floored), as FAF's damage effects expect. Called
-- from inside the change (`Damage`, `SetHealth`), once per change: two hits
-- in one tick are two calls (FAF probe `probe_re_wpn` `hc`).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local u = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local calls = {}
    local orig = u.OnHealthChanged
    u.OnHealthChanged = function(self, new, old)
        table.insert(calls, string.format('%.2f<-%.2f', new, old))
        return orig(self, new, old)
    end
    WaitTicks(2)
    check(table.getn(calls) == 0, 'no call at full health: ' .. table.concat(calls, ' '))
    local max = u:GetMaxHealth()
    u:SetHealth(u, max * 0.7)
    WaitTicks(2)
    check(calls[1] == '0.50<-1.00', '70% health reports 0.50 from 1.00: ' .. table.concat(calls, ' '))
    check(u.DamageEffectsBag ~= nil, 'FAF set up its damage effects')
    u:SetHealth(u, max)
    WaitTicks(2)
    check(calls[2] == '1.00<-0.50', 'healed back: 1.00 from 0.50: ' .. table.concat(calls, ' '))
    -- Inside the call, one per hit.
    u:SetMaxHealth(1000) u:SetHealth(nil, 1000)
    local n = table.getn(calls)
    Damage(nil, u:GetPosition(), u, 300, 'Normal')
    check(table.getn(calls) == n + 1 and calls[n + 1] == '0.50<-1.00', 'first hit reported inside Damage: ' .. table.concat(calls, ' '))
    Damage(nil, u:GetPosition(), u, 300, 'Normal')
    check(table.getn(calls) == n + 2 and calls[n + 2] == '0.25<-0.50', 'second hit, same tick, its own call: ' .. table.concat(calls, ' '))
    WaitTicks(2)
    check(table.getn(calls) == n + 2, 'nothing more at the end of the tick: ' .. table.concat(calls, ' '))
    ScenarioResult.done = true
end)
