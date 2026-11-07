-- Scenario test (tests/scenarios.rs): IssueTeleport puts the unit in the
-- Teleporting state and calls OnTeleportUnit(teleporter, location,
-- orientation); FAF's thread warps it and the order completes. A teleport
-- order cleared before it starts the jump gets OnFailedTeleport.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    -- FAF's teleport waits on its energy drain (WaitFor on the economy event).
    local brain = GetArmyBrain('ARMY_1')
    brain:__engine_SetStorage('ENERGY', 100000)
    brain:GiveResource('ENERGY', 100000)
    WaitTicks(2)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    tank:AddCommandCap('RULEUCC_Teleport')
    WaitTicks(2)
    local seen
    local orig = tank.OnTeleportUnit
    tank.OnTeleportUnit = function(self, teleporter, location, orientation)
        seen = { teleporter = teleporter, x = location[1], z = location[3], q = orientation[4], teleporting = self:IsUnitState('Teleporting') }
        return orig(self, teleporter, location, orientation)
    end
    local dest = { 440, GetTerrainHeight(440, 620), 620 }
    IssueTeleport({tank}, dest)
    WaitTicks(2)
    check(seen ~= nil, 'OnTeleportUnit called')
    if seen then
        check(seen.teleporter == tank and seen.x == 440 and seen.z == 620 and seen.q ~= nil, 'arguments: the unit itself as teleporter, the location, an orientation')
        check(seen.teleporting, 'in the Teleporting state when told')
    end
    for _ = 1, 100 do
        WaitTicks(1)
        if table.getn(tank:GetCommandQueue()) == 0 then break end
    end
    local p = tank:GetPosition()
    check(math.abs(p[1] - 440) < 1 and math.abs(p[3] - 620) < 1, string.format('warped to the location: %.1f, %.1f', p[1], p[3]))
    check(table.getn(tank:GetCommandQueue()) == 0, 'teleport order done')
    check(not tank:IsUnitState('Teleporting'), 'Teleporting state cleared')

    -- Cancelled before the jump: an ACU charges for a while.
    local acu = CreateUnitHPR('uel0001', 'ARMY_1', 380, GetTerrainHeight(380, 640), 640, 0, 0, 0)
    acu:AddCommandCap('RULEUCC_Teleport')
    WaitTicks(2)
    local failed = false
    local origFailed = acu.OnFailedTeleport
    acu.OnFailedTeleport = function(self) failed = true; return origFailed(self) end
    IssueTeleport({acu}, { 300, GetTerrainHeight(300, 640), 640 })
    WaitTicks(3)
    check(acu:IsUnitState('Teleporting'), 'ACU charging its teleport')
    IssueClearCommands({acu})
    WaitTicks(2)
    check(failed, 'OnFailedTeleport when the order was cleared')
    check(not acu:IsUnitState('Teleporting'), 'ACU left the Teleporting state')
    local a = acu:GetPosition()
    check(math.abs(a[1] - 380) < 1, string.format('ACU stayed put: %.1f', a[1]))

    -- A teleport to an entity that's gone doesn't hold the queue.
    local gone = CreateUnitHPR('uel0201', 'ARMY_1', 410, GetTerrainHeight(410, 610), 610, 0, 0, 0)
    local cmd = IssueTeleport({tank}, gone)
    gone:Destroy()
    IssueMove({tank}, { 445, GetTerrainHeight(445, 620), 620 })
    WaitTicks(3)
    check(IsCommandDone(cmd), 'a teleport to a gone target ends')
    check(table.getn(tank:GetCommandQueue()) <= 1, 'and the order after it carries on')

    -- IssueTeleportToBeacon is an unload, for transports only.
    local t = IssueTeleportToBeacon({tank}, acu)
    check(IsCommandDone(t), 'IssueTeleportToBeacon: a tank is no transport')
    ScenarioResult.done = true
end)
