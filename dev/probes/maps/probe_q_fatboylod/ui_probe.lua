-- Interface half of the Fatboy LOD probe: at tick 60 it pauses and, for
-- each view, sets the world camera's LOD scale, points the camera and logs
-- `PROBE shot <view>` (run_faf.ps1 captures the screen), then `PROBE done`.
-- Ours takes one view per run with --screenshot: with `PROBE_NO_PAUSE = true`
-- this only sets up the view named by PROBE_VIEW.
-- A view: name, camera heading, LOD scale.
local views = {
    { 'lod0-south', math.pi, 1 },
    { 'lod0-north', 0, 1 },
    { 'lod1-south', math.pi, 12 },
    { 'lod1-north', 0, 12 },
}
local function look(v)
    ConExecute('cam_SetLOD WorldCamera ' .. v[3])
    GetCamera('WorldCamera'):SnapTo(Vector(394, 18.5, 610), { v[2], 1.0, 0 }, 30 * (rawget(_G, 'PROBE_ZOOM_SCALE') or 1))
end

if rawget(_G, 'PROBE_NO_PAUSE') then
    for _, v in views do
        if v[1] == (rawget(_G, 'PROBE_VIEW') or 'lod0-south') then look(v) end
    end
else
    look(views[1])
    ForkThread(function()
        while GameTick() < 60 do WaitSeconds(0.1) end
        SessionRequestPause()
        WaitSeconds(1)
        for _, v in views do
            look(v)
            WaitSeconds(1.5)
            LOG('PROBE shot ' .. v[1])
            WaitSeconds(4)
        end
        ConExecute('cam_SetLOD WorldCamera 1')
        LOG('PROBE done')
    end)
end
