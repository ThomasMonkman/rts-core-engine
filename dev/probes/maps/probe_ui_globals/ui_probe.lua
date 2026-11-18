-- Interface half of probe_ui_globals: the user layer's session, time, economy,
-- army, preference, file, path, maths and string globals, and what its Lua
-- library does with numbers and strings. Starts at tick 100.
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
local function T(label, f, a, b, c)   -- type of each result only
    if not isfn(f) then return P(label, f) end
    return P(label, function(a, b, c) local r if c ~= nil then r = { f(a, b, c) } elseif b ~= nil then r = { f(a, b) } elseif a ~= nil then r = { f(a) } else r = { f() } end local s = {} for i = 1, 4 do s[i] = type(r[i]) end return table.concat(s, ',') end, a, b, c)
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
    look(160)
    waitTick(100)
    STAGE = 'session'
    P('GetFocusArmy', G('GetFocusArmy')) P('IsObserver', G('IsObserver')) P('SessionIsActive', G('SessionIsActive')) P('SessionIsPaused', G('SessionIsPaused'))
    P('SessionIsReplay', G('SessionIsReplay')) P('SessionIsMultiplayer', G('SessionIsMultiplayer')) P('SessionIsGameOver', G('SessionIsGameOver'))
    P('SessionIsObservingAllowed', G('SessionIsObservingAllowed')) P('SessionGetLocalCommandSource', G('SessionGetLocalCommandSource'))
    T('SessionGetCommandSourceNames', G('SessionGetCommandSourceNames'))
    P('GetSessionClients', G('GetSessionClients'))
    P('SessionGetScenarioInfo', function()
        local t = SessionGetScenarioInfo()
        local s = {}
        for k, v in pairs(t) do
            if type(v) == 'table' or k == 'name' or k == 'description' then s[k] = '<' .. type(v) .. '>' else s[k] = v end
        end
        return s
    end)
    P('ScenarioInfo.Options keys', function()
        local o = SessionGetScenarioInfo().Options
        if not o then return nil end
        local s = {}
        for k, v in pairs(o) do if type(v) == 'table' then s[k] = '<table>' else s[k] = v end end
        return s
    end)
    P('GetCurrentUIState', G('GetCurrentUIState')) T('GetVersion', G('GetVersion'))
    P('GetArmiesTable', function()
        local t = GetArmiesTable()
        local a1 = t.armiesTable and t.armiesTable[1]
        local n = 0
        for _ in pairs(t.armiesTable or {}) do n = n + 1 end
        local top = {}
        for k, v in pairs(t) do if k ~= 'armiesTable' then top[k] = v end end
        return top, n, a1
    end)
    P('GetArmiesTable army 2', function() return GetArmiesTable().armiesTable[2] end)
    P('GetArmyScore(1)', G('GetArmyScore'), 1)
    P('GetResourceSharing', G('GetResourceSharing'))
    P('IsAlly(1,2)', G('IsAlly'), 1, 2) P('IsEnemy(1,2)', G('IsEnemy'), 1, 2) P('IsNeutral(1,2)', G('IsNeutral'), 1, 2) P('IsAlly(1,1)', G('IsAlly'), 1, 1)
    P('IsEnemy(1,1)', G('IsEnemy'), 1, 1) P('IsAlly(1,99)', G('IsAlly'), 1, 99)
    P('GetEconomyTotals', G('GetEconomyTotals'))
    WaitSeconds(0.5)
    P('GetEconomyTotals keys', function()
        local t = GetEconomyTotals()
        local s = {}
        for k, v in pairs(t) do s[k] = type(v) == 'table' and (function() local ks = {} for kk in pairs(v) do table.insert(ks, kk) end table.sort(ks) return table.concat(ks, ',') end)() or type(v) end
        return s
    end)

    STAGE = 'time'
    P('GetSimTicksPerSecond', G('GetSimTicksPerSecond')) P('SecondsPerTick', G('SecondsPerTick')) P('GetGameSpeed', G('GetGameSpeed')) P('GetSimRate', G('GetSimRate'))
    P('GameTime-GameTick/10', function() return GameTime() - GameTick() / 10 end)
    P('GetGameTimeSeconds-GameTick/10', function() return GetGameTimeSeconds() - GameTick() / 10 end)
    P('GetGameTime', function() local s = GetGameTime() return type(s), string.len(s), string.gsub(s, '%d', '9') end)
    T('CurrentTime', G('CurrentTime')) T('GetSystemTimeSecondsOnlyForProfileUse', G('GetSystemTimeSecondsOnlyForProfileUse')) T('GetSystemTime', G('GetSystemTime'))
    P('GameTick is whole', function() return GameTick() == math.floor(GameTick()) end)
    do
        local sys = G('GetSystemTimeSecondsOnlyForProfileUse') or function() return 0 end
        local c0, s0, t0 = CurrentTime(), sys(), GameTick()
        WaitSeconds(1)
        LOG('PROBE time WaitSeconds(1): CurrentTime +' .. string.format('%.1f', CurrentTime() - c0) .. ' system +' .. string.format('%.1f', sys() - s0) .. ' ticks +' .. (GameTick() - t0))
        local frames = 0
        c0, t0 = CurrentTime(), GameTick()
        for i = 1, 30 do WaitFrames(1) frames = frames + 1 end
        local dt = CurrentTime() - c0
        LOG('PROBE time WaitFrames(1) x30: seconds ' .. (dt < 2 and 'under 2' or 'over 2') .. ' ticks>0 ' .. tostring(GameTick() - t0 > 0))
        c0 = CurrentTime()
        WaitSeconds(0)
        LOG('PROBE time WaitSeconds(0) took under 0.2 s ' .. tostring(CurrentTime() - c0 < 0.2))
        c0 = CurrentTime()
        P('WaitTicks in user layer', function() WaitTicks(5) return CurrentTime() - c0 > 0.3 end)
    end
    -- Pause: what stops.
    do
        P('SessionRequestPause', G('SessionRequestPause'))
        P('SessionIsPaused same frame', G('SessionIsPaused'))
        WaitSeconds(0.5)
        P('SessionIsPaused after 0.5 s', G('SessionIsPaused'))
        local c0, t0, g0 = CurrentTime(), GameTick(), GameTime()
        WaitSeconds(1)
        LOG('PROBE time paused 1 s: CurrentTime +' .. string.format('%.1f', CurrentTime() - c0) .. ' ticks +' .. (GameTick() - t0) .. ' GameTime +' .. string.format('%.2f', GameTime() - g0))
        P('SessionResume', G('SessionResume'))
        P('SessionIsPaused same frame after resume', G('SessionIsPaused'))
        WaitSeconds(0.5)
        P('SessionIsPaused after resume', G('SessionIsPaused'))
    end
    -- Game speed.
    do
        P('SetGameSpeed(4)', G('SetGameSpeed'), 4)
        WaitSeconds(0.5)
        P('GetGameSpeed', G('GetGameSpeed')) P('GetSimRate', G('GetSimRate'))
        local t0 = GameTick()
        WaitSeconds(2)
        local d = GameTick() - t0
        LOG('PROBE time speed 4: ticks in 2 s ' .. (d < 23 and 'about 20' or (d < 33 and 'about 28' or (d < 50 and 'about 40' or 'over 50'))) )
        P('SetGameSpeed(0)', G('SetGameSpeed'), 0)
        WaitSeconds(0.5)
        P('GetGameSpeed', G('GetGameSpeed'))
    end

    STAGE = 'prefs'
    P('GetPreference missing', G('GetPreference'), 'probe_ui_missing')
    P('GetPreference missing default', G('GetPreference'), 'probe_ui_missing', 5)
    P('GetPreference missing nested default', G('GetPreference'), 'probe_ui_missing.a.b', 'd')
    P('SetPreference nested', G('SetPreference'), 'probe_ui.a.b', { 1, 2, { x = 1 }, s = 'str', f = 1.5, t = true })
    P('GetPreference probe_ui', G('GetPreference'), 'probe_ui')
    P('GetPreference probe_ui.a.b.3.x', G('GetPreference'), 'probe_ui.a.b.3.x')
    P('GetPreference probe_ui.a.b.1', G('GetPreference'), 'probe_ui.a.b.1')
    P('GetPreference probe_ui.a.b.s', G('GetPreference'), 'probe_ui.a.b.s')
    P('GetPreference probe_ui.a.b.t', G('GetPreference'), 'probe_ui.a.b.t')
    P('GetPreference probe_ui.a.nope', G('GetPreference'), 'probe_ui.a.nope', 'dflt')
    P('GetPreference same table twice', function() return GetPreference('probe_ui') == GetPreference('probe_ui') end)
    P('GetPreference result is a copy', function() local t = GetPreference('probe_ui') t.a.b.s = 'changed' return GetPreference('probe_ui.a.b.s') end)
    P('SetPreference stores a copy', function() local t = { v = 1 } SetPreference('probe_ui.c', t) t.v = 2 return GetPreference('probe_ui.c.v') end)
    P('SetPreference digits', G('SetPreference'), 'probe_ui.d', { 'first', 'second', ['1'] = 'strkey' })
    P('GetPreference probe_ui.d', G('GetPreference'), 'probe_ui.d')
    P('GetPreference probe_ui.d.1', G('GetPreference'), 'probe_ui.d.1')
    P('GetPreference probe_ui.d.2', G('GetPreference'), 'probe_ui.d.2')
    P('SetPreference strdigit only', G('SetPreference'), 'probe_ui.e', { ['7'] = 'seven', x = 1 })
    P('GetPreference probe_ui.e.7', G('GetPreference'), 'probe_ui.e.7')
    P('GetPreference probe_ui.e', G('GetPreference'), 'probe_ui.e')
    P('SetPreference path with number', G('SetPreference'), 'probe_ui.g.3', 'three')
    P('GetPreference probe_ui.g', G('GetPreference'), 'probe_ui.g')
    P('SetPreference number value', G('SetPreference'), 'probe_ui.h', 7)
    P('GetPreference probe_ui.h', G('GetPreference'), 'probe_ui.h')
    P('SetPreference over scalar', G('SetPreference'), 'probe_ui.h.sub', 8)
    P('GetPreference probe_ui.h after', G('GetPreference'), 'probe_ui.h')
    P('SetPreference nil', G('SetPreference'), 'probe_ui', nil)
    P('GetPreference after nil', G('GetPreference'), 'probe_ui')
    P('GetPreference empty key', G('GetPreference'), '')
    P('GetPreference profile.current type', function() return type(GetPreference('profile.current')) end)
    P('GetOptions', function() return type(GetOptions) end)
    P('GetOptions("fidelity")', function() return type(GetOptions('fidelity')) end)
    P('HasCommandLineArg /nope', G('HasCommandLineArg'), '/nope_probe')
    P('GetCommandLineArg /nope', G('GetCommandLineArg'), '/nope_probe', 1)
    P('HasCommandLineArg /map', G('HasCommandLineArg'), '/map')
    P('GetCommandLineArg /map', G('GetCommandLineArg'), '/map', 1)
    P('GetCommandLineArg /map 0', G('GetCommandLineArg'), '/map', 0)
    P('GetFrontEndData missing', G('GetFrontEndData'), 'probe_missing')
    P('SetFrontEndData', G('SetFrontEndData'), 'probe_k', { a = 1 })
    P('GetFrontEndData', G('GetFrontEndData'), 'probe_k')

    STAGE = 'disk'
    P('DiskFindFiles /lua/ui/game *.lua', function() return sortedHead(DiskFindFiles('/lua/ui/game', '*.lua'), 3) end)
    P('DiskFindFiles /lua/ui/game/ g*.lua', function() return sortedHead(DiskFindFiles('/lua/ui/game/', 'g*.lua'), 6) end)
    P('DiskFindFiles /lua/ui/game gamemain.lua', function() return sortedHead(DiskFindFiles('/lua/ui/game', 'gamemain.lua'), 6) end)
    P('DiskFindFiles /lua/maui *', function() return sortedHead(DiskFindFiles('/lua/maui', '*'), 3) end)
    P('DiskFindFiles /lua/ui/game/layouts recursion', function() local n = 0 for _, f in pairs(DiskFindFiles('/lua/ui', 'gamemain.lua')) do n = n + 1 end return n end)
    P('DiskFindFiles missing dir', function() return sortedHead(DiskFindFiles('/nope_probe', '*'), 3) end)
    P('DiskFindFiles case', function() return DiskFindFiles('/lua/ui/game', 'gamemain.lua')[1] end)
    P('DiskFindFiles units *_unit.bp case', function() local l = DiskFindFiles('/units/uel0001', '*_unit.bp') return l[1] end)
    P('DiskGetFileInfo file', function()
        local t = DiskGetFileInfo('/lua/ui/game/gamemain.lua')
        if type(t) ~= 'table' then return t end
        local s = {}
        for k, v in pairs(t) do s[k] = type(v) == 'boolean' and v or type(v) end
        return s
    end)
    P('DiskGetFileInfo folder', function()
        local t = DiskGetFileInfo('/lua/ui/game')
        if type(t) ~= 'table' then return t end
        local s = {}
        for k, v in pairs(t) do s[k] = type(v) == 'boolean' and v or type(v) end
        return s
    end)
    P('DiskGetFileInfo missing', G('DiskGetFileInfo'), '/nope_probe.lua')
    P('exists file', G('exists'), '/lua/ui/game/gamemain.lua') P('exists missing', G('exists'), '/nope_probe.lua') P('exists folder', G('exists'), '/lua/ui/game')
    P('exists upper', G('exists'), '/LUA/UI/GAME/GAMEMAIN.LUA')
    for _, p in ipairs({ '/a/b/c.lua', '/a/b/', '/a/b', 'c.lua', '', '/a/b.c/d', '/A/B/C.Lua.bak', 'a\\b\\c.lua' }) do
        P('Basename ' .. p, G('Basename'), p) P('Basename strip ' .. p, G('Basename'), p, true) P('Dirname ' .. p, G('Dirname'), p)
    end
    for _, p in ipairs({ '/a/b/../c.lua', '/a/./b//c', '/a/b/../../../c', 'a\\b\\..\\c', '/A/B/c', '../x', '/a/b/..' }) do P('FileCollapsePath ' .. p, G('FileCollapsePath'), p) end
    T('DiskToLocal', G('DiskToLocal'), '/lua/ui/game/gamemain.lua')
    P('GetTextureDimensions econ', G('GetTextureDimensions'), '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds')
    P('GetTextureDimensions border', G('GetTextureDimensions'), '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds', 1)
    P('GetTextureDimensions icon', G('GetTextureDimensions'), '/textures/ui/common/icons/units/uel0001_icon.dds')

    STAGE = 'maths'
    for _, n in ipairs({ 2.5, -2.5, 3.5, 0.5, -0.5, 1.49, 1e10 }) do P('MATH_IRound ' .. n, G('MATH_IRound'), n) end
    P('MATH_Lerp', G('MATH_Lerp'), 0.25, 10, 20) P('MATH_Lerp over', G('MATH_Lerp'), 2, 10, 20)
    P('Vector', function() local v = Vector(1, 2, 3) return type(v), v.x, v[1], v, getmetatable(v) ~= nil end)
    P('Vector2', function() local v = Vector2(1, 2) return type(v), v.x, v[1], v end)
    P('Rect', function() local r = Rect(1, 2, 3, 4) return type(r), r.x0, r.y1, r end)
    P('VAdd', G('VAdd'), Vector(1, 2, 3), Vector(1, 1, 1)) P('VDiff', G('VDiff'), Vector(1, 2, 3), Vector(1, 1, 1)) P('VMult', G('VMult'), Vector(1, 2, 3), 2)
    P('VDot', G('VDot'), Vector(1, 2, 3), Vector(1, 1, 1)) P('VDist3', G('VDist3'), Vector(0, 0, 0), Vector(3, 4, 0)) P('VDist2', G('VDist2'), 0, 0, 3, 4)
    P('VDist3Sq', G('VDist3Sq'), Vector(0, 0, 0), Vector(3, 4, 0)) P('VDist2Sq', G('VDist2Sq'), 0, 0, 3, 4) P('VPerpDot', G('VPerpDot'), Vector(1, 0, 0), Vector(0, 0, 1))
    P('VAdd plain tables', G('VAdd'), { 1, 2, 3 }, { 1, 1, 1 })
    P('EulerToQuaternion', G('EulerToQuaternion'), 0.1, 0.2, 0.3) P('OrientFromDir', G('OrientFromDir'), Vector(1, 0, 1))
    P('MinLerp', G('MinLerp'), 0.5, 0, 10) P('MinSlerp', G('MinSlerp'), 0.5, 0, 3)
    P('Random range', function() for i = 1, 50 do local r = Random(1, 3) if r < 1 or r > 3 or r ~= math.floor(r) then return 'bad ' .. r end end return 'ok' end)
    P('Random() range', function() for i = 1, 50 do local r = Random() if r < 0 or r >= 1 then return 'bad ' .. r end end return 'ok' end)
    P('STR_GetTokens', G('STR_GetTokens'), 'a,b,,c', ',') P('STR_GetTokens multi', G('STR_GetTokens'), 'a, b;c', ', ;')
    P('STR_Utf8Len', G('STR_Utf8Len'), 'h\195\169llo') P('STR_Utf8SubString', G('STR_Utf8SubString'), 'h\195\169llo', 2, 2)
    P('STR_itox', G('STR_itox'), 255) P('STR_xtoi', G('STR_xtoi'), 'ff') P('STR_xtoi 0x', G('STR_xtoi'), '0xFF') P('STR_itox neg', G('STR_itox'), -1)
    P('LOC known', G('LOC'), '<LOC _OK>OKfallback') P('LOC unknown', G('LOC'), '<LOC probe_nope_0001>Fallback') P('LOC plain', G('LOC'), 'plain') P('LOC nil', G('LOC'), nil)
    P('LOCF', G('LOCF'), '<LOC probe_nope_0002>%d of %s', 3, 'x')
    P('EntityCategoryGetUnitList COMMAND', function() return sortedHead(EntityCategoryGetUnitList(categories.COMMAND), 8) end)
    P('EntityCategoryGetUnitList count', function() return table.getn(EntityCategoryGetUnitList(categories.TECH1 * categories.LAND * categories.UEF * categories.MOBILE)) end)
    P('ParseEntityCategory', function() return sortedHead(EntityCategoryGetUnitList(ParseEntityCategory('COMMAND UEF')), 4) end)
    P('ParseEntityCategory lower', function() return sortedHead(EntityCategoryGetUnitList(ParseEntityCategory('uel0201')), 4) end)
    P('EntityCategoryEmpty', G('EntityCategoryEmpty'), categories.COMMAND * categories.STRUCTURE)
    P('categories type', function() return type(categories.COMMAND), type(categories.uel0001), type(categories.NOPE_PROBE), tostring(categories.COMMAND) == tostring(categories.COMMAND) end)
    P('category ops', function() return type(categories.LAND + categories.AIR), type(categories.LAND - categories.AIR), type(categories.LAND * categories.AIR), categories.LAND == categories.LAND end)

    STAGE = 'input'
    P('GetNumRootFrames', G('GetNumRootFrames'))
    P('GetFrame(0)', function() local f = GetFrame(0) return type(f), f.Width() > 0, f.Left(), f.Top(), f.Depth() end)
    P('GetFrame(0) same', function() return GetFrame(0) == GetFrame(0) end)
    P('GetFrame(1)', function() return type(GetFrame(1)) end)
    P('IsKeyDown Shift', G('IsKeyDown'), 'Shift') P('IsKeyDown 16', G('IsKeyDown'), 16) P('IsKeyDown A', G('IsKeyDown'), 'A') P('IsKeyDown 65', G('IsKeyDown'), 65)
    P('IsKeyDown SHIFT', G('IsKeyDown'), 'SHIFT') P('IsKeyDown Control', G('IsKeyDown'), 'Control') P('IsKeyDown bogus', G('IsKeyDown'), 'BogusKey')
    T('GetMouseScreenPos', G('GetMouseScreenPos')) T('GetMouseWorldPos', G('GetMouseWorldPos')) T('GetCursor', G('GetCursor'))
    P('GetMouseScreenPos shape', function() local p = GetMouseScreenPos() local n = 0 for k in pairs(p) do n = n + 1 end return n, type(p[1]), type(p.x) end)
    P('GetMouseWorldPos shape', function() local p = GetMouseWorldPos() local n = 0 for k in pairs(p) do n = n + 1 end return n, type(p[1]), type(p.x) end)
    P('GetUIControlsAlpha', G('GetUIControlsAlpha'))
    P('AnyInputCapture', G('AnyInputCapture')) P('GetInputCapture', G('GetInputCapture'))
    for _, k in ipairs({ 65, 13, 27, 310, 342, 8, 9, 32, 127, 300, 316, 317, 318, 319, 366, 367, 48 }) do P('KeycodeMauiToMSW ' .. k, G('KeycodeMauiToMSW'), k) end
    for _, k in ipairs({ 65, 13, 27, 19, 112, 8, 9, 32, 46, 37, 38, 39, 40, 33, 34, 48 }) do P('KeycodeMSWToMaui ' .. k, G('KeycodeMSWToMaui'), k) end
    P('GetValidAttackingUnits none', function() SelectUnits({}) return GetValidAttackingUnits() end)
    T('PlaySound', G('PlaySound'), Sound({ Bank = 'Interface', Cue = 'UI_Menu_MouseDown_01' }))
    P('Sound', function() local s = Sound({ Bank = 'Interface', Cue = 'UI_Menu_MouseDown_01' }) return type(s), type(s) == 'table' and s or nil end)
    P('GetCueBank', function() return GetCueBank(Sound({ Bank = 'Interface', Cue = 'UI_Menu_MouseDown_01' })) end)
    P('ConTextMatches ui_la', function() return sortedHead(ConTextMatches('ui_lu'), 5) end)
    P('ConExecute UI_Lua', G('ConExecute'), 'UI_Lua LOG("PROBE input ConExecute ran")')
    WaitSeconds(0.3)

    STAGE = 'lua'
    local function E(label, f) P(label, f) end
    E('tostring ints', function() return tostring(5) .. '|' .. tostring(5.0) .. '|' .. tostring(-0) .. '|' .. tostring(1e15) .. '|' .. tostring(1e16) .. '|' .. tostring(2 ^ 53) .. '|' .. tostring(123456789012) end)
    E('tostring floats', function() return tostring(0.1) .. '|' .. tostring(10 / 3) .. '|' .. tostring(1.5) .. '|' .. tostring(1e100) .. '|' .. tostring(3.14159265358979) .. '|' .. tostring(100 / 3) .. '|' .. tostring(1e-5) .. '|' .. tostring(0.30000001192093) end)
    E('tostring inf nan', function() return tostring(1 / 0) .. '|' .. tostring(-1 / 0) .. '|' .. tostring(0 / 0) end)
    E('concat numbers', function() return (5 .. '') .. '|' .. (2.50 .. '') .. '|' .. (1 / 4 .. '') .. '|' .. (7 / 2 .. 'x') end)
    E('format %d 3.7', function() return string.format('%d', 3.7) end)
    E('format %d -3.7', function() return string.format('%d', -3.7) end)
    E('format %d "5"', function() return string.format('%d', '5') end)
    E('format %.0f halves', function() return string.format('%.0f %.0f %.0f %.0f', 0.5, 1.5, 2.5, 3.5) end)
    E('format %g', function() return string.format('%g %g %g %g', 100000, 1e20, 0.1, 1234567) end)
    E('format %s number', function() return string.format('%s %s %s', 1.5, 2, true) end)
    E('format %s nil', function() return string.format('%s', nil) end)
    E('format %x %X %o %c %e %i %u', function() return string.format('%x %X %o %c %e %i %u', 255, 255, 8, 65, 12345.678, 3, 7) end)
    E('format pad', function() return string.format('%5s|%-5s|%05d|%+d|%5.1f|%%', 'a', 'b', 42, 3, 2.25) end)
    E('format %q', function() return string.format('%q', 'a"b\nc') end)
    E('format too few args', function() return string.format('%d %d', 1) end)
    E('tonumber', function() return fv({ tonumber('0x10'), tonumber('  5  '), tonumber('5a') or 'nil', tonumber('1e2'), tonumber('') or 'nil', tonumber('10', 2), tonumber('.5'), tonumber('5.'), tonumber('z', 36), tonumber(nil) or 'nil' }, 1) end)
    E('string arith', function() return '10' + 5, '3' * '4', 10 .. 20 end)
    E('compare num str', function() return 1 < '2' end)
    E('math.mod', function() return math.mod(-7, 3), math.mod(7, -3), math.mod(5.5, 2) end)
    E('math.fmod exists', function() return type(math.fmod), type(math.mod), type(math.pow), type(math.log10), type(math.huge), type(math.pi) end)
    E('math.floor', function() return math.floor(-0.5), math.ceil(-0.5), math.max(1, 5, 3), math.min(2, -1) end)
    E('math.random', function() local a, b, c = math.random(), math.random(5), math.random(2, 4) return a >= 0 and a < 1, b >= 1 and b <= 5, c >= 2 and c <= 4 end)
    E('table.getn holes', function() return table.getn({ 1, 2, nil, 4 }), table.getn({ nil, nil, 3 }), table.getn({ n = 3 }), table.getn({}) end)
    E('table.getn after nil', function() local t = { 1, 2, 3 } t[3] = nil return table.getn(t) end)
    E('table.setn', function() local t = {} table.setn(t, 5) return table.getn(t) end)
    E('table.insert pos', function() local t = { 1, 2, 3 } table.insert(t, 1, 0) return table.concat(t, ',') end)
    E('table.remove', function() local t = { 1, 2, 3 } local a = table.remove(t) local b = table.remove(t, 1) return a, b, table.concat(t, ','), table.remove({}) end)
    E('table.concat', function() return table.concat({ 1, 2, 'a' }, '-'), table.concat({}, ','), table.concat({ 1, 2, 3 }, ',', 2, 3) end)
    E('table.sort comparator error', function() local t = { 3, 1, 2 } table.sort(t, function(a, b) return a > b end) return table.concat(t, ',') end)
    E('table.sort mixed', function() local t = { 3, 'a', 2 } table.sort(t) return table.concat(t, ',') end)
    E('table lib names', function() local s = {} for k in pairs(table) do table.insert(s, k) end table.sort(s) return table.concat(s, ',') end)
    E('string lib names', function() local s = {} for k in pairs(string) do table.insert(s, k) end table.sort(s) return table.concat(s, ',') end)
    E('math lib names', function() local s = {} for k in pairs(math) do table.insert(s, k) end table.sort(s) return table.concat(s, ',') end)
    E('os lib names', function() local s = {} for k in pairs(os) do table.insert(s, k) end table.sort(s) return table.concat(s, ',') end)
    E('libs', function() return type(io), type(debug), type(coroutine), type(os), type(package), type(utf8), type(bit), type(bit32), type(loadstring), type(dofile), type(require), type(getfenv), type(setfenv), type(newproxy), type(gcinfo), type(collectgarbage) end)
    E('debug names', function() local s = {} for k in pairs(debug) do table.insert(s, k) end table.sort(s) return table.concat(s, ',') end)
    E('string.find', function() return string.find('hello world', 'o w'), string.find('hello', 'l+'), string.find('a.b', '.', 1, true), string.find('abc', 'x') end)
    E('string.find captures', function() return string.find('key=val', '(%w+)=(%w+)') end)
    E('string.find init neg', function() return string.find('hello', 'l', -2) end)
    E('string.gsub', function() return string.gsub('hello world', 'o', '0'), string.gsub('abc', '%w', '%0%0'), string.gsub('abc', '', '-') end)
    E('string.gsub fn/table', function() return string.gsub('a b', '%w', function(c) return string.upper(c) end), string.gsub('$x $y', '%$(%w+)', { x = '1' }) end)
    E('string.gsub anchor', function() return string.gsub('aaa', '^a', 'b'), string.gsub('hello', 'l', 'L', 1) end)
    E('string.gfind', function() local s = '' for w in string.gfind('a bc d', '%a+') do s = s .. w .. ';' end return s end)
    E('string.gmatch exists', function() return type(string.gmatch), type(string.gfind), type(string.match), type(string.reverse) end)
    E('string.sub', function() return string.sub('hello', 2, -2), string.sub('hello', -3), string.sub('hello', 0), string.sub('hello', 10), string.sub('hello', 2, 100) end)
    E('string.rep byte char', function() return string.rep('ab', 3), string.byte('A'), string.char(72, 105), string.len('a\0b'), string.upper('aBc'), string.lower('aBc') end)
    E('string methods', function() local s = 'hello' return s:len(), s:upper(), ('x'):rep(3) end)
    E('string index', function() local s = 'hello' return s.len ~= nil, s.nope, s[1] end)
    E('number index', function() local n = 5 return n.foo end)
    E('number index nested', function() local n = 5 return n.foo.bar end)
    E('boolean index', function() local b = true return b.foo end)
    E('nil index', function() local x = nil return x.foo end)
    E('number call method', function() local n = 5 return n:foo() end)
    E('function index', function() local f = function() end return f.foo end)
    E('number newindex', function() local n = 5 n.foo = 1 return 'ok' end)
    E('nil newindex', function() local x = nil x.foo = 1 return 'ok' end)
    E('undefined global read', function() return probe_undefined_global_xyz end)
    E('undefined global rawget', function() return rawget(_G, 'probe_undefined_global_xyz') end)
    E('global write', function() probe_global_written = 1 return probe_global_written end)
    E('tostring table prefix', function() return string.sub(tostring({}), 1, 7), string.sub(tostring(print), 1, 9), tostring(nil), tostring(true) end)
    E('type names', function() return type(nil), type(1), type('s'), type({}), type(print), type(coroutine.create(function() end)), type(categories.LAND) end)
    E('unpack', function() return unpack({ 1, 2, 3 }) end)
    E('unpack holes', function() return unpack({ 1, nil, 3 }) end)
    E('select/arg', function() return type(select), type(arg) end)
    E('next/pairs order arrays', function() local s = '' for k, v in pairs({ 10, 20, 30, a = 1 }) do s = s .. tostring(k) .. ';' end return s end)
    E('ipairs stops at nil', function() local n = 0 for i, v in ipairs({ 1, 2, nil, 4 }) do n = n + 1 end return n end)
    E('pcall error value', function() local ok, e = pcall(error, { code = 1 }) return ok, type(e), e.code end)
    E('pcall error string position', function() local ok, e = pcall(function() error('boom') end) return ok, string.find(e, 'boom') ~= nil, string.sub(e, -4) end)
    E('error level 0', function() local ok, e = pcall(function() error('boom', 0) end) return e end)
    E('assert message', function() local ok, e = pcall(assert, false, 'amsg') return e end)
    E('assert default', function() local ok, e = pcall(assert, false) return e end)
    E('xpcall', function() return xpcall(function() error('x', 0) end, function(e) return 'handled ' .. tostring(e) end) end)
    E('coroutine', function() local co = coroutine.create(function(a) local b = coroutine.yield(a + 1) return b * 2 end) local _, x = coroutine.resume(co, 1) local _, y = coroutine.resume(co, 5) return x, y, coroutine.status(co), coroutine.resume(co) end)
    E('setmetatable __index/__call', function() local t = setmetatable({}, { __index = function(t, k) return k .. '!' end, __call = function(self, a) return a + 1 end }) return t.x, t(1) end)
    E('metamethods __lt __le __eq __concat __unm', function()
        local mt = {}
        mt.__lt = function(a, b) return a.v < b.v end mt.__le = function(a, b) return a.v <= b.v end mt.__eq = function(a, b) return a.v == b.v end
        mt.__concat = function(a, b) return 'cat' end mt.__unm = function(a) return 'neg' end
        local a, b = setmetatable({ v = 1 }, mt), setmetatable({ v = 1 }, mt)
        return a < b, a <= b, a == b, a .. b, -a
    end)
    E('__gc/__mode', function() local t = setmetatable({}, { __mode = 'k' }) return getmetatable(t).__mode end)
    E('getmetatable string', function() return type(getmetatable('')), getmetatable('') and type(getmetatable('').__index) end)
    E('os.time/clock/date', function() return type(os.time and os.time()), type(os.clock and os.clock()), type(os.date and os.date('%Y')) end)
    E('integer division results', function() return 7 / 2, 2 ^ 10, 2 ^ 0.5, 10 - 3 * 2, -2 ^ 2 end)
    E('long string/escapes', function() return [[a
b]], 'tab\tx', '\65\066', "q'q" end)
    E('number formats', function() return 0xff, 1e2, .5, 5 == 5.0, 1 == '1' end)
    E('and/or', function() return nil or 'd', false and 1, 0 and 'zero-true', '' and 'empty-true' end)
    E('table.getn string', function() return table.getn('abc') end)
    E('string compare', function() return 'a' < 'B', 'a' < 'b', 'Z' < 'a', '10' < '9' end)
    E('repr()', function() return type(repr), type(reprs), type(reprsl), type(table.copy), type(table.deepcopy), type(table.empty), type(table.getsize), type(string.split) end)
    WaitSeconds(3)
    STAGE = 'risky'
    LOG('PROBE disk before GetTextureDimensions missing')
    P('GetTextureDimensions missing', G('GetTextureDimensions'), '/textures/ui/nope_probe.dds')
    WaitSeconds(3)
    LOG('PROBE done')
end)
