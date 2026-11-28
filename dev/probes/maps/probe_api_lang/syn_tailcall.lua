local function d(n) if n == 0 then return 0 end return d(n - 1) end
local ok, v = pcall(d, 1000000)
r = { ok, v }
