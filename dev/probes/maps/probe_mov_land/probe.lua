-- Differential probe: a movement census of surface units (land, hover,
-- amphibious, walkers, experimentals), logged every tick in a format both
-- engines print identically ("PROBE ..." lines). Engine APIs only.
--   real game (Windows): dev/probes/run_faf.ps1 probe_mov_land 150
--   ours: cargo run -- --headless --no-ai --ticks 900 --script dev/probes/maps/probe_mov_land/probe.lua
-- Each unit has its own flat lane on Seton's and drives the same course,
-- stepped by where it is (k = 1, experimentals 2):
--   p1: from rest facing +x, a move 12k along +x with a queued leg 8k along +z
--       (acceleration, top speed, a 90-degree corner at speed);
--   p2: half-way up the +z leg, cleared and ordered back to 2k behind the
--       corner (a 180 at speed);
--   p3: once at rest, a move 14k along +x (a 90-degree turn from rest);
--   p4: 7k into it, IssueStop (braking from speed);
--   p5: once at rest, a move 4k straight behind it;
--   p6: at rest again: final position logged.
-- Lines: 'PROBE bp <tag> ...' physics blueprint values; 'PROBE t=<tick> <tag>
-- p<phase> x z y yaw v idle moving'; 'PROBE t=<tick> <tag> horz|vert|layer
-- <old> <new>' for the motion / layer callbacks as they come.
local TICKS = 850
local armies = ListArmies()
local function f2(n) return string.format('%.2f', n) end
local function f3(n) return string.format('%.3f', n) end

for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end

local LANES = {}
local function lanes(xs, zs) for _, x in xs do for _, z in zs do table.insert(LANES, { x, z }) end end end
lanes({ 594, 622, 650 }, { 72, 84, 96, 108, 120 })
lanes({ 678, 706 }, { 72, 84, 96 })
lanes({ 622, 650 }, { 132 })
lanes({ 322 }, { 914, 926, 938 })
lanes({ 350, 378 }, { 902, 914, 926, 938 })
lanes({ 406 }, { 902, 914, 926, 938, 950 })
lanes({ 294 }, { 926, 938, 950 })
local BIG = { { 230, 850 }, { 670, 185 }, { 730, 185 }, { 730, 225 } }

local UNITS = {
    'uel0101', 'uel0106', 'uel0201', 'uel0103', 'uel0105', 'uel0202', 'uel0203', 'uel0303', 'uel0304', 'uel0307',
    'uel0111', 'xel0305', 'del0204', 'uel0001', 'uel0309',
    'url0106', 'url0107', 'url0202', 'url0203', 'url0303', 'url0304', 'xrl0305', 'drl0204', 'xrl0302', 'url0001',
    'ual0101', 'ual0106', 'ual0201', 'ual0202', 'ual0303', 'ual0304', 'xal0203', 'xal0305', 'ual0105', 'dal0310',
    'xsl0101', 'xsl0201', 'xsl0202', 'xsl0203', 'xsl0303', 'xsl0305', 'xsl0111',
}
local BIGS = { 'url0402', 'ual0401', 'uel0401', 'xsl0401' }

local movers = {}
local function snap(v, size) return math.floor(v - size / 2 + 0.5) + size / 2 end
local function add(bp, lane, k)
    local b = __blueprints[bp]
    if not b then LOG('PROBE missing ' .. bp) return end
    local fp, ph = b.Footprint or {}, b.Physics or {}
    local sx, sz = fp.SizeX or 1, fp.SizeZ or 1
    local x, z = snap(lane[1], sx), snap(lane[2], sz)
    local u = CreateUnitHPR(bp, armies[1], x, GetTerrainHeight(x, z), z, 0, math.pi / 2, 0)
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
for i, bp in UNITS do if LANES[i] then add(bp, LANES[i], 1) end end
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
    LOG('PROBE t=' .. tick .. ' ' .. m.tag .. ' p' .. m.phase .. ' ' .. f2(x) .. ' ' .. f2(z) .. ' ' .. f2(y - GetTerrainHeight(x, z))
        .. ' yaw=' .. f3(yaw(u)) .. ' v=' .. f2(v) .. ' idle=' .. tostring(u:IsIdleState()) .. ' mv=' .. tostring(u:IsMoving()))
end

ForkThread(function()
    local start = GetGameTick()
    for tick = 0, TICKS do
        local busy = false
        for _, m in movers do
            step(m, GetGameTick() - start)
            if m.phase < 6 then busy = true end
        end
        if not busy then break end
        WaitTicks(1)
    end
    for _, m in movers do LOG('PROBE end ' .. m.tag .. ' p' .. m.phase) end
    WaitTicks(30)
    LOG('PROBE done')
end)
