-- Probe: two things an explosion mod leans on.
-- 1. Does an Entity made with a spec table carry that spec (`e.Spec`)?
-- 2. Does CreateLightParticle take a table where the texture name goes?
local function keys(t)
    local s = ''
    for k, v in t do s = s .. tostring(k) .. '=' .. tostring(v) .. ' ' end
    return s
end
ForkThread(function()
    WaitTicks(10)
    local army = ListArmies()[1]
    local brain = GetArmyBrain(army)
    local sx, sz = brain:GetArmyStartPos()
    local Entity = import('/lua/sim/Entity.lua').Entity
    local spec = { Army = brain:GetArmyIndex(), Foo = 7 }
    local e = Entity(spec)
    LOG('PROBE entity Spec=', tostring(e.Spec), ' same=', tostring(e.Spec == spec), ' Foo=', tostring(e.Foo), ' Army=', tostring(e.Army), ' keys: ', keys(e))
    local ok, err = pcall(function() return e.Spec.Army end)
    LOG('PROBE e.Spec.Army ok=', tostring(ok), ' ', tostring(err))
    ok, err = pcall(function() local t = nil return t.x end)
    LOG('PROBE index-nil ok=', tostring(ok), ' ', tostring(err))

    local u = CreateUnitHPR('uel0201', army, sx + 10, GetTerrainHeight(sx + 10, sz + 10), sz + 10, 0, 0, 0)
    local tex = '/textures/particles/flare_lens_add_02.dds'
    for name, arg in { string = tex, table1 = { tex }, table2 = { tex, tex }, empty = {}, number = 5, boolean = true } do
        local n = name
        local ok2, r = pcall(function() return CreateLightParticle(u, -1, brain:GetArmyIndex(), 8, 60, arg, 'ramp_flare_02') end)
        LOG('PROBE CreateLightParticle texture=', n, ' ok=', tostring(ok2), ' ', tostring(r))
    end
    ok, err = pcall(function() return CreateLightParticle(u, -1, brain:GetArmyIndex(), 8, 60, tex, { 'ramp_flare_02' }) end)
    LOG('PROBE CreateLightParticle ramp=table ok=', tostring(ok), ' ', tostring(err))
    LOG('PROBE done')
end)
