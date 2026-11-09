//! Multiplayer over LAN and direct IP: SupCom's peer-to-peer lockstep.
//!
//! - [`link`]: a reliable, ordered channel to one peer over UDP.
//! - [`transport`]: where packets go: the UDP socket, or iroh (internet
//!   join codes: no port forwarding).
//! - [`conditioner`]: latency, jitter and loss on demand (testing).
//! - [`core`]: the lobby mesh (host, join, every client connected to every
//!   other) and its events.
//! - [`discovery`]: LAN game beacons and the listener behind FAF's LAN
//!   screen.
//! - [`lockstep`]: turns, input delay, desync detection, dropped players.
//! - [`wire`]: the messages.
//! - [`map_download`]: a host's map the player doesn't have, from FAF's
//!   vault.
//!
//! The interface's lobby (`ui::net_lobby`) drives `core`; a launched game
//! hands the same connection to the sim thread's `Lockstep`.

pub mod core;
pub mod bot;
pub mod conditioner;
pub mod discovery;
pub mod harness;
pub mod link;
pub mod lockstep;
pub mod map_download;
pub mod transport;
pub mod wire;

/// Most other clients in one game (players and observers).
pub const MAX_PEERS: usize = 63;
