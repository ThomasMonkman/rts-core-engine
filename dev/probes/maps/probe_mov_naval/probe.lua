-- Differential probe: a movement census of naval units, and surface units
-- crossing a beach, logged every tick in a format both engines print
-- identically ("PROBE ..." lines). Engine APIs only.
--   real game (Windows): dev/probes/run_faf.ps1 probe_mov_naval 200
--   ours: cargo run -- --headless --no-ai --ticks 1000 --script dev/probes/maps/probe_mov_naval/probe.lua
-- Ships: each has its own lane in deep water on Seton's and sails the course
-- of probe_mov_land with k = 4 (a 48 leg along +x with a queued 32 leg along
-- +z; cleared half-way up it and ordered back to 8 behind the corner; from
-- rest a 56 leg along +x, IssueStop 28 into it; from rest a move 16 straight
-- behind; final position).
-- Beach ('B<bp>' tags): amphibious / hover / floating units cross Seton's
-- north beach along -x from dry land (x 652) into water 5 deep (x 606) and,
-- once at rest, back; the Cybran destroyer the other way round. Per tick:
-- height over the terrain, depth of water there, layer; callbacks as they come.
local TICKS = 950
local armies = ListArmies()
local function f2(n) return string.format('%.2f', n) end
local function f3(n) return string.format('%.3f', n) end

for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end

local LANES = {}
local function lanes(xs, zs) for _, z in zs do for _, x in xs do table.insert(LANES, { x, z }) end end end
lanes({ 530, 645, 760 }, { 612, 668, 724, 780 })
lanes({ 545, 660, 775, 890 }, { 836, 892, 948 })

local UNITS = {
    'ues0103', 'ues0203', 'ues0201', 'ues0202', 'ues0302', 'ues0304', 'ues0401', 'xes0102', 'xes0205', 'xes0307',
    'urs0103', 'urs0201', 'urs0202', 'urs0302', 'urs0303', 'xrs0204', 'xrs0205',
    'uas0102', 'uas0103', 'uas0201', 'uas0302', 'uas0401', 'xss0103', 'xss0201', 'xss0302',
}
local BIGS = {}
local BIG = {}

local movers = {}
local function snap(v, size) return math.floor(v - size / 2 + 0.5) + size / 2 end
local function add(bp, lane, k)
    local b = __blueprints[bp]
    if not b then LOG('PROBE missing ' .. bp) return end
    local fp, ph = b.Footprint or {}, b.Physics or {}
    local sx, sz = fp.SizeX or 1, fp.SizeZ or 1
    local x, z = snap(lane[1], sx), snap(lane[2], sz)
    local u = CreateUnitHPR(bp, armies[1], x, GetSurfaceHeight(x, z), z, 0, math.pi / 2, 0)
    u:SetFireState(1)
    local m = { tag = bp, u = u, x = x, z = z, k = k, sx = sx, sz = sz, phase = 0, still = 0 }
    table.insert(movers, m)
    LOG('PROBE bp ' .. bp .. ' motion=' .. tostring(ph.MotionType) .. ' speed=' .. tostring(ph.MaxSpeed)
        .. ' acc=' .. tostring(ph.MaxAcceleration) .. ' brake=' .. tostring(ph.MaxBrake) .. ' turn=' .. tostring(ph.TurnRate)
        .. ' steer=' .. tostring(ph.MaxSteerForce) .. ' spot=' .. tostring(ph.RotateOnSpot) .. ' thresh=' .. tostring(ph.RotateOnSpotThreshold)
        .. ' backup=' .. tostring(ph.BackUpDistance) .. ' rev=' .. tostring(ph.MaxSpeedReverse) .. ' radius=' .. tostring(ph.TurnRadius)
        .. ' facing=' .. tostring(ph.TurnFacingRate) .. ' fp=' .. sx .. 'x' .. sz .. ' at ' .. f2(x) .. ' ' .. f2(z))
    local tag = bp
    local horz, vert, layer = u.OnMotionHorzEventChange, u.OnMotionVertEventChange, u.OnLayerChange
    u.OnMotionHorzEventChange = function(self, new, old)
        LOG('PROBE t=' .. GetGameTick() .. ' ' .. tag .. ' horz ' .. tostring(old) .. ' ' .. tostring(new))
        return horz(self, new, old)
    end
    u.OnMotionVertEventChange = function(self, new, old)
        LOG('PROBE t=' .. GetGameTick() .. ' ' .. tag .. ' vert ' .. tostring(old) .. ' ' .. tostring(new))
        return vert(self, new, old)
    end
    u.OnLayerChange = function(self, new, old)
        LOG('PROBE t=' .. GetGameTick() .. ' ' .. tag .. ' layer ' .. tostring(old) .. ' ' .. tostring(new))
        return layer(self, new, old)
    end
end
for i, bp in UNITS do if LANES[i] then add(bp, LANES[i], 4) end end
for i, bp in BIGS do add(bp, BIG[i], 2) end

local function yaw(u)
    local q = u:GetOrientation()
    local qx, qy, qz, qw = q[1], q[2], q[3], q[4]
    -- forward (0, 0, 1) turned by q
    local fx = 2 * (qx * qz + qw * qy)
    local fz = 1 - 2 * (qx * qx + qy * qy)
    return math.atan2(fx, fz)
end

local function step(m, tick)
    local u = m.u
    if u:BeenDestroyed() or u.Dead then
        if m.phase ~= 99 then LOG('PROBE t=' .. tick .. ' ' .. m.tag .. ' gone') m.phase = 99 end
        return
    end
    if m.phase == 6 then return end
    local x, y, z = u:GetPositionXYZ()
    local vx, vy, vz = u:GetVelocity()
    local v = 10 * math.sqrt(vx * vx + vz * vz)
    if v < 0.001 then m.still = m.still + 1 else m.still = 0 end
    local k = m.k
    local function goal(gx, gz) return { snap(gx, m.sx), 0, snap(gz, m.sz) } end
    if m.phase == 0 and tick >= 10 then
        IssueMove({ u }, goal(m.x + 12 * k, m.z))
        IssueMove({ u }, goal(m.x + 12 * k, m.z + 8 * k))
        m.phase = 1
    elseif m.phase == 1 and x > m.x + 11 * k and z > m.z + 4 * k then
        IssueClearCommands({ u })
        IssueMove({ u }, goal(m.x + 12 * k, m.z - 2 * k))
        m.phase = 2
    elseif m.phase == 2 and m.still >= 8 and u:IsIdleState() then
        IssueMove({ u }, goal(m.x + 26 * k, m.z - 2 * k))
        m.phase = 3
    elseif m.phase == 3 and x > m.x + 19 * k then
        IssueStop({ u })
        m.phase = 4
    elseif m.phase == 4 and m.still >= 10 then
        IssueMove({ u }, goal(x - 4 * k, m.z - 2 * k))
        m.phase = 5
    elseif m.phase == 5 and m.still >= 10 and u:IsIdleState() then
        m.phase = 6
    end
    LOG('PROBE t=' .. tick .. ' ' .. m.tag .. ' p' .. m.phase .. ' ' .. f2(x) .. ' ' .. f2(z) .. ' ' .. f2(y - GetSurfaceHeight(x, z))
        .. ' yaw=' .. f3(yaw(u)) .. ' v=' .. f2(v) .. ' idle=' .. tostring(u:IsIdleState()) .. ' mv=' .. tostring(u:IsMoving()) .. ' ' .. tostring(u:GetCurrentLayer()))
end


-- The beach.
local beach = {}
local function hookb(u, tag, name)
    local old = u[name]
    u[name] = function(self, new, o)
        LOG('PROBE t=' .. GetGameTick() .. ' ' .. tag .. ' ' .. name .. ' ' .. tostring(o) .. ' ' .. tostring(new))
        return old(self, new, o)
    end
end
local function shore(bp, z, from, to)
    local wet = GetSurfaceHeight(from, z) > GetTerrainHeight(from, z)
    local y = wet and GetSurfaceHeight(from, z) or GetTerrainHeight(from, z)
    local u = CreateUnitHPR(bp, armies[1], from + 0.5, y, z + 0.5, 0, from > to and -math.pi / 2 or math.pi / 2, 0)
    u:SetFireState(1)
    local tag = 'B' .. bp
    hookb(u, tag, 'OnLayerChange') hookb(u, tag, 'OnMotionVertEventChange')
    table.insert(beach, { tag = tag, u = u, from = from, to = to, z = z, phase = 0, still = 0 })
end
for i, bp in { 'url0203', 'uel0203', 'uel0105', 'xsl0303', 'xrl0305', 'ual0201', 'xsl0203', 'uel0401' } do
    shore(bp, 244 + i * 3, 652, 606)
end
shore('urs0201', 282, 590, 660)
local function bstep(m, tick)
    local u = m.u
    if u:BeenDestroyed() or u.Dead then
        if m.phase ~= 99 then LOG('PROBE t=' .. tick .. ' ' .. m.tag .. ' gone') m.phase = 99 end
        return
    end
    if m.phase == 3 then return end
    local x, y, z = u:GetPositionXYZ()
    local vx, vy, vz = u:GetVelocity()
    local v = 10 * math.sqrt(vx * vx + vz * vz)
    if v < 0.001 then m.still = m.still + 1 else m.still = 0 end
    if m.phase == 0 and tick >= 10 then
        IssueMove({ u }, { m.to + 0.5, 0, m.z + 0.5 })
        m.phase = 1
    elseif m.phase == 1 and m.still >= 10 and u:IsIdleState() then
        IssueMove({ u }, { m.from + 0.5, 0, m.z + 0.5 })
        m.phase = 2
    elseif m.phase == 2 and m.still >= 10 and u:IsIdleState() then
        m.phase = 3
    end
    local t, s = GetTerrainHeight(x, z), GetSurfaceHeight(x, z)
    LOG('PROBE t=' .. tick .. ' ' .. m.tag .. ' b' .. m.phase .. ' ' .. f2(x) .. ' ' .. f2(z) .. ' h=' .. f2(y - t) .. ' depth=' .. f2(s - t)
        .. ' v=' .. f2(v) .. ' vy=' .. f2(10 * vy) .. ' ' .. tostring(u:GetCurrentLayer()))
end

ForkThread(function()
    local start = GetGameTick()
    for tick = 0, TICKS do
        local busy = false
        for _, m in movers do
            step(m, GetGameTick() - start)
            if m.phase < 6 then busy = true end
        end
        for _, m in beach do
            bstep(m, GetGameTick() - start)
            if m.phase < 3 then busy = true end
        end
        if not busy then break end
        WaitTicks(1)
    end
    for _, m in movers do LOG('PROBE end ' .. m.tag .. ' p' .. m.phase) end
    for _, m in beach do LOG('PROBE end ' .. m.tag .. ' b' .. m.phase) end
    WaitTicks(30)
    LOG('PROBE done')
end)
