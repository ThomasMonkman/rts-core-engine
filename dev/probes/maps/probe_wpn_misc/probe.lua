-- Differential probe: follow-ups to probe_wpn_proj / proj2. Tracking
-- projectiles made without a target, impacts a script survives (on a unit),
-- air and submerged collision shapes, collision checks by alliance, weapon
-- projectile positions, child projectile speed.
--   real game (Windows): dev/probes/run_faf.ps1 probe_wpn_misc 60
--   ours: cargo run -- --headless --no-ai --ticks 160 --script dev/probes/maps/probe_wpn_misc/probe.lua
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
local LAND = { {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232}, {664,320}, {848,320} }
local WATER = { {144,144}, {240,144}, {272,272} }
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local MISSILE = '/projectiles/TIFMissileCruise01/TIFMissileCruise01_proj.bp'
local TORP = '/projectiles/TANAnglerTorpedo01/TANAnglerTorpedo01_proj.bp'
local AAM = '/projectiles/TAAMissileFlayer01/TAAMissileFlayer01_proj.bp'
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
local function shoot(tag, launcher, bp, o, d, opts)
    opts = opts or {}
    local l = math.sqrt(d[1] * d[1] + d[2] * d[2] + d[3] * d[3])
    local p = launcher:CreateProjectile(bp, o[1], o[2], o[3], d[1] / l, d[2] / l, d[3] / l)
    local lp = launcher:GetPosition()
    p.T0 = GetGameTick()
    p.NI = 0
    p.OnImpact = function(self, tt, te)
        self.NI = self.NI + 1
        if not opts.quiet and self.NI <= (opts.max or 1) then
            local pos = self:GetPosition()
            P(fmt('%s impact#%d tick+%d type %s ent %s pos %.2f,%.2f,%.2f', tag, self.NI, GetGameTick() - self.T0, tostring(tt), name(te), pos[1] - lp[1], pos[2] - lp[2], pos[3] - lp[3]))
        end
        self.HitEnt = te
        self.HitType = tt
        if not opts.keep then self:Destroy() end
    end
    return p
end

ForkThread(function()
    WaitTicks(3)
    P('start')
    -- [trk0] tracking blueprints made with no target: what ends them?
    local L = make('ueb5101', A1, LAND[1][1], LAND[1][2])
    local watch = {}
    for i, def in { { 'missile', MISSILE }, { 'torp-air', TORP }, { 'aam', AAM }, { 'los', LOS } } do
        local ok, p = pcall(function() return L:CreateProjectile(def[2], 0, 10, i * 4, 1, 0, 0) end)
        if ok and p then
            local tag = 'trk0.' .. def[1]
            local t0 = GetGameTick()
            local ph = p:GetBlueprint().Physics
            P(fmt('%s bp TrackTarget %s Lifetime %s InitialSpeed %s', tag, tostring(ph.TrackTarget), tostring(ph.Lifetime), tostring(ph.InitialSpeed)))
            for _, cb in { 'OnImpact', 'OnLostTarget', 'OnDestroy', 'OnKilled', 'OnEnterWater', 'OnExitWater' } do
                local orig = p[cb]
                local cbn = cb
                p[cb] = function(self, a, b)
                    P(fmt('%s %s(%s) tick+%d', tag, cbn, tostring(a), GetGameTick() - t0))
                    if orig then return orig(self, a, b) end
                end
            end
            table.insert(watch, { tag = tag, p = p, t0 = t0, lp = L:GetPosition() })
        else
            P('trk0.' .. def[1] .. ' create failed')
        end
    end
    -- [stay] impacts on a unit that the script survives
    local L2 = make('ueb5101', A1, LAND[2][1], LAND[2][2])
    make('ueb5101', A2, LAND[2][1] + 10, LAND[2][2])
    local sp = shoot('stay.unit', L2, LOS, { 1.5, 0.3, 0 }, { 1, 0, 0 }, { keep = true, max = 4 }) sp:SetLifetime(2)
    table.insert(watch, { tag = 'stay.unit', p = sp, t0 = GetGameTick(), lp = L2:GetPosition(), n = 12 })
    make('ueb5101', A2, LAND[2][1] - 6, LAND[2][2]) make('ueb5101', A2, LAND[2][1] - 12, LAND[2][2])
    local sp2 = shoot('stay.two', L2, LOS, { -1.5, 0.3, 0 }, { -1, 0, 0 }, { keep = true, max = 6 }) sp2:SetLifetime(2)
    -- [who] OnCollisionCheck by alliance: who is asked at all
    local L3 = make('ueb5101', A1, LAND[3][1], LAND[3][2])
    local asked = {}
    for i, def in { { 'own', A1 }, { 'ally', A3 }, { 'neutral', NEU }, { 'enemy', A2 } } do
        local w = make('ueb5101', def[2], LAND[3][1] + 4 * i, LAND[3][2])
        local tag = def[1]
        w.OnCollisionCheck = function(self, other, weapon)
            table.insert(asked, tag)
            return false
        end
    end
    shoot('who.line', L3, LOS, { 1.5, 0.3, 0 }, { 1, 0, 0 }):SetLifetime(2)
    -- [air] a hovering gunship's collision shape, scanned from below and from the side
    local L4 = make('ueb5101', A1, LAND[4][1], LAND[4][2])
    local gs = make('uea0203', A2, LAND[4][1] + 12, LAND[4][2])
    -- [child] children of a slowed parent, with and without their own speed
    local L5 = make('ueb5101', A1, LAND[5][1], LAND[5][2])
    do
        local parent = shoot('child.parent', L5, LOS, { 0, 10, 0 }, { 1, 0, 0 }) parent:SetVelocity(6) parent:SetLifetime(3)
        WaitTicks(3)
        local lp = L5:GetPosition()
        for _, bp in { LOS, MISSILE, '/projectiles/TDFGauss01/TDFGauss01_proj.bp' } do
            local c = parent:CreateChildProjectile(bp)
            local tag = 'child.' .. string.sub(bp, 14, 20)
            local ph = c:GetBlueprint().Physics
            c.OnImpact = function(self) self:Destroy() end
            ForkThread(function()
                local prev = c:GetPosition()
                for t = 1, 3 do
                    WaitTicks(1)
                    if c:BeenDestroyed() then P(tag .. ' destroyed at t+' .. t) return end
                    local now = c:GetPosition()
                    P(fmt('%s t+%d step %.3f,%.3f,%.3f InitialSpeed %s launcher %s', tag, t, now[1] - prev[1], now[2] - prev[2], now[3] - prev[3], tostring(ph.InitialSpeed),
                        tostring(c:GetLauncher() == L5)))
                    prev = now
                end
                c:Destroy()
            end)
        end
    end
    -- [sea] a dived sub and a ship: straight shots that ignore the water surface
    local W = make('ues0103', A1, WATER[1][1], WATER[1][2], true)
    local sub = make('ues0203', A2, WATER[1][1] + 14, WATER[1][2], true)
    local ship = make('ues0103', A2, WATER[1][1] - 14, WATER[1][2] + 0, true)

    for t = 1, 14 do
        WaitTicks(1)
        for _, w in watch do
            if not w.gone then
                if w.p:BeenDestroyed() then
                    w.gone = true
                    P(fmt('%s gone tick+%d', w.tag, GetGameTick() - w.t0))
                elseif t <= (w.n or 3) then
                    local pos = w.p:GetPosition()
                    P(fmt('%s t+%d pos %.2f,%.2f,%.2f', w.tag, GetGameTick() - w.t0, pos[1] - w.lp[1], pos[2] - w.lp[2], pos[3] - w.lp[3]))
                end
            end
        end
    end
    local line = 'who asked'
    for _, a in asked do line = line .. ' ' .. a end
    P(line)
    P(fmt('stay.two impacts %d', sp2.NI))

    -- the gunship scans
    local gp, lp = gs:GetPosition(), L4:GetPosition()
    P(fmt('air gunship rel %.2f,%.2f,%.2f layer %s', gp[1] - lp[1], gp[2] - lp[2], gp[3] - lp[3], gs:GetCurrentLayer()))
    local scans = { up = {}, side = {}, sub = {}, ship = {} }
    local d = -3
    while d <= 3.01 do
        table.insert(scans.up, { d, shoot('s', L4, LOS, { gp[1] - lp[1] + d, 1, 0 }, { 0, 1, 0 }, { quiet = true }) })
        table.insert(scans.side, { d, shoot('s', L4, LOS, { 2, gp[2] - lp[2] + d, 0 }, { 1, 0, 0 }, { quiet = true }) })
        d = d + 0.2
    end
    local sp3, wp = sub:GetPosition(), W:GetPosition()
    P(fmt('sea sub rel %.2f,%.2f,%.2f layer %s; ship layer %s', sp3[1] - wp[1], sp3[2] - wp[2], sp3[3] - wp[3], sub:GetCurrentLayer(), ship:GetCurrentLayer()))
    local h = -4
    while h <= 2.01 do
        local a = shoot('s', W, LOS, { 4, h, 0 }, { 1, 0, 0 }, { quiet = true }) a:SetDestroyOnWater(false) a:SetLifetime(1.5)
        table.insert(scans.sub, { h, a })
        local b = shoot('s', W, LOS, { -4, h, 0 }, { -1, 0, 0 }, { quiet = true }) b:SetDestroyOnWater(false) b:SetLifetime(1.5)
        table.insert(scans.ship, { h, b })
        h = h + 0.2
    end
    -- [wpos] a weapon-fired projectile: where it is at creation and at impact
    local T = make('uel0201', A1, LAND[6][1], LAND[6][2])
    local tgt = make('ueb1201', A2, LAND[6][1], LAND[6][2] + 12)
    do
        local wep = T:GetWeapon(1)
        local create = wep.CreateProjectileAtMuzzle
        local n = 0
        wep.CreateProjectileAtMuzzle = function(self, muzzle)
            local pr = create(self, muzzle)
            n = n + 1
            if n <= 2 and type(pr) == 'table' then
                local born = GetGameTick()
                local up = T:GetPosition()
                local a = pr:GetPosition()
                local ax, ay, az = a[1], a[2], a[3]
                local vx, vy, vz = pr:GetVelocity()
                P(fmt('wpos shot%d created at %.2f,%.2f,%.2f vel %.2f,%.2f,%.2f launcher %s target %s', n, ax - up[1], ay - up[2], az - up[3], vx, vy, vz,
                    tostring(pr:GetLauncher() == T), tostring(pr:GetTrackingTarget() == tgt)))
                local oi = pr.OnImpact
                pr.OnImpact = function(pp, tt, te)
                    local b = pp:GetPosition()
                    P(fmt('wpos shot%d impact tick+%d type %s at %.2f,%.2f,%.2f', n, GetGameTick() - born, tostring(tt), b[1] - up[1], b[2] - up[2], b[3] - up[3]))
                    return oi(pp, tt, te)
                end
            end
            return pr
        end
        T:SetFireState(0)
        IssueAttack({ T }, tgt)
    end
    WaitTicks(30)
    for _, k in { 'up', 'side', 'sub', 'ship' } do
        local line = 'scan ' .. k
        local prev
        for _, s in scans[k] do
            local hit = tostring(s[2].HitType) .. (s[2].HitEnt and ':' .. name(s[2].HitEnt) or '')
            if hit ~= prev then line = line .. fmt(' %s@%.1f', hit, s[1]) end
            prev = hit
        end
        P(line)
    end
    WaitTicks(30)
    P('done')
end)
