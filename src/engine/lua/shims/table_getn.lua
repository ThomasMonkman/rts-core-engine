-- Lua 5.0 table sizes. `table.getn` reads a numeric `n` field first, then a
-- size given by `table.setn`, and only then the array length (probed:
-- `getn({n = 5})` = 5; `setn(t, 3)` on an empty table makes getn 3).
-- `table.insert` / `table.remove` on a table that has either size keep it
-- up to date, moving elements raw as Lua 5.0 does (`insert` on `{n = 0}`
-- bumps `n`; after `setn(t, 2)` on four items, `insert(t, x)` puts x at 3).
-- A table with neither uses Luau's length and library calls unchanged. Not
-- replicated: Lua 5.0 counts up to the first nil (`{1, 2, nil, 4}` is 2
-- there, Luau's border is 4), and its insert / remove record a size on
-- every table they touch; both would cost every call, and neither is
-- probed beyond the hole case. `getn` of a string gives its length (the
-- real one raises): the plain path is `#t`, an opcode, not a type check.
local rawget, rawset, type, select = rawget, rawset, type, select
local tinsert, tremove = table.insert, table.remove
local sizes = setmetatable({}, { __mode = "k" })
-- Whether `setn` has ever run: until then no table has a setn size and the
-- weak table needn't be looked at (getn / insert are hot).
local anysized = false

-- The table's Lua 5.0 size (`n` field or setn size) and whether it is `n`,
-- or nil. `rawget` raises for a non-table, as Luau's functions would.
-- getn / insert / remove are hot, so they first test `t.n` (an opcode, not
-- a call) and `anysized`, and fall to Luau's function for a plain table.
local function size(t)
    local n = rawget(t, "n")
    if n ~= nil and type(n) == "number" then
        return n, true
    end
    if anysized then
        return sizes[t], false
    end
    return nil
end

local function setsize(t, n, field)
    if field then
        rawset(t, "n", n)
    else
        sizes[t] = n
    end
end

function table.getn(t)
    if t.n ~= nil or anysized then
        local n = size(t)
        if n then
            return n
        end
    end
    return #t
end

function table.setn(t, n)
    if type(n) ~= "number" then
        return
    end
    if type(t) ~= "table" then
        return
    end
    local _, field = size(t)
    anysized = anysized or not field
    setsize(t, n, field)
end

function table.insert(t, ...)
    local s, field
    if t.n ~= nil or anysized then
        s, field = size(t)
    end
    if s == nil then
        return tinsert(t, ...)
    end
    local e = s + 1
    local pos, v
    if select("#", ...) == 1 then
        pos, v = e, ...
    else
        pos, v = ...
        if type(pos) ~= "number" then
            return tinsert(t, ...)
        end
        if pos > e then
            e = pos
        end
    end
    setsize(t, e, field)
    for i = e - 1, pos, -1 do
        rawset(t, i + 1, rawget(t, i))
    end
    rawset(t, pos, v)
end

function table.remove(t, pos)
    local s, field
    if t.n ~= nil or anysized then
        s, field = size(t)
    end
    if s == nil then
        if pos == nil then
            return tremove(t)
        end
        return tremove(t, pos)
    end
    pos = pos or s
    if s <= 0 then
        return
    end
    setsize(t, s - 1, field)
    local r = rawget(t, pos)
    for i = pos, s - 1 do
        rawset(t, i, rawget(t, i + 1))
    end
    rawset(t, s, nil)
    return r
end
