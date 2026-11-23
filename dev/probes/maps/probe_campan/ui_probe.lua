-- Interface half of the camera pan probe: holds the camera at a zoom and logs
-- every frame on which its focus moved (arrow keys held from outside the
-- game, plain and with Ctrl), putting it back in the middle between holds.
local cam = GetCamera('WorldCamera')
local clock = rawget(_G, 'GetSystemTimeSecondsOnlyForProfileUse') or rawget(_G, 'GetSystemTimeSeconds') or CurrentTime
local function snap(zoom)
    cam:SnapTo(Vector(512, 20, 512), { math.pi, 1.2, 0 }, zoom)
    cam:RevertRotation()
end
ForkThread(function()
    while GameTick() < 80 do WaitSeconds(0.1) end
    local view = import('/lua/ui/game/worldview.lua').viewLeft
    local zooms = { 100, 400 }
    local zi, still, frame = 1, 0, 0
    snap(zooms[zi])
    WaitSeconds(1)
    LOG('PROBE view ' .. string.format('%.0fx%.0f', view.Width(), view.Height()) .. ' maxzoom=' .. string.format('%.3f', cam:GetMaxZoom()))
    local t0 = clock()
    local last = cam:GetFocusPosition()
    local lastT = t0
    while clock() - t0 < 75 do
        WaitFrames(1)
        frame = frame + 1
        local f, t = cam:GetFocusPosition(), clock()
        local dx, dz = f[1] - last[1], f[3] - last[3]
        if math.abs(dx) > 0.001 or math.abs(dz) > 0.02 then
            LOG('PROBE move f=' .. frame .. string.format(' t=%.4f dt=%.4f zoom=%.3f dx=%.4f dz=%.4f x=%.3f z=%.3f', t - t0, t - lastT, cam:GetZoom(), dx, dz, f[1], f[3]))
            still = 0
        else
            still = still + 1
            if still == 40 and math.abs(f[1] - 512) > 1 then
                zi = math.mod(zi, table.getn(zooms)) + 1
                snap(zooms[zi])
                WaitSeconds(0.5)
                f = cam:GetFocusPosition()
                LOG('PROBE recentre zoom=' .. zooms[zi])
            end
        end
        last, lastT = f, clock()
    end
    LOG('PROBE done')
end)
