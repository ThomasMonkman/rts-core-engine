local function f(a, ...) return a, arg.n, arg[1] end
r = { f(1, 2, 3) }
