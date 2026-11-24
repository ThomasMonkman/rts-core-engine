-- Differential probe (open question): does FiringTolerance (30) gate the CZAR's
-- fixed downward beam? Immobile CZARs (uaa0310) each get one enemy tank (hold
-- fire) at a horizontal offset 0 / 8 / 14 / 20 / 26 from below; a mobile one
-- gets a tank 28 away. Per tick: has-target, beam enabled, horizontal offset.
--   real game: run_faf.ps1 probe_q_czar 120
--   ours: --headless --no-ai --ticks 300 --script dev/probes/maps/probe_q_czar/probe.lua
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
local function P(s) LOG(fmt('PROBE %s %s', T and ('T+' .. (GetGameTick() - T)) or ('t' .. GetGameTick()), s)) end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local SITES = { {720,64}, {920,96}, {776,236}, {664,330}, {904,384}, {568,460} }
local specs = { { 'off0', 0, true }, { 'off8', 8, true }, { 'off14', 14, true }, { 'off20', 20, true }, { 'off26', 26, true }, { 'mobile28', 28, false } }
local lanes = {}
for i, s in specs do
    local x, z = SITES[i][1], SITES[i][2]
    local u = make('uaa0310', A1, x, z)
    u:SetMaxHealth(10000000) u:SetHealth(nil, 10000000)
    u:SetIntelRadius('Vision', 70)
    table.insert(lanes, { tag = s[1], off = s[2], imm = s[3], u = u })
end
ForkThread(function()
    WaitTicks(120)
    for _, lane in lanes do
        local l = lane
        local p = l.u:GetPosition()
        if l.imm then l.u:SetImmobile(true) end
        l.w = l.u:GetWeaponByLabel('QuantumBeamGeneratorWeapon')
        P(fmt('%s czar at %.1f,%.1f,%.1f ground %.1f', l.tag, p[1], p[2], p[3], GetTerrainHeight(p[1], p[3])))
    end
    T = GetGameTick()
    for _, lane in lanes do
        local l = lane
        local p = l.u:GetPosition()
        local t = make('uel0201', A2, p[1] + l.off, p[3])
        t:SetFireState(1)
        t:SetMaxHealth(10000000) t:SetHealth(nil, 10000000)
        l.t = t
        l.last = ''
        l.hits = 0
        t.OnDamage = function(self, inst, amt, vec, dt)
            l.hits = l.hits + 1
            if l.hits == 1 then P(fmt('%s first damage %s %.0f', l.tag, tostring(dt), amt)) end
        end
    end
    for k = 0, 150 do
        for _, lane in lanes do
            local l = lane
            local w = l.w
            local on = false
            if w and w.Beams and w.Beams[1] then on = w.Beams[1].Beam:IsEnabled() end
            local p, q = l.u:GetPosition(), l.t:GetPosition()
            local dx, dz = q[1] - p[1], q[3] - p[3]
            local s = fmt('has %s beam %s', tostring(w and w:WeaponHasTarget()), tostring(on))
            if s ~= l.last then
                P(fmt('%s %s hoff %.2f height %.2f', l.tag, s, math.sqrt(dx * dx + dz * dz), p[2] - q[2]))
                l.last = s
            end
        end
        WaitTicks(1)
    end
    for _, l in lanes do P(fmt('%s end hits %d', l.tag, l.hits)) end
    P('done')
end)
