local airRatio = 1.25
local groundRatio = 0.4
local v = { 3, 4, 0 }
local range = 30.5
local base, exp = 1.5, 3
r = { (airRatio - 0.5) ^ 2, groundRatio ^ 2, math.sqrt(v[1]^2 + v[2]^2 + v[3]^2), range^2, base ^ exp, 7.9 ^ 0 }
