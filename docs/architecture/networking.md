# Networking (LAN / direct connect / internet join codes)

SupCom-style peer-to-peer lockstep: every client runs the whole sim, only
player input crosses the network. Code in `src/net/` (transport, lockstep)
and `src/ui/net_lobby.rs` + `src/ui/lobby.lua` (FAF's lobby natives).

## Layers

| Layer | File | What |
|---|---|---|
| Link | `net/link.rs` | Reliable, ordered byte channel to one peer over UDP: numbered 1100-byte chunks, cumulative ack + 64-bit selective ack in every packet, RTT-based resend (`srtt + 4*rttvar`, doubling, 30 ms..2 s), keepalive every 250 ms. RTT is measured from every packet, keepalives included (as moho's `CNetUDPConnection`: a ring of sent-packet times matched by echoed serial, MohoEngine.dll 0x10080f00): each header carries the sender's clock, the last stamp it got and how long it held it, so an idle lobby's `ping` stays current. Pure state machine (tested under 30% loss + reordering). |
| Conditioner | `net/conditioner.rs` | Testing: latency, jitter, spikes and loss on this client's packets each way, set from the stats bar's network panel or `RTS_NET_LAG`. |
| Transport | `net/transport.rs` | Where packets go: the UDP socket, or an [iroh](https://iroh.computer) endpoint beside it for peers joined by internet join code (below). |
| Mesh | `net/core.rs` | One UDP socket per client (listens and connects). Host = member 1; a joiner gets an id and the other members' addresses (`Welcome`) and connects to each: full mesh, no relay. Events (`Event`) queue for the lobby UI, then the lockstep. `Established` gossip feeds FAF's `establishedPeers`. Peers silent 20 s are lost; `Bye` on close is immediate. |
| Wire | `net/wire.rs` | Frames: `u32` length + tag. Lua tables as `LuaData` (canonical key order); commands as replay lines (`sim::input::encode`, exact fixed-point bits). |
| Discovery | `net/discovery.rs` | Host beacon every 1 s (FAF's `GameConfigRequested` table + port) to each interface's subnet broadcast, 255.255.255.255 and multicast 239.255.77.77, port 16000. Listener (SO_REUSEADDR/PORT, several local clients) fills `Address` from the sender. |
| Lockstep | `net/lockstep.rs` | Turns, input delay, speed, catch-up, desync detection, dropped players (below). |
| Lobby natives | `ui/net_lobby.rs`, `ui/lobby.lua` | `InternalCreateLobby` for 'UDP'/'TCP' (FAF's LAN screen: Create via `gamecreate.lua`, Join / Direct Connect via `gameselect.lua`), `InternalCreateDiscoveryService`, `ValidateIPAddress`, in-game `SessionSendChatMessage` / `GetSessionClients`. 'None' (Skirmish) stays offline: there `SessionSendChatMessage` to the local client loops back to it (`LocalChat`; FAF's notify overlay, the ACU upgrade ETA). |
| Harness | `net/harness.rs`, `net/bot.rs` | Headless test games (`--net-host`, `--net-join`, `--net-bot`). |

No encryption or authentication over UDP (LAN between friends, like
SupCom's LAN); iroh connections are encrypted and authenticated by key.

## Internet play: iroh join codes

Every networked lobby also binds an iroh endpoint (`RTS_NET_IROH=0` turns
it off: LAN only, nothing contacts n0). Its public key is the **join code**:
52 base32 characters, new each launch, no account. The host's lobby posts it
in the chat and copies it to the clipboard; a friend pastes it into Direct
Connect's address box (any port: FAF requires one) and joins. Edits paste
with Ctrl/Cmd+V.

- **Finding the host:** iroh's n0 preset publishes a signed record
  (its relay URL and addresses) to n0's DNS address lookup, and a joiner
  resolves the code through it. The record is signed by the key, so it
  can't be forged.
- **Reaching the host:** first packets go through the host's home relay
  (n0's public relays), so it works behind any NAT; iroh then hole-punches
  and moves to a direct path when it can.
- **In the mesh:** each iroh peer gets a stand-in address
  (`[fd69:726f:6800::n]:1`), so `core`, `link` and the lockstep are
  unchanged. QUIC datagrams carry the same link packets (the link keeps its
  reliability, RTT and keepalives). The link's largest packet is one full
  chunk (1139 bytes), under iroh's 1162-byte datagram limit on a fresh path.
  `Hello` carries each member's code; a joiner that came by code is given
  the others' codes rather than their LAN addresses, so LAN and internet
  players can share a lobby.
- **Logs:** `network: iroh up ... code=`, `iroh online (relay reachable)`,
  `peer connected over iroh path=IrohDirect|IrohRelay`. iroh's own crates
  log at warn and above only (`main.rs`).
- **Dev switch:** `RTS_NET_IROH_RELAY_ONLY=1` disables direct paths (the
  case where hole punching fails).
- **Relays:** n0's public relays are free but rate-limited, with no
  guarantee. For a release, run `iroh-relay` (open source) on a small server
  and point the endpoint at it (`RelayMode::Custom`).

## Ping grid

Ctrl+Shift+P (Cmd on the Mac), in the lobby and in game, toggles a window
with every player's round trip to every other: row = the player measuring,
column = the one measured, so a one-sided link shows. Each client sends its
own row once a second (`Msg::Pings`, handled in `core`, never passed to the
lobby or the lockstep). Each cell is a colour, from the smoothed RTT
(green < 80 ms, yellow < 150, red above); hovering one lights its row and
column and spells the link out: smoothed ms, the 99th percentile of its last
512 samples (`Link::rtt_p99_ms`) and the route (iroh relay, iroh direct or
LAN / IP). Column names are turned to read bottom to top
(`Text:SetVertical`, an engine extension). It sizes itself to the screen:
the largest names and cells (up to 24 px) that fit, from 2 players to 63 on
a 1024×768 window. `RTS_PING_GRID_FAKE=<players>` fills it with made up
players to check the
layout (e.g. `RTS_PING_GRID_FAKE=63 cargo run -- --ui-lua
"__engine_toggle_ping_grid()"`). `ui/ping_grid.lua` builds it from FAF's
maui controls (no FAF file changes); the hotkey is handled in `UserVm::key`
before anything else.

## Lobby ping (what FAF shows)

`GetPeer(id)` / `GetPeers()` give `ping` as moho does (float ms from the
connection, `MakePeerTable` 0x1038ae80), `quiet`, `status`,
`establishedPeers`. FAF's lobby shows a player's ping bar only when it is
over 500 ms (`GUI.pingThread`; its tooltip "Only shows when > 500" is on the
hidden bar, so it can't be hovered otherwise): on a healthy LAN the Ping
column is empty by design. Observers get `P:<floor(ping)>` in the observer
list (0 on loopback / a fast LAN). Connection state shows as the slot name in
bold once `CalcConnectionStatus` sees 'Established'.

## Same version only

Every connection opens with `Hello { protocol, build, .. }`. `protocol` (`wire::PROTOCOL`) is the
message format; `build` (`core::build_id`) is `<version>+<commit>, Lua <hash>`: the Cargo version,
the git commit `build.rs` stamps in (`RTS_BUILD_ID`, also `--version`), and `Vfs::code_hash` of the
FAF Lua the engine loaded (`.lua` + `.bp`, not maps / mods / preferences, CR-insensitive). Either
differing would desync, so the receiver refuses the connection. A different protocol is refused as
`IncompatibleVersion`. A different build is refused with a sentence naming both versions, which
FAF's lobby shows in its "Connection failed" dialog (`Strings[reason] or reason`).

The Lua hash (`Vfs::code_hash`) covers every `.lua` and `.bp` in the game-code folders (`/lua/`,
`/units/`, `/projectiles/`, `/effects/`, `/env/`, `/loc/`, `/meshes/`, `/props/`, `/schook/`,
`/textures/`). It hashes the files as they are in the VFS, before translation and hooks, with
carriage returns dropped so line endings don't matter. Each file is hashed with xxh3 on rayon, then
the results are combined in path order, so the result doesn't depend on which thread finished first.
It runs on a thread at startup (a few seconds from a cold disk); `build_id` waits for it if a lobby
opens first. So a host and a joiner on different FAF versions are refused before the game starts.

Mods vary per player, so they aren't part of the build: after `Hello` each side sends `Msg::Mods`,
its installed sim mods as (uid, hash of the mod's files), hashed on a thread at startup like the
code. The host can't launch (`TryLaunch`, engine hook) while a player lacks one of the game's sim
mods, has a different copy, or hasn't sent its list; the lobby chat says who and which mod
(`NetCore::mod_problems`). Mods aren't transferred: players install them themselves (FAF's client
downloads them from its vault), as in the base game and FAF.

## Maps from FAF's vault

The FAF client downloads the host's map before the game starts; here the lobby does
(`net::map_download`). When a host's message (`GameInfo` on joining, `GameOptions`, `Launch`) names a
map the player doesn't have, `ui/lobby.lua` holds it, and every event after it, and calls
`__map_download`, which fetches `https://content.faforever.com/maps/<folder>.zip` on a thread (no
login; FAF's API needs one). `<folder>` is the scenario path's folder, `name.vNNNN`; a path without a
version (the stock maps) isn't a vault map and isn't tried. The zip is unpacked into the vault's
`maps` folder (`custom_vault_path` from the FAF client's `fa_path.lua`, else `My Documents/My
Games/Gas Powered Games/Supreme Commander Forged Alliance`; `--vault` wins), first into a hidden
`.<folder>.download` folder that is then renamed, so a broken download never leaves half a map for
the init file to mount next run. It is then mounted at `/maps/<folder>` in the running VFS
(`Vfs::mount_folder`), which the interface and the session share, and the held events go on: FAF's
lobby finds the map as if it had always been there. The chat says when it starts, finishes or fails;
a failure (not in the vault, offline) just lets FAF's lobby report the map missing as before.
Downloads are capped at 1 GB, unpacking at 4 GB, and zip paths that leave the folder are refused. `RTS_MAP_VAULT_URL` replaces
the content server's address (a mirror, or a local server when testing).

The chat names the map and its size once the vault says (or after a second if it doesn't), then
says it's downloaded: a typical few-MB map shows just those two lines. A download still going after
2 s (a big map, a slow line) adds a progress line every 3 s ("Downloading the map: 42 of 180 MB
(23%)"), since the lobby is held meanwhile and would otherwise look stuck.

The lobby asks `__map_download` whether the map is there, not `DiskGetFileInfo`: FAF's `userInit.lua`
caches that function's answers, a missing file included, so asking it first would hide the download.
Nothing checks the downloaded copy against the host's beyond the version in its name; a different
copy would show as a desync at the first checksum.

## Lockstep

- **Turns.** At the start of tick `t` each client sends every other one turn:
  its new commands stamped for `sched = t + delay`. A turn also means
  "nothing more from me up to `sched`". Tick `T` runs once every peer's
  watermark (last `sched` received) is `>= T`; commands apply in
  `(tick, client, seq)` order. Everyone waits at this sync point every tick.
- **Input delay** (ticks, per client), sized two ways, whichever needs more:
  - *From the links*: `ceil((one way + 30 ms) / tick_ms)`, min 2, max 50, where
    one way = the slowest peer's `max(RTT/2, p99 RTT - RTT/2)` (a spike can be
    all one way; `Link::rtt_p99_ms` over 512 samples). `RTS_NET_JITTER_MS` sets
    the 30 ms margin. `tick_ms` is real time at the current speed (100 ms at
    1x), so faster games take more ticks. LAN at 1x: 2 ticks = 200 ms (SupCom:
    fixed 500 ms `net_Lag`).
  - *From what actually happened*: every receiver times, per sender, how long
    it waited for their turns, else how many whole ticks early they were, and
    once a second reports it in its turn (`Turn.slack`; negative = ticks
    waited). A sender someone waited for raises its delay at once by the ticks
    they waited (at most +3 a second: a player still loading mustn't put
    seconds of lag on everyone), then ignores reports until turns with the new
    delay have arrived (no overshoot). It falls one tick a second, and only
    after 15 s with 2 ticks to spare everywhere; "just in time" (0) holds it.
  A falling delay skips a turn, so `sched` stays strictly increasing. Clients
  needn't agree: each turn carries its own `sched`. The launch sets the
  starting delay for everyone (from the links).

  Bad connections on one machine: the stats bar's network panel (click its `net` readout) sets
  latency, jitter, spikes and loss on this client's packets, outgoing and incoming, while the game
  runs (`net/conditioner.rs`: the transport asks it the fate of every UDP and iroh packet; held
  ones overtake each other). `RTS_NET_LAG=<base>:<jitter>[:<1 in n>:<spike>[:<loss ‰>]]` (ms) sets
  the outgoing ones at start, for headless runs. Stalls and the delay chosen show in the log's
  `lockstep:` lines; changes log as `network: conditions`.

- **Slow machines slow the game.** Each client measures its tick cost
  (smoothed, waiting excluded) and sends `capacity` = fastest speed it keeps
  up with at 1.25x headroom. Everyone paces at `min(requested speed, own
  capacity, peers' capacities)`. Logged as `lockstep: game speed ...
  limited_by=<name>`. Test switch: `RTS_NET_SLOW_MS=n` adds n ms to every tick.
- **Catch-up.** Turns carry the sender's tick (`at`). A client behind every
  other one by more than half a tick shortens its tick period (up to 2x) until
  level (a late loader otherwise stays behind and everyone waits for it every
  tick). Real-time pacing only; never sim state.
- **Desync detection.** Every turn carries the sender's checksum of the
  previous tick (any held back while the input delay falls go in the next
  one), so a mismatch names the first tick that differs; the last 128 are
  kept to compare. Mismatches log `lockstep: DESYNC tick=... peer=...`; the
  first with each peer swaps the checksum's parts (`ChecksumParts`) and logs
  `lockstep: DESYNC parts ... differing=["orders", ...]`
  and open FAF's "Desync Detected" dialog (`uimain.ShowDesyncDialog`, polled
  by `ui/lobby.lua` as `__net_desync_poll`).
- **Local input only through turns.** In a networked game the player's
  input is taken once per tick, by lockstep; input that arrives while the
  tick waits for the peers goes in the next turn.
- **Disconnect dialog and eject votes** (moho's behaviour, FAF's
  `/lua/ui/dialogs/disconnect.lua` unchanged). The interface calls
  `uimain.UpdateDisconnectDialog()` twice a second; FAF opens the
  "Connectivity" dialog when a client's `quiet` (time since its last *turn*,
  not its last packet: a frozen game still sends keepalives) passes 5 s, or
  eject votes are pending. `EjectSessionClient(i)` (the Eject button, enabled
  by FAF after `DisconnectionDelay02` - 5 s; FAF force-ejects after
  `max(2 x delay, 85)` s) broadcasts `EjectVote`; once every remaining client
  has voted, each drops the player (below). Votes are only for a client
  still connected (lagging, frozen): FAF offers Eject only for those. A
  client whose connection is gone (it left, e.g. from the score screen, timed
  out or was ejected) is dropped by the engine at once, windowed or headless,
  and counts as ejected by every client still in (`ejectedBy`), so FAF's
  dialog closes. `quiet` is 0 for us, for a client
  already gone, while paused, and for a peer that hasn't sent a turn yet
  (loading) for its first minute.
- **Dropped player.** Each client broadcasts the watermark it holds
  (`Dropped`); when all remaining clients have reported, the highest is the
  cut; the client holding it (lowest id among equals) relays missing turns
  (`Forward`); everyone stops waiting for the peer after the cut. Its army
  stays, taking no orders. A player who leaves says goodbye and is dropped at
  once; a crashed one's link times out after 20 s. A frozen but connected
  one shows in the dialog after 5 s quiet and Eject resumes the game.
- **Leaving while stalled.** The wait for players' turns ends when the
  player leaves (`SessionEndGame`) or the session closes (back to the menu):
  `SimControl::left`, which the engine gives the lockstep
  (`Lockstep::set_stop`). Otherwise a game stalled on a peer never lets the
  engine thread see it, and the front end never loads.
- **Pause** is everyone's: a pause or resume broadcasts `Pause`, every client
  follows, and the interface gets FAF's `gamemain.OnPause(pausedBy, 3)` /
  `OnResume()` (FAF lets the pauser resume at once, others after 10 s).
- **Game speed** changes (`+`/`-`, `SetGameSpeed`, `WLD_GameSpeed`) are commands
  (`Toggle::GameSpeed(step)`, replay field `w<step>`) in the sender's next turn, applied
  at their tick by every client in `(tick, source, seq)` order, so all clients agree on
  the speed and on who changed it (`uimain.NoteGameSpeedChanged(<their session client>,
  step)`). Until then `SimControl::speed` holds the request and `SimControl::agreed` the
  applied speed (what the game paces at and the interface shows). The lobby's
  `GameSpeed` option rules: `normal` (+0) and `fast` (+4) fix it (requests are refused,
  and every client ignores one that arrives anyway), `adjustable` allows changes. The
  slowest machine's capacity still caps the effective speed.

## Running

### Windowed

Main menu → Multiplayer LAN. Create Game hosts (`gamecreate.lua`), the list shows LAN games (beacons), and Direct Connect takes `ip:port` or a join code.

```sh
cargo run
```

### Headless test games

The test harness (`net/harness.rs`, `net/bot.rs`) runs a networked game with no window and no lobby, so lockstep and desyncs can be tested from a script. One process hosts and the others join:

```sh
# The host: waits for 4 players (itself included), then launches.
cargo run --release -- --headless --net-host 4 --armies 8 --net-bot --speed 1 --ticks 1200 --log h.log
# Each of the other 3 players, pointed at the host:
cargo run --release -- --headless --net-join 10.0.0.5:16010 --net-bot --speed 1 --ticks 1200 --log j1.log
```

| Option | What it does |
|---|---|
| `--net-host N` | Host a game and launch it once `N` players, counting the host, have joined. Listens on 16010, or the next free port above it (`--net-port` to choose) |
| `--net-join ADDR` | Join the game hosted at `ip:port` |
| `--armies 8` | Total armies: the players take start spots 1 to `N`, and AI fills the rest (here spots 5-8) |
| `--net-bot` | Each client gives random orders, so real input goes through the lockstep |
| `--speed 1` | Real time at normal speed (10 ticks a second). Without it a headless run goes as fast as it can; with it, turns, input delay and waits behave as they would for players |
| `--ticks 1200` | Stop after 1200 ticks (2 minutes of game time) |
| `--log h.log` | A log file per client, so the runs can be compared |
| `--net-name NAME` | The player's name in the lobby (default `Player`) |

To check for desyncs, set `RTS_TRACE_TICKS=0-1200` on every client and diff the `trace tick=` lines across the logs. Each log ends with `lockstep: totals`: turns, waits and desyncs. `RTS_NET_LAG=<base>:<jitter>` adds latency and jitter to one client's packets (or the network panel, in a windowed client) (see [Lockstep](#lockstep) above).

Ports: game 16010 up (port 0 = first free from 16010), discovery 16000.
Windows needs an inbound UDP rule (the test PC has "rts-core-engine game (UDP
16000-16080)", LocalSubnet + Tailscale only). macOS asks for Local Network
access the first time; until allowed, LAN unicast fails with "No route to
host" (subnet broadcast and loopback still work), logged once per peer as
`network: send failed ... No route to host (os error 65)`. The symptom: LAN
games show in the list (beacons arrive) but joins time out. The permission
belongs to the app that launched the binary (Terminal, Zed, ...): System
Settings > Privacy & Security > Local Network. Apple's own tools (`nc`) are
exempt, so a working `nc -u` test doesn't clear the engine. Editors and
terminals without `NSLocalNetworkUsageDescription` (Zed) may never get the
prompt, so for LAN games on the Mac run the engine as its own app:
`dev/macos/make-app.sh` (rebuild it with the binary after a profile change),
then `open -n "target/debug/RTS Core Engine.app" --args --log game1.log` (one per
instance; it runs from the repo). macOS asks once for "RTS Core Engine".

A join that fails before the host answers hits a FAF lobby bug:
`ConnectionFailed` parents its dialog to `GUI.panel`, which a joiner only
gets from `CreateUI` once connected, so `Group(nil)` throws (moho's
`InternalCreateGroup` throws on a nil parent too: "Expected a game
object"). As in the real game, the "Connecting..." dialog's Abort Connect
still returns to the menu.

Dev switches: `RTS_NET_JITTER_MS`, `RTS_NET_SLOW_MS`, `RTS_NET_GAME_SPEED` (the harness's lobby Game Speed: `normal` (default, fixed), `fast` or `adjustable`), `RTS_GAME_UI_LUA=file`
(Lua run in the game's interface after a lobby launch, for scripted UI
tests), `__engine_controls()` (every live control, to find a button by its
label in `--ui-lua` scripts), `__engine_screenshot_now = true` (with
`--screenshot`: capture this frame, e.g. a dialog while the sim is stalled).
