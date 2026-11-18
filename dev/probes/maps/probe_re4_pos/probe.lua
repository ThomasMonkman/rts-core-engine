-- Differential probe: when and how GetPosition's shared table is updated.
local fmt = string.format
local function P(s) LOG('PROBE ' .. s) end
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1 = armies[1]
local function make(bp, x, z)
    local u = CreateUnitHPR(bp, A1, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    if u.SetFireState then u:SetFireState(1) end
    return u
end
local function s(t) return fmt('%.2f,%.2f,%.2f', t[1], t[2], t[3]) end
ForkThread(function()
    WaitTicks(5)
    P('start')
    -- [A] a moving tank: read the kept table with NO GetPosition call by this
    -- script in between; a counter on the instance sees method-style calls by FAF.
    local u = make('uel0201', 720, 64)
    local u2 = make('uel0201', 730, 64)
    local calls = 0
    local raw = u.GetPosition
    u.GetPosition = function(self) calls = calls + 1 return raw(self) end
    local ta = raw(u)
    local x0 = ta[1]
    local tb0 = raw(u2)
    P(fmt('A two-units-same-table %s', tostring(ta == tb0)))
    IssueMove({ u }, { 760, GetTerrainHeight(760, 64), 64 })
    for i = 1, 6 do
        WaitTicks(5)
        local x, y, z = u:GetPositionXYZ()
        P(fmt('A t+%d kept.x %.2f xyz.x %.2f lua-calls %d', i * 5, ta[1], x, calls))
    end
    local tb = raw(u)
    P(fmt('A after-call same %s kept.x %.2f fresh.x %.2f start.x %.2f', tostring(ta == tb), ta[1], tb[1], x0))
    -- [B] writes to the table
    ta[1] = 999
    local x = u:GetPositionXYZ()
    P(fmt('B wrote 999: kept.x %.2f xyz.x %.2f', ta[1], x))
    WaitTicks(3)
    P(fmt('B 3 ticks later, no call: kept.x %.2f', ta[1]))
    local tc = raw(u)
    P(fmt('B after call: kept.x %.2f same %s', ta[1], tostring(tc == ta)))
    -- extra fields on the table survive?
    local okt, et = pcall(function() ta.tag = 'mine' end)
    local ok4, e4 = pcall(function() ta[4] = 7 end)
    local td = raw(u)
    P(fmt('B extra fields: set-tag ok %s set-4 ok %s [4] %s meta %s x %s', tostring(okt), tostring(ok4), tostring(rawget(td, 4)), tostring(getmetatable(td) ~= nil), tostring(td.x)))
    local plain = { 1, 2, 3 }
    P(fmt('B plain table meta %s; Vector() meta %s same-meta %s', tostring(getmetatable(plain) ~= nil), tostring(getmetatable(Vector(1,2,3)) ~= nil), tostring(getmetatable(Vector(1,2,3)) == getmetatable(td))))
    -- [C] other getters
    local okc, c = pcall(function() return u:GetCachePosition() end)
    P(fmt('C GetCachePosition ok %s same-as-GetPosition %s', tostring(okc), tostring(okc and c == ta)))
    local okn, n = pcall(function() return u:GetNavigator():GetCurrentTargetPos() end)
    local okn2, n2 = pcall(function() return u:GetNavigator():GetCurrentTargetPos() end)
    P(fmt('C navigator target same-table-twice %s', tostring(okn and okn2 and n == n2)))
    local okb, eb = pcall(function()
        local b1, b2 = raw(u, 'Turret'), raw(u, 'Turret')
        P(fmt('C GetPosition(bone) same-twice %s same-as-plain %s', tostring(b1 == b2), tostring(b1 == ta)))
        local k = raw(u)
        P(fmt('C plain after bone call same-as-kept %s', tostring(k == ta)))
    end)
    if not okb then P('C bone err ' .. tostring(eb)) end
    local oko, eo = pcall(function()
        local o1, o2 = u:GetOrientation(), u:GetOrientation()
        P(fmt('C GetOrientation same-twice %s', tostring(o1 == o2)))
    end)
    if not oko then P('C orient err ' .. tostring(eo)) end
    -- [D] a stationary unit then Warp, no call
    local sx = raw(u2)
    Warp(u2, { 700, GetTerrainHeight(700, 80), 80 })
    P(fmt('D after Warp same tick, no call: kept %s', s(sx)))
    WaitTicks(2)
    P(fmt('D 2 ticks later, no call: kept %s', s(sx)))
    local sy = raw(u2)
    P(fmt('D after call: kept %s same %s', s(sx), tostring(sx == sy)))
    -- [E] projectile and prop
    local p = u2:CreateProjectile('/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp', 0, 5, 0, 0, 1, 0)
    p:SetVelocity(2)
    local pa = p:GetPosition()
    local py = pa[2]
    WaitTicks(5)
    P(fmt('E projectile no call: then %.2f kept %.2f', py, pa[2]))
    local pb = p:GetPosition()
    P(fmt('E projectile after call: kept %.2f same %s', pa[2], tostring(pa == pb)))
    -- [F] destroyed unit
    local dk = raw(u)
    u:Destroy()
    WaitTicks(2)
    P(fmt('F destroyed: kept %s', s(dk)))
    -- [G] platoon / brain getters
    local brain = GetArmyBrain(A1)
    local pl = brain:MakePlatoon('', '')
    brain:AssignUnitsToPlatoon(pl, { u2 }, 'Attack', 'None')
    local g1, g2 = pl:GetPlatoonPosition(), pl:GetPlatoonPosition()
    P(fmt('G GetPlatoonPosition same-twice %s same-as-unit %s', tostring(g1 == g2), tostring(g1 == raw(u2))))
    WaitTicks(30)
    P('done')
end)
