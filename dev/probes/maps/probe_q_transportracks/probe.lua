-- Differential probe: which way a UEF T3 transport (xea0306) circles a target it is
-- ordered to attack, and which of its missile racks fire. Its left racks
-- (HeadingArcCenter 90) cover one half, its right racks (270) the other.
--   L the target (a hovering gunship, holding fire) 30 to one side, 5 ahead
--   R the same on the other side
--   A the target 40 dead ahead
--   B the target 40 behind
-- Each lane's attacker and target are a pair of armies; every other pair is neutral.
-- Logged: every 5 ticks the distance, the transport's heading and the target's bearing
-- off its nose (degrees, signed); every 25 ticks each rack's OnFire count; each rack's
-- first OnFire.
--   real game: run_faf.ps1 probe_q_transportracks 240
--   ours: --headless --no-ai --ticks 400 --script dev/probes/maps/probe_q_transportracks/probe.lua
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
    return fmt('d %.1f h %.0f bearing %.0f at %.0f,%.0f,%.0f', math.sqrt(dx * dx + dz * dz), h,
        wrap(math.atan2(dx, dz) * 180 / math.pi - h), x, y, z)
end

local order = {}
-- the transport faces +x; the target is `ahead` along +x and `side` along +z of it
local function lane(name, n, x, z, ahead, side)
    local a, b = pair(n)
    local u = make('xea0306', a, x, z, math.pi / 2)
    local l = { name = name, u = u, ws = {} }
    l.target = make('uea0203', b, x + ahead, z + side, 0)
    for i = 1, u:GetWeaponCount() do
        local w = u:GetWeapon(i)
        local label = w:GetBlueprint().Label
        if string.find(label, 'MissleRack') then
            local rec = { label = label, fires = 0 }
            table.insert(l.ws, rec)
            w.OnFire = function(self)
                rec.fires = rec.fires + 1
                if rec.fires == 1 then P(fmt('%s %s first OnFire %s', name, label, where(l))) end
                self.OnFire = nil
                self:OnFire()
                self.OnFire = rec.fire
            end
            rec.fire = w.OnFire
        end
    end
    table.insert(order, l)
    return l
end

lane('L', 1, 300, 250, 5, 30)
lane('R', 2, 300, 450, 5, -30)
lane('A', 3, 300, 650, 40, 0)
lane('B', 4, 300, 850, -40, 0)

ForkThread(function()
    WaitTicks(50)
    for _, l in order do P(fmt('%s before %s', l.name, where(l))) end
    T = GetGameTick()
    P('orders')
    for _, l in order do
        IssueAttack({ l.u }, l.target)
        l.u:SetFireState(0)
    end
    for t = 0, 300 do
        if math.mod(t, 5) == 0 then
            for _, l in order do P(fmt('%s %s', l.name, where(l))) end
        end
        if math.mod(t, 25) == 0 then
            for _, l in order do
                local s = ''
                for _, r in l.ws do s = s .. fmt(' %s %d', r.label, r.fires) end
                P(fmt('%s fires%s', l.name, s))
            end
        end
        WaitTicks(1)
    end
    LOG('PROBE done')
    if EndGame then EndGame() end
end)
