-- Interface half of probe_re8_water: points the camera at stretches of water and logs
-- `PROBE shot <name>` without pausing (the runner captures the screen); `PROBE done` at the end.
-- PROBE_VIEW (a number) holds one view only, for a single screenshot in ours.
local function L(s) LOG('PROBE ui ' .. tostring(GameTick()) .. ' ' .. s) end
-- name, x, z, zoom, pitch
local views = {
    { 'sea', 400, 330, 90, 1.2 },
    { 'shore', 490, 330, 90, 1.2 },
    { 'low', 470, 330, 70, 0.6 },
    { 'mid', 450, 330, 260, 1.2 },
    { 'far', 512, 512, 900, 1.5 },
}
local function look(v)
    GetCamera('WorldCamera'):SnapTo(Vector(v[2], 17.5, v[3]), { math.pi, v[5], 0 }, v[4] * (rawget(_G, 'PROBE_ZOOM_SCALE') or 1))
end
local only = rawget(_G, 'PROBE_VIEW')
if only then
    look(views[only])
    ForkThread(function() while true do look(views[only]) WaitSeconds(0.5) end end)
    return
end
look(views[1])
ForkThread(function()
    while GameTick() < 100 do WaitSeconds(0.05) end
    for _, v in views do
        look(v)
        WaitSeconds(1.5)
        LOG('PROBE shot ' .. v[1])
        WaitSeconds(4)
    end
    L('end')
    WaitSeconds(1)
    LOG('PROBE done')
end)
