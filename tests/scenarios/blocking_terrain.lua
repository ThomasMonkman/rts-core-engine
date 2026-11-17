-- Scenario test (tests/scenarios.rs): CheckBlockingTerrain with arc 'none'
-- tolerates ground a little over the line in the real game (dev/probes probe_ai_brain: a
-- 20-unit flat line and a long one rising 0.37 over the chord are clear);
-- an unknown arc never blocks. RNGAI's platoons ask it with 'none'.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local function p(x, z, dy) return { x, GetTerrainHeight(x, z) + (dy or 0), z } end
    -- The most the ground rises over the chord between two ground points.
    local function rise(x0, z0, x1, z1)
        local y0, y1, m = GetTerrainHeight(x0, z0), GetTerrainHeight(x1, z1), -1000
        for i = 0, 100 do
            local t = i / 100
            m = math.max(m, GetTerrainHeight(x0 + (x1 - x0) * t, z0 + (z1 - z0) * t) - (y0 + (y1 - y0) * t))
        end
        return m
    end
    local m = rise(400, 600, 460, 600)
    -- Ends shifted so the ground rises 0.3 over the line: clear; 0.9: blocked.
    check(not brain:CheckBlockingTerrain(p(400, 600, m - 0.3), p(460, 600, m - 0.3), 'none'), 'ground 0.3 over the line is clear')
    check(not brain:CheckBlockingTerrain(p(400, 600, m - 0.3), p(460, 600, m - 0.3), 'None'), 'any case')
    check(brain:CheckBlockingTerrain(p(400, 600, m - 0.9), p(460, 600, m - 0.9), 'none'), 'ground 0.9 over the line blocks')
    check(not brain:CheckBlockingTerrain(p(400, 600, m - 0.9), p(460, 600, m - 0.9), 'bogus'), 'an unknown arc never blocks')
    ScenarioResult.done = true
end)
