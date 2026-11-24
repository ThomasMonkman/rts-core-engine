-- Scenario test (tests/scenarios.rs): a Fatboy made at the edge of a shore
-- cliff, its footprint over the drop, and sent into the water below (the
-- real game: dev/probes probe_fatboy_path C). It stands on a cell its class
-- can't, so it first backs onto one it can and then walks round by the
-- beach to the north; it never goes down the cliff.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    local ok, err = pcall(function()
        WaitTicks(2)
        -- z 620: level until x 432, 2 down by x 434, 5 down by x 442.
        local fatboy = CreateUnitHPR('uel0401', 'ARMY_1', 430, GetTerrainHeight(430, 620), 620, 0, 0, 0)
        fatboy:SetCanTakeDamage(false)
        WaitTicks(30)
        IssueMove({ fatboy }, { 444, GetTerrainHeight(444, 620), 620 })
        local east = 430
        for _ = 1, 250 do
            WaitTicks(1)
            local x, _, z = fatboy:GetPositionXYZ()
            if z > 612 then east = math.max(east, x) end
            if fatboy:GetCurrentLayer() ~= 'Land' then
                check(false, string.format('in the water at %.1f,%.1f: down the cliff', x, z))
                return
            end
        end
        local x, _, z = fatboy:GetPositionXYZ()
        check(east < 432, string.format('it kept off the cliff: as far east as %.1f beside it', east))
        -- (The real game's is at 436.8,600.6 by now; ours turns sooner.)
        check(z < 606 and x > 432, string.format('250 ticks on it is walking round to the north: at %.1f,%.1f', x, z))
    end)
    if not ok then check(false, tostring(err)) end
    ScenarioResult.done = true
end)
