-- Differential probe (open question): OnFailedToBeBuilt when a mobile builder
-- abandons a part-built structure. Lanes (engineer rate 20 on a T1 generator,
-- abandoned 14 ticks after the build starts):
--   clear (IssueClearCommands), stop (IssueStop), kill (Kill), destroy (Destroy),
--   newbuild (IssueClearCommands + a new IssueBuildMobile elsewhere).
--   real game: run_faf.ps1 probe_q_build 120
--   ours: --headless --no-ai --ticks 200 --script dev/probes/maps/probe_q_build/probe.lua
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
local function hookClass(path, names)
    local ok, mod = pcall(import, path)
    local cls = ok and mod and mod.TypeClass
    if not cls then P('hook failed ' .. path) return end
    for _, name in names do
        local orig = cls[name]
        local n = name
        cls[n] = function(self, a, b, c, d)
            P(tag(self) .. ' ' .. n .. ' ' .. S(a) .. ' ' .. S(b))
            if orig then return orig(self, a, b, c, d) end
        end
    end
end
hookClass('/units/uel0105/uel0105_script.lua', { 'OnStartBuild', 'OnStopBuild', 'OnFailedToBuild', 'OnKilled', 'OnDestroy' })
hookClass('/units/ueb1101/ueb1101_script.lua', { 'OnStartBeingBuilt', 'OnStopBeingBuilt', 'OnFailedToBeBuilt', 'OnKilled', 'OnDestroy', 'OnDecayed' })

for i = 0, 3 do
    make('ueb1105', 608 + 4 * i, 80)
    make('ueb1106', 608 + 4 * i, 86)
end
local function clearProps(x, z)
    local ok, list = pcall(GetReclaimablesInRect, Rect(x - 20, z - 20, x + 40, z + 20))
    if not ok or not list then return end
    for _, r in list do if IsProp(r) and not r:BeenDestroyed() then r:Destroy() end end
end
local lanes = {
    { 'clear', 784, 144 }, { 'stop', 656, 160 }, { 'kill', 736, 224 }, { 'destroy', 816, 224 }, { 'newbuild', 912, 240 },
}
ForkThread(function()
    WaitTicks(3)
    brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)
    for _, l in lanes do clearProps(l[2], l[3]) end
    WaitTicks(2)
    t0 = GetGameTick()
    for _, l in lanes do
        local e = make('uel0105', l[2], l[3] + 4)
        e:SetBuildRate(20)
        tagOf[e] = 'eng-' .. l[1]
        l.e = e
        IssueBuildMobile({ e }, { l[2], GetTerrainHeight(l[2], l[3]), l[3] }, 'ueb1101', {})
    end
    local done = 0
    for t = 1, 140 do
        WaitTicks(1)
        for _, l in lanes do
            local e = l.e
            if not l.s and not e:BeenDestroyed() then
                local f = e:GetFocusUnit()
                if f then l.s = f l.start = t tagOf[f] = 'gen-' .. l[1] end
            end
            if l.s and not l.acted and t == l.start + 14 then
                l.acted = true
                P(fmt('%s act frac %.3f', l[1], l.s:GetFractionComplete()))
                if l[1] == 'clear' then IssueClearCommands({ e })
                elseif l[1] == 'stop' then IssueStop({ e })
                elseif l[1] == 'kill' then e:Kill()
                elseif l[1] == 'destroy' then e:Destroy()
                elseif l[1] == 'newbuild' then
                    IssueClearCommands({ e })
                    IssueBuildMobile({ e }, { l[2] + 12, GetTerrainHeight(l[2] + 12, l[3]), l[3] }, 'ueb1101', {})
                end
                P(fmt('%s acted', l[1]))
            end
            if l.acted and not l.rep and t == l.start + 40 then
                l.rep = true
                P(fmt('%s +26 gen %s', l[1], l.s:BeenDestroyed() and 'gone' or fmt('frac %.4f', l.s:GetFractionComplete())))
            end
        end
    end
    P('done')
end)
