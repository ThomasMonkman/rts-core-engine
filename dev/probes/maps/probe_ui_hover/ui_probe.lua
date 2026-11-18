-- Interface half of probe_ui_hover: what the interface reports for the unit
-- under the mouse. The mouse can't be moved from Lua, so the camera is moved
-- until each named unit sits under it: GetRolloverInfo, the world view's
-- right-click order for a few selections, then user-layer threads, the build
-- categories of GetUnitCommandData, orders given from the interface, and
-- (last) methods on a dead unit.
--   ours: --cursor 1000,700 puts the cursor inside the window, but the interface only learns
--   the mouse position from mouse moves, so also set PROBE_CURSOR = { x, y } (interface pixels)
--   ahead of this file: rawset(_G, 'PROBE_CURSOR', { 500, 350 })
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
local function sortedHead(list, n)
    if type(list) ~= 'table' then return fv(list, 1) end
    local out = {}
    for _, v in pairs(list) do table.insert(out, string.lower(tostring(v))) end
    table.sort(out)
    local s = '[' .. table.getn(out) .. ']'
    for i = 1, math.min(n, table.getn(out)) do s = s .. ' ' .. out[i] end
    return s
end
ForkThread(function()
    look(120)
    waitTick(100)
    local cam = GetCamera('WorldCamera')
    local view = import('/lua/ui/game/worldview.lua').viewLeft
    local u = named()
    SelectUnits({})
    local airRef = u.p_air
    STAGE = 'hover'
    P('mouse in view', function() local m = GetMouseScreenPos() return m[1] >= view.Left() and m[1] < view.Right() and m[2] >= view.Top() and m[2] < view.Bottom() end)
    local fixed = rawget(_G, 'PROBE_CURSOR')
    local function mouseWorld()
        if fixed then return UnProject(view, { fixed[1] - view.Left(), fixed[2] - view.Top() }) end
        return GetMouseWorldPos()
    end
    -- Moves the camera until `pos` is under the mouse; returns how far off it ended.
    local function hover(pos, zoom)
        local fx, fz = 405, 610
        for i = 1, 3 do
            cam:SnapTo(Vector(fx, 18.5, fz), { math.pi, 1.3, 0 }, zoom or 80)
            WaitSeconds(0.25)
            local mw = mouseWorld()
            fx, fz = fx + (pos[1] - mw[1]), fz + (pos[3] - mw[3])
        end
        cam:SnapTo(Vector(fx, 18.5, fz), { math.pi, 1.3, 0 }, zoom or 80)
        WaitSeconds(0.5)
        local mw = mouseWorld()
        return VDist2(mw[1], mw[3], pos[1], pos[3])
    end
    local function rollover(name, pos)
        local off = hover(pos)
        LOG('PROBE hover ' .. name .. ' mouse off by under 0.5 ' .. tostring(off < 0.5) .. ' tick ' .. GameTick())
        P(name .. ' GetRolloverInfo', G('GetRolloverInfo'))
        P(name .. ' highlight', function() return view:HasHighlightCommand(), GetHighlightCommand() end)
    end
    local targets = { 'p_tank', 'p_fac', 'p_eng', 'p_tml', 'p_shield', 'p_pgen', 'p_fab', 'p_radar', 'p_acu', 'p_trans', 'p_fac2', 'p_ideng' }
    for _, n in ipairs(targets) do
        if u[n] then rollover(n, u[n]:GetPosition()) else LOG('PROBE hover ' .. n .. ' NOT FOUND') end
    end
    rollover('site ueb0101', { 380.5, 18.5, 606.5 })
    rollover('enemy urb1101', { 394.5, 18.5, 584.5 })
    rollover('ground', { 400, 18.5, 632 })

    -- The right-click order for a selection over a target.
    STAGE = 'rmb'
    local function rmb(label, sel, pos)
        SelectUnits(sel)
        hover(pos)
        P(label, function() return view:GetRightMouseButtonOrder() end)
    end
    local enemy, ground, site = { 394.5, 18.5, 584.5 }, { 400, 18.5, 632 }, { 380.5, 18.5, 606.5 }
    rmb('nothing over enemy', {}, enemy)
    rmb('tank over enemy', L(u.p_tank), enemy)
    rmb('tank over ground', L(u.p_tank), ground)
    rmb('tank over own factory', L(u.p_tank), u.p_fac:GetPosition())
    rmb('tank over transport', L(u.p_tank), u.p_trans:GetPosition())
    rmb('transport over shield unit', L(u.p_trans), u.p_shield:GetPosition())
    rmb('engineer over enemy', L(u.p_ideng), enemy)
    rmb('engineer over site', L(u.p_ideng), site)
    rmb('engineer over damaged tank', L(u.p_ideng), u.p_tank:GetPosition())
    rmb('engineer over own factory', L(u.p_ideng), u.p_fac:GetPosition())
    rmb('engineer+tank over enemy', L(u.p_ideng, u.p_tank), enemy)
    rmb('tank+engineer over site', L(u.p_tank, u.p_ideng), site)
    rmb('factory over ground', L(u.p_fac2), ground)
    rmb('factory over tank', L(u.p_fac2), u.p_tank:GetPosition())
    rmb('radar over ground', L(u.p_radar), ground)
    rmb('tml over enemy', L(u.p_tml), enemy)
    rmb('shield over tank', L(u.p_shield), u.p_tank:GetPosition())
    SelectUnits({})
    look(120)

    STAGE = 'build'
    for _, n in ipairs({ 'p_eng', 'p_fac', 'p_acu', 'p_tank', 'p_radar', 'p_tml' }) do
        P('buildable ' .. n, function()
            local orders, toggles, cats = GetUnitCommandData(L(u[n]))
            return sortedHead(EntityCategoryGetUnitList(cats), 60)
        end)
    end
    P('buildable eng+fac', function() local o, t, cats = GetUnitCommandData(L(u.p_eng, u.p_fac)) table.sort(o) return sortedHead(EntityCategoryGetUnitList(cats), 3), table.concat(o, ',') end)
    P('buildable eng+tank', function() local o, t, cats = GetUnitCommandData(L(u.p_eng, u.p_tank)) table.sort(o) return sortedHead(EntityCategoryGetUnitList(cats), 3), table.concat(o, ',') end)

    -- Orders from the interface.
    STAGE = 'orders'
    P('tank before Stop', function() return u.p_tank:IsIdle(), table.getn(u.p_tank:GetCommandQueue()) end)
    P('IssueUnitCommand Stop', G('IssueUnitCommand'), L(u.p_tank), 'Stop')
    SelectUnits(L(u.p_radar))
    WaitSeconds(0.3)
    P('IssueBlueprintCommand Upgrade', G('IssueBlueprintCommand'), 'UNITCOMMAND_Upgrade', 'ueb3201', 1, false)
    WaitSeconds(2)
    P('tank after Stop', function() return u.p_tank:IsIdle(), table.getn(u.p_tank:GetCommandQueue()) end)
    P('radar upgrading', function() return u.p_radar:GetFocus(), u.p_radar:GetWorkProgress() > 0, u.p_radar:IsIdle(), u.p_radar:GetCommandQueue() end)
    P('radar queue display', function() local q = SetCurrentFactoryForQueueDisplay(u.p_radar) ClearCurrentFactoryForQueueDisplay() return q end)
    P('radar GetUnitCommandData', function() local o, t = GetUnitCommandData(L(u.p_radar)) table.sort(o) table.sort(t) return table.concat(o, ','), table.concat(t, ',') end)
    rollover('radar upgrading', u.p_radar:GetPosition())
    SelectUnits(L(u.p_guard))
    WaitSeconds(0.3)
    P('guard before', function() return u.p_guard:IsIdle(), u.p_guard:GetGuardedEntity(), table.getn(u.p_guard:GetCommandQueue()) end)
    P('IssueCommand Stop', G('IssueCommand'), 'Stop')
    WaitSeconds(1.5)
    P('guard after IssueCommand Stop', function() return u.p_guard:IsIdle(), u.p_guard:GetGuardedEntity(), table.getn(u.p_guard:GetCommandQueue()) end)
    SelectUnits(L(u.p_eng2))
    WaitSeconds(0.3)
    P('IssueCommand Stop with clear=false', G('IssueCommand'), 'Stop', nil, false)
    WaitSeconds(1.5)
    P('eng2 after', function() return u.p_eng2:IsIdle(), u.p_eng2:GetGuardedEntity() end)
    SelectUnits({})

    -- User-layer threads.
    STAGE = 'thread'
    do
        local got = ''
        local th = ForkThread(function(a, b) got = tostring(a) .. ',' .. tostring(b) .. ',' .. type(CurrentThread()) end, 1, 'two')
        P('ForkThread', function() return type(th), got end)
        WaitSeconds(0.2)
        P('ForkThread ran', function() return got end)
        local order = ''
        ForkThread(function() for i = 1, 3 do order = order .. 'A' WaitFrames(1) end end)
        ForkThread(function() for i = 1, 3 do order = order .. 'B' WaitFrames(1) end end)
        P('fork order same frame', function() return order end)
        WaitSeconds(0.5)
        P('fork order', function() return order end)
        local killed = 'not resumed'
        local k = ForkThread(function() WaitSeconds(0.3) killed = 'resumed' end)
        KillThread(k)
        WaitSeconds(0.6)
        P('KillThread', function() return killed end)
        P('KillThread twice', G('KillThread'), k)
        local after = 'no'
        ForkThread(function() error('PROBE thread deliberate error') end)
        ForkThread(function() WaitSeconds(0.1) after = 'yes' end)
        WaitSeconds(0.4)
        P('other threads run after one errors', function() return after end)
        local s = 'start'
        local sus = ForkThread(function() s = 'suspended' SuspendCurrentThread() s = 'resumed' end)
        WaitSeconds(0.3)
        local a = s
        ResumeThread(sus)
        WaitSeconds(0.3)
        P('Suspend/Resume', function() return a, s end)
        local c0, t0 = CurrentTime(), GameTick()
        for i = 1, 20 do WaitSeconds(0.05) end
        LOG('PROBE thread 20 x WaitSeconds(0.05) took ' .. string.format('%.1f', CurrentTime() - c0) .. ' s')
        c0 = CurrentTime()
        for i = 1, 30 do WaitFrames(1) end
        local dt = CurrentTime() - c0
        LOG('PROBE thread 30 x WaitFrames(1) took ' .. (dt < 1.5 and 'under 1.5 s' or string.format('%.1f s', dt)))
        c0 = CurrentTime()
        WaitFrames(10)
        dt = CurrentTime() - c0
        LOG('PROBE thread WaitFrames(10) took ' .. (dt < 0.5 and 'under 0.5 s' or string.format('%.1f s', dt)))
        c0 = CurrentTime()
        WaitSeconds(0.001)
        LOG('PROBE thread WaitSeconds(0.001) took under 0.1 s ' .. tostring(CurrentTime() - c0 < 0.1) .. ' over 0 ' .. tostring(CurrentTime() - c0 > 0))
        -- stops the real game's log here
        -- P('WaitSeconds outside thread', function() local co = coroutine.create(function() WaitSeconds(0.1) return 'x' end) return coroutine.resume(co) end)
        P('coroutine.running', function() return type(coroutine.running), type(coroutine.running and coroutine.running()) end)
    end

    -- Last: a dead unit (the plane dies at tick 420).
    waitTick(455)
    WaitSeconds(3)
    STAGE = 'risky'
    for _, m in ipairs({ 'IsDead', 'GetEntityId', 'GetUnitId', 'GetArmy', 'GetBlueprint', 'GetPosition', 'GetHealth', 'GetMaxHealth', 'GetFuelRatio', 'GetWorkProgress', 'GetCommandQueue', 'GetEconData', 'IsIdle', 'GetFocus', 'IsInCategory' }) do
        LOG('PROBE risky before dead ' .. m) WaitSeconds(0.4)
        P('dead ' .. m, function() local r if m == 'IsInCategory' then r = airRef[m](airRef, 'AIR') else r = airRef[m](airRef) end if m == 'GetBlueprint' then return type(r) end return r end)
    end
    LOG('PROBE risky before SelectUnits dead') WaitSeconds(0.4)
    P('SelectUnits dead', function() SelectUnits({ airRef }) return 'ok' end)
    WaitSeconds(0.3)
    P('selection after', function() return GetSelectedUnits() end)
    LOG('PROBE risky before IssueDockCommand') WaitSeconds(0.4)
    P('IssueDockCommand no selection', G('IssueDockCommand'), true)
    WaitSeconds(3)
    LOG('PROBE done')
end)
