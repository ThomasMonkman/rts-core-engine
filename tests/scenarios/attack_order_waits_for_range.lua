-- Scenario test (tests/scenarios.rs): an attack order's target becomes its
-- weapons' target once the attacker is in range of it, not when the order is
-- given, as in the real game (dev/probes probe_q_needprep2, probe_q_needprep3,
-- probe_q_attackrange):
--   K a destroyer (`ues0201`, 60) shooting a frigate behind it, ordered onto one
--     140 ahead: nothing happens at the order; within 60 of the far one its
--     weapons are reset and take it the tick after.
--   L a battlecruiser (`xes0307`, `NeedPrep` Hiro cannons, 80): its cannons hear
--     `OnGotTarget` with no target on the tick it comes within 80, and take the
--     target the tick after.
--   J a destroyer ordered onto a power generator on the shore: taken within 60
--     of its centre, and both turrets fire (the ship turns its broadside).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local fmt = string.format
ForkThread(function()
    WaitTicks(2)
    local ordered = nil
    local function make(bp, army, x, z, heading)
        local y = math.max(GetSurfaceHeight(x, z), GetTerrainHeight(x, z))
        local u = CreateUnitHPR(bp, army, x, y, z, 0, heading, 0)
        u:SetMaxHealth(1000000) u:SetHealth(nil, 1000000)
        return u
    end
    local function target(bp, x, z)
        local t = make(bp, 'ARMY_2', x, z, 0)
        t:SetFireState(1)
        return t
    end
    local function lane(name, bp, x, z, heading)
        local l = { name = name, u = make(bp, 'ARMY_1', x, z, heading), events = {}, fires = {} }
        l.dist = function()
            local p, q = l.target:GetPosition(), l.u:GetPosition()
            return math.sqrt((p[1] - q[1]) * (p[1] - q[1]) + (p[3] - q[3]) * (p[3] - q[3]))
        end
        for i = 1, l.u:GetWeaponCount() do
            local w = l.u:GetWeapon(i)
            local label = w:GetBlueprint().Label
            -- Each hook steps aside for the class's own function (FAF's
            -- weapon states swap these), then puts itself back.
            local hooks = {}
            local function hook(event, seen)
                hooks[event] = function(self)
                    if ordered then seen(self) end
                    self[event] = nil
                    self[event](self)
                    self[event] = hooks[event]
                end
                w[event] = hooks[event]
            end
            for _, event in { 'OnGotTarget', 'OnLostTarget' } do
                local name = event
                hook(name, function(self)
                    table.insert(l.events, { tick = GetGameTick() - ordered, label = label, event = name, d = l.dist(),
                        has = self:WeaponHasTarget(), onOrder = self:GetCurrentTarget() == l.target })
                end)
            end
            hook('OnFire', function(self)
                if self:GetCurrentTarget() == l.target then l.fires[label] = (l.fires[label] or 0) + 1 end
            end)
        end
        return l
    end
    local lk = lane('K', 'ues0201', 645, 640, math.pi / 2)
    target('ues0103', 615, 640)
    lk.target = target('ues0103', 785, 640)
    local ll = lane('L', 'xes0307', 545, 840, math.pi / 2)
    ll.target = target('ues0103', 685, 840)
    local lj = lane('J', 'ues0201', 545, 700, -math.pi / 2)
    lj.target = target('ueb1101', 410, 700)
    WaitTicks(30)
    check(lk.u:GetWeapon(1):WeaponHasTarget(), 'K is shooting the frigate behind it before the order')
    ordered = GetGameTick()
    for _, l in { lk, ll, lj } do IssueAttack({ l.u }, l.target) end
    WaitTicks(260)

    -- The first event of a kind for a weapon, from `from` on.
    local function first(l, label, event, has, from)
        for _, e in l.events do
            if e.label == label and e.event == event and (has == nil or e.has == has) and e.tick >= (from or 0) then return e end
        end
    end
    -- K: its turrets keep the near frigate through the order.
    for _, e in lk.events do
        if e.tick <= 5 then check(false, fmt('K %s %s at T+%d: nothing happens at the order', e.label, e.event, e.tick)) end
    end
    for _, c in { { lk, 'FrontTurret01', 60 }, { lk, 'BackTurret01', 60 }, { lj, 'FrontTurret01', 60 }, { lj, 'BackTurret01', 60 }, { ll, 'HiroCannonFront', 80 }, { ll, 'HiroCannonBack', 80 } } do
        local l, label, range = c[1], c[2], c[3]
        local got
        for _, e in l.events do
            if e.label == label and e.event == 'OnGotTarget' and e.onOrder then got = e break end
        end
        check(got ~= nil, fmt('%s %s took the order\'s target', l.name, label))
        if got then
            note(fmt('%s %s took the target at T+%d, %.1f away', l.name, label, got.tick, got.d))
            -- The ship moves up to 0.6 a tick, and the target is taken the tick after it's in range.
            check(got.d <= range and got.d > range - 1.5, fmt('%s %s took the target %.1f away, not just inside %d', l.name, label, got.d, range))
        end
    end
    -- L: the prep the tick before the target, with no target.
    for _, label in { 'HiroCannonFront', 'HiroCannonBack' } do
        local prep = first(ll, label, 'OnGotTarget', false)
        local got = first(ll, label, 'OnGotTarget', true)
        check(prep ~= nil and got ~= nil and got.tick == prep.tick + 1,
            fmt('L %s: OnGotTarget with no target at T+%s, with it at T+%s', label, tostring(prep and prep.tick), tostring(got and got.tick)))
        check(prep == nil or (prep.d <= 80 and prep.d > 79), fmt('L %s got ready %.1f away', label, prep and prep.d or -1))
    end
    check(first(lk, 'FrontTurret01', 'OnGotTarget', false) == nil, 'K (no NeedPrep): no OnGotTarget without a target')
    -- J: both turrets fire on the shore generator.
    for _, label in { 'FrontTurret01', 'BackTurret01' } do
        note(fmt('J %s fired %d times', label, lj.fires[label] or 0))
        check((lj.fires[label] or 0) > 0, 'J ' .. label .. ' fired on the generator')
    end
    ScenarioResult.done = true
end)
