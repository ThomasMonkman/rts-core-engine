-- Scenario test (tests/scenarios.rs): torpedo defence. A UEF T1 torpedo
-- launcher (ueb2109, army 2) attacks a ship that carries an anti-torpedo
-- weapon (TargetType RULEWTT_Projectile, OnlyAllow TORPEDO), 40 away in deep
-- water, for 300 ticks; each pair runs alone at a spot of its own, both
-- units immortal. The defence stops torpedoes two ways: its projectile
-- damages them (Aeon Quasar, Seraphim Ajellu), or its charge's DepthCharge
-- entity (FAF's defaultantiprojectile) retargets them onto itself with
-- SetNewTarget (Cybran anti-torpedo flares, the Smart Charges); either way
-- the ship takes less than the unguarded UEF frigate (ues0103) does.
-- The UEF Smart Charge (TIFSmartCharge, ues0201) has no aim and a launch
-- too slow to reach its torpedoes on a ballistic arc: it fires all the same,
-- as moho's weapons without an aim do.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) LOG('torpedo_intercept: ', msg) end
ForkThread(function()
    local ok, err = pcall(function()
        WaitTicks(2)
        local function depth(x, z) return GetSurfaceHeight(x, z) - GetTerrainHeight(x, z) end
        -- deep water for the launcher and a ship 40 along x, spots 100 apart
        local sites = {}
        local w, h = GetMapSize()
        for z = 100, h - 100, 60 do
            for x = 100, w - 100, 8 do
                if table.getn(sites) < 5 and depth(x, z) > 8 and depth(x + 20, z) > 8 and depth(x + 40, z) > 8 then
                    local clear = true
                    for _, s in sites do
                        if math.abs(s[2] - z) < 100 then clear = false end
                    end
                    if clear then table.insert(sites, { x, z }) end
                end
            end
        end
        check(table.getn(sites) >= 5, 'found five deep-water spots: ' .. table.getn(sites))
        if table.getn(sites) < 5 then return end

        -- The defender's anti-torpedo weapon index (nil: none).
        local function defence_of(bp)
            for wi, wbp in __blueprints[bp].Weapon or {} do
                if wbp.TargetType == 'RULEWTT_Projectile' and wbp.TargetRestrictOnlyAllow == 'TORPEDO' then return wi end
            end
            return nil
        end
        -- One engagement at site: the counts after 300 ticks.
        local function run(site, bp)
            local sx, sz = site[1], site[2]
            local ship = CreateUnitHPR(bp, 'ARMY_1', sx + 40, GetSurfaceHeight(sx + 40, sz), sz, 0, 0, 0)
            ship:SetMaxHealth(1e7) ship:SetHealth(nil, 1e7)
            local launcher = CreateUnitHPR('ueb2109', 'ARMY_2', sx, GetSurfaceHeight(sx, sz), sz, 0, 0, 0)
            launcher:SetMaxHealth(1e7) launcher:SetHealth(nil, 1e7)
            local r = { torpedoes = 0, shots = 0, damaged = 0, deflected = 0, shipDamage = 0, held = 0 }
            local tw = launcher:GetWeapon(1)
            local create = tw.CreateProjectileAtMuzzle
            tw.CreateProjectileAtMuzzle = function(self, muzzle)
                local p = create(self, muzzle)
                if type(p) == 'table' then
                    r.torpedoes = r.torpedoes + 1
                    local od = p.OnDamage
                    p.OnDamage = function(pp, ...)
                        if not pp.damagedSeen then pp.damagedSeen = true r.damaged = r.damaged + 1 end
                        if od then return od(pp, ...) end
                    end
                    local snt = p.SetNewTarget
                    p.SetNewTarget = function(pp, ...)
                        if not pp.deflectSeen then pp.deflectSeen = true r.deflected = r.deflected + 1 end
                        return snt(pp, ...)
                    end
                end
                return p
            end
            local di = defence_of(bp)
            local dw = di and ship:GetWeapon(di)
            if dw then
                local dcreate = dw.CreateProjectileAtMuzzle
                dw.CreateProjectileAtMuzzle = function(self, muzzle)
                    r.shots = r.shots + 1
                    return dcreate(self, muzzle)
                end
            end
            local sod = ship.OnDamage
            ship.OnDamage = function(self, instigator, amount, direction, kind)
                if instigator == launcher and kind ~= 'TreeForce' and kind ~= 'TreeFire' then
                    r.shipDamage = r.shipDamage + amount
                end
                return sod(self, instigator, amount, direction, kind)
            end
            WaitTicks(5)
            IssueAttack({ launcher }, ship)
            for _ = 1, 300 do
                WaitTicks(1)
                if dw and dw:WeaponHasTarget() then r.held = r.held + 1 end
            end
            note(string.format('%s: torpedoes %d, defence shots %d (held %d ticks), torpedoes damaged %d, deflected %d, ship damage %d',
                bp, r.torpedoes, r.shots, r.held, r.damaged, r.deflected, r.shipDamage))
            launcher:Destroy()
            ship:Destroy()
            return r
        end

        local control = run(sites[1], 'ues0103')
        check(control.torpedoes > 0 and control.shipDamage > 0, 'the unguarded frigate is hit')
        -- (defender, how it stops torpedoes)
        for i, d in { { 'uas0103', 'damaged' }, { 'xss0201', 'damaged' }, { 'xrs0205', 'deflected' }, { 'ues0201', 'deflected' } } do
            local r = run(sites[i + 1], d[1])
            check(r.shots > 0, d[1] .. ': its anti-torpedo fired')
            check(r[d[2]] > 0, d[1] .. ': torpedoes ' .. d[2] .. ' (' .. r[d[2]] .. ')')
            check(r.shipDamage < control.shipDamage, string.format('%s: took less than the unguarded frigate (%d vs %d)',
                d[1], r.shipDamage, control.shipDamage))
        end
    end)
    if not ok then check(false, tostring(err)) end
    ScenarioResult.done = true
end)
