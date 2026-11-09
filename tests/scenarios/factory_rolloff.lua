-- Scenario test (tests/scenarios.rs): factory roll-off (`IssueMoveOffFactory`).
-- A land factory's tank, released from the raised build pad, drives out (its
-- rally point is its own spot, so FAF picks the +3 x `RollOffPoints` entry,
-- and the goal is the first open cell there, just off the footprint, as in
-- the real game: dev/probes probe_rolloff) and down to the ground without
-- dropping; the factory then builds the next one. An air
-- factory's scout takes off by itself and climbs; another's transport (a
-- hover aircraft) drifts off the pad so the next one isn't built inside it.
-- A factory with a rally point to its east rolls its tank down the east side
-- ramp (FAF's nearest `RollOffPoints` entry, +3 x), not out of the front.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, x, z) return CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
    local land = make('ueb0101', 400, 600)
    local air = make('ueb0102', 430, 600)
    local pad = make('ueb0102', 460, 600)
    IssueBuildFactory({ land }, 'uel0201', 2)
    local side = make('ueb0101', 400, 660)
    IssueFactoryRallyPoint({ side }, { 440, GetTerrainHeight(440, 660), 660 })
    IssueBuildFactory({ side }, 'uel0201', 1)
    local side_tank, side_exit
    IssueBuildFactory({ air }, 'uea0101', 1)
    IssueBuildFactory({ pad }, 'uea0107', 1)
    local tank, scout, transport
    local worst_drop, last_y = 0, nil
    -- The scout's highest climb (idle, it lands again later).
    local scout_top = 0
    for _ = 1, 600 do
        WaitTicks(1)
        for _, u in b:GetListOfUnits(categories.MOBILE, false) or {} do
            if u:GetFractionComplete() >= 1 then
                if u.UnitId == 'uel0201' and not tank then tank = u end
                if u.UnitId == 'uea0101' and not scout then scout = u end
                if u.UnitId == 'uea0107' and not transport then transport = u end
            end
        end
        if not side_tank then
            for _, u in b:GetListOfUnits(categories.uel0201, false) or {} do
                local q = u:GetPosition()
                if math.abs(q[3] - 660) < 8 and math.abs(q[1] - 400) < 8 and u:GetFractionComplete() >= 1 then side_tank = u end
            end
        end
        if side_tank and not side_exit and not side_tank.Dead then
            local q = side_tank:GetPosition()
            -- Where it crosses the factory's edge (the skirt is 8 across).
            if math.max(math.abs(q[1] - 400), math.abs(q[3] - 660)) > 4.5 then side_exit = { q[1] - 400, q[3] - 660 } end
        end
        if tank and not tank.Dead then
            local y = tank:GetPosition()[2]
            if last_y then worst_drop = math.max(worst_drop, last_y - y) end
            last_y = y
        end
        local tanks = 0
        for _, u in b:GetListOfUnits(categories.uel0201, false) or {} do
            if u:GetFractionComplete() >= 1 then tanks = tanks + 1 end
        end
        if scout then
            local sp = scout:GetPosition()
            scout_top = math.max(scout_top, sp[2] - GetTerrainHeight(sp[1], sp[3]))
        end
        local transport_off = transport and math.max(math.abs(transport:GetPosition()[1] - 460), math.abs(transport:GetPosition()[3] - 600)) > 6
        if tanks >= 2 and side_exit and transport_off and scout_top > 0.5 then break end
    end
    check(tank ~= nil, 'the land factory finished a tank')
    if tank then
        local p = tank:GetPosition()
        note(string.format('tank at %.1f, %.1f (factory 400, 600), largest one-tick drop %.2f', p[1], p[3], worst_drop))
        check(math.abs(p[1] - 400) > 2.5 or math.abs(p[3] - 600) > 2.5, 'the tank rolled off the factory')
        check(worst_drop < 0.3, 'the tank came down off the pad gradually')
        check(math.abs(p[2] - GetTerrainHeight(p[1], p[3])) < 0.05, 'and ended on the ground')
    end
    local tanks = 0
    for _, u in b:GetListOfUnits(categories.uel0201, false) or {} do
        if u:GetFractionComplete() >= 1 then tanks = tanks + 1 end
    end
    check(tanks >= 2, 'the factory went on to build the second tank')
    check(side_exit ~= nil, 'the rallied factory\'s tank left it')
    if side_exit then
        note(string.format('rallied tank left the factory at %.1f, %.1f from its centre', side_exit[1], side_exit[2]))
        check(side_exit[1] > 4 and math.abs(side_exit[2]) < 2, 'down the east side towards the rally point')
    end
    check(scout ~= nil, 'the air factory finished a scout')
    if scout then
        -- (Idle off the pad, it lands again after `AutoLandTime`: how high
        -- it climbs first isn't checked against the real game yet.)
        local sp = scout:GetPosition()
        note(string.format('scout climbed to %.1f above the ground, now at %.1f %.1f', scout_top, sp[1], sp[3]))
        check(scout_top > 0.5, 'the scout took off by itself')
    end
    check(transport ~= nil, 'the third air factory finished a transport')
    if transport then
        local p = transport:GetPosition()
        local off = math.max(math.abs(p[1] - 460), math.abs(p[3] - 600))
        note(string.format('transport %.1f off the pad centre, %.1f up', off, p[2] - GetTerrainHeight(p[1], p[3])))
        check(off > 6, 'the transport moved off the pad')
    end
    ScenarioResult.done = true
end)
