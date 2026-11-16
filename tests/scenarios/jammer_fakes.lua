-- Scenario test (tests/scenarios.rs): a jammer's fake radar contacts as an
-- enemy radar sees them (FAF probe dev/probes/maps/probe_intel_jam). Each
-- fake is a Blip of its own: OnIntelChange fires for it, GetSource is the
-- jammer, GetBlueprint the jammer's; it sits within JamRadius at a fixed
-- offset that moves with the jammer; omni makes it a known fake without
-- removing it (and FAF's jammer reset hears Omni go); it adds the jammer's
-- threat where it is, never shows in GetUnitsAroundPoint; switching the
-- jammer off destroys every fake, switching it on makes a new set.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    for _, i in { 1, 2 } do
        ArmyBrains[i]:GiveStorage('ENERGY', 500000)
        ArmyBrains[i]:GiveResource('ENERGY', 500000)
    end
    local brain = ArmyBrains[2]
    local fakes, order, lost = {}, {}, {}
    local original = brain.OnIntelChange
    local spark
    brain.OnIntelChange = function(self, blip, reconType, val)
        if spark and blip:GetSource() == spark and blip:GetEntityId() ~= spark:GetEntityId() then
            local id = blip:GetEntityId()
            if not fakes[id] then fakes[id] = blip table.insert(order, id) end
            if reconType == 'Radar' and not val then lost[id] = true end
        end
        if original then original(self, blip, reconType, val) end
    end
    spark = CreateUnitHPR('xel0209', 'ARMY_1', 430, GetTerrainHeight(430, 600), 600, 0, 0, 0)
    local radar = CreateUnitHPR('ueb3201', 'ARMY_2', 380, GetTerrainHeight(380, 640), 640, 0, 0, 0)
    -- A second Sparky with its jammer off from the start: the jammer's
    -- threat alone (the first one's old fakes stay remembered while it lives).
    local plain = CreateUnitHPR('xel0209', 'ARMY_1', 300, GetTerrainHeight(300, 760), 760, 0, 0, 0)
    plain:DisableIntel('Jammer')
    for _, u in { spark, radar, plain } do u:SetFireState(1) end
    spark:EnableIntel('Jammer')
    local radius = spark:GetBlueprint().Intel.JamRadius.Max
    local count = spark:GetBlueprint().Intel.JammerBlips
    for _ = 1, 150 do
        if table.getn(order) >= count then break end
        WaitTicks(1)
    end
    check(table.getn(order) == count, 'one OnIntelChange blip per JammerBlips fake: ' .. table.getn(order) .. ' of ' .. count)
    local function offsets()
        local out, jp = {}, spark:GetPosition()
        for i, id in order do
            local p = fakes[id]:GetPosition()
            out[i] = { p[1] - jp[1], p[3] - jp[3] }
        end
        return out
    end
    local first = offsets()
    for i, id in order do
        local b = fakes[id]
        check(b:GetBlueprint().BlueprintId == 'xel0209', "a fake has its jammer's blueprint")
        check(b:IsOnRadar(2) and not b:IsSeenNow(2) and not b:IsKnownFake(2), 'a fake is on radar, unseen, not known fake')
        check(not b:BeenDestroyed(), 'a fake is alive')
        local d = math.sqrt(first[i][1] * first[i][1] + first[i][2] * first[i][2])
        check(d <= radius + 0.01, 'a fake within JamRadius: ' .. d)
    end
    local around = brain:GetUnitsAroundPoint(categories.ALLUNITS, spark:GetPosition(), 60, 'Enemy') or {}
    check(table.getn(around) == 1, 'GetUnitsAroundPoint lists the jammer only: ' .. table.getn(around))

    -- Threat: the jammer's own, plus one more per fake (every fake within a
    -- cell of it). The threat map updates every 30 ticks.
    WaitTicks(31)
    local withFakes = brain:GetThreatAtPosition(spark:GetPosition(), 1, true, 'Overall', 1)

    -- Moved (Warp): the fakes follow at the same offsets.
    Warp(spark, { 440, GetTerrainHeight(440, 600), 600 })
    WaitTicks(2)
    local moved = offsets()
    local same = true
    for i in first do
        if math.abs(moved[i][1] - first[i][1]) > 0.01 or math.abs(moved[i][2] - first[i][2]) > 0.01 then same = false end
    end
    check(same, 'fakes keep their offsets as the jammer moves')

    -- Omni: known fakes, still alive and on radar; FAF's reset hears Omni go.
    local omni = CreateUnitHPR('ueb3104', 'ARMY_2', 360, GetTerrainHeight(360, 560), 560, 0, 0, 0)
    omni:SetFireState(1)
    WaitTicks(15)
    for _, id in order do
        local b = fakes[id]
        check(b:IsKnownFake(2) and b:IsOnOmni(2) and b:IsOnRadar(2), 'under omni a fake is a known fake, still on radar')
        check(not b:IsSeenNow(2), 'omni does not see a fake')
        check(not b:BeenDestroyed(), 'and stays alive')
    end
    omni:Destroy()
    WaitTicks(3)
    check((spark.ResetJammer or -1) > 0, "FAF's jammer reset is counting down: " .. tostring(spark.ResetJammer))
    check(fakes[order[1]]:IsKnownFake(2), 'still a known fake once omni is gone')

    -- Off: every fake destroyed, Radar lost on each; on: a new set.
    local hooked = 0
    for _, id in order do fakes[id]:AddDestroyHook(function() hooked = hooked + 1 end) end
    spark:DisableIntel('Jammer')
    WaitTicks(2)
    local gone, heard = 0, 0
    for _, id in order do
        if fakes[id]:BeenDestroyed() then gone = gone + 1 end
        if lost[id] then heard = heard + 1 end
    end
    check(gone == count, 'jammer off: every fake destroyed: ' .. gone)
    check(heard == count, 'jammer off: Radar lost on every fake: ' .. heard)
    check(hooked == count, 'jammer off: destroy hooks ran: ' .. hooked)
    local before = table.getn(order)
    spark:EnableIntel('Jammer')
    WaitTicks(3)
    check(table.getn(order) == before + count, 'jammer on: a fresh set: ' .. (table.getn(order) - before))
    -- The jammer's threat alone (fakes gone) against the earlier reading
    -- (probe_intel_jam: Sparky 51.93 alone, 467.37 with its 8 fakes).
    spark:DisableIntel('Jammer')
    WaitTicks(31)
    local single = brain:GetThreatAtPosition(plain:GetPosition(), 1, true, 'Overall', 1)
    check(single > 0 and math.abs(withFakes - single * (count + 1)) < 0.01 * single, 'each fake adds the jammer threat: ' .. withFakes .. ' vs ' .. single)
    ScenarioResult.done = true
end)
