local function d(n) if n == 0 then return 0 end return 1 + d(n - 1) end
local ok, v = pcall(d, 150)
local ok2, v2 = pcall(d, 5000)
local ok3, v3 = pcall(d, 100000)
r = { ok, v, ok2, ok2 and v2 or "err", ok3, ok3 and v3 or "err" }
