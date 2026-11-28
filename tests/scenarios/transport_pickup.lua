-- Scenario test (tests/scenarios.rs): a transport ordered to load flies to
-- its cargo, which gathers at its group's centre, and every unit attaches on
-- the same tick (the real game: dev/probes probe_mov_air TR1; ours had the
-- cargo drive to a waiting transport and board one at a time).
-- GetPosition() is one table per entity, rewritten by each call: copy it to keep it.
local function pos(e) local p = e:GetPosition() return Vector(p[1], p[2], p[3]) end
rawset(_G, "ScenarioResult", { done = false, failures = {}, notes = {} })
local function check(ok, msg) if not ok then table.insert(ScenarioResult.failures, msg) end end
local function note(msg) table.insert(ScenarioResult.notes, msg) end
local function ground(x, z) return GetTerrainHeight(x, z) end
local function height(u) local p = pos(u) return p[2] - ground(p[1], p[3]) end
ForkThread(function()
    local ok, err = pcall(function()
        WaitTicks(2)
        local t = CreateUnitHPR('uea0107', 'ARMY_1', 430, ground(430, 560) + 8, 560, 0, 0, 0)
        local tanks = {}
        for i = 1, 4 do table.insert(tanks, CreateUnitHPR('uel0201', 'ARMY_1', 395 + i * 3, ground(395 + i * 3, 560), 560, 0, 0, 0)) end
        WaitTicks(10)
        local from, start = {}, pos(t)
        for i, u in tanks do from[i] = pos(u) end
        local attach, stop = {}, nil
        local attached = t.OnTransportAttach
        t.OnTransportAttach = function(self, bone, unit)
            table.insert(attach, GetGameTick())
            return attached(self, bone, unit)
        end
        local stopped = t.OnStopTransportLoading
        t.OnStopTransportLoading = function(self)
            stop = GetGameTick()
            return stopped(self)
        end
        local ordered = GetGameTick()
        IssueTransportLoad(tanks, t)
        -- The unload is queued behind the load (script orders never clear).
        IssueTransportUnload({ t }, { 460, 0, 560 })
        WaitTicks(1)
        check(table.getn(t:GetCommandQueue()) == 2, 'the transport has the load order and the unload behind it: ' .. table.getn(t:GetCommandQueue()))
        check(t:IsUnitState('TransportLoading'), 'loading from the order on')
        for _ = 1, 100 do
            if table.getn(t:GetCargo()) == 4 then break end
            WaitTicks(1)
        end
        check(table.getn(attach) == 4, 'four attached: ' .. table.getn(attach))
        if table.getn(attach) == 4 then
            check(attach[1] == attach[4], 'all on one tick: ' .. table.concat(attach, ' '))
            check(stop == attach[4], 'OnStopTransportLoading on that tick: ' .. tostring(stop))
            -- The real game's took 63 ticks from 28 away (ours is quicker
            -- to start the beam); driving to the transport took 95.
            note('loaded ' .. (attach[4] - ordered) .. ' ticks after the order')
            check(attach[4] - ordered <= 70, 'loaded within 70 ticks: ' .. (attach[4] - ordered))
        end
        local at = pos(t)
        check(VDist2(at[1], at[3], 402.5, 560) < 3, 'the transport came to the cargo (402.5, 560): ' .. at[1] .. ', ' .. at[3])
        check(VDist2(at[1], at[3], start[1], start[3]) > 20, 'it flew there: ' .. VDist2(at[1], at[3], start[1], start[3]))
        check(math.abs(height(t) - t.Blueprint.Air.TransportHoverHeight) < 1, 'down at its hover height: ' .. height(t))
        -- Then it carries them off to the queued drop.
        for _ = 1, 200 do
            if table.getn(t:GetCargo()) == 0 then break end
            WaitTicks(1)
        end
        check(table.getn(t:GetCargo()) == 0, 'unloaded at the queued point')
        local p = pos(t)
        check(VDist2(p[1], p[3], 460, 560) < 4, 'over the drop point: ' .. p[1] .. ', ' .. p[3])
        -- A second load: cargo that only had to close up never drove far.
        WaitTicks(30)
        for i, u in tanks do from[i] = pos(u) end
        IssueClearCommands({ t })
        IssueMove({ t }, { 430, 0, 560 })
        WaitTicks(60)
        IssueTransportLoad(tanks, t)
        for _ = 1, 100 do
            if table.getn(t:GetCargo()) == 4 then break end
            WaitTicks(1)
            for i, u in tanks do
                if not u:IsUnitState('Attached') and not u:IsUnitState('Immobile') then
                    local q = pos(u)
                    check(VDist2(q[1], q[3], from[i][1], from[i][3]) < 8, 'tank ' .. i .. ' stays by its group: ' .. VDist2(q[1], q[3], from[i][1], from[i][3]))
                end
            end
        end
        check(table.getn(t:GetCargo()) == 4, 'all four aboard again: ' .. table.getn(t:GetCargo()))
        check(t:IsIdleState(), 'the load order is done')
    end)
    if not ok then check(false, tostring(err)) end
    ScenarioResult.done = true
end)
