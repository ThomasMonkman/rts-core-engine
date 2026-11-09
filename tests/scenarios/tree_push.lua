-- Scenario test (tests/scenarios.rs): trees hit by area damage fall away
-- from its centre (#16). FAF's `Tree:OnDamage` whacks the tree's fall
-- motor (`FallDown():Whack(dir, 0.5, true)`) with the damage direction;
-- moho's motor (`sim::fall_down`) leans the tree that way: 0.5 rad after a
-- tick, flat after three. Then a commander warp-in, whose effect projectile
-- knocks trees at 11..22 over with `TreeForce` rings. FAF probe
-- `probe_treepush` gave the same orientations.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local TREE = '/env/evergreen/props/trees/brch01_s1_prop.bp'

local function ring(x0, z0, radii)
    local trees = {}
    for _, r in radii do
        for i = 0, 7 do
            local a = (i + 0.25) * 2 * math.pi / 8
            local x, z = x0 + r * math.sin(a), z0 + r * math.cos(a)
            table.insert(trees, { t = CreateProp({ x, GetTerrainHeight(x, z), z }, TREE), x = x - x0, z = z - z0, name = r .. ':' .. i })
        end
    end
    return trees
end

-- The tree's up axis.
local function up(t)
    local q = t:GetOrientation()
    local x, y, z, w = q[1], q[2], q[3], q[4]
    return 2 * (x * y - w * z), 1 - 2 * (x * x + z * z), 2 * (y * z + w * x)
end

-- Every tree leans `tilt` from upright, straight away from the centre.
local function leaning(trees, tilt, label)
    for _, e in trees do
        if e.t:BeenDestroyed() then
            check(false, label .. ' ' .. e.name .. ': gone')
        else
            local ux, uy, uz = up(e.t)
            local d = math.sqrt(e.x * e.x + e.z * e.z)
            local h = math.sqrt(ux * ux + uz * uz)
            local away = (ux * e.x + uz * e.z) / (d * math.max(h, 1e-6))
            check(math.abs(uy - math.cos(tilt)) < 0.01, string.format('%s %s: up.y %.3f, want %.3f', label, e.name, uy, math.cos(tilt)))
            check(away > 0.999, string.format('%s %s: leans %.3f away from the centre', label, e.name, away))
        end
    end
end

local ax, az = 400, 600
local inst = CreateUnitHPR('uel0105', 'ARMY_1', 440, GetTerrainHeight(440, 640), 640, 0, 0, 0)
local a = ring(ax, az, { 3, 6 })

ForkThread(function()
    WaitTicks(2)
    DamageArea(inst, { ax, GetTerrainHeight(ax, az), az }, 9, 1, 'TreeForce', true)
    WaitTicks(1)
    leaning(a, 0.5, 'area +1')
    WaitTicks(1)
    WaitTicks(1)
    leaning(a, math.pi / 2, 'area +3')

    local bx, bz = 420, 610
    local b = ring(bx, bz, { 14, 19 })
    local acu = CreateUnitHPR('uel0001', 'ARMY_1', bx, GetTerrainHeight(bx, bz), bz, 0, 0, 0)
    acu:PlayCommanderWarpInEffect()
    WaitTicks(60)
    leaning(b, math.pi / 2, 'warp-in')
    ScenarioResult.done = true
end)
