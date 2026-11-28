-- Interface half: game speed +10 (the real game's maximum; set PROBE_KEEP_SPEED to leave it), then
-- every 30 s of wall time the game tick, sim rate and frame rate (`__EngineStats` Frame FPS).
if not rawget(_G, 'PROBE_KEEP_SPEED') then SetGameSpeed(10) end
local function fps()
    for _, a in (__EngineStats and __EngineStats.Children) or {} do
        if a.Name == 'Frame' then
            for _, b in a.Children or {} do
                if b.Name == 'FPS' then return b.Value end
            end
        end
    end
end
ForkThread(function()
    while true do
        LOG(string.format('PROBE ui tick=%d wall=%.1f speed=%s simrate=%s fps=%s', GameTick(), GetSystemTimeSeconds(),
            tostring(GetGameSpeed()), tostring(GetSimRate()), tostring(fps())))
        WaitSeconds(30)
    end
end)
-- One screen capture of the default view, a little after the start (real game runner).
ForkThread(function()
    WaitSeconds(20)
    LOG('PROBE shot view')
end)
