-- Scenario test (tests/scenarios.rs): DamageArea's damageSelf counts only
-- with damageFriendly: the instigator is one of its own friends
-- (probe_wpn_damage, probe_wpn_misc2 `self friendly=f self=t: me 0 mate 0 foe 1`).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end LOG('damage_area_self: ', ok and 'ok ' or 'FAIL ', msg) end
local function make(bp, army, x, z) return CreateUnitHPR(bp, army, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
ForkThread(function()
    WaitTicks(2)
    local me = make('ueb5101', 'ARMY_1', 400, 600)
    local mate = make('ueb5101', 'ARMY_1', 402, 600)
    local foe = make('ueb5101', 'ARMY_2', 398, 600)
    local hits = {}
    for tag, u in { me = me, mate = mate, foe = foe } do
        local t = tag
        u.OnDamage = function() hits[t] = (hits[t] or 0) + 1 end
    end
    local function who(friendly, self)
        hits = {}
        DamageArea(me, me:GetPosition(), 5, 1, 'Normal', friendly, self)
        return string.format('me %d mate %d foe %d', hits.me or 0, hits.mate or 0, hits.foe or 0)
    end
    local r = who(false, true)
    check(r == 'me 0 mate 0 foe 1', 'friendly=f self=t: ' .. r)
    r = who(true, false)
    check(r == 'me 0 mate 1 foe 1', 'friendly=t self=f: ' .. r)
    r = who(false, false)
    check(r == 'me 0 mate 0 foe 1', 'friendly=f self=f: ' .. r)
    r = who(true, true)
    check(r == 'me 1 mate 1 foe 1', 'friendly=t self=t: ' .. r)
    ScenarioResult.done = true
end)
