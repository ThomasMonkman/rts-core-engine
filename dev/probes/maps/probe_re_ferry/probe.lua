-- Differential probe (re-check of ai D56): where the real ferry pick-up's
-- wait comes from. tr2 (idle 100 ticks, so landed) gets IssueFerry A, B;
-- a platoon of two tanks uses its beacon (UseFerryBeacon). Per 2 ticks: the
-- transport's position, height, layer, queue; the tanks' position, moving
-- state, queue; plus motion / layer / transport events.
--   real game: dev/probes/run_faf.ps1 probe_re_ferry 80
--   ours: --headless --no-ai --ticks 700 --script dev/probes/maps/probe_re_ferry/probe.lua
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1 = armies[1]
local brain = ArmyBrains[1]
local start = GetGameTick()
local function T() return GetGameTick() - start end
local function P(s) LOG('PROBE ' .. T() .. ' ' .. s) end
local function make(bp, x, z)
    local u = CreateUnitHPR(bp, A1, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    pcall(function() u:SetFireState(1) end)
    return u
end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local QX, QZ = 656, 160
local f1 = make('uel0201', QX - 20, QZ + 20)
local f2 = make('uel0201', QX - 17, QZ + 20)
local tr2 = make('uea0107', QX - 30, QZ + 10)
local units = { f1 = f1, f2 = f2, tr2 = tr2 }
local order = { 'tr2', 'f1', 'f2' }
for name, u in units do
    local nm = name
    for _, cb in { 'OnMotionVertEventChange', 'OnLayerChange', 'OnStartTransportLoading', 'OnStopTransportLoading', 'OnTransportAttach', 'OnAttachedToTransport', 'OnMotionHorzEventChange' } do
        local old = u[cb]
        local cbn = cb
        u[cb] = function(self, a, b, c)
            local sa = type(a) == 'string' and a or (type(a) == 'table' and 'obj' or tostring(a))
            local sb = type(b) == 'string' and b or (type(b) == 'table' and 'obj' or tostring(b))
            P(fmt('ev %s %s %s %s', nm, cbn, sa, sb))
            if old then return old(self, a, b, c) end
        end
    end
end
local function q(u)
    local s = ''
    local ok = pcall(function() for _, c in u:GetCommandQueue() or {} do s = s .. tostring(c.commandType) .. ',' end end)
    return ok and s or 'E'
end
local function st(name)
    local u = units[name]
    if u:BeenDestroyed() then return name .. '=gone' end
    local x, y, z = u:GetPositionXYZ()
    return fmt('%s=%.2f,%.2f,h%.2f,%s,mv=%s,att=%s,q=%s', name, x, z, y - GetTerrainHeight(x, z), tostring(u:GetCurrentLayer()),
        tostring(u:IsUnitState('Moving')), tostring(u:IsUnitState('Attached')), q(u))
end

ForkThread(function()
    P('setup')
    WaitTicks(100)
    P('idle ' .. st('tr2'))
    local p3 = brain:MakePlatoon('ProbeF', '')
    brain:AssignUnitsToPlatoon(p3, { f1, f2 }, 'Attack', 'None')
    local FA, FB = pos(QX - 20, QZ + 34), pos(QX - 20, QZ + 100)
    IssueFerry({ tr2 }, FA)
    IssueFerry({ tr2 }, FB)
    WaitTicks(3)
    local beacon = tr2:GetTransportFerryBeacon()
    if beacon then
        local x, y, z = beacon:GetPositionXYZ()
        P(fmt('beacon %s %.2f,%.2f queue %d', beacon:GetUnitId(), x, z, table.getn(tr2:GetCommandQueue())))
        WaitTicks(1)
        local cmd = p3:UseFerryBeacon(categories.LAND, beacon)
        P('UseFerryBeacon ' .. type(cmd))
        for i = 0, 300, 2 do
            local line = 'st +' .. i
            for _, n in order do line = line .. ' ' .. st(n) end
            local ok, v = pcall(p3.IsMoving, p3, 'Attack')
            line = line .. ' movA=' .. tostring(ok and v)
            P(line)
            WaitTicks(2)
        end
    else
        P('no beacon')
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
