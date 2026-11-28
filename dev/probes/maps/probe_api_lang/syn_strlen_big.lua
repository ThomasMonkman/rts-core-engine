local s = string.rep("x", 100000)
r = { string.len(s), string.len(s .. s) }
