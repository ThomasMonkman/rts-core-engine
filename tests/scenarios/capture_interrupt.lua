-- Scenario test (tests/scenarios.rs): interrupting a capture, as in the real
-- game (dev/probes probe_unit_capture2 CI, CU). `IssueClearCommands` fails it
-- inside the call, the target hearing first, and nothing more is charged.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function ground(x, z) return GetTerrainHeight(x, z) end
ForkThread(function()
    WaitTicks(2)
    local brain = ArmyBrains[1]
    brain:__engine_SetStorage('MASS', 50000) brain:__engine_SetStorage('ENERGY', 500000)
    brain:GiveResource('MASS', 50000) brain:GiveResource('ENERGY', 500000)
    local log = {}
    local function watch(u, tag)
        for _, cb in { 'OnStopCapture', 'OnFailedCapture', 'OnFailedBeingCaptured' } do
            local base = u[cb]
            u[cb] = function(self, other)
                table.insert(log, tag .. ' ' .. cb .. ' ' .. (other == self and 'self' or 'other'))
                return base(self, other)
            end
        end
    end
    local function capturing(eng)
        for _ = 1, 50 do
            if eng:IsUnitState('Capturing') then return true end
            WaitTicks(1)
        end
    end

    -- Cleared.
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, ground(400, 600), 600, 0, 0, 0)
    local prey = CreateUnitHPR('ueb1101', 'ARMY_2', 404, ground(404, 600), 600, 0, 0, 0)
    watch(eng, 'eng') watch(prey, 'prey')
    IssueCapture({eng}, prey)
    check(capturing(eng), 'the capture starts')
    WaitTicks(3)
    IssueClearCommands({eng})
    check(table.concat(log, ', ') == 'prey OnFailedBeingCaptured other, eng OnFailedCapture other', 'cleared: failed inside the call, target first: ' .. table.concat(log, ', '))
    check(not eng:IsUnitState('Capturing'), 'no longer capturing')
    local stored = brain:GetEconomyStored('ENERGY')
    WaitTicks(2)
    check(brain:GetEconomyStored('ENERGY') >= stored, 'nothing more is charged')

    ScenarioResult.done = true
end)
