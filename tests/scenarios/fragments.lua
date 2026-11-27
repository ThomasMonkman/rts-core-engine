-- Scenario test (tests/scenarios.rs): artillery shells that split into
-- fragments near the end of their arc. The engine detonates a shell when it
-- falls through its blueprint `DetonateBelowHeight` (height above the
-- surface): `OnImpact('Air')`, and FAF's script then
-- `CreateChildProjectile`s the fragments, spread around the shell's course,
-- each carrying the shell's damage.
--   * UEF T1 mobile artillery (Lobo, uel0103): splits 3 above the ground
--     into 5 fragments.
--   * Aeon T4 artillery (Salvation, xab2307): splits 45 above the ground
--     into 6, each of which splits again 24 above it into 6 (36 pieces).
-- Logs each split (height above the surface, seconds after launch) and each
-- final fragment's impact (offset from the target, damage it dealt).
-- Run alone: cargo run -- --headless --no-ai --ticks 3000 --script tests/scenarios/fragments.lua
-- (set FRAGMENTS_FILTER, a pattern on the shooter id, to run one; with a
-- window, `--screenshot` around the logged split tick shows the burst).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) LOG('fragments: ', msg) end
local function done() ScenarioResult.done = true end
local function ground(x, z) return GetTerrainHeight(x, z) end
local filter = rawget(_G, 'FRAGMENTS_FILTER')

-- Flat land on the default map.
local CX, CZ = 384, 896

-- shooter, weapon, distance to the target, shots, expected splits: per
-- generation the detonation height and the pieces each shell makes.
local trials = {
    { 'uel0103', 'MainGun', 22, 5, { { height = 3, pieces = 5 } } },
    { 'xab2307', 'MainGun', 300, 3, { { height = 45, pieces = 6 }, { height = 24, pieces = 6 } } },
}

local function run(t)
    local sbp, label, range, wantShots, generations = unpack(t)
    -- Shooter south of the target, facing it (north = +z).
    local sx, sz = CX, CZ - range
    local shooter = CreateUnitHPR(sbp, 'ARMY_1', sx, ground(sx, sz), sz, 0, 0, 0)
    local target = CreateUnitHPR('uel0201', 'ARMY_2', CX, ground(CX, CZ), CZ, 0, 0, 0)
    target:SetMaxHealth(1e7)
    target:SetHealth(nil, 1e7)
    target:SetFireState(1)
    shooter:SetFireState(1)
    local w = shooter:GetWeaponByLabel(label)
    -- The Salvation's shell takes 15000 energy: free shots.
    local brain = ArmyBrains[shooter.Army]
    brain:__engine_SetStorage('ENERGY', 1e6)
    brain:GiveResource('ENERGY', 1e6)
    local shots, splits, finals, damage = 0, {}, {}, 0
    local live = 0
    local function follow(p, generation, launchTick, shot)
        live = live + 1
        local ccp = p.CreateChildProjectile
        p.CreateChildProjectile = function(pr, bp)
            local child = ccp(pr, bp)
            follow(child, generation + 1, launchTick, shot)
            return child
        end
        local oi = p.OnImpact
        p.OnImpact = function(pr, kind, other)
            local pos = pr:GetPosition()
            local before = target:GetHealth()
            local r = oi(pr, kind, other)
            local dealt = math.max(0, before - target:GetHealth())
            if generation <= table.getn(generations) and kind == 'Air' then
                local surface = GetSurfaceHeight(pos[1], pos[3])
                local split = { shot = shot, generation = generation, height = pos[2] - surface,
                    time = (GetGameTick() - launchTick) / 10, tick = GetGameTick(), x = pos[1], z = pos[3] }
                table.insert(splits, split)
                LOG(string.format('fragments split %s shot %d gen %d: %.2f above the surface, %.1fs after launch (tick %d) at %.1f, %.1f',
                    sbp, shot, generation, split.height, split.time, split.tick, pos[1], pos[3]))
            else
                local tp = target:GetPosition()
                local f = { shot = shot, x = pos[1] - tp[1], z = pos[3] - tp[3], damage = dealt, kind = kind }
                table.insert(finals, f)
                damage = damage + dealt
                LOG(string.format('fragments impact %s shot %d gen %d: %s at %.2f, %.2f from the target, damage %d',
                    sbp, shot, generation, kind, f.x, f.z, dealt))
            end
            live = live - 1
            return r
        end
    end
    local orig = w.CreateProjectileForWeapon
    w.CreateProjectileForWeapon = function(self, ...)
        local proj = orig(self, ...)
        if proj and not proj:BeenDestroyed() and shots < wantShots then
            shots = shots + 1
            follow(proj, 1, GetGameTick(), shots)
        end
        return proj
    end
    shooter:SetFireState(0)
    IssueAttack({ shooter }, target)
    for _ = 1, 3000 do
        WaitTicks(1)
        if shots >= wantShots and live == 0 then break end
    end
    shooter:SetFireState(1)

    -- Per generation: how many shells split there, and where.
    local pieces = 1
    for g, want in generations do
        local n, hsum, hmin, hmax, tsum = 0, 0, 1e9, -1e9, 0
        for _, s in splits do
            if s.generation == g then
                n = n + 1
                hsum, tsum = hsum + s.height, tsum + s.time
                hmin, hmax = math.min(hmin, s.height), math.max(hmax, s.height)
            end
        end
        local expect = shots * pieces
        note(string.format('%s generation %d: %d of %d shells split, %.2f..%.2f (mean %.2f) above the surface, mean %.1fs after launch',
            sbp, g, n, expect, n > 0 and hmin or 0, n > 0 and hmax or 0, n > 0 and hsum / n or 0, n > 0 and tsum / n or 0))
        check(n == expect, string.format('%s generation %d: %d of %d shells split', sbp, g, n, expect))
        check(n == 0 or (hmin > want.height - 0.5 and hmax < want.height + 0.5),
            string.format('%s generation %d: split %.2f..%.2f above the surface, not %d', sbp, g, hmin, hmax, want.height))
        pieces = pieces * want.pieces
    end
    -- The last generation's fragments: count, spread and damage per shot.
    local perShot = {}
    for _, f in finals do
        local s = perShot[f.shot] or { n = 0, sx = 0, sz = 0, list = {}, damage = 0, hits = 0 }
        perShot[f.shot] = s
        s.n, s.sx, s.sz, s.damage = s.n + 1, s.sx + f.x, s.sz + f.z, s.damage + f.damage
        if f.damage > 0 then s.hits = s.hits + 1 end
        table.insert(s.list, f)
    end
    for shot = 1, shots do
        local s = perShot[shot] or { n = 0, sx = 0, sz = 0, list = {}, damage = 0, hits = 0 }
        local mx, mz = s.n > 0 and s.sx / s.n or 0, s.n > 0 and s.sz / s.n or 0
        local spread, far = 0, 0
        for _, f in s.list do
            local d = math.sqrt(math.pow(f.x - mx, 2) + math.pow(f.z - mz, 2))
            spread, far = spread + d, math.max(far, d)
        end
        note(string.format('%s shot %d: %d fragments landed, centred %.1f, %.1f from the target, %.1f mean / %.1f max from their centre, %d damaged the target (%d damage)',
            sbp, shot, s.n, mx, mz, s.n > 0 and spread / s.n or 0, far, s.hits, s.damage))
        check(s.n == pieces, string.format('%s shot %d: %d of %d fragments landed', sbp, shot, s.n, pieces))
    end
    check(shots >= wantShots, string.format('%s fired %d of %d shots', sbp, shots, wantShots))
    check(damage > 0, sbp .. ': no fragment damaged the target')
    shooter:Destroy()
    target:Destroy()
    WaitTicks(10)
end

ForkThread(function()
    WaitTicks(2)
    -- Radar so the shooter's army sees past its own vision; a far-off unit
    -- keeps ARMY_2 in the game between trials.
    CreateUnitHPR('ueb3101', 'ARMY_1', CX - 30, ground(CX - 30, CZ - 20), CZ - 20, 0, 0, 0)
    CreateUnitHPR('ueb3104', 'ARMY_1', CX - 40, ground(CX - 40, CZ - 150), CZ - 150, 0, 0, 0)
    CreateUnitHPR('ueb3101', 'ARMY_2', CX + 200, ground(CX + 200, CZ - 300), CZ - 300, 0, 0, 0)
    for _, t in trials do
        if not filter or string.find(t[1], filter) then run(t) end
    end
    done()
end)
