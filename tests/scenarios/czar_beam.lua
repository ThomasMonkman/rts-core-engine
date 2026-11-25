-- Scenario test (tests/scenarios.rs): the CZAR's Quantum Beam Generator is
-- fixed (Turreted = false) along its Center_Muzzle bone, which points
-- straight down. Its FiringTolerance (30 degrees) does not gate it: as in
-- the real game (dev/probes probe_q_czar), an immobile CZAR 20 up switches
-- the beam on for a tank 14 out to the side (35 degrees off straight down),
-- and the beam, going straight down, misses it. Free to move, the CZAR
-- flies over the tank and the beam hits.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local function run()
    WaitTicks(2)
    local czar = make('uaa0310', 'ARMY_1', 400, 600)
    czar:SetMaxHealth(10000000) czar:SetHealth(nil, 10000000)
    WaitTicks(120)
    czar:SetImmobile(true)
    local p = czar:GetPosition()
    local dx, dy, dz = czar:GetBoneDirection('Center_Muzzle')
    note(string.format('muzzle direction %.2f %.2f %.2f, height %.1f', dx, dy, dz, p[2] - GetTerrainHeight(p[1], p[3])))
    local tank = make('uel0201', 'ARMY_2', p[1] + 14, p[3])
    tank:SetFireState(1)
    tank:SetMaxHealth(10000000) tank:SetHealth(nil, 10000000)
    local hits = 0
    tank.OnDamage = function() hits = hits + 1 end
    local beam = czar:GetWeaponByLabel('QuantumBeamGeneratorWeapon')
    local on_at
    for t = 1, 40 do
        WaitTicks(1)
        if not on_at and beam.Beams[1].Beam:IsEnabled() then on_at = t end
    end
    local q = tank:GetPosition()
    local off = math.deg(math.atan2(14, p[2] - q[2]))
    note(string.format('tank %.1f degrees off straight down; beam on after %s ticks; %d hits', off, tostring(on_at), hits))
    check(off > 31, 'the tank is outside the firing tolerance of straight down')
    check(on_at ~= nil, 'the beam switches on for a target outside the firing tolerance')
    check(hits == 0, 'the beam goes straight down and misses the tank: ' .. hits)

    czar:SetImmobile(false)
    for _ = 1, 150 do
        WaitTicks(1)
        if hits > 0 then break end
    end
    local c = czar:GetPosition()
    note(string.format('free to move: %d hits, %.1f from over the tank', hits, VDist2(c[1], c[3], q[1], q[3])))
    check(hits > 0, 'the CZAR got over the tank and its beam hit')
    ScenarioResult.done = true
end
ForkThread(function()
    local ok, err = pcall(run)
    if not ok then check(false, tostring(err)) ScenarioResult.done = true end
end)
