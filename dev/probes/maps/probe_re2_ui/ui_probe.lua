-- Interface half: pause (when SessionIsPaused turns on, when the sim tick
-- stops, what GameTime does while paused) and SetGameSpeed (sim ticks per
-- wall second at speeds 4, 2, -2).
local fmt = string.format
local function S(v) if type(v) == 'number' then return fmt('%.2f', v) end return tostring(v) end
local function waitWall(s)
    local c0 = CurrentTime()
    local f = 0
    while CurrentTime() - c0 < s do WaitFrames(1) f = f + 1 end
    return f
end
ForkThread(function()
    waitWall(3)
    LOG('PROBE ui start tick ' .. GameTick() .. ' speed ' .. S(GetGameSpeed()))
    -- tick rate at normal speed
    local t0, g0 = GameTick(), GameTime()
    waitWall(3)
    LOG(fmt('PROBE ui speed0 ticks in 3 s %d gametime +%.2f', GameTick() - t0, GameTime() - g0))
    -- pause
    local tp = GameTick()
    SessionRequestPause()
    LOG(fmt('PROBE ui pause requested; same frame paused %s tick+%d', S(SessionIsPaused()), GameTick() - tp))
    local seenPaused, lastTick, stopFrame = nil, GameTick(), nil
    for f = 1, 15 do
        WaitFrames(1)
        local t = GameTick()
        if not seenPaused and SessionIsPaused() then seenPaused = f end
        if t ~= lastTick then stopFrame = nil else stopFrame = stopFrame or f end
        lastTick = t
    end
    LOG(fmt('PROBE ui pause: paused flag at frame %s, tick +%d after 15 frames, ticks steady from frame %s', S(seenPaused), GameTick() - tp, S(stopFrame)))
    local c0, t1, g1 = CurrentTime(), GameTick(), GameTime()
    local frames = waitWall(3)
    LOG(fmt('PROBE ui paused 3 s: ticks +%d gametime +%.2f frames>30 %s paused %s', GameTick() - t1, GameTime() - g1, tostring(frames > 30), S(SessionIsPaused())))
    local tr = GameTick()
    SessionResume()
    LOG(fmt('PROBE ui resume; same frame paused %s', S(SessionIsPaused())))
    local moveFrame
    for f = 1, 30 do
        WaitFrames(1)
        if not moveFrame and GameTick() ~= tr then moveFrame = f end
    end
    LOG(fmt('PROBE ui resume: ticks moving from frame %s, paused %s', S(moveFrame), S(SessionIsPaused())))
    -- speeds
    for _, sp in { 4, 2, -2 } do
        SetGameSpeed(sp)
        waitWall(1)
        local ts, gs = GameTick(), GameTime()
        waitWall(3)
        LOG(fmt('PROBE ui SetGameSpeed(%d): GetGameSpeed %s GetSimRate %s ticks in 3 s %d gametime +%.2f', sp, S(GetGameSpeed()), S(GetSimRate()), GameTick() - ts, GameTime() - gs))
    end
    SetGameSpeed(0)
    waitWall(1)
    LOG('PROBE ui speed back ' .. S(GetGameSpeed()))
    LOG('PROBE done')
end)
