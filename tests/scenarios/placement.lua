-- Scenario test (tests/scenarios.rs): structure placement keeps build skirts
-- apart. An extractor and a mass storage both have 2x2 skirts around a 1x1
-- footprint, so storages ring an extractor two cells out, not one.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    CreateUnitHPR('ueb1103', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local brain = ArmyBrains[1]
    local function can(bp, dx, dz) return brain:CanBuildStructureAt(bp, {x + dx, GetTerrainHeight(x + dx, z + dz), z + dz}) end
    check(not can('ueb1106', 1, 0), 'a storage one cell from the extractor overlaps its skirt')
    check(not can('ueb1106', 1, 1), 'nor diagonally one cell out')
    check(can('ueb1106', 2, 0), 'a storage two cells out fits')
    check(can('ueb1106', -2, 0), 'on either side')
    check(can('ueb1106', 2, 2), 'and on the diagonal')
    check(not can('ueb0101', 4, 0), 'a land factory (8x8 skirt) four cells out overlaps')
    check(can('ueb0101', 5, 0), 'five cells out it fits')
    -- A position off the cell grid is snapped (the footprint's corner rounded
    -- half to even) before skirts are compared: a 5x5 factory 5 east of a 2x2
    -- generator, both at whole numbers, overlaps; 6 east fits.
    local gx, gz = 420, 600
    CreateUnitHPR('ueb1101', 'ARMY_1', gx, GetTerrainHeight(gx, gz), gz, 0, 0, 0)
    local function at(bp, px, pz) return brain:CanBuildStructureAt(bp, {px, GetTerrainHeight(px, pz), pz}) end
    check(not at('ueb0101', gx + 5, gz), 'a factory 5 east of a generator (unsnapped) overlaps')
    check(at('ueb0101', gx + 6, gz), 'a factory 6 east of a generator fits')
    -- A mobile blueprint can't be placed on a structure; a structure can go
    -- over a mobile unit.
    check(not at('uel0201', gx, gz), 'a tank blueprint on a generator is refused')
    CreateUnitHPR('uel0201', 'ARMY_1', gx, GetTerrainHeight(gx, gz + 20), gz + 20, 0, 0, 0)
    check(at('ueb1101', gx, gz + 20), 'a generator over a tank is allowed')
    -- A queued build order reserves its site, before the builder gets there.
    local sx, sz = 440, 600
    check(at('ueb1101', sx, sz), 'the site is free before the order')
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', sx - 30, GetTerrainHeight(sx - 30, sz), sz, 0, 0, 0)
    IssueBuildMobile({eng}, {sx, GetTerrainHeight(sx, sz), sz}, 'ueb1101', {})
    IssueBuildMobile({eng}, {sx, GetTerrainHeight(sx, sz + 6), sz + 6}, 'ueb1101', {})
    check(not at('ueb1101', sx, sz), 'the first queued build reserves its site')
    check(not at('ueb1101', sx + 1, sz + 6), 'and so does the second')
    check(at('ueb1101', sx + 2, sz), 'skirts beside the site stay free')
    ScenarioResult.done = true
end)
