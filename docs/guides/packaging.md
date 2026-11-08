# Packaging: giving the engine to other people

The download is the engine alone. Players bring the two things it runs on, which aren't ours to ship:

- **Supreme Commander: Forged Alliance** (Steam app 9420, or GOG): the game's data (`gamedata/*.scd`,
  `sounds/`, `movies/`, `maps/`, `fonts/`).
- **FAF's Lua**: the FAF client's data folder (`C:\ProgramData\FAForever`: `bin/init_faf.lua` mounting
  its `gamedata/*.nx2`), or a checkout of github.com/FAForever/fa (its `init_local_development.lua`: at
  the root before FAF 3839, in `setup/bin/` since).

Everything else is built into the binary: shaders, Lua shims, the engine's `lua_overrides`
(`LUA_OVERRIDES` in `src/engine/mod.rs`, mounted ahead of everything) and the licence (`--license`).

## Finding the game's files (`src/install.rs`)

First match wins:

| # | Where | Notes |
|---|---|---|
| 1 | `--fa <DIR>` / `--game-dir <DIR>`, then `RTS_FA` / `RTS_GAME_DIR` | a path given here that's wrong is an error, not skipped |
| 2 | `../fa`, `../Supreme Commander Forged Alliance` | the development layout, relative to the working directory (ahead of the settings, so a saved pick never redirects `cargo run`) |
| 2½ | the folder above the executable's | when it stands in for the game's `bin/SupremeCommander.exe` (below) |
| 3 | `settings.ini` in the settings folder | written when the player picks folders in the "not found" dialog; a saved folder that's gone is skipped |
| 4 | beside the executable (beside the `.app` on macOS): `fa/`, `FAForever/`, `Supreme Commander Forged Alliance/` | a portable folder |
| 5 | `%ProgramData%\FAForever` (Windows), `~/.faforever` (Linux, macOS) | the FAF client: `bin/init_faf.lua`, and the game folder from its `fa_path.lua` |
| 6 | Steam libraries (`steamapps/libraryfolders.vdf`), then GOG's default folders | Windows, macOS and Linux Steam roots (Linux: `$XDG_DATA_HOME/Steam`, `~/.local/share/Steam`, `~/.steam/steam`, Flatpak's `~/.var/app/com.valvesoftware.Steam/.local/share/Steam`, Snap's `~/snap/steam/common/.local/share/Steam`, as faf-linux's `setup.sh` looks) |

A game folder must hold `gamedata/units.scd`. If something is still missing, a windowed run asks with a
native folder picker (`rfd`) and saves the answer; a headless run logs what's missing and how to give
it, and exits with code 2.

Two adjustments make any FAF install work from anywhere:

- FAF's development init file hard-codes `local locationOfRepository = '<path>'` (a placeholder in a
  fresh clone). The engine replaces that line with the checkout's root (the nearest folder above the
  init file holding `lua/version.lua`) before running it.
- FAF's init files `dofile` the FAF client's `fa_path.lua`, which names the game folder the client
  uses. After that `dofile` the engine restores `fa_path` to the folder it found, so a FAF data folder
  copied from Windows to a Mac still works.

`--fa` also takes any init file: LOUD's (`<FA>/LOUD/bin/LoudDataPath.lua`, or a Git-LOUD
checkout's `bin/SupComDataPath.lua`) runs LOUD instead of FAF ([running.md](running.md#loud)). Init files
are read as moho reads them: Windows paths and wildcards in `mount_dir` work on every platform.
Nothing finds LOUD by itself yet: a player gives `--fa` (or `RTS_FA`).

### Standing in for `bin/SupremeCommander.exe` (LOUD's launcher)

The engine takes the real game's command line, so it can replace the game's exe and be started
by whatever starts that. LOUD's `LOVE-LOUD.exe` runs, from the Forged Alliance folder,
`bin\SupremeCommander.exe /log "..\LOUD\bin\LOUD.log" /init "..\LOUD\bin\LoudDataPath.lua"`.

- `/init <file>` is `--fa <file>`, `/log <file>` is `--log <file>` (`cli::moho_args`). A relative
  path is read from the program's folder, not the working directory: LOUD's paths only name
  their files from `bin/`. Other switches (`/nobugreport`, `/gpgnet ...`) are dropped with their
  values and logged.
- The game folder is the one above the program's (`<FA>/bin/..`), ahead of a saved pick (row 2½
  of the table above).
- `dev/package/install-as-supcom.ps1 -GameDir <FA>` copies a static-CRT build over
  `bin\SupremeCommander.exe`, keeping the original as `SupremeCommander.original.exe`;
  `-Restore` puts it back (so does Steam's "Verify integrity of game files").

The settings folder is `%APPDATA%\rts-core-engine`, `~/Library/Application Support/rts-core-engine` or
`~/.config/rts-core-engine`. Outside a source checkout (no `Cargo.toml` + `src/main.rs` in the working
directory) the log defaults to `game.log` beside the executable (beside the `.app` on macOS, since
writing inside a signed bundle breaks its signature), or in the settings folder when that's read-only.
Games are recorded into `replays/<profile>` in the settings folder (docs/guides/running.md, "Replays"). A checkout keeps both in the working directory, as before. `vfs_dump.txt` is only written with `--vfs-dump`.

Logging to a file costs nothing measurable: the file is written by `tracing_appender`'s background
thread. Measured 2026-10-08 on Windows, dist build, a 3000-tick 4v4 AI game: 13.6-13.7 s whether the log
went to a file or to `NUL` (3 alternating runs each), and the game wrote 7000 lines (0.9 MB).

The FAF client's Lua is FAF's current release. The engine is developed against the `../fa` checkout,
so release-only differences show up there first. Seen 2026-10-07: release `Prop.lua` reads
`Economy.ReclaimMassMax` on every prop with no `RECLAIMABLE` guard. Props without an `Economy` table
(e.g. `massDeposit01_prop.bp`) then fail `OnCreate` 116 times on Seton's Clutch.

## What the log tells you (bug reports)

Players send `game.log` (beside the program). Besides the engine's usual lines it has:

- **At startup**: `system` (build `<version>+<commit>`, OS, CPU, threads, RAM total / free),
  `graphics` (GPU, kind, backend, driver, PCI ids) and `screen` (monitor, size, refresh, scale).
- **Graphics errors** (`target: graphics`): every wgpu validation error, logged instead of
  crashing (wgpu's default aborts); the first 10 in full, then one in 100. A lost GPU device
  (driver reset, GPU removed) is logged too.
- **Crashes** (`target: panic`): the message, where, and a backtrace (the `dist` build is
  stripped, so the backtrace has no function names, but the message names the file and line).
- **Hitches**: `hitch: the game waited for players' turns` with how long and for whom (each
  wait of 20 ms or more); `hitch: slow in the last 10 s` for sim ticks that overran their
  real-time slot and frames over 100 ms (count, worst, when); `hitch: froze` at once for a
  second or more.
- **Every 10 s in a networked game**: `players:` with each player's ping (and spikes, the 99th
  percentile), their machine's top game speed and their input delay, then ours.

## What Lua can reach on disk

- **Sim and interface VMs**: only the VFS (`import`, `doscript`, `DiskFindFiles`, ...). Luau's
  `require` / `package`, which load `.lua` / `.luau` files from any path, are removed
  (`api::remove_disk_access`). FAF never uses them. Native modules were already off (`Lua::new`).
- **Init VM** (FAF's init file, run once to learn the mounts, then dropped): `io.open`, `io.dir`,
  `dofile` read the real file system, as moho's init does. `io.open` is read-only, and it and
  `dofile` only open `.lua` files (FAF reads mods' `mod_info.lua`). `os.remove` does nothing:
  FAF's client `init_faf.lua` empties the real game's shader cache
  (`%LOCALAPPDATA%\Gas Powered Games\Supreme Commander Forged Alliance\cache`) with it.
  Maps and mods never run in this VM; it only lists their folders.

## Building the downloads

```powershell
# Windows: dist\rts-core-engine-<version>-<commit>-windows-x64.zip
powershell -ExecutionPolicy Bypass -File dev\package\package-windows.ps1            # -Profile release for a quicker build
```

```sh
# macOS: dist/rts-core-engine-<version>-<commit>-macos.zip (universal: Apple silicon + Intel)
dev/package/package-macos.sh                 # dist profile, both architectures
dev/package/package-macos.sh release native  # quicker: this Mac's architecture only
```

```sh
# Linux: dist/rts-core-engine-<version>-<commit>-linux-x64.tar.gz (run on Linux)
dev/package/package-linux.sh                 # dist profile
dev/package/package-linux.sh release         # quicker
```

All build into `target/package` so the everyday build cache isn't disturbed. The `dist` profile
strips symbols (`strip = true`) and remaps source paths (`trim-paths = "all"`, a nightly
`cargo-features` entry), so no build-machine paths or user names end up in the binary. Each package holds the
program, `README.txt` (`dev/package/README.txt`: what to install, first start, where settings live),
`LICENSE.txt` (a copy of the repository's `LICENSE`, the GNU GPL v3; also what `--license`
prints) and `THIRD-PARTY-LICENSES.txt`: the licence notices of every Rust crate the program links
and of the vendored Luau and PL_MPEG. That file is committed (`dev/package/THIRD-PARTY-LICENSES.txt`),
so packaging doesn't need extra tools; after changing dependencies, regenerate it with
`dev/package/third-party-licenses.sh` (needs `cargo install --locked --features cli cargo-about`;
crates come from `about.toml`'s accepted licences and targets, the vendored texts from
`dev/package/vendored-licenses/`) and commit the result.

- **Windows**: built with `-C target-feature=+crt-static`. Without it the exe needs the VC++
  redistributable (`MSVCP140.dll`, `VCRUNTIME140.dll`), which a fresh PC may not have. The exe is
  unsigned, so SmartScreen warns until a code-signing certificate is used. It's a console program:
  double-clicking it also opens a console window, which shows the log.
- **macOS**: `RTS Core Engine.app` with an `Info.plist` (including `NSLocalNetworkUsageDescription`, needed
  for LAN discovery to be allowed), ad-hoc signed. Gatekeeper asks on first open until it's signed with
  a Developer ID and notarised (an Apple Developer account). Forged Alliance has no Mac version, so
  players copy the Windows install or fetch it with SteamCMD (`+@sSteamCmdForcePlatformType windows`).
- **Linux**: built on Ubuntu 24.04 (CI's `ubuntu-24.04` runner; a local build needs
  `build-essential pkg-config libasound2-dev` and rustup), so it needs glibc 2.39 or newer. It links
  ALSA (`libasound.so.2`); Vulkan, X11 and Wayland are loaded at run time. Steam on Linux installs
  Forged Alliance (run through Proton) where `src/install.rs` looks (native, Flatpak and Snap Steam),
  and the FAF client (or faf-linux) keeps FAF's Lua in `~/.faforever`.

## Builds in CI and releases

The workflow is `.github/workflows/release.yml`.

- **Every push to `main`** (and a manual run from the Actions tab, `workflow_dispatch`) runs the three
  package scripts above on GitHub's runners: `ubuntu-24.04`, `macos-latest` (arm64, building the
  universal binary) and `windows-latest`, each with the `dist` profile and no build cache. The
  archives appear as artifacts (`rts-core-engine-linux`, `rts-core-engine-macos`, `rts-core-engine-windows`) on the
  run's summary page, kept 30 days. Pull requests never run it.
- The Linux leg also runs the built binary's `--dump-api` and uploads the Lua type files
  (`sim`/`user` `.d.luau` and EmmyLua `.lua`, see [lua-api-types.md](lua-api-types.md)) as a
  fourth artifact, `rts-core-engine-lua-api` (`rts-core-engine-<version>-<commit>-lua-api.zip`).
- **A `v*` tag** builds the same and then creates a GitHub release for that tag holding the three
  archives and the Lua API zip, with notes generated from the merged PRs. To cut one, from an up-to-date `main`:
  `git tag v0.1.0 && git push origin v0.1.0`.
- A tag with a `-` in it (`v0.2.0-rc1`) is published as a **prerelease**.

The builds are unsigned as before (macOS ad-hoc), and use the committed `THIRD-PARTY-LICENSES.txt`,
so regenerate it before tagging if dependencies changed.

## Not done yet

- Third-party licence notices: the MIT / Apache / BSD crates we link ask for their notices to travel
  with binaries. `cargo about generate` can produce one file to embed beside `--license`.
- Code signing (Windows certificate; Apple Developer ID + notarisation), an app icon, and hiding the
  console window on Windows (`windows_subsystem = "windows"` plus attaching to a parent console for
  `--help`).
