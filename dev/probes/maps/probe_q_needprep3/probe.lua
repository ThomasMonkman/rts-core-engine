-- Differential probe, follow-up to probe_q_needprep2: does an attack order that has
-- to close in reset the weapons' current targets at the order or on arrival?
--   K ues0201 shooting a ship 30 behind it, ordered to attack one 140 ahead
--   L xes0307 (NeedPrep Hiro cannons) the same
--   real game: run_faf.ps1 probe_q_needprep3 240
--   ours: --headless --no-ai --ticks 330 --script dev/probes/maps/probe_q_needprep3/probe.lua
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
local T = nil
local function P(s) LOG(fmt('PROBE %s %s', T and ('T+' .. (GetGameTick() - T)) or 'pre', s)) end
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetSurfaceHeight(x, z), z, 0, math.pi / 2, 0)
    P(fmt('spawn %s %d,%d depth %.1f', bp, x, z, GetSurfaceHeight(x, z) - GetTerrainHeight(x, z)))
    return u
end
local tags = {}
local function enemy(x, z, tag)
    local t = make('ues0103', A2, x, z)
    t:SetFireState(1)
    t:SetMaxHealth(1000000) t:SetHealth(nil, 1000000)
    tags[t:GetEntityId()] = tag
    return t
end

local function dist(l)
    local px, pz
    if l.target then local p = l.target:GetPosition() px, pz = p[1], p[3] end
    if l.ground then px, pz = l.ground[1], l.ground[3] end
    if not px then return -1 end
    local x, _, z = l.u:GetPositionXYZ()
    return math.sqrt((px - x) * (px - x) + (pz - z) * (pz - z))
end

local order = {}
local function lane(name, bp, x, z)
    local u = make(bp, A1, x, z)
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    local l = { name = name, u = u, ws = {} }
    for i = 1, u:GetWeaponCount() do
        local w = u:GetWeapon(i)
        local label = w:GetBlueprint().Label
        local who = name .. ' ' .. label
        local fires = 0
        local rec = { w = w, who = who, last = '' }
        table.insert(l.ws, rec)
        w.OnGotTarget = function(self)
            P(fmt('%s OnGotTarget has %s d %.1f', who, tostring(self:WeaponHasTarget()), dist(l)))
            self.OnGotTarget = nil
            self:OnGotTarget()
            self.OnGotTarget = rec.got
        end
        rec.got = w.OnGotTarget
        w.OnLostTarget = function(self)
            P(fmt('%s OnLostTarget has %s d %.1f', who, tostring(self:WeaponHasTarget()), dist(l)))
            self.OnLostTarget = nil
            self:OnLostTarget()
            self.OnLostTarget = rec.lost
        end
        rec.lost = w.OnLostTarget
        w.OnFire = function(self)
            fires = fires + 1
            if fires <= 1 then P(fmt('%s OnFire #%d d %.1f', who, fires, dist(l))) end
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

local lk = lane('K', 'ues0201', 545, 640) enemy(515, 640, 'K-near') lk.target = enemy(685, 640, 'K-far')
local ll = lane('L', 'xes0307', 545, 840) enemy(515, 840, 'L-near') ll.target = enemy(685, 840, 'L-far')

local function watch()
    for _, l in order do
        for _, r in l.ws do
            local s = tgtdesc(r.w)
            if s ~= r.last then P(fmt('%s target %s d %.1f', r.who, s, dist(l))) r.last = s end
        end
        local mv = l.u:IsUnitState('Moving') and 'moving' or 'still'
        if mv ~= l.mv then P(fmt('%s %s d %.1f', l.name, mv, dist(l))) l.mv = mv end
    end
end

ForkThread(function()
    for i = 1, 30 do watch() WaitTicks(1) end
    T = GetGameTick()
    P('orders')
    IssueAttack({ lk.u }, lk.target)
    IssueAttack({ ll.u }, ll.target)
    for t = 0, 280 do
        watch()
        if math.mod(t, 20) == 0 then
            local s = ''
            for _, l in order do s = s .. fmt(' %s %.1f', l.name, dist(l)) end
            P('dist' .. s)
        end
        WaitTicks(1)
    end
    LOG('PROBE done')
    if EndGame then EndGame() end
end)
