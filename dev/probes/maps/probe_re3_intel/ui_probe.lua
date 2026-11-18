-- Interface half of probe_re3_intel: at ticks 60 (radar only), 175 (after
-- vision) and 250 (after the radar is gone) it pauses, logs rollover /
-- the user layer's view, and logs `PROBE shot <phase>_<view>` per view, then
-- resumes. Ours: PROBE_NO_PAUSE only logs (shots come from --camera runs).
local function G(name) return rawget(_G, name) end
local function L(s) LOG('PROBE ui ' .. tostring(GameTick()) .. ' ' .. s) end
-- name, x, z, zoom
local VIEWS = {
    radar = { { 'all', 600, 320, 450 }, { 'air', 650, 432, 120 }, { 'naval', 530, 210, 200 } },
    after = { { 'fatboy', 660, 358, 80 }, { 'base', 640, 345, 200 } },
    lost = { { 'base', 640, 345, 200 }, { 'far', 620, 300, 700 } },
}
local PHASES = { { 'radar', 60 }, { 'after', 175 }, { 'lost', 250 } }
local function look(v)
    GetCamera('WorldCamera'):SnapTo(Vector(v[2], 19, v[3]), { math.pi, 1.2, 0 }, v[4] * (G('PROBE_ZOOM_SCALE') or 1))
end
local function info(tag)
    local okr, r = pcall(function() return GetRolloverInfo() end)
    L(tag .. ' rollover ' .. ((okr and type(r) == 'table') and (tostring(r.blueprintId) .. ' army ' .. tostring(r.armyIndex)) or tostring(okr and r)))
    local okm, mm = pcall(function()
        local m = import('/lua/ui/game/minimap.lua')
        return m and (m.controls and m.controls.displayGroup and 'minimap controls' or 'minimap module')
    end)
    L(tag .. ' minimap ' .. tostring(okm and mm))
end
ForkThread(function()
    for _, ph in PHASES do
        while GameTick() < ph[2] do WaitSeconds(0.05) end
        if not G('PROBE_NO_PAUSE') then SessionRequestPause() WaitSeconds(1) end
        info(ph[1])
        if not G('PROBE_NO_PAUSE') then
            for _, v in VIEWS[ph[1]] do
                look(v)
                WaitSeconds(1.5)
                LOG('PROBE shot ' .. ph[1] .. '_' .. v[1])
                WaitSeconds(4)
            end
            SessionResume()
            WaitSeconds(0.5)
        end
    end
    while GameTick() < 335 do WaitSeconds(0.1) end
    WaitSeconds(2)
    LOG('PROBE done')
end)
