local function f(...) arg[1] = 5 return unpack(arg) end
r = { f(1, 2) }
