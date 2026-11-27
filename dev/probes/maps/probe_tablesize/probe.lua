-- Probe: does the game's Lua keep a size for a table once table.insert /
-- table.remove has touched it (stock Lua 5.0's luaL_setn), so that
-- table.getn ignores entries set to nil by hand afterwards?
local function keys(t)
    local ks = {}
    for k in t do ks[table.getn(ks) + 1] = k end
    table.sort(ks, function(a, b) return tostring(a) < tostring(b) end)
    local s = ''
    for _, k in ipairs(ks) do s = s .. tostring(k) .. '=' .. tostring(t[k]) .. ' ' end
    return s
end
local function show(tag, t, extra)
    LOG('PROBE ', tag, ' getn=', table.getn(t), ' keys: ', keys(t), extra or '')
end
ForkThread(function()
    WaitTicks(5)
    local t

    t = { 'a', 'b', 'c' } t[3] = nil
    show('ctor-nil-last', t)
    t = { 'a', 'b', 'c' } t[1] = nil
    show('ctor-nil-first', t)
    t = { 'a', 'b', 'c', 'd' } t[2] = nil
    show('ctor-nil-middle', t)

    t = {} table.insert(t, 'a') table.insert(t, 'b') table.insert(t, 'c') t[3] = nil
    show('insert-nil-last', t)
    table.insert(t, 'x')
    show('insert-nil-last then insert', t)
    t = {} table.insert(t, 'a') table.insert(t, 'b') table.insert(t, 'c') t[1] = nil
    show('insert-nil-first', t)
    table.insert(t, 'x')
    show('insert-nil-first then insert', t)
    t = {} table.insert(t, 'a') table.insert(t, 'b') table.insert(t, 'c') t[1] = nil t[2] = nil t[3] = nil
    show('insert-nil-all', t)
    table.insert(t, 'x')
    show('insert-nil-all then insert', t)

    t = { 'a', 'b', 'c' } table.insert(t, 'd') t[4] = nil t[3] = nil
    show('ctor+insert-nil-two', t)
    t = { 'a', 'b', 'c' } table.insert(t, 'd') t[5] = 'e' t[6] = 'f'
    show('insert then assign past', t)
    table.insert(t, 'x')
    show('insert then assign past then insert', t)

    t = { 'a', 'b', 'c', 'd' } local r = table.remove(t) t[2] = nil t[3] = nil
    show('remove-then-nil', t, ' removed=' .. tostring(r))
    r = table.remove(t)
    show('remove-then-nil then remove', t, ' removed=' .. tostring(r))
    t = { 'a', 'b', 'c', 'd' } r = table.remove(t, 1) t[3] = nil
    show('remove1-then-nil-last', t, ' removed=' .. tostring(r))

    -- The M28 pattern: compact by hand, then insert.
    t = {} for i = 1, 6 do table.insert(t, 'u' .. i) end
    t[2] = nil t[5] = nil
    local rev = 1
    for i = 1, table.getn(t) do
        if t[i] then
            if i ~= rev then t[rev] = t[i] t[i] = nil end
            rev = rev + 1
        end
    end
    show('compacted', t)
    table.insert(t, 'new')
    show('compacted then insert', t)

    t = {} table.insert(t, 'a') table.insert(t, 'b') t[2] = nil
    local n = 0 for _ in ipairs(t) do n = n + 1 end
    show('ipairs count ' .. n, t, ' concat=' .. table.concat(t, ','))
    t = {} table.insert(t, 3) table.insert(t, 1) table.insert(t, 2) t[3] = nil
    local ok, e = pcall(table.sort, t)
    show('sort ok=' .. tostring(ok), t, ' ' .. tostring(e))
    t = {} table.insert(t, 'a') table.insert(t, 'b') t[2] = nil
    LOG('PROBE getsize=', table.getsize and table.getsize(t), ' empty=', tostring(table.empty and table.empty(t)), ' unpack=', tostring(select and select('#', unpack(t))))
    t = {} table.insert(t, 'a') table.insert(t, 'b') table.setn(t, 5)
    show('insert then setn 5', t)
    t = {} table.insert(t, 'a') t.n = 7
    show('insert then n=7', t)
    table.insert(t, 'z')
    show('insert then n=7 then insert', t)
    LOG('PROBE done')
end)
