-- Map script for the Yolona nuke centre probe in the real game: armies as usual, then the
-- shared probe body (also run by our engine via --script).
local ScenarioUtils = import('/lua/sim/ScenarioUtilities.lua')

function OnPopulate()
    ScenarioUtils.InitializeArmies()
end

function OnStart(self)
    import('/maps/probe_re4_nuke/probe.lua')
end
