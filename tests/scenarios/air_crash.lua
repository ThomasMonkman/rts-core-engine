-- Scenario test (tests/scenarios.rs): ARMY_1 / ARMY_2 have no AI; checks go
-- into ScenarioResult, which the test reads once `done` is set.
-- GetPosition() is one table per entity, rewritten by each call: copy it to keep it.
local function pos(e) local p = e:GetPosition() return Vector(p[1], p[2], p[3]) end
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) LOG('air_crash: ' .. msg) end
local function done() ScenarioResult.done = true end
local function ground(x, z) return GetTerrainHeight(x, z) end

-- Aircraft killed in flight fall out of the sky with their momentum (moho's
-- ballistic motion: the last tick's displacement plus gravity 4.9) and crash
-- (`OnImpact`), where FAF's AirUnit deals its `DeathImpact` weapon's damage
-- around the wreck. A CZAR (8000 in radius 15), a T1 bomber and an
-- interceptor are each killed at speed above a line of enemy tanks; the
-- fall (time, distance from the death point) and the damage are logged.
--
-- Also run windowed for screenshots: `CRASH_TRIAL` picks one trial
-- (1 CZAR, 2 bomber, 3 interceptor), and the log says the tick it dies.

-- Wraps DamageArea to log what the crashes deal (FAF's AirUnit.OnImpact
-- calls the global).
local areas = {}
local damageArea = DamageArea
rawset(_G, 'DamageArea', function(instigator, pos, radius, amount, kind, friendly, damageSelf)
    if instigator and instigator.CrashTrial then
        table.insert(areas, { unit = instigator, x = pos[1], y = pos[2], z = pos[3], radius = radius, amount = amount, kind = kind })
    end
    return damageArea(instigator, pos, radius, amount, kind, friendly, damageSelf)
end)

local function trial(t)
    local x0, z = t.x, t.z
    local plane = CreateUnitHPR(t.bp, 'ARMY_1', x0, ground(x0, z) + 20, z, 0, 0, 0)
    -- (Scripts' aircraft start at their flying height; the trials were laid
    -- out for 20 up.)
    Warp(plane, { x0, ground(x0, z) + 20, z })
    plane.CrashTrial = true
    plane:SetFireState(1)
    IssueMove({ plane }, { x0 + 400, 0, z })
    WaitTicks(t.wait)
    -- Enemy tanks along the flight path, on hold fire, created on the tick it
    -- dies so they never shoot it.
    local p = pos(plane)
    local vx, vy, vz = plane:GetVelocity()
    local speed = 10 * math.sqrt(vx * vx + vz * vz)
    local dirx, dirz = vx / math.max(1e-6, math.sqrt(vx * vx + vz * vz)), vz / math.max(1e-6, math.sqrt(vx * vx + vz * vz))
    local tanks = {}
    for k = (t.count > 0 and -2 or 1), t.count do
        local d = k * t.spacing
        local tx, tz = p[1] + dirx * d, p[3] + dirz * d
        local tank = CreateUnitHPR('ual0201', 'ARMY_2', tx, ground(tx, tz), tz, 0, 0, 0)
        tank:SetFireState(1)
        IssueStop({ tank })
        table.insert(tanks, tank)
    end
    local impacted, impactTick, impactPos, impactLayer, impactTurn
    local o0 = plane:GetOrientation()
    local oi = plane.OnImpact
    plane.OnImpact = function(self, with)
        if not impacted then
            impacted, impactTick, impactPos, impactLayer = with, GetGameTick(), pos(self), self.Layer
            -- How far it tumbled: the angle between the orientations.
            local o1 = self:GetOrientation()
            local dot = math.abs(o0[1] * o1[1] + o0[2] * o1[2] + o0[3] * o1[3] + o0[4] * o1[4])
            impactTurn = math.deg(2 * math.acos(math.min(1, dot)))
        end
        return oi(self, with)
    end
    local start = GetGameTick()
    local height = p[2] - ground(p[1], p[3])
    LOG(string.format('air_crash: %s killed at tick %d at (%.1f, %.1f, %.1f)', t.bp, start, p[1], p[2], p[3]))
    plane:Kill()
    -- Its fall starts from the last tick's displacement.
    local deathPos, fallV = pos(plane), { plane:GetVelocity() }
    local samples = {}
    for _ = 1, 200 do
        if impacted then break end
        WaitTicks(1)
        if plane:BeenDestroyed() then break end
        local q = pos(plane)
        table.insert(samples, string.format('%.1f,%.1f', VDist2(p[1], p[3], q[1], q[3]), q[2] - ground(q[1], q[3])))
    end
    WaitTicks(5)
    local killed, dealt, nearest = 0, 0, 1e9
    local reached = 0
    for _, tank in tanks do
        local lost = tank:BeenDestroyed() and tank.Blueprint.Defense.MaxHealth or (tank:GetMaxHealth() - tank:GetHealth())
        if tank.Dead or tank:BeenDestroyed() then killed = killed + 1 end
        dealt = dealt + lost
    end
    local area = nil
    for _, a in areas do if a.unit == plane and a.kind ~= 'TreeForce' then area = a end end
    local horizontal = impactPos and VDist2(p[1], p[3], impactPos[1], impactPos[3]) or -1
    local secs = impactTick and (impactTick - start) / 10 or -1
    note(string.format('%s: killed at %.1f above the ground moving %.2f/s; crashed (%s) after %.1fs, %.1f from the death point (no drag, g 4.9: %.1f); DamageArea %s; tanks killed %d of %d, damage %d',
        t.bp, height, speed, tostring(impacted), secs, horizontal,
        speed * math.sqrt(2 * height / 4.9),
        area and string.format('%g in radius %g at %.1f above the ground', area.amount, area.radius, area.y - ground(area.x, area.z)) or 'none',
        killed, table.getn(tanks), dealt))
    note(t.bp .. ' fall (distance,height): ' .. table.concat(samples, ' '))
    local surface = t.surface or 'Terrain'
    check(impacted == surface, t.bp .. ': never crashed (OnImpact ' .. surface .. ')')
    check(area ~= nil, t.bp .. ': the crash dealt no area damage')
    if area then
        check(area.y - GetSurfaceHeight(area.x, area.z) < 1, t.bp .. ': the crash damage was dealt in the air')
    end
    -- Moho's ballistic fall over the real ground: the starting velocity
    -- (climbing or sinking as it was) plus gravity 4.9 (0.049 per tick per
    -- tick) until it meets the terrain.
    local fallTime = -1
    do
        local x, y, z = deathPos[1], deathPos[2], deathPos[3]
        local vx, vy, vz = fallV[1], fallV[2], fallV[3]
        for tick = 1, 200 do
            vy = vy - 0.049
            x, y, z = x + vx, y + vy, z + vz
            if y <= ground(x, z) then fallTime = tick / 10 break end
        end
    end
    -- Momentum: it carries on along its path (no drag, so ~speed x fall time).
    local expected = speed * fallTime
    check(surface == 'Water' or horizontal > 0.8 * expected and horizontal < 1.2 * expected + 1,
        string.format('%s: fell %.1f from its death point, expected about %.1f', t.bp, horizontal, expected))
    check(surface == 'Water' or math.abs(secs - fallTime) < 0.4, string.format('%s: fall took %.1fs', t.bp, secs))
    if t.minKilled then
        check(killed >= t.minKilled, string.format('%s: the crash killed %d tanks, expected at least %d', t.bp, killed, t.minKilled))
    end
    for _, tank in tanks do if not tank:BeenDestroyed() then tank:Destroy() end end
    WaitTicks(60)
    -- FAF's DeathThread leaves the wreck where it crashed (on the Land layer).
    local wreck = false
    if impactPos then
        for _, r in GetReclaimablesInRect(impactPos[1] - 4, impactPos[3] - 4, impactPos[1] + 4, impactPos[3] + 4) or {} do
            if r.IsWreckage then wreck = true end
        end
    end
    note(string.format('%s: layer at impact %s, tumbled %.1f degrees on the way down, wreck at the crash site: %s',
        t.bp, tostring(impactLayer), impactTurn or -1, tostring(wreck)))
    local layer = surface == 'Water' and 'Water' or 'Land'
    check(impactLayer == layer, t.bp .. ': not on the ' .. layer .. ' layer when it crashed, but ' .. tostring(impactLayer))
    if surface == 'Terrain' then
        check(wreck, t.bp .. ': no wreck at the crash site')
    else
        -- Over water FAF sinks the wreck, then destroys the unit.
        WaitTicks(150)
        note(string.format('%s: sank and was destroyed: %s', t.bp, tostring(plane:BeenDestroyed())))
        check(plane:BeenDestroyed(), t.bp .. ': never sank')
    end
    return impactTick
end

ForkThread(function()
    WaitTicks(2)
    -- A far-off unit keeps each army in the game.
    CreateUnitHPR('ueb3101', 'ARMY_1', 200, ground(200, 300), 300, 0, 0, 0)
    CreateUnitHPR('ueb3101', 'ARMY_2', 220, ground(220, 300), 300, 0, 0, 0)
    local trials = {
        { bp = 'uaa0310', x = 300, z = 600, wait = 150, spacing = 5, count = 12, minKilled = 5 },
        { bp = 'uaa0103', x = 330, z = 660, wait = 100, spacing = 2, count = 25 },
        { bp = 'uaa0102', x = 330, z = 540, wait = 60, spacing = 2, count = 25 },
        -- Over the sea: it comes down on the water and sinks.
        { bp = 'uaa0102', x = 330, z = 700, wait = 60, spacing = 2, count = 0, surface = 'Water' },
    }
    local only = rawget(_G, 'CRASH_TRIAL')
    for i, t in trials do
        if not only or only == i then trial(t) end
    end
    done()
end)
