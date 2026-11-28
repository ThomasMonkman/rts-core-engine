-- Map script for the unit intel probe in the real game: armies as usual, then the
-- shared probe body (also run by our engine via --script).
local ScenarioUtils = import('/lua/sim/ScenarioUtilities.lua')

function OnPopulate()
    ScenarioUtils.InitializeArmies()
end

function OnStart(self)
    import('/maps/probe_unit_intel/probe.lua')
end
