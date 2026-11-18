-- Differential probe: collision shapes set from Lua, projectile collision
-- flags, detonate heights, child projectiles, collision-check callbacks,
-- damage to projectiles, weapon projectile lifetimes, moving targets.
--   real game (Windows): dev/probes/run_faf.ps1 probe_wpn_proj2 90
--   ours: cargo run -- --headless --no-ai --ticks 420 --script dev/probes/maps/probe_wpn_proj2/probe.lua
-- Lines: PROBE <case> ... ; positions are relative to the launcher's origin.
local fmt = string.format
local function P(s) LOG('PROBE ' .. s) end
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
local LAND = {
    {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232},
    {664,320}, {848,320}, {904,384}, {656,416}, {568,440}, {456,552}, {368,568}, {120,608},
    {176,672}, {360,672}, {256,760}, {88,776}, {248,856}, {352,856}, {104,896}, {408,896},
}
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local MISSILE = '/projectiles/TIFMissileCruise01/TIFMissileCruise01_proj.bp'
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    return u
end
local function name(e)
    if not e then return 'nil' end
    local ok, id = pcall(function() return e:GetBlueprint().BlueprintId end)
    return ok and tostring(id) or 'entity'
end
local function shoot(tag, launcher, bp, o, d, opts)
    opts = opts or {}
    local l = math.sqrt(d[1] * d[1] + d[2] * d[2] + d[3] * d[3])
    local p = launcher:CreateProjectile(bp, o[1], o[2], o[3], d[1] / l, d[2] / l, d[3] / l)
    local lp = launcher:GetPosition()
    p.T0 = GetGameTick()
    p.OnImpact = function(self, tt, te)
        if not opts.quiet then
            local pos = self:GetPosition()
            P(fmt('%s impact tick+%d type %s ent %s pos %.2f,%.2f,%.2f', tag, GetGameTick() - self.T0, tostring(tt), name(te), pos[1] - lp[1], pos[2] - lp[2], pos[3] - lp[3]))
        end
        self.HitEnt = te
        self.HitType = tt
        self:Destroy()
    end
    return p
end
local function hook(obj, nm, note)
    local mine
    mine = function(self, a, b, c, d, e)
        note(self, a, b, c, d, e)
        self[nm] = nil
        local r = self[nm](self, a, b, c, d, e)
        self[nm] = mine
        return r
    end
    obj[nm] = mine
end

ForkThread(function()
    WaitTicks(3)
    P('start')
    -- [shape] SetCollisionShape on a wall, scanned with straight shots along +x
    local shapes = {
        { 'default', function(u) end },
        { 'sphere3', function(u) u:SetCollisionShape('Sphere', 0, 0, 0, 3) end },
        { 'sphere2up1', function(u) u:SetCollisionShape('Sphere', 0, 1, 0, 2) end },
        { 'box1x2x3', function(u) u:SetCollisionShape('Box', 0, 0, 0, 1, 2, 3) end },
        { 'box-off', function(u) u:SetCollisionShape('Box', 0, 2, 1, 0.5, 0.5, 0.5) end },
        { 'none', function(u) u:SetCollisionShape('None') end },
    }
    local scans = {}
    for i, sdef in shapes do
        local st = LAND[i]
        local Lb = make('ueb5101', A1, st[1], st[2])
        local u = make('ueb5101', A2, st[1] + 12, st[2] + 12)
        local ok = pcall(sdef[2], u)
        local e = { tag = sdef[1], u = u, ok = ok, hs = {}, zs = {} }
        local h = 0.05
        while h <= 5 do
            table.insert(e.hs, { h, shoot('s', Lb, LOS, { 2, h, 12 }, { 1, 0, 0 }, { quiet = true }) })
            h = h + 0.1
        end
        local dz = -4.45
        while dz <= 4.5 do
            table.insert(e.zs, { dz, shoot('s', Lb, LOS, { 2, 0.25, 12 + dz }, { 1, 0, 0 }, { quiet = true }) })
            dz = dz + 0.1
        end
        table.insert(scans, e)
    end
    -- [flag] collision flags
    local L = make('ueb5101', A1, LAND[7][1], LAND[7][2])
    make('ueb5101', A2, LAND[7][1] + 10, LAND[7][2])
    make('ueb5101', A2, LAND[7][1] - 10, LAND[7][2])
    make('ueb5101', A2, LAND[7][1], LAND[7][2] + 10)
    local p
    p = shoot('flag.noentity', L, LOS, { 1.5, 0.3, 0 }, { 1, 0, 0 }) p:SetCollideEntity(false) p:SetLifetime(2)
    p = shoot('flag.nocollision', L, LOS, { -1.5, 0.3, 0 }, { -1, 0, 0 }) p:SetCollision(false) p:SetLifetime(2)
    p = shoot('flag.nosurface-unit', L, LOS, { 0, 0.3, 1.5 }, { 0, 0, 1 }) p:SetCollideSurface(false) p:SetLifetime(2)
    p = shoot('flag.nosurface-ground', L, LOS, { 3, 3, -3 }, { 1, -1, -1 }) p:SetCollideSurface(false) p:SetLifetime(1.5)
    p = shoot('flag.nocollision-ground', L, LOS, { 3, 3, -6 }, { 1, -1, -1 }) p:SetCollision(false) p:SetLifetime(1.5)
    p = shoot('flag.noentity-ground', L, LOS, { 3, 3, -9 }, { 1, -1, -1 }) p:SetCollideEntity(false) p:SetLifetime(1.5)
    -- [det] detonate heights
    local L2 = make('ueb5101', A1, LAND[8][1], LAND[8][2])
    p = shoot('det.below5', L2, LOS, { 4, 20, 0 }, { 0, -1, 0 }) p:ChangeDetonateBelowHeight(5)
    p = shoot('det.below5-slow', L2, LOS, { 4, 20, 4 }, { 0, -1, 0 }) p:SetVelocity(7) p:ChangeDetonateBelowHeight(5)
    p = shoot('det.above12', L2, LOS, { 4, 2, 8 }, { 0, 1, 0 }) p:ChangeDetonateAboveHeight(12)
    p = shoot('det.above12-start-above', L2, LOS, { 4, 15, 12 }, { 1, 0, 0 }) p:ChangeDetonateAboveHeight(12) p:SetLifetime(2)
    p = shoot('det.below5-start-below', L2, LOS, { 4, 2, 16 }, { 1, 0.2, 0 }) p:ChangeDetonateBelowHeight(5) p:SetLifetime(2)
    -- [child] child projectiles
    do
        local parent = shoot('child.parent', L2, LOS, { 0, 10, -8 }, { 1, 0, 0 }) parent:SetVelocity(6) parent:SetLifetime(3)
        local c = parent:CreateChildProjectile(LOS)
        local pp, cp = parent:GetPosition(), c:GetPosition()
        local pvx, pvy, pvz = parent:GetVelocity()
        local cvx, cvy, cvz = c:GetVelocity()
        P(fmt('child at birth: dpos %.2f,%.2f,%.2f parentvel %.2f,%.2f,%.2f childvel %.2f,%.2f,%.2f launcher %s', cp[1] - pp[1], cp[2] - pp[2], cp[3] - pp[3],
            pvx, pvy, pvz, cvx, cvy, cvz, tostring(c:GetLauncher() == L2)))
        local lp = L2:GetPosition()
        local t0 = GetGameTick()
        c.OnImpact = function(self, tt, te)
            local pos = self:GetPosition()
            P(fmt('child impact tick+%d type %s pos %.2f,%.2f,%.2f', GetGameTick() - t0, tostring(tt), pos[1] - lp[1], pos[2] - lp[2], pos[3] - lp[3]))
            self:Destroy()
        end
        ForkThread(function()
            for t = 1, 4 do
                WaitTicks(1)
                if not c:BeenDestroyed() then
                    local a, b = parent:GetPosition(), c:GetPosition()
                    P(fmt('child t+%d parent %.2f,%.2f child %.2f,%.2f', t, a[1] - lp[1], a[2] - lp[2], b[1] - lp[1], b[2] - lp[2]))
                end
            end
        end)
    end
    -- [check] collision-check callbacks on a unit and between projectiles
    local L3 = make('ueb5101', A1, LAND[9][1], LAND[9][2])
    do
        local x, z = LAND[9][1], LAND[9][2]
        local function wall(tag, army, dx, dz, result)
            local w = make('ueb5101', army, x + dx, z + dz)
            w.N = 0
            w.OnCollisionCheck = function(self, other, weapon)
                self.N = self.N + 1
                if self.N == 1 then P(fmt('check %s OnCollisionCheck other %s weapon %s -> %s', tag, name(other), tostring(weapon), tostring(result))) end
                return result
            end
            return w
        end
        wall('enemy-false', A2, 6, 0, false) wall('enemy-behind', A2, 12, 0, true)
        shoot('check.enemy-false', L3, LOS, { 1.5, 0.3, 0 }, { 1, 0, 0 })
        wall('own-true', A1, -6, 0, true)
        shoot('check.own-true', L3, LOS, { -1.5, 0.3, 0 }, { -1, 0, 0 })
        wall('enemy-nil', A2, 0, 6, nil) wall('enemy-behind2', A2, 0, 12, true)
        shoot('check.enemy-nil', L3, LOS, { 0, 0.3, 1.5 }, { 0, 0, 1 })
        L3.N = 0
        L3.OnCollisionCheck = function(self, other, weapon) self.N = self.N + 1 return true end
        shoot('check.launcher', L3, LOS, { 0, 4, 0 }, { 0, -1, 0 })
        -- two enemy projectiles head on, with collision checks that say yes
        local E = make('ueb5101', A2, x + 20, z - 12)
        local a = shoot('check.projA', L3, LOS, { 2, 3, -12 }, { 1, 0, 0 }) a:SetVelocity(8) a:SetLifetime(2)
        local b = shoot('check.projB', E, LOS, { -2, 3, 0 }, { -1, 0, 0 }) b:SetVelocity(8) b:SetLifetime(2)
        a.OnCollisionCheck = function(self, other) P('check projA OnCollisionCheck other ' .. name(other)) return true end
        b.OnCollisionCheck = function(self, other) P('check projB OnCollisionCheck other ' .. name(other)) return true end
        -- a missile (has health) against a plain shot
        local m = shoot('check.missile', E, MISSILE, { -2, 6, 0 }, { -1, 0, 0 }) m:SetLifetime(3)
        local s = shoot('check.vs-missile', L3, LOS, { 2, 6, -12 }, { 1, 0, 0 }) s:SetVelocity(8) s:SetLifetime(2)
        m.OnCollisionCheck = function(self, other) P('check missile OnCollisionCheck other ' .. name(other)) return true end
        s.OnCollisionCheck = function(self, other) P('check shot OnCollisionCheck other ' .. name(other)) return true end
    end
    -- [pdmg] damage to projectiles
    do
        local L4 = make('ueb5101', A1, LAND[10][1], LAND[10][2])
        local function victim(tag, bp, dz)
            local v = shoot('pdmg.' .. tag, L4, bp, { 0, 8, dz }, { 0, 1, 0 }) v:SetVelocity(0.5) v:SetLifetime(4)
            v.D = 0
            v.OnDamage = function(self, inst, amt, vec, t) self.D = self.D + 1 self.A = amt end
            return v
        end
        local a, b = victim('los', LOS, 0), victim('missile', MISSILE, 6)
        local ap, bp = a:GetPosition(), b:GetPosition()
        P(fmt('pdmg health los %s/%s missile %s/%s', tostring(a:GetHealth()), tostring(a:GetMaxHealth()), tostring(b:GetHealth()), tostring(b:GetMaxHealth())))
        Damage(nil, ap, a, 5, 'Normal') Damage(nil, bp, b, 5, 'Normal')
        P(fmt('pdmg Damage: los calls %d missile calls %d amt %s health %s', a.D, b.D, tostring(b.A), tostring(b:GetHealth())))
        a.D = 0 b.D = 0
        DamageArea(nil, ap, 2, 5, 'Normal', true, true) DamageArea(nil, bp, 2, 5, 'Normal', true, true)
        P(fmt('pdmg DamageArea: los calls %d missile calls %d', a.D, b.D))
    end
    WaitTicks(40)
    for _, e in scans do
        for _, set in { { 'h', e.hs }, { 'z', e.zs } } do
            local line = fmt('shape %s ok %s %s', e.tag, tostring(e.ok), set[1])
            local prev
            for _, s in set[2] do
                local hit = s[2].HitEnt == e.u and 'unit' or tostring(s[2].HitType)
                if hit ~= prev then line = line .. fmt(' %s@%.2f', hit, s[1]) end
                prev = hit
            end
            P(line)
        end
    end
    P(fmt('check launcher calls %d', L3.N))
    for _, brain in ArmyBrains do
        for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
            if not u:BeenDestroyed() then u:Destroy() end
        end
    end
    WaitTicks(5)

    -- [life] weapon projectile lifetimes: shots that collide with nothing
    local lifes = {}
    for i, bp in { 'uel0201', 'ueb2101', 'ual0201', 'url0107', 'uel0103', 'xsl0201', 'uel0303', 'uel0111', 'ueb2301', 'uel0104', 'ual0106', 'urb2101' } do
        local st = LAND[i]
        local u = make(bp, A1, st[1], st[2])
        u:SetIntelRadius('Vision', 70)
        local wb = u:GetWeapon(1):GetBlueprint()
        local d = math.min(38, (wb.MaxRadius or 10) * 0.6)
        local air = wb.RangeCategory == 'UWRC_AntiAir'
        local t = make(air and 'uea0203' or 'ueb1201', A2, st[1], st[2] + d)
        t:SetCanTakeDamage(false)
        local e = { bp = bp, lifes = {}, wb = wb }
        local wep = u:GetWeapon(1)
        local create = wep.CreateProjectileAtMuzzle
        wep.CreateProjectileAtMuzzle = function(self, muzzle)
            local pr = create(self, muzzle)
            if type(pr) == 'table' and table.getn(e.lifes) < 2 and not e.busy then
                e.busy = true
                local born = GetGameTick()
                local sp = pr:GetPosition()
                pr:SetCollision(false)
                local oi = pr.OnImpact
                pr.OnImpact = function(pp, tt, te)
                    local pos = pp:GetPosition()
                    local dx, dz = pos[1] - sp[1], pos[3] - sp[3]
                    table.insert(e.lifes, fmt('%d:%s:%.1f', GetGameTick() - born, tostring(tt), math.sqrt(dx * dx + dz * dz)))
                    e.busy = false
                    pp:Destroy()
                end
            end
            return pr
        end
        table.insert(lifes, e)
        e.u, e.t = u, t
    end
    -- [lead] turrets against a target crossing at speed
    local leads = {}
    for i, bp in { 'ueb2101', 'uab2101', 'ueb2301', 'uel0201', 'xsb2101', 'uel0103' } do
        local st = LAND[12 + i]
        local u = make(bp, A1, st[1], st[2])
        u:SetIntelRadius('Vision', 70)
        local t = make('uel0101', A2, st[1] - 7, st[2] + 14)
        local e = { bp = bp, u = u, t = t, hits = 0, shots = 0 }
        t.OnDamage = function(self, inst, amt) e.hits = e.hits + 1 end
        local wep = u:GetWeapon(1)
        local create = wep.CreateProjectileAtMuzzle
        wep.CreateProjectileAtMuzzle = function(self, muzzle) e.shots = e.shots + 1 return create(self, muzzle) end
        table.insert(leads, e)
    end
    WaitTicks(15)
    for _, e in lifes do e.u:SetFireState(0) IssueAttack({ e.u }, e.t) end
    for _, e in leads do
        e.u:SetFireState(0) IssueAttack({ e.u }, e.t)
        local tp = e.t:GetPosition()
        IssueMove({ e.t }, { tp[1] + 14, tp[2], tp[3] })
        IssueMove({ e.t }, { tp[1], tp[2], tp[3] })
        IssueMove({ e.t }, { tp[1] + 14, tp[2], tp[3] })
    end
    WaitTicks(170)
    for _, e in lifes do
        P(fmt('life %s lifetime %s mult %s maxradius %s muzzlevel %s -> %s %s', e.bp, tostring(e.wb.ProjectileLifetime), tostring(e.wb.ProjectileLifetimeUsesMultiplier), tostring(e.wb.MaxRadius),
            tostring(e.wb.MuzzleVelocity), e.lifes[1] or '-', e.lifes[2] or '-'))
    end
    for _, e in leads do
        local tp = e.t:GetPosition()
        P(fmt('lead %s shots %d hits %d', e.bp, e.shots, e.hits))
    end
    WaitTicks(30)
    P('done')
end)
