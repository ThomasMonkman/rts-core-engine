-- Interface half of probe_re6_icons: points the camera at the line of army 2
-- units and logs `PROBE shot <name>` WITHOUT pausing (run_faf.ps1 captures
-- the screen), at zooms 260 and 700; `PROBE done` at the end.
local function L(s) LOG('PROBE ui ' .. tostring(GameTick()) .. ' ' .. s) end
local views = { { 'all', 700, 272, 260 }, { 'far', 700, 272, 700 } }
local function look(v)
    GetCamera('WorldCamera'):SnapTo(Vector(v[2], 16, v[3]), { math.pi, 1.2, 0 }, v[4] * (rawget(_G, 'PROBE_ZOOM_SCALE') or 1))
end
look(views[1])
ForkThread(function()
    while GameTick() < 100 do WaitSeconds(0.05) end
    for _, v in views do
        look(v)
        WaitSeconds(1.5)
        L('paused=' .. tostring(rawget(_G, 'SessionIsPaused') and SessionIsPaused()))
        LOG('PROBE shot unpaused_' .. v[1])
        WaitSeconds(4)
    end
    L('end')
    WaitSeconds(1)
    LOG('PROBE done')
end)
