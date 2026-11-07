-- Scenario test (tests/scenarios.rs): moho's dispatch hold and the instant
-- orders (MohoEngine.dll `IAiCommandDispatchImpl::TaskTick` 0x101893a0, the
-- dispatch switch 0x101f4000).
-- A factory rolling a tank off (FAF's `SetBlockCommandQueue(true)`) doesn't
-- start the next tank until the block is lifted. `IssuePause` toggles the
-- unit's pause and pops, so the order after it runs; it waits while the
-- unit's queue is blocked. `IssueKillSelf` kills the unit, `IssueDestroySelf`
-- destroys it.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)

    -- Instant orders.
    local eng = make('uel0105', 420, 620)
    local victim = make('uel0201', 430, 630)
    local gone = make('uel0201', 435, 630)
    eng:SetBlockCommandQueue(true)
    IssuePause({ eng })
    IssuePause({ eng })
    IssueMove({ eng }, { 425, GetTerrainHeight(425, 620), 620 })
    IssueKillSelf({ victim })
    IssueDestroySelf({ gone })
    WaitTicks(3)
    check(not eng:IsPaused() and table.getn(eng:GetCommandQueue()) == 3, 'a blocked queue holds the pause')
    eng:SetBlockCommandQueue(false)
    WaitTicks(1)
    check(eng:IsPaused(), 'the first IssuePause pauses')
    WaitTicks(1)
    check(not eng:IsPaused(), 'the second unpauses')
    WaitTicks(30)
    local q = eng:GetCommandQueue()
    check(table.getn(q) == 0, 'then the move runs: the queue isn\'t stalled (' .. table.getn(q) .. ' left)')
    check(victim.Dead, 'IssueKillSelf kills the unit')
    check(IsDestroyed(gone), 'IssueDestroySelf destroys it')

    -- More than 500 queued: no further (non-clearing) orders.
    local busy = make('uel0105', 410, 620)
    busy:SetBlockCommandQueue(true)
    for _ = 1, 505 do IssuePause({ busy }) end
    check(table.getn(busy:GetCommandQueue()) == 501, 'the queue stops taking orders past 500 (' .. table.getn(busy:GetCommandQueue()) .. ')')
    IssueClearCommands({ busy })
    busy:SetBlockCommandQueue(false)

    -- The factory's roll-off.
    local land = make('ueb0101', 400, 600)
    IssueBuildFactory({ land }, 'uel0201', 2)
    local started_while_blocked, blocked_ticks, tanks = false, 0, 0
    for _ = 1, 900 do
        WaitTicks(1)
        local blocked = land:IsUnitState('BlockCommandQueue')
        if blocked then blocked_ticks = blocked_ticks + 1 end
        local building, done = 0, 0
        for _, u in b:GetListOfUnits(categories.uel0201, false) or {} do
            if u ~= victim and u ~= gone then
                if u:GetFractionComplete() < 1 then building = building + 1 else done = done + 1 end
            end
        end
        tanks = done
        if blocked and done >= 1 and building > 0 then started_while_blocked = true end
        if done >= 2 then break end
    end
    note(string.format('factory blocked %d ticks, %d tanks', blocked_ticks, tanks))
    check(blocked_ticks > 0, 'FAF blocks the factory\'s queue for the roll-off')
    check(not started_while_blocked, 'the next tank waits for the roll-off')
    check(tanks >= 2, 'then the factory builds it')
    ScenarioResult.done = true
end)
