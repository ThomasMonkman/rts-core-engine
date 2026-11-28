-- Interface half of the far-level probe: at tick 150 it pauses, finds army
-- 1's units by blueprint (selects them all, reads them, clears the
-- selection) and, for each view, sets the world camera's LOD scale, points
-- the camera at the unit and logs `PROBE shot <view>` (run_faf.ps1 captures
-- the screen), then `PROBE done`.
-- Ours takes one view per run: with PROBE_VIEW set it takes only that view
-- and then asks for the screenshot (`__engine_screenshot_now`).
-- PROBE_ZOOM_SCALE (ours: 0.81) scales the camera's zoom; the LOD scale is
-- divided by it so the LOD distance (view depth times scale) stays the same.
--
-- A unit: blueprint, camera zoom, LOD1 scale, camera heading. The LOD1 scale
-- puts zoom times scale between the unit's two LODCutoffs as FAF rewrites
-- them (/lua/system/blueprints-lod.lua: 0.44 L + 40 and L + 40 with
-- L = 0.7 sqrt(50000 (0.4 x + 0.2 y + 0.4 z)) from the unit's size).
local units = {
    { 'url0402', 30, 10, math.pi * 0.75 },   -- cutoffs 201 / 402
    { 'uel0001', 9, 19.3, math.pi * 0.75 },  -- 122 / 225
    { 'uel0106', 7, 20, math.pi * 0.75 },    -- 101 / 178
    { 'url0107', 7, 20.5, math.pi * 0.85 },  -- 104 / 184
    { 'uel0201', 7, 20.3, math.pi * 0.85 },  -- 103 / 182
    -- (Ours looks at the ground under the gunship, so it shows above the
    -- middle of the view; far enough out to keep it on screen. Both games
    -- still draw its LOD0 at scale 3.7, against the formula; LOD1 from 4.5.)
    { 'uaa0203', 40, 3.7, math.pi * 0.85, { 4.5, 5.5, 7 } },  -- 104 / 183
}
local views = {}
for _, u in units do
    table.insert(views, { u[1] .. '-lod0', u, 1 })
    table.insert(views, { u[1] .. '-lod1', u, u[3] })
    for _, s in u[5] or {} do
        table.insert(views, { u[1] .. '-lod1-x' .. s, u, s })
    end
end
local zs = rawget(_G, 'PROBE_ZOOM_SCALE') or 1
local one = rawget(_G, 'PROBE_VIEW')

local function find()
    UISelectionByCategory('ALLUNITS', false, false, false, false)
    WaitSeconds(0.5)
    local found = {}
    for _, u in GetSelectedUnits() or {} do
        found[string.lower(u:GetBlueprint().BlueprintId)] = u
    end
    SelectUnits({})
    return found
end

local function look(found, v)
    local u = found[v[2][1]]
    if not u then
        LOG('PROBE missing ' .. v[2][1])
        return
    end
    local p = u:GetPosition()
    ConExecute('cam_SetLOD WorldCamera ' .. (v[3] / zs))
    GetCamera('WorldCamera'):SnapTo(Vector(p[1], p[2], p[3]), { v[2][4], 1.0, 0 }, v[2][2] * zs)
end

ForkThread(function()
    while GameTick() < 150 do WaitSeconds(0.1) end
    SessionRequestPause()
    WaitSeconds(1)
    local found = find()
    WaitSeconds(0.5)
    for _, v in views do
        if not one or v[1] == one then
            look(found, v)
            WaitSeconds(1.5)
            LOG('PROBE shot ' .. v[1])
            if one then rawset(_G, '__engine_screenshot_now', true) end
            WaitSeconds(4)
        end
    end
    ConExecute('cam_SetLOD WorldCamera 1')
    LOG('PROBE done')
end)
