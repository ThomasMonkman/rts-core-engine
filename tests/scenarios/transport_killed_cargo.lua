-- Scenario test (tests/scenarios.rs): a transport killed in flight. Its
-- cargo is detached (OnTransportDetach, OnDetachedFromTransport, then the
-- cargo's OnKilled, before the transport's), flies on with the transport's
-- velocity, falls and gets OnImpact('Terrain') after changing layer (the
-- real game: dev/probes probe_mov_air TR3, 16 ticks from h 8.8; ours left
-- the dead cargo hanging in the air with no callbacks).
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function ground(x, z) return GetTerrainHeight(x, z) end
local function height(u) local p = u:GetPosition() return p[2] - ground(p[1], p[3]) end
ForkThread(function()
    local ok, err = pcall(function()
        WaitTicks(2)
        local t = CreateUnitHPR('uea0107', 'ARMY_1', 400, ground(400, 560) + 8, 560, 0, 0, 0)
        local tanks = {}
        for i = 1, 2 do table.insert(tanks, CreateUnitHPR('uel0201', 'ARMY_1', 398 + i * 3, ground(398 + i * 3, 566), 566, 0, 0, 0)) end
        WaitTicks(10)
        IssueTransportLoad(tanks, t)
        for _ = 1, 100 do
            if table.getn(t:GetCargo()) == 2 then break end
            WaitTicks(1)
        end
        check(table.getn(t:GetCargo()) == 2, 'loaded')
        IssueMove({ t }, { 480, 0, 560 })
        WaitTicks(40)
        local seq, impact = {}, {}
        local function hook(obj, name, tag, at)
            local orig = obj[name]
            obj[name] = function(self, a, b, c)
                table.insert(seq, tag)
                if at then at(a) end
                if orig then return orig(self, a, b, c) end
            end
        end
        hook(t, 'OnTransportDetach', 'detach')
        hook(t, 'OnKilled', 'killed')
        for i, u in tanks do
            hook(u, 'OnDetachedFromTransport', 'off' .. i)
            hook(u, 'OnKilled', 'dead' .. i)
            hook(u, 'OnLayerChange', 'layer' .. i)
            hook(u, 'OnImpact', 'impact' .. i, function(with) impact[i] = { tick = GetGameTick(), with = with } end)
        end
        local killed = GetGameTick()
        local before, high = tanks[1]:GetPosition(), height(tanks[1])
        local y = before[2]
        t:Kill()
        local got = table.concat(seq, ' ')
        check(got == 'detach off1 dead1 detach off2 dead2 killed', 'the kill detaches the cargo first: ' .. got)
        check(not tanks[1]:IsUnitState('Attached'), 'the cargo is off the transport')
        WaitTicks(5)
        local p = tanks[1]:GetPosition()
        check(p[1] - before[1] > 3, 'it flies on with the speed it had: ' .. (p[1] - before[1]))
        check(p[2] < y - 0.3, 'and falls: ' .. p[2] .. ' from ' .. y)
        for _ = 1, 40 do
            if impact[1] and impact[2] then break end
            WaitTicks(1)
        end
        for i = 1, 2 do
            check(impact[i] ~= nil and impact[i].with == 'Terrain', 'tank ' .. i .. " gets OnImpact('Terrain')")
            if impact[i] then
                local fall = impact[i].tick - killed
                note('tank ' .. i .. ' hit the ground ' .. fall .. ' ticks after the kill, from ' .. high)
                check(fall >= 12 and fall <= 22, 'tank ' .. i .. ' falls for 12..22 ticks: ' .. fall)
            end
        end
        got = table.concat(seq, ' ')
        check(string.find(got, 'layer1 impact1', 1, true) ~= nil and string.find(got, 'layer2 impact2', 1, true) ~= nil, 'layer change, then the impact: ' .. got)
        WaitTicks(60)
        for i, u in tanks do check(IsDestroyed(u) or u.Dead, 'tank ' .. i .. ' is dead') end
    end)
    if not ok then check(false, tostring(err)) end
    ScenarioResult.done = true
end)
