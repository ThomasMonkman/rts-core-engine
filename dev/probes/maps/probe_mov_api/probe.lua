-- Differential probe: movement script APIs and a few physics cases, logged
-- in a format both engines print identically ("PROBE ..." lines). Engine
-- APIs only.
--   real game (Windows): dev/probes/run_faf.ps1 probe_mov_api 150
--   ours: cargo run -- --headless --no-ai --ticks 700 --script dev/probes/maps/probe_mov_api/probe.lua
-- All uel0201 on Seton's flat ground unless said, facing +x.
-- M0..M4: a 30 move along +x: baseline, SetSpeedMult 0.5 / 2, SetAccMult 0.25 / 4.
-- M5, M6: a 14 move along +z (a 90-degree turn from rest): baseline, SetTurnMult 0.25.
-- I1: SetImmobile(true) at tick 5, a move at tick 10, SetImmobile(false) at tick 60.
-- I2: a move at tick 10, SetImmobile(true) at tick 30 (at speed), false at tick 70.
-- W1: idle, Warp 10 along +x at tick 20 (position read straight after the call).
-- W2: a 30 move at tick 10, Warp 4 sideways at tick 30: does it carry on.
-- W3: idle, Warp 10 straight up at tick 20.   W4: created 15 above the ground.
-- N1: GetNavigator values before / during / after a 25 move.
-- N2: navigator SetGoal 15 ahead at tick 10 (no order), AbortMove at tick 40.
-- N3: SetSpeedThroughGoal(true), then SetGoal 15 ahead at tick 10.
-- P: IssuePatrol to two points 20 and 12 apart: where it turns, for 500 ticks.
-- R: a UEF T1 land factory builds a tank (no rally point): its roll-off.
-- S*: tank, Mech Marine, Riptide (hover), T3 mobile artillery drive up a
--     ramp (5.3 up in 16) and back down: speed and pitch.
-- Lines: 'PROBE t=<tick> <tag> x y z h= yaw= hd= v= vy= pitch= mv= layer'
-- when changed; events 'PROBE t=<tick> <tag> <what> ...'.
local TICKS = 560
local armies = ListArmies()
local function f2(n) return string.format('%.2f', n) end
local function f3(n) return string.format('%.3f', n) end
local function str(v)
    if type(v) == 'number' then return f2(v) end
    if type(v) == 'table' then
        if type(v[1]) == 'number' and type(v[3]) == 'number' then return '{' .. f2(v[1]) .. ' ' .. f2(v[2]) .. ' ' .. f2(v[3]) .. '}' end
        return 'table'
    end
    return tostring(v)
end

for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end

local tracked, known = {}, {}
local function ev(tag, s) LOG('PROBE t=' .. GetGameTick() .. ' ' .. tag .. ' ' .. s) end
local function track(tag, u)
    table.insert(tracked, { tag = tag, u = u })
    known[u:GetEntityId()] = true
    for _, name in { 'OnMotionHorzEventChange', 'OnMotionVertEventChange', 'OnLayerChange' } do
        local old, n = u[name], name
        u[name] = function(self, new, o)
            ev(tag, n .. ' ' .. tostring(o) .. ' ' .. tostring(new))
            if old then return old(self, new, o) end
        end
    end
    return u
end
local function spawn(tag, x, z, bp, up)
    local u = CreateUnitHPR(bp or 'uel0201', armies[1], x, GetTerrainHeight(x, z) + (up or 0), z, 0, math.pi / 2, 0)
    u:SetFireState(1)
    return track(tag, u)
end
local function state(u)
    if u:BeenDestroyed() then return 'gone' end
    local x, y, z = u:GetPositionXYZ()
    local q = u:GetOrientation()
    local qx, qy, qz, qw = q[1], q[2], q[3], q[4]
    local fx, fy, fz = 2 * (qx * qz + qw * qy), 2 * (qy * qz - qw * qx), 1 - 2 * (qx * qx + qy * qy)
    local ok, vx, vy, vz = pcall(u.GetVelocity, u)
    local okm, mv = pcall(u.IsMoving, u)
    if not okm then mv = 'err' end
    if not ok then vx, vy, vz = 0, 0, 0 end
    local okh, hd = pcall(u.GetHeading, u)
    return f2(x) .. ' ' .. f2(y) .. ' ' .. f2(z) .. ' h=' .. f2(y - GetTerrainHeight(x, z)) .. ' yaw=' .. f3(math.atan2(fx, fz))
        .. ' hd=' .. (okh and f3(hd) or 'err') .. ' v=' .. f2(10 * math.sqrt(vx * vx + vz * vz)) .. ' vy=' .. f2(10 * vy)
        .. ' pitch=' .. f3(math.asin(math.max(-1, math.min(1, fy)))) .. ' mv=' .. tostring(mv) .. ' ' .. tostring(u:GetCurrentLayer())
end

local M = {}
for i = 0, 4 do M[i] = spawn('M' .. i, 596.5, 70.5 + i * 6) end
M[5] = spawn('M5', 640.5, 72.5) M[6] = spawn('M6', 660.5, 72.5)
local I1, I2 = spawn('I1', 600.5, 104.5), spawn('I2', 600.5, 110.5)
local W1, W2, W3 = spawn('W1', 600.5, 118.5), spawn('W2', 600.5, 124.5), spawn('W3', 650.5, 118.5)
local W4 = spawn('W4', 660.5, 118.5, nil, 15)
local N1, N2, N3 = spawn('N1', 640.5, 104.5), spawn('N2', 640.5, 110.5), spawn('N3', 640.5, 124.5)
local P = spawn('P', 330.5, 904.5)
local fac = track('FAC', CreateUnitHPR('ueb0101', armies[1], 380.5, GetTerrainHeight(380.5, 930.5), 930.5, 0, 0, 0))
local S = {}
for i, bp in { 'uel0201', 'uel0106', 'uel0203', 'uel0304' } do
    local m = { u = spawn('S' .. bp, 938.5, 178.5 + i * 6, bp), z = 178.5 + i * 6, phase = 0, still = 0 }
    table.insert(S, m)
end
for x = 936, 978, 2 do LOG('PROBE ramp ' .. x .. ' ' .. f2(GetTerrainHeight(x + 0.5, 184.5)) .. ' ' .. f2(GetTerrainHeight(x + 0.5, 190.5)) .. ' ' .. f2(GetTerrainHeight(x + 0.5, 196.5)) .. ' ' .. f2(GetTerrainHeight(x + 0.5, 202.5))) end

local function nav(tag, u, names)
    local ok, n = pcall(u.GetNavigator, u)
    if not ok or not n then ev(tag, 'nav none ' .. tostring(n)) return end
    local s = ''
    for _, name in names do
        local fn = n[name]
        if not fn then s = s .. ' ' .. name .. '=nil'
        else
            local ok2, r = pcall(fn, n)
            s = s .. ' ' .. name .. '=' .. (ok2 and str(r) or 'err')
        end
    end
    ev(tag, 'nav' .. s)
end
local NAV = { 'GetStatus', 'GetCurrentTargetPos', 'GetGoalPos', 'HasGoodPath', 'AtGoal', 'CanPathToGoal', 'FollowingLeader', 'IsIgnorningFormation' }
local function pos(u) local x, y, z = u:GetPositionXYZ() return x, y, z end

ForkThread(function()
    local start = GetGameTick()
    local brain = ArmyBrains[1]
    brain:GiveStorage('MASS', 50000) brain:GiveStorage('ENERGY', 500000)
    brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
    for t = 0, TICKS do
        if t == 5 then
            M[1]:SetSpeedMult(0.5) M[2]:SetSpeedMult(2) M[3]:SetAccMult(0.25) M[4]:SetAccMult(4) M[6]:SetTurnMult(0.25)
            I1:SetImmobile(true)
            N3:GetNavigator():SetSpeedThroughGoal(true)
        elseif t == 10 then
            for i = 0, 4 do IssueMove({ M[i] }, { 626.5, 0, 70.5 + i * 6 }) end
            IssueMove({ M[5] }, { 640.5, 0, 86.5 }) IssueMove({ M[6] }, { 660.5, 0, 86.5 })
            IssueMove({ I1 }, { 620.5, 0, 104.5 }) IssueMove({ I2 }, { 625.5, 0, 110.5 })
            IssueMove({ W2 }, { 630.5, 0, 124.5 })
            IssueMove({ N1 }, { 665.5, 0, 104.5 })
            N2:GetNavigator():SetGoal({ 655.5, GetTerrainHeight(655.5, 110.5), 110.5 })
            N3:GetNavigator():SetGoal({ 655.5, GetTerrainHeight(655.5, 124.5), 124.5 })
            IssuePatrol({ P }, { 350.5, 0, 904.5 })
            IssuePatrol({ P }, { 350.5, 0, 916.5 })
            IssueBuildFactory({ fac }, 'uel0201', 1)
        elseif t == 20 then
            local x, y, z = pos(W1)
            Warp(W1, { x + 10, y, z })
            local x2, y2, z2 = pos(W1)
            ev('W1', 'warped ' .. f2(x2 - x) .. ' ' .. f2(y2 - y) .. ' ' .. f2(z2 - z))
            x, y, z = pos(W3)
            Warp(W3, { x, y + 10, z })
            x2, y2, z2 = pos(W3)
            ev('W3', 'warped ' .. f2(x2 - x) .. ' ' .. f2(y2 - y) .. ' ' .. f2(z2 - z))
        elseif t == 30 then
            local x, y, z = pos(W2)
            Warp(W2, { x, y, z + 4 })
            I2:SetImmobile(true)
        elseif t == 40 then
            N2:GetNavigator():AbortMove()
        elseif t == 60 then
            I1:SetImmobile(false)
        elseif t == 70 then
            I2:SetImmobile(false)
        end
        if t == 8 or t == 12 or t == 20 or t == 40 or t == 60 or t == 100 or t == 140 then
            nav('N1', N1, NAV)
            nav('N2', N2, NAV)
        end
        if t == 8 then nav('FAC', fac, NAV) end
        -- the factory's tank
        for _, u in brain:GetListOfUnits(categories.MOBILE * categories.LAND, false) or {} do
            if not known[u:GetEntityId()] then
                local x, y, z = pos(u)
                ev('R', 'new ' .. f2(x) .. ' ' .. f2(z) .. ' frac=' .. f2(u:GetFractionComplete()))
                track('R', u)
            end
        end
        -- the ramp
        for _, m in S do
            local u = m.u
            if not u:BeenDestroyed() then
                local vx, vy, vz = u:GetVelocity()
                if math.abs(vx) + math.abs(vz) < 0.0001 then m.still = m.still + 1 else m.still = 0 end
                if m.phase == 0 and t >= 10 then IssueMove({ u }, { 976.5, 0, m.z }) m.phase = 1
                elseif m.phase == 1 and m.still >= 10 and u:IsIdleState() then IssueMove({ u }, { 938.5, 0, m.z }) m.phase = 2 end
            end
        end
        local at = 'PROBE t=' .. (GetGameTick() - start) .. ' '
        for _, e in tracked do
            local s = state(e.u)
            if s ~= e.last then LOG(at .. e.tag .. ' ' .. s) e.last = s end
        end
        WaitTicks(1)
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
