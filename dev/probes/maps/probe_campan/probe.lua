-- Camera pan probe: how far the arrow keys move the view a frame (the keys are pressed from
-- outside the game), on Seton's Clutch with the armies' start units cleared. All of it is in
-- ui_probe.lua.
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
LOG('PROBE sim cleared')
