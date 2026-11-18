-- Differential probe: where the Yolona Oss nuke's damage lands. FAF passes the
-- missile's GetPosition() table to the nuke rings (Projectile.lua DoDamage ->
-- NukeDamage.lua SlowNuke, a forked thread), then the Yolona script's OnImpact
-- takes GetPosition() again and adds 20 to [2] to warp its effect entity.
-- With one shared table, the rings' centre is then 20 up, unless the forked
-- thread already ran (part S checks when a ForkThread first runs).
local fmt = string.format
local function P(s) LOG('PROBE ' .. s) end
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local function v(p) return p and fmt('%.2f,%.2f,%.2f', p[1], p[2], p[3]) or 'nil' end
local oldArea, oldRing = DamageArea, DamageRing
rawset(_G, 'DamageArea', function(inst, pos, r, dmg, typ, a, b)
    if dmg and dmg > 100 then P(fmt('t%d DamageArea pos %s r %.2f dmg %d', GetGameTick(), v(pos), r, dmg)) end
    return oldArea(inst, pos, r, dmg, typ, a, b)
end)
rawset(_G, 'DamageRing', function(inst, pos, r1, r2, dmg, typ, a, b)
    P(fmt('t%d DamageRing pos %s r %.2f-%.2f dmg %d', GetGameTick(), v(pos), r1, r2, dmg))
    return oldRing(inst, pos, r1, r2, dmg, typ, a, b)
end)
ForkThread(function()
    WaitTicks(5)
    P('start')
    -- [S] does a forked thread run before ForkThread returns?
    local box = { 1 }
    ForkThread(function() P(fmt('S fork first ran t%d sees %d', GetGameTick(), box[1])) end)
    box[1] = 2
    P(fmt('S after ForkThread t%d', GetGameTick()))
    WaitTicks(3)
    -- [N] the Yolona at a line of power generators, 0..40 from the aim point
    local x0, z0 = 400.5, 600.5
    P(fmt('N ground %.2f', GetTerrainHeight(x0, z0)))
    local yol = make('xsb2401', A1, x0 - 300, z0)
    local targets = {}
    for i, d in {0, 10, 20, 30, 40} do
        local u = make('xsb1101', A2, x0 + d, z0)
        u:SetCanBeKilled(false)
        targets[i] = { d = d, u = u }
    end
    yol:GiveNukeSiloAmmo(1)
    WaitTicks(5)
    IssueNuke({ yol }, { x0, GetTerrainHeight(x0, z0), z0 })
    P(fmt('N fired t%d', GetGameTick()))
    local last = {}
    for k = 1, 400 do
        WaitTicks(2)
        for i, t in targets do
            local hp = t.u:BeenDestroyed() and -1 or t.u:GetHealth()
            if hp ~= last[i] then
                P(fmt('t%d N target d%d hp %.0f', GetGameTick(), t.d, hp))
                last[i] = hp
            end
        end
    end
    WaitTicks(30)
    P('done')
end)
