-- Probe: why does a factory's new engineer walk before a build it could
-- reach from the pad (probe_padbuild `fac-near`)? The site is a T1 power
-- generator 7.2 from the engineer, inside its reach of 8 (probe_buildreach).
--   attached: the order is given the tick the engineer completes (attached)
--   detached: given the first tick it is no longer attached (still on the pad)
--   placed:   an engineer created on a finished factory's centre
--   rolled:   a factory's engineer after it has rolled off to the rally point
local armies = ListArmies()
local army = armies[1]
local brain = GetArmyBrain(army)
local sx, sz = brain:GetArmyStartPos()
brain:GiveStorage('MASS', 100000)
brain:GiveStorage('ENERGY', 100000)
brain:GiveResource('MASS', 100000)
brain:GiveResource('ENERGY', 100000)
local running = 0
local names = { 'Moving', 'Building', 'Busy', 'Attached', 'Immobile', 'BlockCommandQueue' }
local function states(u)
    if not u or u:BeenDestroyed() then return 'gone' end
    local s = ''
    for _, n in names do if u:IsUnitState(n) then s = s .. n .. ',' end end
    return '[' .. s .. ']'
end

-- Orders `e` to build 7.2 away at offset (dx, dz) from where it stands, and
-- logs its states as they change until the build starts.
local function build(tag, e, f, fp, dx, dz)
    local p0 = e:GetPosition()
    local x0, z0 = p0[1], p0[3]
    local site = { x0 + dx, GetTerrainHeight(x0 + dx, z0 + dz), z0 + dz }
    local t0 = GetGameTick()
    LOG(string.format('PROBE %s order at d=%.2f,%.2f from the factory, eng %s fac %s buildable=%s', tag, x0 - fp[1], z0 - fp[3], states(e), states(f), tostring(brain:CanBuildStructureAt('ueb1101', site))))
    IssueClearCommands({ e })
    IssueBuildMobile({ e }, site, 'ueb1101', {})
    local prev, freed
    for i = 1, 200 do
        WaitTicks(1)
        local p = e:GetPosition()
        local st = states(e) .. ' fac ' .. states(f)
        if st ~= prev then
            prev = st
            LOG(string.format('PROBE %s +%d eng d=%.2f,%.2f walked=%.2f %s', tag, GetGameTick() - t0, p[1] - fp[1], p[3] - fp[3], VDist2(x0, z0, p[1], p[3]), st))
        end
        local s = e.UnitBeingBuilt
        if s then
            local q = s:GetPosition()
            LOG(string.format('PROBE %s build started +%d walked=%.2f startDist=%.2f buildDist=%.2f', tag, GetGameTick() - t0, VDist2(x0, z0, p[1], p[3]), VDist2(x0, z0, q[1], q[3]), VDist2(p[1], p[3], q[1], q[3])))
            break
        end
    end
end

local function case(tag, ox)
    running = running + 1
    local x, z = sx + ox, sz - 25
    local f = CreateUnitHPR('ueb0101', army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    WaitTicks(10)
    local fq = f:GetPosition()
    local fp = { fq[1], fq[2], fq[3] }
    if tag == 'placed' then
        local e = CreateUnitHPR('uel0105', army, fp[1], fp[2], fp[3], 0, 0, 0)
        WaitTicks(5)
        build(tag, e, f, fp, -6, 4)
        running = running - 1
        return
    end
    IssueBuildFactory({ f }, 'uel0105', 1)
    local u
    for i = 1, 400 do
        WaitTicks(1)
        u = u or f.UnitBeingBuilt
        if u and u:GetFractionComplete() >= 1 then break end
    end
    if not u then LOG('PROBE ', tag, ' no engineer') running = running - 1 return end
    if tag == 'detached' then
        for i = 1, 100 do
            if not u:IsUnitState('Attached') then break end
            WaitTicks(1)
        end
        build(tag, u, f, fp, -6, 4)
    elseif tag == 'rolled' then
        WaitTicks(100)
        -- Away from the factory, whichever side it rolled off.
        local p = u:GetPosition()
        local ax, az = p[1] - fp[1], p[3] - fp[3]
        local n = math.sqrt(ax * ax + az * az)
        build(tag, u, f, fp, 7.2 * ax / n, 7.2 * az / n)
    else
        build(tag, u, f, fp, -6, 4)
    end
    running = running - 1
end

ForkThread(function()
    WaitTicks(10)
    ForkThread(case, 'attached', -40)
    ForkThread(case, 'detached', -20)
    ForkThread(case, 'placed', 0)
    ForkThread(case, 'rolled', 20)
    WaitTicks(20)
    while running > 0 do WaitTicks(10) end
    LOG('PROBE done')
end)
