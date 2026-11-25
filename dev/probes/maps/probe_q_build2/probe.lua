-- Differential probe (open question): how a FACTORY build and a structure
-- UPGRADE end when the builder is killed, destroyed, cleared or stopped part
-- way (probe_q_build covered mobile builders only). Lanes, each on its own
-- units, acted on 20 ticks after the build starts:
--   factory (ueb0101 building uel0201): fkill (Kill), fdestroy (Destroy),
--     fclear (IssueClearCommands), fstop (IssueStop);
--   upgrade (ueb3101 to ueb3201): ukill, udestroy, uclear, ustop.
-- Builders are hooked per instance (FAF's states override the class's
-- callbacks), the units being built per class.
--   real game: run_faf.ps1 probe_q_build2 180
--   ours: --headless --no-ai --ticks 260 --script dev/probes/maps/probe_q_build2/probe.lua
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local armies = ListArmies()
local A1 = armies[1]
local brain = ArmyBrains[1]
local t0 = GetGameTick()
local tagOf = {}
local function tag(u)
    if type(u) ~= 'table' then return tostring(u) end
    if tagOf[u] then return tagOf[u] end
    local ok, id = pcall(function() return u:GetUnitId() .. '#' .. u:GetEntityId() end)
    return ok and id or '?'
end
local function S(x) if type(x) == 'table' then return tag(x) end return tostring(x) end
local function P(s) LOG(fmt('PROBE %d %s', GetGameTick() - t0, s)) end
local function make(bp, x, z) return CreateUnitHPR(bp, A1, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local laneOf = {}
-- The unit being built takes its builder's lane name.
local function built(self, builder)
    if not tagOf[self] and laneOf[builder] then tagOf[self] = 'new-' .. laneOf[builder] end
end
local function hookClass(path, names)
    local ok, mod = pcall(import, path)
    local cls = ok and mod and mod.TypeClass
    if not cls then P('hook failed ' .. path) return end
    for _, name in names do
        local orig = cls[name]
        local n = name
        cls[n] = function(self, a, b, c, d)
            if n == 'OnStartBeingBuilt' then built(self, a) end
            P(tag(self) .. ' ' .. n .. ' ' .. S(a) .. ' ' .. S(b))
            if orig then return orig(self, a, b, c, d) end
        end
    end
end
-- Per instance, so a state's own callback is seen too: the original is
-- looked up on the object's current class or state at call time.
local function hookUnit(u, names)
    for _, name in names do
        local n = name
        u[n] = function(self, a, b, c, d)
            if n == 'OnStartBuild' then built(a, self) end
            P(tag(self) .. ' ' .. n .. ' ' .. S(a) .. ' ' .. S(b))
            local f = getmetatable(self)[n]
            if f then return f(self, a, b, c, d) end
        end
    end
end
hookClass('/units/uel0201/uel0201_script.lua', { 'OnStartBeingBuilt', 'OnStopBeingBuilt', 'OnFailedToBeBuilt', 'OnKilled', 'OnDestroy' })
hookClass('/units/ueb3201/ueb3201_script.lua', { 'OnStartBeingBuilt', 'OnStopBeingBuilt', 'OnFailedToBeBuilt', 'OnKilled', 'OnDestroy' })

for i = 0, 3 do
    make('ueb1105', 608 + 4 * i, 80)
    make('ueb1106', 608 + 4 * i, 86)
end
local function clearProps(x, z)
    local ok, list = pcall(GetReclaimablesInRect, Rect(x - 16, z - 16, x + 16, z + 16))
    if not ok or not list then return end
    for _, r in list do if IsProp(r) and not r:BeenDestroyed() then r:Destroy() end end
end
local lanes = {
    { 'fkill', 656, 160, 'f' }, { 'fdestroy', 696, 160, 'f' }, { 'fclear', 736, 160, 'f' }, { 'fstop', 776, 160, 'f' },
    { 'ukill', 656, 224, 'u' }, { 'udestroy', 696, 224, 'u' }, { 'uclear', 736, 224, 'u' }, { 'ustop', 776, 224, 'u' },
}
local function report(l, when)
    local b = l.b
    local bs = b:BeenDestroyed() and 'gone' or fmt('dead %s', tostring(b.Dead))
    local s = l.s
    local ss = s:BeenDestroyed() and 'gone' or fmt('frac %.4f hp %.1f dead %s', s:GetFractionComplete(), s:GetHealth(), tostring(s.Dead))
    P(fmt('%s %s builder %s new %s', l[1], when, bs, ss))
end
ForkThread(function()
    WaitTicks(3)
    brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)
    for _, l in lanes do clearProps(l[2], l[3]) end
    WaitTicks(2)
    t0 = GetGameTick()
    for _, l in lanes do
        local b = make(l[4] == 'f' and 'ueb0101' or 'ueb3101', l[2], l[3])
        tagOf[b] = 'b-' .. l[1]
        laneOf[b] = l[1]
        l.b = b
        hookUnit(b, { 'OnStartBuild', 'OnStopBuild', 'OnFailedToBuild', 'OnKilled', 'OnDestroy' })
        if l[4] == 'f' then IssueBuildFactory({ b }, 'uel0201', 1) else IssueUpgrade({ b }, 'ueb3201') end
    end
    for t = 1, 160 do
        WaitTicks(1)
        for _, l in lanes do
            local b = l.b
            if not l.s and not b:BeenDestroyed() then
                local f = b:GetFocusUnit()
                if f then
                    l.s = f l.start = t
                    if not tagOf[f] then tagOf[f] = 'new-' .. l[1] end
                    P(fmt('%s started', l[1]))
                end
            end
            if l.s and not l.acted and t == l.start + 20 then
                l.acted = t
                report(l, 'act')
                if l[1] == 'fkill' or l[1] == 'ukill' then b:Kill()
                elseif l[1] == 'fdestroy' or l[1] == 'udestroy' then b:Destroy()
                elseif l[1] == 'fclear' or l[1] == 'uclear' then IssueClearCommands({ b })
                elseif l[1] == 'fstop' or l[1] == 'ustop' then IssueStop({ b })
                end
                P(fmt('%s acted', l[1]))
            end
            if l.acted then
                local d = t - l.acted
                if d == 1 or d == 2 or d == 3 or d == 5 or d == 10 or d == 30 then report(l, '+' .. d) end
            end
        end
    end
    P('done')
end)
