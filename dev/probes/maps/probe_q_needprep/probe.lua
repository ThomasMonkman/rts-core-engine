-- Differential probe (open question): does an attack order run a NeedPrep weapon's
-- OnGotTarget at once (SetDesiredTarget, MohoEngine.dll 0x101c48f1), out of range and
-- with the weapon's own target still empty, and again once the target is in range?
-- NeedPrep: Seraphim destroyer xss0201 (FrontTurret / BackTurret). Control without it:
-- UEF destroyer ues0201. Lanes in Seton's deep water, ordered at T+0:
--   A xss0201 attacks an enemy ship 110 away (range 60: it has to sail)
--   B xss0201 attacks the ground 110 away
--   C ues0201 attacks an enemy ship 110 away
--   D ues0201 attacks the ground 110 away
--   E xss0201 already shooting a ship 35 away, ordered to attack another 42 away
--   F ues0201 the same
-- Logged: every OnGotTarget / OnLostTarget (with WeaponHasTarget), the first OnFires,
-- per-tick changes of each weapon's target, and the tick the order's target comes in range.
--   real game: run_faf.ps1 probe_q_needprep 200
--   ours: --headless --no-ai --ticks 300 --script dev/probes/maps/probe_q_needprep/probe.lua
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

local lanes = {}
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
        local got, lost, fire = w.OnGotTarget, w.OnLostTarget, w.OnFire
        w.OnGotTarget = function(self)
            P(fmt('%s OnGotTarget has %s', who, tostring(self:WeaponHasTarget())))
            self.OnGotTarget = nil
            self:OnGotTarget()
            self.OnGotTarget = rec.got
        end
        rec.got = w.OnGotTarget
        w.OnLostTarget = function(self)
            P(fmt('%s OnLostTarget has %s', who, tostring(self:WeaponHasTarget())))
            self.OnLostTarget = nil
            self:OnLostTarget()
            self.OnLostTarget = rec.lost
        end
        rec.lost = w.OnLostTarget
        w.OnFire = function(self)
            fires = fires + 1
            if fires <= 2 then P(fmt('%s OnFire #%d', who, fires)) end
            self.OnFire = nil
            self:OnFire()
            self.OnFire = rec.fire
        end
        rec.fire = w.OnFire
    end
    lanes[name] = l
    return l
end

local function tgtdesc(w)
    if not w:WeaponHasTarget() then return 'none' end
    local e = w:GetCurrentTarget()
    if e then return tags[e:GetEntityId()] or ('id' .. e:GetEntityId()) end
    local p = w:GetCurrentTargetPos()
    if p then return fmt('ground %.0f,%.0f', p[1], p[3]) end
    return 'other'
end

-- lanes
local la = lane('A', 'xss0201', 540, 620) la.target = enemy(650, 620, 'A-tgt')
local lb = lane('B', 'xss0201', 540, 705) lb.ground = { 650, GetSurfaceHeight(650, 705), 705 }
local lc = lane('C', 'ues0201', 540, 790) lc.target = enemy(650, 790, 'C-tgt')
local ld = lane('D', 'ues0201', 550, 875) ld.ground = { 660, GetSurfaceHeight(660, 875), 875 }
local le = lane('E', 'xss0201', 740, 640) le.first = enemy(775, 640, 'E-t1') le.target = enemy(770, 670, 'E-t2')
local lf = lane('F', 'ues0201', 740, 780) lf.first = enemy(775, 780, 'F-t1') lf.target = enemy(770, 810, 'F-t2')
local order = { la, lb, lc, ld, le, lf }

local function watch()
    for _, l in order do
        for _, r in l.ws do
            local s = tgtdesc(r.w)
            if s ~= r.last then P(r.who .. ' target ' .. s) r.last = s end
        end
        if not l.inrange and T then
            local px, pz
            if l.target then local p = l.target:GetPosition() px, pz = p[1], p[3] end
            if l.ground then px, pz = l.ground[1], l.ground[3] end
            local x, _, z = l.u:GetPositionXYZ()
            local d = math.sqrt((px - x) * (px - x) + (pz - z) * (pz - z))
            if d <= 60 then l.inrange = true P(fmt('%s order target in range 60 (d %.1f)', l.name, d)) end
        end
    end
end

ForkThread(function()
    for i = 1, 30 do watch() WaitTicks(1) end
    T = GetGameTick()
    P('orders')
    IssueAttack({ la.u }, la.target)
    IssueAttack({ lb.u }, lb.ground)
    IssueAttack({ lc.u }, lc.target)
    IssueAttack({ ld.u }, ld.ground)
    IssueAttack({ le.u }, le.target)
    IssueAttack({ lf.u }, lf.target)
    for t = 0, 180 do watch() WaitTicks(1) end
    LOG('PROBE done')
    if EndGame then EndGame() end
end)
