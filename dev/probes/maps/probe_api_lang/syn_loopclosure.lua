local fns = {}
for i = 1, 3 do fns[i] = function() return i end end
r = { fns[1](), fns[2](), fns[3]() }
