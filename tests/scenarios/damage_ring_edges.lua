-- Scenario test (tests/scenarios.rs): `DamageRing`'s edges against a wall
-- (collision box half 0.5), as in the real game (dev/probes probe_q_api): a
-- face exactly on the inner radius is hit, one exactly on the outer radius
-- is not.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local wall = CreateUnitHPR('ueb5101', 'ARMY_2', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    wall:SetMaxHealth(1000000) wall:SetHealth(nil, 1000000)
    local hits = 0
    wall.OnDamage = function() hits = hits + 1 end
    local p = wall:GetPosition()
    local function ring(d, along_z)
        local h0 = hits
        if along_z then
            DamageRing(nil, { p[1], p[2], p[3] + d }, 3, 6, 1, 'Normal', true, true)
        else
            DamageRing(nil, { p[1] + d, p[2], p[3] }, 3, 6, 1, 'Normal', true, true)
        end
        return hits > h0
    end
    -- The real game's rows: wall-x 3.499:- 3.500:H 6.499:H 6.500:-, wall-z the same.
    for _, z in { false, true } do
        local axis = z and 'z' or 'x'
        check(not ring(3.375, z), axis .. ': inside the inner sphere is missed')
        check(ring(3.5, z), axis .. ': a face exactly on the inner radius is hit')
        check(ring(3.625, z), axis .. ': just outside the inner radius is hit')
        check(ring(6.375, z), axis .. ': just inside the outer radius is hit')
        check(not ring(6.5, z), axis .. ': a face exactly on the outer radius is missed')
        check(not ring(6.625, z), axis .. ': beyond the outer radius is missed')
    end
    note('hits ' .. hits)
    ScenarioResult.done = true
end)
