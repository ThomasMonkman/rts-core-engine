-- AI growth census (Uveso slow-growth investigation): every 500 ticks, per AI army, what it
-- has and its economy; at the end, which builders each army's managers picked how often.
--   real game: dev/probes/run_faf.ps1 probe_uv_mixed <secs>   (launch_args.txt sets the AIs and the mod)
--   ours: --headless --session --ticks 8001 --ai <key> [--mod ..] --script dev/probes/maps/probe_uv_mixed/probe.lua
local fmt = string.format
local EVERY, LAST = 500, tonumber(rawget(_G, 'PROBE_LAST_TICK') or 8000)
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end

local C = categories
local kinds = {
    {'eng1', C.ENGINEER * C.TECH1 - C.COMMAND}, {'eng2', C.ENGINEER * C.TECH2 - C.COMMAND},
    {'eng3', C.ENGINEER * C.TECH3 - C.COMMAND - C.SUBCOMMANDER}, {'sacu', C.SUBCOMMANDER},
    {'fL1', C.FACTORY * C.LAND * C.TECH1 * C.STRUCTURE}, {'fL2', C.FACTORY * C.LAND * C.TECH2 * C.STRUCTURE},
    {'fL3', C.FACTORY * C.LAND * C.TECH3 * C.STRUCTURE},
    {'fA1', C.FACTORY * C.AIR * C.TECH1 * C.STRUCTURE}, {'fA2', C.FACTORY * C.AIR * C.TECH2 * C.STRUCTURE},
    {'fA3', C.FACTORY * C.AIR * C.TECH3 * C.STRUCTURE},
    {'fN1', C.FACTORY * C.NAVAL * C.TECH1 * C.STRUCTURE}, {'fN2', C.FACTORY * C.NAVAL * C.TECH2 * C.STRUCTURE},
    {'fN3', C.FACTORY * C.NAVAL * C.TECH3 * C.STRUCTURE},
    {'mx1', C.MASSEXTRACTION * C.TECH1}, {'mx2', C.MASSEXTRACTION * C.TECH2}, {'mx3', C.MASSEXTRACTION * C.TECH3},
    {'mfab', C.MASSFABRICATION * C.STRUCTURE},
    {'pw1', C.ENERGYPRODUCTION * C.STRUCTURE * C.TECH1 - C.HYDROCARBON}, {'hydro', C.HYDROCARBON},
    {'pw2', C.ENERGYPRODUCTION * C.STRUCTURE * C.TECH2 - C.HYDROCARBON}, {'pw3', C.ENERGYPRODUCTION * C.STRUCTURE * C.TECH3},
    {'mstor', C.MASSSTORAGE}, {'estor', C.ENERGYSTORAGE},
    {'def', C.DEFENSE * C.STRUCTURE}, {'land', C.MOBILE * C.LAND - C.ENGINEER - C.COMMAND},
    {'air', C.MOBILE * C.AIR}, {'navy', C.MOBILE * C.NAVAL}, {'exp', C.EXPERIMENTAL},
}

local picks = {}   -- army -> manager -> builder name -> count
local function wrapManagers(brain)
    local idx = brain:GetArmyIndex()
    picks[idx] = picks[idx] or {}
    for loc, bm in brain.BuilderManagers or {} do
        for _, mname in {'EngineerManager', 'FactoryManager', 'PlatoonFormManager'} do
            local m = bm[mname]
            if m and not rawget(m, '_probeWrapped') and m.GetHighestBuilder then
                rawset(m, '_probeWrapped', true)
                local orig = m.GetHighestBuilder
                local tally = picks[idx][mname] or {}
                picks[idx][mname] = tally
                rawset(m, 'GetHighestBuilder', function(self, a, b, c, d)
                    local r = orig(self, a, b, c, d)
                    if r and r.BuilderName then
                        tally[r.BuilderName] = (tally[r.BuilderName] or 0) + 1
                    end
                    return r
                end)
            end
        end
    end
end

local function census()
    for _, brain in ArmyBrains do
        if brain.BrainType == 'AI' and not ArmyIsCivilian(brain:GetArmyIndex()) and not brain:IsDefeated() then
            wrapManagers(brain)
            local all = brain:GetListOfUnits(C.ALLUNITS - C.WALL, false) or {}
            local done, building = 0, 0
            for _, u in all do
                if u:GetFractionComplete() >= 1 then done = done + 1 else building = building + 1 end
            end
            local parts = {}
            for _, k in kinds do
                local n = 0
                for _, u in brain:GetListOfUnits(k[2], false) or {} do
                    if u:GetFractionComplete() >= 1 then n = n + 1 end
                end
                if n > 0 then table.insert(parts, k[1] .. '=' .. n) end
            end
            local engs, idle, tasks = 0, 0, {}
            for _, u in brain:GetListOfUnits(C.ENGINEER - C.COMMAND, false) or {} do
                if u:GetFractionComplete() >= 1 then
                    engs = engs + 1
                    if u:IsIdleState() then idle = idle + 1 end
                    local name = u.PlatoonHandle and u.PlatoonHandle.BuilderName or 'none'
                    tasks[name] = (tasks[name] or 0) + 1
                end
            end
            P(fmt('C a=%d u=%d bld=%d idleEng=%d/%d %s', brain:GetArmyIndex(), done, building, idle, engs, table.concat(parts, ' ')))
            P(fmt('E a=%d mInc=%.1f eInc=%.1f mReq=%.1f eReq=%.1f mTr=%.1f eTr=%.1f mSt=%.0f eSt=%.0f mR=%.2f eR=%.2f',
                brain:GetArmyIndex(),
                brain:GetEconomyIncome('MASS') * 10, brain:GetEconomyIncome('ENERGY') * 10,
                brain:GetEconomyRequested('MASS') * 10, brain:GetEconomyRequested('ENERGY') * 10,
                brain:GetEconomyTrend('MASS') * 10, brain:GetEconomyTrend('ENERGY') * 10,
                brain:GetEconomyStored('MASS'), brain:GetEconomyStored('ENERGY'),
                brain:GetEconomyStoredRatio('MASS'), brain:GetEconomyStoredRatio('ENERGY')))
            local t = {}
            for name, n in tasks do table.insert(t, name .. ':' .. n) end
            table.sort(t)
            P(fmt('T a=%d %s', brain:GetArmyIndex(), table.concat(t, ' ')))
        end
    end
end

local function dumpPicks()
    for idx, mans in picks do
        for mname, tally in mans do
            local t = {}
            for name, n in tally do table.insert(t, {name, n}) end
            table.sort(t, function(a, b) return a[2] > b[2] or (a[2] == b[2] and a[1] < b[1]) end)
            local s = {}
            for _, e in t do table.insert(s, e[1] .. ':' .. e[2]) end
            P(fmt('B a=%d %s %s', idx, mname, table.concat(s, ' ')))
        end
    end
end

ForkThread(function()
    local t0 = GetGameTick()
    WaitTicks(1)
    for _, brain in ArmyBrains do
        if brain.BrainType == 'AI' then wrapManagers(brain) end
    end
    while true do
        local t = GetGameTick() - t0
        if math.mod(t, EVERY) == 0 or t == 1 then census() end
        if math.mod(t, 2000) == 0 and t > 0 then dumpPicks() end
        if t >= LAST then
            P('done')
            return
        end
        WaitTicks(1)
    end
end)
P('start')
