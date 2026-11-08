# VFS & Assets

## Archive formats
`.scd` (base FA game) and `.nx2` (FAF patches) are **plain ZIP files with different extensions**. No format difference — the extension is purely a namespacing convention so `init_faf.lua` can glob them separately.

`.nxt` is used for optional FAF featured mods (murderparty, labwars, etc.).

`.zst` (engine-only, not a FAF format): in a **folder** mount, `<name>.zst` is served decompressed as `<name>` (read, streamed, opened, sized). It keeps big generated test content small in the repo: the weapon audit's 81 km map (`tests/vault/maps/audit_flat`, `dev/audit/flat_map.py`) is a 64 MB `.scmap` stored as 5 KB (its 20 km census map `audit_duel`, 4 MB, as 1.3 KB).

## VFS mounting order
1. FAF gamedata (`*.nx2`) — highest priority
2. FA gamedata (`*.scd`) — base game

Files are indexed into a flat path→archive map. Same path in a higher-priority archive wins — the VFS presents a single unified namespace. This is how FAF overrides FA files transparently.

## init flow
```
engine reads init.lua / init_faf.lua
  → VFS mounts archives in priority order
  → Lua VM boots (simInit.lua)
  → import() cache pre-seeded
  → hook files from /hook/ applied (patch functions without replacing files)
  → blueprints registered
  → TypeClass system initialised
```

## Hook system
`/hook/lua/foo.lua` is merged/appended into `/lua/foo.lua` after it loads. FAF uses this heavily. Hooks patch specific functions without copying entire files.

Hook folders apply in order: the init file's `hook` list (FAF: `/schook`), the engine's own `/engine_hook`, then each active mod's (below). A hook file is appended to the script's text, so it runs in the same chunk.

## Mods (`src/engine/mods.rs`)
- **Found**: every `*mod_info.lua` under `/mods` (`discover`), read as FAF's `/lua/mods.lua` `LoadModInfo` does (its defaults, `location` = the file's folder; `enabled = false` left out), for what the engine resolves itself: launch uids, `--mod`, the lobby's hashes. Every mod is found, a `Hotstats` too: leaving that one out is FAF's `mods.lua`'s rule, and it applies through FAF's own Lua (below). `/mods` holds the base game's `mods.scd` and the player's vault mods, which FAF's init file mounts (`SHGetFolderPath('PERSONAL') .. 'My Games/Gas Powered Games/Supreme Commander Forged Alliance/mods'`); `--vault <dir>` also mounts `<dir>/mods/<name>`.
- **Chosen**: sim mods come from the lobby's launch (`gameConfig.GameMods`: the `ModInfo` list `Mods.GetGameMods` gives, or a uid set) and `--mod <uid>` (repeatable; recorded in the replay header). The game's interface starts with those (and UI-only `--mod`s), then runs the game's own `/lua/SessionInit.lua` as moho does, which adds the player's UI mods with its `mods.lua` `GetUiMods`. So each game's rules apply: FAF's leaves out `Hotstats` (it has the score screen built in), LOUD's keeps it and sorts `__active_mods` by name.
- **Ordered** (`resolve`): by uid, then each mod's `before` / `after` (`after` defaults to `requires`), so a set of mods has one order on every machine (FAF's lobby sorts with `pairs`). A cycle is broken by uid.
- **Activated** (`activate`, per VM): `__active_mods` lists the mods' info tables. As moho's script loader (FA exe 0x4ce2c0), each script is hooked when it loads by the `<location><hookdir>` folders (default `/hook`) of the mods `__active_mods` lists at that moment (`sync_hooks`), so a mod the game's Lua adds hooks the scripts loaded after it: the UI mods `SessionInit.lua` adds miss `userInit.lua`'s and the interface's blueprint load, as in the real game. The sim activates its mods before blueprints load (`Engine::activate_mods`), then publishes the session's mods; the game's interface VM waits for them before its own blueprint load. UI-only mods never reach the sim. The front end starts with none; whatever its Lua puts in `__active_mods` (FAF's `mods.lua` `UpdateUIMods`) hooks what it loads next.
- **Scripts**: a VM with mods keeps a `ModHooks` (app data) with the set of scripts its mods hook. `doscript` checks it (one hash lookup); a hooked script is compiled with the mods' hooks and kept per VM, any other comes from the shared `ScriptCache`.
- **Lobby check**: at startup each client hashes every installed sim mod (uid, version, every file in its folder; `hash_installed_in_background`) and sends the list to each member it connects to (`Msg::Mods`). The host's launch (an engine hook wrapping FAF's `TryLaunch`) is refused, with a chat line per problem, while any player lacks a game mod, has a different copy of it, or hasn't sent their list (`NetCore::mod_problems`). FAF's own lobby only compares uids.
- **Lockstep**: as a backstop, a hash of the session's sim mods is XORed into the sim checksum, so peers that somehow run different sim mods show as a desync from the first check.

## .scmap decoder
Versions 56 (FAF / Ozonex editor maps) and 60 (FA) parse; `read_terrain` gives the sim's part, `read_render` also the renderer's (shader, lighting, water, strata, decals, mask / normal / water map blobs). Every stock map parses to exactly its end (test). FAF vault maps mount with `--vault <dir>`; two are kept in `../faf-vault/maps` (Adaptive Monument Valley: TTerrainXP; Texture Test Map: Terrain200), downloaded from `https://content.faforever.com/maps/<folder>.zip`.
Key sections in order:
1. Magic `\x00map` + version (56=vanilla, 60=FA)
2. World width/height floats
3. `WaterParams` block
4. Preview DDS blob (length-prefixed)
5. Heightmap: `(w+1)×(h+1)` u16 samples + scale float
6. Texture layers (5 vanilla, 9 FA)
7. Decals + decal groups
8. Normal/texture/water map blobs
9. TerrainType map
10. Props (blueprint path + 3×3 rotation matrix + position + scale)

Convert heightmap sample to world height: `sample * scale` (scale is the f32 after the heightmap dims, 1/128 on stock maps → max height 512). Header details: `src/engine/assets/scmap.rs`.

## .scm mesh decoder (binrw)
Magic: `MODL`. Key sections:
- Header with offsets to bones, vertices, indices, sub-objects
- Bones: name offset + 4×4 rest pose matrix + parent/child indices
- Vertices: 44 bytes each — position, tangent, normal, binormal, uv0, uv1, bone weights
- Indices: u16 or u32 depending on vertex count
- Sub-objects: material name + index range for draw call splitting

**Note**: Format was reverse-engineered from community docs. Verify offsets against a hex dump of a known-good .scm before using in production.

## .dds textures and generated mips
`src/engine/assets/dds.rs` reads DXT1/3/5 and 8- to 32-bit uncompressed textures, and writes them (`dds::write`). The real game loads every texture with D3DX's default mip count (gpggal.dll), so D3DX builds any levels the file lacks. About 530 of FA's own textures and every map's strata masks and water map ship without a full chain.
- **Generated at load** (`src/engine/assets/mips.rs`): a 2x2 box filter of the level above, in linear light for sRGB textures; BC textures are decoded, filtered and re-encoded (texpresso ClusterFit). Each texture logs a warning. `RTS_NO_GEN_MIPS=1` turns it off (to compare).
- **Cached on disk** (`src/engine/assets/mip_cache.rs`): the whole texture as a `.dds` in `install::cache_dir()/mips` (macOS `~/Library/Caches/rts-core-engine`, Windows `%LOCALAPPDATA%\rts-core-engine\cache`, Linux `$XDG_CACHE_HOME/rts-core-engine`), keyed by the source's `Vfs::fingerprint` (archive CRC, or a loose file's size and time; a map blob's bytes), path, sampling and `GENERATOR_VERSION`. A hit streams like a texture with mips. Entries unused for 90 days, then the oldest beyond 2 GB, are pruned once per run, after a map's textures have loaded and both cache options have been applied (Video tab: Texture Cache Disk Space and Texture Cache: Keep Unused, through the engine's `eng_TextureCacheMB` / `eng_TextureCacheDays`; the defaults are those two values). `RTS_NO_MIP_CACHE=1` skips it; tests never use it. Bump `GENERATOR_VERSION` when the generated levels change.

## Engine globals (LUAGLOBALS)
The wiki LUAGLOBALS page lists all C++-provided functions callable from Lua. You don't implement all of them upfront — implement on demand as scripts call them. Missing globals produce clear "attempt to call nil" errors. Reference: `FAForever/fa` repo `engine/` folder (`Sim.lua`, `User.lua`).
