-- Lua language probe (sim half): FA's Lua 5.0 dialect, one tiny module per syntax case (syn_*.lua),
-- imported under pcall so a case the VM rejects doesn't stop the rest. Then entity id formats.
local LAYER = 'SIM'
local function num(v)
    if v ~= v then return 'nan' end
    if v == 1 / 0 then return 'inf' end
    if v == -1 / 0 then return '-inf' end
    if v == math.floor(v) and v > -1e9 and v < 1e9 then return string.format('%d', v) end
    return string.format('%.4f', v)
end
local function clean(s)
    return (string.gsub(string.sub(s, 1, 130), '[%c]', '~'))
end
local function ser(v, d)
    d = d or 0
    local t = type(v)
    if t == 'number' then return num(v)
    elseif t == 'string' then return '"' .. clean(v) .. '"'
    elseif t == 'nil' or t == 'boolean' then return tostring(v)
    elseif t == 'table' then
        if d >= 2 then return '{..}' end
        local s = '{'
        local n = 0
        for i = 1, 12 do
            if v[i] ~= nil then n = i end
        end
        for i = 1, n do s = s .. ser(v[i], d + 1) .. ' ' end
        local ks = {}
        for k in v do if type(k) ~= 'number' then table.insert(ks, tostring(k)) end end
        table.sort(ks)
        for _, k in ks do s = s .. k .. '=' .. ser(v[k], d + 1) .. ' ' end
        return s .. '}'
    elseif t == 'cfunction' then return 'function'
    else return t end
end
local CASES = { "andor", "bitand", "bitbig", "bitfloat", "bitneg", "bitor", "bitprec", "bitprec2", "bitprec3", "bitshl32", "bitshlbig", "bitstr", "blockcomment", "bnot", "bom", "callstr", "ccomment", "chunkreturn", "cmpchain", "compound", "concat_prec", "continue", "continue_repeat", "continue_while", "crlf", "emptystat", "evalorder", "globals_meta", "globals_module", "globals_read", "gotolabel", "hashcomment", "hashcomment2", "hexfloat", "hexlit", "idiv", "import_rel", "import_self", "lenop", "longcomment_nest", "longstr", "longstr_eq", "longstr_nest", "loopbig", "loopclosure", "loopclosure_in", "loopclosure_while", "loopfloat", "loopstr", "loopvar_assign", "loopzero", "method_colon", "modop", "multiassign", "ne", "neq_tilde", "nulstr", "numlit", "numlit_bin", "numlit_under", "pcalldepth", "powzero", "rngpow", "recursion", "reserved_continue", "reserved_type", "returnmid", "semicolons", "shebang", "shl", "shr", "strescape", "strescape_bad", "strescape_dec", "strescape_x", "strescape_z", "strlen_big", "strnum_eq", "tablefunc", "tablenil", "tablesemi", "tabletrail", "tailcall", "upvalnum", "varargs_arg", "varargs_argassign", "varargs_dots", "varargs_global_arg", "varargs_named", "varargs_nested", "varargs_noargs", "xor",  }
local function syntax()
    for _, name in CASES do
        local env = setmetatable({}, { __index = _G })
        local ok, m = pcall(doscript, '/maps/probe_api_lang/syn_' .. name .. '.lua', env)
        local s
        if ok then
            local r = rawget(env, 'r')
            s = 'ok ' .. (r == nil and '<unset>' or ser(r)) .. ' ret=' .. type(m)
        else
            s = 'ERR ' .. clean(tostring(m))
        end
        LOG('PROBE ' .. LAYER .. ' syn.' .. name .. ' => ' .. s)
    end
end
local function ids()
    local armies = ListArmies()
    local w, h = GetMapSize()
    local cx, cz = w / 2, h / 2
    local s = ''
    for a = 1, 3 do
        for i = 1, 2 do
            local u = CreateUnitHPR('uel0201', armies[a], cx + a * 6, GetTerrainHeight(cx + a * 6, cz + i * 4), cz + i * 4, 0, 0, 0)
            s = s .. ' a' .. a .. ':' .. tostring(u:GetEntityId()) .. '/' .. type(u:GetEntityId()) .. '/' .. tostring(u.EntityId == u:GetEntityId())
            if a == 1 and i == 1 then
                local p = u:CreateProjectile('/projectiles/TDFGauss01/TDFGauss01_proj.bp', 0, 2, 0, 0, 1, 0)
                s = s .. ' proj:' .. tostring(p:GetEntityId()) .. '/' .. type(p:GetEntityId())
                local id = u:GetEntityId()
                LOG('PROBE SIM ids.lookup ' .. tostring(GetEntityById(id) == u) .. ' ' .. tostring(GetUnitById(id) == u)
                    .. ' num:' .. tostring(pcall(GetEntityById, tonumber(id))) .. ' ' .. tostring(pcall(GetUnitById, tonumber(id))))
            end
        end
    end
    LOG('PROBE SIM ids.units' .. s)
    local t = GetReclaimablesInRect(Rect(0, 0, w, h)) or {}
    local lo, hi, n = nil, nil, 0
    for _, r in t do
        if IsProp(r) then
            local id = tonumber(r:GetEntityId()) or -1
            n = n + 1
            if not lo or id < lo then lo = id end
            if not hi or id > hi then hi = id end
        end
    end
    LOG('PROBE SIM ids.props n=' .. n .. ' lo=' .. tostring(lo) .. ' hi=' .. tostring(hi))
    local pr = CreatePropHPR('/env/common/props/markers/M_Mass_prop.bp', cx, GetTerrainHeight(cx, cz), cz, 0, 0, 0)
    LOG('PROBE SIM ids.newprop ' .. tostring(pr and pr:GetEntityId()) .. ' ' .. type(pr and pr:GetEntityId()))
end
ForkThread(function()
    WaitTicks(5)
    for _, f in { syntax, ids } do
        local ok, err = pcall(f)
        if not ok then LOG('PROBE SIM section error ' .. tostring(err)) end
    end
    LOG('PROBE SIM end')
    WaitTicks(250)
    LOG('PROBE done')
end)
