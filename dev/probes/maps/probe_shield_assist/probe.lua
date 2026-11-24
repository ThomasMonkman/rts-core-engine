-- Differential probe: engineers assisting a shield generator regenerate its
-- shield (moho's build-task work step, issue #82), in a format both engines
-- print identically ("PROBE ..." lines).
--   real game (Windows): dev/probes/run_faf.ps1 probe_shield_assist 120
--   ours: cargo run -- --headless --no-ai --ticks 260 --script dev/probes/maps/probe_shield_assist/probe.lua
-- Each lane: a T2 shield generator (ueb4202: ShieldRegenRate 120,
-- RegenAssistMult 60) whose shield takes `AdjustHealth(-dmg)`; that wakes no
-- FAF regen thread, so only the engine moves the shield's health. T1
-- engineers (uel0105, BuildRate 5) beside it get the order. Logged per tick
-- (every 5th after 30): shield health, unit health, then per engineer its
-- queue length, R(epairing) / G(uarding) and its energy / mass a second.
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local A1 = 1
local brain = ArmyBrains[A1]
local function make(bp, x, z)
    return CreateUnitHPR(bp, A1, x, GetTerrainHeight(x, z), z, 0, 0, 0)
end
local SITES = { { 608, 80 }, { 688, 80 }, { 912, 96 }, { 784, 144 }, { 656, 160 }, { 736, 224 }, { 816, 224 }, { 912, 240 }, { 672, 304 }, { 800, 336 } }
for _, s in SITES do
    local ok, list = pcall(GetReclaimablesInRect, Rect(s[1] - 16, s[2] - 16, s[1] + 16, s[2] + 16))
    if ok and list then
        for _, r in list do
            if IsProp(r) and not r:BeenDestroyed() then r:Destroy() end
        end
    end
end
for i = 0, 3 do
    make('ueb1105', SITES[10][1] + 4 * i, SITES[10][2])
    make('ueb1106', SITES[10][1] + 4 * i, SITES[10][2] + 6)
end

-- name, site, engineers, order, shield damage, unit health (nil: full), shield off
local LANES = {
    { 'none', 1, 0, nil, 50 },
    { 'repair1', 2, 1, IssueRepair, 50 },
    { 'repair3', 3, 3, IssueRepair, 50 },
    { 'guard1', 4, 1, IssueGuard, 50 },
    { 'hurt1', 5, 1, IssueRepair, 50, 200 },
    { 'off1', 6, 1, IssueRepair, 50, nil, true },
    { 'full1', 7, 1, IssueRepair, 0 },
    { 'big1', 8, 1, IssueRepair, 3000 },
}

ForkThread(function()
    WaitTicks(2)
    brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)
    local lanes = {}
    for _, l in LANES do
        local x, z = SITES[l[2]][1], SITES[l[2]][2]
        local gen = make('ueb4202', x, z)
        local engs = {}
        for i = 1, l[3] do
            -- in reach (build range 5 from the 6x6 skirt's edge), clear of the footprint
            table.insert(engs, make('uel0105', x - 4, z - 4 + 2 * i))
        end
        table.insert(lanes, { name = l[1], gen = gen, engs = engs, order = l[4], dmg = l[5], hp = l[6], off = l[7] })
    end
    WaitTicks(20)
    brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)
    for _, l in lanes do
        local s = l.gen.MyShield
        LOG(fmt('PROBE setup %s shield %.2f/%.2f on %s regen %s mult %s', l.name, s:GetHealth(), s:GetMaxHealth(), tostring(l.gen:ShieldIsOn()),
            tostring(s.RegenRate), tostring(l.gen:GetBlueprint().Defense.Shield.RegenAssistMult)))
        if l.off then l.gen:SetScriptBit('RULEUTC_ShieldToggle', false) end
        if l.hp then l.gen:SetHealth(nil, l.hp) end
        if l.dmg > 0 then s:AdjustHealth(s, -l.dmg) end
    end
    WaitTicks(1)
    for _, l in lanes do
        if l.order and table.getn(l.engs) > 0 then l.order(l.engs, l.gen) end
    end
    local t0 = GetGameTick()
    for t = 0, 220 do
        if t <= 30 or math.mod(t, 5) == 0 then
            for _, l in lanes do
                local s = l.gen.MyShield
                local line = fmt('PROBE %d %s sh %.2f hp %.2f on %s', GetGameTick() - t0, l.name, s:GetHealth(), l.gen:GetHealth(), tostring(l.gen:ShieldIsOn()))
                for i, e in l.engs do
                    line = line .. fmt(' | e%d q%d %s%s %.1f/%.2f', i, table.getn(e:GetCommandQueue()),
                        e:IsUnitState('Repairing') and 'R' or '-', e:IsUnitState('Guarding') and 'G' or '-',
                        e:GetConsumptionPerSecondEnergy(), e:GetConsumptionPerSecondMass())
                end
                LOG(line)
            end
        end
        WaitTicks(1)
    end
    LOG('PROBE done')
end)
