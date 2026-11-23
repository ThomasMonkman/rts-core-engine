-- Interface half of the camera zoom probe: gives the world view wheel events
-- (`ZoomScale`, as FAF's `ForwardMouseWheelInput` does) and logs the camera's
-- zoom, target zoom and focus each frame, then where the focus is held at
-- several zooms (the look-at clamp).
local cam = GetCamera('WorldCamera')
local clock = rawget(_G, 'GetSystemTimeSecondsOnlyForProfileUse') or rawget(_G, 'GetSystemTimeSeconds') or CurrentTime
local function now()
    return clock()
end
local function state()
    local f = cam:GetFocusPosition()
    return string.format('zoom=%.4f target=%.4f focus=%.3f,%.3f,%.3f', cam:GetZoom(), cam:GetTargetZoom(), f[1], f[2], f[3])
end
local function L(s) LOG('PROBE ' .. s) end
-- Logs `frames` frames after a wheel of `notches` at view fraction (fx, fy).
local function wheel(name, view, notches, fx, fy, frames)
    local x = view.Left() + view.Width() * fx
    local y = view.Top() + view.Height() * fy
    L(name .. ' wheel notches=' .. notches .. ' at=' .. string.format('%.1f,%.1f', x, y) .. ' ' .. state())
    local t0 = now()
    view:ZoomScale(x, y, notches * 120, 120)
    L(name .. ' f=0 t=0.0000 ' .. state())
    for i = 1, frames do
        WaitFrames(1)
        L(name .. ' f=' .. i .. string.format(' t=%.4f ', now() - t0) .. state())
    end
end
local function snap(x, z, zoom)
    cam:SnapTo(Vector(x, 20, z), { math.pi, 1.2, 0 }, zoom)
    cam:RevertRotation()
end

ForkThread(function()
    while GameTick() < 80 do WaitSeconds(0.1) end
    local view = import('/lua/ui/game/worldview.lua').viewLeft
    L('view ' .. string.format('%.0f,%.0f %.0fx%.0f', view.Left(), view.Top(), view.Width(), view.Height())
        .. ' maxzoom=' .. string.format('%.3f', cam:GetMaxZoom()) .. ' minzoom=' .. string.format('%.3f', cam:GetMinZoom()))
    cam:Reset()
    WaitSeconds(1.5)
    L('reset ' .. state())

    -- The ease: one notch in, one out, five in, at the view's middle.
    snap(512, 512, 200)
    WaitSeconds(2)
    wheel('in1', view, 1, 0.5, 0.5, 60)
    wheel('out1', view, -1, 0.5, 0.5, 60)
    wheel('in5', view, 5, 0.5, 0.5, 90)
    wheel('out5', view, -5, 0.5, 0.5, 90)

    -- The anchor: off-centre, in then out.
    snap(512, 512, 200)
    WaitSeconds(2)
    wheel('anchor_in', view, 2, 0.25, 0.25, 70)
    wheel('anchor_out', view, -2, 0.25, 0.25, 70)

    -- The clamp: ask for a corner at several zooms and log where it is held.
    local max = cam:GetMaxZoom()
    for _, share in { 0.05, 0.25, 0.5, 0.75, 1.0 } do
        snap(1000, 30, max * share)
        WaitSeconds(1)
        L('clamp share=' .. share .. ' asked=1000,30 ' .. state())
    end
    -- And zooming out from a corner with the wheel.
    snap(1000, 30, max * 0.05)
    WaitSeconds(1)
    wheel('corner_out', view, -20, 0.5, 0.5, 150)
    L('end')
    WaitSeconds(1)
    LOG('PROBE done')
end)
