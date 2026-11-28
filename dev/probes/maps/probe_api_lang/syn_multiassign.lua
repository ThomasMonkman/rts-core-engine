local a, b, c = 1, 2
local t = { 1, 2 }
local i = 1
i, t[i] = 2, 9
r = { a, b, c, t[1], t[2], i }
