-- probe_re3_intel: jammer timing (J12), intel start-up, flying aircraft and
-- naval jammer contacts (shots), remembered structures after radar loss.
-- Army 1 (focus) observes with a T3 radar R at (800,100) (radar 600, omni
-- 200); army 2 (hold fire): jamming frigate F (530,210), aircraft patrolling
-- around z 430, a base around (640,340) (pgen, PD, tank, Fatboy) more than
-- 200 from R (no omni), and a pgen2 (690,320) never seen.
-- Ticks (absolute): 1..25 per-tick start-up; 60 shots (radar phase);
-- 100 army 1 scout E at (640,352) (vision 24: sees pgen, PD, tank, Fatboy);
-- 133 F's jammer off, 137 on; 140 E destroyed; 160 the tank moves 30 east;
-- 175 shots (after); 200 R destroyed; 215 the tank moves back; 250 shots (lost).
--   real game (Windows): dev/probes/run_faf.ps1 probe_re3_intel 150
--   ours: --headless --no-ai --ticks 400 --script dev/probes/maps/probe_re3_intel/probe.lua
local armies = ListArmies()
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local fmt = string.format
local OBS, OWN = 1, 2
SetAlliance(armies[OBS], armies[OWN], 'Enemy')
SetAlliance(armies[OWN], armies[OBS], 'Enemy')
local brain = ArmyBrains[OBS]
for _, i in { OBS, OWN } do
    ArmyBrains[i]:GiveStorage('MASS', 50000) ArmyBrains[i]:GiveStorage('ENERGY', 500000)
end
ForkThread(function()
    while true do
        for _, i in { OBS, OWN } do
            ArmyBrains[i]:GiveResource('MASS', 50000) ArmyBrains[i]:GiveResource('ENERGY', 500000)
        end
        WaitTicks(5)
    end
end)
local function P(s) LOG('PROBE sim ' .. GetGameTick() .. ' ' .. s) end
local function N(v)
    if type(v) ~= 'number' then return tostring(v) end
    if math.abs(v) < 0.005 then v = 0 end
    return fmt('%.2f', v)
end
local function B(f) local ok, v = pcall(f) if not ok then return 'E' end return v end
local function flag(v, c) if v == 'E' then return '?' end return v and c or '.' end
local function hold(u) if u and u.SetFireState then pcall(u.SetFireState, u, 1) end end
local function make(bp, army, x, z, y)
    local u = CreateUnitHPR(bp, armies[army], x, y or GetTerrainHeight(x, z), z, 0, 0, 0)
    hold(u)
    return u
end

local frig = make('ues0103', OWN, 530, 210, GetSurfaceHeight(530, 210))
local radar = make('ueb3104', OBS, 800, 100)
local U = {
    { 'pgen', 'ueb1101', 630.5, 340.5 },
    { 'pd', 'ueb2101', 645.5, 340.5 },
    { 'tank', 'uel0201', 622, 355 },
    { 'fatboy', 'uel0401', 660, 358 },
    { 'pgen2', 'ueb1101', 690.5, 320.5 },
    { 'fighter', 'uea0102', 600, 430, 'air' },
    { 'bomber', 'uea0103', 640, 440, 'air' },
    { 'gunship', 'uea0203', 680, 425, 'air' },
}
local units = {}
for _, d in U do
    local u = make(d[2], OWN, d[3], d[4])
    units[d[1]] = u
    P('made ' .. d[1] .. ' id=' .. u:GetEntityId() .. ' terrain ' .. N(GetTerrainHeight(d[3], d[4])) .. ' surface ' .. N(GetSurfaceHeight(d[3], d[4])))
end
units.frig = frig
local FID = frig:GetEntityId()
P('made frig id=' .. FID .. ' radar id=' .. radar:GetEntityId())

-- every intel change army 1 hears, with its tick (first 120)
local heard, seenFake = 0, {}
local function srcName(blip)
    local okS, src = pcall(function() return blip:GetSource() end)
    if not okS or not src then return '-' end
    local id = B(function() return src:GetEntityId() end)
    for k, u in units do if B(function() return u:GetEntityId() end) == id then return k end end
    return tostring(id)
end
local orig = brain.OnIntelChange
brain.OnIntelChange = function(self, blip, reconType, val)
    heard = heard + 1
    if heard <= 160 then
        local okp, p = pcall(function() return blip:GetPosition() end)
        local s = srcName(blip)
        local own = ''
        if s == 'frig' and okp and p then
            local d = math.sqrt((p[1] - 530) * (p[1] - 530) + (p[3] - 210) * (p[3] - 210))
            own = d < 0.01 and ' self' or (' fake d=' .. N(d))
        end
        P('ic ' .. s .. own .. ' ' .. tostring(reconType) .. '=' .. tostring(val))
    end
    if orig then return orig(self, blip, reconType, val) end
end

-- one unit's blip for army 1: flags R radar, S seen now, E seen ever, O omni, M maybe dead
local kept = {}
local function bflags(b)
    if B(function() return b:BeenDestroyed() end) == true then return 'D' end
    return flag(B(function() return b:IsOnRadar(OBS) end), 'R') .. flag(B(function() return b:IsSeenNow(OBS) end), 'S')
        .. flag(B(function() return b:IsSeenEver(OBS) end), 'E') .. flag(B(function() return b:IsOnOmni(OBS) end), 'O')
        .. flag(B(function() return b:IsMaybeDead(OBS) end), 'M')
end
local function blipLine(tag)
    for _, d in { 'pgen', 'pd', 'tank', 'fatboy', 'pgen2', 'fighter', 'frig' } do
        local u = units[d]
        local s = tag .. ' ' .. d
        if u:BeenDestroyed() then
            s = s .. ' unit gone'
        else
            local up = u:GetPosition()
            s = s .. ' at ' .. N(up[1]) .. ',' .. N(up[3])
            local b = B(function() return u:GetBlip(OBS) end)
            if type(b) == 'table' or type(b) == 'userdata' then
                kept[d] = b
                local okp, p = pcall(function() return b:GetPosition() end)
                s = s .. ' GetBlip ' .. bflags(b) .. ' pos ' .. (okp and p and (N(p[1]) .. ',' .. N(p[3])) or '?')
            else
                s = s .. ' GetBlip ' .. tostring(b)
            end
        end
        local k = kept[d]
        if k then
            local okp, p = pcall(function() return k:GetPosition() end)
            s = s .. ' | kept ' .. bflags(k) .. ' pos ' .. (okp and p and (N(p[1]) .. ',' .. N(p[3])) or '?')
        end
        P(s)
    end
end

ForkThread(function()
    local eye
    while GetGameTick() < 330 do
        WaitTicks(1)
        local t = GetGameTick()
        if t <= 25 then
            P(fmt('start radar=%s omni=%s jam=%s heard=%d', tostring(B(function() return radar:IsIntelEnabled('Radar') end)), tostring(B(function() return radar:IsIntelEnabled('Omni') end)),
                tostring(B(function() return frig:IsIntelEnabled('Jammer') end)), heard))
        end
        if t == 30 or t == 90 or t == 120 or t == 150 or t == 170 or t == 195 or t == 205 or t == 230 or t == 260 or t == 320 then blipLine('b' .. t) end
        if t == 100 then eye = make('uel0101', OBS, 640, 352) P('do eye') end
        if t == 133 then P('do jam off ' .. tostring(B(function() frig:DisableIntel('Jammer') return true end))) end
        if t == 137 then P('do jam on ' .. tostring(B(function() frig:EnableIntel('Jammer') return true end))) end
        if t == 140 and eye then eye:Destroy() P('do eye gone') end
        if t == 160 then IssueMove({ units.tank }, { 652, GetTerrainHeight(652, 375), 375 }) P('do tank east') end
        if t == 200 then radar:Destroy() P('do radar gone') end
        if t == 215 then IssueMove({ units.tank }, { 622, GetTerrainHeight(622, 355), 355 }) P('do tank back') end
        if t == 5 then
            for _, k in { 'fighter', 'bomber', 'gunship' } do
                local u = units[k]
                local x, y, z = u:GetPositionXYZ()
                IssuePatrol({ u }, { x + 40, GetTerrainHeight(x + 40, z), z })
                IssuePatrol({ u }, { x, GetTerrainHeight(x, z), z })
            end
        end
        if t == 58 or t == 172 then
            for _, k in { 'fighter', 'bomber', 'gunship' } do
                local x, y, z = units[k]:GetPositionXYZ()
                P('air ' .. k .. ' ' .. N(x) .. ' ' .. N(y) .. ' ' .. N(z) .. ' ground ' .. N(GetTerrainHeight(x, z)))
            end
        end
    end
    WaitTicks(30)
    LOG('PROBE sim end')
end)
