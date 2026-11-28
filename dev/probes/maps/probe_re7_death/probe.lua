-- Differential probe (sweep 7): when a killed unit is destroyed and its platoon ends, per kind.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re7_death 110
--   ours: --headless --no-ai --ticks 420 --script dev/probes/maps/probe_re7_death/probe.lua
-- Each unit is made in its own unnamed platoon (land kinds on dry ground, ships in deep water,
-- air units flying over land and, one bomber, over water), then killed with Kill() 10 beats later.
-- IN  at the kill: layer, water depth, the death inputs Unit.lua's DeathThread reads
--     (DestructionExplosionWaitDelayMin/Max, DeathThreadDestructionWaitTime, ShallSink, NAVAL,
--     Display.AnimationDeath, SizeX/SizeZ, fraction complete).
-- ST  per beat, on change: Dead, BeenDestroyed, PlatoonExists, platoon unit count, Sinking,
--     DeathAnimManip, layer. Y every 10 beats while not destroyed.
-- AN  the death animator's rate and animation duration (FAF picks one of the blueprint's
--     AnimationDeath entries and a rate in AnimationRateMin..Max at random).
-- OK / PD  the unit's OnKilled callback and the platoon's destroy callback.
local fmt = string.format
for _, b in ArmyBrains do
    for _, u in b:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function P(text) LOG(fmt('PROBE %d %s', GetGameTick(), text)) end
local brain = ArmyBrains[1]
local CASES = {
    { 'tank', 'uel0201', 950, 150 },
    { 't3land', 'uel0303', 950, 200 },
    { 'pd', 'ueb2101', 950, 250 },
    { 'bomber', 'uea0103', 950, 300 },
    { 'gunship', 'uea0203', 950, 350 },
    { 'frigate', 'ues0103', 760, 700 },
    { 'destroyer', 'ues0201', 820, 760 },
    { 'battleship', 'ues0302', 880, 820 },
    { 'sub', 'ues0203', 760, 820 },
    { 'bomber_w', 'uea0103', 880, 700 },
}
local cases = {}
local function num(v) return type(v) == 'number' and fmt('%.2f', v) or tostring(v) end
ForkThread(function()
    WaitTicks(5)
    for _, c in CASES do
        local tag, bp, x, z = c[1], c[2], c[3], c[4]
        local u = CreateUnitHPR(bp, 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0)
        u:SetFireState(1)
        local p = brain:MakePlatoon('', 'none')
        brain:AssignUnitsToPlatoon(p, { u }, 'Attack', 'None')
        p:AddDestroyCallback(function() P(fmt('PD %s platoon destroy callback', tag)) end)
        u:AddUnitCallback(function() P(fmt('OK %s OnKilled', tag)) end, 'OnKilled')
        P(fmt('MK %s %s at %d %d terrain=%.2f water=%.2f', tag, bp, x, z, GetTerrainHeight(x, z), GetSurfaceHeight(x, z) - GetTerrainHeight(x, z)))
        table.insert(cases, { tag = tag, u = u, p = p, last = '' })
    end
    WaitTicks(10)
    for _, c in cases do
        local u = c.u
        local b = u:GetBlueprint()
        local anim = b.Display and b.Display.AnimationDeath
        local pos = u:GetPosition()
        P(fmt('IN %s layer=%s y=%.2f depth=%.2f min=%s max=%s wait=%s sink=%s naval=%s structure=%s anim=%s size=%s/%s frac=%s motion=%s',
            c.tag, tostring(u.Layer), pos[2], GetSurfaceHeight(pos[1], pos[3]) - GetTerrainHeight(pos[1], pos[3]),
            num(u.DestructionExplosionWaitDelayMin), num(u.DestructionExplosionWaitDelayMax), num(u.DeathThreadDestructionWaitTime),
            tostring(u:ShallSink()), tostring(EntityCategoryContains(categories.NAVAL, u)), tostring(EntityCategoryContains(categories.STRUCTURE, u)),
            anim and tostring(table.getn(anim)) or 'nil', num(b.SizeX), num(b.SizeZ), num(u:GetFractionComplete()),
            tostring(b.Physics and b.Physics.MotionType)))
        c.killed = GetGameTick()
        u:Kill()
        P(fmt('KL %s', c.tag))
    end
    for i = 0, 380 do
        local open = false
        for _, c in cases do
            local u, p = c.u, c.p
            local gone = u:BeenDestroyed()
            local n = table.getn(p:GetPlatoonUnits() or {})
            local s = fmt('dead=%s destroyed=%s exists=%s units=%d', tostring(u.Dead), tostring(gone), tostring(brain:PlatoonExists(p)), n)
            if not gone then
                s = s .. fmt(' sinking=%s anim=%s layer=%s', tostring(u.Sinking), tostring(u.DeathAnimManip ~= nil), tostring(u.Layer))
                local m = u.DeathAnimManip
                if m and not c.manip then
                    c.manip = true
                    local okr, rate = pcall(function() return m:GetRate() end)
                    local okd, dur = pcall(function() return m:GetAnimationDuration() end)
                    P(fmt('AN %s +%d rate=%s duration=%s', c.tag, GetGameTick() - c.killed, okr and num(rate) or 'ERR', okd and num(dur) or 'ERR'))
                end
                if math.mod(i, 10) == 0 then
                    local wb = u:GetBlueprint().WatchBone or 0
                    local okb, bp = pcall(function() return u:GetPosition(wb)[2] end)
                    P(fmt('Y %s +%d y=%.2f watch=%s', c.tag, GetGameTick() - c.killed, u:GetPosition()[2], okb and fmt('%.2f', bp) or 'ERR'))
                end
            end
            if s ~= c.last then P(fmt('ST %s +%d %s', c.tag, GetGameTick() - c.killed, s)) c.last = s end
            if not gone or brain:PlatoonExists(p) then open = true end
        end
        if not open then break end
        WaitTicks(1)
    end
    LOG('PROBE done')
end)
