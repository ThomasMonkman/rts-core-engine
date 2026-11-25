-- Scenario test (tests/scenarios.rs): a rack fixed to the hull fires at any
-- target inside its heading arc, as in the real game (dev/probes
-- probe_q_slaved, probe_q_fatboy): an immobile Aeon MML (`ual0111`,
-- `SlavedToBody`, not turreted, `FiringTolerance` 5, `HeadingArcRange` 60)
-- fires at a target 40 degrees off its nose and not at one 75 degrees off.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
ForkThread(function()
    WaitTicks(2)
    local lanes = {}
    for i, deg in { 40, 75 } do
        -- (Far enough apart that neither can fire at the other's target.)
        local x, z = 385, 490 + 75 * i
        local mml = CreateUnitHPR('ual0111', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
        mml:SetImmobile(true)
        mml:SetIntelRadius('Vision', 70)
        local r = math.rad(deg)
        local tx, tz = x + math.sin(r) * 35, z + math.cos(r) * 35
        local target = CreateUnitHPR('uel0201', 'ARMY_2', tx, GetTerrainHeight(tx, tz), tz, 0, 0, 0)
        target:SetFireState(1)
        target:SetMaxHealth(1000000) target:SetHealth(nil, 1000000)
        local lane = { deg = deg, mml = mml, hits = 0 }
        target.OnDamage = function() lane.hits = lane.hits + 1 end
        table.insert(lanes, lane)
    end
    WaitTicks(150)
    for _, l in lanes do
        note(l.deg .. ' degrees off: ' .. l.hits .. ' hits')
        check(math.abs(l.mml:GetHeading()) < 0.01, l.deg .. ': the immobile launcher did not turn')
    end
    check(lanes[1].hits > 0, 'fires at a target 40 degrees off the nose')
    check(lanes[2].hits == 0, 'does not fire at a target outside its heading arc: ' .. lanes[2].hits)
    ScenarioResult.done = true
end)
