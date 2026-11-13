-- Selection bracket cases on scmp_009: every faction's transports in the
-- air over land, and the experimental and T3 battleships on the nearest
-- deep water, so their brackets can be checked against their meshes. Run
-- with --no-ai.
--
-- Each unit logs `SELCASE <bp> id <n> at x z` in game.log; pass the ids to
-- --select (applied at 6 s of interface time: --screenshot-tick 80 or so).
-- The `SELCASE water` and `SELCASE land` lines give the --camera points.
local function spawn(bp, x, z, alt)
    local u = CreateUnitHPR(bp, 1, x, GetSurfaceHeight(x, z) + (alt or 0), z, 0, 0, 0)
    if u then
        u:SetCanTakeDamage(false)
        LOG(string.format("SELCASE %s id %s at %d %d", bp, tostring(u:GetEntityId()), x, z))
    end
    return u
end

-- Transports, a row per faction: T1, T2, (UEF T3).
local transports = {
    { 'uea0107', 'uea0104', 'xea0306' },
    { 'uaa0107', 'uaa0104' },
    { 'ura0107', 'ura0104' },
    { 'xsa0107', 'xsa0104' },
}
local lx, lz = 680, 380
LOG(string.format("SELCASE land %d %d", lx + 20, lz + 30))
for row, bps in ipairs(transports) do
    for i, bp in ipairs(bps) do
        spawn(bp, lx + i * 14, lz + row * 12, 8)
    end
end

-- Deep water nearest the land patch: the first 8x8 cell, ring by ring,
-- whose whole square is at least 8 deep (the deepest on scmp_009 is about 13).
local function deep(x, z)
    for dx = -24, 24, 8 do
        for dz = -24, 24, 8 do
            if GetSurfaceHeight(x + dx, z + dz) - GetTerrainHeight(x + dx, z + dz) < 8 then
                return false
            end
        end
    end
    return true
end
local wx, wz
for r = 0, 400, 8 do
    for dx = -r, r, 8 do
        for _, dz in ipairs({ -r, r }) do
            if not wx and deep(lx + dx, lz + dz) then wx, wz = lx + dx, lz + dz end
            if not wx and deep(lx + dz, lz + dx) then wx, wz = lx + dz, lz + dx end
        end
    end
    if wx then break end
end
if wx then
    LOG(string.format("SELCASE water %d %d", wx, wz))
    -- Experimentals (Atlantis, Tempest), then the T3 battleships.
    local ships = { 'ues0401', 'uas0401', 'ues0302', 'uas0302', 'urs0302', 'xss0302' }
    for i, bp in ipairs(ships) do
        local col, row = (i - 1) % 2, math.floor((i - 1) / 2)
        spawn(bp, wx - 20 + col * 40, wz - 30 + row * 30)
    end
else
    WARN('SELCASE no deep water found')
end
