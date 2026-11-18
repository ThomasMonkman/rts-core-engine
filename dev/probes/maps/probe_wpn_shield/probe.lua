-- Differential probe: shields against projectiles, area damage and beams;
-- regen and recharge timing; personal shields; overspill.
--   real game (Windows): dev/probes/run_faf.ps1 probe_wpn_shield 120
--   ours: cargo run -- --headless --no-ai --ticks 700 --script dev/probes/maps/probe_wpn_shield/probe.lua
-- Lines: PROBE <case> ... ; impact positions are relative to the shield's unit.
local fmt = string.format
local function P(s) LOG('PROBE ' .. s) end
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2, A3, A4 = armies[1], armies[2], armies[3], armies[4]
SetAlliance(A1, A3, 'Ally') SetAlliance(A3, A1, 'Ally')
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
SetAlliance(A2, A3, 'Enemy') SetAlliance(A3, A2, 'Enemy')
local LAND = {
    {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232},
    {664,320}, {848,320}, {904,384}, {656,416}, {568,440}, {456,552}, {368,568}, {120,608},
}
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local function feed()
    for _, a in { A1, A2, A3 } do
        local b = GetArmyBrain(a)
        b:GiveStorage('ENERGY', 1000000)
        b:GiveResource('ENERGY', 1000000)
    end
end
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    return u
end
local function tough(u) u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000) return u end
local function name(e)
    if not e then return 'nil' end
    -- a shield entity has no blueprint: don't ask the engine for one
    if e.Owner then return 'shield' end
    if e.UnitId then return tostring(e.UnitId) end
    return 'entity'
end
-- a hook that steps aside while the object's own (state) method runs
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
local function watchUnit(u)
    u.PH = 0 u.PS = 0
    hook(u, 'OnDamage', function(self, inst, amt, vec, t) self.PH = self.PH + 1 self.PS = self.PS + amt end)
end
local function watchShield(s)
    s.PH = 0 s.PS = 0 s.PQ = 0 s.PQA = 0
    hook(s, 'OnDamage', function(self, inst, amt, vec, t) self.PH = self.PH + 1 self.PS = self.PS + amt end)
    hook(s, 'OnGetDamageAbsorption', function(self, inst, amt, t) self.PQ = self.PQ + 1 self.PQA = amt end)
end
local function sh(u)
    local s = u.MyShield
    if not s then return 'noshield' end
    return fmt('up %s hp %.1f', tostring(s:IsUp()), s:GetHealth())
end
local function shoot(tag, launcher, ref, o, d)
    local l = math.sqrt(d[1] * d[1] + d[2] * d[2] + d[3] * d[3])
    local p = launcher:CreateProjectile(LOS, o[1], o[2], o[3], d[1] / l, d[2] / l, d[3] / l)
    local rp = ref:GetPosition()
    local t0 = GetGameTick()
    p.OnImpact = function(self, tt, te)
        local pos = self:GetPosition()
        local dx, dy, dz = pos[1] - rp[1], pos[2] - rp[2], pos[3] - rp[3]
        P(fmt('%s impact tick+%d type %s ent %s pos %.2f,%.2f,%.2f dist %.2f', tag, GetGameTick() - t0, tostring(tt), name(te), dx, dy, dz, math.sqrt(dx * dx + dy * dy + dz * dz)))
        self:Destroy()
    end
    return p
end

-- a shot from 1.5 off the launcher (at height h) straight at the target's centre
local function shootAt(tag, launcher, ref, target, h)
    local lp, tp = launcher:GetPosition(), target:GetPosition()
    local dx, dz = tp[1] - lp[1], tp[3] - lp[3]
    local l = math.sqrt(dx * dx + dz * dz)
    local ox, oz = dx / l * 1.5, dz / l * 1.5
    return shoot(tag, launcher, ref, { ox, h, oz }, { tp[1] - lp[1] - ox, tp[2] + 0.25 - lp[2] - h, tp[3] - lp[3] - oz })
end

ForkThread(function()
    WaitTicks(3)
    feed()
    P('start')
    -- site 1: a bubble with a tank inside
    local x, z = LAND[1][1], LAND[1][2]
    local G = tough(make('ueb4202', A2, x, z))
    local K = tough(make('uel0201', A2, x + 6, z)) watchUnit(K)
    local Lout = tough(make('ueb5101', A1, x - 25, z))
    local Lin = tough(make('ueb5101', A1, x - 6, z))
    local Lown = tough(make('ueb5101', A2, x, z - 25))
    local Wfar = tough(make('ueb5101', A1, x, z + 25)) watchUnit(Wfar)
    -- site 2: an allied bubble
    local x2, z2 = LAND[2][1], LAND[2][2]
    local G2 = tough(make('ueb4202', A3, x2, z2))
    local L2 = tough(make('ueb5101', A1, x2 - 25, z2))
    local W2 = tough(make('ueb5101', A2, x2 + 25, z2))
    local L2e = tough(make('ueb5101', A2, x2, z2 - 25))
    -- site 3: personal shield, mobile bubble
    local x3, z3 = LAND[3][1], LAND[3][2]
    local H = tough(make('ual0303', A2, x3, z3)) watchUnit(H)
    local L3 = tough(make('ueb5101', A1, x3 - 10, z3))
    local M = tough(make('uel0307', A2, x3, z3 + 30))
    local L3m = tough(make('ueb5101', A1, x3 - 20, z3 + 30))
    -- site 4: overspill
    local x4, z4 = LAND[4][1], LAND[4][2]
    local O1 = tough(make('ueb4202', A2, x4, z4))
    local O2 = tough(make('ueb4202', A2, x4 + 10, z4))
    local O3 = tough(make('uel0307', A2, x4 + 5, z4 + 5))
    local O4 = tough(make('uab4202', A2, x4 + 60, z4))
    -- site 5: no energy (army 4 gets none)
    local E = make('ueb4202', A4, LAND[5][1], LAND[5][2])
    local e0 = GetGameTick()
    -- sites 6, 7: beams at a tank under a bubble
    local beams = {}
    for i, bp in { 'xsb2301', 'url0402', 'uab2301' } do
        local bx, bz = LAND[5 + i][1], LAND[5 + i][2]
        local g = tough(make('ueb4202', A2, bx, bz + 30))
        local k = tough(make('uel0201', A2, bx, bz + 34)) watchUnit(k)
        local a = tough(make(bp, A1, bx, bz))
        a:SetIntelRadius('Vision', 70)
        table.insert(beams, { bp = bp, g = g, k = k, a = a })
    end

    local up = {}
    local all = { { 'G', G }, { 'G2', G2 }, { 'H', H }, { 'M', M }, { 'O1', O1 }, { 'O2', O2 }, { 'O3', O3 }, { 'O4', O4 }, { 'E', E } }
    for t = 1, 60 do
        WaitTicks(1)
        if math.mod(t, 5) == 0 then feed() end
        for _, e in all do
            local s = e[2].MyShield
            if s and s:IsUp() and not up[e[1]] then
                up[e[1]] = true
                P(fmt('init %s shield up at tick+%d hp %.1f max %.1f', e[1], t, s:GetHealth(), s:GetMaxHealth()))
            end
        end
    end
    for _, e in all do P(fmt('init %s %s', e[1], sh(e[2]))) end
    P(fmt('noenergy E tick+%d %s', GetGameTick() - e0, sh(E)))
    for _, e in { G, G2, M, O1, O2, O3, O4, H } do if e.MyShield then watchShield(e.MyShield) end end

    -- geometry: shots at the bubble from outside at several heights, and from above
    for _, h in { 1, 4, 7, 10, 12, 13, 16 } do shoot(fmt('geo.h%d', h), Lout, G, { 0, h, 0 }, { 1, 0, 0 }) end
    for _, dx in { 0, 6, 12, 14 } do shoot(fmt('geo.top%d', dx), Lout, G, { 25 + dx, 40, 0.5 }, { 0, -1, 0 }) end
    shoot('geo.inside', Lin, G, { 1.5, 0.3, 0 }, { 1, 0, 0 })
    shoot('geo.insideout', Lin, G, { -1.5, 0.3, 0 }, { -1, 0, 0 })
    shootAt('geo.own', Lown, G, Wfar, 0.3)
    shootAt('ally.through', L2, G2, W2, 0.3)
    shootAt('ally.enemy', L2e, G2, G2, 0.3)
    shoot('pers.shot', L3, H, { 1.5, 0.8, 0 }, { 1, 0, 0 })
    shoot('mob.shot', L3m, M, { 1.5, 0.5, 0 }, { 1, 0, 0 })
    WaitTicks(30)
    P(fmt('geo after: G %s shieldhits %d K hits %d Wfar hits %d', sh(G), G.MyShield.PH, K.PH, Wfar.PH))
    P(fmt('pers after shot: H health %.1f hits %d %s shieldhits %d', H:GetHealth(), H.PH, sh(H), H.MyShield and H.MyShield.PH or -1))
    if M.MyShield then P(fmt('mob after shot: %s shieldhits %d', sh(M), M.MyShield.PH)) end

    -- area damage on a shielded tank
    feed()
    local function area(tag, f)
        local S = G.MyShield
        K.PH = 0 K.PS = 0 S.PH = 0 S.PS = 0 S.PQ = 0 S.PQA = 0
        local g0, s0 = G:GetHealth(), S:GetHealth()
        local ok = pcall(f)
        P(fmt('area %s ok %s: K hits %d sum %.1f | shield OnDamage %d sum %.1f absorbQ %d amt %.1f hp -%.1f | G -%.1f', tag, tostring(ok), K.PH, K.PS, S.PH, S.PS, S.PQ, S.PQA,
            s0 - S:GetHealth(), g0 - G:GetHealth()))
    end
    local kp, gp = K:GetPosition(), G:GetPosition()
    area('r3@K 100', function() DamageArea(Lout, kp, 3, 100, 'Normal', true, true) end)
    area('r3@K nil-inst', function() DamageArea(nil, kp, 3, 100, 'Normal', true, true) end)
    area('r3@K friendly-inst', function() DamageArea(Lown, kp, 3, 100, 'Normal', true, true) end)
    area('r3@edge', function() DamageArea(Lout, { gp[1] - 14, gp[2], gp[3] }, 3, 100, 'Normal', true, true) end)
    area('r3@outside', function() DamageArea(Lout, { gp[1] - 18, gp[2], gp[3] }, 3, 100, 'Normal', true, true) end)
    area('r30@K', function() DamageArea(Lout, kp, 30, 100, 'Normal', true, true) end)
    area('r3@G', function() DamageArea(Lout, gp, 3, 100, 'Normal', true, true) end)
    area('ring1-4@K', function() DamageRing(Lout, kp, 1, 4, 100, 'Normal', true, true) end)
    area('Damage K', function() Damage(Lout, kp, K, 100, 'Normal') end)
    area('Damage shield', function() Damage(Lout, kp, G.MyShield, 100, 'Normal') end)
    P(fmt('area before big: %s', sh(G)))
    area('r3@K 20000', function() DamageArea(Lout, kp, 3, 20000, 'Normal', true, true) end)
    P(fmt('area after big: %s', sh(G)))
    area('r3@K down', function() DamageArea(Lout, kp, 3, 100, 'Normal', true, true) end)

    -- personal shield: direct and area damage
    if H.MyShield then
        local S = H.MyShield
        local function pers(tag, f)
            H.PH = 0 H.PS = 0 S.PH = 0 S.PS = 0 S.PQ = 0
            local h0, s0 = H:GetHealth(), S:GetHealth()
            f()
            P(fmt('pers %s: unit OnDamage %d sum %.1f hp -%.1f | shield OnDamage %d absorbQ %d hp -%.1f', tag, H.PH, H.PS, h0 - H:GetHealth(), S.PH, S.PQ, s0 - S:GetHealth()))
        end
        local hp = H:GetPosition()
        pers('Damage 100', function() Damage(L3, hp, H, 100, 'Normal') end)
        pers('Area 100', function() DamageArea(L3, hp, 2, 100, 'Normal', true, true) end)
        pers('Damage 5000', function() Damage(L3, hp, H, 5000, 'Normal') end)
        pers('Damage 100 down', function() Damage(L3, hp, H, 100, 'Normal') end)
    end

    -- overspill between overlapping bubbles
    do
        local function hp(u) return u.MyShield and u.MyShield:GetHealth() or -1 end
        local a, b, c = hp(O1), hp(O2), hp(O3)
        Damage(Lout, O1:GetPosition(), O1.MyShield, 1000, 'Normal')
        P(fmt('spill same tick: O1 -%.1f O2 -%.1f O3 -%.1f', a - hp(O1), b - hp(O2), c - hp(O3)))
        WaitTicks(2)
        P(fmt('spill +2: O1 -%.1f O2 -%.1f O3 -%.1f', a - hp(O1), b - hp(O2), c - hp(O3)))
    end

    -- regen: damage once, then the shield's health every tick
    do
        local S = O4.MyShield
        Damage(Lout, O4:GetPosition(), S, 2000, 'Normal')
        local line = 'regen O4'
        for t = 1, 70 do
            WaitTicks(1)
            if t <= 6 or t >= 24 then line = line .. fmt(' %d:%.0f', t, S:GetHealth()) end
            if t == 38 then P(line) line = 'regen O4' end
            if math.mod(t, 5) == 0 then feed() end
        end
        P(line)
    end

    -- beams
    for _, b in beams do b.a:SetFireState(0) IssueAttack({ b.a }, b.k) b.k.PH = 0 b.k.PS = 0 b.s0 = b.g.MyShield:GetHealth() watchShield(b.g.MyShield) end
    -- collapse and recharge
    local c0 = GetGameTick()
    local S = O1.MyShield
    Damage(Lout, O1:GetPosition(), S, 50000, 'Normal')
    P(fmt('collapse O1 same tick: %s', sh(O1)))
    local was = S:IsUp()
    for t = 1, 300 do
        WaitTicks(1)
        if math.mod(t, 5) == 0 then feed() end
        local now = S:IsUp()
        if now ~= was then
            P(fmt('collapse O1 tick+%d up %s hp %.1f', t, tostring(now), S:GetHealth()))
            was = now
        end
        if t == 100 then
            for _, b in beams do
                P(fmt('beam %s: tank hits %d sum %.1f | shield OnDamage %d sum %.1f hp -%.1f', b.bp, b.k.PH, b.k.PS, b.g.MyShield.PH, b.g.MyShield.PS, b.s0 - b.g.MyShield:GetHealth()))
            end
            P(fmt('noenergy E tick+%d %s', GetGameTick() - e0, sh(E)))
        end
        if now and t > 240 then break end
    end
    P(fmt('collapse O1 end %s', sh(O1)))
    WaitTicks(30)
    P('done')
end)
