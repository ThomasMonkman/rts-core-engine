-- Development scenario: a Monkeylord (army 1) facing a spread of enemy land
-- units (army 2): tanks, fast light bots and a few heavies at mixed ranges,
-- so its beam has to sweep between targets.
-- Run: cargo run -- --script dev/scenes/visual/monkeylord_beam.lua --no-ai --player 1 --camera 512,512,90
local w, h = GetMapSize()
local cx, cz = w / 2, h / 2
local function spawn(bp, army, x, z, heading)
    return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, heading, 0, 0)
end
spawn('url0402', 1, cx - 15, cz, math.pi / 2)
local kinds = { 'uel0201', 'uel0106', 'uel0202', 'xel0305', 'uel0101' }
local i = 0
for row = -4, 4 do
    for col = 0, 5 do
        i = i + 1
        spawn(kinds[(i % table.getn(kinds)) + 1], 2, cx + col * 6, cz + row * 6, -math.pi / 2)
    end
end
