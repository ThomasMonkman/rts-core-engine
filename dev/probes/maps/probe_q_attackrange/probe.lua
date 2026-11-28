-- Differential probe, follow-up to probe_q_needprep3: which range decides that an attack
-- order's target is "in range" (the tick its weapons are reset and take it) when the
-- target is a structure on the shore, and whether a twin-turret ship then turns its
-- broadside and fires both turrets.
--   G xes0307 (NeedPrep Hiro cannons, 80: the prep marks the tick) onto a T3 power
--     generator (footprint 5x5) on the shore, 125 away
--   J ues0201 (60) onto a T1 power generator on the shore, 115 away
--   C ues0201 onto a T3 power generator on the shore, 135 away
--   P ues0201 holding fire, with no target, ordered onto a ship 42 away (in range)
--   Q ues0201 that never saw its target (far from P), fire state normal, the same
-- Each lane's attacker and target are a pair of armies; every other pair is neutral,
-- so lanes can't shoot each other's targets.
-- Logged: every OnGotTarget / OnLostTarget (WeaponHasTarget, the distance between the
-- centres, the ship's heading), each weapon's first two OnFires, per-tick target
-- changes, moving / still, and the distance and heading every 20 ticks.
--   real game: run_faf.ps1 probe_q_attackrange 240
--   ours: --headless --no-ai --ticks 380 --script dev/probes/maps/probe_q_attackrange/probe.lua
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
    P(fmt('spawn %s %d,%d depth %.1f', bp, x, z, GetSurfaceHeight(x, z) - GetTerrainHeight(x, z)))
    return u
end
local tags = {}
local function target(bp, army, x, z, tag)
    local t = make(bp, army, x, z, 0)
    t:SetFireState(1)
    tags[t:GetEntityId()] = tag
    return t
end

local function dist(l)
    local p = l.target:GetPosition()
    local x, _, z = l.u:GetPositionXYZ()
    return math.sqrt((p[1] - x) * (p[1] - x) + (p[3] - z) * (p[3] - z))
end
local function where(l)
    return fmt('d %.1f h %.0f', dist(l), l.u:GetHeading() * 180 / math.pi)
end

local order = {}
local function lane(name, bp, army, x, z)
    -- facing -x, towards the shore
    local u = make(bp, army, x, z, -math.pi / 2)
    local l = { name = name, u = u, ws = {} }
    for i = 1, u:GetWeaponCount() do
        local w = u:GetWeapon(i)
        local label = w:GetBlueprint().Label
        local who = name .. ' ' .. label
        local fires = 0
        local rec = { w = w, who = who, last = '' }
        table.insert(l.ws, rec)
        w.OnGotTarget = function(self)
            P(fmt('%s OnGotTarget has %s %s', who, tostring(self:WeaponHasTarget()), where(l)))
            self.OnGotTarget = nil
            self:OnGotTarget()
            self.OnGotTarget = rec.got
        end
        rec.got = w.OnGotTarget
        w.OnLostTarget = function(self)
            P(fmt('%s OnLostTarget has %s %s', who, tostring(self:WeaponHasTarget()), where(l)))
            self.OnLostTarget = nil
            self:OnLostTarget()
            self.OnLostTarget = rec.lost
        end
        rec.lost = w.OnLostTarget
        w.OnFire = function(self)
            fires = fires + 1
            if fires <= 2 then P(fmt('%s OnFire #%d %s', who, fires, where(l))) end
            self.OnFire = nil
            self:OnFire()
            self.OnFire = rec.fire
        end
        rec.fire = w.OnFire
    end
    table.insert(order, l)
    return l
end

local tgtdesc = function(w)
    if not w:WeaponHasTarget() then return 'none' end
    local e = w:GetCurrentTarget()
    if e then return tags[e:GetEntityId()] or ('id' .. e:GetEntityId()) end
    local p = w:GetCurrentTargetPos()
    if p then return fmt('ground %.0f,%.0f', p[1], p[3]) end
    return 'other'
end

local a, b = pair(1)
local lg = lane('G', 'xes0307', a, 545, 650) lg.target = target('ueb1301', b, 420, 650, 'G-pgen3')
a, b = pair(2)
local lj = lane('J', 'ues0201', a, 545, 600) lj.target = target('ueb1101', b, 430, 600, 'J-pgen1')
a, b = pair(3)
local lc = lane('C', 'ues0201', a, 545, 700) lc.target = target('ueb1301', b, 408, 700, 'C-pgen3')
a, b = pair(4)
local lp = lane('P', 'ues0201', a, 740, 780) lp.target = target('ues0103', b, 770, 810, 'P-ship')
lp.u:SetFireState(1)
-- the fire state back to normal just before the order: nothing held, nothing seen yet
local lq = lane('Q', 'ues0201', a, 740, 900) lq.target = target('ues0103', b, 770, 930, 'Q-ship')
lq.u:SetFireState(1)

local function watch()
    for _, l in order do
        for _, r in l.ws do
            local s = tgtdesc(r.w)
            if s ~= r.last then P(fmt('%s target %s %s', r.who, s, where(l))) r.last = s end
        end
        local mv = l.u:IsUnitState('Moving') and 'moving' or 'still'
        if mv ~= l.mv then P(fmt('%s %s %s', l.name, mv, where(l))) l.mv = mv end
    end
end

ForkThread(function()
    for i = 1, 30 do watch() WaitTicks(1) end
    T = GetGameTick()
    P('orders')
    lq.u:SetFireState(0)
    for _, l in order do IssueAttack({ l.u }, l.target) end
    for t = 0, 330 do
        watch()
        if math.mod(t, 20) == 0 then
            local s = ''
            for _, l in order do s = s .. fmt(' %s %.1f/%.0f', l.name, dist(l), l.u:GetHeading() * 180 / math.pi) end
            P('dist' .. s)
        end
        WaitTicks(1)
    end
    LOG('PROBE done')
    if EndGame then EndGame() end
end)
