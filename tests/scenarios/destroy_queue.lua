-- Scenario test (tests/scenarios.rs): moho's destroy queue (MohoEngine.dll
-- `Entity::Destroy` 0x1025e420, the drain at the end of `Sim::AdvanceBeat`
-- 0x103194a1, `Entity::OnDestroy` 0x1025e4b0, `EntityDB::Purge` 0x10268270).
--  * `Destroy` sets `BeenDestroyed` at once; `OnDestroy` waits for the end of
--    the beat, after the Lua threads, with the entity still attached.
--  * A destroy made inside an `OnDestroy` runs after it, in the same drain
--    (no nesting).
--  * The parent hears `OnAttachedDestroyed`, the children `OnParentDestroyed`.
--  * `Kill` after `Destroy` in the same beat still runs `OnKilled`.
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local log = {}
local function note(s) table.insert(log, s) end

local function unit(x, tag)
    local u = CreateUnitHPR('uel0201', 'ARMY_1', x, GetTerrainHeight(x, 600), 600, 0, 0, 0)
    u.tag = tag
    local onDestroy, onKilled = u.OnDestroy, u.OnKilled
    u.OnDestroy = function(self)
        note(tag .. ':destroy@' .. GetGameTick() .. (self:GetParent() ~= self and ':attached' or ''))
        if self.alsoDestroy then self.alsoDestroy:Destroy() end
        note(tag .. ':end')
        return onDestroy(self)
    end
    u.OnKilled = function(self, ...)
        note(tag .. ':killed')
        return onKilled(self, ...)
    end
    u.OnAttachedDestroyed = function(self, other) note(tag .. ':attachedDestroyed:' .. other.tag) end
    u.OnParentDestroyed = function(self, other) note(tag .. ':parentDestroyed:' .. other.tag) end
    u.OnParentKilled = function(self, other) note(tag .. ':parentKilled:' .. other.tag) end
    local attachedKilled = u.OnAttachedKilled
    u.OnAttachedKilled = function(self, other)
        note(tag .. ':attachedKilled:' .. other.tag)
        return attachedKilled(self, other)
    end
    return u
end

ForkThread(function()
    WaitTicks(2)
    -- Destroy: deferred, not nested.
    local a, b = unit(400, 'a'), unit(404, 'b')
    a.alsoDestroy = b
    local tick = GetGameTick()
    a:Destroy()
    check(a:BeenDestroyed(), 'BeenDestroyed at once')
    check(table.getn(log) == 0, 'OnDestroy not run inside Destroy: ' .. table.concat(log, ' '))
    -- A thread forked now still runs this tick, before the drain.
    ForkThread(function() note('thread@' .. GetGameTick()) end)
    WaitTicks(1)
    local want = 'thread@' .. tick .. ' a:destroy@' .. tick .. ' a:end b:destroy@' .. tick .. ' b:end'
    check(table.concat(log, ' ') == want, 'drain order: ' .. table.concat(log, ' ') .. ' (want ' .. want .. ')')

    -- Attachments: the child's OnDestroy runs attached, then the parent hears.
    log = {}
    local p, c = unit(420, 'p'), unit(424, 'c')
    c:AttachBoneTo(-2, p, -1)
    c:Destroy()
    WaitTicks(1)
    check(table.concat(log, ' ') == 'c:destroy@' .. (GetGameTick() - 1) .. ':attached c:end p:attachedDestroyed:c',
        'child destroy: ' .. table.concat(log, ' '))
    log = {}
    local c2 = unit(428, 'c2')
    c2:AttachBoneTo(-2, p, -1)
    p:Destroy()
    WaitTicks(1)
    check(table.concat(log, ' ') == 'p:destroy@' .. (GetGameTick() - 1) .. ' p:end c2:parentDestroyed:p',
        'parent destroy: ' .. table.concat(log, ' '))
    check(c2:GetParent() == c2, 'children detached after the parent OnDestroy')

    -- Kill after Destroy still runs OnKilled.
    log = {}
    local k = unit(440, 'k')
    k:Destroy()
    k:Kill()
    check(log[1] == 'k:killed', 'Kill after Destroy runs OnKilled: ' .. table.concat(log, ' '))

    ScenarioResult.done = true
end)
