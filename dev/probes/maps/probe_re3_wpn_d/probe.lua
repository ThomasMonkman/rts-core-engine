-- Differential probe (sweep 3, weapons B): DamageArea reach (edge.*), Lua-made
-- projectiles dying at creation (proj.*), shield start-up and beams (sh.*),
-- TreeFire (tree.*), OnCollisionCheck for own / allied (coll.*).
--   real game (Windows): dev/probes/run_faf.ps1 probe_re3_wpn_b 200
--   ours: --headless --ticks 900 --script dev/probes/maps/probe_re3_wpn_b/probe.lua
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
    {176,672}, {360,672}, {256,760}, {88,776}, {248,856}, {352,856}, {104,896}, {408,896},
}
local WATER = { {144,144}, {240,144} }
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local function s(v) return type(v) == 'number' and fmt('%.3f', v) or tostring(v) end
local function make(bp, army, x, z, surface)
    local y = surface and GetSurfaceHeight(x, z) or GetTerrainHeight(x, z)
    local u = CreateUnitHPR(bp, army, x, y, z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    return u
end
local function tough(u) u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000) return u end
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
local function feed()
    for _, a in { A1, A2, A3 } do
        local b = GetArmyBrain(a)
        b:GiveStorage('ENERGY', 1000000) b:GiveResource('ENERGY', 1000000)
    end
end
local function name(e, refs)
    if not e then return 'nil' end
    for k, v in refs or {} do if e == v then return k end end
    if e.UnitId then return tostring(e.UnitId) end
    return 'other'
end
local function guard(tag, f)
    return function()
        f()
    end
end
local done = {}

-- placed at tick 0 so shields can start up: the shield cases (sh.*)
local SH = {}
do
    local x, z = LAND[15][1], LAND[15][2]
    SH.E = make('ueb4202', A4, x, z)           -- army with no energy
    SH.Ef = tough(make('ueb4202', A2, x + 40, z)) -- fed army
    SH.H = tough(make('ual0303', A2, x, z + 40))
    SH.M = tough(make('uel0307', A2, x + 40, z + 40))
    SH.HE = make('ual0303', A4, x - 40, z)
end

ForkThread(function()
    WaitTicks(3)
    feed()
    P('start')
    local t0 = GetGameTick()

    -- [sh] shield start-up, per tick, then beams on a bubble
    ForkThread(guard('sh', function()
        local prev = {}
        local brain4 = GetArmyBrain(A4)
        for t = 0, 60 do
            if t > 0 then WaitTicks(1) end
            if math.mod(t, 5) == 0 then feed() end
            for _, k in { 'E', 'Ef', 'H', 'M', 'HE' } do
                local sh = SH[k].MyShield
                local st = sh and (sh:IsUp() and 'up' or 'down') or 'none'
                if st ~= prev[k] then
                    P(fmt('sh.init %s tick %d (abs %d) %s hp %s E4 %.1f/%.1f', k, t, GetGameTick(), st, sh and s(sh:GetHealth()) or '-',
                        brain4:GetEconomyStored('ENERGY'), brain4:GetEconomyStoredRatio('ENERGY')))
                    prev[k] = st
                end
            end
        end
        -- beams
        local beams = {}
        for i, bp in { 'xsb2301', 'url0402', 'uab2301' } do
            local bx, bz = LAND[15 + i][1], LAND[15 + i][2]
            local g = tough(make('ueb4202', A2, bx, bz + 30))
            local k = tough(make('uel0201', A2, bx, bz + 34))
            local a = tough(make(bp, A1, bx, bz))
            a:SetIntelRadius('Vision', 70)
            table.insert(beams, { bp = bp, g = g, k = k, a = a, d = {}, n = 0, sum = 0, f = {}, kh = 0 })
        end
        WaitTicks(15)
        local b0 = GetGameTick()
        for _, b in beams do
            local S = b.g.MyShield
            b.s0 = S:GetHealth()
            hook(S, 'OnDamage', function(self, inst, amt) b.n = b.n + 1 b.sum = b.sum + amt if table.getn(b.d) < 25 then table.insert(b.d, GetGameTick() - b0) end end)
            hook(b.k, 'OnDamage', function() b.kh = b.kh + 1 end)
            local w = b.a:GetWeapon(1)
            hook(w, 'OnFire', function() if table.getn(b.f) < 12 then table.insert(b.f, GetGameTick() - b0) end end)
            b.a:SetFireState(0) IssueAttack({ b.a }, b.k)
        end
        for t = 1, 100 do WaitTicks(1) if math.mod(t, 5) == 0 then feed() end end
        for _, b in beams do
            P(fmt('sh.beam %s up %s hits %d sum %.1f hp -%.1f tank %d | dmg ticks %s | OnFire %s', b.bp, tostring(b.g.MyShield:IsUp()), b.n, b.sum,
                b.s0 - b.g.MyShield:GetHealth(), b.kh, table.concat(b.d, ','), table.concat(b.f, ',')))
        end
        done.sh = true
    end))

    done.proj = true
    -- [coll] OnCollisionCheck by alliance: called? passes through?
    ForkThread(guard('coll', function()
        local x, z = LAND[7][1], LAND[7][2]
        local refs = {}
        local res = {}
        local function lane(i, tag, army, mode, projcheck)
            local lz = z + (i - 4) * 6
            local L = tough(make('ueb5101', A1, x, lz))
            local w = tough(make('ueb2101', army, x + 5, lz))
            local back = tough(make('ueb5101', A2, x + 10, lz))
            local r = { tag = tag, n = 0, pn = 0, wh = 0, bh = 0, res = {} }
            res[i] = r
            refs = {}
            if mode == 'stock' then
                local orig = w.OnCollisionCheck
                w.OnCollisionCheck = function(self, other, weapon)
                    r.n = r.n + 1
                    local v = orig(self, other, weapon)
                    if r.n == 1 then r.v = tostring(v) end
                    return v
                end
            elseif mode ~= 'none' then
                local val = mode == 'true'
                w.OnCollisionCheck = function(self, other, weapon) r.n = r.n + 1 r.v = tostring(val) return val end
            end
            hook(w, 'OnDamage', function() r.wh = r.wh + 1 end)
            hook(back, 'OnDamage', function() r.bh = r.bh + 1 end)
            local lp, bpos = L:GetPosition(), back:GetPosition()
            local dx, dy, dz = bpos[1] - lp[1] - 1.5, bpos[2] + 0.25 - lp[2] - 0.3, bpos[3] - lp[3]
            local dl = math.sqrt(dx * dx + dy * dy + dz * dz)
            local p = L:CreateProjectile(LOS, 1.5, 0.3, 0, dx / dl, dy / dl, dz / dl)
            p:SetLifetime(2)
            local t0p = GetGameTick()
            if projcheck then
                local po = p.OnCollisionCheck
                p.OnCollisionCheck = function(self, other)
                    r.pn = r.pn + 1
                    if r.pn == 1 then r.pother = name(other, { wall = w, back = back }) end
                    if po then return po(self, other) end
                    return true
                end
            end
            local oi = p.OnImpact
            p.OnImpact = function(self, tt, te)
                local pos = self:GetPosition()
                if not r.imp then r.imp = fmt('%s %s x%.2f t+%d', tostring(tt), name(te, { wall = w, back = back }), pos[1] - x, GetGameTick() - t0p) end
                return oi(self, tt, te)
            end
        end
        lane(1, 'own-stock', A1, 'stock')
        lane(2, 'ally-stock', A3, 'stock')
        lane(3, 'enemy-stock', A2, 'stock')
        lane(4, 'own-true', A1, 'true')
        lane(5, 'ally-true', A3, 'true')
        lane(6, 'own-false', A1, 'false')
        lane(7, 'own-projcheck', A1, 'stock', true)
        lane(8, 'enemy-projcheck', A2, 'stock', true)
        -- weapon-fired: a tank shooting an enemy wall with an own / allied wall in the line
        local wl = {}
        for i, def in { { 'own', A1 }, { 'ally', A3 }, { 'none', nil } } do
            local wx, wz = LAND[21][1] + (i - 2) * 20, LAND[21][2]
            local tk = tough(make('uel0201', A1, wx, wz))
            local en = tough(make('ueb5101', A2, wx, wz + 15))
            local r = { tag = def[1], n = 0, bh = 0, eh = 0 }
            if def[2] then
                local bl = tough(make('ueb5101', def[2], wx, wz + 7))
                local orig = bl.OnCollisionCheck
                bl.OnCollisionCheck = function(self, other, weapon)
                    r.n = r.n + 1
                    local v = orig(self, other, weapon)
                    r.v = tostring(v)
                    return v
                end
                hook(bl, 'OnDamage', function() r.bh = r.bh + 1 end)
            end
            hook(en, 'OnDamage', function() r.eh = r.eh + 1 end)
            r.tk = tk r.en = en
            table.insert(wl, r)
        end
        -- projectile vs projectile, head on, by alliance
        local pp = {}
        local px, pz = LAND[21][1], LAND[21][2] + 40
        for i, def in { { 'own', A1 }, { 'ally', A3 }, { 'enemy', A2 } } do
            for j, bp in { '/projectiles/CIFPulsarEMPMissile01/CIFPulsarEMPMissile01_proj.bp', LOS } do
                local lz = pz + (i - 1) * 12 + (j - 1) * 5
                local La = tough(make('ueb5101', A1, px, lz))
                local Lb = tough(make('ueb5101', def[2], px + 20, lz))
                local a = La:CreateProjectile(bp, 1.5, 3, 0, 1, 0, 0)
                local b = Lb:CreateProjectile(LOS, -1.5, 3, 0, -1, 0, 0)
                local r = { tag = def[1] .. (j == 1 and '.emp' or '.los'), ca = 0, cb = 0 }
                table.insert(pp, r)
                if a:BeenDestroyed() then r.dead = true end
                pcall(function() a:SetVelocity(8) a:SetLifetime(2) b:SetVelocity(8) b:SetLifetime(2) end)
                local t0p = GetGameTick()
                for _, e in { { a, 'a' }, { b, 'b' } } do
                    local pr, nm = e[1], e[2]
                    local oc = pr.OnCollisionCheck
                    pr.OnCollisionCheck = function(self, other)
                        local v = oc and oc(self, other)
                        r['c' .. nm] = r['c' .. nm] + 1
                        r['v' .. nm] = tostring(v)
                        return v
                    end
                    local oi = pr.OnImpact
                    pr.OnImpact = function(self, tt, te)
                        local pos = self:GetPosition()
                        if not r['i' .. nm] then r['i' .. nm] = fmt('%s x%.2f t+%d', tostring(tt), pos[1] - px, GetGameTick() - t0p) end
                        if oi then return oi(self, tt, te) end
                    end
                end
            end
        end
        for _, r in wl do r.tk:SetFireState(0) IssueAttack({ r.tk }, r.en) end
        WaitTicks(80)
        for i = 1, 8 do
            local r = res[i]
            P(fmt('coll %s unitcheck n %d first %s | projcheck n %d other %s | impact %s | wall hits %d back hits %d', r.tag, r.n, tostring(r.v), r.pn,
                tostring(r.pother), tostring(r.imp), r.wh, r.bh))
        end
        for _, r in wl do
            P(fmt('coll weapon %s blocker check n %d last %s blocker hits %d enemy hits %d', r.tag, r.n, tostring(r.v), r.bh, r.eh))
        end
        for _, r in pp do
            P(fmt('coll pp %s dead-at-once %s a checks %d (%s) impact %s | b checks %d (%s) impact %s', r.tag, tostring(r.dead), r.ca, tostring(r.va), tostring(r.ia),
                r.cb, tostring(r.vb), tostring(r.ib)))
        end
        done.coll = true
    end))

    -- [tree] TreeFire
    ForkThread(guard('tree', function()
        local trees = {}
        for _, i in { 5 } do
            local cx, cz = LAND[i][1], LAND[i][2]
            local got = GetReclaimablesInRect(Rect(cx - 60, cz - 60, cx + 60, cz + 60)) or {}
            for _, e in got do
                if not e.UnitId and e.Burn and not e:BeenDestroyed() then table.insert(trees, e) end
            end
        end
        P(fmt('tree found %d', table.getn(trees)))
        -- pick isolated trees (no other tree within 3), far from each other
        local picks = { trees[1] }
        for _, tr in trees do
            if table.getn(picks) < 2 and VDist3(trees[1]:GetPosition(), tr:GetPosition()) >= 6 then table.insert(picks, tr) end
        end
        if table.getn(picks) < 2 then P('tree not enough isolated trees') done.tree = true return end
        local function ring(tr, tag)
            local p = tr:GetPosition()
            local us = {}
            for i, def in { { 'own', A1, 0.8, 0 }, { 'enemy', A2, -0.8, 0 }, { 'ally', A3, 0, 0.8 }, { 'enemy-far', A2, 0, -1.8 } } do
                local u = tough(make('ueb5101', def[2], p[1] + def[3], p[3] + def[4]))
                local r = { tag = def[1], u = u, n = 0, types = '' }
                u.OnDamage = function(self, inst, amt, vec, dt)
                    r.n = r.n + 1
                    if string.len(r.types) < 80 then r.types = r.types .. fmt('%s/%.1f/%s ', tostring(dt), amt, inst == tr and 'tree' or (inst and 'unit' or 'nil')) end
                end
                table.insert(us, r)
            end
            P(fmt('tree %s at %.1f,%.1f bp %s', tag, p[1], p[3], tostring(tr:GetBlueprint().BlueprintId)))
            return us, p
        end
        local function rep(tag, us)
            local line = 'tree ' .. tag
            for _, r in us do line = line .. fmt(' | %s n %d %s', r.tag, r.n, r.types) r.n = 0 r.types = '' end
            P(line)
        end
        local tr1, tr2 = picks[1], picks[2]
        local us1, p1 = ring(tr1, 'direct')
        WaitTicks(2)
        local function call(tag, f) local ok, err = pcall(f) if not ok then P('tree ' .. tag .. ' ERROR ' .. tostring(err)) end rep(tag, us1) end
        call('TreeFire.inst-tree.6args', function() DamageArea(tr1, p1, 1, 1, 'TreeFire', true) end)
        call('TreeFire.inst-tree.7args', function() DamageArea(tr1, p1, 1, 1, 'TreeFire', true, true) end)
        call('TreeFire.inst-nil', function() DamageArea(nil, p1, 1, 1, 'TreeFire', true, true) end)
        call('Normal.inst-tree', function() DamageArea(tr1, p1, 1, 1, 'Normal', true, true) end)
        call('Normal.inst-nil', function() DamageArea(nil, p1, 1, 1, 'Normal', true, true) end)
        call('Fire.inst-tree', function() DamageArea(tr1, p1, 1, 1, 'Fire', true, true) end)
        call('TreeFire.Damage-direct', function() Damage(tr1, p1, us1[1].u, 1, 'TreeFire') end)
        -- a real burn next to units
        local us2 = ring(tr2, 'burn')
        WaitTicks(2)
        tr2:Burn()
        P(fmt('tree burn Burning %s', tostring(tr2.Burning)))
        local b0 = GetGameTick()
        local seen = 0
        for t = 1, 520 do
            WaitTicks(1)
            local tot = 0
            for _, r in us2 do tot = tot + r.n end
            if tot > seen then P(fmt('tree burn damage at t+%d', GetGameTick() - b0)) seen = tot end
        end
        rep('burn.total', us2)
        P(fmt('tree burn tree destroyed %s', tostring(tr2:BeenDestroyed())))
        done.tree = true
    end))

    -- [edge] DamageArea reach
    WaitTicks(20)
    local units = { 'uea0203', 'uaa0303', 'ual0303', 'uaa0102' }
    local off = {}
    local dirs = { {'x',1,0,0}, {'xz',0.70710678,0,0.70710678}, {'xyz',0.57735027,0.57735027,0.57735027} }
    local edge = {}
    for i, bp in units do
        local st = LAND[({ 1, 2, 3, 4, 6, 8, 9, 10, 11, 12, 13, 14, 20, 22 })[i]]
        local u = make(bp, A2, st[1], st[2] + 16)
        u.PH = 0
        u.OnDamage = function(self) self.PH = self.PH + 1 end
        table.insert(edge, { bp = bp, u = u, off = off[i], tag = bp .. (off[i] and '.shieldoff' or '') })
    end
    feed()
    WaitTicks(25)
    for _, e in edge do
        if e.off then e.u:DisableShield() end
    end
    WaitTicks(5)
    for _, e in edge do
        local u = e.u
        local S = u.MyShield
        local sstate = 'none'
        if S then
            S.PH = 0
            S.OnDamage = function(self) self.PH = self.PH + 1 end
            sstate = fmt('up=%s size=%s', tostring(S:IsUp()), s(u:GetBlueprint().Defense.Shield.ShieldSize))
        end
        local b = u:GetBlueprint()
        local p = u:GetPosition()
        P(fmt('edge %s size %s %s %s sphere %s off %s %s %s y-ground %.3f layer %s shield %s', e.tag, s(b.SizeX), s(b.SizeY), s(b.SizeZ), s(b.SizeSphere),
            s(b.CollisionOffsetX), s(b.CollisionOffsetY), s(b.CollisionOffsetZ), p[2] - GetSurfaceHeight(p[1], p[3]), u:GetCurrentLayer(), sstate))
        local function hitAt(d, dist, r)
            local h0, s0 = u.PH, S and S.PH or 0
            DamageArea(nil, {p[1] + d[2] * dist, p[2] + d[3] * dist, p[3] + d[4] * dist}, r, 1, 'Normal', true, true)
            local uh, sh = u.PH > h0, S and S.PH > s0
            return uh or sh, uh, sh
        end
        for _, r in { 3, 3.25, 3.5, 3.75, 4, 4.25, 4.5, 5, 6 } do
            local line = fmt('edge %s r %.2f', e.tag, r)
            for _, d in dirs do
                local last, firstMiss, flips, prev, who = -1, -1, 0, nil, '-'
                local dist = 0
                while dist <= r + 16 do
                    local hit, uh, sh = hitAt(d, dist, r)
                    if hit then
                        last = dist
                        who = (uh and 'u' or '') .. (sh and 's' or '')
                    elseif firstMiss < 0 then firstMiss = dist end
                    if prev ~= nil and prev ~= hit then flips = flips + 1 end
                    prev = hit
                    dist = dist + 0.25
                end
                -- refine the last hit -> miss edge
                if last >= 0 then
                    local lo, hi = last, last + 0.25
                    for k = 1, 5 do
                        local mid = (lo + hi) / 2
                        if hitAt(d, mid, r) then lo = mid else hi = mid end
                    end
                    last = lo
                end
                line = line .. fmt(' %s:%.3f/%d%s', d[1], last, flips, who)
            end
            P(line)
        end
        u:Destroy()
    end
    while not (done.sh and done.proj and done.coll and done.tree) do WaitTicks(5) end
    WaitTicks(30)
    P('done')
end)
