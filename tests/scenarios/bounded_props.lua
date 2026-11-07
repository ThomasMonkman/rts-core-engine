-- Scenario test (tests/scenarios.rs): moho's bounded prop list
-- (`Prop:AddBoundedProp`, #478). It holds 1000 props; adding one more
-- destroys the front one first: the lowest priority, then the oldest (by
-- tick). A destroyed prop leaves the list, making room. Checked with
-- `BeenDestroyed`: `IsDestroyed` turns true only after the beat.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local bp = '/env/evergreen/props/rocks/fieldstone01_prop.bp'
local function rock(i)
    local x, z = 380 + math.mod(i, 40) * 2, 560 + math.floor(i / 40) * 2
    return CreateProp({x, GetTerrainHeight(x, z), z}, bp)
end
ForkThread(function()
    WaitTicks(2)
    -- The front: priority 1 (rounded up from 0.5), added first.
    local low = rock(0)
    low:AddBoundedProp(0.5)
    WaitTicks(1)
    -- A later one with the same priority, then one with a lower priority
    -- than the rest but added later still: it goes before both.
    local same = rock(1)
    same:AddBoundedProp(1)
    WaitTicks(1)
    local lowest = rock(2)
    lowest:AddBoundedProp(-3)
    local props = {}
    for i = 3, 999 do
        local p = rock(i)
        p:AddBoundedProp(100)
        props[i] = p
    end
    -- 1000 in the list: nothing destroyed yet.
    check(not lowest:BeenDestroyed() and not low:BeenDestroyed() and not same:BeenDestroyed(), 'a list of 1000 destroys nothing')
    -- A destroyed prop leaves the list, so the next add destroys nothing.
    props[500]:Destroy()
    rock(1000):AddBoundedProp(100)
    check(not lowest:BeenDestroyed(), 'a destroyed prop made room')
    -- Full again: each add destroys the front, lowest priority first, then
    -- the older of equal priorities.
    rock(1001):AddBoundedProp(100)
    check(lowest:BeenDestroyed(), 'the lowest priority goes first')
    check(not low:BeenDestroyed() and not same:BeenDestroyed(), 'only one goes per add')
    rock(1002):AddBoundedProp(100)
    check(low:BeenDestroyed(), 'then the older of equal priorities')
    check(not same:BeenDestroyed(), 'the newer one stays')
    rock(1003):AddBoundedProp(100)
    check(same:BeenDestroyed(), 'then the newer one')
    check(not props[3]:BeenDestroyed(), 'higher priorities stay')
    ScenarioResult.done = true
end)
