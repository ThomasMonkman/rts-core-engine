-- Differential probe (sweep 3): movement.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re3_mov 150
--   ours: --headless --no-ai --ticks 700 --script dev/probes/maps/probe_re3_mov/probe.lua
-- GM  gunships uea0203 / ura0203 with SetSpeedMult / SetAccMult 0.25..1, a 300 move along +x:
--     'GM t tag x z h v5' every 5 ticks (v5 = horizontal speed over the last 5 ticks, per second).
-- GP  gunships patrolling (created on the ground, as probe_wpn_aa): +-6 and +-30 legs, every 5 ticks.
-- GD  guard pairs (land/land, air/land, engineer/structure, naval) at start distances 4/10/20/35;
--     the guarded mobile units move 60 along +x at t100. 'GD t tag d= v=' every 10 ticks.
-- FL  planes uea0102 / uea0103 on a 300 leg then a 90 degree turn: every tick
--     'FL t tag x z h vh vy hdg dhdg' (vh from GetVelocity, per second; hdg in degrees).
-- MS  MovingUp / MovingDown: sub dive / surface, planes made on the ground, transport load / unload,
--     a plane to an air staging. 'MS t tag state [...] h=' on change, 'MSH t tag h=' every 5 ticks.
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local A1 = 1
local brain = ArmyBrains[A1]
local t0 = 15
local function T() return GetGameTick() - t0 end
local function P(text) LOG(fmt('PROBE %d %s', T(), text)) end
local function surf(x, z) return GetSurfaceHeight(x, z) end
local function make(bp, x, z, up, heading)
    local u = CreateUnitHPR(bp, A1, x, surf(x, z) + (up or 0), z, 0, heading or 0, 0)
    pcall(function() u:SetFireState(1) end)
    return u
end
local function pos(x, z) return { x, surf(x, z), z } end
local function hdist(a, b)
    local dx, dz = a[1] - b[1], a[3] - b[3]
    return math.sqrt(dx * dx + dz * dz)
end
local function try(tag, f)
    local ok, err = pcall(f)
    if not ok then P(tag .. ' ERR ' .. tostring(err)) end
end
local STATES = { 'Immobile', 'Moving', 'MovingUp', 'MovingDown', 'TransportLoading', 'TransportUnloading', 'WaitingForTransport',
    'Attached', 'Guarding', 'Busy', 'Refueling', 'Diving', 'Surfacing' }
local function states(u)
    local on = {}
    for _, s in STATES do
        local ok, v = pcall(function() return u:IsUnitState(s) end)
        if ok and v then table.insert(on, s) end
    end
    return table.concat(on, '+')
end

-- per-tick jobs, run by one thread
local jobs = {}
local function every(f) table.insert(jobs, f) end

ForkThread(function()
while GetGameTick() < t0 do WaitTicks(1) end
P('start abs ' .. GetGameTick())

-- GM
try('GM', function()
    local lanes = {}
    local i = 0
    for _, bp in { 'uea0203', 'ura0203' } do
        for _, kind in { 'S', 'A' } do
            for _, m in { 0.25, 0.5, 0.75, 1 } do
                if not (kind == 'A' and m == 1) then
                    local z = 40 + i * 20
                    i = i + 1
                    local u = make(bp, 60, z, 20, math.pi / 2)
                    if kind == 'S' then u:SetSpeedMult(m) else u:SetAccMult(m) end
                    IssueMove({ u }, pos(360, z))
                    table.insert(lanes, { tag = fmt('%s-%s%.2f', bp, kind, m), u = u })
                end
            end
        end
    end
    every(function(t)
        if math.mod(t, 5) ~= 0 or t > 330 then return end
        for _, l in lanes do
            local p = l.u:GetPosition()
            local v = l.last and hdist(p, l.last) * 2 or 0
            l.last = { p[1], p[2], p[3] }
            P(fmt('GM %s x %.2f z %.2f h %.2f v5 %.2f', l.tag, p[1], p[3], p[2] - surf(p[1], p[3]), v))
        end
    end)
end)

-- GP
try('GP', function()
    local list = {}
    local function patrol(tag, bp, x, z, leg)
        local u = make(bp, x, z, 0, 0)
        IssuePatrol({ u }, pos(x - leg, z))
        IssuePatrol({ u }, pos(x + leg, z))
        table.insert(list, { tag = tag, u = u, x = x })
    end
    patrol('uea0203-6', 'uea0203', 150, 380, 6)
    patrol('uea0203-30', 'uea0203', 150, 420, 30)
    patrol('ura0203-30', 'ura0203', 150, 450, 30)
    every(function(t)
        if math.mod(t, 5) ~= 0 or t > 250 then return end
        for _, l in list do
            local p = l.u:GetPosition()
            local v = l.last and hdist(p, l.last) * 2 or 0
            l.last = { p[1], p[2], p[3] }
            P(fmt('GP %s dx %.2f z %.2f h %.2f v5 %.2f', l.tag, p[1] - l.x, p[3], p[2] - surf(p[1], p[3]), v))
        end
    end)
end)

-- GD
try('GD', function()
    local pairs_ = {}
    local D = { 4, 10, 20, 35 }
    local function group(name, gbp, guardbp, x, z0, dz, movable, setup)
        for i, d in D do
            local z = z0 + (i - 1) * dz
            local g = make(gbp, x, z, 0, 0)
            if setup then setup(g) end
            local up = (guardbp == 'uea0203') and 0 or 0
            local w = make(guardbp, x - d, z, up, math.pi / 2)
            IssueGuard({ w }, g)
            table.insert(pairs_, { tag = name .. d, g = g, w = w, movable = movable, goal = pos(x + 60, z) })
        end
    end
    group('land', 'uel0201', 'uel0201', 620, 64, 16, true)
    group('air', 'uel0201', 'uea0203', 720, 200, 15, true)
    group('eng', 'ueb1101', 'uel0105', 250, 770, 20, false)
    group('naval', 'ues0103', 'ues0103', 650, 640, 30, true)
    every(function(t)
        if t == 100 then
            for _, p in pairs_ do
                if p.movable then IssueMove({ p.g }, p.goal) else p.g:SetHealth(nil, 300) end
            end
        end
        if math.mod(t, 10) ~= 0 or t > 400 then return end
        for _, p in pairs_ do
            if not p.w:BeenDestroyed() and not p.g:BeenDestroyed() then
                local a, b = p.w:GetPosition(), p.g:GetPosition()
                local vx, vy, vz = p.w:GetVelocity()
                P(fmt('GD %s d %.2f v %.2f wx %.2f wz %.2f gx %.2f [%s] hp %.0f', p.tag, hdist(a, b), 10 * math.sqrt(vx * vx + vz * vz), a[1], a[3], b[1], states(p.w), p.g:GetHealth()))
            end
        end
    end)
end)

-- FL
try('FL', function()
    local list = {}
    local function plane(tag, bp, x, z)
        local u = make(bp, x, z, 20, math.pi / 2)
        IssueMove({ u }, pos(x + 300, z))
        IssueMove({ u }, pos(x + 300, z + 330))
        table.insert(list, { tag = tag, u = u })
    end
    plane('uea0102', 'uea0102', 480, 560)
    plane('uea0103', 'uea0103', 480, 600)
    every(function(t)
        if t > 420 then return end
        for _, l in list do
            local p = l.u:GetPosition()
            local vx, vy, vz = l.u:GetVelocity()
            local hdg = math.deg(math.atan2(vx, vz))
            local dh = l.hdg and (hdg - l.hdg) or 0
            if dh > 180 then dh = dh - 360 elseif dh < -180 then dh = dh + 360 end
            l.hdg = hdg
            P(fmt('FL %s x %.2f z %.2f h %.2f vh %.3f vy %.3f hdg %.2f dhdg %.3f', l.tag, p[1], p[3], p[2] - surf(p[1], p[3]), 10 * math.sqrt(vx * vx + vz * vz), 10 * vy, hdg, dh))
        end
    end)
end)

-- MS
try('MS', function()
    local list = {}
    local function watch(tag, u) table.insert(list, { tag = tag, u = u, last = '' }) return u end
    local s1 = watch('sub-ues0203', make('ues0203', 520, 760, 0, 0))
    local s2 = watch('sub-urs0203', make('urs0203', 520, 800, 0, 0))
    local pg = watch('plane-ground', CreateUnitHPR('uea0102', A1, 680, GetTerrainHeight(680, 272), 272, 0, 0, 0))
    local gg = watch('gunship-ground', CreateUnitHPR('uea0203', A1, 700, GetTerrainHeight(700, 272), 272, 0, 0, 0))
    local tr = watch('transport', make('uea0107', 870, 400, 0, 0))
    local c1 = watch('cargo1', make('uel0201', 876, 400, 0, 0))
    local c2 = watch('cargo2', make('uel0201', 876, 404, 0, 0))
    local st = make('ueb5202', 660, 336, 0, 0)
    local pa = watch('plane-staging', make('uea0102', 690, 300, 20, 0))
    local gs = watch('gunship-staging', make('uea0203', 700, 340, 20, 0))
    every(function(t)
        if t == 5 then
            try('MS dive', function() IssueDive({ s1, s2 }) end)
            IssueMove({ pg }, pos(680, 200)) IssueMove({ gg }, pos(700, 200))
            IssueTransportLoad({ c1, c2 }, tr)
        end
        if t == 20 then try('MS staging', function() IssueTransportLoad({ pa, gs }, st) end) end
        if t == 120 then try('MS surface', function() IssueDive({ s1, s2 }) end) end
        if t == 160 then IssueMove({ pg }, pos(680, 300)) end
        if not tr.unl and table.getn(tr:GetCargo()) == 2 then
            tr.unl = true
            P('MS transport loaded')
            IssueTransportUnload({ tr }, pos(910, 400))
        end
        if t == 260 then try('MS unstage', function() IssueTransportUnload({ st }, pos(660, 300)) end) end
        if t > 400 then return end
        for _, l in list do
            local u = l.u
            if not u:BeenDestroyed() then
                local s = states(u) .. ' ' .. tostring(u:GetCurrentLayer())
                local p = u:GetPosition()
                if s ~= l.last then
                    l.last = s
                    P(fmt('MS %s state [%s] h %.2f', l.tag, s, p[2] - surf(p[1], p[3])))
                end
                if math.mod(t, 5) == 0 then
                    local vx, vy, vz = u:GetVelocity()
                    P(fmt('MSH %s h %.2f vy %.3f', l.tag, p[2] - surf(p[1], p[3]), 10 * vy))
                end
            end
        end
    end)
end)

ForkThread(function()
    local start = T()
    while true do
        local t = T()
        for _, f in jobs do
            local ok, err = pcall(f, t)
            if not ok then P('job ERR ' .. tostring(err)) end
        end
        if t >= 430 then break end
        WaitTicks(1)
    end
    WaitTicks(30)
    LOG('PROBE done')
end)
end)
