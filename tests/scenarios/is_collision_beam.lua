-- Scenario test (tests/scenarios.rs): IsCollisionBeam returns the beam itself
-- for a collision beam and nil for anything else (moho returns the object, not
-- a boolean; FA exe 0x75eba0).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local laser = CreateUnitHPR('dslk004', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    WaitTicks(1)
    local w = laser:GetWeaponByLabel('PhasonBeamGround')
    local beam = w and w.Beams and w.Beams[1] and w.Beams[1].Beam
    check(beam ~= nil, 'the beam weapon made a collision beam')
    check(IsCollisionBeam(beam) == beam, 'a beam is returned as itself')
    check(IsCollisionBeam(laser) == nil, 'a unit is not a beam')
    check(IsCollisionBeam(nil) == nil and IsCollisionBeam(5) == nil and IsCollisionBeam({}) == nil,
        'non-entities are not beams')
    laser:Destroy()
    WaitTicks(2)
    check(IsCollisionBeam(beam) == nil, 'a destroyed beam is not a beam')
    ScenarioResult.done = true
end)
