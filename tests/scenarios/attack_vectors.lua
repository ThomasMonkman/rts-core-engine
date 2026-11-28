-- Scenario test (tests/scenarios.rs): attack vectors around the current
-- enemy's structures, read the way FAF's AI brains do (GetBaseVectors,
-- adaptive-ai.lua:851) and picked for a platoon.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local me, enemy = GetArmyBrain('ARMY_1'), GetArmyBrain('ARMY_2')
    -- A power generator at (400, 592): the centre of cell (12, 18).
    CreateUnitHPR('ueb1101', 'ARMY_2', 400, GetTerrainHeight(400, 592), 592, 0, 0, 0)
    -- A tank is MOBILE, so the default category leaves it out.
    CreateUnitHPR('uel0201', 'ARMY_2', 600, GetTerrainHeight(600, 600), 600, 0, 0, 0)
    WaitTicks(2)
    check(me:GetAttackVectors() == nil, 'no vectors before set-up')
    me:SetUpAttackVectorsToArmy()
    check(me:GetAttackVectors() == nil, 'no current enemy: nothing set up')
    me:SetCurrentEnemy(enemy)
    me:SetUpAttackVectorsToArmy()
    local vecs = me:GetAttackVectors()
    check(vecs ~= nil and table.getn(vecs) == 8, 'eight vectors ring the generator: ' .. tostring(vecs and table.getn(vecs)))
    if vecs then
        for _, v in vecs do
            check(v.px + v.vx == 400 and v.pz + v.vz == 592 and v.py == 0 and v.vy == 0,
                string.format('vector points into the generator cell: %d,%d + %d,%d', v.px, v.pz, v.vx, v.vz))
        end
    end
    -- GetBaseVectors, as FAF's brains use it (current enemy set to self).
    local GetBaseVectors = import('/lua/aibrains/adaptive-ai.lua').AIBrain.GetBaseVectors
    local ok, points = pcall(GetBaseVectors, me)
    check(ok, 'GetBaseVectors runs: ' .. tostring(points))
    check(ok and me:GetCurrentEnemy() == enemy, 'GetBaseVectors restores the current enemy')
    -- PickBestAttackVector: every vector leads to the generator, so with a
    -- category the first (row order) wins: from cell (11, 17).
    me:SetUpAttackVectorsToArmy()
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 376, GetTerrainHeight(376, 560), 560, 0, 0, 0)
    local platoon = me:MakePlatoon('', '')
    me:AssignUnitsToPlatoon(platoon, { tank }, 'Attack', 'None')
    -- A platoon made this tick (or the one before) gets nothing yet, as in
    -- the real game (dev/probes probe_re3_ai AV L1).
    check(me:PickBestAttackVector(platoon, 'Attack', 'Enemy', 'Closest', categories.STRUCTURE) == nil, 'a new platoon gets nil')
    WaitTicks(1)
    check(me:PickBestAttackVector(platoon, 'Attack', 'Enemy', 'Closest', categories.STRUCTURE) == nil, 'a platoon made the tick before gets nil')
    WaitTicks(2)
    -- Closest, Furthest and LeastDefended all give the first vector
    -- (dev/probes probe_re3_ai2, probe_re3_ai3).
    for _, cmp in { 'Closest', 'Furthest', 'LeastDefended' } do
        local best = me:PickBestAttackVector(platoon, 'Attack', 'Enemy', cmp, categories.STRUCTURE)
        check(best ~= nil, cmp .. ': a vector is picked')
        if best then
            note(string.format('%s picked %d,%d + %d,%d', cmp, best.px, best.pz, best.vx, best.vz))
            check(best.px == 368 and best.pz == 560, cmp .. ': the first vector')
        end
    end
    local ok7 = pcall(me.PickBestAttackVector, me, platoon, 'Attack', 'Enemy', 'Closest', categories.STRUCTURE, '/lua/x.lua')
    check(not ok7, '7 arguments is an error')
    ScenarioResult.done = true
end)
