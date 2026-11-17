-- Scenario test (tests/scenarios.rs): plain GetPosition() hands back one
-- table per entity, rewritten only by the next call (dev/probes probe_re4_pos,
-- measured in the real game). FAF's AIBehaviors and AI mods keep the table as
-- a live reference and compare kept tables by identity.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function done() ScenarioResult.done = true end
ForkThread(function()
    WaitTicks(2)
    local function make(x, z) return CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0) end
    local u, u2 = make(400, 570), make(410, 570)
    local GetPosition = u.GetPosition
    local kept = GetPosition(u)
    check(GetPosition(u) == kept, 'identity: same table on every call')
    check(GetPosition(u2) ~= kept, 'two entities have different tables')
    check(kept.x == kept[1], 'vector metatable (.x)')
    check(getmetatable(kept) == getmetatable(Vector(1, 2, 3)), 'same metatable as Vector()')

    -- Rewritten only by a call.
    local x0 = kept[1]
    Warp(u, { 420, GetTerrainHeight(420, 580), 580 })
    WaitTicks(3)
    check(kept[1] == x0, 'not updated without a call (got ' .. kept[1] .. ')')
    local again = GetPosition(u)
    check(again == kept and math.abs(kept[1] - 420) < 1e-3, 'updated by the next call (got ' .. kept[1] .. ')')

    -- Lua writes last until the next call, which also drops extras.
    kept[1] = 999
    kept[4] = 7
    WaitTicks(3)
    check(kept[1] == 999, 'a write persists until the next call')
    GetPosition(u)
    check(math.abs(kept[1] - 420) < 1e-3 and rawget(kept, 4) == nil, 'the next call rewrites it')

    -- A bone's position is a new table and leaves the shared one alone.
    kept[1] = 5
    local b1, b2 = GetPosition(u, 'Turret'), GetPosition(u, 'Turret')
    check(b1 ~= b2 and b1 ~= kept, 'GetPosition(bone): a new table each call')
    check(kept[1] == 5, 'a bone call leaves the shared table alone')
    check(GetPosition(u, 0) ~= kept, 'GetPosition(0): a new table')
    check(u:GetCachePosition() == kept, 'GetCachePosition: the shared table')
    check(u:GetOrientation() ~= u:GetOrientation(), 'GetOrientation: new tables')

    -- Projectiles too.
    local p = u2:CreateProjectile('/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp', 0, 5, 0, 0, 1, 0)
    local pk = p:GetPosition()
    check(p:GetPosition() == pk, 'projectile: same table')

    -- A destroyed entity's kept table keeps its last values.
    local last = GetPosition(u)[1]
    u:Destroy()
    WaitTicks(2)
    check(kept[1] == last, 'destroyed: keeps its last values')
    done()
end)
