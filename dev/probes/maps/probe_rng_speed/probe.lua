-- Speed census: every 1000 ticks, the wall clock and the units alive (total, per army), and which
-- armies have no units left. Runs to tick 36000 (one hour), then `PROBE done`.
--   real game: dev/probes/run_faf.ps1 probe_rng_speed 2700   (launch_args.txt: 8 RNGAI, 4v4 odd/even)
--   ours: --session --ai RNGStandard --mod .. --teams 2 --ticks 36000 --script <this> --ui-lua <ui_probe.lua>
local fmt = string.format
local LAST = 36000
local function wall() return GetSystemTimeSecondsOnlyForProfileUse() end
local w0 = wall()
local last = w0
ForkThread(function()
    local t0 = GetGameTick()
    while true do
        local t = GetGameTick() - t0
        if math.mod(t, 1000) == 0 then
            local now = wall()
            local per, total, dead = {}, 0, {}
            for _, brain in ArmyBrains do
                local i = brain:GetArmyIndex()
                if not ArmyIsCivilian(i) then
                    local n = table.getn(brain:GetListOfUnits(categories.ALLUNITS - categories.WALL, false) or {})
                    total = total + n
                    table.insert(per, tostring(n))
                    if n == 0 or brain:IsDefeated() then table.insert(dead, tostring(i)) end
                end
            end
            LOG(fmt('PROBE %d S wall=%.1f dt=%.1f units=%d per=%s dead=%s', GetGameTick(), now - w0, now - last,
                total, table.concat(per, ','), table.concat(dead, ',')))
            last = now
        end
        if t == 30000 then LOG('PROBE shot late') end
        if t >= LAST then
            LOG('PROBE done')
            return
        end
        WaitTicks(1)
    end
end)
LOG('PROBE 0 start')
