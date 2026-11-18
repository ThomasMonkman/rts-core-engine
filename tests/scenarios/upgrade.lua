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
-- A T1 extractor upgrades to T2 in place; an upgrade cancelled or whose old
-- structure dies takes its half-built T2 with it.
ForkThread(function()
    WaitTicks(2)
    local brain = rich(1)
    local function t2s() return brain:GetListOfUnits(categories.ueb1202, false) or {} end
    local full = CreateUnitHPR('ueb1103', 'ARMY_1', 400, ground(400, 600), 600, 0, 0, 0)
    local cancel = CreateUnitHPR('ueb1103', 'ARMY_1', 420, ground(420, 600), 600, 0, 0, 0)
    local kill = CreateUnitHPR('ueb1103', 'ARMY_1', 440, ground(440, 600), 600, 0, 0, 0)
    -- The upgrade and the structure it upgrades never hear of each other
    -- as neighbours (dev/probes probe_unit_build G4).
    local paired = {}
    for _, cb in { 'OnAdjacentTo', 'OnNotAdjacentTo' } do
        local base = full[cb]
        full[cb] = function(self, other, ...)
            if other and other.UnitId == 'ueb1202' then table.insert(paired, cb) end
            return base(self, other, unpack(arg))
        end
    end
    WaitTicks(5)
    IssueUpgrade({full}, 'ueb1202')
    IssueUpgrade({cancel}, 'ueb1202')
    IssueUpgrade({kill}, 'ueb1202')
    WaitTicks(30)
    check(table.getn(t2s()) == 3, 'three upgrades started, got ' .. table.getn(t2s()))
    check(full:IsUnitState('Upgrading'), 'upgrading extractor is in the Upgrading state')
    -- Its upgrade animation (`Display.AnimationUpgrade`) follows the build.
    local anim = full.AnimatorUpgradeManip
    check(anim ~= nil, 'the upgrade animator exists')
    local early = anim and anim:GetAnimationFraction() or -1
    local built_early = full.UnitBeingBuilt and full.UnitBeingBuilt:GetFractionComplete() or -1
    IssueClearCommands({cancel})
    kill:Kill()
    WaitTicks(5)
    check(table.getn(t2s()) == 1, 'cancel and kill removed their half-built T2s, ' .. table.getn(t2s()) .. ' left')
    check(not cancel:IsUnitState('Upgrading'), 'the cancelled extractor left the Upgrading state')
    WaitTicks(40)
    local later = anim and anim:GetAnimationFraction() or -1
    local built_later = full.UnitBeingBuilt and full.UnitBeingBuilt:GetFractionComplete() or -1
    note(string.format('upgrade animation %.3f -> %.3f while built %.3f -> %.3f; clip %.2f s, at %.2f s', early, later, built_early, built_later, anim and anim:GetAnimationDuration() or -1, anim and anim:GetAnimationTime() or -1))
    check(later > early, 'the upgrade animation advances with the build')
    for _ = 1, 300 do
        WaitTicks(10)
        if full:BeenDestroyed() then break end
    end
    check(full:BeenDestroyed(), 'the old T1 is removed once the upgrade completes')
    local t2 = t2s()
    check(table.getn(t2) == 1 and t2[1]:GetFractionComplete() == 1, 'one finished T2 extractor')
    check(table.getn(paired) == 0, 'no adjacency callbacks between the T1 and its T2: ' .. table.concat(paired, ','))
    note('upgrade done at tick ' .. GetGameTick())
    done()
end)
