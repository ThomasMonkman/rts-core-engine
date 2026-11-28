-- Scenario test (tests/scenarios.rs): ARMY_1 / ARMY_2 have no AI; checks go
-- into ScenarioResult, which the test reads once `done` is set.
-- GetPosition() is one table per entity, rewritten by each call: copy it to keep it.
local function pos(e) local p = e:GetPosition() return Vector(p[1], p[2], p[3]) end
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function done() ScenarioResult.done = true end
local function ground(x, z) return GetTerrainHeight(x, z) end
local function rich(army)
    local b = ArmyBrains[army]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    return b
end
-- Four tanks board a T1 transport, it flies them over and unloads them on
-- land, and they drive off.
ForkThread(function()
    WaitTicks(2)
    local t = CreateUnitHPR('uea0107', 'ARMY_1', 400, 40, 600, 0, 0, 0)
    local tanks = {}
    for i = 1, 4 do table.insert(tanks, CreateUnitHPR('uel0201', 'ARMY_1', 390 + i * 3, ground(390 + i * 3, 610), 610, 0, 0, 0)) end
    WaitTicks(10)
    check(t:TransportHasSpaceFor(tanks[1]), 'room for a tank')
    local seq = {}
    local function hook(obj, name, tag)
        local orig = obj[name]
        obj[name] = function(self, ...)
            table.insert(seq, tag)
            if orig then return orig(self, ...) end
        end
    end
    hook(t, 'OnTransportOrdered', 'ordered')
    hook(t, 'OnStartTransportLoading', 'start')
    hook(t, 'OnStopTransportLoading', 'stop')
    hook(t, 'OnTransportAborted', 'aborted')
    hook(t, 'OnTransportAttach', 'attach')
    hook(tanks[1], 'OnStartTransportBeamUp', 'beam')
    hook(tanks[1], 'OnStopTransportBeamUp', 'beamed')
    -- Record the beam-up rise in ticks (moho lifts each boarder over its
    -- beam countdown, ~9 ticks): chain onto the hooked methods.
    local beam_up = tanks[1].OnStartTransportBeamUp
    tanks[1].OnStartTransportBeamUp = function(self, ...)
        self.beam_tick = GetGameTick()
        return beam_up(self, ...)
    end
    local attached = t.OnTransportAttach
    t.OnTransportAttach = function(self, bone, unit, ...)
        if unit == tanks[1] then tanks[1].attach_tick = GetGameTick() end
        return attached(self, bone, unit, ...)
    end
    IssueTransportLoad(tanks, t)
    WaitTicks(80)
    check(table.getn(t:GetCargo()) == 4, 'all four aboard, ' .. table.getn(t:GetCargo()))
    -- Idle, a transport would land (AutoLandTime), but with cargo it holds
    -- at its TransportHoverHeight instead (moho's ShouldHoverInsteadOfLand,
    -- MohoEngine.dll 0x1029df60).
    local thh = t.Blueprint.Air.TransportHoverHeight
    for _ = 1, 30 do WaitTicks(10) end
    local cruise = pos(t)
    local cruise_y = cruise[2] - ground(cruise[1], cruise[3])
    check(math.abs(cruise_y - thh) < 1.5, 'idle loaded transport holds at its TransportHoverHeight: ' .. cruise_y .. ' vs ' .. thh)
    check(t:GetCurrentLayer() == 'Air', 'in the air')
    local got = table.concat(seq, ' ')
    -- Boarding overlaps: each tank beams up on its own rise (~9 ticks) and
    -- the attaches land together, then the beams end.
    check(string.find(got, '^ordered start .*beam .*beamed.* stop$') ~= nil, 'load sequence: ' .. got)
    local beams, attaches, beached = 0, 0, 0
    for tag in string.gmatch(got, '%S+') do
        if tag == 'beam' then beams = beams + 1 elseif tag == 'attach' then attaches = attaches + 1 elseif tag == 'beamed' then beached = beached + 1 end
    end
    check(beams == 1 and attaches == 4 and beached == 1, 'one beam-up, four attaches, one beam end: ' .. got)
    -- The rise takes ~9 ticks (~0.9 s, moho's beam countdown) from beam
    -- start to attach.
    check(tanks[1].beam_tick ~= nil and tanks[1].attach_tick ~= nil, 'beam and attach ticks recorded')
    if tanks[1].beam_tick ~= nil and tanks[1].attach_tick ~= nil then
        local rise = tanks[1].attach_tick - tanks[1].beam_tick
        check(rise >= 8 and rise <= 10, 'beam-up rise is ~9 ticks, took ' .. rise)
    end
    check(not string.find(got, 'aborted', 1, true), 'not aborted: ' .. got)
    check(not t:IsUnitState('TransportLoading'), 'done loading')
    check(tanks[1]:IsUnitState('Attached') and tanks[1]:GetCurrentLayer() == 'Air', 'passengers ride attached, in the air')
    -- Cargo hangs underneath: the passenger's feet sit below the transport.
    local tp, pp = pos(t), pos(tanks[1])
    check(pp[2] < tp[2], 'cargo hangs below the transport: ' .. pp[2] .. ' vs ' .. tp[2])
    IssueTransportUnload({t}, {440, 0, 560})
    local dropped_y = nil
    for _ = 1, 400 do
        WaitTicks(1)
        if table.getn(t:GetCargo()) == 0 then
            local p = pos(t)
            dropped_y = p[2] - ground(p[1], p[3])
            break
        end
    end
    check(table.getn(t:GetCargo()) == 0, 'all unloaded')
    check(dropped_y ~= nil and math.abs(dropped_y - thh) < 1.5, 'transport lowers to TransportHoverHeight to drop: ' .. tostring(dropped_y) .. ' vs ' .. thh)
    -- Empty and idle, the transport lands.
    for _ = 1, 30 do WaitTicks(10) end
    local back = pos(t)
    local back_y = back[2] - ground(back[1], back[3])
    check(t:GetCurrentLayer() == 'Land' and back_y < 1, 'empty idle transport lands: ' .. t:GetCurrentLayer() .. ' ' .. back_y)
    local p = pos(tanks[1])
    check(math.abs(p[2] - ground(p[1], p[3])) < 0.5, 'set down on the ground')
    check(tanks[1]:GetCurrentLayer() == 'Land' and not tanks[1]:IsUnitState('Attached'), 'back on land, detached')
    IssueMove({tanks[1]}, {p[1] + 20, 0, p[3]})
    WaitTicks(50)
    local q = pos(tanks[1])
    check(VDist2(p[1], p[3], q[1], q[3]) > 5, 'drives off after landing')
    local ferried = 0
    t.OnFerryPointSet = function(self) ferried = ferried + 1 end
    IssueFerry({t}, {420, 0, 600})
    -- (As the transport starts the order: moho's ferry task.)
    WaitTicks(1)
    check(ferried == 1, 'OnFerryPointSet on a ferry order: ' .. ferried)
    -- A ferry route flies: the transport heads for its beacon (the route's
    -- first point), reads Ferrying, and waits there for units to ferry: the
    -- order stands until it's cleared (moho's ferry task loops).
    local start = pos(t)
    local arrived
    for _ = 1, 60 do
        WaitTicks(10)
        arrived = pos(t)
        if VDist2(arrived[1], arrived[3], 420, 600) < 3 then break end
    end
    check(VDist2(start[1], start[3], arrived[1], arrived[3]) > 5, 'ferry route flies')
    check(VDist2(arrived[1], arrived[3], 420, 600) < 3, 'to its beacon')
    check(not t:IsIdleState() and t:IsUnitState('Ferrying'), 'still ferrying, waiting at the beacon')
    IssueClearCommands({t})
    WaitTicks(2)
    check(t:IsUnitState('Ferrying') == false, 'not ferrying with no route')
    -- Reverse load: the transport flies out to a tank and picks it up.
    tanks[1].beam_tick, tanks[1].attach_tick = nil, nil
    __engine_IssueTransportReverseLoad({t}, tanks[1])
    -- Sample the beam-up rise: one tick after the beam starts the tank is
    -- still near the ground, and it climbs from there (a flipped ease
    -- would jump it straight to the slot).
    local rise_from, rise_early, rise_mid = nil, nil, nil
    local beam_transport_y = nil
    for _ = 1, 200 do
        WaitTicks(1)
        if tanks[1]:IsUnitState('Attached') then break end
        if tanks[1].beam_tick ~= nil then
            local y = pos(tanks[1])[2]
            if rise_from == nil then rise_from = y elseif rise_early == nil then rise_early = y end
            rise_mid = y
            if beam_transport_y == nil then
                local tp = pos(t)
                beam_transport_y = tp[2] - ground(tp[1], tp[3])
            end
        end
    end
    for _ = 1, 60 do
        WaitTicks(10)
        if tanks[1]:IsUnitState('Attached') then break end
    end
    check(tanks[1]:IsUnitState('Attached'), 'reverse load picks the unit up')
    if rise_from ~= nil and rise_early ~= nil and rise_mid ~= nil then
        check(rise_early - rise_from < (rise_mid - rise_from) / 2, 'rise starts at the ground, one tick in still low: ' .. rise_from .. ' -> ' .. rise_early .. ' -> ' .. rise_mid)
        check(rise_mid > rise_from, 'rise climbs: ' .. rise_from .. ' -> ' .. rise_mid)
    else
        check(false, 'rise sampled')
    end
    check(beam_transport_y ~= nil and math.abs(beam_transport_y - thh) < 1.5, 'transport lowers to TransportHoverHeight before the beam-up: ' .. tostring(beam_transport_y) .. ' vs ' .. thh)
    -- After the pickup, idle with cargo, it holds at TransportHoverHeight.
    for _ = 1, 30 do WaitTicks(10) end
    local after = pos(t)
    local after_y = after[2] - ground(after[1], after[3])
    check(math.abs(after_y - thh) < 1.5, 'idle transport holds at TransportHoverHeight after pickup: ' .. after_y .. ' vs ' .. thh)
    -- Unload-specific: only the named cargo leaves.
    IssueTransportUnloadSpecific({t}, categories.uel0201, {440, 0, 560})
    for _ = 1, 40 do
        WaitTicks(10)
        if table.getn(t:GetCargo()) == 0 then break end
    end
    check(table.getn(t:GetCargo()) == 0, 'unload-specific lets matching cargo off')
    done()
end)
