-- Differential probe (sweep 7, part 4): ship death animations with the pick and rate fixed, so
-- both engines play the same animation. Each ship's ChooseAnimBlock returns blueprint entry k with
-- no rate fields (rate 1). Logs the watch bone's height every 5 beats and the destroy beat.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re7_sink 70
--   ours: --headless --no-ai --ticks 400 --script dev/probes/maps/probe_re7_sink/probe.lua
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end
local CASES = {
    { 'des1', 'ues0201', 1, 760, 700 },
    { 'des2', 'ues0201', 2, 820, 700 },
    { 'des3', 'ues0201', 3, 880, 700 },
    { 'bs1', 'ues0302', 1, 760, 780 },
    { 'bs2', 'ues0302', 2, 830, 800 },
    { 'bs3', 'ues0302', 3, 900, 830 },
    { 'fr1', 'ues0103', 1, 700, 650 },
}
local cases = {}
ForkThread(function()
    WaitTicks(5)
    for _, c in CASES do
        local tag, bp, k, x, z = c[1], c[2], c[3], c[4], c[5]
        local u = CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
        u:SetFireState(1)
        u.ChooseAnimBlock = function(self, blocks)
            return { Animation = blocks[k].Animation, Weight = 100 }
        end
        local wb = u:GetBlueprint().WatchBone or 0
        P(fmt('MK %s %s anim=%d at %d %d floor=%.2f surface=%.2f watchbone=%s', tag, bp, k, x, z, GetTerrainHeight(x, z) + GetTerrainTypeOffset(x, z), GetSurfaceHeight(x, z), tostring(wb)))
        table.insert(cases, { tag = tag, u = u, wb = wb })
    end
    WaitTicks(10)
    for _, c in cases do
        c.killed = GetGameTick()
        c.u:Kill()
    end
    for i = 0, 330 do
        local open = false
        for _, c in cases do
            local u = c.u
            if not c.gone then
                if u:BeenDestroyed() then
                    c.gone = true
                    P(fmt('GONE %s +%d', c.tag, GetGameTick() - c.killed))
                else
                    open = true
                    local m = u.DeathAnimManip
                    if m and not c.an then
                        c.an = true
                        P(fmt('AN %s +%d rate=%.2f duration=%.2f', c.tag, GetGameTick() - c.killed, m:GetRate(), m:GetAnimationDuration()))
                    end
                    if math.mod(i, 5) == 0 then
                        local p = u:GetPosition(c.wb)
                        local r = u:GetPosition()
                        P(fmt('Y %s +%d watch=%.2f %.2f %.2f root=%.2f', c.tag, GetGameTick() - c.killed, p[1], p[2], p[3], r[2]))
                    end
                end
            end
        end
        if not open then break end
        WaitTicks(1)
    end
    LOG('PROBE done')
end)
