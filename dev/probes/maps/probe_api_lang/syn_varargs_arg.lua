local function f(...) return arg.n, arg[1], arg[2] end
r = { f(7, nil, 9) }
