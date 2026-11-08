-- The in-game interface, set up the way moho does at game start: the user
-- layer's SetupUI (skin, layout, cursor), then the world UI provider from
-- FAF's gamemain.lua creates the game interface (gamemain.CreateUI: FAF's
-- panels, unchanged) and the loading dialog ends (its initial animations
-- show the interface).
import('/lua/ui/uimain.lua').SetupUI()
-- The player's key bindings (the lobby does this before the game starts).
IN_AddKeyMapTable(import('/lua/keymap/keymapper.lua').GetKeyMappings())
local gamemain = import('/lua/ui/game/gamemain.lua')
gamemain.CreateWldUIProvider()
local provider = __engine_wld_provider()
provider:CreateGameInterface(false)
provider:StopLoadingDialog()
