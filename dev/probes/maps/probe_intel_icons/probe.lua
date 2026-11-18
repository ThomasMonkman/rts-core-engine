-- Visual probe: which strategic icon a radar / sonar contact shows. Army 2
-- (enemy, hold fire) lines up one unit per icon class along z = 272 on
-- Seton's Clutch (west to east: destroyer, sub, tank, T2 AA, engineer,
-- commander, Fatboy, point defence, power generator, fighter, bomber,
-- gunship, Sparky jammer); army 1 (the focus army) sees them only on its T2
-- radar (720, 130) and T2 sonar (400, 300), no omni. Ticks (absolute):
--   100  radar / sonar only (never seen)            -> ui_probe shots radar_*
--   130  an army 1 scout at (712, 300) with vision / water vision 160
--   150  seen                                       -> shots seen_*
--   170  the scout destroyed
--   200  out of sight again                         -> shots after_*
-- The sim logs army 1's blip of each unit at 100, 150, 200.
--   real game (Windows): dev/probes/run_faf.ps1 probe_intel_icons 300
--   ours: cargo run -- --no-ai --player 1 --script dev/probes/maps/probe_intel_icons/probe.lua
--         --camera 700,272,321 --screenshot x.png --screenshot-tick 100 --speed 3
--   (ui_probe.lua's SnapTo didn't take in ours: use --camera with the view's
--   x,z and zoom / 0.81; mid 695,272,99, west 615,272,99, east 778,272,99.
--   Its logging half runs with --ui-lua "rawset(_G, 'PROBE_NO_PAUSE', true) ...")
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local OBS, OWN = 1, 2
SetAlliance(armies[OBS], armies[OWN], 'Enemy')
SetAlliance(armies[OWN], armies[OBS], 'Enemy')
for _, i in { OBS, OWN } do
    ArmyBrains[i]:GiveStorage('MASS', 50000) ArmyBrains[i]:GiveStorage('ENERGY', 500000)
end
ForkThread(function()
    while true do
        for _, i in { OBS, OWN } do
            ArmyBrains[i]:GiveResource('MASS', 50000) ArmyBrains[i]:GiveResource('ENERGY', 500000)
        end
        WaitTicks(5)
    end
end)
local function P(s) LOG('PROBE sim ' .. GetGameTick() .. ' ' .. s) end
local function hold(u) if u and u.SetFireState then pcall(u.SetFireState, u, 1) end end
local function B(f) local ok, v = pcall(f) if not ok then return 'E' end return v end
local function flag(v, c) if v == 'E' then return '?' end return v and c or '.' end

local Z = 272
local LINE = {
    { 'destroyer', 'ues0201', 585, 'water' },
    { 'sub', 'ues0203', 605, 'sub' },
    { 'tank', 'uel0201', 640 },
    { 'aa', 'uel0205', 655 },
    { 'engineer', 'uel0105', 670 },
    { 'acu', 'uel0001', 685 },
    { 'fatboy', 'uel0401', 705 },
    { 'pd', 'ueb2101', 725 },
    { 'pgen', 'ueb1101', 740 },
    { 'fighter', 'uea0102', 755, 'air' },
    { 'bomber', 'uea0103', 770, 'air' },
    { 'gunship', 'uea0203', 785, 'air' },
    { 'sparky', 'xel0209', 815 },
}
local units = {}
for i, e in LINE do
    local x = e[3]
    local y = GetTerrainHeight(x, Z)
    if e[4] == 'water' then y = GetSurfaceHeight(x, Z) end
    if e[4] == 'sub' then y = GetSurfaceHeight(x, Z) - 3 end
    if e[4] == 'air' then y = GetTerrainHeight(x, Z) + 20 end
    local ok, u = pcall(CreateUnitHPR, e[2], armies[OWN], x, y, Z, 0, 0, 0)
    if ok and u then
        hold(u)
        units[i] = u
        P('unit ' .. e[1] .. ' ' .. e[2] .. ' id=' .. u:GetEntityId())
    else
        P('unit ' .. e[1] .. ' FAILED ' .. tostring(u))
    end
end
local radar = CreateUnitHPR('ueb3201', armies[OBS], 720, GetTerrainHeight(720, 130), 130, 0, 0, 0)
local sonar = CreateUnitHPR('ueb3202', armies[OBS], 400, GetSurfaceHeight(400, 300), 300, 0, 0, 0)
P('obs radar=' .. radar:GetEntityId() .. ' sonar=' .. sonar:GetEntityId())

local function report(tag)
    for i, e in LINE do
        local u = units[i]
        local s = e[1] .. '='
        if not u or B(function() return u:BeenDestroyed() end) == true then
            s = s .. 'gone'
        else
            local b = B(function() return u:GetBlip(OBS) end)
            if type(b) == 'table' or type(b) == 'userdata' then
                s = s .. flag(B(function() return b:IsOnRadar(OBS) end), 'R') .. flag(B(function() return b:IsOnSonar(OBS) end), 's')
                    .. flag(B(function() return b:IsSeenNow(OBS) end), 'S') .. flag(B(function() return b:IsSeenEver(OBS) end), 'E')
                    .. flag(B(function() return b:IsOnOmni(OBS) end), 'O')
            else
                s = s .. 'noblip'
            end
            local p = u:GetPosition()
            s = s .. fmt('@%.1f,%.1f,%.1f', p[1], p[2], p[3])
        end
        P(tag .. ' ' .. s)
    end
end

ForkThread(function()
    local scout
    while GetGameTick() < 230 do
        local t = GetGameTick()
        if t == 98 then report('radar') end
        if t == 130 then
            scout = CreateUnitHPR('uel0101', armies[OBS], 712, GetTerrainHeight(712, 300), 300, 0, 0, 0)
            hold(scout)
            P('scout vision ' .. tostring(B(function() scout:SetIntelRadius('Vision', 160) return true end))
                .. ' water ' .. tostring(B(function() scout:SetIntelRadius('WaterVision', 160) return true end)))
        elseif t == 148 then
            report('seen')
        elseif t == 170 then
            if scout and not scout:BeenDestroyed() then scout:Destroy() end
            P('scout gone')
        elseif t == 198 then
            report('after')
        end
        WaitTicks(1)
    end
end)
