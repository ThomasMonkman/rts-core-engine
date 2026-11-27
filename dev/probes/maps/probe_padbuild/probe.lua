-- Probe, two questions:
-- 1. hold-* (clear: 20 ticks after it finishes; callback: inside the
--    assister's OnStopBuild; unpause: no clear): a structure's builder is paused and another engineer finishes
--    it. When does the structure's OnStopBeingBuilt run, and with whom?
--    What if the paused builder's orders are then cleared?
-- 2. fac-*: a factory's new engineer is given a build order as it completes
--    and can reach the site from the pad. Does it leave the pad first, and
--    how long does the factory stay Busy before its next unit starts?
local armies = ListArmies()
local army = armies[1]
local brain = GetArmyBrain(army)
local sx, sz = brain:GetArmyStartPos()
brain:GiveStorage('MASS', 100000)
brain:GiveStorage('ENERGY', 100000)
brain:GiveResource('MASS', 100000)
brain:GiveResource('ENERGY', 100000)

local names = { 'Moving', 'Building', 'Busy', 'Guarding', 'Repairing', 'BeingBuilt', 'Paused', 'Attached', 'Immobile', 'BlockCommandQueue' }
local function states(u)
    if not u then return 'nil' end
    if u:BeenDestroyed() then return 'destroyed' end
    local s = ''
    for _, n in names do
        local ok, r = pcall(function() return u:IsUnitState(n) end)
        if ok and r then s = s .. n .. ',' end
    end
    return '[' .. s .. ']'
end
local function queue(u)
    local ok, q = pcall(function() return u:GetCommandQueue() end)
    if not ok or not q then return '?' end
    local s = table.getn(q) .. ':'
    for _, c in q do s = s .. tostring(c.commandType) .. ' ' end
    return s
end
local function at(x, z) return x, GetTerrainHeight(x, z), z end
local running = 0

local function hold(tag, ox, clear)
    running = running + 1
    local x, z = sx + ox, sz + 25
    local e1 = CreateUnitHPR('uel0105', army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local e2 = CreateUnitHPR('uel0105', army, x + 2, GetTerrainHeight(x + 2, z), z, 0, 0, 0)
    WaitTicks(5)
    local t0 = GetGameTick()
    local who = function(b) return (b == e1 and 'e1') or (b == e2 and 'e2') or tostring(b) end
    for n, e in { e1 = e1, e2 = e2 } do
        local name, os, of = n, e.OnStopBuild, e.OnFailedToBuild
        e.OnStopBuild = function(self, u, a)
            LOG('PROBE ', tag, ' dt=', GetGameTick() - t0, ' ', name, '.OnStopBuild')
            -- An AI's reaction to the assister's stop: new orders for the builder at once.
            if clear == 'callback' and name == 'e2' then IssueClearCommands({ e1 }) LOG('PROBE ', tag, ' e1 cleared inside e2.OnStopBuild') end
            return os(self, u, a)
        end
        e.OnFailedToBuild = function(self) LOG('PROBE ', tag, ' dt=', GetGameTick() - t0, ' ', name, '.OnFailedToBuild') return of(self) end
    end
    LOG('PROBE ', tag, ' site buildable=', tostring(brain:CanBuildStructureAt('ueb1101', { x, GetTerrainHeight(x, z + 5), z + 5 })))
    IssueBuildMobile({ e1 }, { x, GetTerrainHeight(x, z + 5), z + 5 }, 'ueb1101', {})
    local s
    for i = 1, 100 do
        WaitTicks(1)
        s = e1.UnitBeingBuilt
        if s then break end
    end
    if not s then LOG('PROBE ', tag, ' no structure started') running = running - 1 return end
    local stopped = 'no'
    local o = s.OnStopBeingBuilt
    s.OnStopBeingBuilt = function(self, b, l)
        stopped = 'by-' .. who(b) .. '@' .. (GetGameTick() - t0)
        LOG('PROBE ', tag, ' dt=', GetGameTick() - t0, ' structure.OnStopBeingBuilt builder=', who(b))
        return o(self, b, l)
    end
    local line = function(note)
        LOG('PROBE ', tag, ' dt=', GetGameTick() - t0, ' ', note, ' frac=', string.format('%.3f', s:GetFractionComplete()),
            ' beingBuilt=', tostring(s:IsBeingBuilt()), ' s=', states(s), ' stopped=', stopped,
            ' e1=', states(e1), ' q1=', queue(e1), ' e2=', states(e2), ' q2=', queue(e2))
    end
    WaitTicks(10)
    e1:SetPaused(true)
    IssueRepair({ e2 }, s)
    line('paused+repair')
    local last
    for i = 1, 900 do
        WaitTicks(1)
        local f = s:GetFractionComplete()
        if f >= 1 then break end
        if math.floor(f * 4) ~= last then last = math.floor(f * 4) line('progress') end
    end
    line('frac1')
    for i = 1, 5 do WaitTicks(1) line('frac1+' .. i) end
    WaitTicks(15)
    line('before-' .. tostring(clear))
    if clear == true then IssueClearCommands({ e1 }) elseif not clear then e1:SetPaused(false) end
    line('after-call')
    for i = 1, 6 do WaitTicks(1) line('then+' .. i) end
    if clear then
        e1:SetPaused(false)
        for i = 1, 4 do WaitTicks(1) line('unpaused+' .. i) end
    end
    running = running - 1
end

local function fac(tag, ox, dx, dz)
    running = running + 1
    local x, z = sx + ox, sz - 25
    local f = CreateUnitHPR('ueb0101', army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    WaitTicks(10)
    local fq = f:GetPosition()
    local fp = { fq[1], fq[2], fq[3] } -- a copy: GetPosition's table is shared
    IssueBuildFactory({ f }, 'uel0105', 2)
    local t0 = GetGameTick()
    local u1, done, prev, t1, second
    for i = 1, 700 do
        WaitTicks(1)
        local cur = f.UnitBeingBuilt
        if not u1 and cur then u1 = cur end
        if u1 and not done and not u1:BeenDestroyed() and u1:GetFractionComplete() >= 1 then
            done = true
            t1 = GetGameTick()
            LOG('PROBE ', tag, ' dt=', t1 - t0, ' first engineer complete q=', queue(u1))
            if dx then
                IssueClearCommands({ u1 })
                IssueBuildMobile({ u1 }, { fp[1] + dx, GetTerrainHeight(fp[1] + dx, fp[3] + dz), fp[3] + dz }, 'ueb1101', {})
            end
        end
        if done and not u1:BeenDestroyed() then
            if not second and cur and cur ~= u1 then
                second = true
                LOG('PROBE ', tag, ' second unit started ', GetGameTick() - t1, ' ticks after the first completed')
            end
            local p = u1:GetPosition()
            local b = u1.UnitBeingBuilt
            local s = string.format('eng d=%.0f,%.0f %s q=%s site=%s | fac %s wp=%.2f next=%s', p[1] - fp[1], p[3] - fp[3], states(u1), queue(u1),
                (b and not b:BeenDestroyed()) and string.format('%.1f', math.floor(b:GetFractionComplete() * 5) / 5) or '-', states(f), math.floor(f:GetWorkProgress() * 10) / 10, tostring(second or false))
            if s ~= prev then prev = s LOG('PROBE ', tag, ' +', GetGameTick() - t1, ' ', s) end
            if GetGameTick() - t1 > 320 then break end
        end
    end
    running = running - 1
end

ForkThread(function()
    WaitTicks(10)
    ForkThread(hold, 'hold-clear', -20, true)
    ForkThread(hold, 'hold-unpause', -32, false)
    ForkThread(hold, 'hold-callback', -44, 'callback')
    ForkThread(fac, 'fac-control', -40, nil, nil)
    ForkThread(fac, 'fac-near', 0, -6, 4)
    ForkThread(fac, 'fac-far', 40, -25, 4)
    WaitTicks(20)
    while running > 0 do WaitTicks(10) end
    LOG('PROBE done')
end)
