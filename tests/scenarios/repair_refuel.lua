-- Scenario test (tests/scenarios.rs): repairing a damaged aircraft refuels
-- it, and both go at half speed while it's damaged (moho's build-task work
-- step, MohoEngine.dll 0x101e1c82..0x101e1d1b: `Physics.FuelRechargeRate /
-- FuelUseTime` of the tank a second).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    -- uea0101: MaxHealth 35, BuildTime 200, FuelUseTime 500, FuelRechargeRate 5.
    local plane = CreateUnitHPR('uea0101', 'ARMY_1', 403, GetTerrainHeight(403, 600) + 5, 600, 0, 0, 0)
    WaitTicks(2)
    IssueStop({plane})
    plane:SetHealth(plane, 10)
    plane:SetFuelRatio(0.2)
    IssueRepair({eng}, plane)
    local ticks, h0, f0 = 0, nil, nil
    for _ = 1, 200 do
        WaitTicks(1)
        if eng:IsUnitState('Repairing') then
            if not h0 then h0, f0 = plane:GetHealth(), plane:GetFuelRatio() end
            ticks = ticks + 1
            if ticks >= 60 then break end
        end
    end
    check(ticks >= 60, 'the engineer repaired the plane: ' .. ticks)
    if h0 then
        local dh, df = plane:GetHealth() - h0, plane:GetFuelRatio() - f0
        note(string.format('over %d ticks: health +%.3f, fuel +%.4f', ticks, dh, df))
        -- Damaged: half of BuildRate / BuildTime of max health a second
        -- (5 / 200 / 10 / 2 x 35 = 0.044 a tick).
        check(math.abs(dh / ticks - 0.04375) < 0.01, 'half-speed repair while refuelling: ' .. dh / ticks)
        -- Half of 5 / 500 / 10 a tick, less what flying burns (1 / 5000).
        check(df > 0.01, 'the repair refuels it: ' .. df)
    end
    ScenarioResult.done = true
end)
