local function f() return 1, 2, 3 end
local t = { f(), f() }
local u = { f(), (f()) }
r = { table.getn(t), table.getn(u) }
