-- Differential probe (open question): does the Seraphim torpedo defence
-- xsb2205's AjelluTorpedoDefense fire at incoming torpedoes? An enemy T1 sub
-- (uas0203, vision 70) 22 away attacks it; the structure's own TorpedoTurrets
-- are disabled. Counts projectiles each weapon makes; tracks the first few
-- torpedoes' y against the water surface, and the Ajellu muzzle y.
--   real game: run_faf.ps1 probe_q_torp 120
--   ours: --headless --no-ai --ticks 300 --script dev/probes/maps/probe_q_torp/probe.lua
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
-- deep water spot: first on a grid with depth > 12 over a 40 x 10 box
local sx, sz
for z = 64, 960, 32 do
    for x = 64, 960, 32 do
        local ok = true
        for dx = -6, 34, 4 do
            for dz = -5, 5, 5 do
                if GetSurfaceHeight(x + dx, z + dz) - GetTerrainHeight(x + dx, z + dz) < 12 then ok = false end
            end
        end
        if ok then sx, sz = x, z break end
    end
    if sx then break end
end
P(fmt('site %s,%s depth %.2f', tostring(sx), tostring(sz), sx and (GetSurfaceHeight(sx, sz) - GetTerrainHeight(sx, sz)) or -1))
local function hookProj(w, note)
    local orig = w.CreateProjectileAtMuzzle
    w.CreateProjectileAtMuzzle = function(self, muzzle)
        self.CreateProjectileAtMuzzle = nil
        local pr = self:CreateProjectileAtMuzzle(muzzle)
        self.CreateProjectileAtMuzzle = w.hooked
        note(pr, muzzle)
        return pr
    end
    w.hooked = w.CreateProjectileAtMuzzle
end
ForkThread(function()
    local d = CreateUnitHPR('xsb2205', A1, sx, GetSurfaceHeight(sx, sz), sz, 0, 0, 0)
    d:SetMaxHealth(10000000) d:SetHealth(nil, 10000000)
    d:SetWeaponEnabledByLabel('TorpedoTurrets', false)
    d:SetIntelRadius('Vision', 70) d:SetIntelRadius('Sonar', 70)
    WaitTicks(5)
    local s = CreateUnitHPR('uas0203', A2, sx + 22, GetSurfaceHeight(sx + 22, sz), sz, 0, 0, 0)
    s:SetMaxHealth(10000000) s:SetHealth(nil, 10000000)
    s:SetIntelRadius('Vision', 70) s:SetIntelRadius('Sonar', 70)
    local aj = d:GetWeaponByLabel('AjelluTorpedoDefense')
    local tw = s:GetWeaponByLabel('Torpedo01')
    local nAj, nT = 0, 0
    local torps = {}
    hookProj(aj, function(pr, m) nAj = nAj + 1 if nAj <= 5 then P(fmt('ajellu projectile %d muzzle %s', nAj, tostring(m))) end end)
    hookProj(tw, function(pr, m)
        nT = nT + 1
        if nT <= 3 and pr then table.insert(torps, { pr = pr, n = nT, k = 0 }) end
        if nT <= 5 then P(fmt('torpedo %d made', nT)) end
    end)
    WaitTicks(10)
    T = GetGameTick()
    local mp = d:GetPosition('Muzzle04')
    P(fmt('ajellu muzzle y %.2f surface %.2f struct y %.2f', mp[2], GetSurfaceHeight(sx, sz), d:GetPosition()[2]))
    IssueAttack({ s }, d)
    local lastHas
    for k = 1, 220 do
        WaitTicks(1)
        local has = tostring(aj:WeaponHasTarget())
        if has ~= lastHas then P('ajellu has target ' .. has) lastHas = has end
        for _, t in torps do
            if t.pr and not t.pr:BeenDestroyed() and t.k < 40 then
                t.k = t.k + 1
                local p = t.pr:GetPosition()
                local dx, dz = p[1] - mp[1], p[3] - mp[3]
                if math.mod(t.k, 2) == 1 then
                    P(fmt('torp%d y-surface %.2f y-muzzle %.2f hdist %.2f', t.n, p[2] - GetSurfaceHeight(p[1], p[3]), p[2] - mp[2], math.sqrt(dx * dx + dz * dz)))
                end
            end
        end
        if math.mod(k, 50) == 0 then P(fmt('counts torpedoes %d ajellu %d', nT, nAj)) end
    end
    P('done')
end)
