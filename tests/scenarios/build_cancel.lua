-- Scenario test (tests/scenarios.rs): cancelled builds, as in the real game
-- (dev/probes probe_unit_build G2-G4): a cancelled upgrade stops costing
-- (the old structure's `OnStopBuild` turns its consumption off); a factory
-- cancelled mid-build gets `OnFailedToBuild` then `OnStopBuild`; an
-- engineer `OnStopBuild` then `OnFailedToBuild`.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local log = {}
local function hook(path, names)
    local cls = import(path).TypeClass
    for _, name in names do
        local orig, n = cls[name], name
        cls[n] = function(self, a, b, c)
            table.insert(log, (self.ScenarioTag or '?') .. ' ' .. n)
            if orig then return orig(self, a, b, c) end
        end
    end
end
hook('/units/ueb0101/ueb0101_script.lua', { 'OnStopBuild', 'OnFailedToBuild' })
hook('/units/uel0105/uel0105_script.lua', { 'OnStopBuild', 'OnFailedToBuild' })
local function since(i)
    local out = {}
    for k = i + 1, table.getn(log) do table.insert(out, log[k]) end
    return table.concat(out, ', ')
end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 20000) b:GiveResource('ENERGY', 400000)
    local radar = CreateUnitHPR('ueb3101', 'ARMY_1', 400, GetTerrainHeight(400, 580), 580, 0, 0, 0)
    local fac = CreateUnitHPR('ueb0101', 'ARMY_1', 440, GetTerrainHeight(440, 620), 620, 0, 0, 0)
    fac.ScenarioTag = 'fac'
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 420, GetTerrainHeight(420, 600), 600, 0, 0, 0)
    eng.ScenarioTag = 'eng'
    WaitTicks(2)
    IssueUpgrade({radar}, 'ueb3201')
    IssueBuildFactory({fac}, 'uel0201', 1)
    IssueBuildMobile({eng}, { 424, GetTerrainHeight(424, 600), 600 }, 'ueb1101', {})
    for _ = 1, 30 do
        WaitTicks(2)
        if radar:IsUnitState('Upgrading') and fac:IsUnitState('Building') and eng:IsUnitState('Building') then break end
    end
    check(radar:IsUnitState('Upgrading') and fac:IsUnitState('Building') and eng:IsUnitState('Building'), 'all three started')
    WaitTicks(10)
    local n = table.getn(log)
    IssueClearCommands({radar, fac, eng})
    WaitTicks(5)
    local seen = since(n)
    note('callbacks: ' .. seen)
    check(string.find(seen, 'fac OnFailedToBuild, fac OnStopBuild', 1, true), 'factory: OnFailedToBuild then OnStopBuild')
    check(string.find(seen, 'eng OnStopBuild, eng OnFailedToBuild', 1, true), 'engineer: OnStopBuild then OnFailedToBuild')
    local m0 = b:GetEconomyStored('MASS')
    WaitTicks(21)
    local m1 = b:GetEconomyStored('MASS')
    note(string.format('mass %.2f -> %.2f after the cancel', m0, m1))
    check(m1 >= m0, 'a cancelled upgrade costs nothing more')
    check(not radar:IsUnitState('Upgrading'), 'the old structure is no longer upgrading')
    ScenarioResult.done = true
end)
