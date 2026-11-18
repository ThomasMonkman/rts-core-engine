local t = { v = 5 }
function t:get(x) return self.v + (x or 0) end
function t.stat(x) return x end
r = { t:get(), t:get(1), t.get(t, 2), t.stat(3) }
