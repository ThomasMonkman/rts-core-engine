-- Differential probe (sweep 3): PickBestAttackVector against squad position, land vs air squad.
--   real game: dev/probes/run_faf.ps1 probe_re3_ai2 80 ; ours: --headless --no-ai --ticks 200 --script ...
local fmt = string.format
local armies = ListArmies()
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function A() return GetGameTick() end
local function P(text) LOG(fmt('PROBE %d %s', A(), text)) end
local CIV
for i, a in armies do if a == 'NEUTRAL_CIVILIAN' then CIV = i end end
SetAlliance(1, CIV, 'Enemy') SetAlliance(CIV, 1, 'Enemy')
local brain = ArmyBrains[1]
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    pcall(function() u:SetFireState(1) end)
    return u
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local function S(v)
    if type(v) ~= 'table' then return tostring(v) end
    return fmt('(%s,%s %s,%s)', tostring(v.px), tostring(v.pz), tostring(v.vx), tostring(v.vz))
end
ForkThread(function()
    while A() < 15 do WaitTicks(1) end
    local C = categories
    brain:SetCurrentEnemy(ArmyBrains[CIV])
    for _, d in { { 'ueb1101', 690, 120 }, { 'ueb1101', 880, 240 }, { 'ueb2101', 872, 232 }, { 'ueb2101', 888, 232 }, { 'ueb2101', 880, 252 }, { 'ueb1301', 700, 340 } } do
        make(d[1], CIV, d[2], d[3])
    end
    local land = { make('uel0201', 1, 780, 200), make('uel0201', 1, 783, 200) }
    local air = { make('uea0203', 1, 780, 204), make('uea0203', 1, 783, 204) }
    local pl = brain:MakePlatoon('ProbeLand', '') brain:AssignUnitsToPlatoon(pl, land, 'Attack', 'None')
    local pa = brain:MakePlatoon('ProbeAir', '') brain:AssignUnitsToPlatoon(pa, air, 'Attack', 'None')
    WaitTicks(3)
    brain:SetUpAttackVectorsToArmy(C.STRUCTURE)
    local v = brain:GetAttackVectors()
    local idx = {}
    for i, e in v or {} do idx[S(e)] = i end
    P('vectors n ' .. table.getn(v or {}))
    local SP = { { 780, 200 }, { 656, 250 }, { 900, 400 }, { 600, 100 }, { 700, 200 }, { 820, 120 }, { 760, 300 }, { 680, 160 }, { 860, 300 }, { 960, 200 } }
    for k, s in SP do
        for i, u in land do Warp(u, pos(s[1] + 3 * (i - 1), s[2])) end
        for i, u in air do Warp(u, { s[1] + 3 * (i - 1), GetTerrainHeight(s[1], s[2] + 4) + 10, s[2] + 4 }) end
        WaitTicks(2)
        for _, q in { { 'land', pl }, { 'air', pa } } do
            local res = {}
            for _, cmp in { 'Closest', 'Furthest', 'LeastDefended', 'HighestValue' } do
                local ok, r = pcall(function() return brain:PickBestAttackVector(q[2], 'Attack', 'Enemy', cmp, C.STRUCTURE) end)
                table.insert(res, cmp .. '=' .. (ok and (S(r) .. '#' .. tostring(idx[S(r)])) or ('ERR ' .. tostring(r))))
            end
            local okp, pp = pcall(function() return q[2]:GetPlatoonPosition() end)
            local ps = okp and pp and fmt('%.1f,%.1f', pp[1], pp[3]) or 'nil'
            P(fmt('AV2 %s squad %d,%d ppos %s %s', q[1], s[1], s[2], ps, table.concat(res, ' ')))
        end
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
