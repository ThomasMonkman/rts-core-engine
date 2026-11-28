-- Scenario test (tests/scenarios.rs): a weapon doesn't fire while its unit is
-- Busy (moho's CanFire), which FAF's firing state holds through a salvo. Aeon
-- T1 AA (uab2104: reload 6 ticks, 3 shots 0.2 s apart) fires every 7 ticks
-- with every salvo whole (probe_wpn_aa_hold: `fire 6,13,20,27`, 75 shots); on
-- the reload clock alone OnFire landed mid-salvo every other time and the
-- salvo was lost (45 shots).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) LOG('SCN FAIL ', msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local x, z = 400.5, 600.5
    local aa = CreateUnitHPR('uab2104', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local spot = { x, GetTerrainHeight(x, z + 12), z + 12 }
    local t = CreateUnitHPR('uea0203', 'ARMY_2', x, GetTerrainHeight(x, z + 12), z + 12, 0, 0, 0)
    t:SetMaxHealth(1e6) t:SetHealth(nil, 1e6)
    t:SetFireState(1)
    IssueMove({ t }, spot)
    local w = aa:GetWeapon(1)
    local shots = {}
    local create = w.CreateProjectileAtMuzzle
    w.CreateProjectileAtMuzzle = function(self, muzzle)
        table.insert(shots, GetGameTick())
        return create(self, muzzle)
    end
    for i = 1, 12 do
        WaitTicks(10)
        IssueClearCommands({ t }) IssueMove({ t }, spot)
    end
    local n = table.getn(shots)
    check(n > 0, 'the AA gun fired')
    if n > 0 then
        local first, last = shots[1], shots[n]
        -- every salvo whole: 3 shots per 7 ticks
        local span = last - first
        local expect = math.floor(span / 7) * 3
        note(string.format('%d shots over %d ticks', n, span))
        check(n >= expect, string.format('whole salvos: %d shots over %d ticks (want >= %d)', n, span, expect))
        local gaps = {}
        for i = 4, n, 3 do table.insert(gaps, shots[i] - shots[i - 3]) end
        check(gaps[1] == 7, 'salvos start 7 ticks apart: ' .. table.concat(gaps, ','))
    end
    ScenarioResult.done = true
end)
