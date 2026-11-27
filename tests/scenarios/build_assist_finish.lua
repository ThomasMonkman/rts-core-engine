-- Scenario test (tests/scenarios.rs): a structure whose paused builder holds
-- its job is finished by a helper. As in the real game (dev/probes
-- probe_padbuild `hold-callback`), its `OnStopBeingBuilt(builder)` runs in
-- the tick the fraction reaches 1, before the helper's `OnStopBuild`, and
-- once only, even when the builder's order is cleared inside that
-- `OnStopBuild` (what an AI does to hand the builder its next job).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 20000) b:GiveResource('ENERGY', 400000)
    local x, z = 400, 600
    local e1 = CreateUnitHPR('uel0105', 'ARMY_1', x, GetTerrainHeight(x, z + 4), z + 4, 0, 0, 0)
    local e2 = CreateUnitHPR('uel0105', 'ARMY_1', x + 2, GetTerrainHeight(x + 2, z + 4), z + 4, 0, 0, 0)
    e2:SetBuildRate(40)
    IssueBuildMobile({ e1 }, { x, GetTerrainHeight(x, z), z }, 'ueb1101', {})
    local s
    for _ = 1, 60 do
        WaitTicks(1)
        s = e1:GetFocusUnit()
        if s then break end
    end
    check(s ~= nil, 'the build started')
    if not s then ScenarioResult.done = true return end

    local log = {}
    local oldStopped, oldStop = s.OnStopBeingBuilt, e2.OnStopBuild
    for _, name in { 'OnStopBuild', 'OnFailedToBuild' } do
        local n, orig = name, e1[name]
        e1[n] = function(self, a, b)
            table.insert(log, 'e1.' .. n)
            return orig(self, a, b)
        end
    end
    s.OnStopBeingBuilt = function(self, builder, layer)
        table.insert(log, 'structure.OnStopBeingBuilt by ' .. ((builder == e1 and 'e1') or (builder == e2 and 'e2') or '?') .. ' frac ' .. self:GetFractionComplete())
        return oldStopped(self, builder, layer)
    end
    e2.OnStopBuild = function(self, built, order)
        table.insert(log, 'e2.OnStopBuild')
        IssueClearCommands({ e1 })
        return oldStop(self, built, order)
    end
    e1:SetPaused(true)
    IssueRepair({ e2 }, s)
    for _ = 1, 200 do
        WaitTicks(1)
        if s:GetFractionComplete() >= 1 then break end
    end
    local got = table.concat(log, ', ')
    -- The cleared builder hears both of its callbacks, inside the clear.
    check(got == 'structure.OnStopBeingBuilt by e1 frac 1, e2.OnStopBuild, e1.OnStopBuild, e1.OnFailedToBuild', 'in the tick it reaches 1: ' .. got)
    check(not s:IsUnitState('BeingBuilt'), 'the structure is no longer BeingBuilt')
    e1:SetPaused(false)
    WaitTicks(5)
    check(table.getn(log) == 4, 'nothing more after: ' .. table.concat(log, ', '))
    check(not s:IsUnitState('BeingBuilt') and not e1:IsUnitState('Building'), 'the structure is finished and the builder free')
    ScenarioResult.done = true
end)
