-- Scenario test (tests/scenarios.rs): ARMY_1 / ARMY_2 have no AI; checks go
-- into ScenarioResult, which the test reads once `done` is set.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function done() ScenarioResult.done = true end
local function rich(army)
    local b = ArmyBrains[army]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    return b
end
local function height(u)
    local p = u:GetPosition()
    return p[2] - GetSurfaceHeight(p[1], p[3])
end
-- Idle hovering aircraft land (`AutoLandTime`): a transport off an air
-- factory with no rally point lifts off the pad, then lands beside the
-- factory; a gunship moved somewhere lands there and takes off on its next
-- order; an idle interceptor slows onto its point and lands too (the real
-- game's fighters do, dev/probes probe_air).
ForkThread(function()
    WaitTicks(2)
    rich(1)
    local fx, fz = 400, 600
    local factory = CreateUnitHPR('ueb0102', 'ARMY_1', fx, GetTerrainHeight(fx, fz), fz, 0, 0, 0)
    factory:SetBuildRate(400)
    IssueBuildFactory({ factory }, 'uea0107', 1)
    local gunship = CreateUnitHPR('uea0203', 'ARMY_1', 410, 30, 625, 0, 0, 0)
    IssueMove({ gunship }, { 425, 0, 625 })
    local plane = CreateUnitHPR('uea0102', 'ARMY_1', 420, 40, 570, 0, 0, 0)

    local transport
    for _ = 1, 600 do
        WaitTicks(1)
        for _, u in ArmyBrains[1]:GetListOfUnits(categories.TRANSPORTATION, false) or {} do
            if u.UnitId == 'uea0107' and u:GetFractionComplete() >= 1 and not u:IsUnitState('Attached') then transport = u end
        end
        if transport then break end
    end
    check(transport, 'the factory built the transport')
    if not transport then done() return end
    local top, landed = 0, false
    for _ = 1, 300 do
        WaitTicks(1)
        top = math.max(top, height(transport))
        if transport:GetCurrentLayer() == 'Land' then landed = true break end
    end
    local p = transport:GetPosition()
    note(string.format('transport rose to %.1f, landed at %.1f %.1f (factory %d %d)', top, p[1], p[3], fx, fz))
    check(top > 3, 'the transport lifted off the pad first')
    check(landed, 'then landed')
    check(height(transport) < 1, 'on the ground')
    check(math.abs(p[1] - fx) > 4 or math.abs(p[3] - fz) > 4, 'off the factory')
    check(VDist2(p[1], p[3], fx, fz) < 25, 'beside it')
    check(transport:GetCommandQueue()[1] == nil, 'with no orders of its own')

    -- It reads `MovingDown` as it comes down, cleared on touching down; a
    -- plain take-off for a move reads no `MovingUp` (dev/probes
    -- probe_re3_mov MS).
    local down = false
    for _ = 1, 300 do
        if gunship:GetCurrentLayer() ~= 'Air' then break end
        down = down or gunship:IsUnitState('MovingDown')
        WaitTicks(1)
    end
    check(gunship:GetCurrentLayer() == 'Land', 'the idle gunship landed')
    check(down, 'MovingDown while landing')
    check(not gunship:IsUnitState('MovingDown'), 'not once down')
    local g = gunship:GetPosition()
    check(VDist2(g[1], g[3], 425, 625) < 3, 'where it was sent')
    IssueMove({ gunship }, { 425, 0, 580 })
    local up = false
    for _ = 1, 5 do
        WaitTicks(1)
        up = up or gunship:IsUnitState('MovingUp')
    end
    check(not up, 'no MovingUp on a plain take-off')
    check(gunship:GetCurrentLayer() == 'Air', 'an order takes it off again')
    WaitTicks(30)
    check(height(gunship) > 3, 'and up')

    for _ = 1, 300 do
        if plane:GetCurrentLayer() ~= 'Air' then break end
        WaitTicks(1)
    end
    check(plane:GetCurrentLayer() == 'Land', 'the idle interceptor lands too')
    done()
end)
