-- Sim half of the pause / game-speed probe: clears the armies and logs the
-- sim tick every 10 ticks; the measuring is in ui_probe.lua.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re2_ui 90
--   ours: --session --player 1 --no-ai --ticks 700 --script dev/probes/maps/probe_re2_ui/probe.lua
--         --ui-lua "$(cat dev/probes/maps/probe_re2_ui/ui_probe.lua)"
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
ForkThread(function()
    while true do
        WaitTicks(1)
        if Sync then Sync.ProbeTick = GetGameTick() end
    end
end)
