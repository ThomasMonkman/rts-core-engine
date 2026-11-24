-- Differential probe (open question): sighting lag. Each lane: an army-1 tank
-- (vision 20, hold fire) and an army-2 tank (hold fire) 40 away; at game tick
-- W the enemy is warped to 10 from the observer (W = 101..112, one lane each).
-- Two more lanes: the enemy drives in (move), the observer drives in.
-- Logs the first game tick army 1 sees each enemy (blip IsSeenNow, and
-- OnIntelChange LOSNow), with absolute game ticks.
--   real game: run_faf.ps1 probe_q_sight 90
--   ours: --headless --no-ai --ticks 260 --script dev/probes/maps/probe_q_sight/probe.lua
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1, A2 = armies[1], armies[2]
SetAlliance(A1, A2, 'Enemy') SetAlliance(A2, A1, 'Enemy')
local function P(s) LOG(fmt('PROBE g%d %s', GetGameTick(), s)) end
local function make(bp, army, x, z)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetFireState(1)
    u:SetMaxHealth(100000) u:SetHealth(nil, 100000)
    return u
end
local LAND = {
    {720,64}, {592,80}, {920,96}, {648,112}, {776,136}, {936,208}, {696,216}, {768,232},
    {664,320}, {904,384}, {656,416}, {568,440}, {456,552}, {368,568}, {120,608},
}
local lanes = {}
local byId = {}
for i = 1, 14 do
    local x, z = LAND[i][1], LAND[i][2]
    local o = make('uel0201', A1, x, z)
    o:SetIntelRadius('Vision', 20)
    local e = make('uel0201', A2, x + 40, z)
    local l = { i = i, o = o, e = e, x = x, z = z }
    if i <= 12 then l.tag = 'warp' .. (100 + i) l.warp = 100 + i
    elseif i == 13 then l.tag = 'enemy-drives'
    else l.tag = 'observer-drives' end
    table.insert(lanes, l)
    byId[e:GetEntityId()] = l
end
local brain = ArmyBrains[1]
local orig = brain.OnIntelChange
brain.OnIntelChange = function(self, blip, reconType, val)
    local ok, src = pcall(function() return blip:GetSource() end)
    local l = ok and src and byId[src:GetEntityId()]
    if l and reconType == 'LOSNow' and not l.ic then
        l.ic = GetGameTick()
        P(fmt('%s OnIntelChange LOSNow %s', l.tag, tostring(val)))
    end
    if orig then return orig(self, blip, reconType, val) end
end
local function dist(l)
    local a, b = l.o:GetPosition(), l.e:GetPosition()
    return math.sqrt((a[1] - b[1]) * (a[1] - b[1]) + (a[3] - b[3]) * (a[3] - b[3]))
end
ForkThread(function()
    while GetGameTick() < 230 do
        local g = GetGameTick()
        for _, lane in lanes do
            local l = lane
            if l.e:BeenDestroyed() or l.o:BeenDestroyed() then
                if not l.gone then l.gone = true P(l.tag .. " unit gone") end
            else
            if l.warp and g == l.warp then
                Warp(l.e, { l.x + 10, GetTerrainHeight(l.x + 10, l.z), l.z })
                P(fmt('%s warped dist %.2f', l.tag, dist(l)))
            end
            if g == 100 and l.tag == 'enemy-drives' then IssueMove({ l.e }, { l.x, GetTerrainHeight(l.x, l.z), l.z }) P(l.tag .. ' move') end
            if g == 100 and l.tag == 'observer-drives' then IssueMove({ l.o }, { l.x + 40, GetTerrainHeight(l.x + 40, l.z), l.z }) P(l.tag .. ' move') end
            local okb, b = pcall(function() return l.e:GetBlip(1) end)
            local seen = false
            if okb and b then
                local oks, s = pcall(function() return b:IsSeenNow(1) end)
                seen = oks and s or false
            end
            if seen and not l.seen then
                l.seen = g
                P(fmt('%s first IsSeenNow dist %.2f', l.tag, dist(l)))
            end
            if g >= 100 and g <= 200 and (l.tag == 'enemy-drives' or l.tag == 'observer-drives') and not l.seen then
                local d = dist(l)
                if d < 21 and not l.inr then l.inr = g P(fmt('%s within 20 dist %.2f', l.tag, d)) end
            end
            end
        end
        WaitTicks(1)
    end
    P('done')
end)
