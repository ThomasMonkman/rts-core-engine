-- Probe: a factory's new engineer is given an order the first tick it is no
-- longer attached (still on the pad), with its target inside its reach.
-- Does it work from the pad, or leave first, and when is the factory free?
--   stop:     its orders are cleared, nothing else
--   repair:   repair a damaged power generator
--   reclaim:  reclaim a wall section
--   assist:   guard an engineer that is building a structure
--   guardfac: guard its own factory
--   guardoth: guard another factory that is building
-- (probe_padbuild2 covers a build order: it walks out first.)
local armies = ListArmies()
local army = armies[1]
local brain = GetArmyBrain(army)
local sx, sz = brain:GetArmyStartPos()
brain:GiveStorage('MASS', 100000)
brain:GiveStorage('ENERGY', 100000)
brain:GiveResource('MASS', 100000)
brain:GiveResource('ENERGY', 100000)
local running = 0
local names = { 'Moving', 'Building', 'Repairing', 'Reclaiming', 'Guarding', 'Busy', 'Attached', 'Immobile', 'BlockCommandQueue' }
local function states(u)
    if not u or u:BeenDestroyed() then return 'gone' end
    local s = ''
    for _, n in names do if u:IsUnitState(n) then s = s .. n .. ',' end end
    return '[' .. s .. ']'
end
local function make(bp, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end

local function case(tag, ox, oz)
    running = running + 1
    local x, z = sx + ox, sz + oz
    local f = make('ueb0101', x, z)
    WaitTicks(10)
    local fq = f:GetPosition()
    local fp = { fq[1], fq[2], fq[3] }
    -- Targets 7.2 from the factory centre, as probe_padbuild2's site.
    local tx, tz = fp[1] - 6, fp[3] + 4
    local target, helper
    if tag == 'repair' then
        target = make('ueb1101', tx, tz)
        target:SetHealth(target, 50)
    elseif tag == 'reclaim' then
        target = make('ueb5101', tx, tz)
    elseif tag == 'assist' then
        -- (energy storage: a long build)
        helper = make('uel0105', tx - 4, tz)
        WaitTicks(2)
        IssueBuildMobile({ helper }, { tx, GetTerrainHeight(tx, tz), tz }, 'ueb1105', {})
    elseif tag == 'guardoth' then
        target = make('ueb0101', fp[1] - 12, fp[3])
        WaitTicks(2)
        IssueBuildFactory({ target }, 'uel0201', 5)
    end
    IssueBuildFactory({ f }, 'uel0105', 1)
    local u
    for i = 1, 400 do
        WaitTicks(1)
        u = u or f.UnitBeingBuilt
        if u and u:GetFractionComplete() >= 1 then break end
    end
    if not u then LOG('PROBE ', tag, ' no engineer') running = running - 1 return end
    for i = 1, 100 do
        if not u:IsUnitState('Attached') then break end
        WaitTicks(1)
    end
    local p0 = u:GetPosition()
    local x0, z0 = p0[1], p0[3]
    local t0 = GetGameTick()
    LOG(string.format('PROBE %s order at d=%.2f,%.2f from the factory, eng %s fac %s', tag, x0 - fp[1], z0 - fp[3], states(u), states(f)))
    IssueClearCommands({ u })
    if tag == 'repair' then IssueRepair({ u }, target)
    elseif tag == 'reclaim' then IssueReclaim({ u }, target)
    elseif tag == 'assist' then IssueGuard({ u }, helper)
    elseif tag == 'guardfac' then IssueGuard({ u }, f)
    elseif tag == 'guardoth' then IssueGuard({ u }, target)
    end
    local prev
    for i = 1, 150 do
        WaitTicks(1)
        if u:BeenDestroyed() then break end
        local p = u:GetPosition()
        local st = states(u) .. ' fac ' .. states(f)
        if st ~= prev then
            prev = st
            LOG(string.format('PROBE %s +%d eng d=%.2f,%.2f walked=%.2f %s focus=%s', tag, GetGameTick() - t0, p[1] - fp[1], p[3] - fp[3], VDist2(x0, z0, p[1], p[3]), st,
                tostring(u:GetFocusUnit() and u:GetFocusUnit():GetUnitId())))
        end
    end
    local p = u:GetPosition()
    LOG(string.format('PROBE %s end +%d eng d=%.2f,%.2f walked=%.2f %s fac %s', tag, GetGameTick() - t0, p[1] - fp[1], p[3] - fp[3], VDist2(x0, z0, p[1], p[3]), states(u), states(f)))
    running = running - 1
end

ForkThread(function()
    WaitTicks(10)
    ForkThread(case, 'stop', -60, -30)
    ForkThread(case, 'repair', -30, -30)
    ForkThread(case, 'reclaim', 0, -30)
    ForkThread(case, 'assist', 30, -30)
    ForkThread(case, 'guardfac', -30, 30)
    ForkThread(case, 'guardoth', 20, 30)
    WaitTicks(20)
    while running > 0 do WaitTicks(10) end
    LOG('PROBE done')
end)
