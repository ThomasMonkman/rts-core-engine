-- Scenario test (tests/scenarios.rs): a factory build and an upgrade
-- abandoned, as in the real game (dev/probes probe_q_build2). Cleared:
-- inside `IssueClearCommands`, the builder's `OnFailedToBuild`, the new
-- unit's `OnFailedToBeBuilt`, the builder's `OnStopBuild`, and the unit is
-- gone. Killed: the builder stops at once and nobody hears of the build
-- again; an upgrade's new structure is destroyed inside `Kill`. Destroyed:
-- no build callbacks; an upgrade's new structure goes the tick after.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local log = {}
local tags = {}
local function add(self, n) table.insert(log, (tags[self] or '?') .. ' ' .. n) end
for _, path in { '/units/uel0201/uel0201_script.lua', '/units/ueb3201/ueb3201_script.lua' } do
    local cls = import(path).TypeClass
    for _, name in { 'OnFailedToBeBuilt', 'OnStopBeingBuilt' } do
        local orig, n = cls[name], name
        cls[n] = function(self, a, b, c, d)
            add(self, n)
            if orig then return orig(self, a, b, c, d) end
        end
    end
end
-- Builders per instance: FAF's states override the class's callbacks.
local function hook(u)
    for _, name in { 'OnStopBuild', 'OnFailedToBuild', 'OnKilled' } do
        local n = name
        u[n] = function(self, a, b, c, d)
            add(self, n)
            return getmetatable(self)[n](self, a, b, c, d)
        end
    end
end
local function since(i, tag)
    local out = {}
    for k = i + 1, table.getn(log) do
        if string.find(log[k], tag .. ' ', 1, true) == 1 then table.insert(out, log[k]) end
    end
    return table.concat(out, ', ')
end
ForkThread(function()
    WaitTicks(2)
    local b = GetArmyBrain('ARMY_1')
    b:__engine_SetStorage('MASS', 50000) b:__engine_SetStorage('ENERGY', 500000)
    b:GiveResource('MASS', 20000) b:GiveResource('ENERGY', 400000)
    local lanes = {}
    for i, tag in { 'fkill', 'fclear', 'ukill', 'udestroy', 'uclear' } do
        local x, z = 360 + 24 * i, 600
        local factory = string.sub(tag, 1, 1) == 'f'
        local u = CreateUnitHPR(factory and 'ueb0101' or 'ueb3101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
        tags[u] = tag
        hook(u)
        if factory then IssueBuildFactory({ u }, 'uel0201', 1) else IssueUpgrade({ u }, 'ueb3201') end
        lanes[tag] = u
    end
    local started = false
    for _ = 1, 30 do
        WaitTicks(1)
        started = true
        for _, u in lanes do started = started and u:GetFocusUnit() ~= nil end
        if started then break end
    end
    check(started, 'all builds started')
    if not started then ScenarioResult.done = true return end
    local new = {}
    for tag, u in lanes do
        new[tag] = u:GetFocusUnit()
        tags[new[tag]] = 'new-' .. tag
    end
    WaitTicks(10)

    for _, tag in { 'fclear', 'uclear' } do
        local n = table.getn(log)
        IssueClearCommands({ lanes[tag] })
        local seen = table.concat(log, ', ', n + 1)
        note(tag .. ': ' .. seen)
        check(seen == tag .. ' OnFailedToBuild, new-' .. tag .. ' OnFailedToBeBuilt, ' .. tag .. ' OnStopBuild',
            tag .. ': OnFailedToBuild, OnFailedToBeBuilt, OnStopBuild inside the call: ' .. seen)
        check(new[tag]:BeenDestroyed(), tag .. ': the unfinished unit is destroyed inside the call')
    end

    local n = table.getn(log)
    local frac = new.fkill:GetFractionComplete()
    lanes.fkill:Kill()
    check(since(n, 'fkill') == 'fkill OnKilled', 'killed factory: only OnKilled inside the call: ' .. since(n, 'fkill'))
    n = table.getn(log)
    lanes.ukill:Kill()
    check(since(n, 'ukill') == 'ukill OnKilled', 'killed upgrader: only OnKilled inside the call: ' .. since(n, 'ukill'))
    check(new.ukill:BeenDestroyed(), 'killed upgrader: its new structure is destroyed inside the call')
    lanes.udestroy:Destroy()
    WaitTicks(1)
    check(not new.fkill:BeenDestroyed() and new.fkill:GetFractionComplete() <= frac, 'killed factory: its unit gets no more work')
    WaitTicks(2)
    check(new.udestroy:BeenDestroyed(), 'destroyed upgrader: its new structure is destroyed')
    WaitTicks(10)
    local all = table.concat(log, ', ')
    note(all)
    for _, tag in { 'fkill', 'ukill', 'udestroy' } do
        check(since(0, tag) == (tag == 'udestroy' and '' or tag .. ' OnKilled'), tag .. ': no build callbacks on the builder: ' .. since(0, tag))
        check(since(0, 'new-' .. tag) == '', tag .. ': nothing on the new unit: ' .. since(0, 'new-' .. tag))
    end
    ScenarioResult.done = true
end)
