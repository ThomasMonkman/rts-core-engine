-- Probe: is a unit still under construction in its army's pool platoon, and
-- can FormPlatoon take it?
-- watch: a factory builds a scout; each tick, is it among the pool's units
--        and does CanFormPlatoon see it?
-- form:  a second factory builds a scout; FormPlatoon is tried every tick
--        from the moment the build starts.
-- site:  an engineer builds a power generator; same checks for a structure.
local armies = ListArmies()
local army = armies[1]
local brain = GetArmyBrain(army)
local sx, sz = brain:GetArmyStartPos()
brain:GiveStorage('MASS', 100000)
brain:GiveStorage('ENERGY', 100000)
brain:GiveResource('MASS', 100000)
brain:GiveResource('ENERGY', 100000)
local pool = brain:GetPlatoonUniquelyNamed('ArmyPool')
local function inPool(u)
    for _, v in pool:GetPlatoonUnits() do
        if v == u then return true end
    end
    return false
end
local function template(name, bp) return { name, '', { bp, 1, 1, 'Attack', 'None' } } end
local running = 0

local function factory(tag, ox, form)
    running = running + 1
    local x, z = sx + ox, sz + 30
    local f = CreateUnitHPR('ueb0101', army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local built
    local old = f.OnStartBuild
    f.OnStartBuild = function(self, unit, order)
        built = unit
        LOG('PROBE ', tag, ' OnStartBuild t=', GetGameTick(), ' inPool=', tostring(inPool(unit)))
        return old(self, unit, order)
    end
    WaitTicks(10)
    LOG('PROBE ', tag, ' before: canForm=', tostring(pool:CanFormPlatoon(template(tag, 'uel0101'), 1, { x, 0, z }, 40)))
    IssueBuildFactory({ f }, 'uel0101', 1)
    local t0 = GetGameTick()
    local last = ''
    local formed = false
    for i = 1, 200 do
        WaitTicks(1)
        if built then
            local dead = built:BeenDestroyed()
            local s = 'inPool=' .. tostring(not dead and inPool(built)) ..
                ' canForm=' .. tostring(pool:CanFormPlatoon(template(tag, 'uel0101'), 1, { x, 0, z }, 40)) ..
                ' beingBuilt=' .. tostring(not dead and built:IsBeingBuilt())
            if s ~= last then
                LOG('PROBE ', tag, ' dt=', GetGameTick() - t0, ' frac=', (not dead) and built:GetFractionComplete() or -1, ' ', s)
                last = s
            end
            if form and not formed then
                local p = pool:FormPlatoon(template(tag, 'uel0101'), 1, { x, 0, z }, 40)
                if p then
                    formed = true
                    local units = p:GetPlatoonUnits()
                    LOG('PROBE ', tag, ' dt=', GetGameTick() - t0, ' FormPlatoon gave ', table.getn(units), ' unit(s), is the scout=', tostring(units[1] == built),
                        ' frac=', built:GetFractionComplete(), ' beingBuilt=', tostring(built:IsBeingBuilt()))
                end
            end
            if not dead and not built:IsBeingBuilt() and i > 20 and (formed or not form) then break end
        end
    end
    LOG('PROBE ', tag, ' end formed=', tostring(formed))
    running = running - 1
end

local function site(tag, ox)
    running = running + 1
    local x, z = sx + ox, sz - 30
    local e = CreateUnitHPR('uel0105', army, x, GetTerrainHeight(x, z), z, 0, 0, 0)
    local built
    local old = e.OnStartBuild
    e.OnStartBuild = function(self, unit, order)
        built = unit
        LOG('PROBE ', tag, ' OnStartBuild t=', GetGameTick(), ' inPool=', tostring(inPool(unit)))
        return old(self, unit, order)
    end
    WaitTicks(10)
    IssueBuildMobile({ e }, { x + 6, GetTerrainHeight(x + 6, z), z }, 'ueb1101', {})
    local last = ''
    for i = 1, 400 do
        WaitTicks(1)
        if built then
            local s = 'inPool=' .. tostring(inPool(built)) ..
                ' canForm=' .. tostring(pool:CanFormPlatoon(template(tag, 'ueb1101'), 1, { x, 0, z }, 40)) ..
                ' beingBuilt=' .. tostring(built:IsBeingBuilt())
            if s ~= last then
                LOG('PROBE ', tag, ' t=', GetGameTick(), ' frac=', built:GetFractionComplete(), ' ', s)
                last = s
            end
            if not built:IsBeingBuilt() and i > 20 then break end
        end
    end
    LOG('PROBE ', tag, ' end')
    running = running - 1
end

ForkThread(function()
    WaitTicks(10)
    ForkThread(factory, 'watch', 30, false)
    ForkThread(factory, 'form', 100, true)
    ForkThread(site, 'site', 30)
    WaitTicks(30)
    while running > 0 do WaitTicks(5) end
    LOG('PROBE done')
end)
