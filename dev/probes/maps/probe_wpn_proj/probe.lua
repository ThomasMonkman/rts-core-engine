-- Differential probe: raw projectile behaviour (kinematics, lifetime, impact
-- types, tracking, collision boxes), with projectiles made by
-- Entity:CreateProjectile so no weapon logic is involved.
--   real game (Windows): dev/probes/run_faf.ps1 probe_wpn_proj 60
--   ours: cargo run -- --headless --no-ai --ticks 200 --script dev/probes/maps/probe_wpn_proj/probe.lua
-- Lines: PROBE <case> ... ; positions are relative to the launcher's origin.
local fmt = string.format
local function P(s) LOG('PROBE ' .. s) end
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2, A3, NEU = armies[1], armies[2], armies[3], 'NEUTRAL_CIVILIAN'
SetAlliance(A1, A3, 'Ally') SetAlliance(A3, A1, 'Ally')
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')

local LAND = {
    {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232},
    {664,320}, {848,320}, {904,384}, {656,416}, {568,440}, {456,552}, {368,568}, {120,608},
    {176,672}, {360,672}, {256,760}, {88,776}, {248,856}, {352,856}, {104,896}, {408,896},
}
local WATER = { {144,144}, {240,144}, {272,272}, {368,304}, {656,592}, {752,592} }
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local GAUSS = '/projectiles/TDFGauss01/TDFGauss01_proj.bp'
local LASER = '/projectiles/CDFLaserHeavy01/CDFLaserHeavy01_proj.bp'
local ARTY = '/projectiles/TIFArtillery01/TIFArtillery01_proj.bp'
local TORP = '/projectiles/TANAnglerTorpedo01/TANAnglerTorpedo01_proj.bp'

local function make(bp, army, x, z, water)
    local y = water and GetSurfaceHeight(x, z) or GetTerrainHeight(x, z)
    local u = CreateUnitHPR(bp, army, x, y, z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    return u
end
local function name(e)
    if not e then return 'nil' end
    local ok, id = pcall(function() return e:GetBlueprint().BlueprintId end)
    return ok and tostring(id) or 'entity'
end
local function norm(x, y, z)
    local l = math.sqrt(x * x + y * y + z * z)
    return x / l, y / l, z / l
end

local tracked = {}
-- A projectile off `launcher` at offset o toward d; OnImpact replaced by a logger.
local function shoot(tag, launcher, bp, o, d, opts)
    opts = opts or {}
    local dx, dy, dz = norm(d[1], d[2], d[3])
    local p = launcher:CreateProjectile(bp, o[1], o[2], o[3], dx, dy, dz)
    local lp = launcher:GetPosition()
    p.T0 = GetGameTick()
    p.NI = 0
    p.OnImpact = function(self, tt, te)
        self.NI = self.NI + 1
        if not opts.quiet then
            local pos = self:GetPosition()
            P(fmt('%s impact#%d tick+%d type %s ent %s pos %.2f,%.2f,%.2f', tag, self.NI, GetGameTick() - self.T0, tostring(tt), name(te),
                pos[1] - lp[1], pos[2] - lp[2], pos[3] - lp[3]))
        end
        self.HitEnt = te
        self.HitType = tt
        if not opts.keep then self:Destroy() end
    end
    if opts.water then
        local ew, xw = p.OnEnterWater, p.OnExitWater
        p.OnEnterWater = function(self) P(fmt('%s OnEnterWater tick+%d', tag, GetGameTick() - self.T0)) end
        p.OnExitWater = function(self) P(fmt('%s OnExitWater tick+%d', tag, GetGameTick() - self.T0)) end
    end
    if opts.track then
        table.insert(tracked, { tag = tag, p = p, lp = lp, n = opts.track })
    end
    return p
end
local function logTracked()
    for _, t in tracked do
        if not t.gone then
            if t.p:BeenDestroyed() then
                t.gone = true
                P(fmt('%s gone tick+%d', t.tag, GetGameTick() - t.p.T0))
            elseif GetGameTick() - t.p.T0 <= t.n then
                local pos = t.p:GetPosition()
                local vx, vy, vz = t.p:GetVelocity()
                P(fmt('%s t+%d pos %.3f,%.3f,%.3f vel %.3f,%.3f,%.3f', t.tag, GetGameTick() - t.p.T0, pos[1] - t.lp[1], pos[2] - t.lp[2], pos[3] - t.lp[3],
                    vx or 0, vy or 0, vz or 0))
            end
        end
    end
end

ForkThread(function()
    WaitTicks(3)
    P('start')
    -- [kin] kinematics from 20 up, heading +x.
    local L = make('ueb5101', A1, LAND[1][1], LAND[1][2])
    local up = { 0, 20, 0 }
    shoot('kin.gauss', L, GAUSS, up, { 1, 0, 0 }, { track = 30 })
    shoot('kin.los', L, LOS, { 0, 20, 4 }, { 1, 0, 0 }, { track = 12 })
    shoot('kin.laser', L, LASER, { 0, 20, 8 }, { 1, 0, 0 }, { track = 30 })
    shoot('kin.arty', L, ARTY, { 0, 20, 12 }, { 1, 0, 0 }, { track = 30 })
    local p = shoot('kin.artyvel', L, ARTY, { 0, 20, 16 }, { 1, 0, 0 }, { track = 30 })
    p:SetVelocity(10, 5, 0)
    p = shoot('kin.artyball', L, ARTY, { 0, 20, 20 }, { 1, 0, 0 }, { track = 30 })
    p:SetVelocity(10, 5, 0) p:SetBallisticAcceleration(-2)
    p = shoot('kin.accel', L, LOS, { 0, 20, -4 }, { 1, 0, 0 }, { track = 14 })
    p:SetVelocity(2) p:SetAcceleration(10) p:SetMaxSpeed(9)
    p = shoot('kin.vel1', L, LOS, { 0, 20, -8 }, { 1, 0, 0 }, { track = 8 })
    p:SetVelocity(3)
    p = shoot('kin.vel3', L, LOS, { 0, 20, -12 }, { 1, 0, 0 }, { track = 8 })
    p:SetVelocity(0, 0, -6)
    p = shoot('kin.scale', L, LOS, { 0, 20, -16 }, { 1, 0, 0 }, { track = 8 })
    p:SetScaleVelocity(0.25)
    p = shoot('kin.diag', L, LOS, { 0, 20, -20 }, { 1, 1, 1 }, { track = 6 })
    p = shoot('kin.dir2', L, LOS, { 0, 20, -24 }, { 2, 0, 0 }, { track = 6 })
    p = shoot('kin.gaussdown', L, GAUSS, { 0, 6, -28 }, { 1, -0.2, 0 }, { track = 20 })
    p = shoot('kin.laserup', L, LASER, { 0, 1, -32 }, { 0.3, 1, 0 }, { track = 40 })

    -- [life] lifetime running out.
    local L2 = make('ueb5101', A1, LAND[2][1], LAND[2][2])
    p = shoot('life.set1.0', L2, LOS, { 0, 2, 0 }, { 0, 1, 0 }, { track = 14 }) p:SetLifetime(1.0)
    p = shoot('life.set0.25', L2, LOS, { 0, 2, 4 }, { 0, 1, 0 }, { track = 6 }) p:SetLifetime(0.25)
    p = shoot('life.set0', L2, LOS, { 0, 2, 8 }, { 0, 1, 0 }, { track = 6 }) p:SetLifetime(0)
    p = shoot('life.keep', L2, LOS, { 0, 2, 12 }, { 0, 1, 0 }, { track = 16, keep = true }) p:SetLifetime(1.0)
    p = shoot('life.bp2.5', L2, LASER, { 0, 300, 16 }, { 1, 0, 0 }, { track = 3 })
    p = shoot('life.losdefault', L2, LOS, { 0, 2, 20 }, { 0, 1, 0 }, { track = 2 }) p:SetVelocity(0.5)
    p = shoot('life.gaussdefault', L2, GAUSS, { 0, 2, 24 }, { 0, 1, 0 }, { track = 2 }) p:SetVelocity(0, 0.5, 0) p:SetBallisticAcceleration(0)
    -- a surface impact the script survives
    p = shoot('life.groundkeep', L2, LOS, { 0, 3, -6 }, { 1, -1, 0 }, { track = 12, keep = true }) p:SetLifetime(1.0)

    -- [imp] impact types on land.
    local L3 = make('ueb5101', A1, LAND[3][1], LAND[3][2])
    local x3, z3 = LAND[3][1], LAND[3][2]
    make('ueb5101', A2, x3 + 10, z3)
    shoot('imp.enemy', L3, LOS, { 1.5, 0.3, 0 }, { 1, 0, 0 })
    make('ueb5101', A1, x3 - 5, z3) make('ueb5101', A2, x3 - 10, z3)
    shoot('imp.friend-then-enemy', L3, LOS, { -1.5, 0.3, 0 }, { -1, 0, 0 })
    make('ueb5101', A3, x3, z3 + 5) make('ueb5101', A2, x3, z3 + 10)
    shoot('imp.ally-then-enemy', L3, LOS, { 0, 0.3, 1.5 }, { 0, 0, 1 })
    make('ueb5101', NEU, x3, z3 - 5) make('ueb5101', A2, x3, z3 - 10)
    shoot('imp.neutral-then-enemy', L3, LOS, { 0, 0.3, -1.5 }, { 0, 0, -1 })
    shoot('imp.terrain', L3, LOS, { 3, 3, 3 }, { 1, -1, 1 })
    shoot('imp.self', L3, LOS, { 0, 4, 0 }, { 0, -1, 0 })
    shoot('imp.inside', L3, LOS, { 0, 0.2, 0 }, { 1, 0, 1 })
    local L4 = make('ueb5101', A1, LAND[4][1], LAND[4][2])
    local gs = make('uea0203', A2, LAND[4][1] + 10, LAND[4][2])
    local tk = make('uel0201', A2, LAND[4][1] - 10, LAND[4][2])
    local wreckTank = make('uel0201', A2, LAND[4][1], LAND[4][2] + 10)
    wreckTank:Kill()
    local prop = CreateProp({ LAND[4][1], GetTerrainHeight(LAND[4][1], LAND[4][2] - 10), LAND[4][2] - 10 }, '/env/evergreen/props/rocks/rock01_prop.bp')
    local tree = CreateProp({ LAND[4][1] + 7, GetTerrainHeight(LAND[4][1] + 7, LAND[4][2] - 7), LAND[4][2] - 7 }, '/env/evergreen/props/trees/pine06_prop.bp')

    -- [trk] tracking.
    local L5 = make('ueb5101', A1, LAND[5][1], LAND[5][2])
    local x5, z5 = LAND[5][1], LAND[5][2]
    local t5a = make('ueb5101', A2, x5 + 15, z5 + 15)
    p = shoot('trk.side90', L5, LOS, { 0, 6, 0 }, { 1, 0, 0 }, { track = 45 })
    p:SetVelocity(8) p:SetTurnRate(90) p:SetNewTarget(t5a) p:TrackTarget(true)
    local t5b = make('ueb5101', A2, x5 - 20, z5)
    p = shoot('trk.behind30', L5, LOS, { 0, 6, -4 }, { 1, 0, 0 }, { track = 45 })
    p:SetVelocity(8) p:SetTurnRate(30) p:SetNewTarget(t5b) p:TrackTarget(true) p:SetLifetime(6)
    p = shoot('trk.ground', L5, LOS, { 0, 6, 4 }, { 1, 0, 0 }, { track = 45 })
    p:SetVelocity(8) p:SetTurnRate(120) p:SetNewTargetGround({ x5 + 6, GetTerrainHeight(x5 + 6, z5 - 14), z5 - 14 }) p:TrackTarget(true)
    p = shoot('trk.notrack', L5, LOS, { 0, 6, 8 }, { 1, 0, 0 }, { track = 10 })
    p:SetVelocity(8) p:SetTurnRate(90) p:SetNewTarget(t5a) p:SetLifetime(3)
    p = shoot('trk.up360', L5, LOS, { 0, 2, 12 }, { 0, 1, 0 }, { track = 30 })
    p:SetVelocity(8) p:SetTurnRate(360) p:SetNewTarget(t5a) p:TrackTarget(true)
    P(fmt('trk targetpos %s', tostring(p:GetCurrentTargetPosition() and fmt('%.2f,%.2f,%.2f', p:GetCurrentTargetPosition()[1] - x5, p:GetCurrentTargetPosition()[2] - L5:GetPosition()[2], p:GetCurrentTargetPosition()[3] - z5))))

    -- [sea] water cases: the launcher is a frigate.
    local W = make('ues0103', A1, WATER[1][1], WATER[1][2], true)
    local wx, wz = WATER[1][1], WATER[1][2]
    shoot('sea.water', W, LOS, { 6, 3, 0 }, { 1, -1, 0 }, { water = true })
    shoot('sea.gausswater', W, GAUSS, { 6, 3, 4 }, { 1, -1, 0 }, { water = true })
    p = shoot('sea.nodestroy', W, LOS, { 6, 3, 8 }, { 1, -1, 0 }, { water = true, track = 14 }) p:SetDestroyOnWater(false) p:SetLifetime(1.2)
    p = shoot('sea.torpup', W, TORP, { 6, -3, -4 }, { 1, 0.6, 0 }, { water = true, track = 30 }) p:SetLifetime(3)
    p = shoot('sea.torpair', W, TORP, { 6, 3, -8 }, { 1, 0, 0 }, { water = true, track = 30 }) p:SetLifetime(3)
    p = shoot('sea.torpstayoff', W, TORP, { 6, -3, -12 }, { 1, 0.6, 0 }, { water = true, track = 30 }) p:StayUnderwater(false) p:SetLifetime(3)
    local ship = make('ues0103', A2, wx - 14, wz, true)
    shoot('sea.ship', W, LOS, { -4, 0.4, 0 }, { -1, 0, 0 })
    local sub = make('ues0203', A2, wx, wz + 20, true)

    for t = 1, 12 do WaitTicks(1) logTracked() end
    -- later shots: the gunship has climbed, the sub has dived, the wreck exists
    local gp, lp = gs:GetPosition(), L4:GetPosition()
    P(fmt('imp gunship rel %.2f,%.2f,%.2f layer %s', gp[1] - lp[1], gp[2] - lp[2], gp[3] - lp[3], gs:GetCurrentLayer()))
    shoot('imp.air', L4, LOS, { 0, 1, 0 }, { gp[1] - lp[1], gp[2] - lp[2] - 1, gp[3] - lp[3] })
    shoot('imp.tank', L4, LOS, { -1.5, 0.25, 0 }, { -1, 0, 0 })
    shoot('imp.wreck', L4, LOS, { 0, 0.25, 1.5 }, { 0, 0, 1 }, { }) 
    shoot('imp.rock', L4, LOS, { 0, 0.25, -1.5 }, { 0, 0, -1 })
    shoot('imp.tree', L4, LOS, { 1.5, 0.5, -1.5 }, { 1, 0, -1 })
    local sp, wp = sub:GetPosition(), W:GetPosition()
    P(fmt('sea sub rel %.2f,%.2f,%.2f layer %s', sp[1] - wp[1], sp[2] - wp[2], sp[3] - wp[3], sub:GetCurrentLayer()))
    p = shoot('sea.sub', W, TORP, { 0, -1.5, 8 }, { 0, sp[2] - wp[2] + 1.5, sp[3] - wp[3] - 8 }, { track = 20 })
    p = shoot('sea.subtrack', W, TORP, { 4, -1.5, 8 }, { 1, 0, 0 }, { track = 30 }) p:SetNewTarget(sub) p:SetLifetime(5)
    for t = 1, 45 do WaitTicks(1) logTracked() end

    -- [box] collision shape scans with straight shots along +x.
    local boxUnits = { 'uel0201', 'ueb1101', 'uel0001', 'url0402', 'ueb0101', 'ueb5101', 'xsl0401', 'uel0401' }
    local boxes = {}
    for i, bp in boxUnits do
        local st = LAND[5 + i]
        local Lb = make('ueb5101', A1, st[1], st[2])
        local u = make(bp, A2, st[1] + 12, st[2] + 12)
        local b = u:GetBlueprint()
        local e = { bp = bp, u = u, hs = {}, zs = {}, vs = {} }
        local sy, sz, sx = b.SizeY or 1, b.SizeZ or 1, b.SizeX or 1
        local midY = (b.CollisionOffsetY or 0) + sy / 2
        local h = 0.05
        while h <= sy + 1.2 do
            table.insert(e.hs, { h, shoot('box', Lb, LOS, { 2, h, 12 }, { 1, 0, 0 }, { quiet = true }) })
            h = h + 0.1
        end
        local dz = -sz / 2 - 0.85
        while dz <= sz / 2 + 0.85 do
            table.insert(e.zs, { dz, shoot('box', Lb, LOS, { 2, math.max(midY, 0.15), 12 + dz }, { 1, 0, 0 }, { quiet = true }) })
            dz = dz + 0.1
        end
        -- from above, straight down, along x through the centre
        local dx = -sx / 2 - 0.85
        while dx <= sx / 2 + 0.85 do
            table.insert(e.vs, { dx, shoot('box', Lb, LOS, { 12 + dx, sy + 6, 12 }, { 0, -1, 0 }, { quiet = true }) })
            dx = dx + 0.1
        end
        table.insert(boxes, e)
    end
    WaitTicks(25)
    for _, e in boxes do
        local b = e.u:GetBlueprint()
        P(fmt('box %s size %s %s %s offY %s', e.bp, tostring(b.SizeX), tostring(b.SizeY), tostring(b.SizeZ), tostring(b.CollisionOffsetY)))
        for _, set in { { 'h', e.hs }, { 'z', e.zs }, { 'topx', e.vs } } do
            local line = fmt('box %s %s', e.bp, set[1])
            local prev
            for _, s in set[2] do
                local hit = s[2].HitEnt == e.u and 'unit' or tostring(s[2].HitType)
                if hit ~= prev then line = line .. fmt(' %s@%.2f', hit, s[1]) end
                prev = hit
            end
            P(line)
        end
    end
    WaitTicks(30)
    P('done')
end)
