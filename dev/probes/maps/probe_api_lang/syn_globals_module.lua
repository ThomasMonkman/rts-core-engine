x_module_var = 5
local function f() return x_module_var end
r = { f(), rawget(_G, "x_module_var"), getfenv(1) == _G, type(getfenv(1)) }
