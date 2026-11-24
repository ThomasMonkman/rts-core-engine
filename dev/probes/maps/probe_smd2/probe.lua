-- Differential probe: how missile defences share incoming missiles out.
-- Follows probe_smd (three loaded SMDs, one nuke: only one fires).
--   real game (Windows): dev/probes/run_faf.ps1 probe_smd2 300
--   ours: cargo run -- --headless --no-ai --ticks 1600 --script dev/probes/maps/probe_smd2/probe.lua
-- [E] the SMD the nuke reaches first has no interceptors, another has 3:
--     does the loaded one fire?
-- [F] three loaded SMDs; the first interceptor is destroyed by script 4
--     ticks after its launch: who fires next, and when?
-- [G] two TMDs (ueb4201) beside a generator and one tactical missile: do
--     both fire at it?
-- [H] a SAM (ueb2304) fires at a patrolling transport that is destroyed by
--     script 3 ticks after the first launch: OnLostTarget on its missiles?
-- Missiles are numbered in launch order ('N' attacking, 'I' defending).
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
            local sh = { tag = tag, p = p, from = name, dist = 0, tick = GetGameTick() }
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
                P(fmt('%s%d OnImpact %s', tag, n, tostring(kind)))
                if impact then return impact(pr, kind, target) end
            end
        end
        return p
    end
end

-- Follows every missile until all are gone (or `ticks` pass). `each()` runs
-- first every tick.
local function follow(ticks, each)
    local gone = {}
    for t = 1, ticks do
        WaitTicks(1)
        if each then each() end
        local alive = 0
        for i, sh in shots do
            if sh.p:BeenDestroyed() then
                if not gone[i] then
                    gone[i] = true
                    P(fmt('%s%d gone, flew %.1f', sh.tag, i, sh.dist))
                end
            else
                alive = alive + 1
                local x, y, z = sh.p:GetPositionXYZ()
                if sh.last then sh.dist = sh.dist + VDist3({x, y, z}, sh.last) end
                sh.last = {x, y, z}
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
local function load(u, n)
    u:RemoveTacticalSiloAmmo(u:GetTacticalSiloAmmoCount())
    if n > 0 then u:GiveTacticalSiloAmmo(n) end
end

ForkThread(function()
    WaitTicks(5)
    local site = make('ueb1101', A1, 430.5, 580.5)
    local silo = make('ueb2305', A2, 650.5, 190.5)
    wrap(silo, 'silo1', 'N')
    -- smd1 is the one the nuke reaches first (probe_smd).
    local spots = { {420.5, 580.5}, {410.5, 592.5}, {402.5, 570.5} }
    local smds = {}
    for i, at in spots do
        smds[i] = make('ueb4302', A1, at[1], at[2])
        wrap(smds[i], 'smd' .. i, 'I')
    end
    WaitTicks(5)

    begin('E')
    load(smds[1], 0) load(smds[2], 0) load(smds[3], 3)
    silo:GiveNukeSiloAmmo(1)
    IssueNuke({silo}, site:GetPosition())
    follow(500)
    WaitTicks(30)
    P('ammo left' .. ammo(smds) .. (site:BeenDestroyed() and ' site destroyed' or ' site alive'))
    if site:BeenDestroyed() then site = make('ueb1101', A1, 430.5, 580.5) end

    begin('F')
    for _, u in smds do load(u, 3) end
    silo:GiveNukeSiloAmmo(1)
    IssueNuke({silo}, site:GetPosition())
    local killed
    follow(500, function()
        if killed then return end
        for i, sh in shots do
            if sh.tag == 'I' and GetGameTick() >= sh.tick + 4 and not sh.p:BeenDestroyed() then
                killed = true
                P(fmt('I%d destroyed by script', i))
                sh.p:Destroy()
            end
        end
    end)
    WaitTicks(30)
    P('ammo left' .. ammo(smds) .. (site:BeenDestroyed() and ' site destroyed' or ' site alive'))

    begin('G')
    for _, u in smds do u:Destroy() end
    local charge = make('ueb1101', A1, 404.5, 600.5)
    local tmd1 = make('ueb4201', A1, 410.5, 600.5)
    local tmd2 = make('ueb4201', A1, 398.5, 600.5)
    wrap(tmd1, 'tmd1', 'I') wrap(tmd2, 'tmd2', 'I')
    local tml = make('ueb2108', A2, 404.5, 668.5)
    wrap(tml, 'tml', 'N')
    WaitTicks(10)
    tml:GiveTacticalSiloAmmo(1)
    IssueTactical({tml}, charge:GetPosition())
    follow(400)
    WaitTicks(20)

    begin('H')
    tmd1:Destroy() tmd2:Destroy() tml:Destroy()
    local sam = make('ueb2304', A1, 404.5, 620.5)
    wrap(sam, 'sam', 'I')
    -- (A radar: the SAM's own vision is shorter than its range.)
    make('ueb3101', A1, 398.5, 626.5)
    local plane = CreateUnitHPR('uea0104', A2, 404.5, GetTerrainHeight(404.5, 568.5) + 12, 568.5, 0, 0, 0)
    plane:SetCanTakeDamage(false)
    -- Kept flying: an idle transport lands.
    IssuePatrol({plane}, {404.5, GetTerrainHeight(404.5, 572.5), 572.5})
    IssuePatrol({plane}, {404.5, GetTerrainHeight(404.5, 560.5), 560.5})
    local destroyed
    follow(200, function()
        if destroyed then return end
        for _, sh in shots do
            if GetGameTick() >= sh.tick + 3 then
                destroyed = true
                P('plane destroyed by script')
                plane:Destroy()
                return
            end
        end
    end)
    LOG('PROBE done')
end)
