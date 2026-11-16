-- Scenario test (tests/scenarios.rs): AIBrain:BuildStructure snaps the site
-- to 2 * floor(v / 2) + 0.5 and moves one blocked by a structure 8 to the
-- north-west (then on through the near-spot search); it returns false for
-- an absolute site (dev/probes probe_re2_build BS).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    local function site(e)
        local c = e:GetCommandQueue()[1]
        return c and string.format('%.1f %.1f', c.x, c.z) or '-'
    end
    local e1 = make('uel0105', 400, 620)
    local r = brain:BuildStructure(e1, 'ueb1101', { 400.3, 600.7, 0 }, false)
    check(r == false, 'false for an absolute site: ' .. tostring(r))
    check(site(e1) == '400.5 600.5', 'snapped: ' .. site(e1))
    make('ueb1101', 432, 600)
    local e2 = make('uel0105', 432, 620)
    brain:BuildStructure(e2, 'ueb1101', { 432, 600, 0 }, false)
    check(site(e2) == '424.5 592.5', 'moved 8 north-west off a generator: ' .. site(e2))
    ScenarioResult.done = true
end)
