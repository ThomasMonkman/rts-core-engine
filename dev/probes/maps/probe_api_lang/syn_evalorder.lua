local s = ""
local function f(x) s = s .. x return x end
local t = { f("a"), f("b"), [f("c")] = f("d") }
local _ = f("e") .. f("f")
r = s
