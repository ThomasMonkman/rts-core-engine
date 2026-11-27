-- The front end (the main menu), started the way moho does when no game is
-- running: the player's key map (the executable loads keymapper.lua's
-- GetKeyMappings itself; FAF's StartFrontEndUI then takes the debug keys out
-- again), the user layer's SetupUI (skin, layout, cursor), then FAF's
-- StartFrontEndUI (menus/main.lua, unchanged). Esc reaches FAF's escape
-- handlers through the key map's `escape` action.
IN_AddKeyMapTable(import('/lua/keymap/keymapper.lua').GetKeyMappings())
local uimain = import('/lua/ui/uimain.lua')
uimain.SetupUI()
uimain.StartFrontEndUI()
