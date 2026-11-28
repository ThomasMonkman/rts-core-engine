-- Moho string helpers (Luau: small leaf functions are cheaper than native
-- callbacks; see CLAUDE.md).
local gmatch, gsub = string.gmatch, string.gsub
local patternCache = {}

-- Splits on any character in `delimiter` (strtok semantics: empty tokens
-- skipped), into a 0-based table, as moho: STR_GetTokens('a b c', ' ') =
-- {[0]='a', [1]='b', [2]='c'}, and an empty delimiter gives the whole string
-- (dev/probes probe_api_spot). FAF walks the result with `for k, v in`, which
-- visits [0] last, in the real game as in ours.
function STR_GetTokens(str, delimiter)
    if delimiter == "" then
        return str ~= "" and { [0] = str } or {}
    end
    local pattern = patternCache[delimiter]
    if not pattern then
        pattern = "[^" .. gsub(delimiter, "(%W)", "%%%1") .. "]+"
        patternCache[delimiter] = pattern
    end
    local out, n = {}, 0
    for token in gmatch(str, pattern) do
        out[n] = token
        n = n + 1
    end
    return out
end

-- moho Basename(fullPath, stripExtension): the last component of a path
-- (either slash), without its extension when asked.
function Basename(fullPath, stripExtension)
    local name = (gsub(fullPath, "^.*[/\\]", ""))
    if stripExtension then
        name = (gsub(name, "%.[^.]*$", ""))
    end
    return name
end

-- moho STR_Utf8Len(str): the characters (not bytes) in a UTF-8 string;
-- bytes when it isn't valid UTF-8.
function STR_Utf8Len(str)
    return utf8.len(str) or string.len(str)
end

-- moho STR_Utf8SubString(str, start, count): `count` characters from the
-- `start`th (1-based), clipped to the string.
function STR_Utf8SubString(str, start, count)
    local len = string.len(str)
    local i = utf8.offset(str, start) or (len + 1)
    local j = utf8.offset(str, start + count) or (len + 1)
    return string.sub(str, i, j - 1)
end

-- moho STR_itox(int): upper-case hex, as a 32-bit unsigned value
-- (STR_itox(255) = "FF", STR_itox(-1) = "FFFFFFFF": dev/probes probe_api_spot).
function STR_itox(int)
    return string.format("%X", bit32.band(int, 0xFFFFFFFF))
end

-- moho STR_xtoi(str): the leading hex digits as a signed 32-bit integer, 0
-- when there are none; leading spaces aren't skipped (STR_xtoi('ffffffff')
-- = -1, STR_xtoi(' 1f') = 0: dev/probes probe_api_spot).
function STR_xtoi(str)
    local digits = string.match(str, "^(%x+)")
    if not digits then return 0 end
    local n = bit32.band(tonumber(string.sub(digits, -8), 16), 0xFFFFFFFF)
    if n >= 0x80000000 then n = n - 0x100000000 end
    return n
end

-- Lua 5.0's _TRACEBACK(...) (ldblib's errorfb, moho keeps it as a global):
-- the arguments concatenated, then the stack traceback. A first argument
-- that isn't a string or number is handed back unchanged. FAF passes a level
-- first (`WARN(_TRACEBACK(2, msg))`); 5.0's errorfb doesn't read one.
function _TRACEBACK(...)
    local n = select('#', ...)
    local first = ...
    if n > 0 and type(first) ~= "string" and type(first) ~= "number" then
        return (select(n, ...))
    end
    local parts = {}
    for i = 1, n do
        parts[i] = tostring((select(i, ...)))
    end
    return debug.traceback(table.concat(parts) .. "\nstack traceback:", 2)
end
