-- Scenario test (tests/scenarios.rs): ordering a structure onto a unit makes
-- the unit move clear before construction starts, and the build waits for the
-- site. An engineer building over a parked unit must not spawn the
-- structure on top of it — nor over its own position: the builder itself then
-- walks clear before the structure spawns. Air units about the site are left
-- alone: a hovering interceptor never holds the build.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
    local sx, sz = 410, 600
    -- Builder already in range, so it holds still and the only thing that
    -- moves is the unit the structure lands on.
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 406, GetTerrainHeight(406, 600), 600, 0, 0, 0)
    local blocker = CreateUnitHPR('uel0105', 'ARMY_1', sx, GetTerrainHeight(sx, sz), sz, 0, 0, 0)
    local start = blocker:GetPosition()
    local startd = math.pow(start[1] - sx, 2) + math.pow(start[3] - sz, 2)
    note(string.format('the parked unit starts %.2f from the site', math.sqrt(startd)))
    local moved = false
    IssueBuildMobile({ eng }, Vector(sx, 0, sz), 'ueb1101', {})
    local built, overlap_at_start
    for _ = 1, 300 do
        WaitTicks(1)
        local q = blocker:GetPosition()
        if not moved and math.pow(q[1] - sx, 2) + math.pow(q[3] - sz, 2) > startd + 0.25 then moved = true end
        local s = eng.UnitBeingBuilt
        if s and not s.Dead and not built then
            built = s
            local h = built:GetPosition()
            local dx = math.max(math.abs(q[1] - h[1]) - 0.375, 0)
            local dz = math.max(math.abs(q[3] - h[3]) - 0.375, 0)
            overlap_at_start = dx * dx + dz * dz < 0.5 * 0.5
        end
        if built and built:GetFractionComplete() >= 1 then break end
    end
    local q = blocker:GetPosition()
    local d2 = math.pow(q[1] - sx, 2) + math.pow(q[3] - sz, 2)
    note(string.format('the parked unit ends %.2f from the site', math.sqrt(d2)))
    check(moved, 'the unit standing in the footprint was ordered out')
    check(built ~= nil and built:GetFractionComplete() >= 1, 'the structure was built')
    check(overlap_at_start ~= true, 'the structure did not start on top of the unit')
    check(d2 >= 1.0, 'the unit ended clear of the structure')
    -- The builder itself: a structure centered on the builder's own position
    -- (its factory would spawn in its lap) — the builder walks out first.
    local ox, oz = 420, 590
    local selfer = CreateUnitHPR('uel0105', 'ARMY_1', ox, GetTerrainHeight(ox, oz), oz, 0, 0, 0)
    local sstart = selfer:GetPosition()
    local sstartd = math.pow(sstart[1] - ox, 2) + math.pow(sstart[3] - oz, 2)
    note(string.format('the builder starts %.2f from its own site', math.sqrt(sstartd)))
    local smoved = false
    local sbuilt, overlap_at_start
    IssueBuildMobile({ selfer }, Vector(ox, 0, oz), 'ueb1101', {})
    for _ = 1, 400 do
        WaitTicks(1)
        local q = selfer:GetPosition()
        if not smoved and math.pow(q[1] - ox, 2) + math.pow(q[3] - oz, 2) > sstartd + 0.25 then smoved = true end
        local s = selfer.UnitBeingBuilt
        if s and not s.Dead and not sbuilt then
            sbuilt = s
            local h = sbuilt:GetPosition()
            local dx = math.max(math.abs(q[1] - h[1]) - 0.375, 0)
            local dz = math.max(math.abs(q[3] - h[3]) - 0.375, 0)
            overlap_at_start = dx * dx + dz * dz < 0.5 * 0.5
        end
        if sbuilt and sbuilt:GetFractionComplete() >= 1 then break end
    end
    local q = selfer:GetPosition()
    local sd2 = math.pow(q[1] - ox, 2) + math.pow(q[3] - oz, 2)
    note(string.format('the builder ends %.2f from its own site', math.sqrt(sd2)))
    check(smoved, 'the builder ordered over its own position walked out')
    check(sbuilt ~= nil and sbuilt:GetFractionComplete() >= 1, 'the structure over the builder was built')
    check(overlap_at_start ~= true, 'the structure did not start on top of its builder')
    check(sd2 >= 1.0, 'the builder ended clear of its own structure')
    -- Air units fly over a ground site: they are never ordered out and
    -- never hold the build — an interceptor parked on the site is built under.
    local ax, az = 400, 610
    local air = CreateUnitHPR('uea0103', 'ARMY_1', ax, GetTerrainHeight(ax, az), az, 0, 0, 0)
    local astart = air:GetPosition()
    local flyer = CreateUnitHPR('uel0105', 'ARMY_1', 404, GetTerrainHeight(404, 610), 610, 0, 0, 0)
    IssueBuildMobile({ flyer }, Vector(ax, 0, az), 'ueb1101', {})
    local built3
    for _ = 1, 100 do
        WaitTicks(1)
        local s = flyer.UnitBeingBuilt
        if s and not s.Dead then built3 = s end
        if built3 then break end
    end
    local q = air:GetPosition()
    note(string.format('the air unit stayed %.2f from where it parked', math.sqrt(math.pow(q[1] - astart[1], 2) + math.pow(q[3] - astart[3], 2))))
    check(built3 ~= nil, 'the structure under the hovering air unit started building')
    check(math.pow(q[1] - astart[1], 2) + math.pow(q[3] - astart[3], 2) < 1.0, 'the air unit was never ordered out of the site')
    ScenarioResult.done = true
end)
