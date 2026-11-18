-- Differential probe (sweep 7, part 2): in probe_re7_kill's real run, idle T1 tanks die with no
-- instigator 99 beats after they are made (no Lua Kill() call), but an engineer does not.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re7_kill2 90
--   ours: --headless --no-ai --ticks 160 --script dev/probes/maps/probe_re7_kill2/probe.lua
-- Units of several kinds and spots; per unit, on change each beat: health, layer, position,
-- fire state; every OnDamage (amount, type, instigator) and OnKilled (type, overkill), with
-- a traceback; whether the unit's start units were kept makes no odds (all destroyed here).
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end
local function tb()
    local s = tostring(debug.traceback())
    return (string.gsub(s, '\n%s*', ' | '))
end
local watched = {}
local function make(tag, bp, x, z, fire)
    local u = CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
    if fire then u:SetFireState(fire) end
    local dmg, kil = u.OnDamage, u.OnKilled
    u.OnDamage = function(self, instigator, amount, vector, damageType)
        P(fmt('DM %s amount=%s type=%s by=%s', tag, tostring(amount), tostring(damageType), instigator and 'entity' or 'nil'))
        return dmg(self, instigator, amount, vector, damageType)
    end
    u.OnKilled = function(self, instigator, type, overkill)
        P(fmt('OK %s type=%s overkill=%s by=%s hp=%.1f %s', tag, tostring(type), tostring(overkill), instigator and 'entity' or 'nil', self:GetHealth(), tb()))
        return kil(self, instigator, type, overkill)
    end
    P(fmt('MK %s %s at %d %d terrain=%.2f surface=%.2f fire=%s', tag, bp, x, z, GetTerrainHeight(x, z), GetSurfaceHeight(x, z), tostring(fire)))
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
                    local s = fmt('hp=%.1f layer=%s pos=%.1f %.1f %.1f dead=%s', w.u:GetHealth(), tostring(w.u.Layer), p[1], p[2], p[3], tostring(w.u.Dead))
                    if s ~= w.last then P(fmt('HS %s %s', w.tag, s)) w.last = s end
                end
            end
        end
        WaitTicks(1)
    end
end)
ForkThread(function()
    WaitTicks(5)
    make('tank_hold', 'uel0201', 300, 300, 1)
    make('tank_free', 'uel0201', 310, 300, nil)
    make('tank_far', 'uel0201', 450, 520, nil)
    make('eng', 'uel0105', 320, 300, nil)
    make('scout', 'uel0101', 330, 300, nil)
    make('arty', 'uel0103', 340, 300, nil)
    make('aa', 'uel0104', 350, 300, nil)
    make('t3', 'uel0303', 360, 300, nil)
    make('pd', 'ueb2101', 380, 300, nil)
    make('mex', 'ueb1103', 390, 300, nil)
    make('bomber', 'uea0103', 400, 300, nil)
    WaitTicks(140)
    LOG('PROBE done')
end)
