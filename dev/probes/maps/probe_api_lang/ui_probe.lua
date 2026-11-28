local LAYER = 'UI' -- Lua language probe (interface half): the same syntax cases in the user VM, then loadstring
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
local function T(label, fn)
    local ok, a, b, c = pcall(fn)
    LOG('PROBE UI ' .. label .. ' => ' .. (ok and ('ok ' .. ser(a) .. ' ' .. ser(b) .. ' ' .. ser(c)) or ('ERR ' .. clean(tostring(a)))))
end
local function uiextra()
    T('ls.type', function() return type(loadstring), type(loadfile), type(dofile) end)
    T('ls.basic', function() return loadstring('return 1 + 1')() end)
    T('ls.ne', function() return loadstring('return 1 != 2')() end)
    T('ls.bad', function() return loadstring('return +') end)
    T('ls.name', function() local f, e = loadstring('error("x")', 'chunkname') return pcall(f) end)
    T('ls.env', function() local f = loadstring('return probe_x') setfenv(f, { probe_x = 7 }) return f() end)
    T('ls.global', function() local f = loadstring('probe_ls_tmp = 5 return probe_ls_tmp') local v = f() probe_ls_tmp = nil return v end)
    T('ls.nil', function() return loadstring(nil) end)
    T('ls.num', function() return loadstring(5) end)
    T('ls.empty', function() return type(loadstring('')), loadstring('')() end)
    T('ls.bytecode', function() return type(string.dump), type(loadstring(string.dump(function() return 7 end))) end)
    T('ls.lualex', function() return type(string.lualex), string.lualex and string.lualex('local a = 1') end)
    T('ls.HashChecker', function() return type(HashChecker) end)
end
ForkThread(function()
    WaitSeconds(8)
    for _, f in { syntax, uiextra } do
        local ok, err = pcall(f)
        if not ok then LOG('PROBE UI section error ' .. tostring(err)) end
        WaitSeconds(1)
    end
    LOG('PROBE UI end')
end)
