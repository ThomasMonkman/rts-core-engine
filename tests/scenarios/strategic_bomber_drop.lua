-- Scenario test (tests/scenarios.rs): ARMY_1 / ARMY_2 have no AI; checks go
-- into ScenarioResult, which the test reads once `done` is set.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function done() ScenarioResult.done = true end
local function ground(x, z) return GetTerrainHeight(x, z) end
local function rich(army)
    local b = ArmyBrains[army]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    return b
end
-- Bombers ordered onto a target close ahead bomb it on the first pass:
-- * a strategic bomber 38 from its target is inside its `BreakOffDistance`
--   (50), so its first combat state is the break-off; bombs drop in that
--   state too (the real game's lands at tick 43 of probe_wpn_ctrl);
-- * a UEF T1 bomber drops its whole salvo of 4 (its weapon script calls
--   `SetTargetGround` before each bomb after the first, which mustn't halt
--   the salvo).
ForkThread(function()
    WaitTicks(2)
    local function lane(bp, x)
        local z = 570
        local u = CreateUnitHPR(bp, 'ARMY_1', x, ground(x, z), z, 0, 0, 0)
        u:SetFireState(1)
        local t = CreateUnitHPR('ueb1201', 'ARMY_2', x, ground(x, z + 38), z + 38, 0, 0, 0)
        t:SetMaxHealth(1000000) t:SetHealth(nil, 1000000)
        -- A scout keeps the target in sight from the start (the bombers'
        -- own vision is 32); weapons only take a target their army sees.
        local scout = CreateUnitHPR('uel0101', 'ARMY_1', x + 9, ground(x + 9, z + 38), z + 38, 0, 0, 0)
        scout:SetFireState(1)
        local l = { u = u, t = t, damage = 0, bombs = 0 }
        check(t:GetCurrentLayer() == 'Land', bp .. "'s target is on land")
        t.OnDamage = function(self, instigator, amount, vector, damageType)
            if damageType == 'Normal' then
                l.damage = l.damage + amount
                l.hit = l.hit or GetGameTick()
            end
        end
        for i = 1, u:GetWeaponCount() do
            local w = u:GetWeapon(i)
            if w.Blueprint.NeedToComputeBombDrop then
                local create = w.CreateProjectile
                w.CreateProjectile = function(self, bone)
                    local p = create(self, bone)
                    if p then l.bombs = l.bombs + 1 end
                    return p
                end
            end
        end
        return l
    end
    local strat, t1 = lane('uea0304', 395), lane('uea0103', 425)
    WaitTicks(20)
    local t0 = GetGameTick()
    for _, l in { strat, t1 } do
        l.u:SetFireState(0)
        IssueAttack({ l.u }, l.t)
    end
    WaitTicks(70)
    note('strategic: bombs ' .. strat.bombs .. ' damage ' .. strat.damage .. ' at ' .. tostring(strat.hit and strat.hit - t0))
    note('t1: bombs ' .. t1.bombs .. ' damage ' .. t1.damage .. ' at ' .. tostring(t1.hit and t1.hit - t0))
    check(strat.bombs == 1, 'the strategic bomber dropped its bomb on the first pass, dropped ' .. strat.bombs)
    check(strat.damage == 3100, 'the bomb did its 3100 damage, did ' .. strat.damage)
    check(strat.hit and strat.hit - t0 <= 50, 'within 50 ticks of the order, at ' .. tostring(strat.hit and strat.hit - t0))
    check(t1.bombs == 4, 'the T1 bomber dropped its salvo of 4, dropped ' .. t1.bombs)
    check(t1.damage > 150, 'and its bombs hit, damage ' .. t1.damage)
    done()
end)
