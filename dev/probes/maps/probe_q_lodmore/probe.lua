-- Visual probe: far levels of detail (LOD1) on units whose bones are posed,
-- to check a far level follows its LOD0's bones (by name) the way the real
-- game draws it. On flat ground on Seton's Clutch, army 1:
--   url0402 Monkeylord, uel0001 UEF commander (upgrade bones hidden by its
--   script) and uel0106 Mech Marine walking east (mid-stride at the shot);
--   url0107 Mantis (its LOD1 has `Torso01` where LOD0 has `Torso`), uel0201
--   Striker and uaa0203 Specter gunship (its LOD1 has `Turret` where LOD0 has
--   `C_Turret`) made with heading 0, each shooting an unkillable army 2 power
--   generator to its east, so their turrets are turned a quarter turn.
-- The interface half (ui_probe.lua) pauses at tick 150 and looks at each
-- unit close up, at the default LOD scale (LOD0) and with `cam_SetLOD` scaled
-- to the unit's size (LOD1), logging `PROBE shot <name>` for each view
-- (run_faf.ps1 captures the screen: out\probe_q_lodmore_<name>.png).
--   real game (Windows): dev/probes/run_faf.ps1 probe_q_lodmore 180
--   ours: cargo run -- --no-ai --script dev/probes/maps/probe_q_lodmore/probe.lua
--         --ui-lua "rawset(_G, 'PROBE_ZOOM_SCALE', 0.81) rawset(_G, 'PROBE_VIEW', 'url0107-lod1') $(cat dev/probes/maps/probe_q_lodmore/ui_probe.lua)"
--         --screenshot x.png --screenshot-tick 100000
--   (with PROBE_VIEW set, the interface takes that one view and asks for the
--   screenshot itself.)
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function make(bp, army, x, z, yaw)
    return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, yaw or 0, 0)
end
local units = {}
-- Walkers, heading east across the flat ground at x 130 to 370, z 760 to 810.
for _, w in { { 'url0402', 130, 785 }, { 'uel0001', 190, 805 }, { 'uel0106', 190, 765 } } do
    local u = make(w[1], armies[1], w[2], w[3])
    IssueMove({ u }, Vector(370, GetTerrainHeight(370, w[3]), w[3]))
    table.insert(units, u)
end
-- Turrets turned towards a target to the east (flat ground at x 340 to 460,
-- z 560 to 580, far from the walkers).
for _, t in { { 'url0107', 360, 570 }, { 'uel0201', 400, 570 }, { 'uaa0203', 440, 575 } } do
    local u = make(t[1], armies[1], t[2], t[3])
    local target = make('uab1101', armies[2], t[2] + 10, t[3])
    target:SetCanTakeDamage(false)
    table.insert(units, u)
end
LOG('PROBE units created')
ForkThread(function()
    while GetGameTick() < 150 do WaitTicks(1) end
    for _, u in units do
        local p = u:GetPosition()
        LOG(string.format('PROBE at %d %s %.2f %.2f %.2f', GetGameTick(), u:GetUnitId(), p[1], p[2], p[3]))
    end
end)
