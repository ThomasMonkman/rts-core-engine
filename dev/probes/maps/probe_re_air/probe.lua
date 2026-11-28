-- Differential probe (re-check of open sweep items): gunship speed / acc
-- multipliers on two types with a baseline (M*), winged take-off on a long
-- move (TK*), bomber run height from cruise (B*), a gunship's death fall (D*).
--   real game (Windows): dev/probes/run_faf.ps1 probe_re_air 100
--   ours: --headless --no-ai --ticks 700 --script dev/probes/maps/probe_re_air/probe.lua
-- Lines: 'PROBE t=<tick> <tag> x z h= v= vy=' per tick while it matters,
-- events 'PROBE t=<tick> <tag> <event>'.
local TICKS = 340
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
for _, b in ArmyBrains do b:GiveStorage('ENERGY', 1000000) b:GiveResource('ENERGY', 1000000) b:GiveStorage('MASS', 100000) b:GiveResource('MASS', 100000) end
local start
local function T() return GetGameTick() - start end
local function ev(tag, s) LOG('PROBE t=' .. T() .. ' ' .. tag .. ' ' .. s) end
local function st(u)
    if u:BeenDestroyed() then return 'gone' end
    local x, y, z = u:GetPositionXYZ()
    local vx, vy, vz = u:GetVelocity()
    local g = GetSurfaceHeight(x, z)
    return fmt('%.2f %.2f h=%.2f v=%.2f vy=%.2f %s', x, z, y - g, 10 * math.sqrt(vx * vx + vz * vz), 10 * vy, tostring(u:GetCurrentLayer()))
end
local function spawn(bp, army, x, z, air)
    local u = CreateUnitHPR(bp, army, x, GetSurfaceHeight(x, z) + (air and 20 or 0), z, 0, math.pi / 2, 0)
    if u.SetFireState then u:SetFireState(1) end
    return u
end

-- M: multipliers; spawned over water at elevation, ordered 300 along +x at t=60
local M = {}
for i, spec in { { 'uea0203', 'sp', 1 }, { 'uea0203', 'sp', 0.5 }, { 'uea0203', 'sp', 2 }, { 'uea0203', 'acc', 0.25 }, { 'uea0203', 'acc', 0.5 },
                 { 'uaa0203', 'sp', 1 }, { 'uaa0203', 'sp', 0.5 }, { 'uaa0203', 'acc', 0.25 } } do
    local x, z = 100, 290 + i * 14
    local u = spawn(spec[1], A1, x, z, true)
    if spec[2] == 'sp' then u:SetSpeedMult(spec[3]) else u:SetAccMult(spec[3]) end
    table.insert(M, { tag = 'M' .. spec[1] .. spec[2] .. spec[3], u = u, goal = { x + 300, 0, z } })
end
-- TK: idle winged planes land; ordered 300 along +x at t=150
local TK = {}
for i, bp in { 'uea0102', 'uea0303', 'uea0101', 'uea0304', 'uea0103', 'uea0203' } do
    local x, z = 356, 897 + i * 7
    table.insert(TK, { tag = 'TK' .. bp, u = spawn(bp, A1, x, z, true), goal = { x + 300, 0, z } })
end
-- B: bombers 150 short of a target, attack order at t=5
local B = {}
for i, spec in { { 'uea0103', 904, 384, 0 }, { 'uaa0103', 656, 416, 0 }, { 'uea0103', 768, 232, 1 } } do
    local tx, tz = spec[2], spec[3]
    local t = spawn('ueb1201', A2, tx, tz)
    t:SetMaxHealth(1000000) t:SetHealth(nil, 1000000)
    local u = spawn(spec[1], A1, tx, tz - 150, true)
    u:SetIntelRadius('Vision', 70)
    u:SetFireState(spec[4])
    local b = { tag = 'B' .. spec[1] .. 'fs' .. spec[4], u = u, t = t, drops = 0 }
    for wi = 1, u:GetWeaponCount() do
        local wep = u:GetWeapon(wi)
        local create = wep.CreateProjectileAtMuzzle
        if create then
            wep.CreateProjectileAtMuzzle = function(self, muzzle)
                local p = create(self, muzzle)
                b.drops = b.drops + 1
                if b.drops <= 6 then
                    local up, tp = u:GetPosition(), t:GetPosition()
                    ev(b.tag, fmt('drop#%d dz=%.2f dx=%.2f alt=%.2f', b.drops, up[3] - tp[3], up[1] - tp[1], up[2] - tp[2]))
                end
                return p
            end
        end
    end
    table.insert(B, b)
end
-- D: gunship deaths. D0: made on the ground and killed at once (walls 0 and 3
-- ahead); D1: killed at cruise on a long move; D2: killed hovering 20 ticks
-- after a short move ends.
local D = {}
local function walls(tag, x, z)
    for k, dz in { 0, 3 } do
        local w = spawn('ueb5101', A1, x, z + dz)
        w:SetMaxHealth(100000) w:SetHealth(nil, 100000)
        w.OnDamage = function(self, inst, amt, vec, dt)
            ev(tag, fmt('wall%d damage %.1f %s', k, amt, tostring(dt)))
        end
    end
end
local function gunship(tag, x, z, air)
    local u = spawn('uea0203', A2, x, z, air)
    local d = { tag = tag, u = u, x0 = x, z0 = z }
    local oi = u.OnImpact
    u.OnImpact = function(self, with)
        local x1, y1, z1 = self:GetPositionXYZ()
        ev(tag, fmt('OnImpact %s at %.2f,%.2f', tostring(with), x1 - x, z1 - z))
        if oi then return oi(self, with) end
    end
    local ok = u.OnKilled
    u.OnKilled = function(self, a, b, c)
        ev(tag, 'OnKilled')
        if ok then return ok(self, a, b, c) end
    end
    table.insert(D, d)
    return d
end
walls('D0', 568, 440)
local D0 = gunship('D0', 568, 440, false)
local D1 = gunship('D1', 120, 608, true)
local D2 = gunship('D2', 176, 672, true)
walls('D2', 176, 702)

ForkThread(function()
    start = GetGameTick()
    for t = 0, TICKS do
        if t == 1 then
            ev('D0', 'kill ' .. st(D0.u))
            D0.u:Kill()
            D0.dying = t
        elseif t == 5 then
            for _, b in B do IssueAttack({ b.u }, b.t) end
            IssueMove({ D1.u }, { 120, 0, 908 })
            IssueMove({ D2.u }, { 176, 0, 702 })
        elseif t == 60 then
            for _, m in M do IssueMove({ m.u }, m.goal) end
        elseif t == 100 then
            ev('D1', 'kill ' .. st(D1.u)) D1.u:Kill() D1.dying = t
        elseif t == 150 then
            for _, k in TK do IssueMove({ k.u }, k.goal) end
        end
        if t == 140 and not D2.u:BeenDestroyed() then
            ev('D2', 'kill ' .. st(D2.u)) D2.u:Kill() D2.dying = t
        end
        for _, m in M do
            if t >= 58 and t <= 300 and (t <= 120 or math.mod(t, 5) == 0) then ev(m.tag, st(m.u)) end
        end
        for _, k in TK do
            if (t >= 148 and t <= 215) or t == 100 then ev(k.tag, st(k.u)) end
        end
        for _, b in B do
            if not b.u:BeenDestroyed() and not b.t:BeenDestroyed() then
                local up, tp = b.u:GetPosition(), b.t:GetPosition()
                local dz = up[3] - tp[3]
                if math.mod(t, 10) == 0 or math.abs(dz) < 40 then
                    ev(b.tag, st(b.u) .. fmt(' dz=%.2f alt=%.2f', dz, up[2] - tp[2]))
                end
            end
        end
        for _, d in D do
            if d.dying and not d.done then
                if d.u:BeenDestroyed() then
                    d.done = true ev(d.tag, 'gone')
                elseif t - d.dying <= 30 then
                    local x, y, z = d.u:GetPositionXYZ()
                    ev(d.tag, fmt('fall rel %.2f,%.2f ', x - d.x0, z - d.z0) .. st(d.u))
                end
            elseif not d.dying and math.mod(t, 10) == 0 and not d.u:BeenDestroyed() then
                ev(d.tag, st(d.u))
            end
        end
        WaitTicks(1)
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
