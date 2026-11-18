-- Differential probe (re-check of open sweep items): projectiles that survive
-- their impacts (keep.*), which `Air` impacts carry an entity (air.*), and
-- OnHealthChanged timing (hc.*).
--   real game (Windows): dev/probes/run_faf.ps1 probe_re_wpn 90
--   ours: --headless --no-ai --ticks 600 --script dev/probes/maps/probe_re_wpn/probe.lua
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
}
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local GAUSS = '/projectiles/TDFGauss01/TDFGauss01_proj.bp'
local function make(bp, army, x, z, dy)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z) + (dy or 0), z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    return u
end
local function who(e, refs)
    if not e then return 'nil' end
    for k, v in refs do if e == v then return k end end
    return 'other'
end
local function s(v) return type(v) == 'number' and fmt('%.3f', v) or tostring(v) end

-- [keep] a projectile whose OnImpact never destroys it
local keeps = {}
local function keep(tag, launcher, bp, o, d, lifetime, refs)
    local l = math.sqrt(d[1] * d[1] + d[2] * d[2] + d[3] * d[3])
    local p = launcher:CreateProjectile(bp, o[1], o[2], o[3], d[1] / l, d[2] / l, d[3] / l)
    if lifetime then p:SetLifetime(lifetime) end
    local lp = launcher:GetPosition()
    local k = { tag = tag, p = p, lp = lp, t0 = GetGameTick(), n = 0 }
    p.OnImpact = function(self, tt, te)
        k.n = k.n + 1
        if k.n <= 6 or math.mod(k.n, 10) == 0 then
            local pos = self:GetPosition()
            P(fmt('keep.%s impact#%d tick+%d type %s ent %s pos %.2f,%.2f,%.2f', tag, k.n, GetGameTick() - k.t0, tostring(tt), who(te, refs or {}), pos[1] - lp[1], pos[2] - lp[2], pos[3] - lp[3]))
        end
    end
    table.insert(keeps, k)
    return p
end

ForkThread(function()
    WaitTicks(3)
    P('start')
    -- keep cases
    do
        local L = make('ueb5101', A1, LAND[1][1], LAND[1][2])
        local W = make('ueb5101', A2, LAND[1][1] + 10, LAND[1][2])
        keep('unit2s', L, LOS, { 1.5, 0.3, 0 }, { 1, 0, 0 }, 2, { wall = W })
        local L2 = make('ueb5101', A1, LAND[2][1], LAND[2][2])
        local W2 = make('ueb5101', A2, LAND[2][1] + 10, LAND[2][2])
        keep('unitbp', L2, LOS, { 1.5, 0.3, 0 }, { 1, 0, 0 }, nil, { wall = W2 })
        local L3 = make('ueb5101', A1, LAND[3][1], LAND[3][2])
        local W3 = make('ueb5101', A2, LAND[3][1] + 10, LAND[3][2])
        keep('gauss', L3, GAUSS, { 1.5, 0.3, 0 }, { 1, 0, 0 }, 2, { wall = W3 })
        local L4 = make('ueb5101', A1, LAND[4][1], LAND[4][2])
        keep('ground', L4, LOS, { 3, 6, 0 }, { 1, -1, 0 }, 3)
        keep('expire', L4, LOS, { 0, 8, 0 }, { 0, 1, 0 }, 0.5)
        keep('expireslow', L4, LOS, { -3, 8, 0 }, { 0, 1, 0 }, 0.5).SLOW = true
    end
    for _, k in keeps do if k.tag == 'expireslow' then k.p:SetVelocity(1) end end
    -- air cases: impacts of type Air, with and without a target
    local airrefs
    do
        local L = make('ueb5101', A1, LAND[5][1], LAND[5][2])
        local far = make('ueb5101', A2, LAND[5][1] + 60, LAND[5][2])
        local gs = make('uea0203', A2, LAND[5][1], LAND[5][2] + 30, 15)
        airrefs = { far = far, gs = gs }
        local function air(tag, opts)
            local p = L:CreateProjectile(LOS, 0, 10, 0, 0, 1, 0)
            local t0 = GetGameTick()
            if opts.target then
                local ok = pcall(function() p:SetNewTarget(opts.target) end)
                P(fmt('air.%s SetNewTarget ok %s tracking %s', tag, tostring(ok), who(p:GetTrackingTarget(), airrefs)))
            end
            if opts.life then p:SetLifetime(opts.life) end
            if opts.above then p:ChangeDetonateAboveHeight(opts.above) end
            if opts.speed then p:SetVelocity(opts.speed) end
            p.OnImpact = function(self, tt, te)
                P(fmt('air.%s impact tick+%d type %s ent %s', tag, GetGameTick() - t0, tostring(tt), who(te, airrefs)))
                self:Destroy()
            end
        end
        air('life.notgt', { life = 0.3 })
        air('life.tgtwall', { life = 0.3, target = far })
        air('life.tgtgs', { life = 0.3, target = gs })
        air('det.tgtwall', { life = 3, above = 14, target = far, speed = 20 })
        air('det.notgt', { life = 3, above = 14, speed = 20 })
    end
    -- air.wpn: delk002 and uel0205 against a hovering gunship
    local wl = {}
    for _, b in ArmyBrains do b:GiveResource('ENERGY', 1000000) b:GiveResource('MASS', 100000) end
    do
        for i, spec in { { 'delk002', false }, { 'delk002', true }, { 'uel0205', true }, { 'uel0205', false } } do
            local bp = spec[1] .. (spec[2] and '.patrol' or '.idle')
            local x, z = LAND[5 + i][1], LAND[5 + i][2]
            local u = make(spec[1], A1, x, z)
            local gs = make('uea0203', A2, x, z + 24)
            if spec[2] then
                IssuePatrol({ gs }, { x - 6, GetTerrainHeight(x - 6, z + 24), z + 24 })
                IssuePatrol({ gs }, { x + 6, GetTerrainHeight(x + 6, z + 24), z + 24 })
            end
            local w = { bp = bp, u = u, gs = gs, counts = {}, keys = {}, first = {} }
            table.insert(wl, w)
            for wi = 1, u:GetWeaponCount() do
                local wep = u:GetWeapon(wi)
                local create = wep.CreateProjectileAtMuzzle
                if create then
                    wep.CreateProjectileAtMuzzle = function(self, muzzle)
                        local pr = create(self, muzzle)
                        w.shots = (w.shots or 0) + 1
                        if type(pr) == 'table' and pr.OnImpact then
                            local born = GetGameTick()
                            local trk = pr:GetTrackingTarget()
                            local oi = pr.OnImpact
                            pr.OnImpact = function(pp, tt, te)
                                local pos = pp:GetPosition()
                                local gp = gs:BeenDestroyed() and pos or gs:GetPosition()
                                local d = VDist3(pos, gp)
                                local key = fmt('w%d:%s:%s:trk=%s', wi, tostring(tt), who(te, { gs = gs }), who(trk, { gs = gs }))
                                if not w.counts[key] then w.counts[key] = 0 table.insert(w.keys, key) end
                                w.counts[key] = w.counts[key] + 1
                                if w.counts[key] <= 2 then
                                    P(fmt('air.wpn %s %s tick+%d dist-to-gs %.2f', bp, key, GetGameTick() - born, d))
                                end
                                return oi(pp, tt, te)
                            end
                        end
                        return pr
                    end
                end
            end
            u:SetFireState(0)
            ForkThread(function() WaitTicks(25) if not gs:BeenDestroyed() then IssueAttack({ u }, gs) end end)
        end
    end
    -- hc: OnHealthChanged timing
    local hcdone = false
    ForkThread(function()
        local x, z = LAND[12][1], LAND[12][2]
        local u = CreateUnitHPR('ueb1101', A2, x, GetTerrainHeight(x, z), z, 0, 0, 0)
        u:SetMaxHealth(1000) u:SetHealth(nil, 1000)
        local t0 = GetGameTick()
        local step = 'init'
        u.OnHealthChanged = function(self, new, old)
            P(fmt('hc [%s] OnHealthChanged tick+%d new %s old %s health %.1f', step, GetGameTick() - t0, s(new), s(old), self:GetHealth()))
        end
        WaitTicks(2)
        local function dmg(tag, n)
            step = tag
            P(fmt('hc [%s] call tick+%d', tag, GetGameTick() - t0))
            Damage(nil, u:GetPosition(), u, n, 'Normal')
            P(fmt('hc [%s] ret tick+%d health %.1f', tag, GetGameTick() - t0, u:GetHealth()))
        end
        dmg('d50', 50) WaitTicks(2)        -- 950: same quarter
        dmg('d200', 200) WaitTicks(2)      -- 750: onto the 0.75 line
        dmg('d1', 1) WaitTicks(2)          -- 749: below it
        dmg('d500', 500) WaitTicks(2)      -- 249: crosses two quarters
        step = 'set1000' P('hc [set1000] call') u:SetHealth(nil, 1000) P(fmt('hc [set1000] ret tick+%d', GetGameTick() - t0)) WaitTicks(2)
        dmg('d300a', 300) dmg('d300b', 300) WaitTicks(2)
        step = 'set1000b' u:SetHealth(nil, 1000) WaitTicks(2)
        step = 'regen' u:SetHealth(nil, 740) WaitTicks(1) P(fmt('hc [regen] start tick+%d health %.1f', GetGameTick() - t0, u:GetHealth()))
        u:SetRegenRate(50)
        for i = 1, 25 do WaitTicks(1) end
        P(fmt('hc [regen] end tick+%d health %.1f', GetGameTick() - t0, u:GetHealth()))
        u:SetRegenRate(0)
        step = 'kill' P('hc [kill] call') u:Kill() P(fmt('hc [kill] ret tick+%d', GetGameTick() - t0))
        WaitTicks(3)
        hcdone = true
    end)

    for t = 1, 130 do
        WaitTicks(1)
        if t == 30 then
            for _, w in wl do
                local gp = w.gs:GetPosition()
                local up = w.u:GetPosition() P(fmt('air.wpn %s gs h %.2f layer %s rel %.1f,%.1f', w.bp, gp[2] - GetTerrainHeight(gp[1], gp[3]), w.gs:GetCurrentLayer(), gp[1] - up[1], gp[3] - up[3]))
            end
        end
        for _, k in keeps do
            if not k.gone then
                if k.p:BeenDestroyed() then
                    k.gone = true
                    P(fmt('keep.%s gone tick+%d impacts %d', k.tag, GetGameTick() - k.t0, k.n))
                elseif t <= 14 or math.mod(t, 20) == 0 then
                    local pos = k.p:GetPosition()
                    P(fmt('keep.%s t+%d pos %.2f,%.2f,%.2f n %d', k.tag, GetGameTick() - k.t0, pos[1] - k.lp[1], pos[2] - k.lp[2], pos[3] - k.lp[3], k.n))
                end
            end
        end
    end
    for _, k in keeps do
        if not k.gone then P(fmt('keep.%s alive at end impacts %d', k.tag, k.n)) k.p:Destroy() end
    end
    for _, w in wl do
        local line = 'air.wpn ' .. w.bp .. ' shots ' .. tostring(w.shots or 0) .. ' totals'
        for _, key in w.keys do line = line .. ' ' .. key .. '=' .. w.counts[key] end
        P(line)
    end
    while not hcdone do WaitTicks(5) end
    WaitTicks(30)
    P('done')
end)
