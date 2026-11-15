-- Scenario test (tests/scenarios.rs): an engineer on an attack-move keeps
-- the order and reclaims props on the way, as in the real game (dev/probes
-- probe_unit_misc A). FAF's `ConstructionUnit.OnPrepareArmToBuild` aborts
-- the move when it starts reclaiming; that must not drop the order.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local p = CreateProp({ 406, GetTerrainHeight(406, 601), 601 }, '/env/evergreen/props/rocks/fieldstone01_prop.bp')
    p:SetMaxReclaimValues(2, 20, 0)
    WaitTicks(2)
    IssueAggressiveMove({eng}, { 430, GetTerrainHeight(430, 600), 600 })
    local reclaimed = false
    -- (`WaitTicks(n)` waits n - 1.)
    for t = 1, 350 do
        WaitTicks(3)
        if t == 5 then check(table.getn(eng:GetCommandQueue()) == 1, 'the order is still there 10 ticks on') end
        if p:BeenDestroyed() then reclaimed = true end
        if reclaimed and table.getn(eng:GetCommandQueue()) == 0 then break end
    end
    check(reclaimed, 'the prop on the way was reclaimed')
    local x = eng:GetPosition()[1]
    note(string.format('engineer ended at x %.1f', x))
    check(math.abs(x - 430) < 3, 'and the engineer went on to the goal')
    ScenarioResult.done = true
end)
