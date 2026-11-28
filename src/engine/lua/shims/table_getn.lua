-- Lua 5.0 table sizes, as the real game has them (dev/probes
-- probe_tablesize). `table.getn` reads a numeric `n` field first, then the
-- size the library has recorded for the table, and only then the array
-- length. `table.setn`, `table.insert` and `table.remove` record that size
-- (in `n` when the table has one), so a table they have touched keeps its
-- count when entries are set to nil by hand:
--   t = {} insert a, b, c; t[3] = nil      -> getn 3, and the next insert is t[4]
--   six inserts, two set to nil, compacted by hand -> getn stays 6
-- AI code leans on it: it empties slots by hand, walks `1..getn`, and
-- expects to reach everything it ever inserted (M28's zone unit lists; with
-- Luau's length the entries past the first gap were never visited again).
-- Elements move raw, as in Lua 5.0. Not replicated: for a table the library
-- has never touched, Lua 5.0 counts up to the first nil (`{1, 2, nil, 4}`
-- is 2 there; `{'a','b','c'}` with [1] set to nil is 0), where this uses
-- Luau's length; and `getn` of a string gives its length (the real one
-- raises). One departure on purpose: a table left with nothing in it at all
-- counts as empty again (Lua 5.0 keeps its old size: three inserts, all set
-- to nil, the next insert is t[4]). The engine's own code empties queues
-- that script fills with `table.insert`, and nothing can be lost in an
-- empty table.
local rawget, rawset, type, select, next = rawget, rawset, type, select, next
local sizes = setmetatable({}, { __mode = "k" })
-- Tables whose size `setn` gave: kept even when empty (probed: `setn(t, 3)`
-- on an empty table makes getn 3).
local set = setmetatable({}, { __mode = "k" })

-- The size recorded for `t` (nil for none), dropped once `t` is empty.
local function recorded(t)
    local s = sizes[t]
    if s and s > 0 and rawget(t, s) == nil and next(t) == nil and not set[t] then
        sizes[t] = nil
        return nil
    end
    return s
end

function table.getn(t)
    if t.n ~= nil then
        local n = rawget(t, "n")
        if type(n) == "number" then
            return n
        end
    end
    return recorded(t) or #t
end

function table.setn(t, n)
    if type(n) ~= "number" or type(t) ~= "table" then
        return
    end
    if type(rawget(t, "n")) == "number" then
        rawset(t, "n", n)
    else
        sizes[t] = n
        set[t] = true
    end
end

function table.insert(t, ...)
    local field = false
    local s
    if t.n ~= nil then
        s = rawget(t, "n")
        field = type(s) == "number"
    end
    if not field then
        s = recorded(t) or #t
    end
    local e = s + 1
    if select("#", ...) == 1 then
        if field then rawset(t, "n", e) else sizes[t] = e end
        rawset(t, e, (...))
        return
    end
    local pos, v = ...
    if type(pos) ~= "number" then
        error("bad argument #2 to 'insert' (number expected, got " .. type(pos) .. ")", 2)
    end
    if pos > e then
        e = pos
    end
    if field then rawset(t, "n", e) else sizes[t] = e end
    for i = e - 1, pos, -1 do
        rawset(t, i + 1, rawget(t, i))
    end
    rawset(t, pos, v)
end

function table.remove(t, pos)
    local field = false
    local s
    if t.n ~= nil then
        s = rawget(t, "n")
        field = type(s) == "number"
    end
    if not field then
        s = recorded(t) or #t
    end
    if s <= 0 then
        return
    end
    if field then rawset(t, "n", s - 1) else sizes[t] = s - 1 end
    pos = pos or s
    local r = rawget(t, pos)
    for i = pos, s - 1 do
        rawset(t, i, rawget(t, i + 1))
    end
    rawset(t, s, nil)
    return r
end
