-- Scenario test (tests/scenarios.rs): ARMY_1 / ARMY_2 have no AI; checks go
-- into ScenarioResult, which the test reads once `done` is set.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function done() ScenarioResult.done = true end
local function ground(x, z) return GetTerrainHeight(x, z) end
-- Reclaiming a finished unit drains its health and pays nothing; once it is
-- used up its script leaves its wreck (`CreateWreckageProp(0)`, called by
-- the engine after `OnReclaimed`), which the engineer goes on to reclaim
-- under the same order (MohoEngine.dll 0x10209a86..0x10209b9c). From the
-- real game (dev/probes probe_unit_reclaim2): a T1 tank left a 45.36 mass
-- wreck and the army gained exactly that, damaged or not. A reclaim broken
-- off partway leaves the unit standing, built, with the health it has left.
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    brain:__engine_SetStorage('MASS', 10000)
    brain:__engine_SetStorage('ENERGY', 10000)
    local function stored() return brain:GetEconomyStored('MASS'), brain:GetEconomyStored('ENERGY') end
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 404, ground(404, 600), 600, 0, 0, 0)

    -- Broken off partway: built, weaker, nothing gained.
    local target = CreateUnitHPR('ual0105', 'ARMY_2', 408, ground(408, 600), 600, 0, 0, 0)
    local max = target:GetMaxHealth()
    local m0, e0 = stored()
    IssueReclaim({eng}, target)
    for _ = 1, 200 do
        WaitTicks(2)
        if target:GetHealth() < max * 0.6 then break end
    end
    IssueClearCommands({eng})
    WaitTicks(3)
    local m1, e1 = stored()
    local fraction, health = target:GetFractionComplete(), target:GetHealth()
    note(string.format('broken off at fraction %.3f, health %.1f / %.0f, gained %.2f mass %.2f energy', fraction, health, max, m1 - m0, e1 - e0))
    check(fraction == 1, 'a finished unit keeps its fraction while reclaimed: ' .. fraction)
    check(health < max * 0.6 and health > 0, 'its health was drained: ' .. health)
    check(not target:BeenDestroyed() and not target.Dead, 'a partly reclaimed unit stays')
    check(m1 - m0 < 0.001 and e1 - e0 < 0.001, 'draining a unit pays nothing')
    WaitTicks(30)
    check(math.abs(target:GetHealth() - health) < 1e-6, 'nobody works on it: it kept its health')

    -- Reclaimed to the end: no pay for the unit, its wreck, and the wreck's
    -- value from reclaiming that under the same order.
    local function reclaimToEnd(unit, label)
        local wreckValue
        local hooked = unit.CreateWreckageProp
        unit.CreateWreckageProp = function(self, overkill)
            local prop = hooked(self, overkill)
            wreckValue = prop and prop.MaxMassReclaim
            note(string.format('%s: CreateWreckageProp(%s) -> %s mass', label, tostring(overkill), tostring(wreckValue)))
            return prop
        end
        local ma, ea = stored()
        IssueReclaim({eng}, unit)
        local gone, gainedBefore
        for _ = 1, 600 do
            WaitTicks(2) -- one tick
            if not gone and unit:BeenDestroyed() then
                gone = true
                gainedBefore = brain:GetEconomyStored('MASS') - ma
                check(table.getn(eng:GetCommandQueue()) == 1, label .. ': the order goes on to the wreck')
            end
            if gone and eng:IsIdleState() then break end
        end
        -- Reclaim is stored on the next economy tick.
        WaitTicks(3)
        local mb, eb = stored()
        note(string.format('%s: gained %.2f mass %.2f energy (%.2f before the wreck)', label, mb - ma, eb - ea, gainedBefore or -1))
        check(gone, label .. ': the unit was reclaimed')
        check(gainedBefore and gainedBefore < 0.001, label .. ': the unit itself paid nothing')
        check(wreckValue and wreckValue > 0, label .. ': it left a wreck with value')
        check(wreckValue and math.abs((mb - ma) - wreckValue) < 0.01, label .. ': the army gained the wreck\'s value')
        check(eng:IsIdleState(), label .. ': the engineer finished the wreck')
        return mb - ma
    end
    local full = reclaimToEnd(target, 'weakened unit')
    local hurt = CreateUnitHPR('ual0105', 'ARMY_2', 404, ground(404, 604), 604, 0, 0, 0)
    hurt:SetHealth(nil, hurt:GetMaxHealth() * 0.3)
    local fromHurt = reclaimToEnd(hurt, 'damaged unit')
    check(math.abs(full - fromHurt) < 0.01, 'a damaged unit leaves the same wreck as a healthy one')
    done()
end)
