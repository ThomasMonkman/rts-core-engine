RTS Core Engine: a remake of the Supreme Commander: Forged Alliance engine
==========================================================================

This is only the engine. It plays on two things you need to have yourself:

1. Supreme Commander: Forged Alliance (Steam or GOG). The engine reads the
   game's own files; none are included here.

2. Forged Alliance Forever's game code (FAF's Lua), either:
   - the FAF client (faforever.com), installed and run once, or
   - a FAF release from GitHub, e.g. release 3839:
       https://github.com/FAForever/fa/archive/refs/tags/3839.zip
     (unzip it, and put the folder next to the program named "fa", or pick
     it when asked).

Everyone in a game needs the same program AND the same FAF: all the FAF
client's (same version), or all the same release zip. Otherwise joining
says "Different versions" and what differs.


Windows
-------
Unzip anywhere and run rts-core-engine.exe.

If you have the FAF client and Forged Alliance on Steam, it finds both by
itself. Otherwise it asks for the folders the first time and remembers them.

Windows may warn that the program is from an unknown publisher (it isn't
signed): choose "More info", then "Run anyway".


macOS
-----
Forged Alliance has no Mac version, so copy its install folder from a Windows
PC (Steam: right-click the game > Manage > Browse local files), or download
the Windows files with SteamCMD:

    steamcmd +@sSteamCmdForcePlatformType windows +login <you> +app_update 9420 +quit

The FAF client doesn't run on macOS: use a FAF release zip from GitHub (the
same one as the people you play with).

Move "RTS Core Engine.app" to Applications. The first time, right-click it and
choose Open, then Open again (it isn't notarised by Apple). If macOS still
refuses, run this in Terminal:

    xattr -dr com.apple.quarantine "/Applications/RTS Core Engine.app"

The first start asks for the Forged Alliance folder, then FAF's code, and
remembers both. Allow "Local Network" when asked, for LAN games.


Linux
-----
For 64-bit (x86-64) distributions from about 2024 on (glibc 2.39 or newer:
Ubuntu 24.04, Fedora 40, current Arch). It needs Vulkan graphics drivers
(on Ubuntu: mesa-vulkan-drivers, or your GPU vendor's) and ALSA for sound
(libasound2, installed almost everywhere).

Unpack it anywhere and run ./rts-core-engine from that folder.

Install Forged Alliance through Steam (it runs it with Proton) and it's
found by itself; otherwise the first start asks for the folder. The FAF
client doesn't run on Linux: use a FAF release zip from GitHub (the same
one as the people you play with).


Where things are
----------------
The log, game.log, is written beside rts-core-engine.exe (macOS: beside
"RTS Core Engine.app"; Linux: beside rts-core-engine). If that folder is read-only,
it goes in the settings folder instead:
    Windows: %APPDATA%\rts-core-engine
    macOS:   ~/Library/Application Support/rts-core-engine
    Linux:   ~/.config/rts-core-engine

settings.ini holds the two folders; delete a line to choose again. From a
terminal, --game-dir <folder> and --fa <folder> set them for one run, and
--help lists every option. --fa is FAF's game code only: a folder of your
own maps and mods (with maps and / or mods folders inside) goes to
--vault <folder>.


Licence
-------
RTS Core Engine © Thomas Monkman. It is free software: you can redistribute
it and/or modify it under the terms of the GNU General Public License,
version 3 or (at your option) any later version. Provided as is, with no
warranty. See LICENSE.txt (also built into the program: --license).

It is built with open-source libraries (Rust crates, Luau, PL_MPEG), each
under its own licence: THIRD-PARTY-LICENSES.txt lists them, with the texts.

Supreme Commander: Forged Alliance belongs to its owners; FAF's code to the
Forged Alliance Forever project. Neither is part of this download.
