-- Scenario test (tests/scenarios.rs): a refused collision check costs no
-- time. A shot through a wall whose OnCollisionCheck says false (or nil)
-- reaches the wall behind it on the tick it would with nothing in the way
-- (FAF probe probe_wpn_proj2 `check.enemy-false`: +5, the same as a clear
-- path), and each wall is asked in the tick the shot reaches it.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local LOS = '/projectiles/SAALosaareAutoCannon01/SAALosaareAutoCannon01_proj.bp'
local function make(army, x, z)
    local u = CreateUnitHPR('ueb5101', army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
    return u
end
local function shoot(launcher, _, done)
    local p = launcher:CreateProjectile(LOS, 1.5, 0.3, 0, 1, 0, 0)
    local t0 = GetGameTick()
    p.OnImpact = function(self, tt, te)
        done(GetGameTick() - t0, te)
        self:Destroy()
    end
end
ForkThread(function()
    WaitTicks(2)
    -- One lane, shot three times: clear, through a wall answering false,
    -- then nil.
    local x, z = 664, 320
    local l = make('ARMY_1', x, z)
    local behind = make('ARMY_2', x + 12, z)
    local answer
    local ticks = {}
    for i, a in ipairs({ 'clear', false, 'nil' }) do
        answer = a
        if i == 2 then
            local w = make('ARMY_2', x + 6, z)
            w.OnCollisionCheck = function(self) if answer == 'nil' then return nil end return false end
        end
        shoot(l, 0, function(t, te) ticks[i] = { t = t, behind = te == behind } end)
        WaitTicks(12)
    end
    for i, what in ipairs({ 'clear path', 'refused (false)', 'refused (nil)' }) do
        check(ticks[i] ~= nil, what .. ': hit something')
        check(ticks[i] and ticks[i].behind, what .. ': hit the wall behind')
    end
    if ticks[1] and ticks[2] and ticks[3] then
        check(ticks[2].t == ticks[1].t, string.format('false costs no time: %d vs %d', ticks[2].t, ticks[1].t))
        check(ticks[3].t == ticks[1].t, string.format('nil costs no time: %d vs %d', ticks[3].t, ticks[1].t))
    end
    ScenarioResult.done = true
end)
