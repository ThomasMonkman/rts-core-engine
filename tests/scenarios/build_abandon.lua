-- Scenario test (tests/scenarios.rs): a mobile build abandoned, as in the
-- real game (dev/probes probe_q_build). The builder's `OnFailedToBuild`
-- comes in the same tick: inside `IssueClearCommands` after `OnStopBuild`,
-- inside `Kill` before `OnKilled` (no `OnStopBuild`), and after `OnDestroy`
-- at the end of the tick of a `Destroy`. A dead builder does no more work.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local log = {}
local cls = import('/units/uel0105/uel0105_script.lua').TypeClass
for _, name in { 'OnStopBuild', 'OnFailedToBuild', 'OnKilled', 'OnDestroy' } do
    local orig, n = cls[name], name
    cls[n] = function(self, a, b, c, d)
        table.insert(log, GetGameTick() .. ' ' .. (self.ScenarioTag or '?') .. ' ' .. n)
        if orig then return orig(self, a, b, c, d) end
    end
end
local function since(i, tag)
    local out = {}
    for k = i + 1, table.getn(log) do
        if string.find(log[k], ' ' .. tag .. ' ', 1, true) then table.insert(out, log[k]) end
    end
    return table.concat(out, ', ')
end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 20000) b:GiveResource('ENERGY', 400000)
    local lanes = {}
    for i, tag in { 'clear', 'kill', 'destroy' } do
        local x, z = 388 + 12 * i, 600
        local e = CreateUnitHPR('uel0105', 'ARMY_1', x, GetTerrainHeight(x, z + 4), z + 4, 0, 0, 0)
        e.ScenarioTag = tag
        e:SetBuildRate(20)
        IssueBuildMobile({ e }, { x, GetTerrainHeight(x, z), z }, 'ueb1101', {})
        lanes[tag] = e
    end
    local started = false
    for _ = 1, 60 do
        WaitTicks(1)
        started = true
        for _, e in lanes do started = started and e:GetFocusUnit() ~= nil end
        if started then break end
    end
    check(started, 'all three builds started')
    if not started then ScenarioResult.done = true return end
    local gens = {}
    for tag, e in lanes do gens[tag] = e:GetFocusUnit() end
    WaitTicks(8)
    local now = GetGameTick()

    local n = table.getn(log)
    IssueClearCommands({ lanes.clear })
    local seen = since(n, 'clear')
    note('clear: ' .. seen)
    check(seen == now .. ' clear OnStopBuild, ' .. now .. ' clear OnFailedToBuild', 'cleared: OnStopBuild then OnFailedToBuild inside the call: ' .. seen)

    n = table.getn(log)
    lanes.kill:Kill()
    seen = since(n, 'kill')
    note('kill: ' .. seen)
    check(seen == now .. ' kill OnFailedToBuild, ' .. now .. ' kill OnKilled', 'killed: OnFailedToBuild then OnKilled inside the call: ' .. seen)

    n = table.getn(log)
    lanes.destroy:Destroy()
    check(since(n, 'destroy') == '', 'destroyed: nothing inside the call')
    local frac = {}
    for tag, g in gens do frac[tag] = g:GetFractionComplete() end
    WaitTicks(1)
    seen = since(n, 'destroy')
    note('destroy: ' .. seen)
    check(seen == now .. ' destroy OnDestroy, ' .. now .. ' destroy OnFailedToBuild', 'destroyed: OnDestroy then OnFailedToBuild that tick: ' .. seen)

    WaitTicks(20)
    for tag, g in gens do
        check(not g:BeenDestroyed() and g:GetFractionComplete() <= frac[tag], tag .. ': the structure gets no more work')
    end
    local all = table.concat(log, ', ')
    local _, fails = string.gsub(all, 'OnFailedToBuild', '')
    check(fails == 3, 'one OnFailedToBuild each: ' .. all)
    check(not string.find(all, 'kill OnStopBuild', 1, true) and not string.find(all, 'destroy OnStopBuild', 1, true), 'no OnStopBuild for a dead builder: ' .. all)
    ScenarioResult.done = true
end)
