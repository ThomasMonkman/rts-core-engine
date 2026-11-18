local fns = {}
local i = 1
while i <= 3 do local j = i fns[i] = function() return j end i = i + 1 end
r = { fns[1](), fns[2](), fns[3]() }
