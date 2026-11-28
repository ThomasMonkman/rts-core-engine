-- Scenario test (tests/scenarios.rs): registered unit blueprints carry the
-- engine's reflected defaults, as the real game's `__blueprints` dumps show
-- (dev/probes probe_api_spot `BP.*`): a whole `Air` table on a tank, weapon
-- defaults, `AI` and `Economy` fields, never over what a blueprint sets.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    local tank, eng, plane = __blueprints['uel0201'], __blueprints['uel0105'], __blueprints['uea0102']
    check(tank.Air and tank.Air.CanFly == false and tank.Air.KLift == 1 and tank.Air.TurnSpeed == 1, 'a tank has the Air table')
    check(tank.AI.GuardReturnRadius == 50 and tank.AI.GuardFormationName == 'GuardFormation', 'AI defaults')
    check(tank.Economy.InitialRallyZ == 5 and tank.Economy.InitialRallyX == 0, 'InitialRally defaults')
    check(tank.General.UpgradesTo == '' and tank.Physics.LayerChangeOffsetHeight == -0.1, 'General / Physics defaults')
    local w = tank.Weapon[1]
    check(w.HeadingArcRange == 180 and w.MinRadius == 0 and w.TargetType == 'RULEWTT_Unit' and w.EffectiveRadius == -1, 'weapon defaults')
    check(type(eng.Weapon) == 'table' and table.getn(eng.Weapon) == 0, 'a unit without weapons has an empty Weapon table')
    check(plane.Air.CanFly == true and plane.Air.RandomMinChangeCombatStateTime == 3, 'set fields kept, the rest filled')
    check(tank.ScriptClass == '' and tank.ScriptModule == '', 'empty script fields')
    ScenarioResult.done = true
end)
