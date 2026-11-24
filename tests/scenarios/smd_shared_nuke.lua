-- Scenario test (tests/scenarios.rs): several strategic missile defences
-- and one nuke (issue #84; the real game: dev/probes probe_smd, probe_smd2).
-- A nuke's blueprint has `DesiredShooterCap = 1`: one SMD takes it and
-- launches, the others in range keep their interceptors. An SMD with none
-- takes nothing, and the one holding the nuke launches again once reloaded
-- if its interceptor is lost. An interceptor whose nuke is gone hears
-- `OnLostTarget` once and FAF's script ends it. The Yolona Oss's missile has
-- cap 2 and the health for two interceptors: two SMDs take it (probe_smd3).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
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
            local sh = { tag = tag, p = p, from = name, tick = GetGameTick(), lost = 0 }
            table.insert(shots, sh)
            local lost = p.OnLostTarget
            p.OnLostTarget = function(pr)
                sh.lost = sh.lost + 1
                if lost then return lost(pr) end
            end
        end
        return p
    end
end
local function of(tag)
    local list = {}
    for _, sh in shots do if sh.tag == tag then table.insert(list, sh) end end
    return list
end
-- Runs until every missile is gone; `each()` first every tick.
local function follow(each)
    for t = 1, 500 do
        WaitTicks(1)
        if each then each() end
        local alive = 0
        for _, sh in shots do
            if not sh.p:BeenDestroyed() then alive = alive + 1 elseif not sh.gone then sh.gone = GetGameTick() end
        end
        if alive == 0 and table.getn(shots) > 0 and t > 20 then return end
    end
    check(false, 'missiles still flying after 500 ticks')
end
local function load(u, n)
    u:RemoveTacticalSiloAmmo(u:GetTacticalSiloAmmoCount())
    if n > 0 then u:GiveTacticalSiloAmmo(n) end
end

ForkThread(function()
    WaitTicks(2)
    local site = make('ueb1101', 'ARMY_1', 430.5, 580.5)
    local silo = make('ueb2305', 'ARMY_2', 650.5, 190.5)
    wrap(silo, 'silo', 'N')
    local down = 0
    local intercepted = silo.OnMissileIntercepted
    silo.OnMissileIntercepted = function(self, a, b, c)
        down = down + 1
        if intercepted then return intercepted(self, a, b, c) end
    end
    -- All three are in range of the nuke as it comes down on the site; it
    -- reaches smd1 first.
    local smds = {}
    for i, at in { {420.5, 580.5}, {410.5, 592.5}, {402.5, 570.5} } do
        smds[i] = make('ueb4302', 'ARMY_1', at[1], at[2])
        wrap(smds[i], 'smd' .. i, 'I')
    end
    local function ammo() return smds[1]:GetTacticalSiloAmmoCount() + smds[2]:GetTacticalSiloAmmoCount() + smds[3]:GetTacticalSiloAmmoCount() end
    local function launch()
        shots, down = {}, 0
        silo:GiveNukeSiloAmmo(1)
        IssueNuke({silo}, site:GetPosition())
    end
    WaitTicks(5)

    -- Three loaded SMDs: one interceptor.
    for _, u in smds do load(u, 3) end
    launch()
    follow()
    WaitTicks(20)
    local interceptors = of('I')
    check(table.getn(interceptors) == 1, 'three SMDs, one nuke: one interceptor launched, got ' .. table.getn(interceptors))
    check(down == 1, 'the nuke was intercepted: ' .. down)
    check(ammo() == 8, 'one interceptor spent of nine: ' .. ammo() .. ' left')
    check(interceptors[1] and interceptors[1].lost == 0, 'the interceptor that hit heard no OnLostTarget')
    check(not site:BeenDestroyed(), 'the site survives')

    -- The SMD the nuke reaches first is empty: it takes nothing, a loaded
    -- one fires. Its interceptor is destroyed: it fires again, reloaded.
    load(smds[1], 0) load(smds[2], 0) load(smds[3], 3)
    launch()
    local killed
    follow(function()
        local first = of('I')[1]
        if first and not killed and GetGameTick() >= first.tick + 4 and not first.p:BeenDestroyed() then
            killed = true
            first.p:Destroy()
        end
    end)
    WaitTicks(20)
    interceptors = of('I')
    check(killed, 'the first interceptor was destroyed in flight')
    check(table.getn(interceptors) == 2, 'a second interceptor follows the lost one: ' .. table.getn(interceptors))
    for i, sh in interceptors do check(sh.from == 'smd3', 'interceptor ' .. i .. ' from the loaded SMD: ' .. sh.from) end
    if interceptors[2] then
        local wait = interceptors[2].tick - interceptors[1].tick
        check(wait >= 17 and wait <= 25, 'the second after the reload (real game 21 ticks): ' .. wait)
    end
    check(down == 1, 'the nuke was intercepted by the second: ' .. down)
    check(not site:BeenDestroyed(), 'the site survives')

    -- The nuke is destroyed with an interceptor on its way: the interceptor
    -- hears OnLostTarget once and FAF's script ends it (real game: gone 9
    -- ticks after the nuke); nobody launches another.
    for _, u in smds do load(u, 3) end
    launch()
    local destroyed
    follow(function()
        local nuke, first = of('N')[1], of('I')[1]
        if first and not destroyed and GetGameTick() >= first.tick + 4 and nuke and not nuke.p:BeenDestroyed() then
            destroyed = GetGameTick()
            nuke.p:Destroy()
        end
    end)
    WaitTicks(40)
    interceptors = of('I')
    check(destroyed, 'the nuke was destroyed in flight')
    check(table.getn(interceptors) == 1, 'one interceptor for the lost nuke: ' .. table.getn(interceptors))
    local sh = interceptors[1]
    if sh and destroyed then
        check(sh.lost == 1, 'OnLostTarget once: ' .. sh.lost)
        check(sh.gone and sh.gone - destroyed <= 12, 'the interceptor is gone soon after: ' .. tostring(sh.gone and sh.gone - destroyed))
    end

    -- The Yolona Oss's missile (`DesiredShooterCap = 2`, 60000 health, an
    -- interceptor does 30000): two of three loaded SMDs launch one each
    -- (real game: smd1 and smd3, 4 ticks apart) and the second hit ends it.
    silo:Destroy()
    local yolona = make('xsb2401', 'ARMY_2', 650.5, 190.5)
    wrap(yolona, 'yolona', 'N')
    WaitTicks(5)
    for _, u in smds do load(u, 3) end
    shots = {}
    yolona:GiveNukeSiloAmmo(1)
    IssueNuke({yolona}, site:GetPosition())
    local health = {}
    follow(function()
        local nuke = of('N')[1]
        if nuke and not nuke.p:BeenDestroyed() then
            local h = nuke.p:GetHealth()
            if h ~= health[table.getn(health)] then table.insert(health, h) end
            nuke.at = nuke.p:GetPosition()
        end
    end)
    WaitTicks(20)
    interceptors = of('I')
    check(table.getn(interceptors) == 2, 'three SMDs, one Yolona Oss missile: two interceptors, got ' .. table.getn(interceptors))
    if interceptors[2] then
        check(interceptors[1].from ~= interceptors[2].from, 'from two SMDs: ' .. interceptors[1].from .. ', ' .. interceptors[2].from)
        check(interceptors[2].tick - interceptors[1].tick <= 8, 'together, not a reload apart: ' .. interceptors[2].tick - interceptors[1].tick)
    end
    check(ammo() == 7, 'two interceptors spent of nine: ' .. ammo() .. ' left')
    check(health[1] == 60000 and health[2] == 30000 and not health[3], 'the missile took one hit and died on the second: ' .. table.concat(health, ', '))
    local nuke = of('N')[1]
    check(nuke and nuke.at and nuke.at[2] > 100, 'it was brought down high up: ' .. tostring(nuke and nuke.at and nuke.at[2]))
    ScenarioResult.done = true
end)
