-- Differential probe: what navigator:GetCurrentTargetPos() gives for a unit
-- that isn't going anywhere, on the way, and after it arrives or is stopped
-- (GetGoalPos beside it).
--   real game (Windows): dev/probes/run_faf.ps1 probe_navtarget 90
--   ours: cargo run -- --headless --no-ai --ticks 420 --script dev/probes/maps/probe_navtarget/probe.lua
local armies = ListArmies()
local function f3(n) return string.format('%.3f', n) end

for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end

local function vec(v)
    if v == nil then return 'nil' end
    if type(v) ~= 'table' then return type(v) end
    return f3(v[1]) .. ',' .. f3(v[2]) .. ',' .. f3(v[3])
end
local function report(tag, when, u)
    local ok, nav = pcall(u.GetNavigator, u)
    if not ok or not nav then
        LOG('PROBE ' .. tag .. ' ' .. when .. ' navigator ' .. (ok and 'nil' or 'error'))
        return
    end
    local okT, t = pcall(nav.GetCurrentTargetPos, nav)
    local okG, g = pcall(nav.GetGoalPos, nav)
    local p = u:GetPosition()
    local d = (okT and type(t) == 'table') and f3(VDist2(t[1], t[3], p[1], p[3])) or '-'
    LOG('PROBE ' .. tag .. ' ' .. when .. ' pos ' .. vec(p) .. ' target ' .. (okT and vec(t) or 'error') .. ' dist ' .. d .. ' goal ' .. (okG and vec(g) or 'error'))
end

-- { tag, blueprint, x, z }: off the grid on purpose, to see any snapping.
local specs = {
    { 'tank', 'uel0201', 400.3, 600.7 },
    { 'tank_grid', 'uel0201', 410.5, 600.5 },
    { 'bot3', 'uel0303', 420.3, 600.7 },
    { 'acu', 'uel0001', 430.3, 600.7 },
    { 'hover', 'ual0201', 440.3, 600.7 },
    { 'fighter', 'uea0102', 450.3, 600.7 },
    { 'gunship', 'uea0203', 460.3, 600.7 },
    { 'pd', 'ueb2101', 470.5, 600.5 },
}
local units = {}
for _, s in specs do
    local u = CreateUnitHPR(s[2], armies[1], s[3], GetTerrainHeight(s[3], s[4]), s[4], 0, 0, 0)
    u:SetFireState(1)
    table.insert(units, { tag = s[1], u = u, x = s[3], z = s[4] })
end

ForkThread(function()
    local function all(when)
        for _, e in units do
            if not e.u.Dead then report(e.tag, when, e.u) end
        end
    end
    local function navs(fn)
        for _, e in units do
            local ok, nav = pcall(e.u.GetNavigator, e.u)
            if ok and nav and e.tag ~= 'pd' then pcall(fn, nav, e) end
        end
    end
    all('spawned')
    WaitTicks(10)
    all('idle')
    -- the navigator's own goal
    navs(function(nav, e) nav:SetGoal({ e.x + 0.4, 0, e.z + 12.4 }) end)
    WaitTicks(1)
    all('setgoal+1')
    WaitTicks(9)
    all('setgoal+10')
    WaitTicks(110)
    all('setgoal_arrived')
    -- an order
    for _, e in units do
        if e.tag ~= 'pd' then IssueMove({ e.u }, { e.x + 10.2, GetTerrainHeight(e.x + 10.2, e.z + 20.6), e.z + 20.6 }) end
    end
    WaitTicks(8)
    all('move+8')
    WaitTicks(130)
    all('move_arrived')
    -- stopped part way
    for _, e in units do
        if e.tag ~= 'pd' then IssueMove({ e.u }, { e.x + 40, GetTerrainHeight(e.x + 40, e.z + 40), e.z + 40 }) end
    end
    WaitTicks(25)
    all('move2+25')
    for _, e in units do IssueClearCommands({ e.u }) end
    WaitTicks(1)
    all('cleared+1')
    WaitTicks(30)
    all('cleared+31')
    navs(function(nav, e) nav:SetGoal({ e.x, 0, e.z }) end)
    WaitTicks(5)
    navs(function(nav) nav:AbortMove() end)
    all('aborted')
    WaitTicks(20)
    all('aborted+20')
    LOG('PROBE done')
end)
