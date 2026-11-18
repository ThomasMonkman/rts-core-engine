-- Scenario test (tests/scenarios.rs): land units under water drown (real game,
-- dev/probes probe_re7_kill2 / probe_re7_drown). A T1 tank made on the seabed
-- loses 1% of its max health a beat with no OnDamage, and at 0, 99 beats after
-- it was made, hears OnKilled with no instigator. Wagner and the ACU
-- (amphibious) and the Aurora (hover) are untouched; so is a tank on land.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    check(GetTerrainHeight(300, 300) < GetSurfaceHeight(300, 300) - 5, 'the spot is deep water')
    local made = GetGameTick()
    local tank = make('uel0201', 300, 300)
    local spared = { wagner = make('url0203', 310, 300), acu = make('uel0001', 320, 300), hover = make('ual0201', 330, 300), dry = make('uel0201', 420, 600) }
    local damaged, killed = 0, nil
    local dmg, kil = tank.OnDamage, tank.OnKilled
    tank.OnDamage = function(self, ...) damaged = damaged + 1 return dmg(self, ...) end
    tank.OnKilled = function(self, instigator, type, overkill)
        killed = { tick = GetGameTick(), instigator = instigator, type = type, overkill = overkill, hp = self:GetHealth() }
        return kil(self, instigator, type, overkill)
    end
    WaitTicks(1)
    check(tank.Layer == 'Seabed', 'the tank is on the seabed: ' .. tostring(tank.Layer))
    check(tank:GetHealth() == 297, 'one beat in, the tank lost 1%: ' .. tank:GetHealth())
    WaitTicks(9)
    local beats = GetGameTick() - made
    check(tank:GetHealth() == 300 - 3 * beats, beats .. ' beats in, ' .. beats .. '%: ' .. tank:GetHealth())
    while not killed and GetGameTick() < made + 120 do WaitTicks(1) end
    check(killed ~= nil, 'the tank drowned')
    if killed then
        check(killed.tick == made + 99, 'killed 99 beats after it was made: ' .. (killed.tick - made))
        check(killed.instigator == nil, 'with no instigator')
        check(killed.type == '' and killed.overkill == 0, 'type "" and overkill 0: ' .. tostring(killed.type) .. ' ' .. tostring(killed.overkill))
        check(killed.hp == 0, 'at 0 health: ' .. tostring(killed.hp))
    end
    check(damaged == 0, 'no OnDamage: ' .. damaged)
    for name, u in spared do
        check(not u.Dead and u:GetHealth() == u:GetMaxHealth(), name .. ' untouched: ' .. u:GetHealth() .. ' ' .. tostring(u.Layer))
    end
    ScenarioResult.done = true
end)
