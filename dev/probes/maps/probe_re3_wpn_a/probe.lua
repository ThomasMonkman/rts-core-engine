-- Differential probe (sweep 3, weapons A): SetTargetGround (gt.*), rack-charge
-- salvos (rack.*), SetStunned durations (stun.*).
--   real game (Windows): dev/probes/run_faf.ps1 probe_re3_wpn_a 150
--   ours: --headless --ticks 900 --script dev/probes/maps/probe_re3_wpn_a/probe.lua
-- Ticks are from each lane's T0. Positions are relative to the shooter.
local fmt = string.format
local function P(s) LOG('PROBE ' .. s) end
local function wipe()
    for _, brain in ArmyBrains do
        for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
            if not u:BeenDestroyed() then u:Destroy() end
        end
    end
end
wipe()
local armies = ListArmies()
local A1, A2, A3 = armies[1], armies[2], armies[3]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
SetAlliance(A3, A2, 'Enemy') SetAlliance(A2, A3, 'Enemy')
local LAND = {
    {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232},
    {664,320}, {848,320}, {904,384}, {656,416}, {568,440}, {456,552}, {368,568}, {120,608},
    {176,672}, {360,672}, {256,760}, {88,776}, {248,856}, {352,856}, {104,896}, {408,896},
}
local function list(t, n)
    local out = ''
    for i = 1, math.min(table.getn(t), n) do out = out .. (i > 1 and ',' or '') .. t[i] end
    return out == '' and '-' or out
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

local lanes = {}
-- opts: targets = { {bp, dz, dx} } (army A2), army, start(l), at = { [t] = fn }, every(l, t), allw (hook all weapons)
local function lane(tag, bp, opts)
    opts = opts or {}
    local site = LAND[table.getn(lanes) + 1]
    local x, z = site[1], site[2]
    local u = CreateUnitHPR(bp, opts.army or A1, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    u:SetFireState(1)
    u:SetIntelRadius('Vision', 70)
    local l = { tag = tag, u = u, x = x, z = z, opts = opts, ts = {}, notes = {}, ws = {} }
    for _, td in opts.targets or {} do
        local tx, tz = x + (td[3] or 0), z + td[2]
        local t = CreateUnitHPR(td[1], A2, tx, GetTerrainHeight(tx, tz), tz, 0, 0, 0)
        if t.SetFireState then t:SetFireState(1) end
        t:SetMaxHealth(1000000) t:SetHealth(nil, 1000000)
        local r = { u = t, hits = 0 }
        t.OnDamage = function(self, inst, amt, vec, dt)
            if not l.T0 then return end
            r.hits = r.hits + 1
            if not r.first then r.first = GetGameTick() - l.T0 end
        end
        table.insert(l.ts, r)
    end
    l.t = l.ts[1] and l.ts[1].u
    local nw = opts.allw and u:GetWeaponCount() or math.min(1, u:GetWeaponCount())
    for wi = 1, nw do
        local wep = u:GetWeapon(wi)
        local w = { wep = wep, label = tostring(wep:GetBlueprint().Label), fires = {}, nfire = 0, shots = {}, nshot = 0, imps = {} }
        table.insert(l.ws, w)
        if wi == 1 then l.w = wep end
        hook(wep, 'OnFire', function()
            if l.T0 then
                w.nfire = w.nfire + 1
                if w.nfire <= 16 then table.insert(w.fires, GetGameTick() - l.T0) end
            end
        end)
        local create = wep.CreateProjectileAtMuzzle
        if create then
            wep.CreateProjectileAtMuzzle = function(self, muzzle)
                local p = create(self, muzzle)
                if l.T0 then
                    w.nshot = w.nshot + 1
                    if w.nshot <= 40 then table.insert(w.shots, GetGameTick() - l.T0) end
                    if type(p) == 'table' and p.OnImpact and table.getn(w.imps) < 10 then
                        local born = GetGameTick() - l.T0
                        local rec = { born = born }
                        table.insert(w.imps, rec)
                        local oi = p.OnImpact
                        p.OnImpact = function(pp, tt, te)
                            if not rec.tt then
                                local pos = pp:GetPosition()
                                rec.tt = tostring(tt) rec.dx = pos[1] - x rec.dz = pos[3] - z
                            end
                            return oi(pp, tt, te)
                        end
                    end
                end
                return p
            end
        end
    end
    table.insert(lanes, l)
    return l
end
local function note(l, s) table.insert(l.notes, fmt('%d:%s', GetGameTick() - l.T0, s)) end

local function report(l)
    for _, w in l.ws do
        local line = fmt('%s w=%s fire %s x%d shots %s x%d', l.tag, w.label, list(w.fires, 16), w.nfire, list(w.shots, 40), w.nshot)
        P(line)
        if table.getn(w.imps) > 0 then
            local s = fmt('%s w=%s imps', l.tag, w.label)
            for _, i in w.imps do
                s = s .. fmt(' [b%d %s %s]', i.born, i.tt or 'none', i.dx and fmt('%.1f,%.1f', i.dx, i.dz) or '-')
            end
            P(s)
        end
    end
    for i, r in l.ts do P(fmt('%s t%d hits %d first %s', l.tag, i, r.hits, tostring(r.first))) end
    if table.getn(l.notes) > 0 then P(fmt('%s notes %s', l.tag, list(l.notes, 60))) end
end

local function run(RUN, build)
    lanes = {}
    for _, a in { A1, A2 } do
        local brain = GetArmyBrain(a)
        brain:GiveStorage('ENERGY', 1000000) brain:GiveResource('ENERGY', 1000000)
    end
    build()
    WaitTicks(20)
    for _, l in lanes do
        l.T0 = GetGameTick()
        if l.opts.start then
            local ok, err = pcall(l.opts.start, l)
            if not ok then note(l, 'starterror ' .. tostring(err)) end
        end
    end
    for t = 1, RUN do
        WaitTicks(1)
        if math.mod(t, 10) == 0 then GetArmyBrain(A1):GiveResource('ENERGY', 1000000) end
        for _, l in lanes do
            local f = l.opts.at and l.opts.at[t]
            if f then
                local ok, err = pcall(f, l)
                if not ok then note(l, 'error ' .. tostring(err)) end
            end
            if l.opts.every then
                local ok, err = pcall(l.opts.every, l, t)
                if not ok and not l.everr then l.everr = true note(l, 'everyerror ' .. tostring(err)) end
            end
        end
    end
    for _, l in lanes do report(l) end
    wipe()
    WaitTicks(8)
end

-- target state at a few ticks
local function tstate(l, tag)
    local tp = l.w:GetCurrentTargetPos()
    local ct = l.w:GetCurrentTarget()
    note(l, fmt('%s has=%s cur=%s pos=%s', tag, tostring(l.w:WeaponHasTarget()), ct and (ct == l.t and 'enemy' or 'other') or 'nil',
        tp and fmt('%.1f/%.1f', tp[1] - l.x, tp[3] - l.z) or 'nil'))
end
local function gp(l) return { l.x + 10, GetTerrainHeight(l.x + 10, l.z), l.z } end

ForkThread(function()
    WaitTicks(3)
    P('start')
    -- [gt] SetTargetGround at T+40; ground point at +10 x, enemy (if any) at +12 z
    run(150, function()
        for _, bp in { 'ueb2101', 'uel0201', 'uel0103' } do
            for _, enemy in { true, false } do
                local tg = enemy and { { 'ueb1101', 12 } } or {}
                local en = enemy and 'enemy' or 'none'
                local function at(fs, fire)
                    local a = {}
                    a[39] = function(l) tstate(l, 'pre') end
                    a[40] = function(l)
                        l.w:SetTargetGround(gp(l))
                        tstate(l, 'set')
                    end
                    a[41] = function(l) tstate(l, 'post1') end
                    if fire then a[42] = function(l) l.w:FireWeapon() note(l, 'FireWeapon') tstate(l, 'fired') end end
                    a[45] = function(l) tstate(l, 'post5') end
                    a[60] = function(l) tstate(l, 'post20') end
                    a[100] = function(l) tstate(l, 'post60') end
                    return a
                end
                lane(fmt('gt.%s.%s.fs0', bp, en), bp, { targets = tg, start = function(l) l.u:SetFireState(0) end, at = at(0) })
                lane(fmt('gt.%s.%s.fs1', bp, en), bp, { targets = tg, start = function(l) l.u:SetFireState(1) end, at = at(1) })
                lane(fmt('gt.%s.%s.fs0fire', bp, en), bp, { targets = tg, start = function(l) l.u:SetFireState(0) end, at = at(0, true) })
            end
        end
    end)
    -- [rack] rack-charge weapons on an attack order; [stun] durations
    run(240, function()
        local function atk(l) l.u:SetFireState(0) IssueAttack({ l.u }, l.t) end
        lane('rack.del0204', 'del0204', { targets = { { 'ueb1101', 20 } }, allw = true, start = atk })
        lane('rack.xeb2306', 'xeb2306', { targets = { { 'ueb1101', 40 } }, allw = true, start = atk })
        lane('rack.xeb2306.noE', 'xeb2306', { army = A3, targets = { { 'ueb1101', 40 } }, allw = true, start = atk })
        lane('rack.del0204.ret', 'del0204', { targets = { { 'ueb1101', 20 } }, allw = true, start = function(l) l.u:SetFireState(0) end })
        local function stun(bp, at, d)
            local a = {}
            a[at] = function(l) l.u:SetStunned(d) l.sat = GetGameTick() - l.T0 note(l, fmt('stun%.1f stunned=%s', d, tostring(l.u:IsStunned()))) end
            lane(fmt('stun.%s.d%s.at%d', bp, tostring(d), at), bp, { targets = { { 'ueb1101', 12 } }, start = atk, at = a,
                every = function(l, t)
                    if l.sat then
                        local s = l.u:IsStunned()
                        if s ~= l.prev then note(l, fmt('IsStunned=%s', tostring(s))) l.prev = s end
                    end
                end })
        end
        stun('uel0201', 30, 1) stun('uel0201', 30, 2) stun('uel0201', 30, 4) stun('uel0201', 30, 8)
        stun('uel0201', 33, 4) stun('uel0201', 37, 4) stun('uel0201', 30, 0.5) stun('ueb2101', 30, 4) stun('uel0103', 30, 3)
    end)
    WaitTicks(30)
    P('done')
end)
