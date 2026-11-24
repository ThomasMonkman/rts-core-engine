-- Differential probe: several strategic missile defences and one nuke.
--   real game (Windows): dev/probes/run_faf.ps1 probe_smd 300
--   ours: cargo run -- --headless --no-ai --ticks 1700 --script dev/probes/maps/probe_smd/probe.lua
-- [A] three UEF SMDs (ueb4302, 3 interceptors each) round a generator, one
--     nuke: which launch, when, and what each interceptor does once the
--     nuke is down.
-- [B] one SMD, one nuke; the nuke is destroyed by script 4 ticks after the
--     interceptor's launch: is the interceptor's OnLostTarget run, and when
--     and where does it end?
-- [C] three SMDs, two nukes from two launchers at once: who takes which.
-- [D] one SMD 70 to the side of the nuke's target: the interceptor's path.
-- Each tick an interceptor is alive: every missile's position ('N' nukes,
-- 'I' interceptors, numbered in launch order, with the launching unit's
-- name).
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
        if interceptors > 0 then P('at' .. s) end
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
    local silo = make('ueb2305', A2, 650.5, 190.5)
    local silo2 = make('ueb2305', A2, 670.5, 190.5)
    wrap(silo, 'silo1', 'N') wrap(silo2, 'silo2', 'N')
    local spots = { {420.5, 580.5}, {410.5, 592.5}, {402.5, 570.5} }
    local smds = {}
    for i, at in spots do
        smds[i] = make('ueb4302', A1, at[1], at[2])
        wrap(smds[i], 'smd' .. i, 'I')
        P(fmt('smd%d at %.1f,%.1f ground %.2f water %.2f', i, at[1], at[2], GetTerrainHeight(at[1], at[2]), GetSurfaceHeight(at[1], at[2])))
    end
    WaitTicks(5)

    begin('A')
    for _, u in smds do u:GiveTacticalSiloAmmo(3) end
    silo:GiveNukeSiloAmmo(1)
    IssueNuke({silo}, site:GetPosition())
    follow(500)
    WaitTicks(30)
    P('ammo left' .. ammo(smds))

    begin('B')
    -- Only smd1 keeps interceptors.
    for i = 2, 3 do smds[i]:RemoveTacticalSiloAmmo(smds[i]:GetTacticalSiloAmmoCount()) end
    silo:GiveNukeSiloAmmo(1)
    IssueNuke({silo}, site:GetPosition())
    local since
    follow(500, function()
        local nuke, interceptor
        for _, sh in shots do
            if sh.tag == 'N' then nuke = sh.p else interceptor = interceptor or sh.p end
        end
        if interceptor and not since then since = GetGameTick() end
        if since and GetGameTick() == since + 4 and nuke and not nuke:BeenDestroyed() then
            P('nuke destroyed by script')
            nuke:Destroy()
        end
    end)
    WaitTicks(30)
    P('ammo left' .. ammo(smds))

    begin('C')
    for _, u in smds do
        u:RemoveTacticalSiloAmmo(u:GetTacticalSiloAmmoCount())
        u:GiveTacticalSiloAmmo(3)
    end
    silo:GiveNukeSiloAmmo(1) silo2:GiveNukeSiloAmmo(1)
    IssueNuke({silo, silo2}, site:GetPosition())
    follow(500)
    WaitTicks(30)
    P('ammo left' .. ammo(smds))

    begin('D')
    for _, u in smds do u:Destroy() end
    local far = make('ueb4302', A1, 420.5, 650.5)
    wrap(far, 'far', 'I')
    WaitTicks(5)
    far:GiveTacticalSiloAmmo(3)
    silo:GiveNukeSiloAmmo(1)
    IssueNuke({silo}, site:GetPosition())
    follow(500)
    WaitTicks(30)
    P('ammo left' .. ammo({far}) .. (site:BeenDestroyed() and ' site destroyed' or ' site alive'))
    LOG('PROBE done')
end)
