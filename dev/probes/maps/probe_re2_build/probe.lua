-- Differential probe (sweep 2 leftovers): construction and queries.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re2_build 90
--   ours: --headless --no-ai --ticks 400 --script dev/probes/maps/probe_re2_build/probe.lua
-- UPG   an upgrade's new structure: fraction, hp and the army's spend per tick
--       (one upgrade per army: radar at rate 100 (A1), radar default (A2),
--       land factory default (A3)).
-- NEAR  brain:CreateUnitNearSpot called repeatedly on one spot.
-- RECT  GetUnitsInRect / GetReclaimablesInRect with thin strips swept past
--       one object: which offsets still return it.
-- BS    brain:BuildStructure on free / blocked sites: return value, the
--       queued site, where it is built.
-- Lines: PROBE <tick> <tag> <text>.
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local t0 = GetGameTick()
local function P(tag, text) LOG(fmt('PROBE %d %s %s', GetGameTick() - t0, tag, text)) end
local function make(bp, army, x, z)
    return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local SITES = { { 608, 80 }, { 688, 80 }, { 912, 96 }, { 784, 144 }, { 656, 160 }, { 736, 224 }, { 816, 224 }, { 912, 240 }, { 672, 304 }, { 800, 336 },
    { 960, 336 }, { 656, 384 }, { 880, 400 }, { 576, 464 }, { 384, 560 }, { 96, 608 }, { 352, 640 }, { 64, 688 }, { 176, 688 }, { 320, 720 },
    { 224, 768 }, { 112, 784 }, { 304, 800 }, { 240, 880 }, { 352, 880 }, { 96, 896 }, { 432, 912 } }
for i = 1, 3 do
    local b = ArmyBrains[i]
    b:GiveStorage('MASS', 10000) b:GiveResource('MASS', 10000)
    b:GiveStorage('ENERGY', 100000) b:GiveResource('ENERGY', 100000)
end
local brain = ArmyBrains[1]

-- UPG
local function upgrade(name, army, site, bp, to, rate)
    ForkThread(function()
        WaitTicks(15)
        local b = ArmyBrains[army]
        local x, z = SITES[site][1], SITES[site][2]
        local r = make(bp, army, x, z)
        if rate then r:SetBuildRate(rate) end
        WaitTicks(2)
        local m0, e0 = b:GetEconomyStored('MASS'), b:GetEconomyStored('ENERGY')
        local new
        local o = r.OnStartBuild
        r.OnStartBuild = function(self, unit, order)
            new = unit
            P('UPG ' .. name, 'OnStartBuild ' .. tostring(order) .. fmt(' frac %.4f', unit:GetFractionComplete()))
            if o then return o(self, unit, order) end
        end
        local bpn = __blueprints[to]
        P('UPG ' .. name, fmt('start stored %.2f %.2f cost %s %s time %s rate %s', m0, e0, tostring(bpn.Economy.BuildCostMass), tostring(bpn.Economy.BuildCostEnergy),
            tostring(bpn.Economy.BuildTime), tostring(r:GetBuildRate())))
        IssueUpgrade({ r }, to)
        local done
        for i = 0, 400 do
            local m, e = b:GetEconomyStored('MASS'), b:GetEconomyStored('ENERGY')
            local f = (new and not new:BeenDestroyed()) and new:GetFractionComplete() or -1
            local h = (new and not new:BeenDestroyed()) and new:GetHealth() or -1
            local wp = r:BeenDestroyed() and -1 or (r:GetWorkProgress() or -1)
            if i <= 25 or math.mod(i, 10) == 0 or (f >= 1 and not done) then
                P('UPG ' .. name, fmt('+%d frac %.4f hp %.2f spent %.2f %.2f work %.4f old %s', i, f, h, m0 - m, e0 - e, wp, r:BeenDestroyed() and 'gone' or 'alive'))
            end
            if f >= 1 and not done then done = i end
            if done and i > done + 3 then break end
            WaitTicks(1)
        end
    end)
end
upgrade('radar100', 1, 1, 'ueb3101', 'ueb3201', 100)
upgrade('radar', 2, 2, 'ueb3101', 'ueb3201', nil)
upgrade('factory', 3, 3, 'ueb0101', 'ueb0201', nil)

-- NEAR
ForkThread(function()
    WaitTicks(15)
    local function series(label, site, bp, n, ox, oz)
        local x, z = SITES[site][1], SITES[site][2]
        local out = {}
        for i = 1, n do
            local ok, u = pcall(function() return brain:CreateUnitNearSpot(bp, x + (ox or 0), z + (oz or 0)) end)
            if ok and u then
                local p = u:GetPosition()
                table.insert(out, fmt('%.1f,%.1f', p[1] - x, p[3] - z))
            else
                table.insert(out, 'fail')
            end
        end
        P('NEAR ' .. label, bp .. ' ' .. table.concat(out, ' '))
    end
    series('pgen', 4, 'ueb1101', 14)
    series('wall', 5, 'ueb5101', 12)
    series('factory', 6, 'ueb0101', 7)
    series('tank', 7, 'uel0201', 8)
    series('pgen-off', 8, 'ueb1101', 6, 0.3, 0.7)
end)

-- RECT
ForkThread(function()
    WaitTicks(15)
    local function count(t)
        if t == nil then return 'n' end
        return tostring(table.getn(t))
    end
    local function sweep(label, f, x, z)
        local xs, zs = {}, {}
        for o = -12, 12, 0.5 do
            local ok, a = pcall(f, Rect(x + o, z - 1, x + o + 0.1, z + 1))
            table.insert(xs, ok and count(a) or 'E')
            local ok2, b = pcall(f, Rect(x - 1, z + o, x + 1, z + o + 0.1))
            table.insert(zs, ok2 and count(b) or 'E')
        end
        P('RECT ' .. label, 'x -12..12 by 0.5: ' .. table.concat(xs, ''))
        P('RECT ' .. label, 'z -12..12 by 0.5: ' .. table.concat(zs, ''))
    end
    local function obj(label, site, dx, dz, mk, f)
        local x, z = SITES[site][1] + dx, SITES[site][2] + dz
        local u = mk(x, z)
        local p = u:GetPosition()
        P('RECT ' .. label, fmt('object at %.2f %.2f', p[1], p[3]))
        return { label = label, f = f, x = x, z = z }
    end
    local list = {
        obj('tank-int', 9, 0, 0, function(x, z) return make('uel0201', 1, x, z) end, GetUnitsInRect),
        obj('tank-half', 10, 0.5, 0.5, function(x, z) return make('uel0201', 1, x, z) end, GetUnitsInRect),
        obj('tank-3.3', 11, 3.3, 1.7, function(x, z) return make('uel0201', 1, x, z) end, GetUnitsInRect),
        obj('t3pgen', 12, 0, 0, function(x, z) return make('ueb1301', 1, x, z) end, GetUnitsInRect),
        obj('wall', 13, 0, 0, function(x, z) return make('ueb5101', 1, x, z) end, GetUnitsInRect),
        obj('prop', 14, 0.5, 0.5, function(x, z)
            local p = CreateProp(pos(x, z), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
            p:SetMaxReclaimValues(30, 40, 0)
            return p
        end, GetReclaimablesInRect),
        obj('prop-int', 14, 6.0, 9.0, function(x, z)
            local p = CreateProp(pos(x, z), '/env/evergreen/props/rocks/fieldstone01_prop.bp')
            p:SetMaxReclaimValues(30, 40, 0)
            return p
        end, GetReclaimablesInRect),
    }
    WaitTicks(5)
    for _, o in list do sweep(o.label, o.f, o.x, o.z) end
end)

-- BS
ForkThread(function()
    WaitTicks(15)
    local cases = {}
    local function bs(name, site, bp, loc, setup)
        local x, z = SITES[site][1], SITES[site][2]
        if setup then setup(x, z) end
        local e = make('uel0105', 1, x, z + 14)
        local l = loc(x, z)
        local ok, r = pcall(function() return brain:BuildStructure(e, bp, l, false) end)
        local q = e:GetCommandQueue() or {}
        local c = q[1]
        P('BS ' .. name, fmt('asked %.2f %.2f -> %s %s q %d %s', l[1] - x, l[2] - z, tostring(ok), tostring(r), table.getn(q),
            c and c.x and fmt('at %.2f %.2f', c.x - x, c.z - z) or '-'))
        table.insert(cases, { name = name, e = e, x = x, z = z })
    end
    local function blocker(bp, dx, dz) return function(x, z) make(bp, 1, x + dx, z + dz) end end
    bs('free-int', 15, 'ueb1101', function(x, z) return { x, z, 0 } end)
    bs('free-half', 16, 'ueb1101', function(x, z) return { x + 0.5, z + 0.5, 0 } end)
    bs('free-frac', 17, 'ueb1101', function(x, z) return { x + 0.3, z + 0.7, 0 } end)
    bs('free-third1', 18, 'ueb1101', function(x, z) return { x, z, 1 } end)
    bs('on-pgen', 19, 'ueb1101', function(x, z) return { x, z, 0 } end, blocker('ueb1101', 0, 0))
    bs('pgen+1', 20, 'ueb1101', function(x, z) return { x + 1, z + 1, 0 } end, blocker('ueb1101', 0, 0))
    bs('on-factory', 21, 'ueb1101', function(x, z) return { x, z, 0 } end, blocker('ueb0101', 0, 0))
    bs('on-tank', 22, 'ueb1101', function(x, z) return { x, z, 0 } end, blocker('uel0201', 0, 0))
    bs('nw-blocked', 23, 'ueb1101', function(x, z) return { x, z, 0 } end, function(x, z)
        for _, d in { { 0, 0 }, { -8, -8 }, { -8, 0 }, { 0, -8 }, { -4, -4 } } do make('ueb1101', 1, x + d[1], z + d[2]) end
    end)
    bs('wall-on-pgen', 24, 'ueb5101', function(x, z) return { x, z, 0 } end, blocker('ueb1101', 0, 0))
    bs('fac-on-pgen', 25, 'ueb0101', function(x, z) return { x, z, 0 } end, blocker('ueb1101', 0, 0))
    bs('free-int2', 26, 'ueb1101', function(x, z) return { x + 3, z - 5, 0 } end)
    bs('on-pgen-off', 27, 'ueb1101', function(x, z) return { x + 7, z + 3, 0 } end, blocker('ueb1101', 7, 3))
    WaitTicks(1)
    for _, c in cases do
        local q = c.e:GetCommandQueue() or {}
        local q1 = q[1]
        P('BS ' .. c.name, fmt('+1 q %d %s', table.getn(q), q1 and q1.x and fmt('at %.2f %.2f', q1.x - c.x, q1.z - c.z) or '-'))
    end
    WaitTicks(80)
    for _, c in cases do
        local f = c.e:GetFocusUnit()
        local q = c.e:GetCommandQueue() or {}
        local fp = f and f:GetPosition()
        P('BS ' .. c.name, fmt('+81 q %d building %s', table.getn(q), fp and fmt('%s at %.2f %.2f', f:GetBlueprint().BlueprintId, fp[1] - c.x, fp[3] - c.z) or '-'))
    end
end)

ForkThread(function()
    WaitTicks(330)
    WaitTicks(30)
    LOG('PROBE done')
end)
