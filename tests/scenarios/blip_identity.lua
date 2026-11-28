-- Scenario test (tests/scenarios.rs): an army's blip of a unit is one object
-- of FAF's Blip class (moho's ReconBlip makes its Lua object from
-- /lua/sim/Blip.lua): GetBlip returns it each time, fields set on it stay,
-- and OnIntelChange passes the same one. Moho keeps a blip until its unit
-- dies in its army's intel, then destroys it (OnDestroy runs the destroy
-- hooks); a tank dying out of sight keeps its blip, a structure dying out of
-- sight is "maybe dead" until the army sees the spot (FAF probe probe_blip).
-- The engine's
-- cache holds a blip weakly until a script stores a field on it: one a
-- script keeps, or wrote to, survives a collection (sim::gc collects every
-- 100 ticks), one nothing keeps or touched is collected.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local heard = {}
    local brain = ArmyBrains[2]
    local original = brain.OnIntelChange
    brain.OnIntelChange = function(self, blip, reconType, val)
        heard[blip:GetSource()] = blip
        if original then original(self, blip, reconType, val) end
    end
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local other = CreateUnitHPR('uel0201', 'ARMY_1', 402, GetTerrainHeight(402, 600), 600, 0, 0, 0)
    local noted = CreateUnitHPR('uel0201', 'ARMY_1', 398, GetTerrainHeight(398, 600), 600, 0, 0, 0)
    local wanderer = CreateUnitHPR('uel0201', 'ARMY_1', 404, GetTerrainHeight(404, 604), 604, 0, 0, 0)
    CreateUnitHPR('uel0201', 'ARMY_2', 406, GetTerrainHeight(406, 600), 600, 0, 0, 0)
    local pd = CreateUnitHPR('ueb2101', 'ARMY_1', 360, GetTerrainHeight(360, 600), 600, 0, 0, 0)
    local watcher = CreateUnitHPR('uel0201', 'ARMY_2', 352, GetTerrainHeight(352, 600), 600, 0, 0, 0)
    -- Nobody shoots (army 2 has to keep seeing its targets).
    for _, brain in ArmyBrains do
        for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do u:SetFireState(1) end
    end
    WaitTicks(3)

    local own = tank:GetBlip(1)
    check(own ~= nil, 'army 1 has a blip of its own tank')
    check(rawequal(own, tank:GetBlip(1)), 'GetBlip returns the same blip each time')
    own.Marked = true
    check(tank:GetBlip(1).Marked == true, 'a field set on a blip stays')
    check(own:GetSource() == tank, 'the blip stands for its unit')
    check(getmetatable(own) == import('/lua/sim/Blip.lua').Blip, "blips are FAF's Blip class")
    check(IsBlip(own), 'IsBlip says so')
    local enemy = tank:GetBlip(2)
    check(enemy ~= nil, 'army 2 sees the tank next to its own')
    check(not rawequal(own, enemy), 'each army has its own blip of the unit')
    check(heard[tank] ~= nil, 'army 2 heard OnIntelChange for the tank')
    check(rawequal(heard[tank], enemy), 'OnIntelChange passes the blip GetBlip returns')
    heard = {}

    -- Past a collection: a kept blip is the same, a dropped one is gone.
    local dropped = setmetatable({}, { __mode = 'v' })
    dropped[1] = other:GetBlip(1)
    noted:GetBlip(1).Note = 'kept'
    WaitTicks(110)
    check(rawequal(own, tank:GetBlip(1)), 'a blip a script keeps is the same one after a collection')
    check(dropped[1] == nil, 'a blip nothing keeps or wrote to is collected (the cache holds it weakly)')
    check(noted:GetBlip(1).Note == 'kept', 'a field stored on a dropped blip is still there after a collection')

    -- The unit dies in sight: its blips are destroyed, running their hooks.
    local destroyed
    enemy:AddDestroyHook(function(blip) destroyed = blip end)
    tank:Destroy()
    WaitTicks(2)
    check(destroyed ~= nil, 'a destroy hook runs when the unit dies in sight')
    check(rawequal(destroyed, enemy), 'the hook gets the blip')

    -- A tank dying out of sight keeps its blip: no hook, not maybe dead.
    local lost, lostHooked = wanderer:GetBlip(2), false
    lost:AddDestroyHook(function() lostHooked = true end)
    Warp(wanderer, { 100, GetTerrainHeight(100, 300), 300 })
    WaitTicks(3)
    check(wanderer:GetBlip(2) == nil, 'out of sight, the tank has no blip now')
    check(not lost:IsSeenEver(2), "a mobile unit's dropped blip is not seen ever (probe_re3_intel)")
    wanderer:Destroy()
    WaitTicks(3)
    check(not lostHooked, 'a tank dying out of sight runs no destroy hook')
    check(not lost:IsMaybeDead(2), 'nor is its blip maybe dead')

    -- A structure dying out of sight is maybe dead until the spot is seen.
    local orphan, orphanHooked = pd:GetBlip(2), false
    orphan:AddDestroyHook(function() orphanHooked = true end)
    Warp(watcher, { 300, GetTerrainHeight(300, 520), 520 })
    WaitTicks(3)
    check(orphan:IsSeenEver(2), "a remembered structure's blip stays seen ever")
    pd:Destroy()
    WaitTicks(3)
    check(orphan:IsMaybeDead(2), 'a structure dying out of sight leaves a maybe-dead blip')
    check(not orphanHooked, 'which stays until the army sees the spot')
    Warp(watcher, { 352, GetTerrainHeight(352, 600), 600 })
    WaitTicks(3)
    check(orphanHooked, 'seeing the spot destroys it')
    brain.OnIntelChange = original
    ScenarioResult.done = true
end)
