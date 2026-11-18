local function f(...) local g = function() return arg.n end return g() end
r = f(1, 2, 3)
