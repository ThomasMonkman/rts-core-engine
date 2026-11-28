-- Differential probe: strategic missile defences and the Yolona Oss's
-- missile (xsb2401; its projectile has `DesiredShooterCap = 2` and 60000
-- health, an ordinary nuke 1 and 25000).
--   real game (Windows): dev/probes/run_faf.ps1 probe_smd3 300
--   ours: cargo run -- --headless --no-ai --ticks 2600 --script dev/probes/maps/probe_smd3/probe.lua
-- [I] three UEF SMDs (ueb4302), 3 interceptors each: who launches, when, and
--     how many it takes.
-- [J] one SMD with 3 interceptors, the others empty.
-- [K] two SMDs with one interceptor each, the third empty.
-- [L] one SMD with one interceptor.
-- Lines for each launch, impact and missile gone, the missile's health
-- whenever it changes, then the ammo left and whether the target stands.
local fmt = string.format
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local part, T = 'pre', 0
local function P(s) LOG(fmt('PROBE %s T+%d %s', part, GetGameTick() - T, s)) end
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetCanTakeDamage(false)
    return u
end

local shots = {}
local function wrap(unit, name, tag)
    local w = unit:GetWeapon(1)
    local create = w.CreateProjectileAtMuzzle
    w.CreateProjectileAtMuzzle = function(self, muzzle)
        local p = create(self, muzzle)
        if p then
            local sh = { tag = tag, p = p, from = name, dist = 0 }
            table.insert(shots, sh)
            local n = table.getn(shots)
            P(fmt('%s%d launched by %s', tag, n, name))
            local lost = p.OnLostTarget
            p.OnLostTarget = function(pr)
                P(fmt('%s%d OnLostTarget', tag, n))
                if lost then return lost(pr) end
            end
            local impact = p.OnImpact
            p.OnImpact = function(pr, kind, target)
                local x, y, z = pr:GetPositionXYZ()
                P(fmt('%s%d OnImpact %s at %.1f,%.1f,%.1f', tag, n, tostring(kind), x, y, z))
                if impact then return impact(pr, kind, target) end
            end
        end
        return p
    end
end

-- Follows every missile until all are gone (or `ticks` pass), logging a line
-- a tick while an interceptor is alive. `each()` runs first every tick.
local function follow(ticks, each)
    local gone = {}
    for t = 1, ticks do
        WaitTicks(1)
        if each then each() end
        local s, alive, interceptors = '', 0, 0
        for i, sh in shots do
            if sh.p:BeenDestroyed() then
                if not gone[i] then
                    gone[i] = true
                    P(fmt('%s%d gone, flew %.1f, last %s', sh.tag, i, sh.dist, sh.last and fmt('%.1f,%.1f,%.1f', sh.last[1], sh.last[2], sh.last[3]) or '-'))
                end
            else
                alive = alive + 1
                if sh.tag == 'I' then interceptors = interceptors + 1 end
                local x, y, z = sh.p:GetPositionXYZ()
                if sh.last then sh.dist = sh.dist + VDist3({x, y, z}, sh.last) end
                sh.last = {x, y, z}
                s = s .. fmt(' %s%d %.1f,%.1f,%.1f', sh.tag, i, x, y, z)
            end
        end
        if alive == 0 and table.getn(shots) > 0 and t > 20 then return end
    end
    P('follow timed out')
end

local function begin(name)
    for _, sh in shots do if not sh.p:BeenDestroyed() then sh.p:Destroy() end end
    shots = {}
    part, T = name, GetGameTick()
end
local function ammo(list)
    local s = ''
    for i, u in list do s = s .. fmt(' smd%d %d', i, u:GetTacticalSiloAmmoCount()) end
    return s
end

ForkThread(function()
    WaitTicks(5)
    local site = make('ueb1101', A1, 430.5, 580.5)
    local silo
    local spots = { {420.5, 580.5}, {410.5, 592.5}, {402.5, 570.5} }
    local smds = {}
    for i, at in spots do
        smds[i] = make('ueb4302', A1, at[1], at[2])
        wrap(smds[i], 'smd' .. i, 'I')
    end
    WaitTicks(5)
    local function run(name, loads)
        begin(name)
        for i, u in smds do
            u:RemoveTacticalSiloAmmo(u:GetTacticalSiloAmmoCount())
            if loads[i] > 0 then u:GiveTacticalSiloAmmo(loads[i]) end
        end
        -- A new launcher each time: in the real game one given a second
        -- missile and order by script 40 ticks after its first landed
        -- didn't launch again in 900 ticks (ours does).
        if silo then silo:Destroy() end
        WaitTicks(5)
        silo = make('xsb2401', A2, 650.5, 190.5)
        wrap(silo, 'yolona', 'N')
        WaitTicks(5)
        part, T = name, GetGameTick()
        silo:GiveNukeSiloAmmo(1)
        IssueNuke({silo}, site:GetPosition())
        local health
        follow(900, function()
            for _, sh in shots do
                if sh.tag == 'N' and not sh.p:BeenDestroyed() then
                    local h = sh.p:GetHealth()
                    if h ~= health then
                        health = h
                        P(fmt('missile health %d', h))
                    end
                end
            end
        end)
        WaitTicks(40)
        P('ammo left' .. ammo(smds) .. (site:BeenDestroyed() and ' site destroyed' or ' site alive'))
        if site:BeenDestroyed() then
            site = make('ueb1101', A1, 430.5, 580.5)
            WaitTicks(5)
        end
    end
    run('I', { 3, 3, 3 })
    run('J', { 3, 0, 0 })
    run('K', { 1, 1, 0 })
    run('L', { 1, 0, 0 })
    LOG('PROBE done')
end)
