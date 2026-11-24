-- Differential probe (open question): what triggers weapon OnStartTracking /
-- OnStopTracking. A T1 PD (ueb2101) and a T1 tank (uel0201) each get one enemy
-- tank (hold fire) at 12 in front (+z), behind (-z) or beside (+x), created at T+0 and
-- destroyed at T+40. Per tick: has-target and the aim controller's heading.
--   real game: run_faf.ps1 probe_q_track 90
--   ours: --headless --no-ai --ticks 120 --script dev/probes/maps/probe_q_track/probe.lua
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
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local LAND = { {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232} }
local lanes = {}
local specs = { { 'pd', 'ueb2101' }, { 'tank', 'uel0201' } }
local offs = { { 'front', 0, 12 }, { 'behind', 0, -12 }, { 'side', 12, 0 } }
local i = 0
for _, s in specs do
    for _, o in offs do
        i = i + 1
        local x, z = LAND[i][1], LAND[i][2]
        local u = make(s[2], A1, x, z)
        u:SetMaxHealth(100000) u:SetHealth(nil, 100000)
        local name = s[1] .. '-' .. o[1]
        local w = u:GetWeapon(1)
        w.OnStartTracking = function(self, label)
            P(fmt('%s OnStartTracking %s hasTarget %s', name, tostring(label), tostring(self:WeaponHasTarget())))
            self.OnStartTracking = nil
            self:OnStartTracking(label)
            self.OnStartTracking = w.OnStartTracking_h
        end
        w.OnStartTracking_h = w.OnStartTracking
        w.OnStopTracking = function(self, label)
            P(fmt('%s OnStopTracking %s hasTarget %s', name, tostring(label), tostring(self:WeaponHasTarget())))
            self.OnStopTracking = nil
            self:OnStopTracking(label)
            self.OnStopTracking = w.OnStopTracking_h
        end
        w.OnStopTracking_h = w.OnStopTracking
        table.insert(lanes, { name = name, u = u, w = w, x = x, z = z, o = o })
    end
end
local function aim(l)
    local ac = l.w.AimControl
    if not ac then return 'noaim' end
    local ok, h, p = pcall(function() return ac:GetHeadingPitch() end)
    if not ok then return 'err' end
    return fmt('%.1f', math.deg(h or 0))
end
ForkThread(function()
    WaitTicks(10)
    T = GetGameTick()
    for _, l in lanes do
        local tx, tz = l.x + l.o[2], l.z + l.o[3]
        l.t = make('uel0201', A2, tx, tz) l.t:SetFireState(1)
        l.t:SetMaxHealth(1000000) l.t:SetHealth(nil, 1000000)
        local hits = 0
        l.t.OnDamage = function() hits = hits + 1 if hits == 1 then P(l.name .. ' first hit') end end
        l.last = ''
    end
    for t = 0, 60 do
        for _, l in lanes do
            local s = fmt('has %s aim %s', tostring(l.w:WeaponHasTarget()), aim(l))
            if s ~= l.last then P(l.name .. ' ' .. s) l.last = s end
        end
        if t == 40 then
            for _, l in lanes do l.t:Destroy() end
            P('targets destroyed')
        end
        WaitTicks(1)
    end
    P('done')
end)
