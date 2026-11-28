-- Scenario test (tests/scenarios.rs): repairing a shield generator
-- regenerates its shield by `RegenRate x 0.1 x BuildRate /
-- RegenAssistMult` a tick, the divisor doubled (and the repair halved)
-- while the generator is damaged; at full health the order lasts until the
-- shield is full, and ends at once with the shield off (moho's build-task
-- work step, MohoEngine.dll 0x101e1b33..0x101e1c76; FAF probe
-- `probe_shield_assist`).
-- ueb4202: ShieldRegenRate 120, RegenAssistMult 60; uel0105: BuildRate 5,
-- so 1 a tick, 0.5 while damaged. `AdjustHealth` on the shield wakes no FAF
-- regen thread: only the repair moves it.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function lane(x, z)
    local gen = CreateUnitHPR('ueb4202', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local eng = CreateUnitHPR('uel0105', 'ARMY_1', x - 4, GetTerrainHeight(x - 4, z), z, 0, 0, 0)
    return gen, eng
end
-- Ticks of Repairing (at most `max`), and the shield's and the unit's
-- gains over all but the first.
local function measure(eng, gen, max)
    local shield = gen.MyShield
    local ticks, s0, h0 = 0, nil, nil
    for _ = 1, max + 30 do
        WaitTicks(1)
        if eng:IsUnitState('Repairing') then
            s0, h0 = s0 or shield:GetHealth(), h0 or gen:GetHealth()
            ticks = ticks + 1
            if ticks >= max then break end
        elseif s0 then
            break
        end
    end
    if not s0 then return ticks, 0, 0 end
    return ticks, shield:GetHealth() - s0, gen:GetHealth() - h0
end
ForkThread(function()
    WaitTicks(2)
    local b = ArmyBrains[1]
    b:__engine_SetStorage("MASS", 50000) b:__engine_SetStorage("ENERGY", 500000)
    b:GiveResource("MASS", 50000) b:GiveResource("ENERGY", 500000)
    local full, fullEng = lane(400, 600)
    local hurt, hurtEng = lane(440, 600)
    local off, offEng = lane(480, 600)
    WaitTicks(20)
    -- Full health: 1 a tick until the shield is full, then the order ends.
    local s = full.MyShield
    s:AdjustHealth(s, -30)
    IssueRepair({fullEng}, full)
    local ticks, gain = measure(fullEng, full, 10)
    note(string.format('full health: +%.2f over %d ticks', gain, ticks))
    check(ticks == 10 and math.abs(gain - 9) < 0.01, 'one engineer gives the shield 1 a tick: ' .. gain .. ' over ' .. ticks)
    for _ = 1, 40 do
        if table.getn(fullEng:GetCommandQueue()) == 0 then break end
        WaitTicks(1)
    end
    check(s:GetHealth() == s:GetMaxHealth(), 'the repair filled the shield: ' .. s:GetHealth())
    check(table.getn(fullEng:GetCommandQueue()) == 0, 'the repair order ended with the shield full')
    -- Damaged: half the divisor's worth, and the unit repaired at half speed.
    s = hurt.MyShield
    hurt:SetHealth(nil, 200)
    s:AdjustHealth(s, -30)
    IssueRepair({hurtEng}, hurt)
    local dh
    ticks, gain, dh = measure(hurtEng, hurt, 20)
    note(string.format('damaged: shield +%.2f, health +%.3f over %d ticks', gain, dh, ticks))
    check(ticks == 20 and math.abs(gain - 9.5) < 0.01, 'while damaged the shield gains 0.5 a tick: ' .. gain)
    -- 5 / 1150 / 10 x 250 / 2 = 0.0543 a tick.
    check(math.abs(dh / 19 - 0.0543) < 0.001, 'and the repair goes at half speed: ' .. dh)
    -- Shield off at full health: nothing to do.
    s = off.MyShield
    off:SetScriptBit('RULEUTC_ShieldToggle', false)
    s:AdjustHealth(s, -30)
    local before = s:GetHealth()
    IssueRepair({offEng}, off)
    WaitTicks(10)
    check(s:GetHealth() == before, 'no regeneration with the shield off: ' .. s:GetHealth())
    check(table.getn(offEng:GetCommandQueue()) == 0, 'the repair of a full unit with its shield off ends')
    ScenarioResult.done = true
end)
