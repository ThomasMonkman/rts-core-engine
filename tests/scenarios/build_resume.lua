-- Scenario test (tests/scenarios.rs): a build order given again on the site
-- of the same unit left unfinished there takes that unit up again, rather
-- than starting a second one (real game, dev/probes probe_t4_vanish: FAF's
-- AIs clear an engineer's orders and give the build again, sometimes on the
-- tick it started).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    local site = {403.5, GetTerrainHeight(403.5, 600.5), 600.5}
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 605), 605, 0, 0, 0)
    local first, starts = nil, 0
    eng:AddUnitCallback(function(self, built)
        starts = starts + 1
        if not first then
            first = built
            IssueClearCommands({self})
            IssueBuildMobile({self}, site, 'ueb1101', {})
        end
    end, 'OnStartBuild')
    IssueBuildMobile({eng}, site, 'ueb1101', {})
    for _ = 1, 400 do
        WaitTicks(1)
        if first and not IsDestroyed(first) and first:GetFractionComplete() >= 1 then break end
    end
    check(first ~= nil, 'the engineer started the generator')
    if not first then ScenarioResult.done = true return end
    check(not IsDestroyed(first), 'the first generator still stands')
    check(starts == 2, 'OnStartBuild twice (start, then taken up again): ' .. starts)
    check(not IsDestroyed(first) and first:GetFractionComplete() >= 1, 'the first generator was finished')
    local n = table.getn(b:GetListOfUnits(categories.ueb1101, false))
    check(n == 1, 'one generator, not a second over it: ' .. n)
    ScenarioResult.done = true
end)
