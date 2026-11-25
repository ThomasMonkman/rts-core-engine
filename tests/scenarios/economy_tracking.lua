-- Scenario test (tests/scenarios.rs): OnProductionActive / InActive and
-- OnConsumptionActive / InActive when the setters change the state, and a
-- weapon's OnStartTracking(label) / OnStopTracking(label) as its turret turns.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local calls = {}
    local pgen = CreateUnitHPR('ueb1101', 'ARMY_1', 380, GetTerrainHeight(380, 560), 560, 0, 0, 0)
    for _, name in { 'OnProductionActive', 'OnProductionInActive', 'OnConsumptionActive', 'OnConsumptionInActive' } do
        local orig = pgen[name]
        pgen[name] = function(self) table.insert(calls, name); return orig(self) end
    end
    WaitTicks(2)
    calls = {}
    pgen:SetProductionActive(false)
    pgen:SetProductionActive(false)
    pgen:SetProductionActive(true)
    pgen:SetConsumptionActive(true)
    pgen:SetConsumptionActive(false)
    local got = table.concat(calls, ' ')
    check(got == 'OnProductionInActive OnProductionActive OnConsumptionActive OnConsumptionInActive',
        'one call per change: ' .. got)

    -- Tracking follows the turret, not the target (dev/probes probe_q_track):
    -- start on the first tick it turns, stop when it comes to rest on its
    -- aim, and another pair for the swing back once the target is gone.
    local function watch(x, z, ex, ez)
        local tank = CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
        local w = tank:GetWeapon(1)
        local seen = {}
        local start, stop = w.OnStartTracking, w.OnStopTracking
        w.OnStartTracking = function(self, label) table.insert(seen, 'start:' .. tostring(label) .. ':' .. tostring(self:WeaponHasTarget())); return start(self, label) end
        w.OnStopTracking = function(self, label) table.insert(seen, 'stop:' .. tostring(label) .. ':' .. tostring(self:WeaponHasTarget())); return stop(self, label) end
        local enemy = CreateUnitHPR('uel0201', 'ARMY_2', ex, GetTerrainHeight(ex, ez), ez, 0, 0, 0)
        enemy:SetFireState(1)
        enemy:SetMaxHealth(1000000) enemy:SetHealth(nil, 1000000)
        return { seen = seen, enemy = enemy }
    end
    local side = watch(400, 600, 412, 600)
    WaitTicks(40)
    check(table.concat(side.seen, ' ') == 'start:Default:true stop:Default:true',
        'a target to the side: start, then stop on reaching the aim, target still held: ' .. table.concat(side.seen, ' '))
    side.enemy:Destroy()
    WaitTicks(40)
    check(table.concat(side.seen, ' ') == 'start:Default:true stop:Default:true start:Default:false stop:Default:false',
        'the swing back to rest is another pair: ' .. table.concat(side.seen, ' '))
    ScenarioResult.done = true
end)
