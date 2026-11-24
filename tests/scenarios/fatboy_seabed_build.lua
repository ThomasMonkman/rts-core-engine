-- Scenario test (tests/scenarios.rs): a Fatboy building on land and on the
-- seabed (issue #85; the real game: dev/probes probe_fatboy_water).
-- Its external factory (`uel0401ef`, no motion) is attached to an anchor
-- unit for good yet never `Attached`, so it starts the orders it is given.
-- On the seabed FAF restricts the external factory to nothing: an order to
-- it is not taken. An order to the Fatboy itself (`brain:BuildUnit`, as
-- FAF's AI gives it) still builds there, in the real game too, and a tank
-- made that way drowns.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    local ok, err = pcall(function()
        WaitTicks(2)
        local brain = GetArmyBrain('ARMY_1')
        brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
        brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
        -- On the beach north of the bay, facing the water: the shore west
        -- of here (x 433 at z 620) is a cliff it has to walk round to this
        -- beach (dev/probes probe_fatboy_path), and here the sand runs down
        -- into the water, 3 deep by z 596.
        local fatboy = CreateUnitHPR('uel0401', 'ARMY_1', 479, GetTerrainHeight(479, 579), 579, 0, 0, 0)
        fatboy:SetCanTakeDamage(false)
        WaitTicks(30)
        local ef = fatboy.ExternalFactory
        check(ef ~= nil, 'the Fatboy has its external factory')
        if not ef then return end
        -- The finished tanks, but for `but`.
        local function tanks(but)
            local list = {}
            for _, u in brain:GetListOfUnits(categories.uel0202, false) or {} do
                if u ~= but and not u:BeenDestroyed() and u:GetFractionComplete() >= 1 then table.insert(list, u) end
            end
            return list
        end
        local function wait(ticks, stop)
            for _ = 1, ticks do
                WaitTicks(1)
                if stop() then return true end
            end
            return false
        end

        -- On land, through the external factory.
        check(fatboy:GetCurrentLayer() == 'Land', 'starts on land: ' .. fatboy:GetCurrentLayer())
        check(not ef:IsUnitState('Attached'), 'the external factory is not in the Attached state')
        check(ef:CanBuild('uel0202'), 'the external factory can build a tank on land')
        IssueBuildFactory({ ef }, 'uel0202', 1)
        WaitTicks(3)
        check(ef:IsUnitState('Building'), 'the external factory starts its order')
        check(wait(200, function() return table.getn(tanks()) == 1 end), 'a tank is built on land')
        wait(60, function() return not ef:IsUnitState('BlockCommandQueue') end)
        local first = tanks()[1]
        if first then
            first:SetCanTakeDamage(false)
            check(first:GetCurrentLayer() == 'Land', 'the tank built on land rolls off onto land: ' .. first:GetCurrentLayer())
        end

        -- Onto the seabed.
        IssueMove({ fatboy }, { 478, GetTerrainHeight(478, 600), 600 })
        check(wait(600, function() return fatboy:GetCurrentLayer() == 'Seabed' end), 'the Fatboy walks onto the seabed')
        wait(400, function() return not fatboy:IsUnitState('Moving') end)
        WaitTicks(5)
        check(not ef:CanBuild('uel0202'), 'on the seabed the external factory can build nothing')
        check(fatboy:CanBuild('uel0202'), 'the Fatboy itself is not restricted (FAF restricts only the external factory)')
        IssueBuildFactory({ ef }, 'uel0202', 1)
        check(table.getn(ef:GetCommandQueue()) == 0, 'the restricted order is not taken')
        local started
        wait(60, function() started = started or ef:IsUnitState('Building') or fatboy:IsUnitState('Building') return false end)
        check(not started, 'nothing is started under water through the external factory')
        check(table.getn(tanks(first)) == 0, 'no tank through the external factory')

        -- As FAF's AI orders it (AIBehaviors.lua FatBoyBuildCheck): the real
        -- game builds, and the tank drowns.
        brain:BuildUnit(fatboy, 'uel0202', 1)
        check(wait(200, function() return table.getn(tanks(first)) == 1 end), 'ordered on the Fatboy itself, a tank is built on the seabed')
        local tank = tanks(first)[1]
        if tank then
            check(tank:GetCurrentLayer() == 'Seabed', 'the tank is on the seabed: ' .. tank:GetCurrentLayer())
            check(wait(200, function() return tank.Dead or tank:BeenDestroyed() end), 'the tank drowns')
            check(first and not first.Dead and not first:BeenDestroyed(), 'the tank built on land is still there')
        end
    end)
    if not ok then check(false, tostring(err)) end
    ScenarioResult.done = true
end)
