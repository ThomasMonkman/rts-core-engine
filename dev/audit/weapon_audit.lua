-- dev/audit/weapon_audit.lua: one real engagement per weapon blueprint.
-- Enumerates every weapon-bearing unit blueprint (sorted, like
-- all_units.lua); for each blueprint weapon builds a case
-- (attacker bp, weapon index, target, geometry, budget; every budget, deadline
-- and settle is in game ticks, measured with GetGameTick, never by counting
-- WaitTicks yields: a WaitTicks(n) wakes n - 1 ticks later): the attacker
-- (ARMY_1) at one end of a reserved cell, the target (ARMY_2, hold fire,
-- immortal) at max(MinRadius+2, MaxRadius*0.9) along the row.
-- The census (src/weapon_audit.rs: --audit-table, --audit-test,
-- tests/weapon_audit.rs) runs EVERY CASE IN A SESSION OF ITS OWN
-- (WEAPON_AUDIT_CASE = id) on /maps/audit_duel, after one plan session
-- (WEAPON_AUDIT_PLAN) that resolves the statically known cases (non-attack
-- and unsupported) and lists the rest: cases sharing a session observed each
-- other (neighbours acquired, blocked and hit each other's units; the
-- per-tick path budget and entity-id staggering coupled them; and a case's
-- absolute position changes its result), so a row is only the case's own
-- when nothing else runs. A single case's strip is the one nearest the map
-- centre that fits its media (row 0 through the centre, then rows north).
-- The target kind: dev/audit/scenario.lua lists the kinds the weapon's
-- TargetRestrictOnlyAllow / Disallow allow, in preference order (building,
-- land tank, surface ship, sub, fighter; the fighter first for AA weapons),
-- and the weapon's LIVE caps a few ticks after spawning the attackers pick
-- the first on a layer it can fire at (Land -> building / tank, Water ->
-- frigate, Water/Sub/Seabed -> FA's T1 sub spawned surfaced, Air ->
-- fighter). An explicit override target applies only when the restrictions
-- allow it and it does not fight those caps, or when the caps are empty —
-- the class the overrides exist for. Empty caps -> UNSUPPORTED (no-caps),
-- caps with no allowed kind -> UNSUPPORTED (restrict).
-- Anti-projectile weapons (TargetType RULEWTT_Projectile): a tactical
-- anti-missile (scenario intercept) has a SOURCE at the far end instead of
-- a target: the faction T2 tactical launcher (catalog tml, ARMY_2,
-- immortal) 150 away, given one missile, IssueTactical at the attacker's
-- position; a strategic one (the SMDs, given one interceptor as tactical
-- silo ammo) the faction nuke launcher (catalog nuke) 450 away, IssueNuke;
-- an anti-torpedo one the faction T1 torpedo launcher (catalog torpedo,
-- floating, at the surface) 40 away, IssueAttack on the attacker. The attacker is immortal, gets no order and its fire state goes
-- normal with the others'; the unit's other anti-projectile weapons are
-- switched off (SetWeaponEnabledByLabel) so they can't race it for the
-- missile. The damage column counts the source's missiles STOPPED: damaged
-- by the case's weapon (credit as below), named by the source's
-- OnMissileIntercepted (defense = the attacker, after this weapon fired), or
-- redirected (IsRedirected: flares, the Loyalist's deflector; or retargeted
-- with SetNewTarget onto one of the case's projectiles: FAF's DepthCharge,
-- the Smart Charges and Cybran anti-torpedo flares) without
-- damaging the attacker; the stage text names the rule and the damage
-- missiles did to the attacker.
-- The attacker's layer (scenario.attacker_layer): a key of the weapon's
-- FireTargetLayerCapsTable the unit can occupy, the natural spawn layer
-- first; the spawn medium follows it (floating units at the surface). A
-- SurfacingSub firing from the other depth gets IssueDive, and its attack
-- order once there (DIVE_WAIT). After the spawn settle an attacker off its
-- planned layer is UNSUPPORTED (layer); an attacker that replaced itself
-- (HARMS xrb2308 -> xrb2309) hands its case to the new unit on its spot.
-- Economy and enhancements: ARMY_1 has 10M energy stored before anything
-- spawns; an EnabledByEnhancement weapon's enhancement is made after its
-- unit spawns (Prerequisite chain root first, presets keep theirs); the
-- OverCharge weapon fires through IssueOverCharge at the target, the
-- AutoOverCharge weapon through SetAutoOvercharge(true) and the attack.
-- Silo launchers (CountedProjectile + ManualFire, not anti-projectile):
-- one missile given (GiveNukeSiloAmmo / GiveTacticalSiloAmmo by
-- NukeWeapon), IssueNuke / IssueTactical at the target's position, the
-- target at 0.9 MaxRadius capped at D_SILO, budget from the missile's speed.
-- Long range (MaxRadius > 600): the target at 0.9 MaxRadius, clamped to
-- D_SILO (the 4000-range guns: the map holds no more; the case line says
-- 'map clamp' with the window 0.75..0.9 MaxRadius it could not reach);
-- the budget adds the high-arc flight, the turret's pitch slew, ten reloads
-- and the unpack (WeaponUnpacks), and is not held to the 1800 cap.
-- A case that never damaged its target names its nearest impact.
-- Threads forked while a case's damage code runs (nuke rings, DoT) are the
-- case's: their later damage credits it.
-- Weapons that are not attacks (death, teleport, guidance, dummy weapons;
-- see non_attack_of) never engage: outcome NON-ATTACK, so UNSUPPORTED only
-- lists real weapons the audit can't set up yet.
-- Attackers hold fire from spawn (no auto-acquisition before their order);
-- once the targets exist every case gets its one IssueAttack (units table,
-- target as entity) and all attackers go fires-on the same tick. Stage
-- trail (behaviour-neutral, only counters wrapped around real calls):
--   fired  = CreateProjectileAtMuzzle count on the case's weapon instance
--            (beam weapons start their beam there), plus any
--            CreateProjectileForWeapon call made outside it (FAF's
--            CreateProjectileAtMuzzle chains it: counted once per shot);
--   held   = the weapon holds a target (WeaponHasTarget/GetCurrentTarget)
--            and whether it is the case's target;
--   credit = per weapon instance, never per unit: the case's projectiles
--            (and their fragments / child projectiles) have OnImpact
--            wrapped, its beams' CollisionBeam OnImpact too, and the
--            target's OnDamage counts damage only while one of those runs
--            for this case (Damage/DamageArea call OnDamage synchronously).
--            The unit's other weapons pass nothing on (FAF names the
--            launcher unit as every weapon's instigator); TreeForce /
--            TreeFire (ignored by Unit.OnDamage) never count; damage from
--            the attacker outside them shows as 'uncredited unit dmg'.
-- Outcome per case: pass (credit > 0 within its budget) | no-target (never
-- acquired) | held-no-fire (acquired a target, never fired) | fired-no-hit
-- (fired, nothing landed) | hit-no-damage (impacted, 0 credited) |
-- non-attack | unsupported | map-nosupport. Never-engaged rows carry a
-- reason token (reasonKey: death/teleport/guidance/dummy/not-attack for
-- non-attack; silo/anti-proj:torp/anti-proj:missile/restrict/
-- no-caps/layer/spawn/no-cell for the others) that the table prints and
-- tallies in its legend. One
--   AUDIT <key> <attacker>[<weapon>] vs <target>: <outcome> (fired n, dmg n <stage>)
-- line per case, and machine rows in case order:
--   _G.AUDIT_RESULTS[i] = 'key|attacker|weaponIdx|target|outcome|fired|damage'
--   _G.AUDIT_CASES[i]   = { id=, key=, bearing=, aim=, attacker=, weapon=,
--       target=, x=, z=, budget=, attName=, weaponName=, tgtName=, reason=,
--       reasonKey= }
-- Case ids are 1..N from the (sorted) blueprint walk and STABLE across runs.
-- Bearings (1b below): a Turreted weapon on a ground / naval unit or
-- structure (not silo, intercept, overcharge or long-range cases) also runs
-- with its target at other bearings off the attacker's facing: sub-cases
-- keyed 'id@bearing' (degrees; positive = the side positive
-- HeadingArcCenter points, the unit's right), each its own session, rows
-- right after the canonical row (the plain id, target straight ahead, which
-- stays as it was) in clockwise order. The attacker faces +z, the target
-- stands at the case's distance on the bearing, the line's midpoint as near
-- the map centre as the media allow. WEAPON_AUDIT_BEARINGS = 'gate'
-- (default, the golden's: 180 plus the bearing nearest a limited arc's
-- centre), 'all' (every 45 degrees), 'none' or a list ('90,135'); a
-- structure only takes bearings inside its arc. Each case's AUDIT line ends
-- with its aim evidence (aim_of: bearing, body turn, arc, the turret's yaw
-- offset at the first shot, mean miss across / along the line);
-- dev/audit/bearing_report.py tabulates it. WEAPON_AUDIT_TRACE = true logs
-- AUDITTRACE lines (turn, aim heading / pitch, target position) every 20
-- ticks per open case.
-- Globals set before the run select cases: WEAPON_AUDIT_CASE = n or 'n@b'
-- (the case stands alone, its engagement midpoint at the map centre: the
-- census's session for that case; any bearing b of a weapon that takes
-- bearings) or WEAPON_AUDIT_IDS = 'a,b,c' (same ids as the full run, a bare
-- id with its listed bearings, 'n@b' one bearing; all in ONE session in the
-- scan layout, where bearing sub-cases are map-nosupport: x-strips on rows, probed like
-- all_units.lua's place — for watching several engagements at once on the
-- 81 km /maps/audit_flat; those cases can interfere, so their rows are not
-- the census's); no globals = all of them in one session that way.
-- WEAPON_AUDIT_PLAN = true (with or without WEAPON_AUDIT_IDS) is the
-- census's plan pass: the statically known cases (non-attack, unsupported)
-- resolve (rows as usual), every other id goes to _G.AUDIT_PENDING
-- ({ id =, key =, budget = }) and nothing spawns. A selection matching nothing
-- fails the run naming the case count.
-- Needs --fog none (the engagement search must see targets), the target
-- catalog from dev/audit/targets.lua and the scenario from
-- dev/audit/scenario.lua: cat-join both ahead of this file (the canonical
-- route: run the three files as one --script), or rawset
-- WEAPON_AUDIT_CATALOG / WEAPON_AUDIT_OVERRIDES / WEAPON_AUDIT_SCENARIO
-- before this file runs (the dev/standalone injection point).
rawset(_G, 'ScenarioResult', { done = false, failures = {}, notes = {} })

local function fail(msg)
    LOG('audit: FAIL: ', msg)
    table.insert(ScenarioResult.failures, msg)
end
local function note(msg) table.insert(ScenarioResult.notes, msg) LOG('audit: ', msg) end

ForkThread(function()
    WaitTicks(2)

    local catalog = rawget(_G, 'WEAPON_AUDIT_CATALOG')
    if not catalog then
        fail('dev/audit/targets.lua not loaded: cat-join it ahead of this file or set it as a prelude global')
        ScenarioResult.done = true
        return
    end
    local overrides = rawget(_G, 'WEAPON_AUDIT_OVERRIDES') or {}
    local scenario = rawget(_G, 'WEAPON_AUDIT_SCENARIO')
    if not scenario then
        fail('dev/audit/scenario.lua not loaded: cat-join it ahead of this file or set it as a prelude global')
        ScenarioResult.done = true
        return
    end

    local mapSize = 1024
    if GetMapSize then mapSize = GetMapSize() end
    if type(mapSize) == 'table' then mapSize = mapSize.x or mapSize[1] or 1024 end
    local margin, lane = 60, 10
    local dry_at = function(x, z) return math.abs(GetSurfaceHeight(x, z) - GetTerrainHeight(x, z)) < 0.25 end
    local wet_at = function(x, z) return GetSurfaceHeight(x, z) > GetTerrainHeight(x, z) + 1.5 end
    local ground = function(x, z) return GetTerrainHeight(x, z) end

    local wanted
    -- WEAPON_AUDIT_CASE: n, or 'n@bearing' (a bearing sub-case, 1b below)
    local sel = string.match(tostring(rawget(_G, 'WEAPON_AUDIT_CASE') or ''), '^%s*(%d+)')
    sel = sel and tonumber(sel)
    if sel then
        wanted = { [math.floor(sel)] = true }
    elseif type(rawget(_G, 'WEAPON_AUDIT_IDS')) == 'string' then
        -- bare ids here; 'id@bearing' tokens are read with the bearings (1b)
        local set, any = {}, false
        for tok in string.gmatch(rawget(_G, 'WEAPON_AUDIT_IDS'), '[^,%s]+') do
            any = true
            local n = string.match(tok, '^(%d+)$')
            if n then set[tonumber(n)] = true end
        end
        if any then wanted = set end
    end

    local function foot(bp)
        local f, p = bp.Footprint or {}, bp.Physics or {}
        return math.max(f.SizeX or 1, f.SizeZ or 1, p.SkirtSizeX or 0, p.SkirtSizeZ or 0, bp.SizeX or 1, bp.SizeZ or 1)
    end
    local function is_air(bp)
        local h = bp.CategoriesHash or {}
        return h.AIR ~= nil or (bp.Physics and bp.Physics.MotionType or '') == 'RULEUMT_Air'
    end
    -- Blueprint text is `<LOC tag>English` when source-localized; the tag
    -- is dropped for display (no localization machinery, raw English only).
    local function loc_text(s)
        if type(s) ~= 'string' then return nil end
        local t = s
        local _, e = string.find(s, '^%b<>')
        if e then t = string.sub(s, e + 1) end
        if string.find(t, '%S') then return t end
        return nil
    end
    local FACTIONS = { UEF = 'UEF', AEON = 'Aeon', CYBRAN = 'Cybran', SERAPHIM = 'Seraphim' }
    local TIERS = { TECH1 = 'T1', TECH2 = 'T2', TECH3 = 'T3', TECH4 = 'T4', TECH5 = 'T5' }
    -- `<faction> <Tn> <Description>` plus the proper name when the bp has one.
    local function unit_name(bp)
        local g = bp.General or {}
        local d = loc_text(bp.Description) or loc_text(g.UnitName) or 'unit'
        local un = loc_text(g.UnitName)
        local tag = (FACTIONS[bp.FactionCategory or ''] or bp.FactionCategory or '')
            .. ' ' .. (TIERS[bp.TechCategory or ''] or '')
        local name = (tag ~= ' ' and tag .. ' ' or '') .. d .. (un and ': ' .. un or '')
        return (string.gsub(name, '%s%s+', ' '))
    end
    local function starts(s, pre) return string.sub(s, 1, #pre) == pre end
    -- The table's short token for an unsupported / map-nosupport reason
    -- (the full sentences stay on the case and in the log; the table prints
    -- the token and a legend line tallies them).
    local function reason_key(s)
        if not s then return nil end
        if starts(s, 'death weapon') then return 'death' end
        if starts(s, 'teleport') then return 'teleport' end
        if starts(s, 'guidance') then return 'guidance' end
        if starts(s, 'dummy') then return 'dummy' end
        if starts(s, 'no projectile and no damage') then return 'not-attack' end
        if starts(s, 'silo') then return 'silo' end
        if starts(s, 'anti-projectile (anti-torpedo)') then return 'anti-proj:torp' end
        if starts(s, 'anti-projectile') then return 'anti-proj:missile' end
        if starts(s, 'restrictions') or starts(s, 'no allowed target') then return 'restrict' end
        if starts(s, 'no reachable layer') then return 'no-caps' end
        if starts(s, 'attacker did not survive') then return 'spawn' end
        return nil
    end
    -- A weapon that is not an attack: it never damages a target by itself,
    -- so no engagement can pass it (outcome non-attack, not unsupported).
    -- Death weapons fire through the unit's death; the teleport module;
    -- the transports' GuidanceSystem (aims the cargo's weapons, Damage 0);
    -- dummy weapons (a Dummy label, or Damage 0 and no projectile: the
    -- shield / stealth / scout target trackers, target painters, the
    -- ACU's Chrono Dampener stun); a weapon with neither projectile nor
    -- Damage field at all.
    local function non_attack_of(label, wbp)
        if label == 'DeathWeapon' or string.find(label, 'Death') then
            return 'death weapon'
        elseif label == 'TeleportWeapon' then
            return 'teleport module: not an attack weapon'
        elseif string.find(label, 'Guidance') then
            return 'guidance system: aims other weapons, no damage of its own'
        elseif string.find(label, 'Dummy') or (wbp.Damage == 0 and wbp.ProjectileId == nil) then
            return 'dummy weapon: Damage 0 and no projectile, or a Dummy label (target trackers, painters, effects)'
        elseif wbp.Damage == nil and wbp.ProjectileId == nil then
            return 'no projectile and no damage: not an attack weapon'
        end
        return nil
    end
    local function weapon_name(wbp, wi)
        local n = loc_text(wbp.DisplayName) or wbp.Label
        if n and string.find(n, '%S') then return n end
        return 'weapon' .. wi
    end
    -- A target blueprint's spawn medium (attackers plan theirs by layer:
    -- scenario.attacker_layer).
    local function medium(bp)
        local h = bp.CategoriesHash or {}
        local mt = bp.Physics and bp.Physics.MotionType or ''
        if h.NAVAL ~= nil and mt ~= 'RULEUMT_Air' then return 'wet' end
        if mt == 'RULEUMT_Water' or mt == 'RULEUMT_SurfacingSub' or mt == 'RULEUMT_Sub'
            or (mt == 'RULEUMT_Hover' and h.NAVAL ~= nil) then
            return 'wet'
        end
        return 'dry'
    end
    -- The weapon's cap-table row for a layer ('' when none).
    local function bp_caps(wbp, layer)
        local set = wbp.FireTargetLayerCapsTable and wbp.FireTargetLayerCapsTable[layer]
        return type(set) == 'string' and set or ''
    end
    -- How long a SurfacingSub gets to change depth after its IssueDive, in
    -- game ticks (GetGameTick deltas; a WaitTicks(n) wakes n - 1 ticks later,
    -- so no budget here counts yields). Both
    -- engines take 40 ticks at Elevation -1.5, 52 at -2, 93 at -3.6 and 129
    -- at -5 (probe_subdive).
    local DIVE_WAIT = 120
    -- Does an override's target pass the weapon's restrictions? A catalog
    -- key must be one of the scenario's kinds; a bp id is checked directly.
    local function override_allowed(o, wbp, kinds)
        local t = o.target
        if t == nil then return true end
        if catalog[t] then
            local k = t == 'naval' and 'ship' or t
            for _, kind in kinds do
                if kind == k then return true end
            end
            return false
        end
        return scenario.allows(wbp, __blueprints[t])
    end
    -- Does an override target fit a caps string ('Air|Land|...')? A
    -- catalog key by its kind's layers, a bp id by its medium.
    local function fits_caps(t, caps)
        if catalog[t] then return scenario.pick_kind({ t }, caps) ~= nil end
        local set = {}
        for l in string.gmatch(caps, '[^|]+') do set[string.upper(l)] = true end
        local tbp = __blueprints[t] or {}
        if is_air(tbp) then return set.AIR == true end
        if medium(tbp) == 'wet' then return set.WATER or set.SEABED or set.SUB or false end
        return set.LAND == true
    end
    -- Ticks a weapon that charges from the economy (EnergyRequired at
    -- EnergyDrainPerSecond) takes before its first shot.
    local function charge_of(wbp)
        local need, rate = wbp.EnergyRequired or 0, wbp.EnergyDrainPerSecond or 0
        if need > 0 and rate > 0 then return math.ceil(need / rate * 10) end
        return 0
    end
    -- Silo launches: the strategic missiles' distance cap (the audit map is
    -- 1024 across: the case's strip, centred, must fit inside its margins),
    -- and the projectile's own flight speed (MuzzleVelocity is ~0 for
    -- missiles that boost from the silo).
    local D_SILO = 800
    local function proj_speed(wbp)
        local pbp = wbp.ProjectileId and __blueprints[string.lower(wbp.ProjectileId)]
        local ph = pbp and pbp.Physics or {}
        return ph.MaxSpeed or 0
    end
    -- A silo missile's budget: unpack and launch, then the flight at the
    -- projectile's MaxSpeed (strategic missiles climb first: 2.5x the ground
    -- distance), then the generic settle.
    local function silo_budget(wbp, dist)
        local speed = math.max(proj_speed(wbp), wbp.MuzzleVelocity or 0, 5)
        return math.floor(200 + dist * 10 / speed * (wbp.NukeWeapon and 2.5 or 1.5) + 300)
    end
    -- Anti-projectile cases (scenario intercept): the source stands this far
    -- from the attacker (tactical launchers: MinRadius 25, MaxRadius 256).
    -- Strategic: 450, as dev/probes/maps/probe_antinuke (the nuke climbs
    -- and dives; the SMD's interceptor needs the room to meet it).
    -- Torpedo: 40, inside the T1 torpedo launcher's 50.
    local D_INTERCEPT = { tactical = 150, strategic = 450, torpedo = 40 }
    -- The source's weapon blueprint and index: its first CountedProjectile
    -- weapon (a silo, launched by order), else its first weapon with a
    -- projectile (the torpedo launcher, which attacks).
    local function source_weapon(sbp)
        local first, firstI
        for wi, w in (sbp and sbp.Weapon) or {} do
            if type(w) == 'table' then
                if w.CountedProjectile then return w, wi end
                if not first and w.ProjectileId and not string.find(w.Label or '', 'Death') then first, firstI = w, wi end
            end
        end
        return first, firstI
    end
    -- A high-arc shell's flight (ticks) to dist at its muzzle velocity
    -- (gravity 4.9, the steep solution); 0 when out of reach.
    local function high_arc_ticks(wbp, dist)
        local v = wbp.MuzzleVelocity or 0
        if wbp.BallisticArc ~= 'RULEUBA_HighArc' or v <= 2 then return 0 end
        local sn = dist * 4.9 / (v * v)
        if sn > 1 then return 0 end
        local theta = (math.pi - math.asin(sn)) / 2
        return math.ceil(2 * v * math.sin(theta) / 4.9 * 10)
    end
    -- Long range (MaxRadius > LONG): the turret's pitch slew up to the high
    -- arc (ticks, at TurretPitchSpeed degrees a second, from level), and the
    -- reloads given: at 742 a FiringRandomness of 0.35-0.47 scatters the
    -- shells round the target (measured: sigma ~13 per axis) and a shell
    -- lands within DamageRadius of the 2x2 building ~1 time in 8. Long-range
    -- budgets are not held to the generic 1800 cap (SAFETY_CAP 4000 holds).
    local LONG, LONG_SHOTS = 600, 10
    local function pitch_ticks(wbp, dist)
        local v, speed = wbp.MuzzleVelocity or 0, wbp.TurretPitchSpeed or 0
        if wbp.BallisticArc ~= 'RULEUBA_HighArc' or v <= 2 or speed <= 0 then return 0 end
        local sn = math.min(dist * 4.9 / (v * v), 1)
        local deg = math.deg((math.pi - math.asin(sn)) / 2)
        local top = (wbp.TurretPitch or 0) + (wbp.TurretPitchRange or 90)
        return math.ceil(math.min(deg, top) / speed * 10)
    end
    local function budget_of(bp, wbp, dist, air)
        local close = air and 120 or 40
        local muzzle = wbp.MuzzleVelocity or 0
        local flight = math.max(dist * 10 / math.max(muzzle > 2 and muzzle or 35, 8), high_arc_ticks(wbp, dist))
            + (wbp.WeaponUnpacks and 100 or 0)
        local reload = 10 / math.max(wbp.RateOfFire or 1, 0.05)
        local fire = reload + 50
        if (wbp.MaxRadius or 0) > LONG then
            fire = pitch_ticks(wbp, dist) + LONG_SHOTS * reload + 50
        end
        local base = math.max(close + flight + fire + (air and 400 or 250), 300)
        if (wbp.MaxRadius or 0) <= LONG then base = math.min(base, 1800) end
        return math.floor(base + charge_of(wbp))
    end
    -- Case walk: sorted blueprint ids, weapon index ascending, ids 1..N.
    local ids = {}
    for id, bp in __blueprints do
        if type(id) == 'string' and bp.Categories and bp.Display and bp.Display.Mesh
            and id == string.lower(bp.BlueprintId or id)
            and not string.find(id, '/')
            and not (bp.CategoriesHash and (bp.CategoriesHash.DUMMYUNIT or bp.CategoriesHash.INSIGNIFICANTUNIT))
        then
            table.insert(ids, id)
        end
    end
    table.sort(ids)

    local cases = {}
    local count = 0
    for _, id in ids do
        local bp = __blueprints[id]
        if bp.Weapon == nil and bp.Weapons ~= nil then bp.Weapon = bp.Weapons end
        if type(bp.Weapon) == 'table' then
            for wi, wbp in bp.Weapon do
                if type(wbp) == 'table' then
                    local label = wbp.Label or ('weapon' .. wi)
                    local reason, ov
                    local nonAttack = non_attack_of(label, wbp)
                    -- silo launchers fire on IssueNuke / IssueTactical with
                    -- ammo given (anti-projectile silos, the SMDs, keep their
                    -- anti-projectile verdict from scenario_of)
                    local silo = wbp.CountedProjectile and wbp.ManualFire
                        and wbp.TargetType ~= 'RULEWTT_Projectile'
                        and (wbp.NukeWeapon and 'nuke' or 'tactical') or nil
                    if nonAttack then
                        reason = nonAttack
                    elseif wbp.CountedProjectile and not silo and wbp.TargetType ~= 'RULEWTT_Projectile' then
                        reason = 'silo weapon: needs built-up ammo'
                    end
                    local sc = { kinds = {} }
                    local al = scenario.attacker_layer(bp, wbp)
                    if not reason then
                        sc = scenario.scenario_of(bp, wbp, wi, catalog)
                        reason = sc.unsupported
                    end
                    local maxR = wbp.MaxRadius or 0
                    local dist = math.max((wbp.MinRadius or 0) + 2, maxR * 0.9)
                    -- long range: the window is max(MinRadius + 2,
                    -- 0.75 MaxRadius) .. 0.9 MaxRadius, clamped to what the
                    -- map holds (D_SILO); the 825-range artillery fits at
                    -- 0.9, the 4000-range guns are clamped below the window
                    local clamped
                    if dist > D_SILO then
                        if not silo then
                            clamped = math.floor(math.max((wbp.MinRadius or 0) + 2, maxR * 0.75)) .. '..' .. math.floor(maxR * 0.9)
                        end
                        dist = D_SILO
                    end
                    -- an intercept case: the source's missile flies at the
                    -- attacker from D_INTERCEPT away
                    local icpt = not reason and sc.intercept or nil
                    local srcId, srcW, srcWi
                    if icpt then
                        dist = D_INTERCEPT[icpt]
                        srcId = scenario.target_of(catalog, sc.source, id)
                        srcW, srcWi = source_weapon(srcId and __blueprints[srcId])
                        if not srcW then
                            reason = 'anti-projectile: no ' .. sc.source .. ' source in the catalog'
                            icpt = nil
                        end
                    end
                    local air = is_air(bp)
                    -- silo and intercept cases take no override rows (their
                    -- own order, distance and budget)
                    if not reason and not silo and not icpt then
                        for _, o in overrides do
                            local hit = o.attacker == nil or (type(o.attacker) == 'string' and string.find(id, '^' .. o.attacker))
                            if hit and o.label ~= nil and not (type(o.label) == 'string' and string.find(label, o.label)) then
                                hit = false
                            end
                            if hit and o.air ~= nil and o.air ~= air then
                                hit = false
                            end
                            if hit and o.weapon ~= nil and o.weapon ~= wi then
                                hit = false
                            end
                            -- a row whose target the weapon may not engage
                            -- does not apply (nor its geometry / budget)
                            if hit and not override_allowed(o, wbp, sc.kinds) then
                                hit = false
                            end
                            -- nor a row whose target fights the weapon's caps
                            -- on its planned layer (torpedo bombers carry a
                            -- Bomb label but fire at Water only); empty caps
                            -- are what the rows exist for
                            local row = bp_caps(wbp, al.layer)
                            if hit and o.target and row ~= '' and not fits_caps(o.target, row) then
                                hit = false
                            end
                            if hit then
                                ov = o
                                break
                            end
                        end
                        if ov then
                            reason = ov.unsupported
                            if ov.distance then dist = ov.distance end
                        end
                    end
                    count = count + 1
                    -- the attacker stands on a layer its weapon fires from;
                    -- that layer's cap-table row predicts the kind (and so
                    -- the far end's medium) for placement; the live read
                    -- stays authoritative
                    local kind, env = scenario.pick_kind(sc.kinds, bp_caps(wbp, al.layer))
                    -- the source structure's medium at the far end, from
                    -- the layer it builds on (the torpedo launcher floats)
                    if icpt then kind, env = 'source', scenario.attacker_layer(__blueprints[srcId], srcW).medium end
                    -- an override target places the far end for its own
                    -- medium (as step 4 spawns it)
                    if ov and ov.target then
                        local t = ov.target
                        if catalog[t] then
                            env = scenario.kind_env[t]
                        elseif __blueprints[t] then
                            env = is_air(__blueprints[t]) and 'any' or medium(__blueprints[t])
                        end
                    end
                    cases[count] = {
                        id = count,
                        -- the row id: the canonical case is its id, a
                        -- bearing sub-case 'id@bearing' (bearing_case)
                        key = tostring(count),
                        bearing = 0,
                        attacker = id,
                        weapon = wi,
                        label = label,
                        attName = unit_name(bp),
                        weaponName = weapon_name(wbp, wi),
                        maxR = maxR,
                        dist = dist,
                        air = air,
                        attMedium = al.medium,
                        attLayer = al.layer,
                        spawnLayer = al.spawn,
                        dive = al.dive,
                        attFoot = foot(bp),
                        predKind = kind,
                        predEnv = env,
                        unsupported = reason,
                        nonAttack = nonAttack ~= nil,
                        reasonKey = sc.reasonKey or reason_key(reason),
                        kinds = sc.kinds,
                        oTarget = ov and ov.target or nil,
                        -- the enhancement that enables the weapon (its
                        -- Prerequisite chain is built first)
                        enh = wbp.EnabledByEnhancement,
                        -- overcharge: 'manual' (IssueOverCharge at the
                        -- target), 'auto' (SetAutoOvercharge(true))
                        oc = (label == 'AutoOverCharge' and 'auto') or (wbp.OverChargeWeapon and 'manual') or nil,
                        -- silo launch: 'nuke' | 'tactical'
                        silo = silo,
                        -- the long-range window the map clamped ('lo..hi')
                        clamped = clamped,
                        -- intercept: 'tactical'; the source bp and its
                        -- launch order ('nuke' | 'tactical')
                        icpt = icpt,
                        source = srcId,
                        srcOrder = srcW and ((not srcW.CountedProjectile and 'attack') or (srcW.NukeWeapon and 'nuke') or 'tactical') or nil,
                        srcWeapon = srcWi,
                        missiles = icpt and {} or nil,
                        stopped = 0,
                        attHit = 0,
                        budget = ((ov and ov.budget) or (silo and silo_budget(wbp, dist))
                                or (icpt and silo_budget(srcW, dist)) or budget_of(bp, wbp, dist, air))
                            + (al.dive and DIVE_WAIT or 0),
                        x = 0,
                        z = 0,
                        fired = 0,
                        impacted = 0,
                        dmg = 0,
                        unitDmg = 0,
                    }
                end
            end
        end
    end
    if count == 0 then
        fail('no weapon-bearing blueprints found')
        ScenarioResult.done = true
        return
    end

    -- 1b. Bearings. A turreted weapon's case also runs with its target at
    -- other bearings off the attacker's facing: the attacker faces +z
    -- (heading 0, as structures are built) and the target stands at the
    -- same distance on the bearing (place_bearing), so the world direction
    -- of fire changes with the bearing. (Turning the attacker instead,
    -- target still on +x, keeps the world direction, and xsb2303's
    -- held-no-fire with its target dead behind does not show.) Positive bearings are the side positive HeadingArcCenter
    -- values point to (the unit's right: the Fatboy's right turrets have
    -- +75 / +120); the engine's arc test adds the centre to the body
    -- heading the same way. Sub-case rows are keyed 'id@bearing' and follow
    -- their canonical row (bearing 0, the plain id) in clockwise order.
    -- Only plain engagements whose aim depends on the bearing get them: a
    -- Turreted weapon on a ground / naval unit or structure, not air units
    -- (they circle to attack), silo launches, anti-projectile intercepts,
    -- overcharge, nor long range (MaxRadius > LONG: one session costs up to
    -- 4000 ticks, the 4000-range guns are clamped by the map). The weapon's
    -- arc: HeadingArcCenter / HeadingArcRange (half-range, unit frame), else
    -- TurretYaw / TurretYawRange (assumes the yaw bone rests facing forward),
    -- else all round. A structure only takes bearings inside its arc (it
    -- can't turn: outside is no-target by design); a mobile unit takes them
    -- all, those outside its arc are where it must turn to engage.
    -- WEAPON_AUDIT_BEARINGS picks the set the plan lists: 'gate' (default:
    -- 90, 180, -90, plus the bearing nearest a limited arc's centre),
    -- 'all' (every 45 degrees), 'none', or a list ('90,135'). A single case
    -- (WEAPON_AUDIT_CASE = 'id@b') runs any bearing b of an eligible
    -- weapon, listed or not.
    local COMPASS = { 45, 90, 135, 180, -135, -90, -45 }
    local GATE_BEARINGS = { 180 }
    local function wrap180(a)
        while a > 180 do a = a - 360 end
        while a <= -180 do a = a + 360 end
        return a
    end
    local function arc_of(wbp)
        local r = wbp.HeadingArcRange
        if r and r < 180 then return wbp.HeadingArcCenter or 0, r end
        r = wbp.TurretYawRange
        if r and r < 180 then return wbp.TurretYaw or 0, r end
        return nil
    end
    local function in_arc(wbp, b)
        local centre, range = arc_of(wbp)
        if not centre then return true end
        return math.abs(wrap180(b - centre)) <= range + 0.5
    end
    -- why a case takes no bearings (nil: it does)
    local function no_bearings(c)
        local wbp = __blueprints[c.attacker].Weapon[c.weapon]
        if c.unsupported then return 'never engages (' .. (c.reasonKey or c.unsupported) .. ')' end
        if not wbp.Turreted then return 'not turreted' end
        if c.air then return 'air unit' end
        if c.silo then return 'silo launch' end
        if c.icpt then return 'anti-projectile intercept' end
        if c.oc then return 'overcharge' end
        if c.maxR > LONG then return 'long range' end
        return nil
    end
    local function sweep(b) return (b + 360) % 360 end
    local function by_sweep(a, b) return sweep(a) < sweep(b) end
    local bearingSet = rawget(_G, 'WEAPON_AUDIT_BEARINGS') or 'gate'
    local listed = {}
    if bearingSet == 'all' then
        listed = COMPASS
    elseif bearingSet == 'gate' then
        listed = GATE_BEARINGS
    elseif bearingSet ~= 'none' then
        for n in string.gmatch(tostring(bearingSet), '-?%d+') do
            table.insert(listed, tonumber(n))
        end
    end
    -- the listed bearings case c runs at (sweep order)
    local function bearings_of(c)
        if no_bearings(c) then return {} end
        local bp = __blueprints[c.attacker]
        local wbp = bp.Weapon[c.weapon]
        local structure = bp.CategoriesHash and bp.CategoriesHash.STRUCTURE ~= nil
        local set, out = {}, {}
        local function add(b)
            b = wrap180(b)
            if b ~= 0 and not set[b] and (not structure or in_arc(wbp, b)) then
                set[b] = true
                table.insert(out, b)
            end
        end
        for _, b in listed do add(b) end
        local centre = arc_of(wbp)
        if centre and bearingSet == 'gate' then add(math.floor(centre / 45 + 0.5) * 45) end
        table.sort(out, by_sweep)
        return out
    end
    -- case c's sub-case at bearing b: a fresh copy of its plan
    local function bearing_case(c, b)
        local s = {}
        for k, v in c do s[k] = v end
        s.bearing = b
        s.key = c.id .. '@' .. b
        return s
    end

    local selected = {}
    local selText = tostring(rawget(_G, 'WEAPON_AUDIT_CASE') or '')
    if sel then
        -- one case: the canonical row, or one bearing of it
        local c = cases[math.floor(sel)]
        local selB = string.match(selText, '@%s*(-?%d+)%s*$')
        if c and selB then
            local b = wrap180(tonumber(selB))
            local why = no_bearings(c)
            if why then
                fail('case selection matched nothing (WEAPON_AUDIT_CASE = ' .. selText .. ': case ' .. c.id
                    .. ' takes no bearings: ' .. why .. '; case count is ' .. count .. ')')
                ScenarioResult.done = true
                return
            end
            table.insert(selected, b == 0 and c or bearing_case(c, b))
        elseif c then
            table.insert(selected, c)
        end
    else
        -- WEAPON_AUDIT_IDS: a bare id selects the case and its listed
        -- bearings, 'id@b' that one bearing (listed or not)
        local keyed = {}
        if type(rawget(_G, 'WEAPON_AUDIT_IDS')) == 'string' then
            for tok in string.gmatch(rawget(_G, 'WEAPON_AUDIT_IDS'), '[^,%s]+') do
                local n, b = string.match(tok, '^(%d+)@(-?%d+)$')
                if n then
                    n = tonumber(n)
                    keyed[n] = keyed[n] or {}
                    table.insert(keyed[n], wrap180(tonumber(b)))
                end
            end
        end
        for _, c in cases do
            local whole = not wanted or wanted[c.id]
            local subs = whole and bearings_of(c) or {}
            local canonical = whole
            if keyed[c.id] then
                for _, b in keyed[c.id] do
                    if b == 0 then
                        canonical = true
                    elseif not no_bearings(c) then
                        table.insert(subs, b)
                    end
                end
            end
            table.sort(subs, by_sweep)
            if canonical then table.insert(selected, c) end
            local seen = {}
            for _, b in subs do
                if not seen[b] then
                    seen[b] = true
                    table.insert(selected, bearing_case(c, b))
                end
            end
        end
    end
    for _, c in selected do
        c.aimed = no_bearings(c) == nil
    end
    if #selected == 0 then
        fail('case selection matched nothing (WEAPON_AUDIT_CASE = ' .. selText .. '; case count is ' .. count .. ')')
        ScenarioResult.done = true
        return
    end

    -- The plan pass (WEAPON_AUDIT_PLAN, the Rust census's first session):
    -- only the statically known cases (non-attack, unsupported) resolve here (no units, rows
    -- through the normal path below); every other selected id is listed in
    -- _G.AUDIT_PENDING ({ id =, budget = }, case order) for a session of its
    -- own (WEAPON_AUDIT_CASE).
    if rawget(_G, 'WEAPON_AUDIT_PLAN') then
        local pending, static = {}, {}
        for _, c in selected do
            if c.unsupported then
                table.insert(static, c)
            else
                table.insert(pending, { id = c.id, key = c.key, budget = c.budget })
            end
        end
        rawset(_G, 'AUDIT_PENDING', pending)
        selected = static
    end

    local maxBudget = 0
    for _, c in selected do
        if not c.unsupported then maxBudget = math.max(maxBudget, c.budget) end
    end
    note('cases: ' .. #selected .. ' of ' .. count .. ' (unselected ' .. count - #selected .. '); ceiling hint: --ticks >= ' .. (maxBudget + 45) .. ' (max case budget ' .. maxBudget .. ' + spawn settle 20 + issue and margin 25)')

    -- The single selected case (WEAPON_AUDIT_CASE): a centred standalone
    -- layout, its engagement midpoint at the map's exact centre, so the
    -- coordinates to watch are the same fixed point every run. The strip
    -- probe still slides it if the terrain there disagrees.
    local single = #selected == 1 and not selected[1].unsupported and selected[1] or nil

    -- 2. Layout. Fixed-height rows (the tallest case's) shared by every
    -- media pair (start medium > far-end medium, centred for a case
    -- subset); each pair scans them with its own cursor and a case takes the
    -- pair's next free x-strip whose ends fit its media: the attacker's own
    -- at the start, its predicted target's at the far end (any = no water
    -- constraint; dry strips must stay flat and dry). Only placed strips
    -- are taken, so a pair's misses never use up ground another pair could
    -- use, and a dry case never skips over water a wet case could use.
    local baseX = margin
    local baseZ = margin
    if wanted and not single then
        baseX = mapSize / 2 - 400
        baseZ = mapSize / 2 - 400
    elseif single then
        baseX = mapSize / 2 - single.attFoot / 2 - single.dist / 2
        baseZ = mapSize / 2 - math.max(single.attFoot + lane, 10) / 2
    end
    local function pair_of(c)
        return c.attMedium .. '>' .. (c.predEnv or c.attMedium)
    end
    local function height_of(c)
        return math.max(c.attFoot + lane, 10)
    end
    local rowHeight = 10
    for _, c in selected do
        if not c.unsupported then rowHeight = math.max(rowHeight, height_of(c)) end
    end
    -- rows 40 units apart: room to watch each engagement on its own
    rowHeight = rowHeight + 40
    -- row index -> the x spans placed in it
    local taken = {}
    local cursors = {}
    -- the end of the furthest placed span overlapping [x0, x1), if any
    local function blocked(row, x0, x1)
        local out
        for _, span in taken[row] or {} do
            if x0 < span[2] and x1 > span[1] then out = math.max(out or span[2], span[2]) end
        end
        return out
    end
    local function place(key, w, h, strip)
        local cursor = cursors[key] or { row = 0, x = baseX }
        cursors[key] = cursor
        local row, x = cursor.row, cursor.x
        for _ = 1, 25000 do
            local z0 = baseZ + row * rowHeight
            if z0 + h > mapSize - margin then return nil end
            if x + w > mapSize - margin then
                row, x = row + 1, baseX
            else
                local skip = blocked(row, x, x + w)
                if skip then
                    x = skip
                elseif strip(x, z0, h) then
                    taken[row] = taken[row] or {}
                    table.insert(taken[row], { x, x + w })
                    cursor.row, cursor.x = row, x + w
                    return x, z0
                else
                    -- a miss slides on in short steps: a shore crossing must
                    -- land in the strip's last quarter, which strip-width
                    -- steps can skip in every row alike
                    x = x + math.min(w, 4)
                end
            end
        end
        return nil
    end
    -- the medium holds across a unit's whole footprint (radius r about
    -- x, z), and dry ground is level there: a big unit centred on dry
    -- ground can still hang over a shore slope
    local function fits(need, x, z, r)
        local at = need == 'wet' and wet_at or dry_at
        local level = ground(x, z)
        for _, d in { { 0, 0 }, { -r, 0 }, { r, 0 }, { 0, -r }, { 0, r } } do
            local px, pz = x + d[1], z + d[2]
            if not at(px, pz) then return false end
            if need ~= 'wet' and math.abs(ground(px, pz) - level) > 0.25 then return false end
        end
        return true
    end
    local function probe_strip(startMed, endMed, ax, dist, z, startRadius)
        local tx = ax + dist
        for i = 0, 4 do
            local x = ax + (tx - ax) * i / 4
            -- midpoints keep a same-medium strip in its medium (no water
            -- under a land duel, no shore under a naval one); a strip that
            -- crosses a shore only needs its ends right
            local need = i == 0 and startMed or i == 4 and endMed or (endMed == startMed and startMed or nil)
            if need == 'any' then need = nil end
            if need then
                -- ends hold a unit (the attacker; a target up to a T1
                -- building's size), the midpoints only need the medium
                local r = i == 0 and startRadius or i == 4 and 5 or 0
                if not fits(need, x, z, r) then return false end
            end
        end
        return true
    end
    -- The centred case takes the strip nearest the centre that fits its
    -- media: row by row (rows run +z from the centre), in each row the x
    -- offsets 0, +4, -4, +8, ... (the census map, audit_duel, is all land
    -- in the centre row and has its water band one row north).
    local function place_single(w, h, strip)
        for row = 0, 1000 do
            local z0 = baseZ + row * rowHeight
            if z0 + h > mapSize - margin then return nil end
            for k = 0, mapSize do
                local step = math.floor((k + 1) / 2) * 4
                local x = baseX + (k % 2 == 1 and step or -step)
                if x >= margin and x + w <= mapSize - margin and strip(x, z0, h) then
                    return x, z0
                end
            end
        end
        return nil
    end
    -- A bearing sub-case's line: the attacker faces +z (heading 0) and its
    -- target stands dist away at the bearing, direction (sin b, cos b)
    -- (heading convention: +90 is +x). The line's midpoint is the
    -- point nearest the map centre (rings 4 units apart, outwards) where the
    -- line fits the case's media, each end inside the margins.
    local function probe_line(startMed, endMed, ax, az, ux, uz, dist, startRadius)
        for i = 0, 4 do
            local x, z = ax + ux * dist * i / 4, az + uz * dist * i / 4
            local need = i == 0 and startMed or i == 4 and endMed or (endMed == startMed and startMed or nil)
            if need == 'any' then need = nil end
            if need then
                local r = i == 0 and startRadius or i == 4 and 5 or 0
                if not fits(need, x, z, r) then return false end
            end
        end
        -- a shore crossing keeps the start medium to within 12 of the far
        -- end along the map's axes, as the canonical strip's first fit does: the land is 2 above
        -- the water, so a flat shot from a ship's low muzzle at a target
        -- far inland hits the shore (decks guns fell 16-18 short at 180)
        if startMed ~= endMed and endMed ~= 'any' then
            -- (12 across the map's axis-aligned shores: a diagonal line
            -- takes 12 / cos to cover it)
            local k = math.max(dist - 12 / math.max(math.abs(ux), math.abs(uz)), dist / 2)
            local at = startMed == 'wet' and wet_at or dry_at
            if not at(ax + ux * k, az + uz * k) then return false end
        end
        return true
    end
    local function place_bearing(c)
        local t = math.rad(c.bearing)
        local ux, uz = math.sin(t), math.cos(t)
        local half = c.dist / 2
        local lo, hi = margin + c.attFoot / 2, mapSize - margin - c.attFoot / 2
        local function try(mx, mz)
            local ax, az = mx - ux * half, mz - uz * half
            local tx, tz = mx + ux * half, mz + uz * half
            if ax < lo or ax > hi or az < lo or az > hi or tx < margin or tx > mapSize - margin
                or tz < margin or tz > mapSize - margin then
                return false
            end
            -- a ship turns to bring its guns round: its spot keeps 8 more
            -- of water round it, clear of the shore ramps (on a ramp's
            -- edge frigates fired 15-17 short at the diagonals)
            local room = c.attFoot / 2 + (c.attMedium == 'wet' and 8 or 0)
            if not probe_line(c.attMedium, c.predEnv or c.attMedium, ax, az, ux, uz, c.dist, room) then
                return false
            end
            c.attX, c.x, c.z = ax, ax, az
            c.tX, c.tZ, c.ux, c.uz = tx, tz, ux, uz
            return true
        end
        local cx, cz = mapSize / 2, mapSize / 2
        for r = 0, mapSize / 2, 4 do
            local ring = {}
            for k = -r, r, 4 do
                table.insert(ring, { k, -r })
                if r > 0 then
                    table.insert(ring, { k, r })
                    if k ~= -r and k ~= r then
                        table.insert(ring, { -r, k })
                        table.insert(ring, { r, k })
                    end
                end
            end
            table.sort(ring, function(a, b)
                local da, db = a[1] * a[1] + a[2] * a[2], b[1] * b[1] + b[2] * b[2]
                if da ~= db then return da < db end
                if a[2] ~= b[2] then return a[2] < b[2] end
                return a[1] < b[1]
            end)
            for _, o in ring do
                if try(cx + o[1], cz + o[2]) then return true end
            end
        end
        return false
    end
    for _, c in selected do
        if not c.unsupported and c.bearing ~= 0 then
            -- bearing sub-cases stand alone (the census's sessions); the
            -- multi-case layouts' strips run along +x only
            if not single then
                c.mapNo = 'bearing sub-case: runs in the single-case layout only (WEAPON_AUDIT_CASE, the census)'
                c.reasonKey = 'no-cell'
            elseif not place_bearing(c) then
                c.mapNo = 'no fitting line at bearing ' .. c.bearing .. ' (start medium ' .. c.attMedium
                    .. ', far end ' .. (c.predEnv or c.attMedium) .. ')'
                c.reasonKey = 'no-cell'
            end
        elseif not c.unsupported then
            -- strips 60 units apart along the row: a neighbour sits clear of
            -- the far end (an attacker beside a target plane drew the anti-air)
            local w = c.dist + c.attFoot / 2 + 12 + (single and 0 or 60)
            local h = height_of(c)
            local strip = function(x0, z0, hh)
                return probe_strip(c.attMedium, c.predEnv or c.attMedium, x0 + c.attFoot / 2, c.dist, z0 + hh / 2, c.attFoot / 2)
            end
            local x0, z0
            if single then
                x0, z0 = place_single(w, h, strip)
            else
                x0, z0 = place(pair_of(c), w, h, strip)
            end
            if not x0 then
                c.mapNo = 'no fitting cell (start medium ' .. c.attMedium .. ', far end ' .. (c.predEnv or c.attMedium) .. ')'
                c.reasonKey = 'no-cell'
            else
                c.attX = x0 + c.attFoot / 2
                c.x = c.attX
                c.z = z0 + h / 2
            end
        end
    end

    -- Per-weapon credit. `cur` is the case whose weapon's damage code is
    -- running right now: set around each of its projectiles' OnImpact
    -- (fragments and other child projectiles included) and its beams'
    -- OnImpact. Damage() / DamageArea() call OnDamage synchronously, so a
    -- target credits damage only when `cur` is its own case — not the
    -- unit's other weapons (FAF passes the launcher unit as instigator for
    -- every weapon, and a fragment's instigator is its parent projectile).
    -- Only wrappers that call straight through: no behaviour changes.
    local cur
    local tracked = setmetatable({}, { __mode = 'k' })
    local function within(c, f, ...)
        local prev = cur
        cur = c
        local r = table.pack(pcall(f, ...))
        cur = prev
        if not r[1] then error(r[2], 0) end
        return table.unpack(r, 2, r.n)
    end
    -- Threads forked while a case's damage code runs (FAF's nuke rings,
    -- NukeAOE.SlowNuke; damage-over-time pulses) belong to that case: the
    -- damage they deal later, after OnImpact returned, credits it too. The
    -- global ForkThread is wrapped to tag the new thread (a call straight
    -- through; scripts that cached ForkThread as a local are not covered).
    local threadCase = setmetatable({}, { __mode = 'k' })
    local function case_now()
        return cur or threadCase[coroutine.running()]
    end
    local fork = ForkThread
    rawset(_G, 'ForkThread', function(fn, ...)
        local c = case_now()
        if c and type(fn) == 'function' then
            return fork(function(...)
                threadCase[coroutine.running()] = c
                return fn(...)
            end, ...)
        end
        return fork(fn, ...)
    end)
    -- Marks proj as case c's: its OnImpact runs as c, and the children it
    -- makes through its own methods (self:CreateChildProjectile /
    -- self:CreateProjectile, also from its threads) are c's too.
    local track
    track = function(proj, c)
        if type(proj) ~= 'table' or tracked[proj] then return proj end
        tracked[proj] = c
        local oi = proj.OnImpact
        if oi then
            proj.OnImpact = function(p, ...)
                -- evidence for a miss: the nearest impact to the target
                -- (ground distance), measured before the impact runs
                if not c.done and c.tunit and not c.tunit:BeenDestroyed() then
                    local a, b = p:GetPosition(), c.tunit:GetPosition()
                    local d = VDist2(a[1], a[3], b[1], b[3])
                    if not c.nearest or d < c.nearest then c.nearest = d end
                    -- aim error (aim_of): the impact off the target along
                    -- the attacker -> target line (long, + beyond) and
                    -- across it (lat, + towards positive bearings)
                    local lx, lz = b[1] - c.attX, b[3] - c.z
                    local ll = math.sqrt(lx * lx + lz * lz)
                    if ll > 0.001 then
                        lx, lz = lx / ll, lz / ll
                        local dx, dz = a[1] - b[1], a[3] - b[3]
                        c.aimN = (c.aimN or 0) + 1
                        c.aimLong = (c.aimLong or 0) + dx * lx + dz * lz
                        c.aimLat = (c.aimLat or 0) + dx * lz - dz * lx
                        c.aimMax = math.max(c.aimMax or 0, d)
                    end
                end
                return within(c, oi, p, ...)
            end
        end
        for _, k in { 'CreateChildProjectile', 'CreateProjectile' } do
            local f = proj[k]
            if f then
                proj[k] = function(p, ...) return track(f(p, ...), c) end
            end
        end
        return proj
    end
    -- Scripts that keep moho's own CreateChildProjectile as an upvalue
    -- (SIFThunthoArtilleryShell01's fragments) bypass the instance
    -- wrapper: route the moho method through the context instead. Those
    -- scripts load on their first projectile, after this runs.
    local pm = moho.projectile_methods
    local mohoChild = pm.CreateChildProjectile
    pm.CreateChildProjectile = function(self, bp)
        local child = mohoChild(self, bp)
        if cur then track(child, cur) end
        return child
    end

    -- Energy for weapons that charge each shot from it (EnergyRequired):
    -- FAF's weapon waits on that drain before it fires again. Also
    -- OverCharge's EnergyRequired stock. Real APIs only, one GiveStorage
    -- (the real one keeps a single script storage pair: a later MASS call
    -- would zero this); GiveResource may land on the next economy
    -- tick, well before the orders 20+ ticks later. No mass: no audited
    -- weapon spends it (silo ammo is given, enhancements are created).
    GetArmyBrain('ARMY_1'):GiveStorage('ENERGY', 10000000)
    GetArmyBrain('ARMY_1'):GiveResource('ENERGY', 10000000)
    -- 3. Attackers spawn (case id order) holding fire — an early fires-on
    -- lets auto-acquisition shoot into neighbouring cases before their
    -- orders land — and the weapon's muzzle wrap counts its shots.
    -- Holds the case's unit's fire and wraps its weapon (index wi): the
    -- muzzle wrap counts its shots, its beams credit as the case.
    local function arm(c, unit, wi)
        c.unit = unit
        unit:SetFireState(1)
        local w = unit:GetWeapon(wi)
        c.w = w
        if w then
            -- shots: CreateProjectileAtMuzzle calls (beam weapons
            -- start their beam there), plus CreateProjectileForWeapon
            -- calls made outside it (FAF's CreateProjectileAtMuzzle
            -- chains it, so it only counts on its own). Both mark
            -- the projectile as this case's.
            local capm, cpfw = w.CreateProjectileAtMuzzle, w.CreateProjectileForWeapon
            local inMuzzle = 0
            w.CreateProjectileAtMuzzle = function(self, muzzle)
                -- count only while the case is open: a resolved
                -- case's evidence must not keep growing for the
                -- session's remaining ticks (rows must be the
                -- resolve-time numbers the AUDIT line shows).
                if c.done then return capm(self, muzzle) end
                c.fired = c.fired + 1
                if c.fired == 1 and c.aimed and c.t0 then c.shotTicks = GetGameTick() - c.t0 end
                inMuzzle = inMuzzle + 1
                local proj = capm(self, muzzle)
                inMuzzle = inMuzzle - 1
                return track(proj, c)
            end
            if cpfw then
                w.CreateProjectileForWeapon = function(self, bone)
                    local proj = cpfw(self, bone)
                    if not c.done then
                        if inMuzzle == 0 then c.fired = c.fired + 1 end
                        track(proj, c)
                    end
                    return proj
                end
            end
            for _, info in w.Beams or {} do
                local beam = info.Beam
                if beam and beam.OnImpact then
                    local bi = beam.OnImpact
                    beam.OnImpact = function(b, ...) return within(c, bi, b, ...) end
                end
            end
        end
    end
    -- Builds the enhancement that enables the case's weapon, its
    -- Prerequisite chain root first, skipping what the unit already has
    -- (presets). The unit script's CreateEnhancement enables the weapon and
    -- adds the command caps the enhancement brings.
    local function enhance(c, unit)
        local enhs = unit:GetBlueprint().Enhancements or {}
        local chain, e = {}, c.enh
        while e and #chain < 8 do
            table.insert(chain, 1, e)
            e = enhs[e] and enhs[e].Prerequisite
        end
        for _, name in chain do
            if not unit:HasEnhancement(name) then
                local ok, err = pcall(unit.CreateEnhancement, unit, name)
                if not ok then
                    c.enhErr = name .. ': ' .. tostring(err)
                    return
                end
            end
        end
        c.enhanced = table.concat(chain, '>')
    end
    -- Intercept cases: the source (ARMY_2, immortal, fire state left normal:
    -- its silo weapon only fires on the launch order) gets one missile, and
    -- every missile it launches is watched. A missile is STOPPED (the case's
    -- damage column counts them) when the case's weapon damages it (its
    -- OnDamage runs while the case's projectile / beam impact does), when
    -- the source's OnMissileIntercepted names the attacker as the defense
    -- after this weapon fired, or when it was redirected (IsRedirected:
    -- flares, the Loyalist's deflector) and never damaged the attacker.
    -- The attacker is immortal too; damage a missile's impact deals it
    -- counts as 'attacker hit'.
    local curMissile
    local function source_arm(c, src)
        src:SetMaxHealth(1e7)
        src:SetHealth(nil, 1e7)
        if c.srcOrder ~= 'attack' then
            local give = c.srcOrder == 'nuke' and src.GiveNukeSiloAmmo or src.GiveTacticalSiloAmmo
            local okG, errG = pcall(give, src, 1)
            if not okG then c.issueErr = 'source ammo: ' .. tostring(errG) end
        end
        local omi = src.OnMissileIntercepted
        src.OnMissileIntercepted = function(self, target, defense, pos, proj, ...)
            if not c.done and defense == c.unit and c.fired > 0 and type(proj) == 'table' then
                proj.auditIntercepted = true
            end
            if omi then return omi(self, target, defense, pos, proj, ...) end
        end
        local sw = src:GetWeapon(c.srcWeapon or 1)
        if not sw then
            c.issueErr = 'source has no weapon'
            return
        end
        local capm = sw.CreateProjectileAtMuzzle
        sw.CreateProjectileAtMuzzle = function(self, muzzle)
            local m = capm(self, muzzle)
            if type(m) == 'table' and not c.done then
                table.insert(c.missiles, m)
                local od = m.OnDamage
                if od then
                    m.OnDamage = function(p, instigator, amount, ...)
                        if case_now() == c then p.auditHit = true end
                        return od(p, instigator, amount, ...)
                    end
                end
                -- deflection: FAF's DepthCharge entity (Smart Charges,
                -- Cybran anti-torpedo flares) and the Aeon Flare retarget
                -- the projectile onto the defence's own projectile
                local snt = m.SetNewTarget
                if snt then
                    m.SetNewTarget = function(p, t, ...)
                        if tracked[t] == c then p.auditDeflected = true end
                        return snt(p, t, ...)
                    end
                end
                local oi = m.OnImpact
                if oi then
                    m.OnImpact = function(p, ...)
                        local prev = curMissile
                        curMissile = p
                        local r = table.pack(pcall(oi, p, ...))
                        curMissile = prev
                        if not r[1] then error(r[2], 0) end
                        return table.unpack(r, 2, r.n)
                    end
                end
            end
            return m
        end
        local unit = c.unit
        unit:SetMaxHealth(1e7)
        unit:SetHealth(nil, 1e7)
        -- the unit's other anti-projectile weapons are switched off: they
        -- would race the case's weapon for the one missile (a ship's
        -- left and right guns), and a missile they stop is not this
        -- weapon's (the intercept callback names the unit, not the weapon)
        local off = {}
        for wi, wbp in unit:GetBlueprint().Weapon or {} do
            if wi ~= c.weapon and type(wbp) == 'table' and wbp.TargetType == 'RULEWTT_Projectile'
                and wbp.Label and wbp.Label ~= c.label then
                unit:SetWeaponEnabledByLabel(wbp.Label, false)
                table.insert(off, wbp.Label)
            end
        end
        if off[1] then c.othersOff = table.concat(off, '/') end
        local uod = unit.OnDamage
        unit.OnDamage = function(self, instigator, amount, direction, kind)
            if curMissile and not c.done and kind ~= 'TreeForce' and kind ~= 'TreeFire' then
                curMissile.auditHitAttacker = true
                c.attHit = c.attHit + amount
            end
            return uod(self, instigator, amount, direction, kind)
        end
    end
    -- An intercept case's missiles that have ended: each counts once,
    -- stopped or not.
    local function judge_missiles(c)
        for _, m in c.missiles or {} do
            if not m.auditJudged then
                local gone = m:BeenDestroyed()
                if not gone and m.IsRedirected then m.auditRedirected = true end
                if gone then
                    m.auditJudged = true
                    local redirected = m.auditRedirected or m.IsRedirected or m.auditDeflected
                    if m.auditHit then c.impacted = c.impacted + 1 end
                    -- which rule stopped it (the first that applies)
                    local by = (m.auditHit and 'hit') or (m.auditIntercepted and 'callback')
                        or (redirected and not m.auditHitAttacker and 'redirect') or nil
                    if by then
                        c.stopped = c.stopped + 1
                        c.dmg = c.stopped
                        c.stopBy = c.stopBy or {}
                        c.stopBy[by] = (c.stopBy[by] or 0) + 1
                    end
                end
            end
        end
    end
    local spawned, attempted = 0, 0
    for _, c in selected do
        if not c.unsupported and not c.mapNo then
            attempted = attempted + 1
            -- facing down the row: fixed-arc turrets must be able to aim at
            -- their target (heading convention atan2(x, z); target on +x).
            -- A bearing sub-case faces +z (heading 0, as a structure is
            -- built) with its target at the bearing (place_bearing).
            local yaw = math.atan2(c.dist, 0)
            if c.bearing ~= 0 then yaw = 0 end
            -- a unit floating on the water spawns at the surface (a
            -- structure keeps the height it is given)
            local y = c.spawnLayer == 'Water' and GetSurfaceHeight(c.attX, c.z) or ground(c.attX, c.z)
            local ok, unit = pcall(CreateUnitHPR, c.attacker, 'ARMY_1', c.attX, y, c.z, 0, yaw, 0)
            if ok and unit and c.bearing ~= 0 and unit:GetHeading() ~= 0 then
                -- FAF turns a new weapon structure to face the nearest
                -- enemy in reach, else the map centre
                -- (StructureUnit.RotateTowardsEnemy, in OnCreate; the real
                -- game does the same). The bearing line runs through the
                -- map centre towards the target, so the structure would
                -- face its target at every bearing: turn it back to
                -- heading 0 about world up as that function turns it
                -- (quatRotate * quatOrient), keeping any terrain tilt.
                local h = unit:GetHeading()
                local cs, sn = math.cos(-h / 2), math.sin(-h / 2)
                local q1, q2, q3, q4 = unpack(unit:GetOrientation())
                unit:SetOrientation({ cs * q1 - sn * q3, cs * q2 + sn * q4, cs * q3 + sn * q1, cs * q4 - sn * q2 }, true)
            end
            if ok and unit then
                spawned = spawned + 1
                if c.enh then enhance(c, unit) end
                -- a counted anti-missile (the SMDs) gets one interceptor:
                -- FAF's SMDs build tactical silo ammo (probe_antinuke)
                if c.icpt and __blueprints[c.attacker].Weapon[c.weapon].CountedProjectile then
                    local okG, errG = pcall(unit.GiveTacticalSiloAmmo, unit, 1)
                    if not okG then c.issueErr = 'interceptor ammo: ' .. tostring(errG) end
                end
                -- a silo launcher gets one missile
                if c.silo then
                    local give = c.silo == 'nuke' and unit.GiveNukeSiloAmmo or unit.GiveTacticalSiloAmmo
                    local okG, errG = pcall(give, unit, 1)
                    if not okG then c.issueErr = 'ammo: ' .. tostring(errG) end
                end
                arm(c, unit, c.weapon)
            else
                c.spawnErr = tostring(unit)
            end
        end
    end
    -- the spawn settle: 20 game ticks (a WaitTicks(20) is 19)
    local settle0 = GetGameTick()
    while GetGameTick() - settle0 < 20 do WaitTicks(2) end

    local VizMarker = import('/lua/sim/VizMarker.lua').VizMarker
    local attackerArmy = GetArmyBrain('ARMY_1'):GetArmyIndex()

    -- 3b. Units that replace themselves once built (the HARMS xrb2308 ->
    -- xrb2309 in its OnStopBeingBuilt): an attacker destroyed during the
    -- settle hands its case to the ARMY_1 unit now standing on its spot
    -- with a weapon of the same Label (not another case's unit), re-armed
    -- and re-planned for its own blueprint. Then every attacker must stand
    -- on its planned spawn layer, and a SurfacingSub that fires from the
    -- other depth gets its IssueDive (its attack waits for the layer).
    local owned = {}
    for _, c in selected do
        if c.unit then owned[c.unit] = true end
    end
    for _, c in selected do
        if c.unit and c.unit:BeenDestroyed() then
            local near = GetUnitsInRect(Rect(c.attX - 1, c.z - 1, c.attX + 1, c.z + 1)) or {}
            local pick
            for _, u in near do
                if not pick and not owned[u] and not u:BeenDestroyed() and u:GetArmy() == attackerArmy then
                    local ubp = u:GetBlueprint()
                    for wi, wbp in ubp.Weapon or {} do
                        if wbp.Label == c.label then
                            pick = { u = u, wi = wi, bp = ubp, wbp = wbp }
                            break
                        end
                    end
                end
            end
            if pick then
                owned[pick.u] = true
                c.adopted = string.lower(pick.bp.BlueprintId)
                arm(c, pick.u, pick.wi)
                local al = scenario.attacker_layer(pick.bp, pick.wbp)
                c.attLayer, c.spawnLayer, c.dive = al.layer, al.spawn, al.dive
                c.wbp = pick.wbp
            end
        end
    end
    for _, c in selected do
        if c.unit and not c.unit:BeenDestroyed() and c.spawnLayer then
            local got = c.unit:GetCurrentLayer()
            if got ~= c.spawnLayer then
                c.unsupported = 'layer ' .. c.spawnLayer .. ' not reached (got ' .. tostring(got) .. ')'
                c.reasonKey = 'layer'
            elseif c.dive then
                IssueDive({ c.unit })
                c.diveTick = GetGameTick()
                -- its live caps are the other depth's until it gets there:
                -- the planned layer's blueprint row picks the target
                c.planCaps = bp_caps(c.wbp or __blueprints[c.attacker].Weapon[c.weapon], c.attLayer)
            end
        end
    end

    -- 4. Targets: live caps pick the kind (an override target only when it
    -- does not fight the caps), then targets spawn in case order — hold fire,
    -- immortal, OnDamage wrapped.
    for _, c in selected do
        if not c.unsupported and not c.mapNo then
            if not c.unit or c.unit:BeenDestroyed() then
                c.attDied = true
                c.unsupported = 'attacker did not survive spawn'
                c.reasonKey = 'spawn'
            elseif c.icpt then
                -- the source stands at the far end (its kind is fixed)
                c.caps = (c.w and c.w:__engine_GetFireTargetLayerCaps()) or ''
                c.target = c.source
                c.tgtMedium = c.predEnv
            else
                local caps = c.planCaps or (c.w and c.w:__engine_GetFireTargetLayerCaps()) or ''
                c.caps = caps
                local set = {}
                for s in string.gmatch(caps, '[^|]+') do set[string.upper(s)] = true end
                local kind, env = scenario.pick_kind(c.kinds, caps)
                if c.oTarget then
                    -- an explicit override target must not fight the caps: keep
                    -- the caps-chosen kind unless the caps are empty or can reach
                    -- the override's kind.
                    local wants = fits_caps(c.oTarget, caps)
                    if not kind or wants then kind = 'override' end
                end
                if kind then
                    local tid
                    if kind == 'override' then
                        -- 'building'/'naval'/... resolves through the catalog
                        -- for the attacker's faction; anything else is a bp id.
                        tid = c.oTarget
                        if catalog[tid] then
                            c.tgtMedium = scenario.kind_env[tid]
                            tid = scenario.target_of(catalog, tid, c.attacker)
                        else
                            c.tgtMedium = medium(__blueprints[tid] or {})
                        end
                    else
                        tid = scenario.target_of(catalog, kind, c.attacker)
                        c.tgtMedium = env
                    end
                    if tid and __blueprints[tid] then
                        c.target = tid
                    else
                        c.unsupported = 'no ' .. kind .. ' target in the catalog for ' .. c.attacker
                    end
                elseif next(set) ~= nil then
                    c.unsupported = 'no allowed target on the live caps (' .. caps .. ')'
                    c.reasonKey = 'restrict'
                else
                    c.unsupported = 'no reachable layer (live caps empty)'
                    c.reasonKey = 'no-caps'
                end
            end
        end
    end
    for _, c in selected do
        if not c.unsupported and not c.mapNo and c.target then
            local tbp = __blueprints[c.target]
            local wantMed = c.tgtMedium or 'any'
            -- the far end: down the row, or a bearing sub-case's spot
            local tx, tz = c.attX + c.dist, c.z
            if c.tX then tx, tz = c.tX, c.tZ end
            if wantMed ~= 'any' then
                local ok = wantMed == 'wet' and wet_at(tx, tz) or dry_at(tx, tz)
                if not ok then
                    -- the live kind disagreed with the predicted media: slide
                    -- inside the cell's slack before giving up (along the
                    -- line, staying 2 beyond the attacker).
                    for d = 1, 5 do
                        for _, dx in { 6 * d, -6 * d } do
                            local px, pz = tx + dx, tz
                            local ahead = px > c.attX + 2
                            if c.tX then
                                px, pz = tx + c.ux * dx, tz + c.uz * dx
                                ahead = c.dist + dx > 2
                            end
                            if ahead and (wantMed == 'wet' and wet_at(px, pz) or dry_at(px, pz)) then
                                tx, tz = px, pz
                                ok = true
                                break
                            end
                        end
                        if ok then break end
                    end
                end
                if not ok then
                    c.mapNo = 'no ' .. wantMed .. ' at the reserved far end for ' .. c.target
                    c.reasonKey = 'no-cell'
                end
            end
            if not c.mapNo then
                local y = wantMed == 'any' and ground(tx, tz) + 18 or ground(tx, tz)
                -- a floating source structure (the torpedo launcher) at
                -- the surface, as a floating attacker spawns
                if c.icpt and wantMed == 'wet' then y = GetSurfaceHeight(tx, tz) end
                local okT, tu = pcall(CreateUnitHPR, c.target, 'ARMY_2', tx, y, tz, 0, 0, 0)
                if okT and tu and c.icpt then
                    c.tunit = tu
                    source_arm(c, tu)
                elseif okT and tu then
                    -- An idle plane slows onto its point and lands (as in
                    -- the real game): keep winged targets flying a short
                    -- beat round where they spawned (two points: a
                    -- script's single patrol point is a one-way trip).
                    local tbp = __blueprints[c.target]
                    if wantMed == 'any' and tbp.Air and tbp.Air.Winged then
                        IssuePatrol({ tu }, { tx + 12, y, tz })
                        IssuePatrol({ tu }, { tx, y, tz })
                    end
                    tu:SetFireState(1)
                    tu:SetMaxHealth(1e7)
                    tu:SetHealth(nil, 1e7)
                    local od = tu.OnDamage
                    tu.OnDamage = function(self, instigator, amount, direction, kind)
                        -- tree damage (the 1-point TreeForce every
                        -- projectile impact deals round it) is not damage:
                        -- Unit.OnDamage ignores it
                        if not c.done and kind ~= 'TreeForce' and kind ~= 'TreeFire' then
                            if case_now() == c then
                                c.impacted = c.impacted + 1
                                c.dmg = c.dmg + amount
                            elseif instigator == c.unit then
                                -- the attacker's other weapons (or damage
                                -- outside its projectiles' / beams' impact):
                                -- evidence only, never credit
                                c.unitDmg = c.unitDmg + amount
                            end
                        end
                        return od(self, instigator, amount, direction, kind)
                    end
                    c.tunit = tu
                    -- the attacker's army sees its own target: weapons
                    -- only acquire what their army detects, and many
                    -- outrange their own sight (artillery, missiles), so
                    -- without it a case passed only when a neighbouring
                    -- case's unit happened to spot for it
                    VizMarker({ X = tx, Z = tz, Radius = 15, LifeTime = -1, Army = attackerArmy,
                        Omni = false, Radar = false, Vision = true, WaterVision = true })
                else
                    c.spawnErr = 'target: ' .. tostring(tu)
                end
            end
        end
    end

    -- The aim evidence of a case whose weapon takes bearings (and of any
    -- case with tracked impacts): its bearing, the body's turn from the
    -- order to the verdict (degrees), whether the target then sat inside the
    -- weapon's arc ('-': unrestricted), the ticks to the first shot (and a
    -- structure's turret slew time), and over the case's own projectile
    -- impacts while open: count, mean lateral / longitudinal miss off the
    -- target (see track) and the largest miss. key=value, one token each
    -- (dev/audit/bearing_report.py reads them back from the AUDIT lines).
    local function aim_of(c)
        local parts = {}
        if c.aimed then
            parts[#parts + 1] = 'b=' .. c.bearing
            if c.turn then parts[#parts + 1] = string.format('turn=%.0f', c.turn) end
            if c.turn and arc_of(__blueprints[c.attacker].Weapon[c.weapon]) then
                parts[#parts + 1] = 'arc=' .. (in_arc(__blueprints[c.attacker].Weapon[c.weapon], c.bearing - c.turn) and 'in' or 'out')
            end
            -- ticks from the order to the first shot, and for a structure
            -- (it can't turn its body) the fewest ticks its turret needs to
            -- swing from straight ahead to the bearing at TurretYawSpeed
            -- (degrees a second): a first shot before that fired without
            -- turning round
            if c.shotTicks then parts[#parts + 1] = 'shot=' .. c.shotTicks end
            local bp = __blueprints[c.attacker]
            local speed = bp.Weapon[c.weapon].TurretYawSpeed
            if bp.CategoriesHash and bp.CategoriesHash.STRUCTURE and speed and speed > 0 then
                parts[#parts + 1] = 'slew=' .. math.floor(math.abs(c.bearing) / speed * 10)
            end
        end
        if c.aimN then
            parts[#parts + 1] = string.format('n=%d lat=%.2f long=%.2f max=%.2f', c.aimN, c.aimLat / c.aimN, c.aimLong / c.aimN, c.aimMax)
        end
        if not parts[1] then return '' end
        return 'aim ' .. table.concat(parts, ' ')
    end
    -- Stage text: everything the trail knows, deterministic, one line.
    local function stage_of(c)
        local parts = {}
        if c.unsupported then parts[#parts + 1] = c.unsupported end
        if c.mapNo then parts[#parts + 1] = c.mapNo end
        if c.adopted then parts[#parts + 1] = 'adopted ' .. c.adopted end
        if c.enhanced then parts[#parts + 1] = 'enhanced ' .. c.enhanced end
        if c.enhErr then parts[#parts + 1] = 'enhancement error ' .. c.enhErr end
        if c.oc then parts[#parts + 1] = 'overcharge ' .. c.oc end
        if c.silo then parts[#parts + 1] = c.silo .. ' launch at ' .. math.floor(c.dist) end
        if c.clamped then parts[#parts + 1] = 'at ' .. math.floor(c.dist) .. ' (map clamp, window ' .. c.clamped .. ')' end
        if c.icpt then
            parts[#parts + 1] = 'intercept ' .. c.icpt .. ': ' .. (c.source or '-') .. ' ' .. (c.srcOrder or '-')
                .. ' launch from ' .. math.floor(c.dist) .. ', missiles ' .. #c.missiles
                .. ', stopped ' .. c.stopped .. ' (the dmg column'
                .. (c.stopBy and ((c.stopBy.hit and (': hit ' .. c.stopBy.hit) or '')
                    .. (c.stopBy.callback and (': callback ' .. c.stopBy.callback) or '')
                    .. (c.stopBy.redirect and (': redirect ' .. c.stopBy.redirect) or '')) or '')
                .. '), attacker hit ' .. c.attHit
                .. (c.othersOff and (', off ' .. c.othersOff) or '')
        end
        if c.capMissing then parts[#parts + 1] = 'no ' .. c.capMissing end
        if c.dived then parts[#parts + 1] = 'dived to ' .. c.attLayer .. ' in ' .. c.dived .. ' ticks' end
        if c.attDied then parts[#parts + 1] = 'attacker died' end
        if c.spawnErr then parts[#parts + 1] = 'spawn error ' .. c.spawnErr end
        if c.issueErr then parts[#parts + 1] = 'issue error ' .. c.issueErr end
        if c.tDead then parts[#parts + 1] = 'target died' end
        if c.w == nil then parts[#parts + 1] = 'no weapon object' end
        parts[#parts + 1] = 'caps ' .. (c.caps or '-')
        if c.label then parts[#parts + 1] = 'label ' .. c.label end
        parts[#parts + 1] = 'held ' .. (c.heldCase and 'case' or (c.held and 'other' or 'never'))
        parts[#parts + 1] = 'impacts ' .. c.impacted
        if c.dmg == 0 and c.nearest then parts[#parts + 1] = string.format('nearest impact %.1f', c.nearest) end
        if c.unitDmg > 0 then parts[#parts + 1] = 'uncredited unit dmg ' .. c.unitDmg end
        local aim = aim_of(c)
        if aim ~= '' then parts[#parts + 1] = aim end
        return table.concat(parts, ', ')
    end
    local function audit(c, outcome)
        c.outcome = outcome
        LOG('AUDIT ' .. c.key .. ' ' .. c.attacker .. '[' .. c.weapon .. '] vs ' .. (c.target or '-')
            .. ': ' .. outcome .. ' (fired ' .. c.fired .. ', dmg ' .. c.dmg .. ' ' .. stage_of(c) .. ')')
    end
    local function resolve(c, outcome)
        if c.done then return end
        c.done = true
        if c.h0 and c.unit and not c.unit:BeenDestroyed() then
            c.turn = wrap180(math.deg(c.unit:GetHeading() - c.h0))
        end
        audit(c, outcome)
    end

    -- Pre-resolve the never-engaged cases (unsupported / map-nosupport /
    -- spawn errors) so the AUDIT trail covers the whole selection.
    for _, c in selected do
        if not c.done then
            if c.unsupported or c.mapNo or c.spawnErr then
                if c.spawnErr then fail('case ' .. c.key .. ' spawn error: ' .. c.spawnErr) end
                if c.nonAttack then
                    resolve(c, 'non-attack')
                elseif c.unsupported then
                    resolve(c, 'unsupported')
                elseif c.mapNo then
                    resolve(c, 'map-nosupport')
                else
                    resolve(c, 'no-target')
                end
            end
        end
    end

    -- The case table is live before the engagement (a watching run reads
    -- the spot while the case plays); the tail re-rawsets it.
    local function evidence_of()
        local ev = {}
        local i = 0
        for _, c in selected do
            i = i + 1
            ev[i] = { id = c.id, key = c.key, bearing = c.bearing, aim = aim_of(c), attacker = c.attacker, weapon = c.weapon, target = c.target or '-',
                x = c.x or 0, z = c.z or 0, budget = c.budget or 0,
                attName = c.attName, weaponName = c.weaponName,
                tgtName = c.target and unit_name(__blueprints[c.target] or {}) or '-',
                reason = c.unsupported or c.mapNo or '', reasonKey = c.reasonKey or '' }
        end
        return ev
    end
    if wanted then rawset(_G, 'AUDIT_CASES', evidence_of()) end
    if single and not single.mapNo then
        note('standalone: case ' .. single.key .. ' centred at ' .. single.x .. ', ' .. single.z)
    end

    -- 5. Engagement trigger: one IssueAttack per case while every attacker
    -- still holds fire, then all attackers go fires-on the same tick (an
    -- early fires-on races auto-acquisition before the orders land).
    -- A diving case's order and fires-on wait for its layer (the loop
    -- below).
    -- Overcharge: the manual weapon fires through IssueOverCharge at the
    -- target; the AutoOverCharge weapon is switched on (SetAutoOvercharge)
    -- and the unit attacks.
    local function engage(c)
        local okIssue, err
        if c.aimed and c.unit and not c.unit:BeenDestroyed() then
            c.h0 = c.unit:GetHeading()
            c.t0 = GetGameTick()
        end

        if c.icpt then
            -- no order for the attacker (fire state normal below): the
            -- source launches at its position
            if c.srcOrder == 'attack' then
                okIssue, err = pcall(IssueAttack, { c.tunit }, c.unit)
            else
                local issue = c.srcOrder == 'nuke' and IssueNuke or IssueTactical
                okIssue, err = pcall(issue, { c.tunit }, c.unit:GetPosition())
            end
        elseif c.silo then
            -- a silo fires on the ground position (IssueNuke /
            -- IssueTactical), not an attack on the unit
            local issue = c.silo == 'nuke' and IssueNuke or IssueTactical
            -- the real Issue* drop a unit without the order's command cap:
            -- an enhancement silo must have gained it
            local cap = c.silo == 'nuke' and 'RULEUCC_Nuke' or 'RULEUCC_Tactical'
            if not c.unit:TestCommandCaps(cap) then c.capMissing = cap end
            okIssue, err = pcall(issue, { c.unit }, c.tunit:GetPosition())
        elseif c.oc == 'manual' then
            okIssue, err = pcall(IssueOverCharge, { c.unit }, c.tunit)
        else
            if c.oc == 'auto' then
                local okA, errA = pcall(c.unit.SetAutoOvercharge, c.unit, true)
                if not okA then c.issueErr = tostring(errA) end
            end
            okIssue, err = pcall(IssueAttack, { c.unit }, c.tunit)
        end
        if not okIssue then c.issueErr = tostring(err) end
    end
    for _, c in selected do
        if not c.done and c.unit and c.tunit and not c.dive then engage(c) end
    end
    for _, c in selected do
        if c.unit and not c.dive and not c.unit:BeenDestroyed() then c.unit:SetFireState(0) end
    end

    local active = {}
    for _, c in selected do
        if not c.done then
            table.insert(active, c)
            c.w = c.w or (c.unit and not c.unit:BeenDestroyed() and c.unit:GetWeapon(c.weapon) or nil)
        end
    end
    local deadline = maxBudget + 45
    local function rank_of(c)
        if c.dmg > 0 then return 'pass' end
        if c.fired > 0 then
            if c.impacted > 0 then return 'hit-no-damage' end
            return 'fired-no-hit'
        end
        if c.held then return 'held-no-fire' end
        return 'no-target'
    end
    -- WEAPON_AUDIT_TRACE = true: every 20 ticks per open case an AUDITTRACE
    -- line: the body's turn since the order, the weapon's aim manipulator
    -- heading / pitch (GetHeadingPitch, degrees), the weapon's target
    -- position, shots and credited impacts so far.
    local trace = rawget(_G, 'WEAPON_AUDIT_TRACE')
    local function trace_of(c, waited)
        local mh, mp = '-', '-'
        local okM, m = pcall(c.unit.GetWeaponManipulatorByLabel, c.unit, c.label)
        if okM and m then
            local okH, hh, pp = pcall(m.GetHeadingPitch, m)
            if okH and hh then mh, mp = string.format('%.1f', math.deg(hh)), string.format('%.1f', math.deg(pp)) end
        end
        local tps = '-'
        if c.w then
            local okP, tp = pcall(c.w.GetCurrentTargetPos, c.w)
            if okP and tp then tps = string.format('%.1f,%.1f,%.1f', tp[1], tp[2], tp[3]) end
        end
        local turn = c.h0 and string.format('%.1f', wrap180(math.deg(c.unit:GetHeading() - c.h0))) or '-'
        LOG(string.format('AUDITTRACE %s %s[%d] tick+%d turn %s aim %s/%s targetPos %s fired %d impacts %d',
            c.key, c.attacker, c.weapon, waited, turn, mh, mp, tps, c.fired, c.impacted))
    end
    -- every deadline and wait here is in game ticks since the loop began
    -- (a WaitTicks(4) is 3 of them)
    local loop0, nextTrace = GetGameTick(), 0
    local lived, waited = #active, 0
    while lived > 0 and waited < deadline do
        WaitTicks(4)
        waited = GetGameTick() - loop0
        lived = 0
        for _, c in active do
            if not c.done and c.dive and not c.attDied and c.unit and not c.unit:BeenDestroyed() then
                local got = c.unit:GetCurrentLayer()
                if got == c.attLayer then
                    c.dive = false
                    c.dived = GetGameTick() - c.diveTick
                    if c.tunit then engage(c) end
                    c.unit:SetFireState(0)
                elseif waited >= DIVE_WAIT then
                    c.unsupported = 'layer ' .. c.attLayer .. ' not reached after IssueDive (got ' .. tostring(got) .. ')'
                    c.reasonKey = 'layer'
                    resolve(c, 'unsupported')
                end
            end
            if not c.done then
                if c.unit and not c.attDied and c.unit:BeenDestroyed() then c.attDied = true end
                if c.tunit and not c.tDead and c.tunit:BeenDestroyed() then c.tDead = true end
                if c.w and not c.attDied and c.w:WeaponHasTarget() then
                    c.held = true
                    local t = c.w:GetCurrentTarget()
                    if t == c.tunit then c.heldCase = true end
                    if c.icpt and t then
                        for _, m in c.missiles do
                            if m == t then c.heldCase = true end
                        end
                    end
                end
                if c.icpt then judge_missiles(c) end
                if trace and waited >= nextTrace and c.unit and not c.unit:BeenDestroyed() then trace_of(c, waited) end
                if c.dmg > 0 then
                    resolve(c, 'pass')
                else
                    lived = lived + 1
                end
            end
        end
        if waited >= nextTrace then nextTrace = nextTrace + 20 end
    end
    for _, c in active do
        resolve(c, rank_of(c))
    end

    -- 6. Machine rows (case-order arrays; AUDIT_CASES is the reopening
    -- contract: same order, same ids as this run).
    local results = {}
    local evidence = evidence_of()
    local by = {}
    local i = 0
    for _, c in selected do
        i = i + 1
        results[i] = c.key .. '|' .. c.attacker .. '|' .. c.weapon .. '|' .. (c.target or '-') .. '|'
            .. (c.outcome or 'no-target') .. '|' .. c.fired .. '|' .. c.dmg
        local o = c.outcome or 'no-target'
        by[o] = (by[o] or 0) + 1
    end
    rawset(_G, 'AUDIT_RESULTS', results)
    rawset(_G, 'AUDIT_CASES', evidence)
    note('rows[1]: ' .. (results[1] or '-'))
    note('cases[1]: id=' .. (evidence[1] and evidence[1].id or '-') .. ' attacker=' .. (evidence[1] and evidence[1].attacker or '-')
        .. ' weapon=' .. (evidence[1] and evidence[1].weapon or '-') .. ' target=' .. (evidence[1] and evidence[1].target or '-')
        .. ' x=' .. (evidence[1] and evidence[1].x or '-') .. ' z=' .. (evidence[1] and evidence[1].z or '-')
        .. ' budget=' .. (evidence[1] and evidence[1].budget or '-'))
    local byStr = {}
    local keys = {}
    for k in by do table.insert(keys, k) end
    table.sort(keys)
    for _, k in keys do table.insert(byStr, k .. ' ' .. by[k]) end
    note('outcomes: ' .. table.concat(byStr, ', '))
    -- (a selection of statically known cases spawns nothing by design)
    if attempted > 0 and spawned == 0 then
        fail('no case could spawn')
        ScenarioResult.done = true
        return
    end
    ScenarioResult.done = true
end)
