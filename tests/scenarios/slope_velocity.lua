-- Scenario test (tests/scenarios.rs): `GetVelocity` of a unit driving up a
-- slope gives the whole ground speed along x / z and the climb in y on top;
-- a hover unit's y follows the ground too. The real game's T1 tank on
-- Seton's 5.3-high ramp reads 3.40 along and up to 1.45-1.49 up a second,
-- the Riptide 3.70 along and its climb (dev/probes probe_mov_api S*).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local function make(bp, z)
        local u = CreateUnitHPR(bp, 'ARMY_1', 938.5, GetTerrainHeight(938.5, z), z, 0, math.pi / 2, 0)
        u:SetFireState(1)
        IssueMove({ u }, { 976.5, 0, z })
        return { u = u, top = 0, along = 0 }
    end
    local tank, hover = make('uel0201', 184.5), make('uel0203', 196.5)
    for t = 1, 70 do
        WaitTicks(1)
        for _, m in { tank, hover } do
            local vx, vy, vz = m.u:GetVelocity()
            if vy > m.top then m.top = vy m.along = math.sqrt(vx * vx + vz * vz) end
        end
    end
    check(math.abs(10 * tank.along - 3.40) < 0.03, string.format('the tank keeps 3.40 along the ground on the ramp: %.2f', 10 * tank.along))
    check(10 * tank.top > 1.3, string.format('the tank reports its climb in y: %.2f', 10 * tank.top))
    check(math.abs(10 * hover.along - 3.70) < 0.03, string.format('the hover tank keeps 3.70 along: %.2f', 10 * hover.along))
    check(10 * hover.top > 1.0, string.format('the hover tank reports its climb in y: %.2f', 10 * hover.top))
    note(string.format('tank %.2f up %.2f, hover %.2f up %.2f', 10 * tank.along, 10 * tank.top, 10 * hover.along, 10 * hover.top))
    ScenarioResult.done = true
end)
