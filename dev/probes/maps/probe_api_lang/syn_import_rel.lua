local ok, m = pcall(import, "syn_neq_tilde.lua")
local ok2, m2 = pcall(import, "./syn_neq_tilde.lua")
local ok3, m3 = pcall(import, "../probe_api_lang/syn_neq_tilde.lua")
r = { ok, ok2, ok3 }
