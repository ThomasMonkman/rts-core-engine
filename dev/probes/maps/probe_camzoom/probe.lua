-- Camera zoom probe: the wheel zoom's ease, its pull toward the cursor and the look-at clamp, on
-- Seton's Clutch with the armies' start units cleared. All of it is in ui_probe.lua.
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
LOG('PROBE sim cleared')
