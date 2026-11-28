-- Map script for the kill-cause probe in the real game: armies as usual (default victory
-- condition, no sandbox), then the shared probe body (also run by our engine via --script).
local ScenarioUtils = import('/lua/sim/ScenarioUtilities.lua')

function OnPopulate()
    ScenarioUtils.InitializeArmies()
end

function OnStart(self)
    import('/maps/probe_re7_kill/probe.lua')
end
