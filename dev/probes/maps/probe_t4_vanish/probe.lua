-- Differential probe: what happens to an unfinished unit whose builder stops.
--   real game (Windows): dev/probes/run_faf.ps1 probe_t4_vanish 120
--   ours: --headless --no-ai --ticks 700 --script dev/probes/maps/probe_t4_vanish/probe.lua
-- A Seraphim T3 engineer starts an Ahwassa (xsa0402) at each site; then:
-- CBCLR   IssueClearCommands inside the engineer's OnStartBuild callback (fraction 0)
-- CBRE    the same, then the same IssueBuildMobile again at the same spot
-- THRE    a thread that sees the focus first: clear, then the same build again (RNG's loop)
-- THCLR   a thread that sees the focus first: clear only
-- CLR5    clear 5 ticks after the start; CLR50 50 ticks after
-- MOVE5   a move order 5 ticks after the start
-- KILL5   the engineer killed 5 ticks after the start
-- GENCLR  a T1 engineer's power generator, cleared inside OnStartBuild
-- Lines: PROBE <tick> <case> <event> ..., ticks from the case's start.
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local A1 = 1
local brain = ArmyBrains[A1]
local function make(bp, x, z)
    return CreateUnitHPR(bp, A1, x, GetTerrainHeight(x, z), z, 0, 0, 0)
end
local function clearProps(x, z)
    local ok, list = pcall(GetReclaimablesInRect, Rect(x - 25, z - 25, x + 25, z + 25))
    if not ok or not list then return end
    for _, r in list do
        if IsProp(r) and not r:BeenDestroyed() then r:Destroy() end
    end
end
for i = 0, 3 do
    make('ueb1105', 608 + 4 * i, 80)
    make('ueb1106', 608 + 4 * i, 86)
end
brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)

local cases = {}
local function info(c)
    local u = c.target
    if not u then return 'none' end
    if u:BeenDestroyed() then return 'destroyed' end
    local e = c.eng
    local f = (not e:BeenDestroyed()) and e:GetFocusUnit()
    local same = f and (f == u) or false
    local other = f and not same and f:GetEntityId() or '-'
    return fmt('frac %.4f hp %.1f focus_same %s other %s building %s', u:GetFractionComplete(), u:GetHealth(), tostring(same),
        tostring(other), tostring((not e:BeenDestroyed()) and e:IsUnitState('Building')))
end
local function P(c, text)
    LOG(fmt('PROBE %d %s %s', c.t0 and (GetGameTick() - c.t0) or -1, c.name, text))
end
local function hookTarget(c, u)
    for _, n in { 'OnDecayed', 'OnDestroy', 'OnFailedToBeBuilt', 'OnKilled', 'OnStopBeingBuilt' } do
        local name = n
        local orig = u[name]
        u[name] = function(self, a, b, d)
            P(c, fmt('target %s frac %.4f', name, self:GetFractionComplete()))
            return orig(self, a, b, d)
        end
    end
end
local function newCase(name, x, z, engBp, bp, onStart)
    clearProps(x, z)
    local dz = (engBp == 'xsl0309') and 14 or 5
    local e = make(engBp, x, z + dz)
    local c = { name = name, eng = e, x = x, z = z, bp = bp }
    e:AddUnitCallback(function(self, built)
        if c.target then
            if built ~= c.target then P(c, fmt('OnStartBuild again, new unit (old %s)', info(c))) c.second = built end
            if built == c.target then P(c, 'OnStartBuild again, same unit') end
            return
        end
        c.target = built
        c.t0 = GetGameTick()
        hookTarget(c, built)
        P(c, fmt('started %s', info(c)))
        if onStart then onStart(c) end
    end, 'OnStartBuild')
    table.insert(cases, c)
    IssueBuildMobile({ e }, { x, GetTerrainHeight(x, z), z }, bp, {})
    return c
end
local function rebuild(c)
    IssueBuildMobile({ c.eng }, { c.x, GetTerrainHeight(c.x, c.z), c.z }, c.bp, {})
end

newCase('CBCLR', 784, 144, 'xsl0309', 'xsa0402', function(c) IssueClearCommands({ c.eng }) P(c, 'cleared in callback') end)
newCase('CBRE', 656, 160, 'xsl0309', 'xsa0402', function(c) IssueClearCommands({ c.eng }) rebuild(c) P(c, 'cleared and rebuilt in callback') end)
local thre = newCase('THRE', 736, 224, 'xsl0309', 'xsa0402')
local thclr = newCase('THCLR', 816, 224, 'xsl0309', 'xsa0402')
local clr5 = newCase('CLR5', 912, 240, 'xsl0309', 'xsa0402')
local clr50 = newCase('CLR50', 672, 304, 'xsl0309', 'xsa0402')
local move5 = newCase('MOVE5', 800, 336, 'xsl0309', 'xsa0402')
local kill5 = newCase('KILL5', 960, 336, 'xsl0309', 'xsa0402')
newCase('GENCLR', 656, 384, 'uel0105', 'ueb1101', function(c) IssueClearCommands({ c.eng }) P(c, 'cleared in callback') end)

ForkThread(function()
    local done = {}
    for tick = 1, 600 do
        for _, c in cases do
            if c.t0 then
                local age = GetGameTick() - c.t0
                if c == thre and not done[c] then done[c] = true IssueClearCommands({ c.eng }) rebuild(c) P(c, 'thread cleared and rebuilt') end
                if c == thclr and not done[c] then done[c] = true IssueClearCommands({ c.eng }) P(c, 'thread cleared') end
                if c == clr5 and age == 5 then IssueClearCommands({ c.eng }) P(c, 'cleared ' .. info(c)) end
                if c == clr50 and age == 50 then IssueClearCommands({ c.eng }) P(c, 'cleared ' .. info(c)) end
                if c == move5 and age == 5 then IssueMove({ c.eng }, { c.x + 30, GetTerrainHeight(c.x + 30, c.z + 30), c.z + 30 }) P(c, 'moved ' .. info(c)) end
                if c == kill5 and age == 5 and not c.eng:BeenDestroyed() then P(c, 'killing ' .. info(c)) c.eng:Kill() end
                if not c.over and (age <= 12 or math.mod(age, 10) == 0) then
                    local s = info(c)
                    P(c, s)
                    if s == 'destroyed' then c.over = true end
                end
            elseif tick == 100 then
                P(c, 'never started')
            end
        end
        WaitTicks(1)
    end
    LOG('PROBE end')
end)
