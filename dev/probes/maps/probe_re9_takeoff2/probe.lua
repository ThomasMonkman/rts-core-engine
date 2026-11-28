-- Differential probe (sweep 9, part 2): take-off cases beside probe_re9_takeoff's plain one.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re9_takeoff2 90
--   ours: --headless --no-ai --ticks 330 --script dev/probes/maps/probe_re9_takeoff2/probe.lua
-- AIR  made in the air (at Elevation) and ordered 300 straight on at once: cruise from rest.
-- SH   landed, a short move (40) straight on: the goal is reached before the climb ends.
-- PX   landed with the nose 90 degrees off the goal (yaw 0, goal +x): the heading factor.
-- BA   a landed UEF T1 bomber ordered to attack an enemy tank 30 / 50 / 80 ahead: the height
--      of its first bomb ('F <tag> beat height ...' from OnWeaponFired, and the bomb's).
-- Lines 'T ...' as in probe_re9_takeoff.
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


local cases = {}
local function add(tag, id, x, z, yaw, dx, dz, air) table.insert(cases, {tag = tag, id = id, x = x, z = z, yaw = yaw, dx = dx, dz = dz, air = air}) end
-- NE strip (x 576..744, z 72..108), SW strip (x 288..450, z 918..954): moving +x.
local ne, sw = 0, 0
local function NE(tag, id, yaw, dx) add(tag, id, 580 + math.mod(ne, 2) * 12, 74 + ne * 3.8, yaw, dx, 0) ne = ne + 1 end
local function SW(tag, id, yaw, dx, air) add(tag, id, 292 + math.mod(sw, 2) * 12, 920 + sw * 3.8, yaw, dx, 0, air) sw = sw + 1 end
for _, id in {'uea0102', 'uea0103', 'uea0304', 'uea0203'} do NE('SH_' .. id, id, math.pi / 2, 40) end
NE('PX_uea0102', 'uea0102', 0, 300)
for _, d in {30, 50, 80} do NE('BA' .. d, 'uea0103', math.pi / 2, 0) cases[table.getn(cases)].target = d end
SW('PX_uea0103', 'uea0103', 0, 300)
SW('PX_uea0304', 'uea0304', 0, 300)
for _, id in {'uea0102', 'uea0103', 'uea0303', 'uea0304', 'uea0203', 'uea0107'} do SW('AIR_' .. id, id, math.pi / 2, 300, true) end

ForkThread(function()
    WaitTicks(2)
    for _, w in cases do
        w.u = CreateUnitHPR(w.id, 'ARMY_1', w.x, GetTerrainHeight(w.x, w.z), w.z, 0, w.yaw, 0)
        w.u:SetFireState(1)
        table.insert(tracked, w)
    end
    local t0 = GetGameTick()
    -- AIR: ordered at once, from Elevation at rest.
    for _, w in cases do
        if w.air then
            local x, y, z = w.u:GetPositionXYZ()
            IssueMove({w.u}, {x + w.dx, GetTerrainHeight(x + w.dx, z + w.dz), z + w.dz})
            w.start = t0
        end
    end
    local function landed()
        for _, w in cases do
            if not w.air and not w.u:BeenDestroyed() and w.u:GetCurrentLayer() ~= 'Land' then return false end
        end
        return true
    end
    local waited = 0
    for b = 0, 120 do
        for _, w in cases do if w.air then line(w, b) end end
        if b >= 20 and landed() then break end
        waited = b
        WaitTicks(1)
    end
    P('settled after ' .. waited .. ' all_landed=' .. tostring(landed()))
    for _, w in cases do
        if not w.air and w.u:GetCurrentLayer() ~= 'Land' then P('notlanded ' .. w.tag .. ' ' .. tostring(w.u:GetCurrentLayer())) end
    end
    for _, w in cases do if not w.air then line(w, -1) end end
    for _, w in cases do
        if w.target then
            local tx = w.x + w.target
            w.tank = CreateUnitHPR('uel0201', 'ARMY_2', tx, GetTerrainHeight(tx, w.z), w.z, 0, 0, 0)
            w.tank:SetFireState(1)
            local wep = w.u:GetWeapon(1)
            local old = wep.OnWeaponFired
            local tag, u = w.tag, w.u
            wep.OnWeaponFired = function(self, a, b, c)
                local x, y, z = u:GetPositionXYZ()
                local vx, vy, vz = u:GetVelocity()
                P(fmt('F %s %d h=%.3f x=%.3f dx=%.3f v=%.3f vy=%.3f', tag, GetGameTick(), y - GetTerrainHeight(x, z), x, tx - x, 10 * math.sqrt(vx * vx + vz * vz), 10 * vy))
                if old then return old(self, a, b, c) end
            end
            w.u:SetFireState(0)
            IssueAttack({w.u}, w.tank)
        elseif not w.air then
            local x, y, z = w.u:GetPositionXYZ()
            IssueMove({w.u}, {x + w.dx, GetTerrainHeight(x + w.dx, z + w.dz), z + w.dz})
        end
    end
    P('ordered at tick ' .. GetGameTick())
    for b = 0, 150 do
        for _, w in cases do if not w.air then line(w, b) end end
        WaitTicks(1)
    end
    LOG('PROBE done')
end)
