-- Interface half of probe_ui_controls: FAF's control classes on the engine's
-- controls (text and font metrics, bitmap sizes, hidden / alpha / hit-test
-- inheritance, depth, destroy order, frame updates, keyboard focus, edits,
-- item lists), then the world camera and the world view's projections.
-- Screen sizes differ: positions are logged as fractions of the view.
-- Shared helpers (kept in each ui_probe.lua: ours takes the file as one --ui-lua string).
-- Values under these keys are logged by type only (player names, local paths).
local MASK = { name = true, nickname = true, Nickname = true, authorizedCommandSources = false }
local function fv(v, depth)
    local t = type(v)
    if t == 'number' then return string.format('%.2f', v) end
    if t == 'string' then return '"' .. v .. '"' end
    if t == 'boolean' or t == 'nil' then return tostring(v) end
    if t == 'table' then
        local okb, bp = pcall(function() return v.GetBlueprint and v:GetBlueprint().BlueprintId end)
        if okb and bp then
            local okn, nm = pcall(function() return v:GetCustomName(v) end)
            if bp == 'uel0001' then nm = 'p_acu' end
            return '<unit ' .. tostring(bp) .. ' ' .. tostring(okn and nm) .. '>'
        end
        if depth <= 0 then return '{..}' end
        local keys = {}
        for k in pairs(v) do table.insert(keys, k) end
        table.sort(keys, function(a, b)
            if type(a) == type(b) and (type(a) == 'number' or type(a) == 'string') then return a < b end
            return tostring(type(a)) .. tostring(a) < tostring(type(b)) .. tostring(b)
        end)
        local s = '{'
        local n = 0
        for _, k in ipairs(keys) do
            n = n + 1
            if n > 40 then s = s .. ' ...' break end
            s = s .. ' ' .. tostring(k) .. '=' .. (MASK[k] and ('<' .. type(v[k]) .. '>') or fv(v[k], depth - 1))
        end
        return s .. ' }'
    end
    return '<' .. t .. '>'
end
local function G(name) return rawget(_G, name) end
-- moho's Lua calls a C function's type 'cfunction'.
local function isfn(f) return type(f) == 'function' or type(f) == 'cfunction' end
local STAGE = 'init'
-- P(label, f, a, b, c, d): calls f(a, b, c, d) protected, logs up to 4 results.
local function P(label, f, a, b, c, d)
    if not isfn(f) then
        LOG('PROBE ' .. STAGE .. ' ' .. label .. ' MISSING ' .. type(f))
        return nil
    end
    -- moho's functions check how many arguments they get: no trailing nils.
    local r
    if d ~= nil then r = { pcall(f, a, b, c, d) }
    elseif c ~= nil then r = { pcall(f, a, b, c) }
    elseif b ~= nil then r = { pcall(f, a, b) }
    elseif a ~= nil then r = { pcall(f, a) }
    else r = { pcall(f) } end
    if not r[1] then
        LOG('PROBE ' .. STAGE .. ' ' .. label .. ' ERR ' .. string.sub(tostring(r[2]), 1, 120))
        return nil
    end
    local last = 1
    for i = 2, 6 do if r[i] ~= nil then last = i end end
    local s = ''
    for i = 2, last do s = s .. ' ' .. fv(r[i], 3) end
    if last == 1 then s = ' (nothing)' end
    LOG('PROBE ' .. STAGE .. ' ' .. label .. ' =' .. s)
    return r[2], r[3], r[4]
end
local function waitTick(t)
    while GameTick() < t do WaitSeconds(0.1) end
end
local function named()
    local out = {}
    UISelectionByCategory('ALLUNITS', false, false, false, false)
    WaitSeconds(0.3)
    local sel = GetSelectedUnits() or {}
    for _, u in pairs(sel) do
        local ok, n = pcall(function() return u:GetCustomName(u) end)
        if ok and n then out[n] = u end
        if u:GetUnitId() == 'uel0001' then out.p_acu = u end
    end
    return out, sel
end
local function look(zoom)
    GetCamera('WorldCamera'):SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, zoom or 160)
end
local Group = import('/lua/maui/group.lua').Group
local Bitmap = import('/lua/maui/bitmap.lua').Bitmap
local Text = import('/lua/maui/text.lua').Text
local Edit = import('/lua/maui/edit.lua').Edit
local ItemList = import('/lua/maui/itemlist.lua').ItemList
local function M(label, obj, m, a, b, c)
    return P(label .. ':' .. m, obj[m], obj, a, b, c)
end
local function f2(n) return type(n) == 'number' and string.format('%.2f', n) or tostring(n) end
local rd1, rd2
ForkThread(function()
    look(160)
    waitTick(100)
    local frame = GetFrame(0)
    STAGE = 'ctl'
    local g = Group(frame, 'probe_g')
    g.Left:Set(100) g.Top:Set(120) g.Width:Set(300) g.Height:Set(200)
    local child = Group(g, 'probe_child')
    child.Left:Set(110) child.Top:Set(130) child.Width:Set(50) child.Height:Set(40)
    local grand = Bitmap(child)
    grand.Left:Set(115) grand.Top:Set(135) grand.Width:Set(10) grand.Height:Set(10)
    M('g', g, 'GetName') M('grand', grand, 'GetName') M('g', g, 'SetName', 'renamed') M('g', g, 'GetName')
    P('GetParent', function() return child:GetParent() == g, g:GetParent() == frame, frame:GetParent() == nil, type(frame:GetParent()) end)
    P('GetRootFrame', function() return grand:GetRootFrame() == frame, frame:GetRootFrame() == frame end)
    P('Depth', function() return frame.Depth(), g.Depth(), child.Depth(), grand.Depth() end)
    P('rect', function() return g.Right(), g.Bottom(), child.Right(), child.Bottom() end)
    P('IsHidden defaults', function() return g:IsHidden(), child:IsHidden(), frame:IsHidden() end)
    local hides = ''
    child.OnHide = function(self, hidden) hides = hides .. 'child:' .. tostring(hidden) .. ';' end
    grand.OnHide = function(self, hidden) hides = hides .. 'grand:' .. tostring(hidden) .. ';' end
    g:Hide()
    P('after g:Hide', function() return g:IsHidden(), child:IsHidden(), grand:IsHidden(), hides end)
    hides = ''
    child:Show()
    P('after child:Show', function() return g:IsHidden(), child:IsHidden(), grand:IsHidden(), hides end)
    hides = ''
    g:Show()
    P('after g:Show', function() return g:IsHidden(), child:IsHidden(), grand:IsHidden(), hides end)
    hides = ''
    g:SetHidden(true) g:SetHidden(true)
    P('after SetHidden(true) twice', function() return g:IsHidden(), child:IsHidden(), hides end)
    hides = ''
    child.OnHide = function(self, hidden) hides = hides .. 'child-veto:' .. tostring(hidden) .. ';' return true end
    g:Show()
    P('OnHide returning true stops children', function() return g:IsHidden(), child:IsHidden(), grand:IsHidden(), hides end)
    child.OnHide = nil grand.OnHide = nil
    g:Show() child:Show() grand:Show()
    P('GetAlpha defaults', function() return g:GetAlpha(), child:GetAlpha() end)
    g:SetAlpha(0.5)
    P('SetAlpha(0.5)', function() return g:GetAlpha(), child:GetAlpha(), grand:GetAlpha() end)
    g:SetAlpha(0.25, true)
    P('SetAlpha(0.25,true)', function() return g:GetAlpha(), child:GetAlpha(), grand:GetAlpha() end)
    g:SetAlpha(2) child:SetAlpha(-1)
    P('SetAlpha out of range', function() return g:GetAlpha(), child:GetAlpha() end)
    g:SetAlpha(1, true)
    P('GetRenderPass default', function() return g:GetRenderPass(), frame:GetRenderPass() end)
    g:SetRenderPass(3)
    P('SetRenderPass(3)', function() return g:GetRenderPass(), child:GetRenderPass() end)
    g:SetRenderPass(0)
    P('IsHitTestDisabled defaults', function() return g:IsHitTestDisabled(), child:IsHitTestDisabled(), grand:IsHitTestDisabled(), frame:IsHitTestDisabled() end)
    g:DisableHitTest()
    P('DisableHitTest()', function() return g:IsHitTestDisabled(), child:IsHitTestDisabled() end)
    g:DisableHitTest(true)
    P('DisableHitTest(true)', function() return g:IsHitTestDisabled(), child:IsHitTestDisabled(), grand:IsHitTestDisabled() end)
    g:EnableHitTest()
    P('EnableHitTest()', function() return g:IsHitTestDisabled(), child:IsHitTestDisabled(), grand:IsHitTestDisabled() end)
    g:EnableHitTest(true)
    P('HitTest', function() return g:HitTest(100, 120), g:HitTest(99, 120), g:HitTest(399, 319), g:HitTest(400, 320), g:HitTest(400, 319), g:HitTest(250, 200) end)
    P('HitTest .5', function() return g:HitTest(99.5, 120), g:HitTest(399.5, 319.5) end)
    g:Hide()
    P('HitTest hidden', function() return g:HitTest(250, 200) end)
    g:Show()
    g:DisableHitTest()
    P('HitTest disabled', function() return g:HitTest(250, 200) end)
    g:EnableHitTest()
    P('Disable/Enable', function() child:Disable() local a, b = child:IsDisabled(), child:IsHitTestDisabled() child:Enable() return a, b, child:IsDisabled(), child:IsHitTestDisabled() end)
    P('NeedsFrameUpdate default', function() return g:NeedsFrameUpdate() end)
    P('ApplyFunction order', function() local s = '' g:ApplyFunction(function(c) s = s .. tostring(c:GetName()) .. ';' end) return s end)
    P('frame:GetTopmostDepth type', function() return type(frame:GetTopmostDepth()), frame:GetTopmostDepth() >= g.Depth() end)
    P('frame:GetTargetHead', function() return frame:GetTargetHead() end)
    P('Dump exists', function() return type(g.Dump), type(g.GetChildren), type(g.SetParent), type(g.ClearChildren) end)

    -- Frame updates.
    do
        local deltas = {}
        local c0 = CurrentTime()
        g.OnFrame = function(self, dt) table.insert(deltas, dt) end
        g:SetNeedsFrameUpdate(true)
        P('NeedsFrameUpdate set', function() return g:NeedsFrameUpdate() end)
        WaitSeconds(1)
        g:SetNeedsFrameUpdate(false)
        local elapsed = CurrentTime() - c0
        local sum, mn, mx = 0, 99, 0
        for _, d in ipairs(deltas) do sum = sum + d if d < mn then mn = d end if d > mx then mx = d end end
        LOG('PROBE ctl OnFrame calls>10 ' .. tostring(table.getn(deltas) > 10) .. ' sum/elapsed ' .. string.format('%.1f', sum / elapsed) .. ' min>0 ' .. tostring(mn > 0) .. ' max<0.5 ' .. tostring(mx < 0.5) .. ' type ' .. type(deltas[1]))
        local hiddenCalls = 0
        g.OnFrame = function(self, dt) hiddenCalls = hiddenCalls + 1 end
        g:Hide() g:SetNeedsFrameUpdate(true)
        WaitSeconds(0.5)
        LOG('PROBE ctl OnFrame while hidden ' .. tostring(hiddenCalls > 0))
        g:SetNeedsFrameUpdate(false) g:Show()
    end

    -- Destroy.
    do
        local order = ''
        local d1 = Group(g, 'd1')
        local d2 = Group(d1, 'd2')
        local d3 = Bitmap(d2)
        d1.OnDestroy = function() order = order .. 'd1;' end
        d2.OnDestroy = function() order = order .. 'd2;' end
        d3.OnDestroy = function() order = order .. 'd3;' end
        d1:Destroy()
        rd1, rd2 = d1, d2
        P('Destroy order', function() return order end)
        P('IsDestroyed', function() return IsDestroyed(d1), IsDestroyed(d2), IsDestroyed(g), IsDestroyed(nil) end)
        local c1 = Group(g, 'c1') local c2 = Group(g, 'c2')
        order = ''
        c1.OnDestroy = function() order = order .. 'c1;' end c2.OnDestroy = function() order = order .. 'c2;' end
        local before = ''
        g:ApplyFunction(function(c) before = before .. tostring(c:GetName()) .. ';' end)
        g:ClearChildren()
        P('ClearChildren', function() return before, order, IsDestroyed(c1), IsDestroyed(child), IsDestroyed(g) end)
    end

    STAGE = 'text'
    local t = Text(g)
    t.Left:Set(100) t.Top:Set(120)
    P('Text before font', function() return t.FontAscent(), t.FontDescent(), t.FontExternalLeading(), t.TextAdvance(), t:GetText() end)
    for _, fnt in ipairs({ { 'Arial', 12 }, { 'Arial', 14 }, { 'Arial', 18 }, { 'Arial', 24 }, { 'Arial Bold', 14 }, { 'Arial Black', 16 }, { 'Zeroes Three', 14 }, { 'Butterbelly', 14 }, { 'Arial Gras', 14 }, { 'Arial Narrow', 12 }, { 'wintermute', 14 }, { 'VDub', 12 } }) do
        P('SetFont ' .. fnt[1] .. ' ' .. fnt[2], function()
            t:SetFont(fnt[1], fnt[2])
            t:SetText('Hello World')
            return 'asc ' .. f2(t.FontAscent()) .. ' desc ' .. f2(t.FontDescent()) .. ' lead ' .. f2(t.FontExternalLeading()) .. ' adv ' .. f2(t.TextAdvance()) .. ' H ' .. f2(t.Height()) .. ' W ' .. f2(t.Width())
                .. ' M ' .. f2(t:GetStringAdvance('M')) .. ' i ' .. f2(t:GetStringAdvance('i')) .. ' sp ' .. f2(t:GetStringAdvance(' ')) .. ' 0-9 ' .. f2(t:GetStringAdvance('0123456789'))
                .. ' AV ' .. f2(t:GetStringAdvance('AV')) .. ' A+V ' .. f2(t:GetStringAdvance('A') + t:GetStringAdvance('V')) .. ' long ' .. f2(t:GetStringAdvance('The quick brown fox jumps over the lazy dog'))
        end)
    end
    t:SetFont('Arial', 14)
    P('GetStringAdvance empty', function() return t:GetStringAdvance('') end)
    P('GetStringAdvance utf8', function() return t:GetStringAdvance('h\195\169llo'), t:GetStringAdvance('hello') end)
    P('GetStringAdvance newline/tab', function() return t:GetStringAdvance('a\nb'), t:GetStringAdvance('a\tb'), t:GetStringAdvance('ab') end)
    P('SetText number', function() t:SetText(5) return t:GetText(), type(t:GetText()) end)
    P('SetText nil', function() t:SetText(nil) return t:GetText() end)
    P('SetText LOC tag', function() t:SetText('<LOC _OK>OK') return t:GetText() end)
    P('TextAdvance follows SetText', function() t:SetText('MMMM') local a = t.TextAdvance() t:SetText('M') return a > t.TextAdvance(), t.Width() == math.floor(t.TextAdvance()) end)
    P('SetClipToWidth', function() t:SetText('MMMMMMMMMM') t.Width:Set(20) t:SetClipToWidth(true) return t.Width(), t.TextAdvance() > 20 end)

    STAGE = 'bmp'
    local b = Bitmap(g)
    b.Left:Set(100) b.Top:Set(120)
    P('Bitmap no texture', function() return b.BitmapWidth(), b.BitmapHeight(), b.Width(), b.Height(), b:GetNumFrames() end)
    for _, tex in ipairs({ '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds', '/textures/ui/common/icons/units/uel0001_icon.dds', '/textures/ui/common/game/avatar/avatar_bmp.dds', '/textures/ui/common/widgets/large-h_scr/bar-mid_scr_up.dds', '/textures/ui/common/game/strategicicons/icon_land1_generic_rest.dds' }) do
        P('SetTexture ' .. string.gsub(tex, '.*/', ''), function() b:SetTexture(tex) return b.BitmapWidth(), b.BitmapHeight(), b.Width(), b.Height(), b:GetNumFrames() end)
    end
    P('SetTexture border 1', function() b:SetTexture('/textures/ui/common/game/economic-overlay/econ_bmp_m.dds', 1) return b.BitmapWidth(), b.BitmapHeight() end)
    P('SetTexture frames', function() b:SetTexture({ '/textures/ui/common/game/economic-overlay/econ_bmp_m.dds', '/textures/ui/common/icons/units/uel0001_icon.dds' }) return b:GetNumFrames(), b.BitmapWidth(), b:GetFrame() end)
    P('SetFrame', function() b:SetFrame(1) return b:GetFrame(), b.BitmapWidth() end)
    P('SetSolidColor', function() b:SetSolidColor('ff00ff00') return b.BitmapWidth(), b.BitmapHeight(), b:GetNumFrames() end)
    P('SetSolidColor name', function() return b:SetSolidColor('red') end)
    P('SetSolidColor short', function() return b:SetSolidColor('f00') end)
    P('SetUV/Tiled/ColorMask', function() return b:SetUV(0, 0, 1, 1), b:SetTiled(true), b:SetColorMask('ffffffff'), b:UseAlphaHitTest(true) end)
    P('EnumColorNames', function() local n = EnumColorNames() table.sort(n) return table.getn(n), n[1], n[2], n[table.getn(n)] end)

    STAGE = 'focus'
    P('GetCurrentFocusControl none', function() return g:GetCurrentFocusControl() end)
    g:AcquireKeyboardFocus(false)
    P('after Acquire', function() return g:GetCurrentFocusControl() == g, t:GetCurrentFocusControl() == g end)
    local lost = ''
    g.OnLoseKeyboardFocus = function() lost = lost .. 'g-lost;' end
    g.OnKeyboardFocusChange = function() lost = lost .. 'g-change;' end
    t:AcquireKeyboardFocus(true)
    P('second Acquire', function() return t:GetCurrentFocusControl() == t, lost end)
    t:AbandonKeyboardFocus()
    P('after Abandon', function() return t:GetCurrentFocusControl() == g, t:GetCurrentFocusControl() == nil, lost end)
    g:AbandonKeyboardFocus()
    P('after both Abandon', function() return g:GetCurrentFocusControl() end)
    P('AddInputCapture', function() AddInputCapture(g) return GetInputCapture() == g, AnyInputCapture() end)
    P('AddInputCapture second', function() AddInputCapture(t) return GetInputCapture() == t end)
    P('RemoveInputCapture first', function() RemoveInputCapture(g) return GetInputCapture() == t, AnyInputCapture() end)
    P('RemoveInputCapture second', function() RemoveInputCapture(t) return GetInputCapture(), AnyInputCapture() end)

    STAGE = 'edit'
    local e = Edit(g)
    e.Left:Set(100) e.Top:Set(150) e.Width:Set(200) e.Height:Set(20)
    local changes = ''
    e.OnTextChanged = function(self, new, old) changes = changes .. tostring(old) .. '>' .. tostring(new) .. ';' end
    P('Edit defaults', function() return e:GetText(), e:GetCaretPosition(), e:GetMaxChars(), e:IsEnabled(), e:IsCaretVisible(), e:IsBackgroundVisible() end)
    P('Edit default colours', function() return e:GetForegroundColor(), e:GetBackgroundColor(), e:GetCaretColor(), e:GetHighlightForegroundColor(), e:GetHighlightBackgroundColor() end)
    P('Edit default font', function() return e:GetFontHeight(), e:GetStringAdvance('Hello World') end)
    e:SetFont('Arial', 14)
    P('Edit Arial 14', function() return e:GetFontHeight(), e:GetStringAdvance('Hello World') end)
    P('SetText', function() e:SetText('abcdef') return e:GetText(), e:GetCaretPosition(), changes end)
    P('SetCaretPosition', function() e:SetCaretPosition(2) local a = e:GetCaretPosition() e:SetCaretPosition(99) local b = e:GetCaretPosition() e:SetCaretPosition(-1) return a, b, e:GetCaretPosition() end)
    P('SetMaxChars', function() e:SetMaxChars(3) return e:GetMaxChars(), e:GetText() end)
    P('SetText over max', function() e:SetText('123456') return e:GetText() end)
    P('ClearText', function() changes = '' e:ClearText() return e:GetText(), changes end)
    P('SetNewForegroundColor', function() e:SetNewForegroundColor('ff112233') return e:GetForegroundColor() end)
    P('SetNewForegroundColor name', function() e:SetNewForegroundColor('red') return e:GetForegroundColor() end)
    P('ShowBackground/Caret', function() e:ShowBackground(false) e:ShowCaret(false) return e:IsBackgroundVisible(), e:IsCaretVisible() end)
    P('DisableInput', function() e:DisableInput() local a = e:IsEnabled() e:EnableInput() return a, e:IsEnabled() end)
    P('AcquireFocus', function() e:AcquireFocus() return e:GetCurrentFocusControl() == e end)
    P('AbandonFocus', function() e:AbandonFocus() return e:GetCurrentFocusControl() end)

    STAGE = 'list'
    local l = ItemList(g)
    l.Left:Set(100) l.Top:Set(180) l.Width:Set(200) l.Height:Set(50)
    P('List defaults', function() return l:GetItemCount(), l:GetSelection(), l:Empty(), l:GetRowHeight(), l:NeedsScrollBar() end)
    l:SetFont('Arial', 14)
    P('List Arial 14', function() return l:GetRowHeight(), l:GetStringAdvance('Hello World') end)
    for i = 1, 6 do l:AddItem('item' .. i) end
    P('List filled', function() return l:GetItemCount(), l:GetItem(0), l:GetItem(5), l:GetSelection(), l:Empty(), l:NeedsScrollBar() end)
    P('GetScrollValues', function() return l:GetScrollValues('Vert') end)
    P('SetSelection', function() l:SetSelection(2) local a = l:GetSelection() l:SetSelection(99) local b = l:GetSelection() l:SetSelection(-1) return a, b, l:GetSelection() end)
    P('ScrollLines', function() l:ScrollLines('Vert', 2) return l:GetScrollValues('Vert') end)
    P('ScrollToBottom', function() l:ScrollToBottom() return l:GetScrollValues('Vert') end)
    P('ScrollSetTop', function() l:ScrollSetTop('Vert', 1) return l:GetScrollValues('Vert') end)
    P('ShowItem', function() l:ShowItem(5) return l:GetScrollValues('Vert') end)
    P('ScrollToTop', function() l:ScrollToTop() return l:GetScrollValues('Vert') end)
    P('IsScrollable', function() return l:IsScrollable('Vert'), l:IsScrollable('Horz') end)
    P('ModifyItem', function() l:ModifyItem(1, 'changed') return l:GetItem(1) end)
    P('DeleteItem', function() l:SetSelection(3) l:DeleteItem(0) return l:GetItemCount(), l:GetItem(0), l:GetSelection() end)
    P('DeleteAllItems', function() l:DeleteAllItems() return l:GetItemCount(), l:GetSelection(), l:Empty() end)
    P('AddItem number', function() l:AddItem(5) return l:GetItem(0) end)
    for i = 1, 6 do l:AddItem('item' .. i) end

    -- The world camera.
    STAGE = 'cam'
    local cam = GetCamera('WorldCamera')
    local view = import('/lua/ui/game/worldview.lua').viewLeft
    P('GetCamera', function() return type(cam), GetCamera('WorldCamera') == cam, type(GetCamera('NopeProbeCam')) end)
    local function settings(label)
        P(label .. ' SaveSettings', function() return cam:SaveSettings() end)
        P(label .. ' zoom', function() return cam:GetZoom(), cam:GetTargetZoom(), cam:GetFocusPosition() end)
    end
    P('GetMaxZoom/GetMinZoom', function() return cam:GetMaxZoom(), cam:GetMinZoom() end)
    cam:SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, 100)
    settings('same frame after SnapTo')
    WaitSeconds(0.3)
    settings('SnapTo 100')
    local saved = cam:SaveSettings()
    local W, H = view.Width(), view.Height()
    LOG('PROBE cam view aspect ' .. string.format('%.2f', W / H))
    local function frac(p) if not p then return 'nil' end return string.format('%.3f,%.3f', (p[1] or p.x) / W, (p[2] or p.y) / H) end
    P('Project focus', function() return frac(view:Project(Vector(405, 18.5, 610))) end)
    P('Project result shape', function() local p = view:Project(Vector(405, 18.5, 610)) local n = 0 for k in pairs(p) do n = n + 1 end return type(p), n, type(p[1]), type(p.x) end)
    P('Project +20x ground width', function() local a, b = view:Project(Vector(405, 18.5, 610)), view:Project(Vector(425, 18.5, 610)) return 'px per unit x zoom / view width ' .. string.format('%.3f', math.abs((b[1] or b.x) - (a[1] or a.x)) / 20 * 100 / W) .. ' sign ' .. (((b[1] or b.x) > (a[1] or a.x)) and '+' or '-') end)
    P('Project +20z', function() local a, b = view:Project(Vector(405, 18.5, 610)), view:Project(Vector(405, 18.5, 630)) return 'dy frac ' .. string.format('%.3f', ((b[2] or b.y) - (a[2] or a.y)) / H) end)
    P('Project +20y', function() local a, b = view:Project(Vector(405, 18.5, 610)), view:Project(Vector(405, 38.5, 610)) return 'dy frac ' .. string.format('%.3f', ((b[2] or b.y) - (a[2] or a.y)) / H) end)
    P('Project behind/offscreen', function() return frac(view:Project(Vector(0, 18.5, 0))) end)
    P('ProjectMultiple', function() local r = view:ProjectMultiple({ Vector(405, 18.5, 610), Vector(425, 18.5, 610) }) return type(r), r and frac(r[1]), r and frac(r[2]) end)
    P('UnProject centre', function() local p = UnProject(view, { W / 2, H / 2 }) return p end)
    P('UnProject centre Vector2', function() local p = UnProject(view, Vector2(W / 2, H / 2)) return p end)
    P('UnProject top left', function() local p = UnProject(view, { 0, 0 }) return (p[1] or p.x) < 405, (p[3] or p.z) > 610 or (p[3] or p.z) < 610 end)
    P('view methods', function() return view:HasHighlightCommand(), view:ShowConvertToPatrolCursor(), view:IsCartographic(), view:IsResourceRenderingEnabled(), view:GetRightMouseButtonOrder() end)
    P('IsInputLocked', function() return view:IsInputLocked() end)
    P('IsInputLocked(cam)', function() return view:IsInputLocked('WorldCamera') end)
    P('LockInput', function() view:LockInput() local a = view:IsInputLocked() view:UnlockInput() return a, view:IsInputLocked() end)
    P('LockInput nested', function() view:LockInput() view:LockInput() view:UnlockInput() local a = view:IsInputLocked() view:UnlockInput() return a, view:IsInputLocked() end)
    P('GetsGlobalCameraCommands', function() return view:GetsGlobalCameraCommands(true) end)
    P('SetCartographic', function() view:SetCartographic(true) local a = view:IsCartographic() view:SetCartographic(false) return a, view:IsCartographic() end)
    P('EnableResourceRendering', function() view:EnableResourceRendering(true) local a = view:IsResourceRenderingEnabled() view:EnableResourceRendering(false) return a, view:IsResourceRenderingEnabled() end)
    local units = named()
    SelectUnits({})
    P('GetScreenPos fac', function() return frac(view:GetScreenPos(units.p_fac)), frac(view:Project(units.p_fac:GetPosition())) end)
    P('GetScreenPos shape', function() local p = view:GetScreenPos(units.p_fac) local n = 0 for k in pairs(p) do n = n + 1 end return n, type(p[1]), type(p.x) end)
    P('GetMouseWorldPos vs UnProject', function() local m, w = GetMouseScreenPos(), GetMouseWorldPos() local p = UnProject(view, { m[1] - view.Left(), m[2] - view.Top() }) return VDist3(p, w) < 2 end)

    -- Zoom limits and timed moves.
    cam:SetZoom(50, 0)
    settings('same frame after SetZoom(50,0)')
    WaitSeconds(0.3)
    settings('SetZoom(50,0)')
    cam:SetZoom(100000, 0)
    WaitSeconds(0.3)
    P('SetZoom(100000) vs max', function() return cam:GetZoom() / cam:GetMaxZoom() end)
    cam:SetZoom(0.01, 0)
    WaitSeconds(0.3)
    P('SetZoom(0.01) vs min', function() return cam:GetZoom(), cam:GetMinZoom() end)
    cam:SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, 50)
    WaitSeconds(0.3)
    local function timed(label, start)
        local c0 = CurrentTime()
        start()
        local line = 'PROBE cam ' .. label
        for i = 1, 9 do
            WaitSeconds(0.25)
            local f = cam:GetFocusPosition()
            line = line .. ' | t' .. string.format('%.2f', CurrentTime() - c0) .. ' z ' .. string.format('%.1f', cam:GetZoom()) .. ' tz ' .. string.format('%.1f', cam:GetTargetZoom()) .. ' x ' .. string.format('%.1f', f[1] or f.x)
        end
        LOG(line)
    end
    timed('SetZoom(200,2)', function() cam:SetZoom(200, 2) end)
    cam:SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, 50)
    WaitSeconds(0.3)
    timed('MoveTo x+100 zoom 200 2s', function() cam:MoveTo(Vector(505, 18.5, 610), { math.pi, 1.3, 0 }, 200, 2) end)
    settings('after MoveTo')
    cam:SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, 50)
    WaitSeconds(0.3)
    timed('SetTargetZoom(200)', function() cam:SetTargetZoom(200) end)
    P('RestoreSettings', function() cam:RestoreSettings(saved) end)
    WaitSeconds(0.3)
    settings('restored')
    P('Reset', function() cam:Reset() end)
    WaitSeconds(0.5)
    settings('reset')
    P('reset zoom vs max', function() return cam:GetZoom() / cam:GetMaxZoom() end)
    cam:SnapTo(Vector(405, 18.5, 610), { 0, 0.5, 0 }, 80)
    WaitSeconds(0.3)
    settings('SnapTo heading 0 pitch 0.5')
    cam:SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, 80)
    WaitSeconds(0.3)
    P('TargetEntities', function() cam:TargetEntities({ units.p_fab }, 60, 0) end)
    WaitSeconds(0.5)
    P('after TargetEntities', function() local f, p = cam:GetFocusPosition(), units.p_fab:GetPosition() return cam:GetZoom(), VDist2(f[1], f[3], p[1], p[3]) < 3 end)
    P('TrackEntities', function() cam:TrackEntities({ units.p_tank:GetEntityId() }, 60, 0) end)
    WaitSeconds(0.5)
    P('after TrackEntities', function() local f, p = cam:GetFocusPosition(), units.p_tank:GetPosition() return cam:GetZoom(), VDist2(f[1], f[3], p[1], p[3]) < 3 end)
    cam:SnapTo(Vector(405, 18.5, 610), { math.pi, 1.3, 0 }, 160)
    WaitSeconds(0.3)
    P('after track then SnapTo', function() local f = cam:GetFocusPosition() return VDist2(f[1], f[3], 405, 610) < 3 end)
    WaitSeconds(3)
    STAGE = 'risky'
    LOG('PROBE risky before destroyed GetParent') WaitSeconds(0.4)
    P('destroyed GetParent', function() return rd1:GetParent() end)
    LOG('PROBE risky before destroyed IsHidden') WaitSeconds(0.4)
    P('destroyed IsHidden', function() return rd2:IsHidden() end)
    LOG('PROBE risky before destroyed Destroy again') WaitSeconds(0.4)
    P('destroyed Destroy again', function() return rd1:Destroy() end)
    LOG('PROBE risky before destroyed lazyvar') WaitSeconds(0.4)
    P('destroyed lazyvar', function() return rd1.Left end)
    LOG('PROBE risky before destroyed _c_object') WaitSeconds(0.4)
    P('destroyed _c_object', function() return type(rawget(rd1, '_c_object')) end)
    LOG('PROBE risky before SetFont unknown') WaitSeconds(0.4)
    P('SetFont unknown', function() t:SetFont('NopeProbeFont', 14) t:SetText('Hello World') return 'asc ' .. f2(t.FontAscent()) .. ' adv ' .. f2(t.TextAdvance()) end)
    LOG('PROBE risky before SetFont size 0') WaitSeconds(0.4)
    P('SetFont size 0', function() t:SetFont('Arial', 0) return t.FontAscent(), t.TextAdvance() end)
    LOG('PROBE risky before SetFont size 7.5') WaitSeconds(0.4)
    P('SetFont size 7.5', function() t:SetFont('Arial', 7.5) return t.FontAscent(), t.FontDescent() end)
    LOG('PROBE risky before SetTexture missing') WaitSeconds(0.4)
    P('SetTexture missing', function() b:SetTexture('/textures/ui/nope_probe.dds') return b.BitmapWidth(), b.BitmapHeight(), b:GetNumFrames() end)
    LOG('PROBE risky before GetItem(6)') WaitSeconds(0.4)
    P('GetItem(6)', function() return l:GetItem(6) end)
    LOG('PROBE risky before GetItem(-1)') WaitSeconds(0.4)
    P('GetItem(-1)', function() return l:GetItem(-1) end)
    LOG('PROBE risky before DeleteItem(99)') WaitSeconds(0.4)
    P('DeleteItem(99)', function() return l:DeleteItem(99) end)

    P('dead unit methods', function() return 'n/a' end)
    WaitSeconds(3)
    LOG('PROBE done')
end)
