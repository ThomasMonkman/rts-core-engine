local e = getfenv(1)
local mt = getmetatable(e)
r = { type(mt), mt and type(mt.__index), mt and type(mt.__newindex), mt and mt.__index == _G }
