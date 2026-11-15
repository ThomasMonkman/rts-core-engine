-- Scenario test (tests/scenarios.rs): a plane ordered onto an air staging
-- pad lands on it before it docks. The real game's interceptor, 40 from the
-- pad, is over it at t~41, comes down, changes layer `Air -> Land` at t=153
-- and docks at t=154, at rest; ours docked on arrival (t=40), still in the
-- `Air` layer at its flight speed (dev/probes probe_mov_air F4).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local pad = CreateUnitHPR('ueb5202', 'ARMY_1', 662, GetTerrainHeight(662, 134), 134, 0, 0, 0)
    local plane = CreateUnitHPR('uea0102', 'ARMY_1', 622, GetTerrainHeight(622, 134), 134, 0, math.pi / 2, 0)
    plane:SetFireState(1)
    WaitTicks(10)
    plane:SetFuelRatio(0.3)
    IssueTransportLoad({ plane }, pad)
    local docked, layer
    for t = 1, 400 do
        WaitTicks(1)
        if plane:IsUnitState('Attached') then
            docked = t
            layer = plane:GetCurrentLayer()
            break
        end
    end
    check(docked ~= nil, 'the plane docked')
    if docked then
        check(docked > 80, 'it docked only after landing, not on arrival over the pad: t=' .. docked)
        check(layer == 'Land', 'docked on the Land layer: ' .. tostring(layer))
        WaitTicks(1)
        local vx, vy, vz = plane:GetVelocity()
        check(vx == 0 and vy == 0 and vz == 0, 'docked at rest')
        note('docked at t=' .. docked)
    end
    ScenarioResult.done = true
end)
