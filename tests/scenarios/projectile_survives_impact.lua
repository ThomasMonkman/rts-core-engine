-- Scenario test (tests/scenarios.rs): a projectile whose OnImpact doesn't
-- destroy it rests a tick and is hit again: a wall every 2 ticks at the
-- same contact point; an expired lifetime every 2 ticks, a step further on
-- each time. Never destroyed by the engine (FAF probe probe_re_wpn keep.*).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    return u
end
local function keep(launcher, o, d, lifetime)
    local p = launcher:CreateProjectile(LOS, o[1], o[2], o[3], d[1], d[2], d[3])
    p:SetLifetime(lifetime)
    local k = { p = p, hits = {}, t0 = GetGameTick() }
    p.OnImpact = function(self, tt, te)
        local pos = self:GetPosition()
        table.insert(k.hits, { t = GetGameTick() - k.t0, type = tt, ent = te, x = pos[1], y = pos[2] })
    end
    return k
end
ForkThread(function()
    WaitTicks(2)
    SetAlliance('ARMY_1', 'ARMY_2', 'Enemy') SetAlliance('ARMY_2', 'ARMY_1', 'Enemy')
    local L = make('ueb5101', 'ARMY_1', 720, 64)
    local W = make('ueb5101', 'ARMY_2', 730, 64)
    local wall = keep(L, { 1.5, 0.3, 0 }, { 1, 0, 0 }, 3)
    local up = keep(L, { 0, 8, 0 }, { 0, 1, 0 }, 0.5)
    WaitTicks(40)
    local h = wall.hits
    check(table.getn(h) >= 15, 'the wall is hit again and again: ' .. table.getn(h))
    for i = 2, table.getn(h) do
        check(h[i].t - h[i - 1].t == 2, 'every 2 ticks: ' .. h[i - 1].t .. ' -> ' .. h[i].t)
        check(h[i].type == 'Unit' and h[i].ent == W, 'the wall each time: ' .. tostring(h[i].type))
        check(math.abs(h[i].x - h[1].x) < 0.01, 'at the contact point: ' .. h[1].x .. ' -> ' .. h[i].x)
    end
    local a = up.hits
    check(table.getn(a) >= 15, 'an expired lifetime ends again and again: ' .. table.getn(a))
    for i = 2, table.getn(a) do
        check(a[i].t - a[i - 1].t == 2 and a[i].type == 'Air' and a[i].ent == nil, 'Air every 2 ticks: ' .. a[i].t .. ' ' .. tostring(a[i].type))
        check(math.abs(a[i].y - a[i - 1].y - 2) < 0.01, 'a step on each time: ' .. a[i - 1].y .. ' -> ' .. a[i].y)
    end
    check(not wall.p:BeenDestroyed() and not up.p:BeenDestroyed(), 'neither destroyed by the engine')
    wall.p:Destroy() up.p:Destroy()
    ScenarioResult.done = true
end)
