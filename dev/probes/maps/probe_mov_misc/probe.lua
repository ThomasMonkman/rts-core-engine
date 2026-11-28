-- Differential probe: assorted movement cases, logged in a format both
-- engines print identically ("PROBE ..." lines). Engine APIs only.
--   real game (Windows): dev/probes/run_faf.ps1 probe_mov_misc 150
--   ours: cargo run -- --headless --no-ai --ticks 700 --script dev/probes/maps/probe_mov_misc/probe.lua
-- All uel0201 on hold fire unless said.
-- U1: on the north beach, ordered 50 out into water 5 deep (unreachable):
--     where it ends and when it goes idle.
-- U2: ordered to the centre of a T1 land factory 20 ahead.
-- U3a / U3b: drives through a row of three idle tanks across its lane:
--     army 1's own (a) and another army's (b); the blockers' positions too.
-- G: 'Gb' guards 'Ga' (IssueGuard), which is then sent 40 along +x.
-- SP: SetPosition 10 along +x at tick 20 (second argument true).
-- HG: a UEF T2 gunship idle over deep water (it can't land): its drift.
-- FM: six tanks in a 3 x 2 block form-moved 40 along +x
--     (IssueFormMove, 'AttackFormation', facing 0): each one's position.
-- Q: a move 20 along +x, then at tick 30 (at speed) IssueClearCommands
--    alone: how it comes to rest.
-- ST: three moves queued (a 10 square); IssueStop at tick 30 on the first
--    leg, alone: what's left of the queue.
-- Lines: 'PROBE t=<tick> <tag> x z h= yaw= v= idle= mv= layer' when changed.
local TICKS = 520
local armies = ListArmies()
local function f2(n) return string.format('%.2f', n) end
local function f3(n) return string.format('%.3f', n) end

for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end

local tracked = {}
local function ev(tag, s) LOG('PROBE t=' .. GetGameTick() .. ' ' .. tag .. ' ' .. s) end
local function spawn(tag, x, z, bp, army, air)
    local u = CreateUnitHPR(bp or 'uel0201', army or armies[1], x, GetTerrainHeight(x, z) + (air and 20 or 0), z, 0, math.pi / 2, 0)
    u:SetFireState(1)
    table.insert(tracked, { tag = tag, u = u })
    return u
end
local function state(u)
    if u:BeenDestroyed() then return 'gone' end
    local x, y, z = u:GetPositionXYZ()
    local q = u:GetOrientation()
    local qx, qy, qz, qw = q[1], q[2], q[3], q[4]
    local fx, fz = 2 * (qx * qz + qw * qy), 1 - 2 * (qx * qx + qy * qy)
    local ok, vx, vy, vz = pcall(u.GetVelocity, u)
    local okm, mv = pcall(u.IsMoving, u)
    if not okm then mv = 'err' end
    if not ok then vx, vy, vz = 0, 0, 0 end
    return f2(x) .. ' ' .. f2(z) .. ' h=' .. f2(y - GetTerrainHeight(x, z)) .. ' yaw=' .. f3(math.atan2(fx, fz))
        .. ' v=' .. f2(10 * math.sqrt(vx * vx + vz * vz)) .. ' idle=' .. tostring(u:IsIdleState()) .. ' mv=' .. tostring(mv)
        .. ' ' .. tostring(u:GetCurrentLayer())
end

local U1 = spawn('U1', 656.5, 256.5)
spawn('FAC', 640.5, 100.5, 'ueb0101')
local U2 = spawn('U2', 615.5, 100.5)
local U3a, U3b = spawn('U3a', 596.5, 72.5), spawn('U3b', 596.5, 124.5)
for i = -1, 1 do
    spawn('Ba' .. (i + 2), 608.5, 72.5 + i * 1.5)
    spawn('Bb' .. (i + 2), 608.5, 124.5 + i * 1.5, nil, armies[2])
end
local Ga, Gb = spawn('Ga', 600.5, 84.5), spawn('Gb', 596.5, 88.5)
local SP = spawn('SP', 660.5, 72.5)
local HG = spawn('HG', 300, 100, 'uea0203', nil, true)
local FM = {}
for i = 0, 5 do FM[i + 1] = spawn('FM' .. (i + 1), 330.5 + math.mod(i, 3) * 3, 920.5 + math.floor(i / 3) * 3) end
local Q = spawn('Q', 660.5, 84.5)
local ST = spawn('ST', 330.5, 940.5)

ForkThread(function()
    local start = GetGameTick()
    for t = 0, TICKS do
        if t == 10 then
            IssueMove({ U1 }, { 606.5, 0, 256.5 })
            IssueMove({ U2 }, { 640.5, 0, 100.5 })
            IssueMove({ U3a }, { 626.5, 0, 72.5 })
            IssueMove({ U3b }, { 626.5, 0, 124.5 })
            IssueGuard({ Gb }, Ga)
            IssueFormMove(FM, { 372.5, 0, 922.5 }, 'AttackFormation', 0)
            IssueMove({ Q }, { 680.5, 0, 84.5 })
            IssueMove({ ST }, { 345.5, 0, 940.5 })
            IssueMove({ ST }, { 345.5, 0, 950.5 })
            IssueMove({ ST }, { 330.5, 0, 950.5 })
        elseif t == 20 then
            local x, y, z = SP:GetPositionXYZ()
            SP:SetPosition({ x + 10, y, z }, true)
            local x2, y2, z2 = SP:GetPositionXYZ()
            ev('SP', 'set ' .. f2(x2 - x) .. ' ' .. f2(y2 - y) .. ' ' .. f2(z2 - z))
        elseif t == 30 then
            IssueClearCommands({ Q })
            IssueStop({ ST })
        elseif t == 40 then
            IssueMove({ Ga }, { 640.5, 0, 84.5 })
        end
        if t == 29 or t == 31 or t == 33 or t == 60 then
            for _, e in { { 'Q', Q }, { 'ST', ST } } do
                local q = e[2]:GetCommandQueue()
                local s = ''
                for _, c in q or {} do s = s .. ' ' .. tostring(c.commandType) end
                ev(e[1], 'queue' .. s)
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
