-- Every emitter blueprint the game ships, in a grid on the land patch of
-- scmp_009 (alphabetical, row-major, 4 ogrids apart, 20 per row), each on
-- its own invulnerable wall segment and re-created whenever its lifetime
-- runs out. The index (row, column -> blueprint) is logged as `EMITTER`
-- lines. Beam and trail blueprints are skipped (they need two ends / a
-- moving bone).
-- Run: cargo run -- --script dev/scenes/visual/all_emitters.lua --no-ai --camera 380,600,40
local files = DiskFindFiles('/effects/emitters', '*.bp')
table.sort(files)
local emitters = {}
for _, f in files do
    -- Beams and trails are named `_beam_` / `polytrail`; the rest are
    -- emitters (the oddly named files too).
    if not string.find(f, '_beam_') and not string.find(f, 'polytrail') then
        table.insert(emitters, f)
    end
end
LOG('EMITTER count ' .. table.getn(emitters))

local COLUMNS, SPACING = 20, 4
local x0, z0 = 345, 530
local hosts = {}
for i, bp in emitters do
    local row, col = math.floor((i - 1) / COLUMNS), math.mod(i - 1, COLUMNS)
    local x, z = x0 + col * SPACING, z0 + row * SPACING
    local u = CreateUnitHPR('ueb5101', 1, x, GetSurfaceHeight(x, z), z, 0, 0, 0)
    u:SetCanTakeDamage(false)
    hosts[i] = u
    LOG(string.format('EMITTER row %d col %d at %d,%d %s', row, col, x, z, bp))
end

-- Re-create each emitter when it ends: an effect object's lifetime is the
-- blueprint's, so poll the ones we made and respawn those gone.
ForkThread(function()
    local live = {}
    while true do
        for i, bp in emitters do
            local e = live[i]
            if not e or e:BeenDestroyed() then
                live[i] = CreateEmitterAtEntity(hosts[i], 1, bp)
            end
        end
        WaitTicks(10)
    end
end)
