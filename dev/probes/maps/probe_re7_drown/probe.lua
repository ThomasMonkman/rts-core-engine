-- Differential probe (sweep 7, part 3): which units drown on the seabed. Land tanks lose 1% of
-- max health a beat under water (probe_re7_kill2); this checks amphibious / hover / ACU units,
-- a tank that cannot take damage, a tank warped out of the water, and one warped into it.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re7_drown 60
--   ours: --headless --no-ai --ticks 80 --script dev/probes/maps/probe_re7_drown/probe.lua
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end
local watched = {}
local function make(tag, bp, x, z)
    local u = CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local dmg, kil = u.OnDamage, u.OnKilled
    u.OnDamage = function(self, instigator, amount, vector, damageType)
        P(fmt('DM %s amount=%s type=%s', tag, tostring(amount), tostring(damageType)))
        return dmg(self, instigator, amount, vector, damageType)
    end
    u.OnKilled = function(self, instigator, type, overkill)
        P(fmt('OK %s type=[%s] overkill=%s by=%s hp=%.2f', tag, tostring(type), tostring(overkill), instigator and 'entity' or 'nil', self:GetHealth()))
        return kil(self, instigator, type, overkill)
    end
    P(fmt('MK %s %s at %d %d terrain=%.2f surface=%.2f', tag, bp, x, z, GetTerrainHeight(x, z), GetSurfaceHeight(x, z)))
    table.insert(watched, { tag = tag, u = u, last = '' })
    return u
end
ForkThread(function()
    while true do
        for _, w in watched do
            if not w.gone then
                if w.u:BeenDestroyed() then
                    w.gone = true
                    P(fmt('GONE %s', w.tag))
                else
                    local p = w.u:GetPosition()
                    local s = fmt('hp=%.2f layer=%s pos=%.1f %.1f %.1f', w.u:GetHealth(), tostring(w.u.Layer), p[1], p[2], p[3])
                    if s ~= w.last then P(fmt('HS %s %s', w.tag, s)) w.last = s end
                end
            end
        end
        WaitTicks(1)
    end
end)
ForkThread(function()
    WaitTicks(5)
    -- a dry spot: walk from the shallow point (450, 520) until the terrain is above the water
    local start = import('/lua/sim/ScenarioUtilities.lua').GetMarker('ARMY_1').position
    local lx, lz = math.floor(start[1]), math.floor(start[3])
    P(fmt('LANDH terrain=%.2f surface=%.2f', GetTerrainHeight(lx, lz), GetSurfaceHeight(lx, lz)))
    P(fmt('LAND %s %s', tostring(lx), tostring(lz)))
    make('wagner', 'url0203', 300, 300)
    make('acu', 'uel0001', 310, 300)
    make('hover', 'ual0201', 320, 300)
    local nd = make('tank_nodmg', 'uel0201', 330, 300)
    nd:SetCanTakeDamage(false)
    local out = make('tank_out', 'uel0201', 340, 300)
    local into
    if lx then into = make('tank_in', 'uel0201', lx, lz) end
    WaitTicks(20)
    if lx then
        Warp(out, Vector(lx + 4, GetTerrainHeight(lx + 4, lz), lz))
        Warp(into, Vector(350, GetTerrainHeight(350, 300), 300))
        P('WARPED')
    end
    WaitTicks(25)
    LOG('PROBE done')
end)
