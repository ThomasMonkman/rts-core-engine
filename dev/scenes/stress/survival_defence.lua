-- Survival maps, for testing and timing: gives one army a ring of heavily
-- buffed UEF defences around its start so it outlasts the waves without
-- anyone playing, and logs a `SURVDEF` line every 30 game seconds (units per
-- army, defences left, the commander's health).
--
-- Run (any survival map from a vault; the player army builds nothing):
--   ./target/release/rts-core-engine --headless --vault <vault> --player 1 --armies 1 \
--       --map /maps/<map>/<map>_scenario.lua --script dev/scenes/stress/survival_defence.lua --ticks 18000
--
-- `SurvivalDefenceArmy` (default 1) picks the army.
local ARMY = rawget(_G, 'SurvivalDefenceArmy') or 1
local HEALTH = 20000000
local REGEN = 200000
local EXTRA_DAMAGE = 30000
local RANGE = 110

-- Through FAF's buff system, not `SetMaxHealth`: veterancy (these units kill
-- a lot) works max health out again from the blueprint and the unit's buffs.
BuffBlueprint {
    Name = 'SurvivalDefenceHealth',
    DisplayName = 'SurvivalDefenceHealth',
    BuffType = 'SURVIVALDEFENCEHEALTH',
    Stacks = 'REPLACE',
    Duration = -1,
    Affects = {
        MaxHealth = { Add = HEALTH, Mult = 1 },
        Regen = { Add = REGEN, Mult = 1 },
    },
}
local Buff = import('/lua/sim/buff.lua')

local function buffHealth(unit)
    if unit.SurvDefBuffed or unit.Dead then return end
    unit.SurvDefBuffed = true
    Buff.ApplyBuff(unit, 'SurvivalDefenceHealth')
end

local function buffWeapons(unit)
    for i = 1, unit:GetWeaponCount() do
        local weapon = unit:GetWeapon(i)
        weapon:AddDamageMod(EXTRA_DAMAGE)
        local bp = weapon:GetBlueprint()
        if bp.MaxRadius and bp.MaxRadius < RANGE and bp.RangeCategory ~= 'UWRC_Countermeasure' then
            weapon:ChangeMaxRadius(RANGE)
        end
        if bp.RateOfFire then weapon:ChangeRateOfFire(bp.RateOfFire * 3) end
    end
end

-- Blueprint, how many, ring radius.
local RINGS = {
    { 'ueb1301', 4, 6 },    -- T3 power (the radar and the missile defences draw energy)
    { 'ueb2304', 8, 10 },   -- T3 anti-air missiles
    { 'xeb2306', 12, 14 },  -- T3 point defence
    { 'ueb2204', 8, 17 },   -- T2 flak
    { 'ueb2301', 16, 20 },  -- T2 point defence
    { 'ueb4201', 4, 23 },   -- tactical missile defence
    { 'ueb2205', 4, 23 },   -- torpedo launchers (placed only on water)
}

ForkThread(function()
    -- After the map's own start-up threads have placed their units.
    WaitSeconds(2)
    local brain = ArmyBrains[ARMY]
    local sx, sz = brain:GetArmyStartPos()
    local defences = {}
    local function place(bp, x, z)
        local water = GetSurfaceHeight(x, z) > GetTerrainHeight(x, z)
        if (bp == 'ueb2205') ~= water then return end
        local unit = CreateUnitHPR(bp, ARMY, x, GetSurfaceHeight(x, z), z, 0, 0, 0)
        buffHealth(unit)
        buffWeapons(unit)
        table.insert(defences, unit)
    end
    for r, ring in ipairs(RINGS) do
        local bp, count, radius = ring[1], ring[2], ring[3]
        for i = 1, count do
            local a = (i + r * 0.5) / count * 2 * math.pi
            place(bp, math.floor(sx + math.cos(a) * radius) + 0.5, math.floor(sz + math.sin(a) * radius) + 0.5)
        end
    end
    place('ueb3104', sx + 3.5, sz + 3.5) -- T3 omni radar: targets for the long ranges
    place('ueb4302', sx - 3.5, sz - 3.5) -- strategic missile defence
    brain:GiveResource('ENERGY', 1000000)
    brain:GiveResource('MASS', 100000)
    LOG('SURVDEF placed ', table.getn(defences), ' defences for army ', ARMY, ' at ', sx, ',', sz)

    local seconds = 2
    while true do
        -- The commander, and whatever a map script gives the army later,
        -- and the allied civilians (a map's "defend this" object).
        for index, other in ipairs(ArmyBrains) do
            if index == ARMY or IsAlly(ARMY, index) then
                for _, unit in ipairs(other:GetListOfUnits(categories.ALLUNITS, false)) do
                    buffHealth(unit)
                end
            end
        end
        if math.mod(seconds - 2, 30) == 0 then
            local line = ''
            for index, other in ipairs(ArmyBrains) do
                line = line .. ' ' .. other.Name .. '=' .. other:GetCurrentUnits(categories.ALLUNITS)
            end
            local alive = 0
            for _, unit in ipairs(defences) do
                if not unit.Dead then alive = alive + 1 end
            end
            local acu = brain:GetListOfUnits(categories.COMMAND, false)[1]
            LOG('SURVDEF t=', math.floor(GetGameTimeSeconds()), ' units:', line, ' defences=', alive, '/', table.getn(defences),
                ' acu=', acu and math.floor(acu:GetHealth()) or 'none',
                ' defeated=', tostring(brain:IsDefeated()))
        end
        WaitSeconds(5)
        seconds = seconds + 5
    end
end)
