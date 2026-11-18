-- Differential probe (sweep 7): why every unit made in probe_re6_platoon's real run is killed with
-- no instigator 99 beats after it is made.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re7_kill 90
--   ours: --headless --no-ai --ticks 260 --script dev/probes/maps/probe_re7_kill/probe.lua
-- KEEP (set by probe_re7_killb) keeps the start units of ARMY_1 and ARMY_2, so those two
-- armies are not defeated; otherwise every start unit is destroyed, as in re6.
-- ST  victory option, share option, per army Status / IsDefeated / ArmyIsOutOfGame, IsGameOver,
--     SimUtils.GameIsEnding: logged on change, every beat.
-- DF  a brain's OnDefeat runs.
-- KL  a Kill() on a watched unit, with the Lua traceback of the caller.
-- OK  a watched unit's OnKilled callback, with its traceback.
local fmt = string.format
local KEEP = rawget(_G, 'PROBE_RE7_KEEP') or false
for i, b in ArmyBrains do
    if not (KEEP and (i == 1 or i == 2)) then
        for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
            if not u:BeenDestroyed() then u:Destroy() end
        end
    end
end
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end
local function tb()
    local s = debug.traceback('', 2) or ''
    s = string.gsub(s, '\n%s*', ' | ')
    return s
end
P(fmt('ST keep=%s victory=%s share=%s', tostring(KEEP), tostring(ScenarioInfo.Options.Victory), tostring(ScenarioInfo.Options.Share)))
for i, b in ArmyBrains do
    local orig = b.OnDefeat
    local idx = i
    b.OnDefeat = function(self)
        P(fmt('DF army %d OnDefeat %s', idx, tb()))
        return orig(self)
    end
end
local simutils = import('/lua/simutils.lua')
ForkThread(function()
    local last = ''
    while true do
        local parts = { fmt('over=%s ending=%s', tostring(IsGameOver()), tostring(simutils.GameIsEnding)) }
        for i, b in ArmyBrains do
            table.insert(parts, fmt('a%d=%s/%s/%s', i, tostring(b.Status), tostring(b:IsDefeated()), tostring(ArmyIsOutOfGame(i))))
        end
        local s = table.concat(parts, ' ')
        if s ~= last then P('ST ' .. s) last = s end
        WaitTicks(1)
    end
end)
local watched = {}
local function make(tag, bp, x, z, army)
    local u = CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetFireState(1)
    local kill = u.Kill
    u.Kill = function(self, a, b, c)
        P(fmt('KL %s Kill %s', tag, tb()))
        return kill(self, a, b, c)
    end
    u:AddUnitCallback(function(self, instigator)
        P(fmt('OK %s OnKilled by=%s %s', tag, instigator and 'entity' or 'nil', tb()))
    end, 'OnKilled')
    P(fmt('MK %s %s army=%s', tag, bp, army))
    table.insert(watched, { tag = tag, u = u })
    return u
end
ForkThread(function()
    while true do
        for _, w in watched do
            if not w.gone and w.u:BeenDestroyed() then
                w.gone = true
                P(fmt('GONE %s', w.tag))
            end
        end
        WaitTicks(1)
    end
end)
ForkThread(function()
    WaitTicks(5)
    make('a1t5', 'uel0201', 300, 300, 'ARMY_1')
    make('a3t5', 'uel0201', 300, 320, 'ARMY_3')
    make('a3e5', 'uel0105', 300, 340, 'ARMY_3')
    WaitTicks(35)
    make('a1t40', 'uel0201', 320, 300, 'ARMY_1')
    make('a3t40', 'uel0201', 320, 320, 'ARMY_3')
    WaitTicks(80)
    make('a1t120', 'uel0201', 340, 300, 'ARMY_1')
    make('a3t120', 'uel0201', 340, 320, 'ARMY_3')
    WaitTicks(120)
    LOG('PROBE done')
end)
