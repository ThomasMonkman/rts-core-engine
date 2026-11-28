-- Probe: what Unit:AlterArmor(damageType, multiplier) does.
-- For a structure (armour 'Structure': Normal 1, Overcharge 0.25, Deathnuke
-- 0.032) and a tank (armour 'Normal': Normal 1): the multiplier
-- GetArmorMult reports and the health a 10 damage hit takes, before and
-- after AlterArmor, for a listed type and an unlisted one; whether a second
-- call replaces or stacks; whether another unit of the same blueprint is
-- changed; odd arguments.
local armies = ListArmies()
local army = armies[1]
local brain = GetArmyBrain(army)
local sx, sz = brain:GetArmyStartPos()

local function hit(u, kind)
    local before = u:GetHealth()
    Damage(nil, u:GetPosition(), u, 10, kind)
    return before - u:GetHealth()
end
local function show(tag, u, kinds)
    for _, k in kinds do
        LOG('PROBE ', tag, ' ', k, ' mult=', u:GetArmorMult(k), ' hit10=', hit(u, k))
    end
end
local function try(tag, u, ...)
    local ok, err = pcall(u.AlterArmor, u, unpack({ ... }))
    LOG('PROBE ', tag, ' AlterArmor ok=', tostring(ok), ' ret=', tostring(err))
end

local function case(tag, bp, ox, kinds)
    local x, z = sx + ox, sz + 30
    local a = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local b = CreateUnitHPR(bp, army, x + 8, GetTerrainHeight(x + 8, z), z, 0, 0, 0)
    a:SetRegenRate(0) b:SetRegenRate(0)
    WaitTicks(5)
    show(tag .. ' start a', a, kinds)
    for _, k in kinds do
        try(tag .. ' ' .. k .. ' 0.5', a, k, 0.5)
    end
    show(tag .. ' half a', a, kinds)
    show(tag .. ' half b', b, kinds)
    for _, k in kinds do
        try(tag .. ' ' .. k .. ' 0.5 again', a, k, 0.5)
    end
    show(tag .. ' again a', a, kinds)
    for _, k in kinds do a:AlterArmor(k, 2) end
    show(tag .. ' two a', a, kinds)
    for _, k in kinds do a:AlterArmor(k, 0) end
    show(tag .. ' zero a', a, kinds)
    for _, k in kinds do a:AlterArmor(k, -1) end
    show(tag .. ' neg a', a, kinds)
    for _, k in kinds do a:AlterArmor(k, 1) end
    show(tag .. ' one a', a, kinds)
    try(tag .. ' no mult', a, 'Normal')
    show(tag .. ' nomult a', a, { 'Normal' })
    try(tag .. ' no args', a)
    try(tag .. ' string mult', a, 'Normal', '0.5')
    show(tag .. ' strmult a', a, { 'Normal' })
    try(tag .. ' lowercase', a, 'normal', 0.25)
    show(tag .. ' lower a', a, { 'Normal', 'normal' })
    -- a new unit of the same blueprint, made after the changes
    local c = CreateUnitHPR(bp, army, x + 16, GetTerrainHeight(x + 16, z), z, 0, 0, 0)
    WaitTicks(2)
    show(tag .. ' new c', c, kinds)
end

ForkThread(function()
    WaitTicks(10)
    case('struct', 'ueb1101', 30, { 'Normal', 'Overcharge', 'Fire' })
    case('tank', 'uel0201', 30 + 40, { 'Normal', 'Overcharge' })
    LOG('PROBE done')
end)
