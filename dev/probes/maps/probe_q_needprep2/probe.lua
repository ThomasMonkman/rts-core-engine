-- Differential probe, follow-up to probe_q_needprep: when, on an attack order that
-- has to close in, does the attacker reset its weapons' targets and run a NeedPrep
-- weapon's OnGotTarget? Each lane logs its distance to the order's target on every
-- callback and when its Moving state changes. Lanes in Seton's deep water, 140 apart,
-- ordered at T+0:
--   A xss0201 (NeedPrep beams, range 60; torpedo 45) attacks an enemy ship
--   C ues0201 (no NeedPrep, guns 60, torpedo 45) attacks an enemy ship
--   G xes0307 (NeedPrep Hiro cannons, range 80; torpedo 60, phalanx 31) attacks an enemy ship
--   H xes0307 attacks the ground
--   real game: run_faf.ps1 probe_q_needprep2 240
--   ours: --headless --no-ai --ticks 330 --script dev/probes/maps/probe_q_needprep2/probe.lua
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

local la = lane('A', 'xss0201', 545, 620) la.target = enemy(685, 620, 'A-tgt')
local lc = lane('C', 'ues0201', 545, 720) lc.target = enemy(685, 720, 'C-tgt')
local lg = lane('G', 'xes0307', 545, 820) lg.target = enemy(685, 820, 'G-tgt')
local lh = lane('H', 'xes0307', 545, 920) lh.ground = { 685, GetSurfaceHeight(685, 920), 920 }

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
    IssueAttack({ la.u }, la.target)
    IssueAttack({ lc.u }, lc.target)
    IssueAttack({ lg.u }, lg.target)
    IssueAttack({ lh.u }, lh.ground)
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
