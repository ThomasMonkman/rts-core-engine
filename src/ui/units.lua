-- UserUnit: the user layer's view of a sim unit, rebuilt from snapshots
-- (moho's UserUnit, /engine/User/UserUnit.lua). The engine keeps one object
-- per unit id the interface has seen (__engine_unit_objects) and its latest
-- data (__engine_unit_data[id] = {bp, army, health, build, x, y, z} plus
-- the selected units' details: queue, econ, stats, ...).
local methods = moho.userunit_methods
local objects = {}
local data = {}
rawset(_G, '__engine_unit_objects', objects)
rawset(_G, '__engine_unit_data', data)

local function d(self) return data[self._id] end

methods.GetEntityId = function(self) return tostring(self._id) end
methods.GetUnitId = function(self) local u = d(self) return u and u.bp end
methods.GetBlueprint = function(self) local u = d(self) return u and __blueprints[u.bp] end
methods.GetArmy = function(self) local u = d(self) return u and u.army or -1 end
-- The sim's current maximum (a buff or script may have changed it from
-- the blueprint's).
methods.GetMaxHealth = function(self)
    local u = d(self)
    if u and u.max_health then return u.max_health end
    local bp = self:GetBlueprint()
    return bp and bp.Defense and bp.Defense.MaxHealth or 1
end
methods.GetHealth = function(self) local u = d(self) return u and u.health * self:GetMaxHealth() or 0 end
methods.GetFractionComplete = function(self) local u = d(self) return u and u.build or 1 end
methods.GetPosition = function(self) local u = d(self) return u and Vector(u.x, u.y, u.z) or Vector(0, 0, 0) end
methods.GetInterpolatedPosition = methods.GetPosition
methods.IsDead = function(self) return d(self) == nil end
methods.IsIdle = function(self) local u = d(self) return not (u and u.queue and u.queue[1]) end
methods.IsInCategory = function(self, category)
    local u = d(self)
    local cat = categories[category]
    return u ~= nil and cat ~= nil and EntityCategoryContains(cat, u.bp)
end
-- Blueprints in the FACTORY category, by id (looked up once each).
local factoryBlueprints = {}
local function isFactory(bp)
    local known = factoryBlueprints[bp]
    if known == nil then
        known = categories.FACTORY ~= nil and EntityCategoryContains(categories.FACTORY, bp) or false
        factoryBlueprints[bp] = known
    end
    return known
end
-- A FACTORY's is its factory command queue (its rally orders), not its
-- builds: moho's GetCommandQueue takes the unit's factory queue when it has
-- one (MohoEngine.dll 0x104865d0), as FAF's UserUnit.lua says and its
-- rallypoint.lua relies on.
methods.GetCommandQueue = function(self)
    local u = d(self)
    local out = {}
    local queue = u and (isFactory(u.bp) and u.factoryQueue or u.queue)
    for i, c in queue or {} do
        out[i] = { type = c.type, ID = c.ID, position = Vector(c.x, c.y, c.z), blueprintId = c.blueprintId }
    end
    return out
end
methods.GetWorkProgress = function(self) local u = d(self) return u and u.work or 0 end
methods.GetFocus = function(self)
    local u = d(self)
    if u and u.focus and data[u.focus] then return __engine_unit(u.focus) end
    return nil
end
-- The unit its current Guard order guards (from its details).
methods.GetGuardedEntity = function(self) local g = (d(self) or {}).guarding return g and GetUnitById(g) end
-- The unit that made this one (from its details: selected units).
methods.GetCreator = function(self) local c = (d(self) or {}).creator return c and GetUnitById(c) end
methods.GetEconData = function(self)
    local u = d(self)
    return (u and u.econ) or { energyConsumed = 0, energyProduced = 0, energyRequested = 0, massConsumed = 0, massProduced = 0, massRequested = 0 }
end
methods.GetFuelRatio = function(self) local u = d(self) return u and u.fuel or -1 end
methods.GetShieldRatio = function(self) local u = d(self) return u and u.shield or 0 end
-- The sim's current rate (enhancements, buffs and veterancy change it).
methods.GetBuildRate = function(self)
    local u = d(self)
    if u and u.build_rate then return u.build_rate end
    local bp = self:GetBlueprint()
    return bp and bp.Economy and bp.Economy.BuildRate or 0
end
methods.GetStat = function(self, name, default)
    local u = d(self)
    local v = u and u.stats and u.stats[name]
    -- moho fills a missing stat with the default (sim `GetStat` creates
    -- it); the UI side only reads, so it returns the default instead.
    if v == nil then v = default or 0 end
    return { Value = v }
end
methods.GetCustomName = function(self) return self._customName end
methods.SetCustomName = function(self, name) self._customName = name end
-- Selection sets (control groups) the unit belongs to: FAF's selection.lua
-- keeps the sets and tags units through these.
methods.GetSelectionSets = function(self)
    local out = {}
    for name in (self._sets or {}) do table.insert(out, name) end
    table.sort(out)
    return out
end
methods.HasSelectionSet = function(self, set) return self._sets ~= nil and self._sets[tostring(set)] == true end
-- Moho's (MohoEngine.dll, `UserUnit`); FAF's annotations leave it out.
methods.AddSelectionSet = function(self, set) self._sets = self._sets or {} self._sets[tostring(set)] = true end
methods.RemoveSelectionSet = function(self, set) if self._sets then self._sets[tostring(set)] = nil end end
-- Units of the given blueprints in a category expression (`UI_SelectByCategory`:
-- names intersected), as entity ids; `pairs` is { id, blueprintId, ... }.
function __engine_filter_category(list, expr)
    -- A full category expression (`ENGINEER - COMMAND`, FAF's
    -- `UISelectionByCategory`): parsed as moho does.
    if string.find(expr, '[%+%-%*%(%)]') then
        local cat = ParseEntityCategory(expr)
        local out = {}
        for i = 1, table.getn(list), 2 do
            if __blueprints[list[i + 1]] and EntityCategoryContains(cat, list[i + 1]) then table.insert(out, list[i]) end
        end
        return out
    end
    local names = {}
    for name in string.gfind(expr, '%S+') do table.insert(names, string.upper(name)) end
    local out = {}
    for i = 1, table.getn(list), 2 do
        local bp = __blueprints[list[i + 1]]
        local hash = bp and bp.CategoriesHash
        local ok = hash ~= nil
        for _, n in names do
            if ok and n ~= 'ALLUNITS' and not hash[n] then ok = false end
        end
        if ok then table.insert(out, list[i]) end
    end
    return out
end
methods.IsAutoMode = function(self) local u = d(self) return u and u.autoMode or false end
methods.IsAutoSurfaceMode = function(self) local u = d(self) return u and u.autoSurface or false end
methods.IsRepeatQueue = function(self) return self._repeat == true end
-- Unit settings sent to the sim like orders (`__engine_ui_orders`); the
-- repeat flag is remembered here for `IsRepeatQueue`.
methods.ProcessInfo = function(self, command, value)
    local on = value == true or value == 'true'
    local orders = rawget(_G, '__engine_ui_orders')
    if command == 'SetRepeatQueue' then
        self._repeat = on
        table.insert(orders, { units = { self }, toggle = 'repeat', value = on and 1 or 0 })
    elseif command == 'SetPaused' then
        SetPaused({ self }, on)
    elseif command == 'SetFireState' then
        SetFireState({ self }, value)
    elseif command == 'PlayNoStagingPlatformsVO' or command == 'PlayBusyStagingPlatformsVO' then
        table.insert(orders, { units = { self }, toggle = 'vo', value = command == 'PlayBusyStagingPlatformsVO' and 1 or 0 })
    end
end
methods.IsStunned = function(self) local u = d(self) return u and u.stunned or false end
-- Silo missiles (`MissileInfo`): from the unit's details (selected units),
-- zeros otherwise.
methods.GetMissileInfo = function(self)
    local m = (d(self) or {}).missiles or {}
    return {
        nukeSiloBuildCount = m[1] or 0, nukeSiloMaxStorageCount = m[2] or 0, nukeSiloStorageCount = m[3] or 0,
        tacticalSiloBuildCount = m[4] or 0, tacticalSiloMaxStorageCount = m[5] or 0, tacticalSiloStorageCount = m[6] or 0,
    }
end
methods.IsOverchargePaused = function(self) local u = d(self) return u and u.overchargePaused or false end
methods.HasUnloadCommandQueuedUp = function(self)
    for _, c in (d(self) or {}).queue or {} do
        if c.type == 'TransportUnloadUnits' or c.type == 'TransportUnloadSpecificUnits' then return true end
    end
    return false
end
methods.GetFootPrintSize = function(self)
    local bp = self:GetBlueprint()
    local f = bp and bp.Footprint
    return f and math.max(f.SizeX or 1, f.SizeZ or 1) or 1
end

local meta = { __index = methods }

--- The UserUnit for entity id `id` (a string or number), if the interface
--- knows that unit.
function GetUnitById(id)
    local n = tonumber(id)
    -- Units the interface has no data for yet come from the latest frame.
    if n and (data[n] or __engine_find_unit(n)) then return __engine_unit(n) end
    return nil
end

--- The focus army's avatar units (its ACUs), or nil when it has none (FAF
--- reads `avatars and avatars[1]`).
function GetArmyAvatars()
    local ids = __engine_avatars()
    if table.getn(ids) == 0 then return nil end
    local units = {}
    for _, id in ids do
        local unit = GetUnitById(id)
        if unit then table.insert(units, unit) end
    end
    if table.getn(units) == 0 then return nil end
    return units
end

--- The units of `units` still alive.
function ValidateUnitsList(units)
    local out = {}
    for _, u in units or {} do
        if not u:IsDead() then table.insert(out, u) end
    end
    return out
end

--- The UserUnit for entity `id` (made on first use).
function __engine_unit(id)
    local o = objects[id]
    if not o then
        o = setmetatable({ _id = id }, meta)
        objects[id] = o
    end
    return o
end

--- The factory build queue the construction panel shows: its factory build
--- and upgrade orders stacked by blueprint ({id, count}, front first; the
--- sim groups them the same way, `Commands::build_groups`).
-- The factory whose queue the construction panel shows, and that queue's
-- last shape: when the engine's data changes it, FAF hears OnQueueChanged.
local queueFactory, queueShape
local function factoryQueue(id)
    local out = {}
    local u = id and data[id]
    for _, c in (u and u.queue) or {} do
        if (c.type == 'BuildFactory' or c.type == 'Upgrade') and c.blueprintId then
            local last = out[table.getn(out)]
            local n = math.max(c.count or 1, 1)
            if last and last.id == c.blueprintId then
                last.count = last.count + n
            else
                table.insert(out, { id = c.blueprintId, count = n })
            end
        end
    end
    return out
end
local function shapeOf(q)
    local parts = {}
    for _, e in q do table.insert(parts, e.id .. 'x' .. e.count) end
    return table.concat(parts, ',')
end
-- The engine reads `__engine_queue_factory` to send the shown factory's
-- queue when it isn't selected (a mobile factory's external factory:
-- FAF's construction.lua shows `selection[1]:GetCreator()`'s queue).
function SetCurrentFactoryForQueueDisplay(unit)
    queueFactory = unit and unit._id
    rawset(_G, '__engine_queue_factory', queueFactory)
    local q = factoryQueue(queueFactory)
    queueShape = shapeOf(q)
    return q
end
function ClearCurrentFactoryForQueueDisplay()
    queueFactory, queueShape = nil, nil
    rawset(_G, '__engine_queue_factory', nil)
end
-- The construction panel's queue edits: the shown factory's entry `index`
-- gets `count` more (its last build) or fewer (from its back; reaching the
-- build in progress cancels it). The panel shows the change with the next
-- unit data.
local function changeBuildCount(index, delta)
    local unit = queueFactory and GetUnitById(tostring(queueFactory))
    if not unit or not index or delta == 0 then return end
    table.insert(rawget(_G, '__engine_ui_orders'), { units = { unit }, toggle = 'count', index = index, value = delta })
end
function IncreaseBuildCountInQueue(index, count) changeBuildCount(index, math.floor(count or 1)) end
function DecreaseBuildCountInQueue(index, count) changeBuildCount(index, -math.floor(count or 1)) end
-- Called by the engine after new unit data (`UserVm::update_units`).
function __engine_check_queue()
    if not queueFactory then return end
    local q = factoryQueue(queueFactory)
    local shape = shapeOf(q)
    if shape ~= queueShape then
        queueShape = shape
        import('/lua/ui/game/gamemain.lua').OnQueueChanged(q)
    end
end

-- An order the player just gave, in its units' queues until the sim's
-- confirm it (moho's UserUnit queue has it at once; ours come a snapshot or
-- a lockstep turn later). FAF reads it straight from OnCommandIssued: an
-- enhancement order's IntegrateEnhancements counts the queue's Script
-- commands against its enhancement queue and drops the ones it can't find.
-- Called by the engine (`UserVm::command_issued`), which also drops a
-- prediction the sim never confirms.
function __engine_predict_command(ids, kind, clear)
    for _, id in ids do
        local u = data[id]
        if u then
            local q = {}
            if not clear then
                for _, c in u.queue or {} do table.insert(q, c) end
            end
            table.insert(q, { type = kind, ID = table.getn(q) + 1, x = u.x, y = u.y, z = u.z, predicted = true })
            u.queue = q
        end
    end
end

-- Toggles as the selected units have them (the first unit's state).
local function first(units) local u = units and units[1] return u and data[u._id] end
function GetFireState(units) local u = first(units) return u and u.fireState or 0 end
function GetScriptBit(units, bit)
    local u = first(units)
    if not (u and u.scriptBits) then return false end
    return math.mod(math.floor(u.scriptBits / (2 ^ bit)), 2) == 1
end
function GetIsPaused(units) local u = first(units) return u and u.paused or false end
function GetIsPausedOfUnit(unit) local u = unit and data[unit._id] return u and u.paused or false end
