-- Interface half of probe_engine_stats: points the camera at sea, shore and land and logs what
-- __EngineStats' Camera / Cursor node holds there next to GetMouseWorldPos (the mouse must be over
-- the window: run the PC's cursor jiggler), then the tree's groups and a few values.
local function L(s) LOG('PROBE ui ' .. tostring(GameTick()) .. ' ' .. s) end
local function node(parent, name)
    for _, c in parent.Children or {} do
        if c.Name == name then return c end
    end
end
local function fmt(v)
    if type(v) == 'number' then return string.format('%.4f', v) end
    return tostring(v)
end
local function dumpNode(n, prefix)
    for _, c in n.Children or {} do
        L(prefix .. c.Name .. ' [' .. tostring(c.Type) .. '] = ' .. fmt(c.Value))
    end
end
-- name, x, z, zoom, pitch
local views = {
    { 'sea', 400, 330, 90, 1.2 },
    { 'shore', 490, 330, 90, 1.2 },
    { 'land', 330, 250, 90, 1.2 },
    { 'sea_low', 400, 330, 60, 0.6 },
}
local function look(v)
    GetCamera('WorldCamera'):SnapTo(Vector(v[2], 17.5, v[3]), { math.pi, v[5], 0 }, v[4])
end
look(views[1])
ForkThread(function()
    while GameTick() < 60 do WaitSeconds(0.05) end
    -- The tree's shape: groups, their types, entries.
    for _, g in __EngineStats.Children do
        local n = 0
        for _ in g.Children or {} do n = n + 1 end
        L('group ' .. g.Name .. ' [' .. tostring(g.Type) .. '] = ' .. fmt(g.Value) .. ' children=' .. n)
    end
    dumpNode(node(__EngineStats, 'EntityCount'), 'EntityCount/')
    dumpNode(node(__EngineStats, 'Heap'), 'Heap/')
    dumpNode(node(__EngineStats, 'Frame'), 'Frame/')
    local ic = node(__EngineStats, 'Instance Counts')
    for _, k in { 'CTask', 'CTaskThread', 'CScriptObject', 'CLuaTask', 'Entity', 'Prop', 'CDecalHandle', 'ReconBlip', 'Unit', 'CPlatoon', 'Projectile', 'CollisionBeamEntity', 'Shield' } do
        local c = node(ic, 'class Moho::' .. k)
        L('Instance Counts/' .. k .. ' = ' .. (c and (fmt(c.Value) .. ' [' .. tostring(c.Type) .. ']') or 'absent'))
    end
    for _, v in views do
        look(v)
        WaitSeconds(2)
        for i = 1, 3 do
            local sp = GetMouseScreenPos()
            local wp = GetMouseWorldPos()
            L(v[1] .. ' #' .. i .. ' screen=' .. fmt(sp[1]) .. ',' .. fmt(sp[2]) .. ' world=' .. fmt(wp[1]) .. ',' .. fmt(wp[2]) .. ',' .. fmt(wp[3]) .. ' GetMouseTerrainElevation=' .. fmt(GetMouseTerrainElevation()))
            dumpNode(node(node(__EngineStats, 'Camera'), 'Cursor'), v[1] .. ' Camera/Cursor/')
            WaitSeconds(0.7)
        end
    end
    L('end')
    WaitSeconds(1)
    LOG('PROBE done')
end)
