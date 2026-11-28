-- Scenario test (tests/scenarios.rs): the creation callbacks' order, as in
-- the real game (dev/probes probe_q_api): `OnPreCreate`, `OnLayerChange`,
-- `OnTerrainTypeChange(new, nil)`, then `OnCreate` with `TerrainType` set.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local log = {}
local cls = import('/units/uel0201/uel0201_script.lua').TypeClass
for _, name in { 'OnPreCreate', 'OnLayerChange', 'OnTerrainTypeChange', 'OnCreate', 'OnStopBeingBuilt' } do
    local orig, n = cls[name], name
    cls[n] = function(self, a, b, c)
        local s = n
        if n == 'OnTerrainTypeChange' then
            s = s .. '(' .. type(a) .. ',' .. type(b) .. ')'
            self.ScenarioNew = a
        elseif n == 'OnCreate' then
            s = s .. ((self.TerrainType ~= nil and self.TerrainType == self.ScenarioNew) and '+type' or '-type')
        end
        table.insert(log, s)
        if orig then return orig(self, a, b, c) end
    end
end
ForkThread(function()
    WaitTicks(2)
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    local seen = table.concat(log, ' ')
    note(seen)
    check(string.find(seen, 'OnPreCreate OnLayerChange OnTerrainTypeChange(table,nil) OnCreate+type', 1, true) == 1,
        'OnTerrainTypeChange(new, nil) between the first OnLayerChange and OnCreate: ' .. seen)
    local _, count = string.gsub(seen, 'OnTerrainTypeChange', '')
    check(count == 1, 'called once at creation: ' .. count)
    check(tank.TerrainType == GetTerrainType(400, 600), 'TerrainType is the type under the unit')
    ScenarioResult.done = true
end)
