-- Scenario test (tests/scenarios.rs): `CreateUnitHPR` puts a unit on its
-- layer's height whatever y it's given, as the real game does (dev/probes
-- probe_unit_api `layer`): a tank made 30 up stands on the ground, a power
-- generator made at the water surface sits on the seabed, a hover tank and a
-- torpedo launcher made on the seabed are on the surface.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local site
    local w, h = GetMapSize()
    for z = 100, h - 100, 4 do
        for x = 100, w - 100, 4 do
            if not site and GetSurfaceHeight(x, z) - GetTerrainHeight(x, z) > 6 and GetSurfaceHeight(x + 12, z) - GetTerrainHeight(x + 12, z) > 6 then site = { x, z } end
        end
    end
    check(site ~= nil, 'found deep water')
    if not site then ScenarioResult.done = true return end
    local function y(u) local _, py = u:GetPositionXYZ() return py end
    local function make(bp, x, z, py) return CreateUnitHPR(bp, 'ARMY_1', x, py, z, 0, 0, 0) end
    local high = make('uel0201', 420, 600, GetTerrainHeight(420, 600) + 30)
    check(math.abs(y(high) - GetTerrainHeight(420, 600)) < 0.01, 'a tank made 30 up is on the ground: ' .. y(high))
    local sx, sz = site[1], site[2]
    local pgen = make('ueb1101', sx, sz, GetSurfaceHeight(sx, sz))
    check(math.abs(y(pgen) - GetTerrainHeight(sx, sz)) < 0.01 and pgen:GetCurrentLayer() == 'Seabed', 'a generator made at the surface is on the seabed: ' .. y(pgen))
    local hover = make('ual0201', sx + 12, sz, GetTerrainHeight(sx + 12, sz))
    check(math.abs(y(hover) - GetSurfaceHeight(sx + 12, sz)) < 0.01, 'a hover tank made on the seabed is on the surface: ' .. y(hover))
    local torp = make('ueb2109', sx, sz + 8, GetTerrainHeight(sx, sz + 8))
    check(math.abs(y(torp) - GetSurfaceHeight(sx, sz + 8)) < 0.01, 'a torpedo launcher made on the seabed is on the surface: ' .. y(torp))
    note(string.format('tank %.2f pgen %.2f hover %.2f torp %.2f', y(high), y(pgen), y(hover), y(torp)))
    ScenarioResult.done = true
end)
