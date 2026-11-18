-- Sim half of probe_re3_ui: (1) exists / DiskGetFileInfo on loose and
-- archived paths (both layers); (2) GetIdleEngineers with only the ACU, then
-- with engineers; (3) right-click orders with a checked cursor (ui_probe.lua).
--   real game (Windows): dev/probes/run_faf.ps1 probe_re3_ui 150
--   ours: --session --player 1 --no-ai --vault dev/probes --ticks 900 --script dev/probes/maps/probe_re3_ui/probe.lua
--         --ui-lua "$(cat dev/probes/maps/probe_re3_ui/ui_probe.lua)"
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local brain = ArmyBrains[1]
local function P(s) LOG('PROBE sim ' .. GetGameTick() .. ' ' .. s) end

-- (1) paths
local PATHS = {
    '/maps/probe_re3_ui', '/maps/probe_re3_ui/', '/maps/probe_re3_ui/lua/AI/CustomAIs_v2', '/maps/probe_re3_ui/lua/AI/CustomAIs_v2/',
    '/maps/probe_re3_ui/lua/AI/CustomAIs_v2/marker.lua', '/maps/probe_re3_ui/probe.lua', '/maps/probe_re3_ui/nope', '/maps/probe_re3_ui/nope/',
    '/MAPS/PROBE_RE3_UI', '/maps', '/maps/', '/schook', '/schook/', '/lua', '/lua/', '/lua/AI', '/lua/AI/', '/lua/AI/AIBaseTemplates',
    '/lua/ui/uiutil.lua', '/lua/nope.lua', '/maps/SCMP_009', '/maps/SCMP_009/', '/maps/SCMP_009/SCMP_009_scenario.lua', '/mods', '/mods/',
}
local function fileinfo(v)
    if type(v) ~= 'table' then return tostring(v) end
    local keys = {}
    for k in v do table.insert(keys, tostring(k)) end
    table.sort(keys)
    local s = '{'
    for _, k in keys do
        local x = v[k]
        if k == 'TimeStamp' or k == 'WriteTime' then x = '<' .. type(x) .. '>' end
        if type(x) == 'number' then x = fmt('%d', x) end
        s = s .. ' ' .. k .. '=' .. tostring(x)
    end
    return s .. ' }'
end
local function paths(tag)
    for _, p in PATHS do
        local oke, e = pcall(function() return exists(p) end)
        local okd, d = pcall(function() return DiskGetFileInfo(p) end)
        LOG('PROBE ' .. tag .. ' path ' .. p .. ' exists=' .. (oke and tostring(e) or ('ERR ' .. tostring(e))) .. ' info=' .. (okd and fileinfo(d) or ('ERR ' .. tostring(d))))
    end
    local ok, f = pcall(function() return DiskFindFiles('/maps/probe_re3_ui/lua/AI/CustomAIs_v2', '*.lua') end)
    LOG('PROBE ' .. tag .. ' findfiles ' .. (ok and (type(f) == 'table' and (table.getn(f) .. ' ' .. tostring(f[1])) or tostring(f)) or ('ERR ' .. tostring(f))))
end
paths('sim')

brain:GiveStorage('MASS', 50000) brain:GiveStorage('ENERGY', 500000)
ForkThread(function()
    while true do
        WaitTicks(5)
        brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
    end
end)

local named = {}
local function make(name, bp, x, z, army)
    local u = CreateUnitHPR(bp, army or armies[1], x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetCustomName(name)
    named[name] = u
    if u.SetFireState then pcall(u.SetFireState, u, 1) end
    P('made ' .. name .. ' ' .. bp .. ' id=' .. tostring(u:GetEntityId()))
    return u
end

-- (2) phase 1: only the ACU (and an enemy structure).
make('p_acu', 'uel0001', 394, 598)
make('p_enemy', 'urb1101', 394.5, 570.5, armies[2])

ForkThread(function()
    while GetGameTick() < 200 do WaitTicks(1) end
    -- phase 2: the rest, spread out (>= 12 apart).
    make('p_ideng', 'uel0105', 370, 640)
    local beng = make('p_busyeng', 'uel0105', 360, 600)
    make('p_scu', 'uel0301', 350, 620)
    make('p_tank', 'uel0201', 380, 600)
    make('p_tank2', 'uel0201', 420, 640)
    make('p_trans', 'uea0107', 410, 600)
    local t2 = make('p_trans2', 'uea0107', 440, 600)
    local cargo = make('p_cargo', 'uel0106', 444, 600)
    make('p_shield', 'uel0307', 395, 625)
    make('p_radar', 'ueb3101', 430.5, 625.5)
    make('p_pgen', 'ueb1101', 444.5, 640.5)
    make('p_fac', 'ueb0101', 400.5, 655.5)
    WaitTicks(2)
    IssueBuildMobile({ beng }, { 360.5, GetTerrainHeight(360.5, 588.5), 588.5 }, 'ueb1101', {})
    IssueTransportLoad({ cargo }, t2)
    while GetGameTick() < 250 do WaitTicks(1) end
    for _, n in { 'p_trans', 'p_trans2', 'p_tank', 'p_shield', 'p_busyeng', 'p_ideng', 'p_scu', 'p_acu' } do
        local u = named[n]
        local x, y, z = u:GetPositionXYZ()
        P('pos ' .. n .. fmt(' %.2f %.2f %.2f', x, y, z) .. ' terrain ' .. fmt('%.2f', GetTerrainHeight(x, z)) .. ' idle ' .. tostring(u:IsIdleState()) .. ' q ' .. table.getn(u:GetCommandQueue()))
    end
end)

ForkThread(function()
    while true do
        Sync.ProbeTick = GetGameTick()
        WaitTicks(1)
    end
end)
