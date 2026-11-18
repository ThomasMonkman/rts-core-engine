-- Scenario test (tests/scenarios.rs): when an engineer's guard (assist)
-- ends. As moho's guard task (MohoEngine.dll 0x101fe9b0 / 0x101fcfa0):
-- an engineer with nothing to help with lets go of its guard when another
-- order is queued behind it, so a shift-queued build takes over once the
-- assisted engineer's build is done (not before: it keeps helping while
-- there is work); a guard started on a unit under construction ends once
-- that unit is finished and idle. Non-engineers keep guarding, as does an
-- engineer with nothing queued behind.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function at(x, z) return { x, GetTerrainHeight(x, z), z } end
local function unit(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local function waitFor(ticks, f)
    for _ = 1, ticks do
        if f() then return true end
        WaitTicks(1)
    end
    return f()
end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)

    -- Engineer A assists builder B, then gets a build queued behind the assist.
    local builder, helper = unit('uel0105', 400, 600), unit('uel0105', 396, 600)
    -- An engineer guarding B's construction itself (started unfinished).
    local siteGuard = unit('uel0105', 404, 594)
    -- Guards that keep going: an engineer with nothing queued, and a tank
    -- with a move queued (only engineers let go).
    local tank = unit('uel0201', 400, 650)
    local idleGuard, tankGuard = unit('uel0105', 396, 650), unit('uel0201', 396, 654)
    -- An engineer guarding a tank, a move queued behind: lets go at once.
    local movingGuard = unit('uel0105', 404, 650)
    -- Non-queued: a build replaces the assist whatever.
    local other, replaced = unit('uel0105', 500, 600), unit('uel0105', 496, 600)

    IssueBuildMobile({ builder }, at(410.5, 600.5), 'ueb1101', {})
    IssueBuildMobile({ other }, at(510.5, 600.5), 'ueb1101', {})
    local built
    check(waitFor(200, function() built = builder:GetFocusUnit() return built ~= nil end), 'B started its build')
    if not built then ScenarioResult.done = true return end
    IssueGuard({ helper }, builder)
    IssueGuard({ siteGuard }, built)
    IssueGuard({ idleGuard, tankGuard, movingGuard }, tank)
    IssueGuard({ replaced }, other)
    check(waitFor(100, function() return helper:GetFocusUnit() == built end), 'A helps with B\'s build')
    -- Queued behind the assist (Issue* appends, as shift-click does).
    IssueBuildMobile({ helper }, at(400.5, 614.5), 'ueb1101', {})
    IssueMove({ tankGuard, movingGuard }, at(420, 670))
    WaitTicks(10)
    check(built:GetFractionComplete() < 1, 'B\'s build still unfinished: ' .. built:GetFractionComplete())
    check(helper:GetFocusUnit() == built, 'A keeps helping while B\'s build is unfinished, its own build queued')
    check(helper:GetGuardedUnit() == builder, 'A still guarding B while there is work')
    check(movingGuard:GetGuardedUnit() == nil, 'an engineer with nothing to help lets go of its guard for the queued move')
    check(tankGuard:GetGuardedUnit() == tank, 'a tank keeps guarding with a move queued')

    -- Non-queued: cleared and given a build, it starts on it straight away.
    IssueClearCommands({ replaced })
    IssueBuildMobile({ replaced }, at(500.5, 614.5), 'ueb1101', {})
    WaitTicks(2)
    check(replaced:GetGuardedUnit() == nil, 'a non-queued build replaces the assist')

    check(waitFor(600, function() return built:GetFractionComplete() >= 1 end), 'B\'s build finished')
    local own
    check(waitFor(200, function()
        own = helper:GetFocusUnit()
        return own ~= nil and own ~= built
    end), 'A went on to its queued build after B\'s was done')
    if own and own ~= built then
        local p = own:GetPosition()
        check(VDist2(p[1], p[3], 400.5, 614.5) < 1, 'A builds at its own site: ' .. p[1] .. ', ' .. p[3])
        check(helper:GetGuardedUnit() == nil, 'A\'s guard is over')
    end
    check(waitFor(30, function() return siteGuard:GetGuardedUnit() == nil end), 'the guard on the finished construction ended')
    check(siteGuard:IsIdleState(), 'its guard over, the construction\'s guard is idle')
    check(idleGuard:GetGuardedUnit() == tank, 'an engineer with nothing queued keeps guarding')
    note('A finished assisting at fraction ' .. built:GetFractionComplete())
    ScenarioResult.done = true
end)
