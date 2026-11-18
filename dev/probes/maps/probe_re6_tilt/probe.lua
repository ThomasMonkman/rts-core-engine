-- Differential probe (sweep 6): how far structures lean on sloped ground, and
-- what orientation an upgrade ends up with.
--   real game (Windows): dev/probes/run_faf.ps1 probe_re6_tilt 150
--   ours: --headless --no-ai --ticks 900 --script dev/probes/maps/probe_re6_tilt/probe.lua
-- Sites are on the slopes of Seton's Clutch's south-west (x 16-150, z 780-930);
-- *_gentle sites slope about 0.1 (inside MaxGroundVariation for an engineer
-- build), FLAT (88, 650) is level.
-- Lines: PROBE <tick> <tag> <text>.
-- BP    the blueprint fields FAF's StructureUnit.OnCreate tilts by
-- GROUND heights at the site's centre and at +-2 / +-4 (corners and edges)
--        before anything is made, and again after (FlattenSkirt reshapes it)
-- U     a unit's position, raw GetOrientation() q1..q4, GetHeading, and the
--        lean (angle of its up axis from vertical) read both ways:
--        leanA treats q as {x,y,z,w}, leanB as {w,x,y,z}
-- Groups: C_* made with CreateUnitHPR; E_* built by an engineer
--   (IssueBuildMobile); UPG a created urb4202 upgraded to urb4204.
local fmt = string.format
for _, brain in ArmyBrains do
    for _, u in brain:GetListOfUnits(categories.ALLUNITS, false) or {} do
        if not u:BeenDestroyed() then u:Destroy() end
    end
end
local t0 = GetGameTick()
local function P(tag, text) LOG(fmt('PROBE %d %s %s', GetGameTick() - t0, tag, text)) end
local H = GetTerrainHeight
local brain = ArmyBrains[1]
local function feed()
    brain:GiveStorage('MASS', 100000) brain:GiveStorage('ENERGY', 1000000)
    brain:GiveResource('MASS', 100000) brain:GiveResource('ENERGY', 1000000)
end
feed()
for k = 0, 3 do CreateUnitHPR('urb1106', 1, 300 + 3 * k, H(300 + 3 * k, 300), 300, 0, 0, 0) end
for k = 0, 3 do CreateUnitHPR('urb1105', 1, 300 + 3 * k, H(300 + 3 * k, 306), 306, 0, 0, 0) end

local function S(v) if v == nil then return '-' end if type(v) == 'number' then return fmt('%.3f', v) end return tostring(v) end
local function bpline(id)
    local bp = __blueprints[id]
    local ph = bp.Physics or {}
    local fp = bp.Footprint or {}
    local d = bp.Display or {}
    P('BP ' .. id, fmt('Flatten %s HorizSkirt %s AltToTerrain %s StandUpright %s AlwaysAlign %s SlopeToTerrain %s TiltTwoAxis %s MaxGroundVar %s Skirt %s,%s off %s,%s Footprint %s,%s Tarmacs %s',
        S(ph.FlattenSkirt), S(ph.HorizontalSkirt), S(ph.AltitudeToTerrain), S(ph.StandUpright), S(ph.AlwaysAlignToTerrain), S(ph.SlopeToTerrain), S(ph.TiltTwoAxis),
        S(ph.MaxGroundVariation), S(ph.SkirtSizeX), S(ph.SkirtSizeZ), S(ph.SkirtOffsetX), S(ph.SkirtOffsetZ), S(fp.SizeX), S(fp.SizeZ), S(d.TarmacsEnabled)))
end
local function ground(tag, x, z)
    local s = fmt('c %.3f', H(x, z))
    for _, r in { 2, 4 } do
        s = s .. fmt(' | r%d corners %.3f %.3f %.3f %.3f edges -x %.3f +x %.3f -z %.3f +z %.3f', r,
            H(x - r, z - r), H(x + r, z - r), H(x - r, z + r), H(x + r, z + r), H(x - r, z), H(x + r, z), H(x, z - r), H(x, z + r))
    end
    P('GROUND ' .. tag, s)
end
local function lean(a, b)
    local up = 1 - 2 * (a * a + b * b)
    if up > 1 then up = 1 elseif up < -1 then up = -1 end
    return math.deg(math.acos(up))
end
local function U(tag, u, extra)
    if not u or u:BeenDestroyed() then P('U ' .. tag, 'gone') return end
    local p = u:GetPosition()
    local o = u:GetOrientation()
    local okh, hd = pcall(u.GetHeading, u)
    P('U ' .. tag, fmt('%s pos %.3f %.3f %.3f q %.4f %.4f %.4f %.4f heading %s leanA %.2f leanB %.2f frac %.3f slope %s %s',
        u.UnitId or '?', p[1], p[2], p[3], o[1], o[2], o[3], o[4], okh and fmt('%.4f', hd) or 'E',
        lean(o[1], o[3]), lean(o[2], o[4]), u:GetFractionComplete(), tostring(u.TerrainSlope ~= nil), extra or ''))
    local s = u.MyShield
    if s then
        local oks, so = pcall(s.GetOrientation, s)
        local okp, sp = pcall(s.GetPosition, s)
        if oks and so and okp and sp then
            P('U ' .. tag .. ' shield', fmt('pos %.3f %.3f %.3f q %.4f %.4f %.4f %.4f leanA %.2f leanB %.2f', sp[1], sp[2], sp[3], so[1], so[2], so[3], so[4],
                lean(so[1], so[3]), lean(so[2], so[4])))
        else
            P('U ' .. tag .. ' shield', 'no orientation ' .. tostring(oks) .. ' ' .. tostring(okp))
        end
    end
end

local CREATE = {
    { 'C_urb4202', 'urb4202', 144, 876 },
    { 'C_ueb4202', 'ueb4202', 76, 860 },
    { 'C_uab4202', 'uab4202', 64, 824 },
    { 'C_urb1101', 'urb1101', 100, 832 },
    { 'C_urb2101', 'urb2101', 120, 808 },
    { 'C_urb0101', 'urb0101', 24, 864 },
    { 'C_urb4202_gentle', 'urb4202', 72, 876 },
}
local ENG = {
    { 'E_urb4202', 'urb4202', 128, 856, 'url0208' },
    { 'E_ueb4202', 'ueb4202', 56, 920, 'uel0208' },
    { 'E_urb4202_gentle', 'urb4202', 76, 784, 'url0208' },
    { 'E_urb4202_flat', 'urb4202', 76, 696, 'url0208' },
}
local UPG = { 'UPG', 'urb4202', 144, 824 }
for _, id in { 'urb4202', 'urb4204', 'ueb4202', 'uab4202', 'urb1101', 'urb2101', 'urb0101' } do bpline(id) end
for _, e in CREATE do ground(e[1] .. ' before', e[3], e[4]) end
for _, e in ENG do ground(e[1] .. ' before', e[3], e[4]) end
ground('UPG before', UPG[3], UPG[4])
-- a flat-ground reference: the same shield on a flat spot nearby
ground('FLAT before', 88, 650)

ForkThread(function()
    WaitTicks(2)
    local made = {}
    for _, e in CREATE do
        local u = CreateUnitHPR(e[2], 1, e[3], H(e[3], e[4]), e[4], 0, 0, 0)
        made[e[1]] = u
        U(e[1] .. ' +0', u)
    end
    local flat = CreateUnitHPR('urb4202', 1, 88, H(88, 650), 650, 0, 0, 0)
    U('FLAT +0', flat)
    local old = CreateUnitHPR(UPG[2], 1, UPG[3], H(UPG[3], UPG[4]), UPG[4], 0, 0, 0)
    U('UPG old +0', old)
    WaitTicks(1)
    for _, e in CREATE do U(e[1] .. ' +1', made[e[1]]) end
    WaitTicks(9)
    for _, e in CREATE do U(e[1] .. ' +10', made[e[1]]) ground(e[1] .. ' after', e[3], e[4]) end
    U('FLAT +10', flat)
    U('UPG old +10', old)
    ground('UPG after create', UPG[3], UPG[4])

    -- engineer builds
    feed()
    local engs = {}
    for _, e in ENG do
        local eng = CreateUnitHPR(e[5], 1, e[3] + 7, H(e[3] + 7, e[4]), e[4], 0, 0, 0)
        eng:SetBuildRate(200)
        IssueBuildMobile({ eng }, { e[3], H(e[3], e[4]), e[4] }, e[2], {})
        engs[e[1]] = { eng = eng }
    end
    for i = 0, 200 do
        local all = true
        for _, e in ENG do
            local r = engs[e[1]]
            local f = r.eng:GetFocusUnit()
            if f and not r.unit then r.unit = f U(e[1] .. ' first', f, 'at +' .. i) ground(e[1] .. ' at start', e[3], e[4]) end
            if r.unit and not r.unit:BeenDestroyed() and r.unit:GetFractionComplete() >= 1 then
                if not r.done then r.done = i U(e[1] .. ' complete', r.unit, 'at +' .. i) end
            else
                all = false
            end
            if i == 60 and not r.unit then P('E ' .. e[1], 'no unit after 60 ticks; idle ' .. tostring(r.eng:IsIdleState())) end
        end
        if all then break end
        if i == 120 then break end
        WaitTicks(1)
    end
    WaitTicks(10)
    for _, e in ENG do
        U(e[1] .. ' +10 after', engs[e[1]].unit)
        ground(e[1] .. ' after', e[3], e[4])
    end

    -- upgrade
    feed()
    old:SetBuildRate(300)
    local new
    local o = old.OnStartBuild
    old.OnStartBuild = function(self, unit, order)
        new = unit
        U('UPG new OnStartBuild', unit, tostring(order))
        if o then return o(self, unit, order) end
    end
    U('UPG old before', old)
    IssueUpgrade({ old }, 'urb4204')
    local done
    for i = 0, 300 do
        if new and not new:BeenDestroyed() then
            if i <= 3 or math.mod(i, 20) == 0 then U('UPG new +' .. i, new) end
            if new:GetFractionComplete() >= 1 and not done then done = i U('UPG new complete', new, 'at +' .. i) end
        end
        if done and i > done + 15 then break end
        WaitTicks(1)
    end
    U('UPG new end', new)
    U('UPG old end', old)
    ground('UPG end', UPG[3], UPG[4])
    for _, e in CREATE do U(e[1] .. ' end', made[e[1]]) end
    U('FLAT end', flat)
    WaitTicks(5)
    LOG('PROBE done')
end)
