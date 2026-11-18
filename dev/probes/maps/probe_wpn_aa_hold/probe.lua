-- Differential probe: the anti-air batch of probe_wpn_census against a
-- gunship held hovering on its spot: a move order to the spot is re-issued
-- every 10 ticks so it never idles and lands (copy of probe_wpn_aa). Per weapon: when it got a target,
-- fired, how many shots, flight times and impact types; per target: hits,
-- damage, damage types; and the target's height above the ground.
--   real game (Windows): run_faf.ps1 probe_wpn_aa_hold 120
--   ours: --headless --no-ai --ticks 400 --script dev/probes/maps/probe_wpn_aa_hold/probe.lua
-- Lines: PROBE cen <unit> ... (ticks are counted from the attack order).
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
local WATER = {
    {48,48}, {144,48}, {240,48}, {336,80}, {432,112}, {48,144}, {144,144}, {240,144}, {336,176}, {432,208}, {176,240}, {272,272},
    {368,304}, {464,304}, {176,336}, {272,368}, {368,400}, {224,464}, {736,496}, {656,592}, {752,592}, {848,656}, {560,672}, {656,688},
    {752,688}, {848,752}, {544,768}, {640,784}, {736,784}, {832,848}, {640,880}, {736,880}, {928,880}, {832,944}, {736,976}, {928,976},
}
local RUN = 200
local BATCHES = {
    { 'uel0104:air', 'ual0104:air', 'url0104:air', 'xsl0104:air', 'uel0205:air', 'ual0205:air', 'url0205:air', 'xsl0205:air', 'ueb2104:air', 'uab2104:air', 'urb2104:air', 'xsb2104:air',
      'ueb2204:air', 'uab2204:air', 'urb2204:air', 'xsb2204:air', 'ueb2304:air', 'uab2304:air', 'urb2304:air', 'xsb2304:air', 'drlk001:air', 'delk002:air', 'dalk003:air', 'uea0102:air' },
}
local NAVAL = { 'ues0103', 'uas0103', 'urs0103', 'xss0103', 'ues0201', 'uas0201', 'urs0201', 'xss0201', 'ues0202', 'uas0202', 'urs0202', 'xss0202',
    'ues0302', 'uas0302', 'urs0302', 'xss0302', 'ues0203', 'uas0203', 'urs0203', 'xss0203', 'ueb2109', 'uab2109', 'urb2109', 'xsb2109',
    'xes0102', 'xrs0204', 'xes0307', 'xas0204', 'xss0304', 'uas0304:landfar', 'ues0304:landfar', 'urs0304:landfar', 'xsb2205', 'uab2205', 'ueb2205', 'urb2205' }

local function list(t, n)
    local out = ''
    for i = 1, math.min(table.getn(t), n) do out = out .. (i > 1 and ',' or '') .. t[i] end
    return out == '' and '-' or out
end

local function lane(spec, site, water)
    local bp, mode = spec, nil
    local c = string.find(spec, ':')
    if c then bp, mode = string.sub(spec, 1, c - 1), string.sub(spec, c + 1) end
    local x, z = site[1], site[2]
    local y = water and GetSurfaceHeight(x, z) or GetTerrainHeight(x, z)
    local ok, u = pcall(CreateUnitHPR, bp, A1, x, y, z, 0, 0, 0)
    if not ok or not u then P(fmt('cen %s nocreate', bp)) return nil end
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    -- hold fire until the attack order; sight of the target at any range
    u:SetFireState(1)
    u:SetIntelRadius('Vision', 70)
    local l = { bp = bp, u = u, weps = {}, hits = 0, sum = 0, types = {}, typeList = {}, amounts = {} }
    -- the range to stand the target at
    local reach, minr = 0, 0
    for i = 1, u:GetWeaponCount() do
        local wb = u:GetWeapon(i):GetBlueprint()
        local wantAir = mode == 'air'
        local isAA = wb.RangeCategory == 'UWRC_AntiAir'
        if (wb.Damage or 0) > 0 and wb.RangeCategory ~= 'UWRC_Countermeasure' and (wantAir == isAA) and (wb.MaxRadius or 0) > reach then
            reach = wb.MaxRadius
            minr = wb.MinRadius or 0
        end
    end
    local dist = math.min(38, reach * 0.6)
    if dist < minr + 3 then dist = minr + 3 end
    if reach == 0 then dist = 10 end
    local t
    if mode == 'air' then
        t = CreateUnitHPR('uea0203', A2, x, GetTerrainHeight(x, z + dist), z + dist, 0, 0, 0)
        l.tx = x
        l.spot = { x, GetTerrainHeight(x, z + dist), z + dist }
        IssueMove({ t }, l.spot)
    elseif water then
        t = CreateUnitHPR('ues0103', A2, x, GetSurfaceHeight(x, z + dist), z + dist, 0, 0, 0)
    else
        t = CreateUnitHPR('ueb1201', A2, x, GetTerrainHeight(x, z + dist), z + dist, 0, 0, 0)
    end
    if t.SetFireState then t:SetFireState(1) end
    t:SetMaxHealth(1000000) t:SetHealth(nil, 1000000)
    l.air = {}
    t.OnDamage = function(self, inst, amt, vec, dt)
        if not l.T0 then return end
        l.hits = l.hits + 1
        l.sum = l.sum + amt
        if not l.first then l.first = GetGameTick() - l.T0 end
        l.last = GetGameTick() - l.T0
        if table.getn(l.amounts) < 4 then table.insert(l.amounts, fmt('%.1f', amt)) end
        local k = tostring(dt)
        if not l.types[k] then l.types[k] = true table.insert(l.typeList, k) end
    end
    l.t = t
    l.dist = dist
    for i = 1, u:GetWeaponCount() do
        local wep = u:GetWeapon(i)
        local wb = wep:GetBlueprint()
        local w = { label = wb.Label or ('w' .. i), fires = {}, nfire = 0, shots = {}, nshot = 0, imps = {}, impc = {}, impk = {}, got = nil, ngot = 0 }
        table.insert(l.weps, w)
        -- FAF's weapons are state machines: the hook steps aside while the
        -- current state's own method runs, so it doesn't shadow it.
        local function hook(name, note)
            local mine
            mine = function(self)
                note()
                self[name] = nil
                local r = self[name](self)
                self[name] = mine
                return r
            end
            wep[name] = mine
        end
        hook('OnFire', function()
            if l.T0 then
                w.nfire = w.nfire + 1
                if w.nfire <= 4 then table.insert(w.fires, GetGameTick() - l.T0) end
            end
        end)
        hook('OnGotTarget', function()
            if l.T0 then
                w.ngot = w.ngot + 1
                if not w.got then w.got = GetGameTick() - l.T0 end
            end
        end)
        local create = wep.CreateProjectileAtMuzzle
        if create then
            wep.CreateProjectileAtMuzzle = function(self, muzzle)
                local p = create(self, muzzle)
                if l.T0 then
                    w.nshot = w.nshot + 1
                    if w.nshot <= 8 then table.insert(w.shots, GetGameTick() - l.T0) end
                    if type(p) == 'table' and p.OnImpact then
                        local born = GetGameTick()
                        local oi = p.OnImpact
                        local n = w.nshot
                        p.OnImpact = function(pp, tt, te)
                            if not pp.CenSeen then
                                pp.CenSeen = true
                                local key = tostring(tt) .. (te == l.t and '*' or '')
                                if not w.impc[key] then w.impc[key] = 0 table.insert(w.impk, key) end
                                w.impc[key] = w.impc[key] + 1
                            end
                            if n <= 3 and not pp.CenSeen3 then
                                pp.CenSeen3 = true
                                table.insert(w.imps, fmt('%d:%s%s', GetGameTick() - born, tostring(tt), te == l.t and '*' or ''))
                            end
                            return oi(pp, tt, te)
                        end
                    end
                end
                return p
            end
        end
    end
    return l
end

local function tgt(l)
    if l.t:BeenDestroyed() then return 'gone' end
    local x, y, z = l.t:GetPositionXYZ()
    local vx, vy, vz = l.t:GetVelocity()
    return fmt('%.1f/%.1f/%.1f', y - GetTerrainHeight(x, z), x - l.tx, math.sqrt(vx * vx + vz * vz) * 10)
end
local function report(l)
    for i, w in l.weps do
        if w.ngot > 0 or w.nfire > 0 or w.nshot > 0 then
            P(fmt('cen %s w%d %s got %s x%d fire %s x%d shots %s x%d imp %s', l.bp, i, w.label, tostring(w.got), w.ngot, list(w.fires, 4), w.nfire,
                list(w.shots, 8), w.nshot, list(w.imps, 3)))
            local c = {}
            for _, k in w.impk do table.insert(c, k .. '=' .. w.impc[k]) end
            P(fmt('cen %s w%d impacts %s', l.bp, i, table.concat(c, ' ')))
        else
            P(fmt('cen %s w%d %s idle', l.bp, i, w.label))
        end
    end
    local ux, uy, uz = l.x0, 0, l.z0
    if not l.u:BeenDestroyed() then ux, uy, uz = l.u:GetPositionXYZ() end
    P(fmt('cen %s target alt %s', l.bp, table.concat(l.air, ',')))
    P(fmt('cen %s target dist %.1f hits %d dmg %.0f first %s last %s amts %s types %s moved %.1f', l.bp, l.dist, l.hits, l.sum, tostring(l.first), tostring(l.last),
        list(l.amounts, 4), list(l.typeList, 4), math.sqrt((ux - l.x0) * (ux - l.x0) + (uz - l.z0) * (uz - l.z0))))
end

ForkThread(function()
    WaitTicks(3)
    P('start')
    for b, batch in BATCHES do
        local brain = GetArmyBrain(A1)
        brain:GiveStorage('ENERGY', 1000000) brain:GiveStorage('MASS', 100000)
        brain:GiveResource('ENERGY', 1000000) brain:GiveResource('MASS', 100000)
        local lanes = {}
        for i, spec in batch do
            local l = lane(spec, LAND[i], false)
            if l then table.insert(lanes, l) end
        end
        if false then
            for i, spec in NAVAL do
                local l = lane(spec, WATER[i], true)
                if l then table.insert(lanes, l) end
            end
        end
        for k = 1, 3 do
            WaitTicks(8)
            for _, l in lanes do
                if l.spot and not l.t:BeenDestroyed() then IssueClearCommands({ l.t }) IssueMove({ l.t }, l.spot) end
            end
        end
        WaitTicks(1)
        for _, l in lanes do
            l.T0 = GetGameTick()
            local ux, uy, uz = l.u:GetPositionXYZ()
            l.x0, l.z0 = ux, uz
            l.u:SetFireState(0)
            IssueAttack({ l.u }, l.t)
        end
        P(fmt('batch %d attack tick %d', b, GetGameTick()))
        for k = 0, RUN - 1, 10 do
            for _, l in lanes do
                if l.spot and not l.t:BeenDestroyed() then IssueClearCommands({ l.t }) IssueMove({ l.t }, l.spot) end
            end
            for _, l in lanes do if k < 60 or math.mod(k, 50) == 0 then table.insert(l.air, tgt(l)) end end
            WaitTicks(10)
        end
        for _, l in lanes do report(l) end
        wipe()
        WaitTicks(8)
    end
    WaitTicks(30)
    P('done')
end)
