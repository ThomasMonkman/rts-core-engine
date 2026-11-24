-- Differential probe (open questions): (1) Fatboy (uel0401) ordered to attack
-- a stationary enemy tank 50 away at bearing 0 / +90 / -90 from the nose:
-- which main turrets fire, and where the body stops turning. (2) Aeon MML
-- (ual0111, slaved, not turreted, FiringTolerance 5) immobile, target 35 away
-- at 10 / 30 / 40 / 50 degrees: which fire.
--   real game: run_faf.ps1 probe_q_fatboy 120
--   ours: --headless --no-ai --ticks 300 --script dev/probes/maps/probe_q_fatboy/probe.lua
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
local T
local function P(s) LOG(fmt('PROBE %s %s', T and ('T+' .. (GetGameTick() - T)) or 'pre', s)) end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local function heading(u)
    local ok, h = pcall(function() return u:GetHeading() end)
    return ok and h and math.deg(h) or 0
end
local LABELS = { 'RightTurret01', 'RightTurret02', 'LeftTurret01', 'LeftTurret02' }
local lanes = {}
local specs = {
    { 'fat-0', 'uel0401', 0, 50, false, {720,64} },
    { 'fat+90', 'uel0401', 90, 50, false, {920,96} },
    { 'fat-90', 'uel0401', -90, 50, false, {776,236} },
    { 'mml-10-imm', 'ual0111', 10, 35, true, {664,330} },
    { 'mml-30-imm', 'ual0111', 30, 35, true, {904,384} },
    { 'mml-40-imm', 'ual0111', 40, 35, true, {568,460} },
    { 'mml-50-imm', 'ual0111', 50, 35, true, {456,552} },
}
for _, s in specs do
    local x, z = s[6][1], s[6][2]
    local u = make(s[2], A1, x, z)
    u:SetMaxHealth(10000000) u:SetHealth(nil, 10000000)
    u:SetIntelRadius('Vision', 80)
    if s[5] then u:SetImmobile(true) end
    local l = { tag = s[1], u = u, x = x, z = z, b = s[3], d = s[4], fat = s[2] == 'uel0401', counts = {} }
    if l.fat then
        for _, lab in LABELS do
            local w = u:GetWeaponByLabel(lab)
            local name = lab
            l.counts[name] = 0
            w.CreateProjectileAtMuzzle = function(self, m)
                l.counts[name] = l.counts[name] + 1
                if l.counts[name] == 1 then P(fmt('%s %s first shot heading %.1f', l.tag, name, heading(u))) end
                self.CreateProjectileAtMuzzle = nil
                local pr = self:CreateProjectileAtMuzzle(m)
                self.CreateProjectileAtMuzzle = w.hk
                return pr
            end
            w.hk = w.CreateProjectileAtMuzzle
        end
    end
    table.insert(lanes, l)
end
ForkThread(function()
    WaitTicks(10)
    T = GetGameTick()
    for _, lane in lanes do
        local l = lane
        local r = math.rad(l.b)
        local tx, tz = l.x + math.sin(r) * l.d, l.z + math.cos(r) * l.d
        local t = make('uel0201', A2, tx, tz)
        t:SetFireState(1)
        t:SetMaxHealth(10000000) t:SetHealth(nil, 10000000)
        l.t = t l.tx = tx l.tz = tz l.hits = 0
        t.OnDamage = function(self, inst, amt)
            l.hits = l.hits + 1
            if l.hits == 1 then P(fmt('%s first hit amt %.0f', l.tag, amt)) end
        end
        if l.fat then IssueAttack({ l.u }, t) end
        P(fmt('%s start heading %.1f', l.tag, heading(l.u)))
        l.lh = heading(l.u)
    end
    for k = 1, 250 do
        WaitTicks(1)
        for _, l in lanes do
            local h = heading(l.u)
            if math.abs(h - l.lh) > 10 then P(fmt('%s heading %.1f', l.tag, h)) l.lh = h end
            if math.mod(k, 50) == 0 then
                local p = l.u:GetPosition()
                local bear = math.deg(math.atan2(l.tx - p[1], l.tz - p[3])) - h
                while bear > 180 do bear = bear - 360 end
                while bear < -180 do bear = bear + 360 end
                local s = ''
                for _, lab in LABELS do if l.counts[lab] then s = s .. ' ' .. lab .. '=' .. l.counts[lab] end end
                P(fmt('%s at %d heading %.1f target-off-nose %.1f hits %d%s', l.tag, k, h, bear, l.hits, s))
            end
        end
    end
    P('done')
end)
