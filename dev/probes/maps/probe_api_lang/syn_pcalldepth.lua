local function d(n) if n == 0 then return 0 end local ok, v = pcall(d, n - 1) if not ok then error(v, 0) end return v + 1 end
local ok, v = pcall(d, 150)
local ok2, v2 = pcall(d, 250)
r = { ok, v, ok2, ok2 and v2 or "err" }
