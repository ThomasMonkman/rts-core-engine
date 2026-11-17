-- Scenario test (tests/scenarios.rs): AIBrain:BuildStructure snaps the site
-- as build orders do (footprint from round(v - size / 2), halves to even: a
-- 1x1 generator at an even 400.3 goes to 400.5, at an odd 403 to 402.5) and
-- moves one blocked by a structure 8 to the north-west (then on through the
-- near-spot search); it returns false for an absolute site (dev/probes
-- probe_re2_build BS). An extractor asked on an odd deposit stays on it.
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
    local e3 = make('uel0105', 420, 620)
    brain:BuildStructure(e3, 'ueb1101', { 403, 605, 0 }, false)
    check(site(e3) == '402.5 604.5', 'odd whole numbers go down to the cell centre: ' .. site(e3))
    local e4 = make('uel0105', 160, 650)
    brain:BuildStructure(e4, 'ueb1103', { 155.5, 659.5, 0 }, false)
    check(site(e4) == '155.5 659.5', 'an extractor stays on its deposit at odd 155.5 659.5: ' .. site(e4))
    ScenarioResult.done = true
end)
