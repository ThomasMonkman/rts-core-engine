-- Differential probe: a Fatboy (uel0401) building on land and on the seabed.
--   real game (Windows): dev/probes/run_faf.ps1 probe_fatboy_water 300
--   ours: cargo run -- --headless --no-ai --ticks 2600 --script dev/probes/maps/probe_fatboy_water/probe.lua
-- [L]  on land: a tank (uel0202) ordered through the external factory.
-- [W1] walked onto the seabed (depth ~5): CanBuild on the external factory
--      and on the Fatboy; a tank ordered through the external factory.
-- [W2] the same tank ordered on the Fatboy itself with brain:BuildUnit (how
--      FAF's AI does it, AIBehaviors.lua FatBoyBuildCheck).
-- [W3] an amphibious unit (uel0203) ordered both ways.
-- [M]  back on land a slow unit (uel0303) is started, then the Fatboy is
--      walked back into the water mid-build.
-- A line whenever the state changes: the Fatboy's layer and states, the
-- external factory's states and queue length, the unit being built (its
-- blueprint, fraction in tenths, layer) and the army's finished mobile
-- units (blueprint, layer, health in quarters).
local fmt = string.format
local armies = ListArmies()
local A1 = armies[1]
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local brain = ArmyBrains[1]
local part, T = 'pre', GetGameTick()
local function P(s) LOG(fmt('PROBE %s T+%d %s', part, GetGameTick() - T, s)) end
local function make(bp, x, z) return CreateUnitHPR(bp, A1, x, GetTerrainHeight(x, z), z, 0, 0, 0) end
local function pos(x, z) return { x, GetTerrainHeight(x, z), z } end
local LAND, SEA = { 424, 620 }, { 462, 620 }

local fatboy, ef
local function states(u)
    local s = ''
    for _, n in { 'Building', 'Moving', 'Busy', 'BlockCommandQueue', 'Attached' } do
        if u:IsUnitState(n) then s = s .. n:sub(1, 2) end
    end
    return s == '' and '-' or s
end
local function snapshot()
    local s = fmt('fat %s %s', fatboy:GetCurrentLayer(), states(fatboy))
    s = s .. fmt(' | ef %s q%d', states(ef), table.getn(ef:GetCommandQueue() or {}))
    local b = fatboy.UnitBeingBuilt
    if b and not b:BeenDestroyed() and b:GetFractionComplete() < 1 then
        s = s .. fmt(' | building %s %d/10 %s', b:GetBlueprint().BlueprintId, math.floor(b:GetFractionComplete() * 10), b:GetCurrentLayer())
    else
        s = s .. ' | building -'
    end
    local made = ''
    for _, u in brain:GetListOfUnits(categories.MOBILE * categories.LAND - categories.EXPERIMENTAL, false) or {} do
        if not u:BeenDestroyed() and u:GetFractionComplete() >= 1 then
            made = made .. fmt(' %s:%s:%d/4hp', u:GetBlueprint().BlueprintId, u:GetCurrentLayer(), math.ceil(4 * u:GetHealth() / u:GetMaxHealth()))
        end
    end
    return s .. ' | made' .. (made == '' and ' -' or made)
end
local last
-- Logs state changes for `ticks`, ending early once `stop()` says so.
local function watch(ticks, stop)
    for _ = 1, ticks do
        WaitTicks(1)
        local s = snapshot()
        if s ~= last then last = s P(s) end
        if stop and stop() then return true end
    end
    return false
end
local function begin(name) part, T, last = name, GetGameTick(), nil end
local function can()
    local s = ''
    for _, bp in { 'uel0202', 'uel0203', 'uel0303' } do
        s = s .. fmt(' %s ef:%s fat:%s', bp, tostring(ef:CanBuild(bp)), tostring(fatboy:CanBuild(bp)))
    end
    P('CanBuild' .. s)
end
local function count(bp)
    local n = 0
    for _, u in brain:GetListOfUnits(categories.MOBILE * categories.LAND - categories.EXPERIMENTAL, false) or {} do
        if not u:BeenDestroyed() and u:GetBlueprint().BlueprintId == bp and u:GetFractionComplete() >= 1 then n = n + 1 end
    end
    return n
end
local function clear()
    for _, u in brain:GetListOfUnits(categories.MOBILE * categories.LAND - categories.EXPERIMENTAL, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local function walk(to, layer)
    IssueClearCommands({ fatboy })
    IssueMove({ fatboy }, pos(to[1], to[2]))
    local ok = watch(900, function()
        local x, _, z = fatboy:GetPositionXYZ()
        return fatboy:GetCurrentLayer() == layer and VDist2(x, z, to[1], to[2]) < 3
    end)
    local x, y, z = fatboy:GetPositionXYZ()
    P(fmt('walk %s: at %.1f,%.1f,%.1f depth %.2f', ok and 'done' or 'TIMED OUT', x, y, z, GetSurfaceHeight(x, z) - GetTerrainHeight(x, z)))
    watch(20)
end

ForkThread(function()
    WaitTicks(5)
    -- A paragon pays for everything.
    make('xab1401', 380, 580)
    fatboy = make('uel0401', LAND[1], LAND[2])
    fatboy:SetCanTakeDamage(false)
    local change = function(self, new, old)
        P(fmt('OnLayerChange %s <- %s', tostring(new), tostring(old)))
        local hook = self.OnLayerChange
        self.OnLayerChange = nil
        self:OnLayerChange(new, old)
        self.OnLayerChange = hook
    end
    fatboy.OnLayerChange = change
    WaitTicks(30)
    ef = fatboy.ExternalFactory
    P(fmt('external factory %s', ef and ef:GetBlueprint().BlueprintId or 'nil'))

    begin('L')
    can()
    IssueBuildFactory({ ef }, 'uel0202', 1)
    watch(400, function() return count('uel0202') == 1 and not fatboy:IsUnitState('Building') end)
    watch(60)
    clear()

    begin('W1')
    walk(SEA, 'Seabed')
    can()
    IssueBuildFactory({ ef }, 'uel0202', 1)
    watch(300, function() return count('uel0202') == 1 end)
    watch(60)
    P(fmt('tanks made %d', count('uel0202')))
    IssueClearCommands({ ef })
    clear()

    begin('W2')
    brain:BuildUnit(fatboy, 'uel0202', 1)
    watch(300, function() return count('uel0202') == 1 end)
    watch(120)
    P(fmt('tanks made %d', count('uel0202')))
    IssueClearCommands({ fatboy })
    clear()

    begin('W3')
    IssueBuildFactory({ ef }, 'uel0203', 1)
    watch(200, function() return count('uel0203') == 1 end)
    P(fmt('through the external factory: hover tanks made %d', count('uel0203')))
    IssueClearCommands({ ef })
    brain:BuildUnit(fatboy, 'uel0203', 1)
    watch(300, function() return count('uel0203') >= 1 end)
    watch(60)
    P(fmt('on the Fatboy: hover tanks made %d', count('uel0203')))
    IssueClearCommands({ fatboy })
    clear()

    begin('M')
    walk(LAND, 'Land')
    can()
    IssueBuildFactory({ ef }, 'uel0303', 1)
    watch(300, function()
        local b = fatboy.UnitBeingBuilt
        return b and not b:BeenDestroyed() and b:GetFractionComplete() > 0.3 and b:GetFractionComplete() < 1
    end)
    P('ordered into the water mid-build')
    IssueMove({ fatboy }, pos(SEA[1], SEA[2]))
    watch(900, function()
        local x, _, z = fatboy:GetPositionXYZ()
        return fatboy:GetCurrentLayer() == 'Seabed' and VDist2(x, z, SEA[1], SEA[2]) < 3
    end)
    watch(150)
    P(fmt('siege bots made %d', count('uel0303')))
    LOG('PROBE done')
end)
