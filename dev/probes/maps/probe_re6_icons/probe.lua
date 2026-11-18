-- Visual probe: one UNPAUSED shot of strategic icons (probe_intel_icons'
-- shots were taken paused). Army 2 (enemy, hold fire) has a few land units
-- and structures along z = 272 on Seton's Clutch; army 1 (the focus army)
-- sees them with a scout given vision 160. ui_probe.lua points the camera and
-- logs `PROBE shot <name>` while the game runs.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re6_icons 90
--   ours: --no-ai --player 1 --script dev/probes/maps/probe_re6_icons/probe.lua
--         --camera 700,272,321 --screenshot x.png --screenshot-tick 120
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local OBS, OWN = 1, 2
SetAlliance(armies[OBS], armies[OWN], 'Enemy')
SetAlliance(armies[OWN], armies[OBS], 'Enemy')
local function P(s) LOG('PROBE sim ' .. GetGameTick() .. ' ' .. s) end
local function hold(u) if u and u.SetFireState then pcall(u.SetFireState, u, 1) end end
local Z = 272
local LINE = { { 'uel0201', 640 }, { 'uel0205', 655 }, { 'uel0105', 670 }, { 'ueb2101', 725 }, { 'ueb1101', 740 }, { 'uel0201', 760 } }
for _, e in LINE do
    local u = CreateUnitHPR(e[1], armies[OWN], e[2], GetTerrainHeight(e[2], Z), Z, 0, 0, 0)
    hold(u)
    P('unit ' .. e[1] .. ' id=' .. u:GetEntityId())
end
local scout = CreateUnitHPR('uel0101', armies[OBS], 700, GetTerrainHeight(700, 300), 300, 0, 0, 0)
hold(scout)
scout:SetIntelRadius('Vision', 160)
-- a moving unit, so the shot is plainly of a running game
ForkThread(function()
    WaitTicks(5)
    IssueMove({ scout }, { 700, GetTerrainHeight(700, 240), 240 })
end)
