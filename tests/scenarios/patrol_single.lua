-- Scenario test (tests/scenarios.rs): a script's single `IssuePatrol` point
-- is a one-way trip, as in the real game (dev/probes probe_unit_orders O3):
-- the unit goes there and the order ends. (The player's patrol loops back:
-- the interface's leg, `Command::patrol_home`, tests/player_orders.rs.)
-- Two points still go round.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local one = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0)
    local two = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    WaitTicks(2)
    IssuePatrol({one}, { 415, 0, 580 })
    IssuePatrol({two}, { 415, 0, 600 })
    IssuePatrol({two}, { 415, 0, 615 })
    for _ = 1, 80 do
        WaitTicks(3)
        if table.getn(one:GetCommandQueue()) == 0 then break end
    end
    local p = one:GetPosition()
    note(string.format('single-point patroller ended at %.1f, %.1f', p[1], p[3]))
    check(table.getn(one:GetCommandQueue()) == 0, 'a single patrol point ends on arrival')
    check(VDist2(p[1], p[3], 415, 580) < 3, 'after getting there')
    WaitTicks(30)
    check(table.getn(one:GetCommandQueue()) == 0 and VDist2(one:GetPosition()[1], one:GetPosition()[3], 415, 580) < 3, 'and the unit stays put')
    check(table.getn(two:GetCommandQueue()) == 2, 'a two-point patrol keeps going round')
    ScenarioResult.done = true
end)
