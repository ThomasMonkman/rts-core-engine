-- API census probe (sim half): every global, moho class method, library member.
local LAYER = 'SIM'
local function kind(v)
    local t = type(v)
    if t == 'function' then
        local c = false
        if debug and debug.getinfo then
            local ok, i = pcall(debug.getinfo, v, 'S')
            if ok and type(i) == 'table' and i.what == 'C' then c = true end
        elseif debug and debug.info then
            local ok, s = pcall(debug.info, v, 's')
            if ok and s == '[C]' then c = true end
        end
        return c and 'cf' or 'f'
    end
    return t
end
local function keys(t, seen, acc)
    seen = seen or {}; acc = acc or {}
    if type(t) ~= 'table' or seen[t] then return acc end
    seen[t] = true
    for k, v in next, t do
        if type(k) == 'string' and acc[k] == nil then acc[k] = kind(v) end
    end
    local mt = getmetatable(t)
    if type(mt) == 'table' then
        local ix = rawget(mt, '__index')
        if type(ix) == 'table' and ix ~= _G then keys(ix, seen, acc) end
    end
    return acc
end
local function emit(tag, acc)
    local parts = {}
    for k, v in acc do table.insert(parts, k .. ':' .. v) end
    table.sort(parts)
    local n = table.getn(parts)
    LOG('PROBE ' .. LAYER .. ' ' .. tag .. ' #' .. n)
    local i = 1
    while i <= n do
        local j = math.min(i + 11, n)
        local s = ''
        for x = i, j do s = s .. ' ' .. parts[x] end
        LOG('PROBE ' .. LAYER .. ' ' .. tag .. ' =' .. s)
        i = j + 1
    end
end
local function census()
    local g = {}
    for k, v in next, _G do if type(k) == 'string' then g[k] = kind(v) end end
    emit('G', g)
    local m = rawget(_G, 'moho')
    if type(m) == 'table' then
        local names = {}
        for cname, ct in next, m do names[cname] = type(ct) end
        emit('MOHO', names)
        for cname, ct in next, m do
            if type(ct) == 'table' then emit('M.' .. cname, keys(ct)) end
        end
    end
    for _, lib in {'math', 'string', 'table', 'os', 'io', 'debug', 'coroutine', 'utf', 'lazyvar', 'bit', 'bit32', 'buffer', 'utf8', 'task', 'vector'} do
        local t = rawget(_G, lib)
        if type(t) == 'table' then emit('L.' .. lib, keys(t)) else LOG('PROBE ' .. LAYER .. ' L.' .. lib .. ' absent ' .. type(t)) end
    end
    local cats = rawget(_G, 'categories')
    if type(cats) == 'table' then
        local n, up = 0, {}
        for k, v in next, cats do
            n = n + 1
            if type(k) == 'string' and string.upper(k) == k and not string.find(k, '%d%d%d') then up[k] = type(v) end
        end
        LOG('PROBE ' .. LAYER .. ' categories count ' .. n)
        emit('CAT', up)
    else
        LOG('PROBE ' .. LAYER .. ' categories ' .. type(cats))
    end
    local bps = rawget(_G, '__blueprints')
    if type(bps) == 'table' then
        local n, ns, nn = 0, 0, 0
        local tk = {}
        for k, v in next, bps do
            n = n + 1
            if type(k) == 'string' then ns = ns + 1 else nn = nn + 1 end
            if type(v) == 'table' then
                local bt = rawget(v, 'BlueprintType') or '?'
                tk[bt] = (tk[bt] or 0) + 1
            end
        end
        local s = ''
        local ks = {}
        for k in tk do table.insert(ks, k) end
        table.sort(ks)
        for _, k in ks do s = s .. ' ' .. k .. '=' .. tk[k] end
        LOG('PROBE ' .. LAYER .. ' blueprints n=' .. n .. ' str=' .. ns .. ' num=' .. nn .. s)
    else
        LOG('PROBE ' .. LAYER .. ' __blueprints ' .. type(bps))
    end
    local gm = getmetatable(_G)
    LOG('PROBE ' .. LAYER .. ' _G metatable ' .. type(gm))
    if type(gm) == 'table' then emit('GMT', keys(gm)) end
    LOG('PROBE ' .. LAYER .. ' _VERSION ' .. tostring(rawget(_G, '_VERSION')))
end
ForkThread(function()
    WaitTicks(5)
    local ok, err = pcall(census)
    if not ok then LOG('PROBE SIM census error ' .. tostring(err)) end
    LOG('PROBE SIM end')
    WaitTicks(60)
    LOG('PROBE done')
end)
