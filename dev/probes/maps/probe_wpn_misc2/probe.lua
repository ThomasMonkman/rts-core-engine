-- Differential probe: follow-ups. Is GetPosition's table shared between
-- calls; the default projectile lifetime; projectiles made under water or
-- tracking with / without a target; child projectile speed; Kill arguments.
--   real game (Windows): dev/probes/run_faf.ps1 probe_wpn_misc2 60
--   ours: cargo run -- --headless --no-ai --ticks 230 --script dev/probes/maps/probe_wpn_misc2/probe.lua
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
local LAND = { {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208} }
local WATER = { {144,144}, {240,144} }
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local MISSILE = '/projectiles/TIFMissileCruise01/TIFMissileCruise01_proj.bp'
local TORP = '/projectiles/TANAnglerTorpedo01/TANAnglerTorpedo01_proj.bp'
local AAM = '/projectiles/TAAMissileFlayer01/TAAMissileFlayer01_proj.bp'
local function make(bp, army, x, z, water)
    local y = water and GetSurfaceHeight(x, z) or GetTerrainHeight(x, z)
    local u = CreateUnitHPR(bp, army, x, y, z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    return u
end
-- a projectile with every callback logged; positions kept as numbers
local function proj(tag, launcher, bp, o, d)
    local p = launcher:CreateProjectile(bp, o[1], o[2], o[3], d[1], d[2], d[3])
    local t0 = GetGameTick()
    local lp = launcher:GetPosition()
    local lx, ly, lz = lp[1], lp[2], lp[3]
    for _, cb in { 'OnImpact', 'OnLostTarget', 'OnDestroy', 'OnEnterWater', 'OnExitWater' } do
        local cbn = cb
        p[cb] = function(self, a, b)
            local x, y, z = self:GetPositionXYZ()
            P(fmt('%s %s(%s) tick+%d at %.2f,%.2f,%.2f', tag, cbn, tostring(a), GetGameTick() - t0, x - lx, y - ly, z - lz))
            if cbn == 'OnImpact' then self:Destroy() end
        end
    end
    return p
end

ForkThread(function()
    WaitTicks(3)
    P('start')
    local L = make('ueb5101', A1, LAND[1][1], LAND[1][2])
    -- [tbl] is GetPosition's table the same object on every call?
    do
        local p = proj('tbl.p', L, LOS, { 0, 5, 0 }, { 0, 1, 0 }) p:SetVelocity(1)
        local tank = make('uel0201', A1, LAND[2][1], LAND[2][2])
        local a, ua = p:GetPosition(), tank:GetPosition()
        local ay, ux = a[2], ua[1]
        local a2 = p:GetPosition()
        IssueMove({ tank }, { LAND[2][1] + 10, 0, LAND[2][2] })
        WaitTicks(12)
        local b, ub = p:GetPosition(), tank:GetPosition()
        P(fmt('tbl projectile same-call-same-table %s later-same-table %s first.y then %.2f now %.2f fresh.y %.2f', tostring(a == a2), tostring(a == b), ay, a[2], b[2]))
        P(fmt('tbl unit later-same-table %s first.x then %.2f now %.2f fresh.x %.2f', tostring(ua == ub), ux, ua[1], ub[1]))
        p:Destroy()
    end
    -- [life] no Lifetime in the blueprint: straight up until something ends it
    proj('life.default', L, LOS, { 0, 5, 4 }, { 0, 1, 0 }):SetVelocity(1)
    local ph = __blueprints[string.lower(LOS)].Physics
    P(fmt('life bp Lifetime %s', tostring(ph.Lifetime)))
    -- [trk] tracking blueprints with a target given at once
    local T = make('ueb5101', A2, LAND[1][1] + 25, LAND[1][2] + 12)
    T:SetMaxHealth(100000) T:SetHealth(nil, 100000)
    local p1 = proj('trk.missile-target', L, MISSILE, { 0, 8, 8 }, { 1, 0, 0 }) p1:SetNewTarget(T)
    local p2 = proj('trk.missile-ground', L, MISSILE, { 0, 8, 12 }, { 1, 0, 0 }) p2:SetNewTargetGround({ LAND[1][1] + 25, GetTerrainHeight(LAND[1][1] + 25, LAND[1][2] + 12), LAND[1][2] + 12 })
    local p3 = proj('trk.aam-target', L, AAM, { 0, 8, 16 }, { 1, 0, 0 }) p3:SetNewTarget(T)
    local p4 = proj('trk.aam-none', L, AAM, { 0, 8, 20 }, { 1, 0, 0 })
    local p5 = proj('trk.los-trackon-none', L, LOS, { 0, 8, 24 }, { 1, 0, 0 }) p5:TrackTarget(true) p5:SetLifetime(2)
    local p6 = proj('trk.aam-trackoff-none', L, AAM, { 0, 8, 28 }, { 1, 0, 0 }) p6:TrackTarget(false)
    -- [sea] made under water
    local W = make('ues0103', A1, WATER[1][1], WATER[1][2], true)
    local S = make('ues0103', A2, WATER[1][1] + 20, WATER[1][2], true)
    S:SetMaxHealth(100000) S:SetHealth(nil, 100000)
    proj('sea.los-under', W, LOS, { 5, -2, 0 }, { 1, 0, 0 }):SetLifetime(2)
    local q = proj('sea.los-under-nodestroy', W, LOS, { 5, -2, 4 }, { 1, 0, 0 }) q:SetDestroyOnWater(false) q:SetLifetime(2)
    proj('sea.torp-under-none', W, TORP, { 5, -2, 8 }, { 1, 0, 0 })
    local q2 = proj('sea.torp-under-target', W, TORP, { 5, -2, -6 }, { 1, 0, 0 }) q2:SetNewTarget(S)
    local q3 = proj('sea.torp-air-target', W, TORP, { 5, 2, -10 }, { 1, 0, 0 }) q3:SetNewTarget(S)
    -- [child] child speed, measured with numbers
    do
        local parent = proj('child.parent', L, LOS, { 0, 12, -8 }, { 1, 0, 0 }) parent:SetVelocity(6) parent:SetLifetime(3)
        WaitTicks(3)
        for _, def in { { 'los', LOS }, { 'gauss', '/projectiles/TDFGauss01/TDFGauss01_proj.bp' } } do
            local c = parent:CreateChildProjectile(def[2])
            local tag = 'child.' .. def[1]
            c.OnImpact = function(self) self:Destroy() end
            ForkThread(function()
                local px, py, pz = c:GetPositionXYZ()
                for t = 1, 3 do
                    WaitTicks(1)
                    if c:BeenDestroyed() then P(tag .. ' destroyed at t+' .. t) return end
                    local x, y, z = c:GetPositionXYZ()
                    P(fmt('%s t+%d step %.3f,%.3f,%.3f', tag, t, x - px, y - py, z - pz))
                    px, py, pz = x, y, z
                end
                c:Destroy()
            end)
        end
    end
    -- [kill] Kill's arguments and the dead unit
    do
        local x, z = LAND[3][1], LAND[3][2]
        local inst = make('ueb5101', A1, x - 10, z)
        local function victim(tag, dx)
            local u = make('uel0201', A2, x + dx, z)
            u.K = 0 u.D = 0
            local ok = u.OnKilled
            u.OnKilled = function(self, i, t, over)
                self.K = self.K + 1
                P(fmt('kill %s OnKilled inst %s type [%s] over %s health %.1f Dead %s IsDead %s', tag, i == inst and 'inst' or tostring(i), tostring(t), tostring(over), self:GetHealth(),
                    tostring(self.Dead), tostring(self:IsDead())))
                return ok(self, i, t, over)
            end
            local od = u.OnDamage
            u.OnDamage = function(self, i, a, v, t) self.D = self.D + 1 return od(self, i, a, v, t) end
            return u
        end
        local a = victim('args', 0)
        a:Kill(inst, 'Fire', 2.5)
        P(fmt('kill args after: health %.1f IsDead %s K %d', a:GetHealth(), tostring(a:IsDead()), a.K))
        Damage(inst, a:GetPosition(), a, 10, 'Normal')
        DamageArea(inst, a:GetPosition(), 2, 10, 'Normal', true, true)
        P(fmt('kill args damage-after-death OnDamage calls %d', a.D))
        local b = victim('noargs', 6)
        b:Kill()
        P(fmt('kill noargs after: health %.1f K %d', b:GetHealth(), b.K))
        local c = victim('cantkill', 12)
        c:SetCanBeKilled(false)
        c:Kill(inst, 'Normal', 0)
        P(fmt('kill cantkill after Kill: health %.1f IsDead %s K %d', c:GetHealth(), tostring(c:IsDead()), c.K))
        Damage(inst, c:GetPosition(), c, 5000, 'Normal')
        P(fmt('kill cantkill after lethal damage: health %.1f IsDead %s K %d D %d', c:GetHealth(), tostring(c:IsDead()), c.K, c.D))
        c:SetCanBeKilled(true)
        Damage(inst, c:GetPosition(), c, 1, 'Normal')
        P(fmt('kill cantkill re-enabled +1 damage: health %.1f IsDead %s K %d D %d', c:GetHealth(), tostring(c:IsDead()), c.K, c.D))
        -- self-damage flags with the instigator inside the area
        local me = make('ueb5101', A1, x, z + 20)
        local mate = make('ueb5101', A1, x + 2, z + 20)
        local foe = make('ueb5101', A2, x - 2, z + 20)
        local hits = {}
        for tag, u in { me = me, mate = mate, foe = foe } do
            local t = tag
            u.OnDamage = function() hits[t] = (hits[t] or 0) + 1 end
        end
        local function who(f)
            hits = {}
            f()
            return fmt('me %d mate %d foe %d', hits.me or 0, hits.mate or 0, hits.foe or 0)
        end
        local c0 = me:GetPosition()
        P('self friendly=f self=t: ' .. who(function() DamageArea(me, c0, 5, 1, 'Normal', false, true) end))
        P('self friendly=t self=f: ' .. who(function() DamageArea(me, c0, 5, 1, 'Normal', true, false) end))
        P('self friendly=f self=f: ' .. who(function() DamageArea(me, c0, 5, 1, 'Normal', false, false) end))
        P('self friendly=t self=t: ' .. who(function() DamageArea(me, c0, 5, 1, 'Normal', true, true) end))
        P('self ring friendly=f self=t: ' .. who(function() DamageRing(me, c0, 0.1, 5, 1, 'Normal', false, true) end))
    end
    WaitTicks(160)
    WaitTicks(30)
    P('done')
end)
