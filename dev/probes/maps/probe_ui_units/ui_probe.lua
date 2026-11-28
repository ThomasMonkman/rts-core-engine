-- Interface half of probe_ui_units: every UserUnit method and the unit globals
-- on the sim half's named units, at tick 120 (building, guarding, patrolling)
-- and tick 225 (factory paused, tank moving), then the unit settings and
-- queue edits with the tick each shows up, and a plane killed in flight.
-- A list of the non-nil units given.
-- Shared helpers (kept in each ui_probe.lua: ours takes the file as one --ui-lua string).
-- Values under these keys are logged by type only (player names, local paths).
local MASK = { name = true, nickname = true, Nickname = true, authorizedCommandSources = false }
local function fv(v, depth)
    local t = type(v)
    if t == 'number' then return string.format('%.2f', v) end
    if t == 'string' then return '"' .. v .. '"' end
    if t == 'boolean' or t == 'nil' then return tostring(v) end
    if t == 'table' then
        local okb, bp = pcall(function() return v.GetBlueprint and v:GetBlueprint().BlueprintId end)
        if okb and bp then
            local okn, nm = pcall(function() return v:GetCustomName(v) end)
            if bp == 'uel0001' then nm = 'p_acu' end
            return '<unit ' .. tostring(bp) .. ' ' .. tostring(okn and nm) .. '>'
        end
        if depth <= 0 then return '{..}' end
        local keys = {}
        for k in pairs(v) do table.insert(keys, k) end
        table.sort(keys, function(a, b)
            if type(a) == type(b) and (type(a) == 'number' or type(a) == 'string') then return a < b end
            return tostring(type(a)) .. tostring(a) < tostring(type(b)) .. tostring(b)
        end)
        local s = '{'
        local n = 0
        for _, k in ipairs(keys) do
            n = n + 1
            if n > 40 then s = s .. ' ...' break end
            s = s .. ' ' .. tostring(k) .. '=' .. (MASK[k] and ('<' .. type(v[k]) .. '>') or fv(v[k], depth - 1))
        end
        return s .. ' }'
    end
    return '<' .. t .. '>'
end
local function G(name) return rawget(_G, name) end
-- moho's Lua calls a C function's type 'cfunction'.
local function isfn(f) return type(f) == 'function' or type(f) == 'cfunction' end
local STAGE = 'init'
-- P(label, f, a, b, c, d): calls f(a, b, c, d) protected, logs up to 4 results.
local function P(label, f, a, b, c, d)
    if not isfn(f) then
        LOG('PROBE ' .. STAGE .. ' ' .. label .. ' MISSING ' .. type(f))
        return nil
    end
    -- moho's functions check how many arguments they get: no trailing nils.
    local r
    if d ~= nil then r = { pcall(f, a, b, c, d) }
    elseif c ~= nil then r = { pcall(f, a, b, c) }
    elseif b ~= nil then r = { pcall(f, a, b) }
    elseif a ~= nil then r = { pcall(f, a) }
    else r = { pcall(f) } end
    if not r[1] then
        LOG('PROBE ' .. STAGE .. ' ' .. label .. ' ERR ' .. string.sub(tostring(r[2]), 1, 120))
        return nil
    end
    local last = 1
    for i = 2, 6 do if r[i] ~= nil then last = i end end
    local s = ''
    for i = 2, last do s = s .. ' ' .. fv(r[i], 3) end
    if last == 1 then s = ' (nothing)' end
    LOG('PROBE ' .. STAGE .. ' ' .. label .. ' =' .. s)
    return r[2], r[3], r[4]
end
local function waitTick(t)
    while GameTick() < t do WaitSeconds(0.1) end
end
local function named()
    local out = {}
    UISelectionByCategory('ALLUNITS', false, false, false, false)
    WaitSeconds(0.3)
    local sel = GetSelectedUnits() or {}
    for _, u in pairs(sel) do
        local ok, n = pcall(function() return u:GetCustomName(u) end)
        if ok and n then out[n] = u end
        if u:GetUnitId() == 'uel0001' then out.p_acu = u end
    end
    return out, sel
end
local function look(zoom)
    GetCamera('WorldCamera'):SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, zoom or 160)
end
local function L(a, b, c, d, e, f, g, h, i)
    local out = {}
    for _, x in pairs({ a = a, b = b, c = c, d = d, e = e, f = f, g = g, h = h, i = i }) do table.insert(out, x) end
    return out
end
local function names(list)
    if type(list) ~= 'table' then return fv(list, 1) end
    local out = {}
    for _, u in pairs(list) do
        local ok, n = pcall(function() return (u:GetUnitId() == 'uel0001' and 'p_acu') or u:GetCustomName(u) or ('?' .. u:GetBlueprint().BlueprintId) end)
        table.insert(out, ok and n or '?')
    end
    table.sort(out)
    return '[' .. table.getn(out) .. '] ' .. table.concat(out, ',')
end
local function N(label, f, a, b, c)
    if not isfn(f) then LOG('PROBE ' .. STAGE .. ' ' .. label .. ' MISSING') return end
    local r
    if c ~= nil then r = { pcall(f, a, b, c) } elseif b ~= nil then r = { pcall(f, a, b) } elseif a ~= nil then r = { pcall(f, a) } else r = { pcall(f) } end
    if not r[1] then LOG('PROBE ' .. STAGE .. ' ' .. label .. ' ERR ' .. string.sub(tostring(r[2]), 1, 120)) return end
    LOG('PROBE ' .. STAGE .. ' ' .. label .. ' = ' .. names(r[2]))
    return r[2]
end
local function dumpUnit(n, u)
    local function M(m, a, b)
        local f = u[m]
        P(n .. ':' .. m .. (a ~= nil and ('(' .. (type(a) == 'table' and 'self' or tostring(a)) .. (b ~= nil and (',' .. tostring(b)) or '') .. ')') or ''), f, u, a, b)
    end
    LOG('PROBE ' .. STAGE .. ' ' .. n .. ' type ' .. type(u) .. ' idtype ' .. type(u:GetEntityId()))
    M('GetUnitId') M('GetArmy')
    P(n .. ':GetBlueprint.BlueprintId', function() return u:GetBlueprint().BlueprintId end)
    P(n .. ':GetBlueprint==__blueprints', function() return u:GetBlueprint() == __blueprints[u:GetUnitId()] end)
    M('GetPosition') M('GetInterpolatedPosition') M('GetHealth') M('GetMaxHealth') M('GetFuelRatio') M('GetShieldRatio')
    M('GetWorkProgress') M('GetFractionComplete') M('GetBuildRate') M('GetFocus') M('GetGuardedEntity') M('GetCreator')
    M('GetCommandQueue') M('GetEconData') M('GetMissileInfo')
    M('GetStat', 'KILLS') M('GetStat', 'KILLS', 0) M('GetStat', 'NOPE', 7) M('GetStat', 'NOPE')
    if n ~= 'p_acu' then M('GetCustomName', u) end M('GetFootPrintSize') M('GetSelectionSets') M('HasSelectionSet', '1') M('HasUnloadCommandQueuedUp')
    M('IsAutoMode') M('IsAutoSurfaceMode') M('IsDead') M('IsIdle') M('IsInCategory', 'LAND') M('IsInCategory', 'STRUCTURE')
    M('IsInCategory', 'nonsense') M('IsOverchargePaused') M('IsRepeatQueue') M('IsStunned')
end
local ORDER = { 'p_tank', 'p_guard', 'p_eng', 'p_eng2', 'p_ideng', 'p_fac', 'p_fac2', 'p_shield', 'p_air', 'p_tml', 'p_radar', 'p_pgen', 'p_fab', 'p_acu', 'p_trans', 'p_cargo' }
local ids = {}
local echoTick = nil
local syncLog = 0
local syncCalls = 0
local oldOnSync = OnSync
OnSync = function()
    syncCalls = syncCalls + 1
    if Sync.ProbeUnits then ids = Sync.ProbeUnits end
    if Sync.ProbeEcho then
        LOG('PROBE sync ProbeEcho at GameTick ' .. GameTick() .. ' ' .. fv(Sync.ProbeEcho, 4))
    end
    if syncLog > 0 then
        syncLog = syncLog - 1
        LOG('PROBE order OnSync GameTick ' .. GameTick() .. ' Sync.ProbeTick ' .. tostring(Sync.ProbeTick))
    end
    oldOnSync()
end
-- moho doesn't call a replaced global OnSync (the hook above only runs in
-- ours), so the same through FAF's own sync callbacks.
local cbCalls = 0
pcall(function()
    import('/lua/UserSync.lua').AddOnSyncCallback(function(sync)
        cbCalls = cbCalls + 1
        if sync.ProbeUnits then for k, v in pairs(sync.ProbeUnits) do ids[k] = v end end
        if sync.ProbeEcho then
            LOG('PROBE synccb ProbeEcho at GameTick ' .. GameTick() .. ' ' .. fv(sync.ProbeEcho, 4))
        end
        if syncLog > 0 then
            LOG('PROBE order sync callback GameTick ' .. GameTick() .. ' Sync.ProbeTick ' .. tostring(sync.ProbeTick) .. ' GameTime ' .. string.format('%.2f', GameTime()))
        end
    end, 'probe_ui')
end)
local beatLog = 0
import('/lua/ui/game/gamemain.lua').AddBeatFunction(function()
    if beatLog > 0 then
        beatLog = beatLog - 1
        LOG('PROBE order OnBeat GameTick ' .. GameTick() .. ' GameTime ' .. string.format('%.2f', GameTime()))
    end
end)

local function stage(tag)
    STAGE = tag
    local u, sel = named()
    LOG('PROBE ' .. tag .. ' at GameTick>=' .. tag .. ' selected ' .. names(sel))
    for _, n in ipairs(ORDER) do
        if u[n] then dumpUnit(n, u[n]) else LOG('PROBE ' .. tag .. ' ' .. n .. ' NOT FOUND') end
    end
    P('ids from Sync', function() return ids.p_tank ~= nil, type(ids.p_tank), 'hook calls>0', syncCalls > 0, 'callback calls>0', cbCalls > 0, 'Sync.ProbeTick', type(Sync.ProbeTick), type(rawget(_G, 'Sync')) end)
    -- The enemy structure and the loaded cargo aren't in the selection: by id
    -- (from Sync, else the ids near the first enemy id / our own).
    local e = P('GetUnitById(str enemy)', G('GetUnitById'), tostring(ids.p_enemy))
    if not e then
        for k = 0, 6 do
            local c = GetUnitById(tostring(1048576 + k))
            if c and c:GetUnitId() == 'urb1101' then e = c ids.p_enemy = tostring(1048576 + k) end
        end
    end
    if e then dumpUnit('p_enemy', e) else LOG('PROBE ' .. tag .. ' p_enemy NOT FOUND') end
    if not u.p_cargo then
        local base = tonumber(u.p_tank:GetEntityId())
        for k = 1, 30 do
            local c = GetUnitById(tostring(base + k))
            if c and c:GetUnitId() == 'uel0106' and c:GetCustomName(c) == 'p_cargo' then u.p_cargo = c end
        end
        if u.p_cargo then LOG('PROBE ' .. tag .. ' p_cargo found by id, not selectable') dumpUnit('p_cargo', u.p_cargo) end
    end
    if not ids.p_air then ids.p_air = u.p_air and u.p_air:GetEntityId() end
    if not ids.p_tank then ids.p_tank = u.p_tank:GetEntityId() end
    P('GetUnitById(num)', G('GetUnitById'), ids.p_tank)
    P('GetUnitById(str)==sel', function() return GetUnitById(tostring(ids.p_tank)) == u.p_tank end)
    P('GetUnitById(missing)', G('GetUnitById'), '999999')
    N('GetIdleEngineers', G('GetIdleEngineers'))
    N('GetIdleFactories', G('GetIdleFactories'))
    N('GetAssistingUnitsList{eng}', G('GetAssistingUnitsList'), L(u.p_eng))
    N('GetAssistingUnitsList{tank}', G('GetAssistingUnitsList'), L(u.p_tank))
    N('GetAttachedUnitsList{trans}', G('GetAttachedUnitsList'), L(u.p_trans))
    N('GetValidAttackingUnits', G('GetValidAttackingUnits'))
    N('GetArmyAvatars', G('GetArmyAvatars'))
    N('FilterDown ENGINEER', G('EntityCategoryFilterDown'), categories.ENGINEER, sel)
    N('FilterDown STRUCTURE-FACTORY', G('EntityCategoryFilterDown'), categories.STRUCTURE - categories.FACTORY, sel)
    N('FilterOut STRUCTURE+LAND', G('EntityCategoryFilterOut'), categories.STRUCTURE + categories.LAND, sel)
    P('Contains LAND tank', G('EntityCategoryContains'), categories.LAND, u.p_tank)
    P('Contains AIR tank', G('EntityCategoryContains'), categories.AIR, u.p_tank)
    P('Contains LAND "uel0201"', G('EntityCategoryContains'), categories.LAND, 'uel0201')
    for _, n in ipairs({ 'p_tank', 'p_eng', 'p_fac', 'p_radar', 'p_tml', 'p_acu', 'p_air', 'p_trans' }) do
        P('GetUnitCommandData{' .. n .. '}', function()
            local orders, toggles, cats = GetUnitCommandData(L(u[n]))
            table.sort(orders) table.sort(toggles)
            return table.concat(orders, ','), table.concat(toggles, ','), type(cats)
        end)
        P('GetFireState{' .. n .. '}', GetFireState, L(u[n]))
        P('GetIsPaused{' .. n .. '}', GetIsPaused, L(u[n]))
        for bit = 0, 8 do P('GetScriptBit{' .. n .. '},' .. bit, GetScriptBit, L(u[n]), bit) end
    end
    P('GetUnitCommandData{}', function() local o, t, c = GetUnitCommandData({}) return o, t, type(c) end)
    P('GetFireState{tank,fac}', G('GetFireState'), L(u.p_tank, u.p_fac))
    for _, cap in ipairs({ 'RULEUCC_Move', 'RULEUCC_Stop', 'RULEUCC_Attack', 'RULEUCC_Guard', 'RULEUCC_Patrol', 'RULEUCC_RetaliateToggle', 'RULEUCC_Repair', 'RULEUCC_Capture', 'RULEUCC_Transport', 'RULEUCC_CallTransport', 'RULEUCC_Nuke', 'RULEUCC_Tactical', 'RULEUCC_Teleport', 'RULEUCC_Ferry', 'RULEUCC_SiloBuildTactical', 'RULEUCC_SiloBuildNuke', 'RULEUCC_Sacrifice', 'RULEUCC_Pause', 'RULEUCC_Overcharge', 'RULEUCC_Dive', 'RULEUCC_Reclaim', 'RULEUCC_SpecialAction', 'RULEUCC_Dock', 'RULEUCC_Script', 'RULEUCC_Invalid', 'bogus' }) do
        P('CommandFromCap ' .. cap, G('GetUnitCommandFromCommandCap'), cap)
    end
    P('GetRolloverInfo', G('GetRolloverInfo'))
    P('ValidateUnitsList', function() return names(ValidateUnitsList(L(u.p_tank, u.p_fac))) end)
    return u, sel
end

ForkThread(function()
    look(160)
    waitTick(120)
    local ua = stage('A')
    -- A plane killed at tick 420: watched from its own thread.
    ForkThread(function()
        waitTick(405)
        local air = ua.p_air
        local deadAt = nil
        local goneAt = nil
        while GameTick() < 470 do
            if not deadAt and air:IsDead() then deadAt = GameTick() end
            if not goneAt and GetUnitById(tostring(ids.p_air)) == nil then goneAt = GameTick() end
            WaitFrames(1)
        end
        LOG('PROBE D air IsDead first true at GameTick ' .. tostring(deadAt) .. ' GetUnitById nil at ' .. tostring(goneAt))
    end)
    waitTick(225)
    local u = stage('B')

    STAGE = 'C'
    -- Selection round trips.
    SelectUnits(L(u.p_tank, u.p_eng))
    P('GetSelectedUnits same frame', function() return names(GetSelectedUnits()) end)
    WaitSeconds(0.3)
    P('GetSelectedUnits later', function() return names(GetSelectedUnits()) end)
    SelectUnits(nil)
    WaitSeconds(0.3)
    P('GetSelectedUnits after SelectUnits(nil)', function() return GetSelectedUnits() end)
    SelectUnits({})
    WaitSeconds(0.3)
    P('GetSelectedUnits after SelectUnits({})', function() return GetSelectedUnits() end)

    -- Settings: the tick each change shows up in the interface, first with
    -- nothing selected, then (other units / values) with the units selected.
    local function settings(tag, tank, radar, eng, ideng, fac, tml, fire, newname)
        local t0 = GameTick()
        LOG('PROBE C ' .. tag .. ' settings issued at GameTick ' .. t0)
        P(tag .. ' SetFireState', SetFireState, L(tank), fire)
        P(tag .. ' ToggleScriptBit', ToggleScriptBit, L(radar), 3, false)
        P(tag .. ' SetPaused', SetPaused, L(eng), true)
        P(tag .. ' SetCustomName', function() return ideng:SetCustomName(newname) end)
        P(tag .. ' ProcessInfo SetRepeatQueue', function() return fac:ProcessInfo('SetRepeatQueue', 'true') end)
        P(tag .. ' ProcessInfo SetAutoMode', function() return tml:ProcessInfo('SetAutoMode', 'true') end)
        local seen = {}
        local checks = {
            fire = function() return GetFireState(L(tank)) == fire end,
            bit3 = function() return GetScriptBit(L(radar), 3) == true end,
            paused = function() return GetIsPaused(L(eng)) == true end,
            name = function() return ideng:GetCustomName(ideng) == newname end,
            ['repeat'] = function() return fac:IsRepeatQueue() == true end,
            auto = function() return tml:IsAutoMode() == true end,
        }
        for i = 1, 300 do
            for k, f in pairs(checks) do
                local ok, r = pcall(f)
                if ok and r and not seen[k] then seen[k] = GameTick() - t0 end
            end
            if GameTick() - t0 > 12 then break end
            WaitFrames(1)
        end
        for _, k in ipairs({ 'fire', 'bit3', 'paused', 'name', 'repeat', 'auto' }) do
            LOG('PROBE C ' .. tag .. ' setting ' .. k .. ' seen after ticks ' .. tostring(seen[k]))
        end
    end
    settings('unselected', u.p_tank, u.p_radar, u.p_eng, u.p_ideng, u.p_fac, u.p_tml, 2, 'p_ideng_r')
    SelectUnits(L(u.p_guard, u.p_fab, u.p_eng2, u.p_shield, u.p_fac2, u.p_tml, u.p_tank, u.p_radar, u.p_eng))
    WaitSeconds(0.5)
    P('selected GetFireState{tank}', G('GetFireState'), L(u.p_tank))
    settings('selected', u.p_guard, u.p_fab, u.p_eng2, u.p_shield, u.p_fac2, u.p_tml, 0, 'p_shield_r')
    P('GetFireState{tank}', G('GetFireState'), L(u.p_tank))
    P('GetScriptBit{radar},3', G('GetScriptBit'), L(u.p_radar), 3)
    P('GetIsPaused{eng}', G('GetIsPaused'), L(u.p_eng))
    P('eng:GetEconData', function() return u.p_eng:GetEconData() end)
    P('eng:GetWorkProgress', function() return u.p_eng:GetWorkProgress() end)
    P('eng:GetFocus', function() return u.p_eng:GetFocus() end)
    P('ToggleFireState', G('ToggleFireState'), L(u.p_guard), 0)
    WaitSeconds(1)
    P('GetFireState{guard} after ToggleFireState(0)', G('GetFireState'), L(u.p_guard))
    SelectUnits({})

    -- The factory's build queue and edits to it.
    STAGE = 'Q'
    P('fac:GetCommandQueue', function() return u.p_fac:GetCommandQueue() end)
    P('SetCurrentFactoryForQueueDisplay(fac)', G('SetCurrentFactoryForQueueDisplay'), u.p_fac)
    P('IncreaseBuildCountInQueue(1,2)', G('IncreaseBuildCountInQueue'), 1, 2)
    WaitSeconds(1)
    P('queue after increase', G('SetCurrentFactoryForQueueDisplay'), u.p_fac)
    P('DecreaseBuildCountInQueue(2,1)', G('DecreaseBuildCountInQueue'), 2, 1)
    WaitSeconds(1)
    P('queue after decrease', G('SetCurrentFactoryForQueueDisplay'), u.p_fac)
    P('ClearCurrentFactoryForQueueDisplay', G('ClearCurrentFactoryForQueueDisplay'))
    P('SetCurrentFactoryForQueueDisplay(fac2 idle)', G('SetCurrentFactoryForQueueDisplay'), u.p_fac2)
    P('SetCurrentFactoryForQueueDisplay(tank)', G('SetCurrentFactoryForQueueDisplay'), u.p_tank)
    P('ClearCurrentFactoryForQueueDisplay', G('ClearCurrentFactoryForQueueDisplay'))
    SelectUnits(L(u.p_fac2))
    WaitSeconds(0.3)
    P('IssueBlueprintCommand', G('IssueBlueprintCommand'), 'UNITCOMMAND_BuildFactory', 'uel0201', 2)
    WaitSeconds(1.5)
    P('fac2:GetCommandQueue', function() return u.p_fac2:GetCommandQueue() end)
    P('fac2 queue', G('SetCurrentFactoryForQueueDisplay'), u.p_fac2)
    P('fac2:IsIdle', function() return u.p_fac2:IsIdle() end)
    P('fac2:GetFocus', function() return u.p_fac2:GetFocus() end)
    P('ClearCurrentFactoryForQueueDisplay', G('ClearCurrentFactoryForQueueDisplay'))

    -- Select, order and reselect in one frame (FAF's hotbuild / selection
    -- helpers do this): which selection does the order go to?
    STAGE = 'X'
    SelectUnits(L(u.p_tank))
    WaitSeconds(0.3)
    SelectUnits(L(u.p_fac2))
    P('IssueBlueprintCommand same frame as SelectUnits', G('IssueBlueprintCommand'), 'UNITCOMMAND_BuildFactory', 'uel0106', 1)
    P('GetSelectedUnits same frame', function() return names(GetSelectedUnits()) end)
    SelectUnits(L(u.p_tank))
    WaitSeconds(1.5)
    P('fac2 queue after select+issue+reselect', function() local q = SetCurrentFactoryForQueueDisplay(u.p_fac2) ClearCurrentFactoryForQueueDisplay() return q end)
    P('tank queue', function() return u.p_tank:GetCommandQueue() end)
    P('GetSelectedUnits', function() return names(GetSelectedUnits()) end)
    SelectUnits(L(u.p_guard))
    P('GetFireState same frame as select', function() return GetFireState(GetSelectedUnits()) end)
    P('GetUnitCommandData same frame as select', function() local o = GetUnitCommandData(GetSelectedUnits()) table.sort(o) return table.concat(o, ',') end)
    SelectUnits({})

    -- SimCallback and the Sync echo; OnSync / OnBeat order.
    STAGE = 'S'
    SelectUnits(L(u.p_tank, u.p_guard))
    WaitSeconds(0.3)
    LOG('PROBE S SimCallback issued at GameTick ' .. GameTick())
    P('SimCallback with units', G('SimCallback'), { Func = 'SetStatByCallback', Args = { ProbeStat = true } }, true)
    local statAt = nil
    local s0 = GameTick()
    while GameTick() < s0 + 15 do
        if u.p_tank:GetStat('ProbeStat', 0).Value == 1 then statAt = GameTick() break end
        WaitFrames(1)
    end
    LOG('PROBE S ProbeStat seen at GameTick ' .. tostring(statAt))
    LOG('PROBE S SimCallback issued at GameTick ' .. GameTick())
    P('SimCallback no units', G('SimCallback'), { Func = 'EmptyCallback', Args = { tag = 'nounits' } })
    WaitSeconds(1)
    P('SimCallback no Args', G('SimCallback'), { Func = 'EmptyCallback' })
    WaitSeconds(1)
    syncLog = 4 beatLog = 4
    WaitSeconds(1)

    STAGE = 'D'
    waitTick(475)
    local air = u.p_air
    P('dead air:IsDead', function() return air:IsDead() end)
    P('GetSelectedUnits end', function() return names(GetSelectedUnits()) end)
    WaitSeconds(3)
    LOG('PROBE done')
end)
