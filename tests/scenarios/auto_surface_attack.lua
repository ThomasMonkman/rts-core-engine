-- Scenario test (tests/scenarios.rs): `AI.AutoSurfaceToAttack`.
-- Moho's attack task surfaces a `SurfacingSub` in the `Sub` layer when the
-- blueprint sets it and none of its weapons can hit the target from where it
-- is (MohoEngine.dll 0x101df772..0x101df8d4, AI +0x6c). No FAF unit sets it,
-- and the engine's default is true (every real blueprint dump has it,
-- dev/probes probe_api_spot): a submerged sub ordered to attack a tank on the
-- shore surfaces; one with the flag set false stays under.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    local ok, err = pcall(function()
        WaitTicks(2)
        local function depth(x, z) return GetSurfaceHeight(x, z) - GetTerrainHeight(x, z) end
        local site
        local w, h = GetMapSize()
        for z = 100, h - 100, 4 do
            for x = 100, w - 100, 4 do
                if not site and depth(x, z) > 8 and depth(x + 4, z) > 8 and depth(x + 20, z) <= 0 then site = { x, z } end
            end
        end
        check(site ~= nil, 'found deep water by a shore')
        if not site then return end
        local sx, sz = site[1], site[2]
        local bp = __blueprints['ues0203']
        local function trial(flag)
            bp.AI = bp.AI or {}
            bp.AI.AutoSurfaceToAttack = flag
            local sub = CreateUnitHPR('ues0203', 'ARMY_1', sx, GetSurfaceHeight(sx, sz), sz, 0, 0, 0)
            local tank = CreateUnitHPR('uel0201', 'ARMY_2', sx + 20, GetTerrainHeight(sx + 20, sz), sz, 0, 0, 0)
            tank:SetCanTakeDamage(false)
            tank:SetFireState(1)
            WaitTicks(2)
            local was = sub:GetCurrentLayer()
            IssueAttack({ sub }, tank)
            local surfacing = false
            for _ = 1, 60 do
                WaitTicks(1)
                surfacing = surfacing or sub:IsUnitState('MovingUp')
            end
            local layer = sub:GetCurrentLayer()
            sub:Destroy() tank:Destroy()
            WaitTicks(2)
            return was, surfacing, layer
        end
        check(bp.AI.AutoSurfaceToAttack == true, 'the default is true')
        local was, surfacing, layer = trial(true)
        note(string.format('with the flag: %s -> %s (surfacing seen %s)', was, layer, tostring(surfacing)))
        check(was == 'Sub', 'the sub starts under the water: ' .. was)
        check(surfacing and layer == 'Water', 'it surfaces to attack the tank: ' .. layer)
        was, surfacing, layer = trial(false)
        note(string.format('without: %s -> %s', was, layer))
        check(not surfacing and layer == 'Sub', 'without the flag it stays under: ' .. layer)
    end)
    if not ok then check(false, tostring(err)) end
    ScenarioResult.done = true
end)
