-- Probe: how close must an engineer be to start a mobile build without
-- moving, and where does it stop when it has to move? A T1 engineer
-- (MaxBuildDistance 5) stands D from the site of a wall (1x1), a T1 power
-- generator (2x2) and a T1 land factory (5x5, skirt 8).
local armies = ListArmies()
local army = armies[1]
local brain = GetArmyBrain(army)
local sx, sz = brain:GetArmyStartPos()
brain:GiveStorage('MASS', 100000)
brain:GiveStorage('ENERGY', 100000)
brain:GiveResource('MASS', 100000)
brain:GiveResource('ENERGY', 100000)
local running = 0

local function lane(bp, ox, from, to, oz)
    running = running + 1
    local x, z = sx + ox, sz + (oz or 34)
    if not oz then
        -- A large structure: the first spot of a grid that takes it.
        local found
        for dz = 52, 76, 4 do
            for dx = -60, 60, 4 do
                if not found and brain:CanBuildStructureAt(bp, { sx + dx, GetTerrainHeight(sx + dx, sz + dz), sz + dz }) then
                    found = true
                    x, z = sx + dx, sz + dz
                end
            end
        end
        LOG('PROBE ', bp, ' spot ', x - sx, ',', z - sz)
    end
    local site = { x, GetTerrainHeight(x, z), z }
    LOG('PROBE ', bp, ' site buildable=', tostring(brain:CanBuildStructureAt(bp, site)))
    local d = from
    while d <= to + 0.01 do
        local ez = z - d
        local e = CreateUnitHPR('uel0105', army, x, GetTerrainHeight(x, ez), ez, 0, 0, 0)
        WaitTicks(3)
        local p0 = e:GetPosition()
        local x0, z0 = p0[1], p0[3]
        IssueBuildMobile({ e }, site, bp, {})
        local s, t, moved = nil, 0, false
        for i = 1, 250 do
            WaitTicks(1)
            t = i
            if e:IsUnitState('Moving') then moved = true end
            s = e.UnitBeingBuilt
            if s then break end
        end
        local p = e:GetPosition()
        if s then
            local q = s:GetPosition()
            LOG(string.format('PROBE %s D=%.1f started after %d ticks moved=%s walked=%.2f startDist=%.2f buildDist=%.2f structure=%.1f,%.1f eng=%.2f,%.2f',
                bp, d, t, tostring(moved), VDist2(x0, z0, p[1], p[3]), VDist2(x0, z0, q[1], q[3]), VDist2(p[1], p[3], q[1], q[3]), q[1] - x, q[3] - z, p[1] - x, p[3] - z))
            s:Destroy()
        else
            LOG(string.format('PROBE %s D=%.1f not started moved=%s eng=%.2f,%.2f', bp, d, tostring(moved), p[1] - x, p[3] - z))
        end
        e:Destroy()
        WaitTicks(8)
        d = d + 0.5
    end
    running = running - 1
end

ForkThread(function()
    WaitTicks(10)
    ForkThread(lane, 'ueb5101', -20, 4.5, 8, 34)
    ForkThread(lane, 'ueb1101', -32, 5, 9.5, 34)
    ForkThread(lane, 'ueb0101', 0, 8, 15)
    WaitTicks(20)
    while running > 0 do WaitTicks(10) end
    LOG('PROBE done')
end)
