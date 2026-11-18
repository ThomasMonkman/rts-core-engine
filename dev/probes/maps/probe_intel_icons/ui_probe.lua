-- Interface half of probe_intel_icons: at ticks 100 (radar / sonar only),
-- 150 (seen) and 200 (out of sight again) it pauses, logs what the
-- interface knows of the enemy units (GetUnitById over the id ranges both
-- engines use, GetRolloverInfo) and logs `PROBE shot <phase>_<view>` for
-- each view (run_faf.ps1 captures the screen), then resumes; `PROBE done`
-- at the end. Ours: with PROBE_NO_PAUSE = true it only points the camera at
-- PROBE_VIEW (e.g. 'radar_mid'; the phase part is ignored) and logs the
-- interface's view of the units at the same ticks, without pausing.
local function G(name) return rawget(_G, name) end
local function L(s) LOG('PROBE ui ' .. tostring(GameTick()) .. ' ' .. s) end
local function N(v) if type(v) == 'number' then return string.format('%.1f', v) end return tostring(v) end
-- name, x, z, zoom (ground across the screen); ground height there ~16.
local views = {
    { 'all', 700, 272, 260 },
    { 'west', 615, 272, 80 },
    { 'mid', 695, 272, 80 },
    { 'east', 778, 272, 80 },
    { 'far', 700, 272, 700 },
}
local function look(v)
    GetCamera('WorldCamera'):SnapTo(Vector(v[2], 16, v[3]), { math.pi, 1.2, 0 }, v[4] * (G('PROBE_ZOOM_SCALE') or 1))
end
local RANGES = { { 1, 48 }, { 1048577, 1048640 }, { 5300, 5500 } }
local function units(tag)
    L(tag .. ' focus=' .. tostring(G('GetFocusArmy') and GetFocusArmy()))
    local n = 0
    for _, r in RANGES do
        for id = r[1], r[2] do
            local ok, u = pcall(GetUnitById, tostring(id))
            if ok and u then
                n = n + 1
                local s = tag .. ' unit ' .. id
                local okb, bp = pcall(function() return u:GetBlueprint().BlueprintId end)
                s = s .. ' bp=' .. tostring(okb and bp)
                local oka, a = pcall(function() return u:GetArmy() end)
                s = s .. ' army=' .. tostring(oka and a)
                local okp, p = pcall(function() return u:GetPosition() end)
                if okp and p then s = s .. ' pos=' .. N(p[1]) .. ',' .. N(p[3]) end
                local okd, d = pcall(function() return u:IsDead() end)
                s = s .. ' dead=' .. tostring(okd and d)
                local okh, h = pcall(function() return u:GetHealth() end)
                s = s .. ' hp=' .. tostring(okh and N(h))
                L(s)
            end
        end
    end
    L(tag .. ' units found=' .. n)
    local okr, info = pcall(function() return GetRolloverInfo() end)
    if okr and type(info) == 'table' then
        L(tag .. ' rollover bp=' .. tostring(info.blueprintId) .. ' army=' .. tostring(info.armyIndex) .. ' id=' .. tostring(info.entityId)
            .. ' user=' .. tostring(info.userUnit ~= nil))
    else
        L(tag .. ' rollover ' .. tostring(okr and info))
    end
    local okm, m = pcall(function() return GetMouseWorldPos() end)
    if okm and m then L(tag .. ' mouse ' .. N(m[1]) .. ',' .. N(m[3])) end
end

local PHASES = { { 'radar', 100 }, { 'seen', 150 }, { 'after', 200 } }
if G('PROBE_NO_PAUSE') then
    local want = G('PROBE_VIEW') or 'radar_all'
    for _, v in views do
        if string.find(want, '_' .. v[1] .. '$') then look(v) end
    end
    ForkThread(function()
        for _, ph in PHASES do
            while GameTick() < ph[2] do WaitSeconds(0.05) end
            units(ph[1])
        end
    end)
else
    look(views[1])
    ForkThread(function()
        for _, ph in PHASES do
            while GameTick() < ph[2] do WaitSeconds(0.05) end
            SessionRequestPause()
            WaitSeconds(1)
            units(ph[1])
            for _, v in views do
                look(v)
                WaitSeconds(1.5)
                LOG('PROBE shot ' .. ph[1] .. '_' .. v[1])
                WaitSeconds(4)
            end
            SessionResume()
            WaitSeconds(0.5)
        end
        L('end')
        WaitSeconds(3)
        LOG('PROBE done')
    end)
end
