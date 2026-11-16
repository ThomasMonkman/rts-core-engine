-- Scenario test (tests/scenarios.rs): unit states as the real game reports
-- them by task (dev/probes probe_re2_orders ST). A tank closing on its
-- attack target reads Attacking, not Moving; a tank guarding a building
-- engineer never helps build; an engineer guarding it reads GuardBusy and
-- Building without Repairing.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local brain = GetArmyBrain('ARMY_1')
    brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
    brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
    local target = make('ueb1101', 'ARMY_2', 440, 560)
    local atk = make('uel0201', 'ARMY_1', 400, 560)
    IssueAttack({ atk }, target)
    local builder = make('uel0105', 'ARMY_1', 400, 600)
    local tank = make('uel0201', 'ARMY_1', 392, 600)
    local eng = make('uel0105', 'ARMY_1', 396, 604)
    IssueBuildMobile({ builder }, { 410.5, GetTerrainHeight(410.5, 600.5), 600.5 }, 'ueb1101', {})
    IssueGuard({ tank }, builder) IssueGuard({ eng }, builder)
    local walked, movingSeen, busy, built, repairing, tankBuilt = false, false, false, false, false, false
    for i = 1, 250 do
        WaitTicks(1)
        local vx, _, vz = atk:GetVelocity()
        if vx * vx + vz * vz > 0.0001 then
            walked = true
            if atk:IsUnitState('Moving') then movingSeen = true end
        end
        if eng:IsUnitState('GuardBusy') then busy = true end
        if eng:IsUnitState('Building') then built = true end
        if eng:IsUnitState('Repairing') then repairing = true end
        if tank:IsUnitState('Building') or tank:IsUnitState('Repairing') then tankBuilt = true end
    end
    check(walked, 'the attacker walked')
    check(not movingSeen, 'without reading Moving')
    check(busy, 'the guarding engineer reads GuardBusy')
    check(built, 'and Building')
    check(not repairing, 'not Repairing')
    check(not tankBuilt, 'the guarding tank never builds')
    ScenarioResult.done = true
end)
