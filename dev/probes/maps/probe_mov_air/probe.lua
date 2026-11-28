-- Differential probe: aircraft fuel, take-off, air staging and transports,
-- logged in a format both engines print identically ("PROBE ..." lines).
-- Engine APIs only.
--   real game (Windows): dev/probes/run_faf.ps1 probe_mov_air 150
--   ours: cargo run -- --headless --no-ai --ticks 900 --script dev/probes/maps/probe_mov_air/probe.lua
-- F1: a UEF interceptor flies 500 along +x: fuel ratio as it goes.
-- F2: a UEF interceptor idle over flat land: lands; fuel ratio in the air
--     and on the ground.
-- F3: as F1 with SetFuelRatio(0.01) at tick 10: speed once it's out.
-- F4: a UEF interceptor (fuel 0.3, half health) ordered onto a UEF air
--     staging platform 40 away (IssueTransportLoad); fuel / health while
--     docked; ordered away at tick 420.
-- TK*: seven aircraft idle over flat land (they land), ordered 60 along +x
--     at tick 160: the landing and the take-off.
-- TR1: a UEF T1 transport loads 4 tanks (IssueTransportLoad at tick 10),
--     then unloads them 50 away (IssueTransportUnload) once all are aboard.
-- TR2: a UEF T2 transport with 2 uel0303, 4 uel0201, 2 uel0101, the same.
-- TR3: a UEF T1 transport loads 2 tanks, flies off and is killed 40 ticks
--     after loading: what happens to the cargo.
-- Lines: 'PROBE t=<tick> <tag> x y z h= v= vy= layer fuel= hp= att=' per
-- tick per unit (cargo too), and events 'PROBE t=<tick> <tag> <event> ...'.
local TICKS = 700
local armies = ListArmies()
local function f2(n) return string.format('%.2f', n) end
local function f4(n) return string.format('%.4f', n) end

for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end

local tracked = {}
local function ev(tag, s) LOG('PROBE t=' .. GetGameTick() .. ' ' .. tag .. ' ' .. s) end
local function hook(u, tag, name, fmt)
    local old = u[name]
    u[name] = function(self, a, b, c)
        ev(tag, name .. ' ' .. fmt(a, b, c))
        if old then return old(self, a, b, c) end
    end
end
local function two(a, b) return tostring(a) .. ' ' .. tostring(b) end
local function spawn(tag, bp, x, z, yaw, air)
    local u = CreateUnitHPR(bp, armies[1], x, GetTerrainHeight(x, z) + (air and 20 or 0), z, 0, yaw or math.pi / 2, 0)
    u:SetFireState(1)
    table.insert(tracked, { tag = tag, u = u })
    hook(u, tag, 'OnMotionHorzEventChange', function(new, old) return two(old, new) end)
    hook(u, tag, 'OnMotionVertEventChange', function(new, old) return two(old, new) end)
    hook(u, tag, 'OnLayerChange', function(new, old) return two(old, new) end)
    hook(u, tag, 'OnRunOutOfFuel', function() return '' end)
    hook(u, tag, 'OnGotFuel', function() return '' end)
    hook(u, tag, 'OnAttachedToTransport', function(t, bone) return tostring(bone) end)
    hook(u, tag, 'OnDetachedFromTransport', function(t, bone) return tostring(bone) end)
    hook(u, tag, 'OnTransportAttach', function(bone, unit) return tostring(bone) end)
    hook(u, tag, 'OnTransportDetach', function(bone, unit) return tostring(bone) end)
    hook(u, tag, 'OnStartTransportLoading', function() return '' end)
    hook(u, tag, 'OnStopTransportLoading', function() return '' end)
    hook(u, tag, 'OnKilled', function() return '' end)
    hook(u, tag, 'OnImpact', function(with) return tostring(with) end)
    return u
end

local function state(u)
    if u:BeenDestroyed() then return 'gone' end
    local x, y, z = u:GetPositionXYZ()
    local ok, vx, vy, vz = pcall(u.GetVelocity, u)
    if not ok then vx, vy, vz = 0, 0, 0 end
    local okf, fuel = pcall(u.GetFuelRatio, u)
    return f2(x) .. ' ' .. f2(y) .. ' ' .. f2(z) .. ' h=' .. f2(y - GetTerrainHeight(x, z))
        .. ' v=' .. f2(10 * math.sqrt(vx * vx + vz * vz)) .. ' vy=' .. f2(10 * vy)
        .. ' ' .. tostring(u:GetCurrentLayer()) .. ' fuel=' .. (okf and f4(fuel) or 'err') .. ' hp=' .. f2(u:GetHealth())
        .. ' att=' .. tostring(u:IsUnitState('Attached')) .. ' dead=' .. tostring(u.Dead or false)
end

local f1 = spawn('F1', 'uea0102', 100, 300, nil, true)
local f2u = spawn('F2', 'uea0102', 300, 934, nil, true)
local f3 = spawn('F3', 'uea0102', 100, 330, nil, true)
local stage = spawn('STG', 'ueb5202', 662, 134, 0)
local f4u = spawn('F4', 'uea0102', 622, 134, nil, true)
local tk = {}
for i, bp in { 'uea0101', 'uea0103', 'uea0203', 'uea0107', 'uea0304', 'uaa0303', 'xra0105' } do
    tk[i] = spawn('TK' .. bp, bp, 356, 897 + i * 7, nil, true)
end

local function cargo(tag, list, x, z)
    local out = {}
    local n = 0
    for _, e in list do
        for j = 1, e[2] do
            n = n + 1
            table.insert(out, spawn(tag .. 'c' .. n, e[1], x + math.mod(n - 1, 4) * 3, z + math.floor((n - 1) / 4) * 3))
        end
    end
    return out
end
local tr = {
    { tag = 'TR1', t = spawn('TR1', 'uea0107', 630, 76, nil, true), c = cargo('TR1', { { 'uel0201', 4 } }, 598, 76), dest = { 682, 76 } },
    { tag = 'TR2', t = spawn('TR2', 'uea0104', 632, 104, nil, true), c = cargo('TR2', { { 'uel0303', 2 }, { 'uel0201', 4 }, { 'uel0101', 2 } }, 598, 102), dest = { 692, 100 } },
    { tag = 'TR3', t = spawn('TR3', 'uea0107', 300, 912, nil, true), c = cargo('TR3', { { 'uel0201', 2 } }, 330, 912), kill = true },
}

local function aboard(c)
    local n = 0
    for _, u in c do if not u:BeenDestroyed() and u:IsUnitState('Attached') then n = n + 1 end end
    return n
end

ForkThread(function()
    local start = GetGameTick()
    for t = 0, TICKS do
        if t == 10 then
            IssueMove({ f1 }, { 600, 0, 300 })
            IssueMove({ f3 }, { 600, 0, 330 })
            f3:SetFuelRatio(0.01)
            f4u:SetFuelRatio(0.3)
            f4u:SetHealth(f4u, f4u:GetMaxHealth() / 2)
            IssueTransportLoad({ f4u }, stage)
            for _, g in tr do IssueTransportLoad(g.c, g.t) g.phase = 1 end
        elseif t == 160 then
            for i, u in tk do
                local x, y, z = u:GetPositionXYZ()
                IssueMove({ u }, { x + 60, 0, z })
            end
        elseif t == 420 then
            IssueMove({ f4u }, { 622, 0, 134 })
        end
        for _, g in tr do
            local n = aboard(g.c)
            if g.phase == 1 and n == table.getn(g.c) then
                ev(g.tag, 'loaded')
                local tx, ty, tz = g.t:GetPositionXYZ()
                for i, u in g.c do
                    local x, y, z = u:GetPositionXYZ()
                    ev(g.tag .. 'c' .. i, 'slot ' .. f2(x - tx) .. ' ' .. f2(y - ty) .. ' ' .. f2(z - tz))
                end
                if g.kill then
                    IssueMove({ g.t }, { 420, 0, 912 })
                    g.phase, g.at = 2, t + 40
                else
                    IssueTransportUnload({ g.t }, { g.dest[1], 0, g.dest[2] })
                    g.phase = 3
                end
            elseif g.phase == 2 and t >= g.at then
                ev(g.tag, 'kill')
                g.t:Kill()
                g.phase = 4
            elseif g.phase == 3 and n == 0 then
                ev(g.tag, 'unloaded')
                g.phase = 4
            end
        end
        local at = 'PROBE t=' .. (GetGameTick() - start) .. ' '
        for _, e in tracked do
            local s = state(e.u)
            if s ~= e.last or math.mod(t, 10) == 0 then LOG(at .. e.tag .. ' ' .. s) e.last = s end
        end
        WaitTicks(1)
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
