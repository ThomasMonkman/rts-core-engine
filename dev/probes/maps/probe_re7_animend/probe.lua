-- Differential probe (sweep 7, part 5): when an animator's clip ends, for WaitFor and the
-- animation time. A live destroyer plays its death clips at a fixed rate; per beat near the start
-- and the end: GetAnimationTime, GetAnimationFraction; and the beat WaitFor returns.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re7_animend 60
--   ours: --headless --no-ai --ticks 300 --script dev/probes/maps/probe_re7_animend/probe.lua
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end
local CASES = {
    { 'a10', '/units/ues0201/ues0201_a001.sca', 10, 760, 700 },
    { 'a3', '/units/ues0201/ues0201_a001.sca', 3, 820, 700 },
    { 'd1', '/units/ues0201/ues0201_ADeath01.sca', 1, 880, 700 },
}
ForkThread(function()
    WaitTicks(5)
    for _, c in CASES do
        local tag, anim, rate, x, z = c[1], c[2], c[3], c[4], c[5]
        local u = CreateUnitHPR('ues0201', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
        u:SetFireState(1)
        local m = CreateAnimator(u)
        m:PlayAnim(anim)
        m:SetRate(rate)
        local start = GetGameTick()
        local dur = m:GetAnimationDuration()
        P(fmt('ST %s rate=%s duration=%.4f t=%.4f', tag, tostring(rate), dur, m:GetAnimationTime()))
        ForkThread(function()
            local last
            while true do
                WaitTicks(1)
                local t = m:GetAnimationTime()
                local k = GetGameTick() - start
                if k <= 3 or t >= dur - 0.5 * rate then
                    local s = fmt('%.4f %.4f', t, m:GetAnimationFraction())
                    if s ~= last then P(fmt('T %s +%d t=%s', tag, k, s)) last = s end
                end
                if k > 260 then break end
            end
        end)
        ForkThread(function()
            WaitFor(m)
            P(fmt('WF %s +%d t=%.4f', tag, GetGameTick() - start, m:GetAnimationTime()))
        end)
    end
    WaitTicks(240)
    LOG('PROBE done')
end)
