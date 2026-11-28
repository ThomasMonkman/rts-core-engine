-- Scenario test (tests/scenarios.rs): a unit moving onto another terrain
-- type gets OnTerrainTypeChange(new, old) with the TerrainTypes entries.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    -- Off the map is the Default type, not the edge cell's (FAF's
    -- `DefaultTerrainType = GetTerrainType(-1, -1)`).
    local mx, mz = GetMapSize()
    check(GetTerrainType(-1, -1).Name == 'Default', 'GetTerrainType(-1, -1) is Default')
    check(GetTerrainType(mx, mz).Name == 'Default', 'GetTerrainType at the far edge is Default')
    -- A dry spot 40 east of which the type differs, with the same type along
    -- the first 10.
    local ax, az, bx
    for z = 64, 960, 8 do
        for x = 64, 900, 8 do
            local a = GetTerrainType(x, z).TypeCode
            if GetSurfaceHeight(x, z) == GetTerrainHeight(x, z) and GetTerrainType(x + 10, z).TypeCode == a then
                local b = GetTerrainType(x + 40, z).TypeCode
                if b ~= a and GetSurfaceHeight(x + 40, z) == GetTerrainHeight(x + 40, z) then ax, az, bx = x, z, x + 40 break end
            end
        end
        if ax then break end
    end
    if not ax then
        table.insert(ScenarioResult.notes, 'one terrain type on this map: skipped')
        ScenarioResult.done = true
        return
    end
    local tank = CreateUnitHPR('uel0201', 'ARMY_1', ax, GetTerrainHeight(ax, az), az, 0, 0, 0)
    local calls = {}
    local orig = tank.OnTerrainTypeChange
    tank.OnTerrainTypeChange = function(self, new, old)
        table.insert(calls, { new = new.TypeCode, old = old.TypeCode })
        return orig(self, new, old)
    end
    IssueMove({tank}, { ax + 4, GetTerrainHeight(ax + 4, az), az })
    WaitTicks(20)
    check(table.getn(calls) == 0, 'no change on the same type')
    IssueMove({tank}, { bx, GetTerrainHeight(bx, az), az })
    for _ = 1, 200 do
        WaitTicks(1)
        if table.getn(tank:GetCommandQueue()) == 0 then break end
    end
    local want = GetTerrainType(bx, az).TypeCode
    local last = calls[table.getn(calls)]
    check(last ~= nil, 'OnTerrainTypeChange on the way')
    if last then
        check(last.new == want, string.format('ended on type %d, last change to %d', want, last.new))
        check(calls[1].old == GetTerrainType(ax, az).TypeCode, 'first change from the start type')
        check(tank.TerrainType and tank.TerrainType.TypeCode == want, 'FAF stored the new type')
    end
    ScenarioResult.done = true
end)
