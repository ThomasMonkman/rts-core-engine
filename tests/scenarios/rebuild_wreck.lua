-- Scenario test (tests/scenarios.rs): an engineer building a unit on a wreck
-- of the same unit rebuilds it: moho asks GetRebuildBonus, FAF's
-- SetRebuildProgress gives the head start from the wreck, the wreck goes
-- (it isn't reclaimed first).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 50000) b:GiveResource('ENERGY', 500000)
    local bp = __blueprints['ueb1101']
    local x, z = 410.5, 600.5
    -- Where a 1 x 1 structure stands: build sites snap to a cell centre.
    local pos = { x, GetTerrainHeight(x, z), z }
    local wreck = import('/lua/wreckage.lua').CreateWreckage(bp, pos, { 0, 0, 0, 1 }, 50, 0, 1)
    WaitTicks(1)
    local asked = 0
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local orig = eng.GetRebuildBonus
    eng.GetRebuildBonus = function(self, b) asked = asked + 1 return orig(self, b) end
    local reclaimed = false
    eng.OnStartReclaim = function(self, t) reclaimed = true end
    IssueBuildMobile({ eng }, pos, 'ueb1101', {})
    local built
    for _ = 1, 200 do
        WaitTicks(1)
        built = eng:GetFocusUnit()
        if built then break end
    end
    check(built ~= nil, 'the build started')
    check(asked == 1, 'GetRebuildBonus asked once: ' .. asked)
    check(not reclaimed, 'the wreck was not reclaimed first')
    check(wreck:BeenDestroyed(), 'the wreck is gone')
    if built then
        check(built:GetHealth() > 1, 'a head start from the wreck: health ' .. built:GetHealth())
    end
    ScenarioResult.done = true
end)
