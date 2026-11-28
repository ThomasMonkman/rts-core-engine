-- Scenario test (tests/scenarios.rs): structure adjacency (moho's
-- OnAdjacentTo / OnNotAdjacentTo). A mass storage two cells from an
-- extractor (their build skirts touch) makes them adjacent and buffs the
-- extractor's production; destroying the storage undoes it. A storage one
-- cell further off isn't adjacent.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local ex = make('ueb1103', 400.5, 600.5)
    WaitTicks(5)
    local base = ex:GetProductionPerSecondMass()
    local far = make('ueb1106', 400.5, 603.5)
    WaitTicks(5)
    check(not (ex.AdjacentUnits and ex.AdjacentUnits[far.EntityId]), 'a storage three cells off is not adjacent')
    local st = make('ueb1106', 402.5, 600.5)
    WaitTicks(5)
    local buffed = ex:GetProductionPerSecondMass()
    note(string.format('extractor mass/s alone %.3f, with an adjacent storage %.3f', base, buffed))
    check(ex.AdjacentUnits and ex.AdjacentUnits[st.EntityId] == st, 'the extractor knows the adjacent storage')
    check(st.AdjacentUnits and st.AdjacentUnits[ex.EntityId] == ex, 'and the storage knows the extractor')
    check(buffed > base, 'the storage buffs the extractor')
    st:Destroy()
    WaitTicks(5)
    check(not ex.AdjacentUnits[st.EntityId], 'a destroyed storage is no longer adjacent')
    check(math.abs(ex:GetProductionPerSecondMass() - base) < 1e-6, 'and its buff is gone')

    -- Two structures destroyed together part from each other once each way
    -- (dev/probes probe_unit_build G1).
    local log = {}
    local function watch(u, tag)
        local base = u.OnNotAdjacentTo
        u.OnNotAdjacentTo = function(self, other)
            table.insert(log, tag)
            return base(self, other)
        end
    end
    local a = make('ueb1101', 420, 600)
    local b = make('ueb1101', 422, 600)
    WaitTicks(2)
    watch(a, 'a') watch(b, 'b')
    a:Destroy() b:Destroy()
    WaitTicks(2)
    check(table.concat(log, ',') == 'a,b', 'destroyed together: each parts once: ' .. table.concat(log, ','))
    ScenarioResult.done = true
end)
