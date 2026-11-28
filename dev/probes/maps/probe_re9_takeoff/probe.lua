-- Differential probe (sweep 9): take-off of every aircraft.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re9_takeoff 150
--   ours: --headless --no-ai --ticks 800 --script dev/probes/maps/probe_re9_takeoff/probe.lua
-- BP   every flying unit blueprint: its whole Air table and the Physics / size fields once.
-- TO   each one created (nose toward its goal) and left to land on flat dry land (three batches, lanes below), left
--      to settle, then one IssueMove 300 straight on; every beat for 130 beats:
--      'T <tag> <beat> x y z ground surface vx vy vz (per second) heading layer moving up down q1..q4'.
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function P(text) LOG('PROBE ' .. text) end

local function scalar(v)
    if type(v) == 'number' then return fmt('%.4g', v) end
    if type(v) == 'table' then return 'table' end
    return tostring(v)
end
local function dump(prefix, t)
    if type(t) ~= 'table' then P(prefix .. ' nil') return end
    local keys = {}
    for k, _ in t do table.insert(keys, tostring(k)) end
    table.sort(keys)
    local out = {}
    for _, k in keys do table.insert(out, k .. '=' .. scalar(t[k])) end
    P(prefix .. ' ' .. table.concat(out, ' '))
end

-- Playable flying units: the faction air ids, FAF's three extra ones.
local ids = {}
for id, bp in __blueprints do
    if type(id) == 'string' and type(bp) == 'table' and bp.Physics and bp.Physics.MotionType == 'RULEUMT_Air'
        and bp.Air and bp.Air.CanFly and (string.find(id, '^[ux][aers]a0[1-4]%d%d$') or id == 'daa0206' or id == 'dea0202' or id == 'dra0202') then
        table.insert(ids, id)
    end
end
table.sort(ids)
for _, id in ids do
    local bp = __blueprints[id]
    dump('BP ' .. id .. ' Air', bp.Air)
    local ph = bp.Physics
    P(fmt('BP %s Phys Elevation=%s MaxSpeed=%s GroundCollisionOffset=%s SizeX=%s SizeY=%s SizeZ=%s SizeSphere=%s exp=%s',
        id, scalar(ph.Elevation), scalar(ph.MaxSpeed), scalar(ph.GroundCollisionOffset), scalar(bp.SizeX), scalar(bp.SizeY),
        scalar(bp.SizeZ), scalar(bp.SizeSphere), tostring(bp.CategoriesHash and bp.CategoriesHash.EXPERIMENTAL or false)))
end
P('BP count ' .. table.getn(ids))

local states = {}
for _, s in {'Moving', 'MovingUp', 'MovingDown', 'TakingOff'} do states[s] = true end
local tracked = {}
local function line(w, beat)
    local u = w.u
    if u:BeenDestroyed() then return end
    local x, y, z = u:GetPositionXYZ()
    local vx, vy, vz = u:GetVelocity()
    local q = u:GetOrientation()
    local st = {}
    for _, s in {'Moving', 'MovingUp', 'MovingDown'} do
        local ok, v = pcall(u.IsUnitState, u, s)
        table.insert(st, ok and (v and '1' or '0') or 'E')
    end
    P(fmt('T %s %d %.3f %.3f %.3f %.3f %.3f %.3f %.3f %.3f %.4f %s %s %.4f %.4f %.4f %.4f', w.tag, beat, x, y, z,
        GetTerrainHeight(x, z), GetSurfaceHeight(x, z), 10 * vx, 10 * vy, 10 * vz, u:GetHeading(), tostring(u:GetCurrentLayer()),
        table.concat(st, ''), q[1] or 0, q[2] or 0, q[3] or 0, q[4] or 0))
end

-- Layout: three batches, each on three flat dry lanes (ground 18..19.5): the NE strip
-- (x 576..744, z 72..108) and the SW strip (x 288..450, z 918..954), moving +x, units
-- 4 apart across and staggered 12 along; one experimental alone on the lane at x 666,
-- z 318..462, moving +z.
local small, big = {}, {}
for _, id in ids do
    local bp = __blueprints[id]
    if bp.CategoriesHash and bp.CategoriesHash.EXPERIMENTAL then table.insert(big, id) else table.insert(small, id) end
end
local batches = {{}, {}, {}}
local nb = table.getn(batches)
local per = math.ceil(table.getn(small) / nb)
for i, id in small do
    local b = math.floor((i - 1) / per) + 1
    local k = table.getn(batches[b])
    local lane = math.mod(k, 2)
    local slot = math.floor(k / 2)
    local x0, z0 = 580, 74
    if lane == 1 then x0, z0 = 292, 920 end
    table.insert(batches[b], {tag = 'TO_' .. id, id = id, x = x0 + math.mod(slot, 2) * 12, z = z0 + slot * 3.8, dx = 300, dz = 0, yaw = math.pi / 2})
end
for i, id in big do
    table.insert(batches[math.mod(i - 1, nb) + 1], {tag = 'TO_' .. id, id = id, x = 666, z = 330, dx = 0, dz = 300, yaw = 0})
end

ForkThread(function()
    WaitTicks(2)
    for bi, tracked in batches do
        for _, w in tracked do
            P(fmt('G %s ground=%.3f surface=%.3f', w.tag, GetTerrainHeight(w.x, w.z), GetSurfaceHeight(w.x, w.z)))
            w.u = CreateUnitHPR(w.id, 'ARMY_1', w.x, GetTerrainHeight(w.x, w.z), w.z, 0, w.yaw, 0)
            w.u:SetFireState(1)
        end
        local function all_landed()
            for _, w in tracked do
                if not w.u:BeenDestroyed() and w.u:GetCurrentLayer() ~= 'Land' then return false end
            end
            return true
        end
        local waited = 0
        for b = 0, 120 do
            if b == 0 or b == 10 then for _, w in tracked do line(w, b - 100) end end
            if b >= 20 and all_landed() then break end
            waited = b
            WaitTicks(1)
        end
        P('batch ' .. bi .. ' settled after ' .. waited .. ' all_landed=' .. tostring(all_landed()))
        for _, w in tracked do
            if not w.u:BeenDestroyed() and w.u:GetCurrentLayer() ~= 'Land' then P('notlanded ' .. w.tag .. ' ' .. tostring(w.u:GetCurrentLayer())) end
        end
        for _, w in tracked do line(w, -1) end
        for _, w in tracked do
            local x, y, z = w.u:GetPositionXYZ()
            IssueMove({w.u}, {x + w.dx, GetTerrainHeight(x + w.dx, z + w.dz), z + w.dz})
        end
        P('batch ' .. bi .. ' ordered at tick ' .. GetGameTick())
        for b = 0, 130 do
            for _, w in tracked do line(w, b) end
            WaitTicks(1)
        end
        for _, w in tracked do if not w.u:BeenDestroyed() then w.u:Destroy() end end
        WaitTicks(2)
    end
    LOG('PROBE done')
end)
