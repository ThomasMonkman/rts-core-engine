-- Differential probe: the army economy tick by tick, in a format both
-- engines print identically ("PROBE ..." lines).
--   real game (Windows): dev/probes/run_faf.ps1 probe_unit_econ 150
--   ours: cargo run -- --headless --no-ai --ticks 800 --script dev/probes/maps/probe_unit_econ/probe.lua
-- One phase after another in army 1 (each makes its units and destroys them
-- at the end). Tanks stand in as producers / consumers: their rates are set
-- with SetProductionPerSecond* / SetConsumptionPerSecond*.
-- Lines: PROBE <phase> <tick in phase> M <stored> <income> <requested>
-- <usage> <trend> <ratio> E <the same> | <unit>:<mass consumed>/<energy
-- consumed ratio = GetResourceConsumed> ...
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local function indexOf(name)
    for i, a in armies do
        if a == name then return i end
    end
end
local A1, A9 = 1, indexOf('ARMY_9') or 9 -- (the real game names its ninth army differently)
local brain, brain9 = ArmyBrains[A1], ArmyBrains[A9]
local X, Z = 656, 160
local function make(bp, army, x, z)
    return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
end
local function P(text) LOG('PROBE ' .. text) end
local function num(f)
    local ok, v = pcall(f)
    if not ok then return 'ERR' end
    if type(v) ~= 'number' then return tostring(v) end
    return fmt('%.3f', v)
end
local function side(b, kind)
    return fmt('%s %s %s %s %s %s %s', string.sub(kind, 1, 1),
        num(function() return b:GetEconomyStored(kind) end), num(function() return b:GetEconomyIncome(kind) end),
        num(function() return b:GetEconomyRequested(kind) end), num(function() return b:GetEconomyUsage(kind) end),
        num(function() return b:GetEconomyTrend(kind) end), num(function() return b:GetEconomyStoredRatio(kind) end))
end
local function line(phase, t, units, b)
    b = b or brain
    local s = fmt('%s %d %s %s |', phase, t, side(b, 'MASS'), side(b, 'ENERGY'))
    for name, u in units or {} do
        if u:BeenDestroyed() then
            s = s .. ' ' .. name .. ':gone'
        else
            s = s .. ' ' .. name .. ':' .. num(function() return u:GetResourceConsumed() end)
        end
    end
    P(s)
end
local function run(phase, n, units, each)
    for t = 1, n do
        WaitTicks(1)
        line(phase, t, units)
        if each then each(t) end
    end
end
local function setStored(kind, amount)
    local cur = brain:GetEconomyStored(kind)
    if cur > amount then brain:TakeResource(kind, cur - amount) else brain:GiveResource(kind, amount - cur) end
end
local function clear(units)
    for _, u in units do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function producer(x, m, e)
    local u = make('uel0201', A1, x, Z)
    u:SetProductionPerSecondMass(m) u:SetProductionPerSecondEnergy(e)
    u:SetProductionActive(true)
    return u
end
local function consumer(x, m, e)
    local u = make('uel0201', A1, x, Z)
    u:SetConsumptionPerSecondMass(m) u:SetConsumptionPerSecondEnergy(e)
    u:SetConsumptionActive(true)
    return u
end

ForkThread(function()
    -- A: no units at all.
    line('A', 0)
    run('A', 3)
    brain:GiveResource('MASS', 100) brain:GiveResource('ENERGY', 100)
    line('A-give', 0)
    run('A-give', 3)
    P('A take returns ' .. num(function() return brain:TakeResource('MASS', 10) end))
    line('A-take', 0)
    run('A-take', 2)
    -- B: storage buildings (2 mass, 2 energy), kept for every later phase.
    local stores = { make('ueb1105', A1, X, Z + 10), make('ueb1105', A1, X + 4, Z + 10), make('ueb1106', A1, X + 8, Z + 10), make('ueb1106', A1, X + 12, Z + 10) }
    line('B', 0)
    run('B', 2)
    brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)
    run('B-full', 3)
    P('B take over ' .. num(function() return brain:TakeResource('MASS', 999999) end))
    line('B-take', 0)
    run('B-take', 2)
    brain:GiveResource('MASS', 1000) brain:GiveResource('ENERGY', 5000)
    WaitTicks(2)
    -- C: production.
    local p = producer(X, 7, 30)
    line('C', 0)
    run('C', 5)
    P('C getters ' .. num(function() return p:GetProductionPerSecondMass() end) .. ' ' .. num(function() return p:GetProductionPerSecondEnergy() end))
    p:SetProductionActive(false)
    run('C-off', 3)
    p:SetProductionActive(true)
    p:SetProductionPerSecondMass(-4)
    run('C-neg', 3)
    p:SetProductionPerSecondMass(7)
    p:SetPaused(true)
    run('C-paused', 3)
    p:SetPaused(false)
    clear({ p })
    run('C-gone', 2)
    -- D: consumption with plenty stored.
    local c = consumer(X, 5, 50)
    line('D', 0, { c = c })
    run('D', 5, { c = c })
    P('D getters ' .. num(function() return c:GetConsumptionPerSecondMass() end) .. ' ' .. num(function() return c:GetConsumptionPerSecondEnergy() end))
    c:SetConsumptionActive(false)
    run('D-off', 3, { c = c })
    c:SetConsumptionActive(true)
    c:SetPaused(true)
    run('D-paused', 3, { c = c })
    c:SetPaused(false)
    c:SetConsumptionPerSecondEnergy(-20)
    run('D-neg', 3, { c = c })
    clear({ c })
    -- E: energy stalled. Income 20/s; c1 wants 50 E/s, c2 30 E/s and 4 M/s.
    setStored('ENERGY', 0) setStored('MASS', 1000)
    WaitTicks(2)
    local u = { p = producer(X, 0, 20), c1 = consumer(X + 4, 0, 50), c2 = consumer(X + 8, 4, 30) }
    line('E', 0, u)
    run('E', 8, u)
    clear(u)
    -- F: mass stalled. Income 2/s; c1 wants 5 M/s, c2 3 M/s and 30 E/s.
    setStored('MASS', 0) setStored('ENERGY', 5000)
    WaitTicks(2)
    u = { p = producer(X, 2, 0), c1 = consumer(X + 4, 5, 0), c2 = consumer(X + 8, 3, 30) }
    line('F', 0, u)
    run('F', 8, u)
    clear(u)
    -- G: both stalled, differently (mass 2 of 8 = 25%, energy 40 of 80 = 50%).
    setStored('MASS', 0) setStored('ENERGY', 0)
    WaitTicks(2)
    u = { p = producer(X, 2, 40), c1 = consumer(X + 4, 5, 50), c2 = consumer(X + 8, 3, 30) }
    line('G', 0, u)
    run('G', 8, u)
    clear(u)
    -- H: a little stored, no income: 12 energy against 50/s, 1.2 mass against 5/s.
    setStored('MASS', 1.2) setStored('ENERGY', 12)
    WaitTicks(2)
    u = { c1 = consumer(X + 4, 5, 50) }
    line('H', 0, u)
    run('H', 6, u)
    clear(u)
    -- I: income over a full storage (overflow), then sharing to an ally.
    setStored('MASS', 999999) setStored('ENERGY', 9999999)
    WaitTicks(2)
    u = { p = producer(X, 50, 500) }
    run('I', 3, u)
    local s9 = { make('ueb1105', A9, X, Z + 30), make('ueb1106', A9, X + 8, Z + 30) }
    WaitTicks(2)
    line('I-ally-before', 0, nil, brain9)
    SetAlliance(A1, A9, 'Ally') SetAlliance(A9, A1, 'Ally')
    for t = 1, 4 do
        WaitTicks(1)
        line('I-allied', t, nil, brain9)
    end
    P('I SetResourceSharing ' .. num(function() brain:SetResourceSharing(true) brain9:SetResourceSharing(true) return 1 end))
    for t = 1, 6 do
        WaitTicks(1)
        line('I-sharing', t, nil, brain9)
    end
    line('I-giver', 0, u)
    P('I SetResourceSharing off ' .. num(function() brain:SetResourceSharing(false) return 1 end))
    for t = 1, 3 do
        WaitTicks(1)
        line('I-sharing-off', t, nil, brain9)
    end
    clear(u) clear(s9)
    SetAlliance(A1, A9, 'Enemy') SetAlliance(A9, A1, 'Enemy')
    -- J: economy events.
    setStored('MASS', 1000) setStored('ENERGY', 5000)
    WaitTicks(2)
    local host = make('uel0201', A1, X, Z)
    local ev
    P('J create ' .. num(function() ev = CreateEconomyEvent(host, 100, 20, 2) return type(ev) end))
    line('J', 0, { h = host })
    for t = 1, 24 do
        WaitTicks(1)
        line('J', t, { h = host })
        if t <= 22 then P('J done ' .. num(function() return EconomyEventIsDone(ev) end)) end
    end
    P('J create0 ' .. num(function() ev = CreateEconomyEvent(host, 100, 20, 0) return type(ev) end))
    run('J0', 3, { h = host })
    P('J0 done ' .. num(function() return EconomyEventIsDone(ev) end))
    P('J create-remove ' .. num(function() ev = CreateEconomyEvent(host, 100, 20, 2) return type(ev) end))
    run('Jr', 3, { h = host })
    P('Jr remove ' .. num(function() RemoveEconomyEvent(host, ev) return 1 end))
    run('Jr-after', 3, { h = host })
    -- stalled event: 100 energy over 1 s with nothing stored
    setStored('ENERGY', 0)
    WaitTicks(2)
    P('J create-stalled ' .. num(function() ev = CreateEconomyEvent(host, 100, 0, 1) return type(ev) end))
    for t = 1, 14 do
        WaitTicks(1)
        if t == 6 then brain:GiveResource('ENERGY', 60) end
        if t == 10 then brain:GiveResource('ENERGY', 600) end
        line('Js', t, { h = host })
        P('Js done ' .. num(function() return EconomyEventIsDone(ev) end))
    end
    clear({ host })
    -- K: a T1 mass fabricator and a T1 generator / extractor as made.
    setStored('MASS', 500) setStored('ENERGY', 5000)
    WaitTicks(2)
    u = { fab = make('ueb1104', A1, X, Z), pgen = make('ueb1101', A1, X + 6, Z + 20), mex = make('ueb1103', A1, X + 12, Z + 20) }
    line('K', 0, u)
    run('K', 6, u)
    for name, unit in u do
        P(fmt('K %s prod %s %s cons %s %s', name, num(function() return unit:GetProductionPerSecondMass() end), num(function() return unit:GetProductionPerSecondEnergy() end),
            num(function() return unit:GetConsumptionPerSecondMass() end), num(function() return unit:GetConsumptionPerSecondEnergy() end)))
    end
    clear({ u.pgen })
    setStored('ENERGY', 0)
    run('K-stall', 8, u)
    clear(u)
    -- L: a factory building a tank: full economy.
    setStored('MASS', 1000) setStored('ENERGY', 5000)
    WaitTicks(2)
    local fac = make('ueb0101', A1, X + 40, Z)
    WaitTicks(5)
    local function build(phase, ticks, untilDone, each)
        IssueBuildFactory({ fac }, 'uel0201', 1)
        local seen, done = false, false
        for t = 1, ticks do
            WaitTicks(1)
            local f = fac:GetFocusUnit()
            if f then seen = true end
            if each then each(t, f) end
            if seen and not f and not done then
                done = true
                P(fmt('%s finished at %d', phase, t))
                line(phase, t, { fac = fac })
                if untilDone then break end
            end
            if t <= 8 or math.mod(t, 10) == 0 then
                line(phase, t, { fac = fac })
                P(fmt('%s %d frac %s hp %s work %s cons %s %s queue %d building %s', phase, t, f and num(function() return f:GetFractionComplete() end) or '-',
                    f and num(function() return f:GetHealth() end) or '-', num(function() return fac:GetWorkProgress() end),
                    num(function() return fac:GetConsumptionPerSecondMass() end), num(function() return fac:GetConsumptionPerSecondEnergy() end),
                    table.getn(fac:GetCommandQueue()), tostring(fac:IsUnitState('Building'))))
            end
        end
        IssueClearCommands({ fac })
        local f = fac:GetFocusUnit()
        if f and not f:BeenDestroyed() then f:Destroy() end
        WaitTicks(3)
        for _, t in brain:GetListOfUnits(categories.uel0201, false) do t:Destroy() end
        WaitTicks(2)
    end
    build('L', 220, true)
    -- M: the same with energy stalled at half the factory's drain.
    setStored('ENERGY', 0)
    local half = producer(X + 20, 0, 0)
    build('M', 50, false, function(t, f)
        if t == 3 then
            half:SetProductionPerSecondEnergy(fac:GetConsumptionPerSecondEnergy() / 2)
            P('M half income ' .. num(function() return fac:GetConsumptionPerSecondEnergy() / 2 end))
        end
    end)
    clear({ half })
    -- N: full economy, two engineers assisting from tick 1; paused 30..40.
    setStored('MASS', 1000) setStored('ENERGY', 5000)
    local e1, e2 = make('uel0105', A1, X + 34, Z + 6), make('uel0105', A1, X + 46, Z + 6)
    IssueGuard({ e1, e2 }, fac)
    build('N', 60, false, function(t, f)
        if t == 30 then fac:SetPaused(true) end
        if t == 40 then fac:SetPaused(false) end
        if t >= 28 and t <= 44 or t == 12 then
            line('N+', t, { fac = fac, e1 = e1, e2 = e2 })
            P(fmt('N+ %d frac %s e1 focus %s cons %s %s', t, f and num(function() return f:GetFractionComplete() end) or '-', tostring(e1:GetFocusUnit() ~= nil),
                num(function() return e1:GetConsumptionPerSecondMass() end), num(function() return e1:GetConsumptionPerSecondEnergy() end)))
        end
    end)
    WaitTicks(30)
    LOG('PROBE done')
end)
