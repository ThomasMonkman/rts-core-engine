-- Map script for probe_re7_kill's variant: the same body, keeping ARMY_1's and ARMY_2's start units.
local ScenarioUtils = import('/lua/sim/ScenarioUtilities.lua')

function OnPopulate()
    ScenarioUtils.InitializeArmies()
end

function OnStart(self)
    PROBE_RE7_KEEP = true
    import('/maps/probe_re7_kill/probe.lua')
end
