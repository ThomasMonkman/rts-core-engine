-- Scenario test (tests/scenarios.rs): a transport flies its load and unload
-- legs at its Elevation and comes down to its TransportHoverHeight only
-- over the pick-up / drop point; the cargo it lets go falls to the ground
-- and changes layer there (the real game: dev/probes probe_mov_air TR1, h
-- 8.2..8.8 on the leg, down from 10 short, tanks on the ground 12 ticks
-- after the detach; ours flew the whole leg at the hover height).
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
        local t = CreateUnitHPR('uea0107', 'ARMY_1', 440, ground(440, 560) + 8, 560, 0, 0, 0)
        local tanks = {}
        for i = 1, 2 do table.insert(tanks, CreateUnitHPR('uel0201', 'ARMY_1', 398 + i * 3, ground(398 + i * 3, 560), 560, 0, 0, 0)) end
        WaitTicks(10)
        local elevation, hover = t.Blueprint.Physics.Elevation, t.Blueprint.Air.TransportHoverHeight
        -- The leg to the pick-up: at Elevation until it's near.
        IssueTransportLoad(tanks, t)
        local low_far, high = 100, 0
        for _ = 1, 120 do
            if table.getn(t:GetCargo()) == 2 then break end
            local p = pos(t)
            if VDist2(p[1], p[3], 402.5, 560) > 14 then low_far = math.min(low_far, height(t)) end
            WaitTicks(1)
        end
        check(table.getn(t:GetCargo()) == 2, 'loaded')
        check(low_far > elevation - 1.2, 'flies to the pick-up at its Elevation: lowest ' .. low_far .. ' vs ' .. elevation)
        check(math.abs(height(t) - hover) < 1, 'and picks up at its hover height: ' .. height(t))
        -- The leg to the drop: up to Elevation, down over the point.
        local detached, landed = nil, {}
        local detach = t.OnTransportDetach
        t.OnTransportDetach = function(self, bone, unit)
            detached = GetGameTick()
            return detach(self, bone, unit)
        end
        for i, u in tanks do
            local layer = u.OnLayerChange
            u.OnLayerChange = function(self, new, old)
                if new == 'Land' then landed[i] = GetGameTick() end
                return layer(self, new, old)
            end
        end
        IssueTransportUnload({ t }, { 480, 0, 560 })
        local low_leg, drop_height = 100, nil
        for _ = 1, 200 do
            local p = pos(t)
            local d = VDist2(p[1], p[3], 480, 560)
            high = math.max(high, height(t))
            if d > 14 and d < 60 then low_leg = math.min(low_leg, height(t)) end
            if detached and not drop_height then drop_height = height(t) end
            if detached and landed[1] and landed[2] then break end
            if detached and not landed[1] then
                check(tanks[1]:GetCurrentLayer() == 'Air' and height(tanks[1]) > 0, 'let go, the tank is in the air until it lands')
            end
            WaitTicks(1)
        end
        check(high > elevation - 0.5, 'climbs back to its Elevation with cargo: ' .. high .. ' vs ' .. elevation)
        check(low_leg > elevation - 1.5, 'holds it on the leg: lowest ' .. low_leg)
        check(drop_height ~= nil and drop_height < hover + 1, 'lets go at its hover height: ' .. tostring(drop_height) .. ' vs ' .. hover)
        check(detached ~= nil and landed[1] ~= nil and landed[2] ~= nil, 'both tanks came down')
        if detached and landed[1] then
            -- (The real game's fell 12 ticks from 3.5.)
            local fall = landed[1] - detached
            note('cargo on the ground ' .. fall .. ' ticks after the detach')
            check(fall >= 6 and fall <= 16, 'the cargo falls to the ground: ' .. fall .. ' ticks')
        end
        WaitTicks(3)
        check(height(tanks[1]) < 0.2 and tanks[1]:GetCurrentLayer() == 'Land', 'on the ground: ' .. height(tanks[1]))
        local p = pos(tanks[1])
        IssueMove({ tanks[1] }, { p[1] + 15, 0, p[3] })
        WaitTicks(40)
        local q = pos(tanks[1])
        check(VDist2(p[1], p[3], q[1], q[3]) > 5, 'and drives off')
    end)
    if not ok then check(false, tostring(err)) end
    ScenarioResult.done = true
end)
