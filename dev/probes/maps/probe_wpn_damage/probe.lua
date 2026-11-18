-- Differential probe: the damage API (Damage, DamageArea, DamageRing, Kill),
-- armour multipliers, callback arguments and health callbacks.
--   real game (Windows): dev/probes/run_faf.ps1 probe_wpn_damage 60
--   ours: cargo run -- --headless --no-ai --ticks 260 --script dev/probes/maps/probe_wpn_damage/probe.lua
-- Each section sits at its own flat site on SCMP_009. Lines: PROBE <section> ...
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
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    return u
end
local function tough(u, hp)
    u:SetMaxHealth(hp) u:SetHealth(nil, hp)
end
-- Replace OnDamage with a recorder (no health change).
local function record(u)
    u.PH = 0
    u.OnDamage = function(self, inst, amt, vec, t)
        self.PH = self.PH + 1
        self.PA = amt self.PV = vec self.PT = t self.PI = inst
    end
end
local function v3(v)
    if not v then return 'nil' end
    return fmt('%.3f,%.3f,%.3f', v[1] or v.x or 0, v[2] or v.y or 0, v[3] or v.z or 0)
end
local function s(v)
    if type(v) == 'number' then return fmt('%.3f', v) end
    return tostring(v)
end

ForkThread(function()
    WaitTicks(3)
    P('start')

    -- [armor] damage of each type on each armour class.
    local armorUnits = { 'uel0201', 'uel0001', 'ueb1101', 'ueb5101', 'xrl0302', 'uea0303', 'url0402', 'ueb4201', 'uel0105', 'ueb2302', 'xsb2401', 'uel0106' }
    local types = { 'Normal', 'Overcharge', 'Deathnuke', 'Nuke', 'ExperimentalFootfall', 'FireBeetleExplosion', 'CzarBeam', 'TacticalMissile', 'TreeForce', 'Reclaimed', 'Bogus' }
    for i, bp in armorUnits do
        local x, z = LAND[1][1] + (math.mod(i - 1, 4)) * 12 - 18, LAND[1][2] + math.floor((i - 1) / 4) * 14
        local u = make(bp, A2, x, z)
        tough(u, 100000)
        local seen
        local od = u.OnDamage
        u.OnDamage = function(self, inst, amt, vec, t)
            seen = amt
            return od(self, inst, amt, vec, t)
        end
        local line = fmt('armor %s %s', bp, tostring(u:GetBlueprint().Defense.ArmorType))
        for _, t in types do
            seen = nil
            local h0 = u:GetHealth()
            Damage(nil, u:GetPosition(), u, 100, t)
            line = line .. fmt(' %s=%s/%.1f', t, s(seen), h0 - u:GetHealth())
        end
        P(line)
        u:Destroy()
    end

    -- [vec] OnDamage arguments from Damage and DamageArea.
    do
        local x, z = LAND[2][1], LAND[2][2]
        local y = GetTerrainHeight(x, z)
        local t = make('ueb5101', A2, x, z) record(t)
        local inst = make('ueb5101', A1, x + 20, z)
        local tp = t:GetPosition()
        P(fmt('vec targetpos %s', v3(tp)))
        local offs = { {10,0,0}, {0,5,0}, {-3,0,4}, {0,0,0}, {0.5,0.5,0.5}, {0,-2,0} }
        for _, o in offs do
            Damage(inst, {tp[1] + o[1], tp[2] + o[2], tp[3] + o[3]}, t, 7, 'Normal')
            P(fmt('vec Damage off %s -> vec %s amt %s type %s inst %s', v3(o), v3(t.PV), s(t.PA), s(t.PT), tostring(t.PI == inst)))
        end
        Damage(nil, tp, t, 7, 'Fire')
        P(fmt('vec Damage nil-inst -> inst %s type %s', tostring(t.PI), s(t.PT)))
        for _, o in offs do
            t.PV = nil
            DamageArea(inst, {tp[1] + o[1], tp[2] + o[2], tp[3] + o[3]}, 12, 9, 'Normal', true, true)
            P(fmt('vec DamageArea off %s -> vec %s amt %s type %s inst %s', v3(o), v3(t.PV), s(t.PA), s(t.PT), tostring(t.PI == inst)))
        end
        DamageRing(inst, {tp[1] + 4, tp[2], tp[3]}, 1, 12, 11, 'Normal', true, true)
        P(fmt('vec DamageRing off 4 -> vec %s amt %s', v3(t.PV), s(t.PA)))
        t:Destroy() inst:Destroy()
    end

    -- [edge] the farthest centre distance at which DamageArea reaches each shape.
    local edgeUnits = { 'uel0201', 'ueb1101', 'ueb0101', 'uel0401', 'ueb5101', 'url0402', 'uea0203', 'uel0001', 'ueb2301', 'xsl0401', 'ual0303', 'ues0302' }
    local dirs = { {'x',1,0,0}, {'z',0,0,1}, {'xz',0.70710678,0,0.70710678}, {'up',0,1,0}, {'down',0,-1,0}, {'xup',0.70710678,0.70710678,0} }
    local edge = {}
    for i, bp in edgeUnits do
        local st = LAND[2 + i]
        local u
        if bp == 'ues0302' then
            u = CreateUnitHPR(bp, A2, 240, GetSurfaceHeight(240, 144), 144, 0, 0, 0)
            u:SetFireState(1)
        else
            u = make(bp, A2, st[1], st[2] + 16)
        end
        record(u)
        table.insert(edge, { bp = bp, u = u })
    end
    WaitTicks(20)
    for _, e in edge do
        local u = e.u
        local b = u:GetBlueprint()
        local p = u:GetPosition()
        P(fmt('edge %s size %s %s %s sphere %s off %s %s %s y-ground %.3f layer %s', e.bp, s(b.SizeX), s(b.SizeY), s(b.SizeZ), s(b.SizeSphere),
            s(b.CollisionOffsetX), s(b.CollisionOffsetY), s(b.CollisionOffsetZ), p[2] - GetSurfaceHeight(p[1], p[3]), u:GetCurrentLayer()))
        for _, r in { 0.5, 2, 5 } do
            local line = fmt('edge %s r %.1f', e.bp, r)
            for _, d in dirs do
                local last, firstMiss, flips, prev = -1, -1, 0, nil
                local dist = 0
                while dist <= r + 14 do
                    local h0 = u.PH
                    DamageArea(nil, {p[1] + d[2] * dist, p[2] + d[3] * dist, p[3] + d[4] * dist}, r, 1, 'Normal', true, true)
                    local hit = u.PH > h0
                    if hit then last = dist elseif firstMiss < 0 then firstMiss = dist end
                    if prev ~= nil and prev ~= hit then flips = flips + 1 end
                    prev = hit
                    dist = dist + 0.125
                end
                line = line .. fmt(' %s:%.3f/%.3f/%d', d[1], last, firstMiss, flips)
            end
            P(line)
        end
        -- radius 0 at the origin (the real game raises '0 radius specified')
        local h0 = u.PH
        local ok0 = pcall(DamageArea, nil, p, 0, 1, 'Normal', true, true)
        local a = u.PH - h0
        h0 = u.PH
        local ok1 = pcall(DamageArea, nil, {p[1], p[2] + (b.SizeY or 1) / 2, p[3]}, 0.01, 1, 'Normal', true, true)
        P(fmt('edge %s r0 origin ok %s hits %d; r0.01 centre ok %s hits %d', e.bp, tostring(ok0), a, tostring(ok1), u.PH - h0))
        -- ring 3..6 along x
        local line = fmt('edge %s ring3-6', e.bp)
        local prev
        local dist = 0
        while dist <= 16 do
            local h0 = u.PH
            DamageRing(nil, {p[1] + dist, p[2], p[3]}, 3, 6, 1, 'Normal', true, true)
            local hit = u.PH > h0
            if prev == nil or prev ~= hit then line = line .. fmt(' %s@%.3f', hit and 'hit' or 'miss', dist) end
            prev = hit
            dist = dist + 0.125
        end
        P(line)
        u:Destroy()
    end

    -- [args] who DamageArea / DamageRing hit by alliance and the two flags.
    do
        local x, z = LAND[16][1], LAND[16][2]
        local y = GetTerrainHeight(x, z)
        local set = {
            { 'I', make('ueb5101', A1, x + 2, z) },
            { 'F', make('ueb5101', A1, x - 2, z) },
            { 'A', make('ueb5101', A3, x, z + 2) },
            { 'E', make('ueb5101', A2, x, z - 2) },
            { 'N', make('ueb5101', NEU, x + 2, z + 2) },
        }
        for _, e in set do record(e[2]) end
        local I = set[1][2]
        local function who(f)
            for _, e in set do e[2].PH = 0 end
            local ok, err = pcall(f)
            if not ok then return 'error' end
            local out = ''
            for _, e in set do if e[2].PH > 0 then out = out .. e[1] .. (e[2].PH > 1 and e[2].PH or '') end end
            return out == '' and '-' or out
        end
        local c = {x, y, z}
        P('args area I,t,t ' .. who(function() DamageArea(I, c, 5, 1, 'Normal', true, true) end))
        P('args area I,t,f ' .. who(function() DamageArea(I, c, 5, 1, 'Normal', true, false) end))
        P('args area I,f,t ' .. who(function() DamageArea(I, c, 5, 1, 'Normal', false, true) end))
        P('args area I,f,f ' .. who(function() DamageArea(I, c, 5, 1, 'Normal', false, false) end))
        P('args area I,t ' .. who(function() DamageArea(I, c, 5, 1, 'Normal', true) end))
        P('args area I,f ' .. who(function() DamageArea(I, c, 5, 1, 'Normal', false) end))
        P('args area I ' .. who(function() DamageArea(I, c, 5, 1, 'Normal') end))
        P('args area nil,t,t ' .. who(function() DamageArea(nil, c, 5, 1, 'Normal', true, true) end))
        P('args area nil,f,f ' .. who(function() DamageArea(nil, c, 5, 1, 'Normal', false, false) end))
        P('args area nil,f ' .. who(function() DamageArea(nil, c, 5, 1, 'Normal', false) end))
        P('args ring I,t,t ' .. who(function() DamageRing(I, c, 0, 5, 1, 'Normal', true, true) end))
        P('args ring I,t,f ' .. who(function() DamageRing(I, c, 0, 5, 1, 'Normal', true, false) end))
        P('args ring I,f,t ' .. who(function() DamageRing(I, c, 0, 5, 1, 'Normal', false, true) end))
        P('args ring I,f,f ' .. who(function() DamageRing(I, c, 0, 5, 1, 'Normal', false, false) end))
        P('args ring nil,f,f ' .. who(function() DamageRing(nil, c, 0, 5, 1, 'Normal', false, false) end))
        P('args area dmg0 ' .. who(function() DamageArea(I, c, 5, 0, 'Normal', true, true) end))
        P('args area neg ' .. who(function() DamageArea(I, c, 5, -3, 'Normal', true, true) end))
        P('args Damage dmg0 ' .. who(function() Damage(I, c, set[4][2], 0, 'Normal') end))
        P('args Damage self ' .. who(function() Damage(I, c, I, 1, 'Normal') end))
        P('args Damage friend ' .. who(function() Damage(I, c, set[2][2], 1, 'Normal') end))
        -- falloff: amount by distance
        for _, e in set do e[2].PA = nil end
        DamageArea(nil, {x + 2, y, z}, 10, 100, 'Normal', true, true)
        local line = 'args falloff'
        for _, e in set do line = line .. ' ' .. e[1] .. '=' .. s(e[2].PA) end
        P(line)
        for _, e in set do e[2]:Destroy() end
    end

    -- [kill] Kill / lethal damage / flags.
    do
        local x, z = LAND[17][1], LAND[17][2]
        local inst = make('ueb5101', A1, x - 10, z)
        local function victim(tag, bp, dx)
            local u = make(bp, A2, x + dx, z)
            u.tag = tag u.K = 0 u.D = 0
            local ok = u.OnKilled
            u.OnKilled = function(self, i, t, over)
                self.K = self.K + 1
                P(fmt('kill %s OnKilled#%d tick+%d inst %s type %s over %s health %.1f dead %s', tag, self.K, GetGameTick() - self.T0, i == inst and 'inst' or tostring(i ~= nil), s(t), s(over), self:GetHealth(), tostring(self.Dead)))
                return ok(self, i, t, over)
            end
            local od = u.OnDamage
            u.OnDamage = function(self, i, a, v, t)
                self.D = self.D + 1
                return od(self, i, a, v, t)
            end
            local odes = u.OnDestroy
            u.OnDestroy = function(self)
                P(fmt('kill %s OnDestroy tick+%d', tag, GetGameTick() - self.T0))
                return odes(self)
            end
            u.T0 = GetGameTick()
            return u
        end
        local function st(u)
            return fmt('health %.1f dead %s destroyed %s K %d D %d', u:BeenDestroyed() and -1 or u:GetHealth(), tostring(u.Dead), tostring(u:BeenDestroyed()), u.K, u.D)
        end
        local hp = make('uel0201', A2, x + 60, z):GetHealth()
        P(fmt('kill tank health %.1f', hp))
        local a = victim('Kill', 'uel0201', 0)
        a:Kill()
        P('kill Kill after ' .. st(a))
        a:Kill()
        Damage(inst, a:GetPosition(), a, 50, 'Normal')
        P('kill Kill again+damage ' .. st(a))
        local b = victim('lethal', 'uel0201', 6)
        Damage(inst, b:GetPosition(), b, hp + 150, 'Normal')
        P('kill lethal after ' .. st(b))
        local c = victim('exact', 'uel0201', 12)
        Damage(inst, c:GetPosition(), c, hp, 'Normal')
        P('kill exact after ' .. st(c))
        local d = victim('nokill', 'uel0201', 18)
        d:SetCanBeKilled(false)
        Damage(inst, d:GetPosition(), d, hp + 150, 'Normal')
        P('kill nokill after ' .. st(d))
        d:Kill()
        P('kill nokill Kill() ' .. st(d))
        local e = victim('nodmg', 'uel0201', 24)
        e:SetCanTakeDamage(false)
        Damage(inst, e:GetPosition(), e, 50, 'Normal')
        DamageArea(inst, e:GetPosition(), 1, 50, 'Normal', true, true)
        P('kill nodmg after ' .. st(e))
        e:Kill()
        P('kill nodmg Kill() ' .. st(e))
        local f = victim('destroy', 'uel0201', 30)
        f:Destroy()
        P(fmt('kill destroy after destroyed %s K %d', tostring(f:BeenDestroyed()), f.K))
        local g = victim('area', 'uel0201', 36)
        DamageArea(inst, g:GetPosition(), 2, hp * 3, 'Normal', true, true)
        P('kill area after ' .. st(g))
        local h = victim('nilinst', 'uel0201', 42)
        Damage(nil, h:GetPosition(), h, hp * 2, 'Deathnuke')
        P('kill nilinst after ' .. st(h))
        local k = victim('sethealth0', 'uel0201', 48)
        k:SetHealth(nil, 0)
        P('kill sethealth0 after ' .. st(k))
        local m = victim('adjust', 'uel0201', 54)
        m:AdjustHealth(inst, -hp - 10)
        P('kill adjust after ' .. st(m))
        local all = { a, b, c, d, e, g, h, k, m }
        local gone = {}
        for t = 1, 120 do
            WaitTicks(1)
            for _, u in all do
                if not gone[u.tag] and u:BeenDestroyed() then
                    gone[u.tag] = true
                    P(fmt('kill %s destroyed tick+%d', u.tag, GetGameTick() - u.T0))
                end
            end
        end
        local rect = { x0 = x - 5, y0 = z - 5, x1 = x + 60, y1 = z + 5 }
        local recl = GetReclaimablesInRect(Rect(x - 5, z - 5, x + 60, z + 5)) or {}
        local props = 0
        for _, r in recl do
            if IsProp(r) then
                props = props + 1
                local pp = r:GetPosition()
                P(fmt('kill wreck at dx %.2f mass %s hp %.1f', pp[1] - x, s(r.MaxMassReclaim), r:GetHealth()))
            end
        end
        P(fmt('kill props %d', props))
        for _, u in all do if not u:BeenDestroyed() then P('kill alive ' .. u.tag .. ' ' .. st(u)) u:Destroy() end end
    end

    -- [health] OnHealthChanged calls.
    do
        local x, z = LAND[18][1], LAND[18][2]
        local u = make('ueb1101', A2, x, z)
        tough(u, 1000)
        local t0 = GetGameTick()
        local step = 'init'
        u.OnHealthChanged = function(self, new, old)
            P(fmt('health [%s] OnHealthChanged tick+%d new %s old %s health %.1f', step, GetGameTick() - t0, s(new), s(old), self:GetHealth()))
        end
        WaitTicks(2)
        local function dmg(tag, n) step = tag Damage(nil, u:GetPosition(), u, n, 'Normal') P(fmt('health [%s] tick+%d health %.1f', tag, GetGameTick() - t0, u:GetHealth())) end
        dmg('d100', 100) WaitTicks(3)
        dmg('d200', 200) WaitTicks(3)
        dmg('d300a', 300) dmg('d300b', 300) WaitTicks(3)
        step = 'set900' u:SetHealth(nil, 900) P(fmt('health [set900] tick+%d health %.1f', GetGameTick() - t0, u:GetHealth())) WaitTicks(3)
        step = 'adj-500' u:AdjustHealth(nil, -500) P(fmt('health [adj-500] tick+%d health %.1f', GetGameTick() - t0, u:GetHealth())) WaitTicks(3)
        step = 'adj+5000' u:AdjustHealth(nil, 5000) P(fmt('health [adj+5000] tick+%d health %.1f max %.1f', GetGameTick() - t0, u:GetHealth(), u:GetMaxHealth())) WaitTicks(3)
        step = 'set5000' u:SetHealth(nil, 5000) P(fmt('health [set5000] tick+%d health %.1f', GetGameTick() - t0, u:GetHealth())) WaitTicks(3)
        step = 'setmax500' u:SetMaxHealth(500) P(fmt('health [setmax500] tick+%d health %.1f max %.1f', GetGameTick() - t0, u:GetHealth(), u:GetMaxHealth())) WaitTicks(3)
        step = 'd10' dmg('d10', 10) u:SetRegenRate(7)
        for i = 1, 12 do
            WaitTicks(1)
            P(fmt('health regen7 tick+%d health %.2f', i, u:GetHealth()))
        end
        u:SetRegenRate(0)
        step = 'neg' dmg('neg', -50) WaitTicks(2)
        step = 'kill' dmg('kill', 99999) WaitTicks(3)
    end
    WaitTicks(30)
    P('done')
end)
