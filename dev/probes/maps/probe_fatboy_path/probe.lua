-- Differential probe: the path a Fatboy (uel0401) takes from the shore into
-- the water and back, where the shore is steep (z 620: level until x 432,
-- 2 down by x 434, 5 down by x 442).
--   real game (Windows): dev/probes/run_faf.ps1 probe_fatboy_path 420
--   ours: cargo run -- --headless --no-ai --ticks 3600 --script dev/probes/maps/probe_fatboy_path/probe.lua
-- [A] from 424,620 to 462,620: straight over the edge, or round it?
-- [B] back to 424,620.
-- [C] a new Fatboy made at 430,620 (its footprint over the edge), to 444,620.
-- [T] a tank (uel0201) from 424,620 to 440,620, for the same edge.
-- A line every 10 ticks: position, layer, and the depth under it.
local fmt = string.format
local armies = ListArmies()
local A1 = armies[1]
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local part, T = 'pre', GetGameTick()
local function P(s) LOG(fmt('PROBE %s T+%d %s', part, GetGameTick() - T, s)) end
local function make(bp, x, z)
    local u = CreateUnitHPR(bp, A1, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetCanTakeDamage(false)
    return u
end
local function walk(name, u, to, limit)
    part, T = name, GetGameTick()
    IssueClearCommands({ u })
    IssueMove({ u }, { to[1], GetTerrainHeight(to[1], to[2]), to[2] })
    local lx, lz, still = nil, nil, 0
    for t = 0, limit do
        local x, y, z = u:GetPositionXYZ()
        if math.mod(t, 10) == 0 then
            P(fmt('at %.1f,%.1f,%.1f %s depth %.1f%s', x, y, z, u:GetCurrentLayer(), GetSurfaceHeight(x, z) - GetTerrainHeight(x, z), u:IsUnitState('Moving') and '' or ' stopped'))
        end
        if t > 20 and lx and VDist2(x, z, lx, lz) < 0.01 then still = still + 1 else still = 0 end
        lx, lz = x, z
        if still >= 20 then break end
        WaitTicks(1)
    end
    local x, y, z = u:GetPositionXYZ()
    P(fmt('end at %.1f,%.1f,%.1f %s, %.1f from the goal', x, y, z, u:GetCurrentLayer(), VDist2(x, z, to[1], to[2])))
end

ForkThread(function()
    WaitTicks(5)
    local line = 'depth z=620:'
    for x = 426, 446, 1 do line = line .. fmt(' %d:%.1f', x, GetSurfaceHeight(x, 620) - GetTerrainHeight(x, 620)) end
    P(line)
    local fatboy = make('uel0401', 424, 620)
    WaitTicks(30)
    walk('A', fatboy, { 462, 620 }, 1200)
    walk('B', fatboy, { 424, 620 }, 1200)
    fatboy:Destroy()
    WaitTicks(10)
    fatboy = make('uel0401', 430, 620)
    WaitTicks(30)
    walk('C', fatboy, { 444, 620 }, 600)
    fatboy:Destroy()
    WaitTicks(10)
    local tank = make('uel0201', 424, 620)
    WaitTicks(10)
    walk('T', tank, { 440, 620 }, 400)
    part = 'done'
    P('end')
end)
