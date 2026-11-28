local LAYER = 'UI'
-- Calls every engine C function (globals and moho class methods) with twelve nils: more than any
-- takes, so the engine's argument-count check rejects the call and its message gives the real arity.
local SKIP = { assert = 1, collectgarbage = 1, dofile = 1, error = 1, gcinfo = 1, getfenv = 1, getmetatable = 1, ipairs = 1,
    loadfile = 1, loadlib = 1, loadstring = 1, next = 1, pairs = 1, pcall = 1, print = 1, rawequal = 1, rawget = 1, rawset = 1,
    require = 1, setfenv = 1, setmetatable = 1, tonumber = 1, tostring = 1, type = 1, unpack = 1, xpcall = 1, doscript = 1,
    newproxy = 1, select = 1, rawlen = 1, typeof = 1, LOG = 1, WARN = 1, SPEW = 1, _ALERT = 1, _TRACEBACK = 1, import = 1,
    ForkThread = 1, KillThread = 1, exit = 1, ExitApplication = 1, ExitGame = 1, EndGame = 1 }
-- thread waits only count-check in the real game; elsewhere they could park this thread
if _VERSION ~= 'Lua 5.0.1' then
    SKIP.WaitFor = 1 SKIP.SuspendCurrentThread = 1 SKIP.WaitTicks = 1 SKIP.WaitSeconds = 1 SKIP.WaitFrames = 1
end
local function isc(v)
    local t = type(v)
    if t == 'cfunction' then return true end
    if t ~= 'function' then return false end
    if debug and debug.info then
        local ok, s = pcall(debug.info, v, 's')
        return ok and s == '[C]'
    end
    return false
end
local function clean(s)
    s = tostring(s)
    s = string.gsub(s, '[%c]+', ' ')
    return string.sub(s, 1, 150)
end
local pausen = 0
local function PAUSE()
    pausen = pausen + 1
    if math.mod(pausen, 2) == 0 then WaitSeconds(0.1) end
end
local function arity()
    local names = {}
    for k, v in next, _G do
        if type(k) == 'string' and isc(v) and not SKIP[k]
            and not string.find(k, '^Register') and not string.find(k, '^Spec') and not string.find(k, 'Exit') and not string.find(k, 'Restart') and not string.find(k, 'Quit')
            and not string.find(k, 'EndGame') and not string.find(k, 'Launch') and not string.find(k, 'Disconnect')
            and not string.find(k, 'Shutdown') then
            table.insert(names, k)
        end
    end
    table.sort(names)
    for _, k in names do
        local f = rawget(_G, k)
        LOG('PROBE ' .. LAYER .. ' next G.' .. k)
        local ok, a = pcall(f, nil, nil, nil, nil, nil, nil, nil, nil, nil, nil, nil, nil)
        LOG('PROBE ' .. LAYER .. ' A G.' .. k .. ' ' .. (ok and ('ok ' .. type(a)) or ('ERR ' .. clean(a))))
        PAUSE()
    end
    local m = rawget(_G, 'moho')
    local seen = {}
    local classes = {}
    for cname, ct in next, m do if type(ct) == 'table' then table.insert(classes, cname) end end
    table.sort(classes)
    for _, cname in classes do
        local ct = m[cname]
        local ms = {}
        for k, v in next, ct do if type(k) == 'string' and isc(v) then table.insert(ms, k) end end
        table.sort(ms)
        local dup = ''
        for _, k in ms do
            local f = ct[k]
            if seen[f] then
                dup = dup .. ' ' .. k
            else
                seen[f] = cname .. '.' .. k
                LOG('PROBE ' .. LAYER .. ' next M.' .. cname .. '.' .. k)
                local ok, a = pcall(f, nil, nil, nil, nil, nil, nil, nil, nil, nil, nil, nil, nil)
                LOG('PROBE ' .. LAYER .. ' A M.' .. cname .. '.' .. k .. ' ' .. (ok and ('ok ' .. type(a)) or ('ERR ' .. clean(a))))
                PAUSE()
            end
        end
    end
end
-- starts after the sim half has finished, so a call that takes the game down costs only this half
ForkThread(function()
    WaitSeconds(25)
    arity() -- not under pcall: it waits between batches, and Lua 5.0 can't yield across a pcall
    LOG('PROBE UI end')
end)
