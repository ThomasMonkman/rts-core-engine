-- Air demo: every aircraft in the game (all factions, T1 to T4) for you to
-- fly, and an enemy range to try them on.
--
--   cargo run -- --no-ai --player 1 --script dev/scenes/play/air_demo.lua
--
-- Your army (ARMY_1) gets every gunship, bomber, fighter, transport, scout
-- and experimental, parked in rows by role (gunships front, experimentals
-- back), with a full economy. The camera starts on them.
-- The enemy range (ARMY_2) sits AIR_DEMO_RANGE to the east: tanks and
-- structures to attack, and a few interceptors patrolling for dogfights.
-- Everything there holds fire, so nothing shoots back; what dies comes back
-- 20 s later.
-- Set AUTO = true below to have the fleet fly itself: gunships attack the
-- tanks, bombers bomb the structures, fighters chase the interceptors,
-- transports pick up the tanks next to them, the rest patrol.

local AUTO = false
local SPACING = 9          -- between aircraft in a row
local ROW_GAP = 16         -- between rows
local RANGE_DISTANCE = 150 -- fleet to enemy range

-- Every flyable aircraft: mobile AIR units with a mesh, minus build drones
-- (they need their commander), satellites, civilians and placeholders.
local function air_ids()
    local ids = {}
    for id, bp in __blueprints do
        local c = type(id) == 'string' and not string.find(id, '/') and bp.CategoriesHash
        if c and c.AIR and c.MOBILE and bp.Display and bp.Display.Mesh
            and not (c.DUMMYUNIT or c.INSIGNIFICANTUNIT or c.CIVILIAN or c.SATELLITE or c.POD)
            and not (bp.General and bp.General.Icon == 'air' and c.ENGINEER and c.SUBCOMMANDER)
            and not (id == 'uea0001' or id == 'uea0003' or id == 'xea3204')
        then
            table.insert(ids, id)
        end
    end
    table.sort(ids)
    return ids
end

-- Rows by role (display order). A unit goes to the first role, in
-- `priority` order, whose test it passes: experimentals before bombers (the
-- Ahwassa is a BOMBER), gunships (GROUNDATTACK) before transports (the
-- Stinger can carry a unit).
local ROLES = {
    { name = 'gunships', priority = 2, test = function(c) return c.GROUNDATTACK end },
    { name = 'bombers', priority = 3, test = function(c) return c.BOMBER end },
    { name = 'fighters', priority = 4, test = function(c) return c.ANTIAIR end },
    { name = 'transports', priority = 5, test = function(c) return c.TRANSPORTATION end },
    { name = 'scouts and others', priority = 6, test = function(c) return true end },
    { name = 'experimentals', priority = 1, test = function(c) return c.EXPERIMENTAL end },
}

local function role_of(c)
    local best
    for i, r in ROLES do
        if r.test(c) and (not best or r.priority < ROLES[best].priority) then best = i end
    end
    return best
end

local function dry(x, z) return GetSurfaceHeight(x, z) <= GetTerrainHeight(x, z) + 0.5 end

-- A dry, fairly flat w x h patch (corner), scanning the map: the first one,
-- or with `near` = {x, z} the closest to it.
local function find_land(w, h, avoid, near, rough)
    local size = GetMapSize and GetMapSize() or 1024
    local best, bx, bz
    for z = 40, size - h - 40, 16 do
        for x = 40, size - w - 40, 16 do
            local ok = not (avoid and avoid(x, z))
            local lo, hi = 1e9, -1e9
            for dz = 0, h, 16 do
                if not ok then break end
                for dx = 0, w, 16 do
                    if not dry(x + dx, z + dz) then ok = false break end
                    local y = GetTerrainHeight(x + dx, z + dz)
                    lo, hi = math.min(lo, y), math.max(hi, y)
                end
            end
            if ok and hi - lo < (rough or 12) then
                if not near then return x, z end
                local d = math.pow(x + w / 2 - near[1], 2) + math.pow(z + h / 2 - near[2], 2)
                if not best or d < best then best, bx, bz = d, x, z end
            end
        end
    end
    return bx, bz
end

local fleet = {}
local range = {}

local function spawn_range(rx, rz)
    local function make(bp, x, z, flying)
        local y = GetTerrainHeight(x, z) + (flying and 20 or 0)
        local u = CreateUnitHPR(bp, 'ARMY_2', x, y, z, 0, 0, 0)
        u:SetFireState(1) -- hold fire
        return u
    end
    local tanks, buildings, planes = {}, {}, {}
    for i = 0, 11 do
        table.insert(tanks, make(({ 'uel0201', 'ual0201', 'url0107', 'xsl0201' })[i % 4 + 1], rx + (i % 4) * 7, rz + math.floor(i / 4) * 7))
    end
    for i, bp in { 'ueb1101', 'uab1101', 'urb1101', 'xsb1101', 'ueb0101', 'urb0101' } do
        table.insert(buildings, make(bp, rx + 45 + ((i - 1) % 3) * 12, rz + math.floor((i - 1) / 3) * 14))
    end
    for i, bp in { 'uea0102', 'ura0102', 'uaa0102', 'xsa0102' } do
        local p = make(bp, rx + 20 + i * 8, rz + 50, true)
        IssuePatrol({ p }, { rx - 20, GetTerrainHeight(rx - 20, rz + 60), rz + 60 })
        IssuePatrol({ p }, { rx + 80, GetTerrainHeight(rx + 80, rz + 60), rz + 60 })
        table.insert(planes, p)
    end
    return tanks, buildings, planes
end

ForkThread(function()
    WaitTicks(2)
    for _, army in { 'ARMY_1', 'ARMY_2' } do
        local b = GetArmyBrain(army)
        b:__engine_SetStorage('MASS', 100000) b:__engine_SetStorage('ENERGY', 1000000)
        b:GiveResource('MASS', 100000) b:GiveResource('ENERGY', 1000000)
    end

    -- Sort the aircraft into role rows.
    local rows = {}
    for _, role in ROLES do table.insert(rows, { name = role.name, ids = {} }) end
    for _, id in air_ids() do
        table.insert(rows[role_of(__blueprints[id].CategoriesHash)].ids, id)
    end

    local width = 0
    for _, r in rows do width = math.max(width, table.getn(r.ids) * SPACING) end
    local depth = table.getn(rows) * ROW_GAP + 40
    local fx, fz = find_land(width + 40, depth + 20)
    if not fx then
        WARN('air demo: no dry patch big enough on this map')
        return
    end
    -- The range: a patch at least RANGE_DISTANCE clear of the fleet's
    -- (so nothing fires until you send it).
    local fw, fd = width + 40, depth + 60
    local rx, rz = find_land(90, 60, function(x, z)
        local gx = math.max(fx - (x + 90), x - (fx + fw), 0)
        local gz = math.max(fz - (z + 60), z - (fz + fd), 0)
        return math.sqrt(gx * gx + gz * gz) < RANGE_DISTANCE
    end, { fx + fw / 2, fz + fd / 2 }, 30)

    -- The fleet, parked in the air in rows.
    local z = fz + 20
    for _, r in rows do
        local x = fx + 20
        local step = r.name == 'experimentals' and SPACING * 4 or SPACING
        for _, id in r.ids do
            local ok, u = pcall(CreateUnitHPR, id, 'ARMY_1', x, GetTerrainHeight(x, z) + 15, z, 0, 0, 0)
            if ok and u then
                table.insert(fleet, { unit = u, role = r.name })
            else
                WARN('air demo: could not create ', id, ': ', tostring(u))
            end
            x = x + step
        end
        LOG('AIR_DEMO ', r.name, ': ', table.concat(r.ids, ' '))
        z = z + (r.name == 'transports' and ROW_GAP * 1.5 or ROW_GAP)
    end
    -- Cargo for the transports: a few tanks beside the transport row.
    local cargo = {}
    for i = 0, 7 do
        table.insert(cargo, CreateUnitHPR('uel0201', 'ARMY_1', fx + 20 + i * 4, GetTerrainHeight(fx + 20 + i * 4, fz + 10), fz + 10, 0, 0, 0))
    end
    LOG(string.format('AIR_DEMO fleet of %d at %d, %d; enemy range at %s, %s; camera on the fleet: --camera %d,%d,%d',
        table.getn(fleet), fx, fz, tostring(rx), tostring(rz), fx + width / 2 + 20, fz + depth / 2 + 20, math.max(120, width)))

    if not rx then
        WARN('air demo: no room for the enemy range')
        return
    end
    local tanks, buildings, planes = spawn_range(rx, rz)

    -- Bring the range back as it gets destroyed.
    ForkThread(function()
        while true do
            WaitSeconds(20)
            local alive = 0
            for _, u in tanks do if not u.Dead then alive = alive + 1 end end
            for _, u in buildings do if not u.Dead then alive = alive + 1 end end
            if alive < 9 then
                tanks, buildings, planes = spawn_range(rx, rz)
                LOG('AIR_DEMO enemy range respawned')
            end
        end
    end)

    if not AUTO then return end
    WaitSeconds(3)
    local function by_role(name)
        local out = {}
        for _, f in fleet do if f.role == name and not f.unit.Dead then table.insert(out, f.unit) end end
        return out
    end
    local function alive(list) local out = {} for _, u in list do if not u.Dead then table.insert(out, u) end end return out end
    while true do
        local t, b, p = alive(tanks), alive(buildings), alive(planes)
        if t[1] then IssueAttack(by_role('gunships'), t[1]) end
        if b[1] then IssueAttack(by_role('bombers'), b[1]) end
        if p[1] then IssueAttack(by_role('fighters'), p[1]) end
        local transports = by_role('transports')
        if transports[1] then IssueTransportLoad(alive(cargo), transports[1]) end
        local others = by_role('scouts and others')
        IssuePatrol(others, { rx, GetTerrainHeight(rx, rz), rz })
        IssuePatrol(by_role('experimentals'), { rx + 40, GetTerrainHeight(rx + 40, rz), rz + 20 })
        WaitSeconds(15)
    end
end)
