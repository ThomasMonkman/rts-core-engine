local fns = {}
for k, v in ipairs({ 10, 20, 30 }) do fns[k] = function() return v end end
r = { fns[1](), fns[2](), fns[3]() }
