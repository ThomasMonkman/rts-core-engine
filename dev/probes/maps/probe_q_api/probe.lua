-- Differential probe (open questions): DamageRing inner edge (ring.*),
-- OnTerrainTypeChange at creation (terr.*), alliance name case (ally.*).
--   real game: run_faf.ps1 probe_q_api 90
--   ours: --headless --no-ai --ticks 80 --script dev/probes/maps/probe_q_api/probe.lua
local fmt = string.format
local function P(s) LOG('PROBE ' .. s) end
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy')
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local function tname(t)
    if type(t) ~= 'table' then return tostring(t) end
    return tostring(t.Name or t.Description or t.TypeCode or '?')
end

-- [terr] class hooks before the units exist: order of creation callbacks.
local t0 = GetGameTick()
local function hookClass(path, tag)
    local ok, mod = pcall(import, path)
    local cls = ok and mod and mod.TypeClass
    if not cls then P('terr hook failed ' .. path) return end
    for _, name in { 'OnPreCreate', 'OnCreate', 'OnStopBeingBuilt', 'OnLayerChange', 'OnTerrainTypeChange', 'OnMotionHorzEventChange' } do
        local orig = cls[name]
        local n = name
        cls[n] = function(self, a, b, c)
            local extra = ''
            if n == 'OnTerrainTypeChange' then extra = ' new ' .. tname(a) .. ' old ' .. tname(b)
            elseif n == 'OnLayerChange' or n == 'OnMotionHorzEventChange' then extra = ' ' .. tostring(a) .. ' ' .. tostring(b) end
            P(fmt('terr %s tick+%d %s%s TerrainType %s', tag, GetGameTick() - t0, n, extra, tname(self.TerrainType)))
            if orig then return orig(self, a, b, c) end
        end
    end
end
hookClass('/units/uel0201/uel0201_script.lua', 'tank')
hookClass('/units/ueb1101/ueb1101_script.lua', 'pgen')
P('terr default ' .. tname(GetTerrainType(-1, -1)) .. ' at-site ' .. tname(GetTerrainType(720, 64)))
P('terr make tank')
local tank = make('uel0201', A1, 720, 64)
P('terr made tank')
P('terr make pgen')
local pgen = make('ueb1101', A1, 592, 80)
P('terr made pgen')

-- [ally] alliance strings, case variants
do
    local i1, i2 = 1, 2
    local function st()
        return fmt('E%s A%s N%s', tostring(IsEnemy(i1, i2)), tostring(IsAlly(i1, i2)), tostring(IsNeutral(i1, i2)))
    end
    for _, s in { 'Enemy', 'enemy', 'ENEMY', 'eNeMy', 'Ally', 'ally', 'ALLY', 'Neutral', 'neutral', 'NEUTRAL', 'bogus' } do
        local base = (string.lower(s) == 'neutral') and 'Enemy' or 'Neutral'
        SetAlliance(A1, A2, base)
        local ok, err = pcall(SetAlliance, A1, A2, s)
        P(fmt('ally SetAlliance %s from %s: ok %s -> %s%s', s, base, tostring(ok), st(), ok and '' or (' err ' .. tostring(err))))
    end
    for _, s in { 'enemy', 'ally' } do
        SetAlliance(A1, A2, 'Neutral') SetAlliance(A2, A1, 'Neutral')
        local ok, err = pcall(SetAllianceOneWay, A1, A2, s)
        P(fmt('ally SetAllianceOneWay %s: ok %s -> %s back E%s A%s', s, tostring(ok), st(), tostring(IsEnemy(i2, i1)), tostring(IsAlly(i2, i1))))
    end
    SetAlliance(A1, A2, 'Enemy')
    local brain = ArmyBrains[1]
    local foe = make('ueb5101', A2, 760, 64)
    local mine = make('ueb5101', A1, 762, 64)
    for _, s in { 'Enemy', 'enemy', 'ENEMY', 'Ally', 'ally' } do
        local ok, list = pcall(function() return brain:GetUnitsAroundPoint(categories.ueb5101, { 761, GetTerrainHeight(761, 64), 64 }, 5, s) end)
        P(fmt('ally GetUnitsAroundPoint %s ok %s n %s', s, tostring(ok), ok and tostring(table.getn(list or {})) or tostring(list)))
    end
    foe:Destroy() mine:Destroy()
end

-- [ring] DamageRing(inst, pos, 3, 6, ...) against a wall (half size 0.5) and a
-- tank (SizeX 0.7) at fine distances around the inner and outer edges, along x.
do
    local function ringAt(tag, bp, x, z, list, dz)
        local u = make(bp, A2, x, z)
        u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
        local hits = 0
        u.OnDamage = function(self, inst, amt, vec, dt) hits = hits + 1 end
        local p = u:GetPosition()
        local line = 'ring ' .. tag
        for _, d in list do
            local h0 = hits
            if dz then
                DamageRing(nil, { p[1], p[2], p[3] + d }, 3, 6, 1, 'Normal', true, true)
            else
                DamageRing(nil, { p[1] + d, p[2], p[3] }, 3, 6, 1, 'Normal', true, true)
            end
            line = line .. fmt(' %.3f:%s', d, hits > h0 and 'H' or '-')
        end
        P(line)
        u:Destroy()
    end
    ringAt('wall-x', 'ueb5101', 648, 112, { 3.375, 3.45, 3.49, 3.495, 3.499, 3.5, 3.501, 3.505, 3.51, 3.55, 3.625, 6.375, 6.45, 6.49, 6.499, 6.5, 6.501, 6.51 })
    ringAt('tank-x', 'uel0201', 776, 136, { 3.3, 3.34, 3.349, 3.35, 3.351, 3.36, 3.375, 3.4, 6.3, 6.34, 6.349, 6.35, 6.351, 6.36, 6.4 })
    ringAt('wall-z', 'ueb5101', 936, 208, { 3.49, 3.499, 3.5, 3.501, 3.51, 6.49, 6.5, 6.51 }, true)
end

ForkThread(function()
    WaitTicks(2)
    P('terr move tank')
    IssueMove({ tank }, { 720, GetTerrainHeight(720, 104), 104 })
    WaitTicks(60)
    P(fmt('terr end tank TerrainType %s pgen TerrainType %s', tname(tank.TerrainType), tname(pgen.TerrainType)))
    P('done')
end)
