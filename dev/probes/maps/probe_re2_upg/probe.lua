-- Differential probe (sweep 2 leftovers): upgrade start and spend, with resources given after storage applies.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re2_upg 90
--   ours: --headless --no-ai --ticks 400 --script dev/probes/maps/probe_re2_upg/probe.lua
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
    for k = 0, 3 do make('ueb1106', i, SITES[i][1] - 20 + 3 * k, SITES[i][2] - 15) end
    b:GiveStorage('ENERGY', 100000)
end
ForkThread(function()
    for t = 1, 12 do
        WaitTicks(1)
        if t == 5 then
            for i = 1, 3 do ArmyBrains[i]:GiveResource('MASS', 10000) ArmyBrains[i]:GiveResource('ENERGY', 100000) end
        end
        P('ECON', fmt('A1 mass %.2f energy %.2f max %s', ArmyBrains[1]:GetEconomyStored('MASS'), ArmyBrains[1]:GetEconomyStored('ENERGY'), tostring(ArmyBrains[1]:GetArmyStat('Economy_MaxStorage_Mass', 0).Value)))
    end
end)
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

ForkThread(function()
    WaitTicks(330)
    WaitTicks(30)
    LOG('PROBE done')
end)
