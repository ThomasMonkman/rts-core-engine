-- Differential probe (open question): SlavedToBody arc gate. Seraphim T3 tank
-- xsl0303 (Left/RightTurret SlavedToBodyArcRange 44, MainTurret 175, all
-- turreted, yaw range 180) and Aeon MML ual0111 (not turreted, slaved, arc
-- default). Each lane: one idle unit, one enemy tank target (hold fire) at a
-- bearing from the nose; "imm" lanes call SetImmobile(true) first.
-- Logs heading changes and each hit with its amount (57 side guns, 625 main).
--   real game: run_faf.ps1 probe_q_slaved 120
--   ours: --headless --no-ai --ticks 260 --script dev/probes/maps/probe_q_slaved/probe.lua
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
local LAND = {
    {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232},
    {664,320}, {848,320}, {904,384}, {656,416}, {568,440}, {456,552}, {368,568}, {120,608},
}
-- tag, bp, bearing (deg, clockwise from nose seen from above), distance, immobile
local specs = {
    { 'tank-0-imm', 'xsl0303', 0, 16, true },
    { 'tank-30-imm', 'xsl0303', 30, 16, true },
    { 'tank-60-imm', 'xsl0303', 60, 16, true },
    { 'tank-120-imm', 'xsl0303', 120, 16, true },
    { 'tank-179-imm', 'xsl0303', 179, 16, true },
    { 'tank-120', 'xsl0303', 120, 16, false },
    { 'tank-179', 'xsl0303', 179, 16, false },
    { 'mml-0-imm', 'ual0111', 0, 35, true },
    { 'mml-20-imm', 'ual0111', 20, 35, true },
    { 'mml-60-imm', 'ual0111', 60, 35, true },
    { 'mml-90', 'ual0111', 90, 35, false },
}
local function heading(u)
    local ok, h = pcall(function() return u:GetHeading() end)
    if ok and h then return math.deg(h) end
    return 0
end
local lanes = {}
for i, s in specs do
    local x, z = LAND[i][1], LAND[i][2]
    local u = make(s[2], A1, x, z)
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    u:SetIntelRadius('Vision', 70)
    if s[5] then u:SetImmobile(true) end
    table.insert(lanes, { tag = s[1], u = u, x = x, z = z, b = s[3], d = s[4] })
end
ForkThread(function()
    WaitTicks(10)
    for _, l in lanes do
        local f = l.u:GetOrientation()
        P(fmt('%s start heading %.1f', l.tag, heading(l.u)))
    end
    T = GetGameTick()
    for _, lane in lanes do
        local l = lane
        -- forward of a unit made with heading 0 is +z
        local r = math.rad(l.b)
        local tx, tz = l.x + math.sin(r) * l.d, l.z + math.cos(r) * l.d
        local t = make('uel0201', A2, tx, tz) t:SetFireState(1)
        t:SetMaxHealth(1000000) t:SetHealth(nil, 1000000)
        l.t = t
        l.hits = {}
        t.OnDamage = function(self, inst, amt, vec, dt)
            local k = fmt('%.0f', amt)
            l.hits[k] = (l.hits[k] or 0) + 1
            if l.hits[k] == 1 then
                local p = l.u:GetPosition()
                local bear = math.deg(math.atan2(tx - p[1], tz - p[3])) - heading(l.u)
                P(fmt('%s first hit amt %s heading %.1f target-bearing-from-heading %.1f', l.tag, k, heading(l.u), bear))
            end
        end
        l.lh = heading(l.u)
    end
    for t = 1, 200 do
        WaitTicks(1)
        for _, l in lanes do
            local h = heading(l.u)
            if math.abs(h - l.lh) > 5 then P(fmt('%s heading %.1f', l.tag, h)) l.lh = h end
        end
        if math.mod(t, 50) == 0 then
            for _, l in lanes do
                local s = ''
                for k, n in l.hits do s = s .. ' ' .. k .. 'x' .. n end
                local p = l.u:GetPosition()
                P(fmt('%s at %d heading %.1f moved %.1f hits%s', l.tag, t, heading(l.u), math.sqrt((p[1] - l.x) * (p[1] - l.x) + (p[3] - l.z) * (p[3] - l.z)), s))
            end
        end
    end
    P('done')
end)
