-- Differential probe, follow-up to probe_q_transportracks: there a UEF T3 transport
-- (xea0306) ordered onto a hovering gunship 30 to its side lifted off, never moved
-- sideways and landed again, where one 40 ahead was flown to and circled. Is the order
-- dropped, and is it the distance or the side that decides?
--   S the target 30 to one side, 5 ahead (the earlier lane R)
--   T the target 30 to the other side, 5 ahead (the earlier lane L)
--   F the target 30 dead ahead
--   W the target 40 to the side, 5 ahead
-- Each lane's attacker and target are a pair of armies; every other pair is neutral.
-- Logged on change, every tick: the command queue's length and first command type,
-- the Attacking / Moving unit states, idle, the layer, each rack's target. Every 5
-- ticks: distance, heading, the target's bearing off the nose, position. Each rack's
-- OnGotTarget / OnLostTarget, its first OnFire, and its OnFire count every 25 ticks.
--   real game: run_faf.ps1 probe_q_transportracks2 240
--   ours: --headless --no-ai --ticks 400 --script dev/probes/maps/probe_q_transportracks2/probe.lua
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
for i = 1, 8 do
    for j = 1, 8 do
        if i ~= j then SetAlliance(armies[i], armies[j], 'Neutral') end
    end
end
local function pair(n)
    local a, b = armies[n * 2 - 1], armies[n * 2]
    SetAlliance(a, b, 'Enemy') SetAlliance(b, a, 'Enemy')
    return a, b
end
local T = nil
local function P(s) LOG(fmt('PROBE %s %s', T and ('T+' .. (GetGameTick() - T)) or 'pre', s)) end
local function make(bp, army, x, z, heading)
    local y = math.max(GetSurfaceHeight(x, z), GetTerrainHeight(x, z))
    local u = CreateUnitHPR(bp, army, x, y, z, 0, heading, 0)
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    u:SetFireState(1)
    return u
end
local function wrap(d)
    while d > 180 do d = d - 360 end
    while d < -180 do d = d + 360 end
    return d
end
local function where(l)
    local p = l.target:GetPosition()
    local x, y, z = l.u:GetPositionXYZ()
    local dx, dz = p[1] - x, p[3] - z
    local h = l.u:GetHeading() * 180 / math.pi
    return fmt('d %.1f h %.0f bearing %.0f at %.0f,%.0f,%.0f tgt y %.0f', math.sqrt(dx * dx + dz * dz), h,
        wrap(math.atan2(dx, dz) * 180 / math.pi - h), x, y, z, p[2])
end
local function try(f, a, b)
    local ok, v = pcall(f, a, b)
    if ok then return v end
    return 'err'
end
local function state(l)
    local u = l.u
    local q = try(u.GetCommandQueue, u)
    local qs = 'err'
    if type(q) == 'table' then
        qs = tostring(table.getn(q))
        if q[1] then qs = qs .. ':' .. tostring(q[1].commandType) end
    end
    local s = fmt('queue %s attacking %s moving %s idle %s layer %s', qs, tostring(try(u.IsUnitState, u, 'Attacking')),
        tostring(try(u.IsUnitState, u, 'Moving')), tostring(try(u.IsIdleState, u)), tostring(try(u.GetCurrentLayer, u)))
    for _, r in l.ws do
        local t = 'none'
        if r.w:WeaponHasTarget() then
            local e = r.w:GetCurrentTarget()
            t = e and (e == l.target and 'gunship' or 'other') or 'ground'
        end
        s = s .. ' ' .. r.short .. '=' .. t
    end
    return s
end

local order = {}
-- the transport faces +x; the target is `ahead` along +x and `side` along +z of it
local function lane(name, n, x, z, ahead, side)
    local a, b = pair(n)
    local u = make('xea0306', a, x, z, math.pi / 2)
    local l = { name = name, u = u, ws = {}, last = '' }
    l.target = make('uea0203', b, x + ahead, z + side, 0)
    for i = 1, u:GetWeaponCount() do
        local w = u:GetWeapon(i)
        local label = w:GetBlueprint().Label
        if string.find(label, 'MissleRack') then
            local short = string.gsub(label, 'MissleRack', '')
            local rec = { w = w, label = label, short = short, fires = 0 }
            table.insert(l.ws, rec)
            w.OnFire = function(self)
                rec.fires = rec.fires + 1
                if rec.fires == 1 then P(fmt('%s %s first OnFire %s', name, short, where(l))) end
                self.OnFire = nil
                self:OnFire()
                self.OnFire = rec.fire
            end
            rec.fire = w.OnFire
            w.OnGotTarget = function(self)
                P(fmt('%s %s OnGotTarget %s', name, short, where(l)))
                self.OnGotTarget = nil
                self:OnGotTarget()
                self.OnGotTarget = rec.got
            end
            rec.got = w.OnGotTarget
            w.OnLostTarget = function(self)
                P(fmt('%s %s OnLostTarget %s', name, short, where(l)))
                self.OnLostTarget = nil
                self:OnLostTarget()
                self.OnLostTarget = rec.lost
            end
            rec.lost = w.OnLostTarget
        end
    end
    table.insert(order, l)
    return l
end

lane('S', 1, 300, 250, 5, 30)
lane('T', 2, 300, 450, 5, -30)
lane('F', 3, 300, 650, 30, 0)
lane('W', 4, 300, 850, 5, 40)

local function watch()
    for _, l in order do
        local s = state(l)
        if s ~= l.last then P(fmt('%s %s', l.name, s)) l.last = s end
    end
end

ForkThread(function()
    WaitTicks(50)
    for _, l in order do P(fmt('%s before %s', l.name, where(l))) end
    watch()
    T = GetGameTick()
    P('orders')
    for _, l in order do
        IssueAttack({ l.u }, l.target)
        l.u:SetFireState(0)
    end
    for t = 0, 300 do
        watch()
        if math.mod(t, 5) == 0 then
            for _, l in order do P(fmt('%s %s', l.name, where(l))) end
        end
        if math.mod(t, 25) == 0 then
            for _, l in order do
                local s = ''
                for _, r in l.ws do s = s .. fmt(' %s %d', r.short, r.fires) end
                P(fmt('%s fires%s', l.name, s))
            end
        end
        WaitTicks(1)
    end
    LOG('PROBE done')
    if EndGame then EndGame() end
end)
