-- Interface half of probe_re3_ui: paths (exists / DiskGetFileInfo), then
-- GetIdleEngineers at tick 40 (only the ACU) and tick 100 (engineers too),
-- then right-click orders with the cursor checked: for each target the
-- camera is moved until the target's ground point is under the mouse
-- (UnProject), and we log the pixel distance (Project), GetMouseWorldPos's
-- height above the terrain (> 0.3: the ray hit a unit), GetRolloverInfo, and
-- a control order (an engineer selected: Guard when the cursor is on a unit
-- of ours, Move over ground).
--   ours: rawset(_G, 'PROBE_CURSOR', { x, y }) ahead of this file (interface pixels).
local fmt = string.format
local function G(name) return rawget(_G, name) end
local function L(s) LOG('PROBE ui ' .. tostring(GameTick()) .. ' ' .. s) end
local function S(v)
    if type(v) == 'number' then return fmt('%.2f', v) end
    return tostring(v)
end
local function waitTick(t) while GameTick() < t do WaitSeconds(0.05) end end

-- (1) paths
local PATHS = {
    '/maps/probe_re3_ui', '/maps/probe_re3_ui/', '/maps/probe_re3_ui/lua/AI/CustomAIs_v2', '/maps/probe_re3_ui/lua/AI/CustomAIs_v2/',
    '/maps/probe_re3_ui/lua/AI/CustomAIs_v2/marker.lua', '/maps/probe_re3_ui/probe.lua', '/maps/probe_re3_ui/nope', '/maps/probe_re3_ui/nope/',
    '/MAPS/PROBE_RE3_UI', '/maps', '/maps/', '/schook', '/schook/', '/lua', '/lua/', '/lua/AI', '/lua/AI/', '/lua/AI/AIBaseTemplates',
    '/lua/ui/uiutil.lua', '/lua/nope.lua', '/maps/SCMP_009', '/maps/SCMP_009/', '/maps/SCMP_009/SCMP_009_scenario.lua', '/mods', '/mods/',
}
local function fileinfo(v)
    if type(v) ~= 'table' then return tostring(v) end
    local keys = {}
    for k in v do table.insert(keys, tostring(k)) end
    table.sort(keys)
    local s = '{'
    for _, k in keys do
        local x = v[k]
        if k == 'TimeStamp' or k == 'WriteTime' then x = '<' .. type(x) .. '>' end
        if type(x) == 'number' then x = fmt('%d', x) end
        s = s .. ' ' .. k .. '=' .. tostring(x)
    end
    return s .. ' }'
end
for _, p in PATHS do
    local oke, e = pcall(function() return exists(p) end)
    local okd, d = pcall(function() return DiskGetFileInfo(p) end)
    LOG('PROBE ui path ' .. p .. ' exists=' .. (oke and tostring(e) or ('ERR ' .. tostring(e))) .. ' info=' .. (okd and fileinfo(d) or ('ERR ' .. tostring(d))))
end
do
    local ok, f = pcall(function() return DiskFindFiles('/maps/probe_re3_ui/lua/AI/CustomAIs_v2', '*.lua') end)
    LOG('PROBE ui findfiles ' .. (ok and (type(f) == 'table' and (table.getn(f) .. ' ' .. tostring(f[1])) or tostring(f)) or ('ERR ' .. tostring(f))))
end

local function uname(u)
    local okn, n = pcall(function() return u:GetCustomName(u) end)
    local oki, id = pcall(function() return u:GetEntityId() end)
    local okb, bp = pcall(function() return u:GetUnitId() end)
    local okl, idle = pcall(function() return u:IsIdle() end)
    return tostring(okb and bp) .. '#' .. tostring(oki and id) .. ' ' .. tostring(okn and n) .. ' idle=' .. tostring(okl and idle)
end
local function idleEng(tag)
    local ok, list = pcall(function() return GetIdleEngineers() end)
    if not ok then L(tag .. ' GetIdleEngineers ERR ' .. tostring(list)) return end
    if type(list) ~= 'table' then L(tag .. ' GetIdleEngineers = ' .. tostring(list)) return end
    local s = tag .. ' GetIdleEngineers n=' .. table.getn(list)
    for _, u in list do s = s .. ' [' .. uname(u) .. ']' end
    L(s)
    local okf, fl = pcall(function() return GetIdleFactories() end)
    L(tag .. ' GetIdleFactories ' .. (okf and (type(fl) == 'table' and ('n=' .. table.getn(fl)) or tostring(fl)) or 'ERR'))
end

local function named()
    local out = {}
    UISelectionByCategory('ALLUNITS', false, false, false, false)
    WaitSeconds(0.3)
    for _, u in pairs(GetSelectedUnits() or {}) do
        local ok, n = pcall(function() return u:GetCustomName(u) end)
        if ok and n then out[n] = u end
        if u:GetUnitId() == 'uel0001' then out.p_acu = u end
    end
    SelectUnits({})
    return out
end

ForkThread(function()
    WaitSeconds(0.5)
    idleEng('phase1')
    waitTick(230)
    idleEng('phase2')
    local u = named()
    local s = 'named:'
    for k, v in u do s = s .. ' ' .. k end
    L(s)

    local cam = GetCamera('WorldCamera')
    local view = import('/lua/ui/game/worldview.lua').viewLeft
    local fixed = G('PROBE_CURSOR')
    local function mouseScreen()
        if fixed then return { fixed[1], fixed[2] } end
        local m = GetMouseScreenPos()
        return { m[1], m[2] }
    end
    L(fmt('view %s %s %s %s mouse %s,%s', S(view.Left()), S(view.Top()), S(view.Right()), S(view.Bottom()), S(mouseScreen()[1]), S(mouseScreen()[2])))
    local function ground()
        local m = mouseScreen()
        return UnProject(view, Vector2(m[1], m[2]))
    end
    local ZOOM = 60
    -- no waits inside pcall (Lua 5.0 can't yield across it)
    local function snap(x, y, z) if not G('PROBE_ONLY') then pcall(function() cam:SnapTo(Vector(x, y, z), { math.pi, 1.2, 0 }, ZOOM) end) end end
    local function hover(pos)
        local fx, fz = pos[1], pos[3]
        for i = 1, (G('PROBE_ONLY') and 0 or 4) do
            snap(fx, pos[2], fz)
            WaitSeconds(0.25)
            local ok, g = pcall(ground)
            if ok and g then fx, fz = fx + (pos[1] - g[1]), fz + (pos[3] - g[3]) end
        end
        snap(fx, pos[2], fz)
        WaitSeconds(0.4)
        local ok, r = pcall(function()
            local g = ground()
            local m = mouseScreen()
            local pr = view:Project(Vector(pos[1], pos[2], pos[3]))
            local okw, mw = pcall(GetMouseWorldPos)
            local hit = okw and mw and (mw[2] - g[2]) or -99
            return fmt('groundoff %.2f px %.1f,%.1f hitY %.2f', VDist2(g[1], g[3], pos[1], pos[3]), pr[1] + view.Left() - m[1], pr[2] + view.Top() - m[2], hit)
        end)
        return ok and r or ('hoverERR ' .. tostring(r))
    end
    local function rollover()
        local ok, info = pcall(function() return GetRolloverInfo() end)
        if ok and type(info) == 'table' then
            return 'rollover ' .. tostring(info.blueprintId) .. '#' .. tostring(info.entityId) .. ' army ' .. tostring(info.armyIndex)
        end
        return 'rollover ' .. (ok and tostring(info) or ('ERR ' .. string.sub(tostring(info), 1, 60)))
    end
    local function order(sel)
        pcall(SelectUnits, sel)
        WaitSeconds(0.3)
        local ok, o = pcall(function() return view:GetRightMouseButtonOrder() end)
        return ok and tostring(o) or ('ERR ' .. tostring(o))
    end
    local function where(n)
        if type(n) == 'table' then return n end
        local p = u[n]:GetPosition()
        return { p[1], p[2], p[3] }
    end
    -- one target, several selections; the control (engineer) first.
    local CASES = {
        { 'ground', { 400, 18.5, 612 }, { { 'eng', 'p_ideng' }, { 'tank', 'p_tank' }, { 'radar', 'p_radar' }, { 'pgen', 'p_pgen' }, { 'radar+tank', 'p_radar', 'p_tank' }, { 'trans', 'p_trans' } } },
        { 'p_trans', nil, { { 'eng', 'p_ideng' }, { 'tank', 'p_tank' }, { 'tank+tank2', 'p_tank', 'p_tank2' }, { 'shield', 'p_shield' }, { 'radar', 'p_radar' } } },
        { 'p_trans2', nil, { { 'eng', 'p_ideng' }, { 'tank', 'p_tank' } } },
        { 'p_shield', nil, { { 'eng', 'p_ideng' }, { 'trans', 'p_trans' }, { 'tank', 'p_tank' } } },
        { 'p_tank2', nil, { { 'eng', 'p_ideng' }, { 'trans', 'p_trans' }, { 'radar', 'p_radar' } } },
        { 'p_ideng', nil, { { 'tank', 'p_tank' }, { 'trans', 'p_trans' } } },
        { 'p_enemy', { 394.5, 18.5, 570.5 }, { { 'eng', 'p_ideng' }, { 'tank', 'p_tank' }, { 'radar', 'p_radar' }, { 'trans', 'p_trans' } } },
        { 'p_fac', nil, { { 'eng', 'p_ideng' }, { 'tank', 'p_tank' }, { 'radar', 'p_radar' } } },
    }
    for _, c in CASES do
        if not G('PROBE_ONLY') or G('PROBE_ONLY') == c[1] then
            local okp, pos = pcall(function() return c[2] or where(c[1]) end)
            if not okp or not pos then
                L('rmb over ' .. c[1] .. ' ERR ' .. tostring(pos))
            else
                pcall(SelectUnits, {})
                local h = hover(pos)
                L('rmb over ' .. c[1] .. ' ' .. h .. ' ' .. rollover())
                for _, s in c[3] do
                    local sel = {}
                    for i = 2, table.getn(s) do if u[s[i]] then table.insert(sel, u[s[i]]) end end
                    local o = order(sel)
                    L('rmb ' .. s[1] .. ' over ' .. c[1] .. ' = ' .. o .. ' ' .. rollover())
                end
            end
        end
    end
    SelectUnits({})
    WaitSeconds(1)
    LOG('PROBE done')
end)
