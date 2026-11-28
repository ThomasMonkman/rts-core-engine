-- Visual probe: the Fatboy's (uel0401) two mesh LODs. One Fatboy on hold
-- fire on flat ground on Seton's Clutch (x 394, z 610), turned an eighth of
-- a turn. The interface half (ui_probe.lua) looks at it from close by, from
-- the south and the north, first at the default LOD scale (LOD 0) and then
-- with `cam_SetLOD WorldCamera 12` (LOD 1), and logs `PROBE shot <name>` for
-- each view (run_faf.ps1 captures the screen: out\probe_q_fatboylod_<name>.png).
--   real game (Windows): dev/probes/run_faf.ps1 probe_q_fatboylod 120
--   ours: cargo run -- --no-ai --script dev/probes/maps/probe_q_fatboylod/probe.lua
--         --ui-lua "rawset(_G, 'PROBE_NO_PAUSE', true) rawset(_G, 'PROBE_VIEW', 'lod1-south') $(cat dev/probes/maps/probe_q_fatboylod/ui_probe.lua)"
--         --screenshot x.png --screenshot-tick 100
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local u = CreateUnitHPR('uel0401', armies[1], 394, GetTerrainHeight(394, 610), 610, 0, math.pi / 4, 0)
u:SetFireState(1)
LOG('PROBE fatboy created')
