-- Scenario test (tests/scenarios.rs): an unfinished structure nobody is
-- building decays (0.1 a tick of its highest cost, here a wall's 20 energy:
-- 0.5% a tick), and at nothing left gets OnDecayed (FAF destroys it). Work
-- on it holds the decay off. Health is what runs out: one abandoned at the
-- tick it was started (fraction 0, health 1) stands until that 1 health has
-- decayed, as in the real game (dev/probes probe_t4_vanish).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', 400, GetTerrainHeight(400, 600), 600, 0, 0, 0)
    IssueBuildMobile({eng}, {403.5, GetTerrainHeight(403.5, 600.5), 600.5}, 'ueb5101', {})
    local wall
    for _ = 1, 100 do
        WaitTicks(1)
        wall = eng:GetFocusUnit()
        if wall and wall:GetFractionComplete() > 0.3 then break end
    end
    check(wall ~= nil, 'the engineer started the wall')
    if not wall then ScenarioResult.done = true return end
    IssueClearCommands({eng})
    WaitTicks(2)
    local f0, t0 = wall:GetFractionComplete(), GetGameTick()
    WaitTicks(10)
    local f1, ticks = wall:GetFractionComplete(), GetGameTick() - t0
    check(math.abs((f0 - f1) - 0.005 * ticks) < 0.002, string.format('decays 0.5%% a tick: %.4f -> %.4f in %d ticks', f0, f1, ticks))
    local decayed = false
    local old = wall.OnDecayed
    wall.OnDecayed = function(self) decayed = true return old(self) end
    for _ = 1, 200 do
        WaitTicks(1)
        if decayed then break end
    end
    check(decayed, 'OnDecayed at no progress left')
    WaitTicks(1)
    check(wall.Dead or IsDestroyed(wall), 'FAF destroys the decayed wall')

    -- Abandoned before any progress: an Ahwassa (52000 health, 1920000
    -- energy: 0.0027 health a tick, 1 health lasts 370 ticks) cleared inside
    -- its builder's OnStartBuild.
    local seng = CreateUnitHPR('xsl0309', 'ARMY_1', 404, GetTerrainHeight(404, 614), 614, 0, 0, 0)
    local t4
    seng:AddUnitCallback(function(self, built)
        if not t4 then t4 = built IssueClearCommands({self}) end
    end, 'OnStartBuild')
    IssueBuildMobile({seng}, {404.5, GetTerrainHeight(404.5, 600.5), 600.5}, 'xsa0402', {})
    for _ = 1, 100 do
        WaitTicks(1)
        if t4 then break end
    end
    check(t4 ~= nil, 'the engineer started the Ahwassa')
    if not t4 then ScenarioResult.done = true return end
    WaitTicks(100)
    check(not (t4.Dead or IsDestroyed(t4)), 'an Ahwassa abandoned at fraction 0 still stands 100 ticks on')
    if not IsDestroyed(t4) then
        local h = t4:GetHealth()
        check(t4:GetFractionComplete() == 0, 'its fraction stays 0')
        check(math.abs(h - (1 - 101 * 52000 * 0.1 / 1920000)) < 0.01, string.format('its 1 health decays 0.0027 a tick: %.4f', h))
    end
    local gone
    for i = 1, 300 do
        WaitTicks(1)
        if IsDestroyed(t4) then gone = i + 100 break end
    end
    check(gone and gone > 360 and gone < 375, 'it decays away about 370 ticks on: ' .. tostring(gone))
    ScenarioResult.done = true
end)
