-- Differential probe: SetSpeedMult / SetAccMult across motion types, and
-- what a Warp or IssueStop does to a move in progress, logged in a format
-- both engines print identically ("PROBE ..." lines). Engine APIs only.
--   real game (Windows): dev/probes/run_faf.ps1 probe_mov_mult 120
--   ours: cargo run -- --headless --no-ai --ticks 500 --script dev/probes/maps/probe_mov_mult/probe.lua
-- L<m>: uel0201 (land) with SetSpeedMult(m), a 30 move along +x.
-- H<m>: uel0203 (hover), the same.   N<m>: ues0103 (frigate), a 60 move.
-- G<m>: uea0203 (gunship), I<m>: uea0102 (interceptor), a 300 move.
-- Lmid: uel0201, SetSpeedMult(0.5) at tick 40, at speed.
-- La<m> / Ia<m> / Ga<m>: SetAccMult(m) instead.
-- WP: uel0201 on a 30 move, Warp 4 sideways at tick 30; a new move to the
--     same goal at tick 120.
-- SQ: uel0201 with moves queued round a 10 square, IssueStop at tick 30.
-- Lines: 'PROBE t=<tick> <tag> x z v= idle= q=<queued command types>' when
-- changed.
local TICKS = 420
local armies = ListArmies()
local function f2(n) return string.format('%.2f', n) end

for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end

local tracked = {}
local function spawn(tag, bp, x, z, dist, water, air)
    local y = water and GetSurfaceHeight(x, z) or GetTerrainHeight(x, z)
    local u = CreateUnitHPR(bp, armies[1], x, y + (air and 20 or 0), z, 0, math.pi / 2, 0)
    u:SetFireState(1)
    local e = { tag = tag, u = u, goal = { x + dist, 0, z } }
    table.insert(tracked, e)
    return u, e
end
local function state(u)
    if u:BeenDestroyed() then return 'gone' end
    local x, y, z = u:GetPositionXYZ()
    local vx, vy, vz = u:GetVelocity()
    local s = ''
    for _, c in u:GetCommandQueue() or {} do s = s .. tostring(c.commandType) .. ',' end
    return f2(x) .. ' ' .. f2(z) .. ' v=' .. f2(10 * math.sqrt(vx * vx + vz * vz)) .. ' idle=' .. tostring(u:IsIdleState()) .. ' q=' .. s
end

local movers = {}
local function mult(prefix, bp, list, x, z, dz, dist, water, air, acc)
    for i, m in list do
        local u, e = spawn(prefix .. m, bp, x, z + (i - 1) * dz, dist, water, air)
        if acc then u:SetAccMult(m) else u:SetSpeedMult(m) end
        table.insert(movers, e)
    end
end
mult('L', 'uel0201', { 0.25, 0.5, 0.75, 0.9, 1.5, 3 }, 596.5, 70.5, 6, 30)
mult('La', 'uel0201', { 0.5, 2 }, 596.5, 106.5, 6, 30, false, false, true)
mult('H', 'uel0203', { 0.25, 0.5, 0.75, 2 }, 640, 72, 6, 30)
mult('N', 'ues0103', { 0.5, 0.75, 2 }, 60, 60, 30, 60, true)
mult('G', 'uea0203', { 0.5, 2 }, 100, 300, 15, 300, true, true)
mult('I', 'uea0102', { 0.5, 0.75, 2 }, 100, 340, 15, 300, true, true)
mult('Ia', 'uea0102', { 0.25 }, 100, 400, 15, 300, true, true, true)
mult('Ga', 'uea0203', { 0.25 }, 100, 420, 15, 300, true, true, true)
local Lmid, emid = spawn('Lmid', 'uel0201', 596.5, 118.5, 30)
table.insert(movers, emid)
local WP, ewp = spawn('WP', 'uel0201', 596.5, 124.5, 30)
table.insert(movers, ewp)
local SQ = spawn('SQ', 'uel0201', 330.5, 920.5, 0)

ForkThread(function()
    local start = GetGameTick()
    for t = 0, TICKS do
        if t == 10 then
            for _, e in movers do IssueMove({ e.u }, e.goal) end
            IssueMove({ SQ }, { 345.5, 0, 920.5 })
            IssueMove({ SQ }, { 345.5, 0, 930.5 })
            IssueMove({ SQ }, { 330.5, 0, 930.5 })
        elseif t == 30 then
            local x, y, z = WP:GetPositionXYZ()
            Warp(WP, { x, y, z + 4 })
            IssueStop({ SQ })
        elseif t == 40 then
            Lmid:SetSpeedMult(0.5)
        elseif t == 120 then
            IssueMove({ WP }, ewp.goal)
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
