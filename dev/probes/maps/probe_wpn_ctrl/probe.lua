-- Differential probe: fire states, the weapon control API (SetEnabled,
-- ChangeMaxRadius, ChangeRateOfFire, SetTargetingPriorities, ...), stun,
-- spread, retargeting, veterancy, death weapons, bombers and artillery arcs.
--   real game (Windows): dev/probes/run_faf.ps1 probe_wpn_ctrl 120
--   ours: cargo run -- --headless --no-ai --ticks 420 --script dev/probes/maps/probe_wpn_ctrl/probe.lua
-- One lane per case; ticks count from the lane's start (T0). Lines: PROBE ctl <case> ...
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
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
local LAND = {
    {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232},
    {664,320}, {848,320}, {904,384}, {656,416}, {568,440}, {456,552}, {368,568}, {120,608},
    {176,672}, {360,672}, {256,760}, {88,776}, {248,856}, {352,856}, {104,896}, {408,896},
}
local RUN = 150
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
-- opts: targets = { {bp, dz, dx, hp} }, start(l), at = { [tick] = fn(l) }, army, track (log impacts), every(l, t)
local function lane(tag, bp, opts)
    opts = opts or {}
    local site = LAND[table.getn(lanes) + 1]
    local x, z = site[1], site[2]
    local u = CreateUnitHPR(bp, opts.army or A1, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    u:SetFireState(1)
    u:SetIntelRadius('Vision', 70)
    local l = { tag = tag, u = u, x = x, z = z, opts = opts, ts = {}, fires = {}, nfire = 0, nshot = 0, imps = {}, notes = {} }
    for i, td in opts.targets or { { 'ueb1101', 12 } } do
        local tx, tz = x + (td[3] or 0), z + td[2]
        local t = CreateUnitHPR(td[1], opts.army and A1 or A2, tx, GetTerrainHeight(tx, tz), tz, 0, 0, 0)
        if t.SetFireState then t:SetFireState(1) end
        local r = { u = t, hits = 0, sum = 0, amts = {}, types = {}, typeList = {} }
        if td[4] then
            t:SetMaxHealth(td[4]) t:SetHealth(nil, td[4])
            hook(t, 'OnKilled', function() r.died = GetGameTick() - (l.T0 or 0) end)
        else
            t:SetMaxHealth(1000000) t:SetHealth(nil, 1000000)
            t.OnDamage = function(self, inst, amt, vec, dt)
                if not l.T0 then return end
                r.hits = r.hits + 1
                r.sum = r.sum + amt
                if not r.first then r.first = GetGameTick() - l.T0 end
                if table.getn(r.amts) < 3 then table.insert(r.amts, fmt('%.1f', amt)) end
                local k = tostring(dt)
                if not r.types[k] then r.types[k] = true table.insert(r.typeList, k) end
            end
        end
        table.insert(l.ts, r)
    end
    l.t = l.ts[1] and l.ts[1].u
    if u:GetWeaponCount() > 0 then
        local wep = u:GetWeapon(1)
        l.w = wep
        hook(wep, 'OnFire', function()
            if l.T0 then
                l.nfire = l.nfire + 1
                if l.nfire <= 12 then table.insert(l.fires, GetGameTick() - l.T0) end
            end
        end)
        local create = wep.CreateProjectileAtMuzzle
        if create then
            wep.CreateProjectileAtMuzzle = function(self, muzzle)
                local p = create(self, muzzle)
                if l.T0 then
                    l.nshot = l.nshot + 1
                    if not l.firstShot then
                        l.firstShot = GetGameTick() - l.T0
                        if opts.onShot then opts.onShot(l, p) end
                    end
                    if type(p) == 'table' and opts.track and p.OnImpact then
                        local born = GetGameTick()
                        if opts.apex and not l.apexP then l.apexP = p l.apex = -1e9 end
                        local oi = p.OnImpact
                        p.OnImpact = function(pp, tt, te)
                            if not pp.Seen then
                                pp.Seen = true
                                local pos = pp:GetPosition()
                                table.insert(l.imps, { pos[1] - l.ax, pos[3] - l.az, GetGameTick() - born, tostring(tt) })
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
local function attack(l) l.u:SetFireState(0) IssueAttack({ l.u }, l.t) end
local function note(l, s) table.insert(l.notes, fmt('%d:%s', GetGameTick() - l.T0, s)) end

local function report(l)
    local line = fmt('ctl %s fire %s x%d shots x%d', l.tag, list(l.fires, 12), l.nfire, l.nshot)
    for i, r in l.ts do
        if r.died or l.opts.targets and l.opts.targets[i][4] then
            line = line .. fmt(' | t%d died %s', i, tostring(r.died))
        else
            line = line .. fmt(' | t%d hits %d sum %.0f first %s amts %s types %s', i, r.hits, r.sum, tostring(r.first), list(r.amts, 3), list(r.typeList, 3))
        end
    end
    P(line)
    if table.getn(l.notes) > 0 then P(fmt('ctl %s notes %s', l.tag, list(l.notes, 40))) end
    local n = table.getn(l.imps)
    if n > 0 then
        local sx, sz, sxx, szz = 0, 0, 0, 0
        for _, i in l.imps do sx = sx + i[1] sz = sz + i[2] end
        local mx, mz = sx / n, sz / n
        for _, i in l.imps do sxx = sxx + (i[1] - mx) * (i[1] - mx) szz = szz + (i[2] - mz) * (i[2] - mz) end
        local first = ''
        for k = 1, math.min(n, 3) do first = first .. fmt(' (%.2f,%.2f t%d %s)', l.imps[k][1], l.imps[k][2], l.imps[k][3], l.imps[k][4]) end
        P(fmt('ctl %s impacts n %d mean %.2f,%.2f std %.2f,%.2f first%s apex %s', l.tag, n, mx, mz, math.sqrt(sxx / n), math.sqrt(szz / n), first,
            l.apex and fmt('%.2f', l.apex) or '-'))
    end
end

local function run(build)
    lanes = {}
    local brain = GetArmyBrain(A1)
    brain:GiveStorage('ENERGY', 1000000) brain:GiveResource('ENERGY', 1000000)
    build()
    WaitTicks(20)
    for _, l in lanes do
        l.T0 = GetGameTick()
        l.ax, l.az = l.x, l.z + 20
        if l.t then local tp = l.t:GetPosition() l.ax, l.az = tp[1], tp[3] end
        if l.opts.start then
            local ok = pcall(l.opts.start, l)
            if not ok then note(l, 'starterror') end
        else
            attack(l)
        end
    end
    for t = 1, RUN do
        WaitTicks(1)
        for _, l in lanes do
            local f = l.opts.at and l.opts.at[t]
            if f then
                local ok, err = pcall(f, l)
                if not ok then note(l, 'error') end
            end
            if l.opts.every then l.opts.every(l, t) end
            if l.apexP and not l.apexP:BeenDestroyed() then
                local y = l.apexP:GetPosition()[2] - GetTerrainHeight(l.x, l.z)
                if y > l.apex then l.apex = y end
            end
        end
    end
    for _, l in lanes do report(l) end
    wipe()
    WaitTicks(8)
end

local function ground(l, dz)
    l.u:SetFireState(0)
    l.ax, l.az = l.x, l.z + dz
    IssueAttack({ l.u }, { l.x, GetTerrainHeight(l.x, l.z + dz), l.z + dz })
end

ForkThread(function()
    WaitTicks(3)
    P('start')
    run(function()
        lane('hold+attack', 'uel0201', { start = function(l) IssueAttack({ l.u }, l.t) end })
        lane('hold-idle', 'ueb2101', { start = function(l) end })
        lane('return-idle', 'ueb2101', { start = function(l) l.u:SetFireState(0) end })
        lane('ground-idle', 'ueb2101', { start = function(l) l.u:SetFireState(2) end })
        lane('groundattack', 'ueb2101', { targets = {}, track = true, start = function(l) ground(l, 15) end })
        lane('enable', 'ueb2101', { start = function(l) attack(l) l.w:SetEnabled(false) end, at = { [50] = function(l) l.w:SetEnabled(true) end } })
        lane('maxradius', 'ueb2101', { start = function(l) attack(l) l.w:ChangeMaxRadius(8) end, at = { [50] = function(l) l.w:ChangeMaxRadius(30) end } })
        lane('minradius', 'ueb2101', { start = function(l) attack(l) l.w:ChangeMinRadius(15) end, at = { [50] = function(l) l.w:ChangeMinRadius(0) end } })
        lane('rof', 'uel0201', { start = function(l) attack(l) l.w:ChangeRateOfFire(2.5) end, at = { [60] = function(l) l.w:ChangeRateOfFire(0.5) end } })
        lane('dmg', 'uel0201', { targets = { { 'ueb1101', 12 }, { 'ueb5101', 12, 2 } },
            start = function(l)
                attack(l)
                note(l, 'ChangeDamage=' .. tostring(pcall(function() l.w:ChangeDamage(77) end)))
                note(l, 'ChangeDamageRadius=' .. tostring(pcall(function() l.w:ChangeDamageRadius(4) end)))
                note(l, 'ChangeDamageType=' .. tostring(pcall(function() l.w:ChangeDamageType('Fire') end)))
            end })
        local prioTargets = { { 'ueb5101', 8, -3 }, { 'ueb1101', 12, 3 }, { 'uel0201', 10, 0 } }
        lane('prio-default', 'ueb2101', { targets = prioTargets, start = function(l) l.u:SetFireState(0) end })
        lane('prio-wall', 'ueb2101', { targets = prioTargets, start = function(l) l.w:SetTargetingPriorities({ ParseEntityCategory('WALL'), ParseEntityCategory('ALLUNITS') }) l.u:SetFireState(0) end })
        lane('prio-energy', 'ueb2101', { targets = prioTargets, start = function(l) l.w:SetTargetingPriorities({ ParseEntityCategory('ENERGYPRODUCTION'), ParseEntityCategory('ALLUNITS') }) l.u:SetFireState(0) end })
        lane('prio-energyonly', 'ueb2101', { targets = prioTargets, start = function(l) l.w:SetTargetingPriorities({ ParseEntityCategory('ENERGYPRODUCTION') }) l.u:SetFireState(0) end,
            at = { [80] = function(l) l.ts[2].u:Destroy() end } })
        lane('layercaps', 'ueb2101', { start = function(l) attack(l) l.w:SetFireTargetLayerCaps('Air') end, at = { [60] = function(l) l.w:SetFireTargetLayerCaps('Land|Water') end } })
        lane('stun', 'uel0201', { at = { [30] = function(l) l.u:SetStunned(4) end, [35] = function(l) note(l, 'stunned=' .. tostring(l.u:IsStunned())) end,
            [80] = function(l) note(l, 'stunned=' .. tostring(l.u:IsStunned())) end } })
        lane('spread3', 'ueb2101', { targets = {}, track = true, start = function(l) l.w:SetFiringRandomness(3) note(l, fmt('rand=%.2f', l.w:GetFiringRandomness())) ground(l, 20) end })
        lane('manual', 'ueb2101', { start = function(l) l.w:SetTargetEntity(l.t) end,
            at = { [10] = function(l) note(l, fmt('has=%s cur=%s', tostring(l.w:WeaponHasTarget()), tostring(l.w:GetCurrentTarget() == l.t))) end,
                [40] = function(l) l.w:ResetTarget() end,
                [50] = function(l) note(l, fmt('has=%s nfire=%d', tostring(l.w:WeaponHasTarget()), l.nfire)) end,
                [60] = function(l) note(l, fmt('FireWeapon=%s', tostring(l.w:FireWeapon()))) end,
                [70] = function(l) note(l, fmt('nfire=%d', l.nfire)) l.u:SetFireState(0) l.w:SetTargetGround({ l.x + 10, GetTerrainHeight(l.x + 10, l.z), l.z }) end,
                [110] = function(l) local tp = l.w:GetCurrentTargetPos() note(l, fmt('nfire=%d has=%s pos=%s', l.nfire, tostring(l.w:WeaponHasTarget()), tp and fmt('%.1f/%.1f', tp[1] - l.x, tp[3] - l.z) or 'nil')) end } })
        lane('fireclock', 'uel0201', { every = function(l, t) if t <= 26 then table.insert(l.notes, fmt('%.2f%s', l.w:GetFireClockPct(), l.w:CanFire() and 'c' or '')) end end })
        lane('retarget', 'ueb2101', { targets = { { 'ueb5101', 10, -4, 120 }, { 'ueb5101', 10, 0, 120 }, { 'ueb5101', 10, 4, 120 } }, start = function(l) l.u:SetFireState(0) end })
        lane('vet', 'uel0201', { targets = { { 'ueb1101', 12, 0, 40 } },
            at = { [60] = function(l) note(l, fmt('xp=%s level=%s', tostring(l.u.VetExperience), tostring(l.u.VetLevel))) end } })
        -- deaths: the dying unit is army 2, recorder walls army 1
        lane('death-acu', 'uel0001', { army = A2, targets = { { 'ueb5101', 5 }, { 'ueb5101', 15 }, { 'ueb5101', 30 }, { 'ueb5101', 39 } }, start = function(l) l.u:Kill() end })
        lane('death-ml', 'url0402', { army = A2, targets = { { 'ueb5101', 4 }, { 'ueb5101', 9 } }, start = function(l) l.u:Kill() end })
        lane('death-gunship', 'uea0203', { army = A2, targets = { { 'ueb5101', 0 }, { 'ueb5101', 3 } },
            start = function(l)
                hook(l.u, 'OnImpact', function(self, with) note(l, 'OnImpact=' .. tostring(with)) end)
                l.u:Kill()
            end })
    end)
    run(function()
        local function bomber(bp)
            lane('bomber-' .. bp, bp, { targets = { { 'ueb1201', 38 } }, track = true,
                onShot = function(l, p)
                    local up, tp = l.u:GetPosition(), l.t:GetPosition()
                    local vx, vy, vz = l.u:GetVelocity()
                    note(l, fmt('drop dz=%.1f dx=%.1f alt=%.1f v=%.2f', up[3] - tp[3], up[1] - tp[1], up[2] - tp[2], math.sqrt(vx * vx + vz * vz) * 10))
                end })
        end
        bomber('uea0103') bomber('uaa0103') bomber('ura0103') bomber('xsa0103') bomber('uea0304') bomber('uaa0304')
        local function arty(bp, d)
            lane('arc-' .. bp, bp, { targets = { { 'ueb1201', d } }, track = true, apex = true })
        end
        arty('uel0103', 25) arty('ual0103', 25) arty('url0103', 25) arty('xsl0103', 25) arty('uel0304', 38) arty('url0304', 38)
        arty('ueb2303', 38) arty('uel0111', 38) arty('xsl0111', 38) arty('uel0202', 15) arty('xel0305', 20) arty('ual0304', 38)
        -- DoT and beams on a recorder target
        lane('dot-uab2303', 'uab2303', { targets = { { 'ueb1201', 38 } } })
        lane('dot-url0001', 'xrl0305', { targets = { { 'ueb1201', 15 } } })
        lane('beam-ual0401', 'ual0401', { targets = { { 'ueb1201', 20 } } })
        lane('beam-url0402', 'url0402', { targets = { { 'ueb1201', 20 } } })
        lane('beam-xsb2301', 'xsb2301', { targets = { { 'ueb1201', 30 } } })
        lane('beam-dslk004', 'dslk004', { targets = { { 'ueb1201', 15 } } })
    end)
    WaitTicks(30)
    P('done')
end)
