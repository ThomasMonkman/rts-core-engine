-- Scenario test (tests/scenarios.rs): own units in a shot's line are asked
-- OnCollisionCheck like enemies (FAF probe probe_re3_wpn_d): FAF's stock
-- answer lets the shot through, an override answering true stops it.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local lanes = {}
    for i, answer in { 'stock', 'true' } do
        local x, z = 402.5 - (i - 1) * 20, 600.5
        local tank = make('uel0201', 'ARMY_1', x, z)
        local enemy = make('ueb5101', 'ARMY_2', x, z + 15)
        local blocker = make('ueb1101', 'ARMY_1', x, z + 7)
        for _, u in { enemy, blocker } do u:SetMaxHealth(100000) u:SetHealth(nil, 100000) end
        local r = { tank = tank, enemy = enemy, blocker = blocker, asked = 0, answer = answer }
        local stock = blocker.OnCollisionCheck
        blocker.OnCollisionCheck = function(self, other, weapon)
            r.asked = r.asked + 1
            if answer == 'true' then return true end
            return stock(self, other, weapon)
        end
        table.insert(lanes, r)
    end
    for _, r in lanes do IssueAttack({ r.tank }, r.enemy) end
    WaitTicks(80)
    for _, r in lanes do
        local blockerHit = r.blocker:GetHealth() < r.blocker:GetMaxHealth()
        local enemyHit = r.enemy:GetHealth() < r.enemy:GetMaxHealth()
        check(r.asked > 0, r.answer .. ': the own blocker is asked (' .. r.asked .. ')')
        if r.answer == 'stock' then
            check(not blockerHit and enemyHit, 'stock: shots pass the own blocker and hit the enemy')
        else
            check(blockerHit and not enemyHit, 'override true: shots stop at the own blocker')
        end
    end
    ScenarioResult.done = true
end)
