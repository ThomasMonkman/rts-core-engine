-- Sim half of the interface probes (probe_ui_*): a small fixed base for army 1
-- on Seton's Clutch so the interface half (ui_probe.lua) has known units to
-- read: moving, building, paused, being built, shielded, fuelled, guarding.
-- Each unit carries a custom name (p_<role>) the interface half finds it by.
--   real game (Windows): dev/probes/run_faf.ps1 probe_ui_<name> 120
--   ours: cargo run -- --session --player 1 --no-ai --ticks 500 --script dev/probes/maps/probe_ui_<name>/probe.lua
--         --ui-lua "$(cat dev/probes/maps/probe_ui_<name>/ui_probe.lua)"
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local brain = ArmyBrains[1]
brain:GiveStorage('MASS', 50000) brain:GiveStorage('ENERGY', 500000)
ForkThread(function()
    while true do
        brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
        WaitTicks(5)
    end
end)

local named = {}
local function make(name, bp, x, z, army)
    local u = CreateUnitHPR(bp, army or armies[1], x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetCustomName(name)
    named[name] = u
    return u
end
local function try(what, f)
    local ok, err = pcall(f)
    if not ok then LOG('PROBE sim ERR ' .. what .. ' ' .. tostring(err)) end
end

local tank = make('p_tank', 'uel0201', 380, 600)
local guard = make('p_guard', 'uel0103', 384, 600)
local eng = make('p_eng', 'uel0105', 380, 614)
local eng2 = make('p_eng2', 'uel0105', 384, 614)
local ideng = make('p_ideng', 'uel0105', 400, 622)
local fac = make('p_fac', 'ueb0101', 400.5, 606.5)
local fac2 = make('p_fac2', 'ueb0101', 416.5, 606.5)
local shield = make('p_shield', 'uel0307', 390, 622)
local air = make('p_air', 'uea0102', 394, 632)
local tml = make('p_tml', 'ueb2108', 410.5, 620.5)
local radar = make('p_radar', 'ueb3101', 420.5, 620.5)
local pgen = make('p_pgen', 'ueb1101', 426.5, 612.5)
local fab = make('p_fab', 'ueb1104', 430.5, 620.5)
local acu = make('p_acu', 'uel0001', 394, 598)
local trans = make('p_trans', 'uea0107', 436, 600)
local cargo = make('p_cargo', 'uel0106', 440, 600)
local enemy = make('p_enemy', 'urb1101', 394.5, 584.5, armies[2])

for _, u in { tank, guard, acu, air, cargo } do u:SetFireState(1) end
try('SetHealth', function() tank:SetHealth(tank, 100) end)
try('SetFuelRatio', function() air:SetFuelRatio(0.5) end)
try('SetWorkProgress', function() radar:SetWorkProgress(0.25) end)
try('SetStat', function() tank:SetStat('KILLS', 3) end)
try('SiloAmmo', function() tml:GiveTacticalSiloAmmo(2) end)

local ids = {}
for name, u in named do ids[name] = u:GetEntityId() end
local line = 'PROBE sim ids'
for _, name in { 'p_tank', 'p_eng', 'p_fac', 'p_enemy' } do line = line .. ' ' .. name .. '=' .. tostring(ids[name]) .. ':' .. type(ids[name]) end
LOG(line)

-- Sim callbacks the interface half sends (SimCallback timing / arguments):
-- FAF's DoCallback wrapped to log each one and echo it through Sync.
try('Callbacks', function()
    local mod = import('/lua/SimCallbacks.lua')
    local old = mod.DoCallback
    mod.DoCallback = function(name, data, units)
        if name == 'SetStatByCallback' or name == 'EmptyCallback' then
            local n = 0
            local first = '-'
            if units then for _, u in units do n = n + 1 if n == 1 then first = u:GetBlueprint().BlueprintId end end end
            LOG('PROBE sim cb ' .. name .. ' tick ' .. GetGameTick() .. ' data ' .. type(data) .. ' units ' .. type(units) .. ' ' .. n .. ' ' .. first)
            Sync.ProbeEcho = { name = name, tick = GetGameTick(), units = n, nested = { 1, 2, { x = 'y' } }, f = 1.5, b = true }
        end
        return old(name, data, units)
    end
end)

local function fmt(n) return string.format('%.2f', n) end
local function safe(f)
    local ok, r = pcall(f)
    if ok then return type(r) == 'number' and fmt(r) or tostring(r) end
    return 'ERR'
end
local function truth(tag)
    for _, name in { 'p_tank', 'p_eng', 'p_fac', 'p_air', 'p_shield', 'p_radar', 'p_tml', 'p_cargo' } do
        local u = named[name]
        if u:BeenDestroyed() then
            LOG('PROBE sim ' .. tag .. ' ' .. name .. ' gone')
        else
            local x, y, z = u:GetPositionXYZ()
            LOG('PROBE sim ' .. tag .. ' ' .. name .. ' pos ' .. fmt(x) .. ' ' .. fmt(y) .. ' ' .. fmt(z) .. ' hp ' .. fmt(u:GetHealth()) .. '/' .. fmt(u:GetMaxHealth())
                .. ' work ' .. safe(function() return u:GetWorkProgress() end) .. ' fuel ' .. safe(function() return u:GetFuelRatio() end) .. ' shield ' .. safe(function() return u:GetShieldRatio(u) end)
                .. ' focus ' .. safe(function() local focus = u:GetFocusUnit() return focus and (focus:GetBlueprint().BlueprintId .. '@' .. fmt(focus:GetFractionComplete())) or '-' end)
                .. ' paused ' .. safe(function() return u:IsPaused() end) .. ' attached ' .. safe(function() return u:IsUnitState('Attached') end)
                .. ' fire ' .. safe(function() return u:GetFireState() end) .. ' bit3 ' .. safe(function() return u:GetScriptBit(3) end))
        end
    end
end

-- Every tick: the ids and the tick, so the interface half can see when a
-- Sync field arrives.
ForkThread(function()
    while true do
        Sync.ProbeUnits = ids
        Sync.ProbeTick = GetGameTick()
        WaitTicks(1)
    end
end)

ForkThread(function()
    WaitTicks(2)
    IssueBuildMobile({ eng }, { 380.5, GetTerrainHeight(380.5, 606.5), 606.5 }, 'ueb0101', {})
    IssueBuildMobile({ eng }, { 370.5, GetTerrainHeight(370.5, 614.5), 614.5 }, 'ueb1101', {})
    IssueGuard({ eng2 }, eng)
    IssueGuard({ guard }, tank)
    IssueBuildFactory({ fac }, 'uel0201', 3)
    IssueBuildFactory({ fac }, 'uel0106', 2)
    IssueTransportLoad({ cargo }, trans)
    IssuePatrol({ air }, { 380, GetTerrainHeight(380, 640), 640 })
    IssuePatrol({ air }, { 420, GetTerrainHeight(420, 640), 640 })
    while GetGameTick() < 120 do WaitTicks(1) end
    truth('t120')
    while GetGameTick() < 180 do WaitTicks(1) end
    fac:SetPaused(true)
    IssueMove({ tank }, { 420, GetTerrainHeight(420, 594), 594 })
    IssueMove({ tank }, { 420, GetTerrainHeight(420, 580), 580 })
    while GetGameTick() < 220 do WaitTicks(1) end
    truth('t220')
    for _, t in { 262, 300, 330 } do
        while GetGameTick() < t do WaitTicks(1) end
        truth('t' .. t)
        LOG('PROBE sim t' .. t .. ' guard fire ' .. safe(function() return guard:GetFireState() end) .. ' eng paused ' .. safe(function() return eng:IsPaused() end)
            .. ' eng2 paused ' .. safe(function() return eng2:IsPaused() end) .. ' fab bit3 ' .. safe(function() return fab:GetScriptBit(3) end)
            .. ' tank ProbeStat ' .. safe(function() return tank:GetStat('ProbeStat', 0).Value end))
    end
end)

-- A plane killed in flight: when does the interface stop seeing it as a unit?
ForkThread(function()
    while GetGameTick() < 420 do WaitTicks(1) end
    LOG('PROBE sim air kill tick ' .. GetGameTick())
    air:Kill()
    while not air:BeenDestroyed() do WaitTicks(1) end
    LOG('PROBE sim air destroyed tick ' .. GetGameTick())
end)
