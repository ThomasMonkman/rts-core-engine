-- Engine stats probe: __EngineStats in the interface (cursor elevation over sea and land, counts, heap) on Seton's Clutch.
-- Army 1's start units are cleared; four power generators given a wide vision radius light the views.
-- ui_probe.lua points the camera and logs `PROBE shot <name>` while the game runs.
--   real game (Windows): winprobe.sh probe_engine_stats 100
--   ours: --no-ai --player 1 --script dev/probes/maps/probe_engine_stats/probe.lua
--         --ui-lua "PROBE_VIEW=<n> PROBE_ZOOM_SCALE=0.81 <ui_probe.lua>" --screenshot x.png --screenshot-tick 160
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
for _, s in { { 330, 250 }, { 560, 250 }, { 330, 420 }, { 560, 420 } } do
    local u = CreateUnitHPR('ueb1101', armies[1], s[1], GetTerrainHeight(s[1], s[2]), s[2], 0, 0, 0)
    u:SetIntelRadius('Vision', 220)
    LOG('PROBE sim lamp ' .. s[1] .. ',' .. s[2] .. ' ground=' .. GetTerrainHeight(s[1], s[2]) .. ' surface=' .. GetSurfaceHeight(s[1], s[2]))
end
for _, s in { { 400, 330 }, { 490, 330 }, { 330, 250 } } do
    LOG('PROBE sim spot ' .. s[1] .. ',' .. s[2] .. ' ground=' .. GetTerrainHeight(s[1], s[2]) .. ' surface=' .. GetSurfaceHeight(s[1], s[2]))
end
