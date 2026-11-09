//! Where the mesh's packets go: the UDP socket (LAN, direct IP) and,
//! beside it, an [iroh] endpoint for internet play without port forwarding.
//!
//! iroh dials peers by public key (the *join code*, 52 base32 characters),
//! finds them through n0's DNS address lookup, reaches them through a relay
//! server at once and moves to a direct path when hole punching works. Its
//! QUIC datagrams carry the same [`Link`](super::link::Link) packets as UDP,
//! so the link's reliability, RTT (the lobby's ping) and keepalives, and
//! everything above it, don't know which transport a peer is on.
//!
//! The mesh keys peers by socket address, so each iroh peer gets a stand-in
//! address (`[fd69:726f:6800::n]:1`, a unique-local prefix a LAN peer won't
//! have) and member addresses travel as `iroh:<code>` strings. A send to a
//! peer with no connection yet starts dialing and drops the packet: the
//! link resends it once the connection is up.
//!
//! Network threads only: none of this touches sim state.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use iroh::dns::{DnsResolver, FallbackMode};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointId};

use super::conditioner::{Dir, Fate, HoldQueue, fate};
use super::wire::PathKind;

/// The game's QUIC protocol id: peers on anything else are refused.
const ALPN: &[u8] = b"rts-core-engine/lockstep/1";
/// How a member address names an iroh peer.
pub const IROH_PREFIX: &str = "iroh:";
/// The stand-in addresses' prefix (`fd69:726f:6800::/48`).
const STANDIN: [u16; 3] = [0xfd69, 0x726f, 0x6800];
/// The smallest datagram iroh allows on a path (measured: 1162 bytes, QUIC's
/// 1200-byte minimum less its overhead): every link packet must fit.
const IROH_MIN_DATAGRAM: usize = 1162;
const _: () = assert!(super::link::MTU <= IROH_MIN_DATAGRAM);

/// iroh is used unless `RTS_NET_IROH=0` (LAN only: nothing contacts n0).
pub fn iroh_enabled() -> bool {
    std::env::var("RTS_NET_IROH").map_or(true, |v| v != "0")
}

/// An address that stands in for an iroh peer.
pub fn is_iroh(addr: SocketAddr) -> bool {
    index_of(addr).is_some()
}

fn standin(index: u32) -> SocketAddr {
    let ip = Ipv6Addr::new(STANDIN[0], STANDIN[1], STANDIN[2], 0, 0, 0, (index >> 16) as u16, index as u16);
    SocketAddr::V6(SocketAddrV6::new(ip, 1, 0, 0))
}

fn index_of(addr: SocketAddr) -> Option<u32> {
    let SocketAddr::V6(v6) = addr else { return None };
    let s = v6.ip().segments();
    (s[..3] == STANDIN && s[3..6] == [0, 0, 0]).then(|| (s[6] as u32) << 16 | s[7] as u32)
}

/// A join code (`iroh:` or a log's `code=` prefix optional; quotes, spaces,
/// dashes and case ignored) to the peer it names.
pub fn parse_code(text: &str) -> Option<EndpointId> {
    let t = text.trim();
    let t = t.strip_prefix("code=").unwrap_or(t);
    let t = t.trim_matches(|c| c == '"' || c == '\'');
    let t = t.strip_prefix(IROH_PREFIX).unwrap_or(t);
    let code: String = t.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    if code.len() != 52 {
        return None;
    }
    code.parse().ok()
}

/// How a join code is shown: the key in lowercase base32 (52 characters).
pub fn format_code(id: EndpointId) -> String {
    data_encoding::BASE32_NOPAD.encode(id.as_bytes()).to_ascii_lowercase()
}

pub struct Transport {
    udp: UdpSocket,
    iroh: Option<Arc<Iroh>>,
    /// Sends held by the [conditioner](super::conditioner), for the
    /// `net-hold` thread to make when due.
    held_out: Sender<(std::time::Instant, Vec<u8>, SocketAddr)>,
    /// Arrivals held by the conditioner, delivered by `recv_from` when due.
    held_in: Mutex<HoldQueue<(Vec<u8>, SocketAddr)>>,
}

/// A packet out now: over iroh for a stand-in address, else UDP.
fn send_now(udp: &UdpSocket, iroh: Option<&Iroh>, bytes: &[u8], to: SocketAddr) -> io::Result<()> {
    match index_of(to) {
        Some(index) => match iroh {
            Some(i) => i.send(index, bytes),
            None => Err(io::Error::new(io::ErrorKind::Unsupported, "iroh is off")),
        },
        None => udp.send_to(bytes, to).map(|_| ()),
    }
}

/// The thread that sends held packets when they're due (ends with the
/// transport). Its failures are the link's to notice, as a lost packet.
fn spawn_holder(udp: UdpSocket, iroh: Option<Arc<Iroh>>) -> Sender<(std::time::Instant, Vec<u8>, SocketAddr)> {
    let (tx, rx) = channel::<(std::time::Instant, Vec<u8>, SocketAddr)>();
    let spawned = std::thread::Builder::new().name("net-hold".into()).spawn(move || {
        let mut queue: HoldQueue<(Vec<u8>, SocketAddr)> = HoldQueue::default();
        loop {
            let got = match queue.next_due() {
                Some(due) => rx.recv_timeout(due.saturating_duration_since(std::time::Instant::now())).map_err(|e| matches!(e, std::sync::mpsc::RecvTimeoutError::Disconnected)),
                None => rx.recv().map_err(|_| true),
            };
            match got {
                Ok((due, bytes, to)) => queue.push(due, (bytes, to)),
                Err(true) => return,
                Err(false) => {}
            }
            while let Some((bytes, to)) = queue.pop_due(std::time::Instant::now()) {
                let _ = send_now(&udp, iroh.as_deref(), &bytes, to);
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "network: no thread for held packets (conditions apply as loss)");
    }
    tx
}

impl Transport {
    /// `udp` with its read timeout set (the network thread's poll). With
    /// `iroh`, an endpoint is bound too (a failure only logs: LAN play
    /// still works).
    pub fn new(udp: UdpSocket, iroh: bool) -> Transport {
        let iroh = if iroh {
            Iroh::start().map_err(|e| tracing::warn!(error = %e, "network: iroh unavailable (internet join codes off)")).ok()
        } else {
            None
        };
        let iroh = iroh.map(Arc::new);
        super::conditioner::init_from_env();
        let held_out = match udp.try_clone() {
            Ok(u) => spawn_holder(u, iroh.clone()),
            Err(e) => {
                tracing::warn!(error = %e, "network: no socket for held packets");
                channel().0
            }
        };
        Transport { udp, iroh, held_out, held_in: Mutex::new(HoldQueue::default()) }
    }

    pub fn local_port(&self) -> io::Result<u16> {
        Ok(self.udp.local_addr()?.port())
    }

    /// This client's join code, when iroh is up.
    pub fn join_code(&self) -> Option<String> {
        self.iroh.as_ref().map(|i| format_code(i.endpoint.id()))
    }

    pub fn send_to(&self, bytes: &[u8], to: SocketAddr) -> io::Result<()> {
        match fate(Dir::Out) {
            Fate::Pass => send_now(&self.udp, self.iroh.as_deref(), bytes, to),
            Fate::Drop => Ok(()),
            Fate::Hold(due) => {
                let _ = self.held_out.send((due, bytes.to_vec(), to));
                Ok(())
            }
        }
    }

    /// The next packet: a held one now due, one iroh already delivered,
    /// else from the UDP socket (waiting up to its read timeout). The
    /// conditioner may drop or hold what arrives.
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let deliver = |buf: &mut [u8], data: &[u8], from: SocketAddr| {
            let n = data.len().min(buf.len());
            buf[..n].copy_from_slice(&data[..n]);
            (n, from)
        };
        loop {
            if let Some((data, from)) = self.held_in.lock().ok().and_then(|mut q| q.pop_due(std::time::Instant::now())) {
                return Ok(deliver(buf, &data, from));
            }
            let iroh = self.iroh.as_ref().and_then(|i| i.rx.lock().ok().and_then(|rx| rx.try_recv().ok()));
            let (n, from) = match iroh {
                Some((from, data)) => deliver(buf, &data, from),
                None => self.udp.recv_from(buf)?,
            };
            match fate(Dir::In) {
                Fate::Pass => return Ok((n, from)),
                Fate::Drop => {}
                Fate::Hold(due) => {
                    if let Ok(mut q) = self.held_in.lock() {
                        q.push(due, (buf[..n].to_vec(), from));
                    }
                }
            }
        }
    }

    /// A member address or Direct Connect entry: `iroh:<code>` (or a bare
    /// code) for an iroh peer, else `host:port` (a name or an address,
    /// IPv4 first).
    pub fn resolve(&self, addr: &str) -> Result<SocketAddr, String> {
        if let Some(id) = parse_code(addr) {
            let i = self.iroh.as_ref().ok_or("that's an internet join code, but iroh is off on this machine")?;
            return Ok(standin(i.peer_index(id)));
        }
        let addrs: Vec<SocketAddr> = addr.to_socket_addrs().map_err(|e| format!("{addr}: {e}"))?.collect();
        addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()).copied().ok_or_else(|| format!("{addr}: no address"))
    }

    /// How another client reaches `addr`: `iroh:<code>` for an iroh peer.
    pub fn describe(&self, addr: SocketAddr) -> String {
        match (index_of(addr), &self.iroh) {
            (Some(index), Some(i)) => i.peer_id(index).map_or_else(|| addr.to_string(), |id| format!("{IROH_PREFIX}{}", format_code(id))),
            _ => addr.to_string(),
        }
    }

    /// Which path a peer's packets take: iroh direct or relayed (`None`
    /// for an iroh peer not connected yet), else UDP.
    pub fn path(&self, addr: SocketAddr) -> Option<PathKind> {
        let Some(index) = index_of(addr) else { return Some(PathKind::Udp) };
        let conn = self.iroh.as_ref()?.peers.lock().ok()?.slots.get(index as usize)?.conn.clone()?;
        let relay = conn.paths().iter().find(|p| p.is_selected()).map(|p| p.is_relay())?;
        Some(if relay { PathKind::IrohRelay } else { PathKind::IrohDirect })
    }

    /// Closes the iroh endpoint (peers see the connections end).
    pub fn close(&self) {
        if let Some(i) = &self.iroh {
            let ep = i.endpoint.clone();
            // Bounded: a relay that's gone mustn't hold up leaving.
            let _ = i.rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(1), ep.close()).await });
        }
    }
}

struct Slot {
    id: EndpointId,
    conn: Option<Connection>,
    dialing: bool,
    /// A packet was too large for this peer's datagrams (logged once).
    warned_size: bool,
}

#[derive(Default)]
struct Peers {
    index: BTreeMap<EndpointId, u32>,
    slots: Vec<Slot>,
}

impl Peers {
    fn index(&mut self, id: EndpointId) -> u32 {
        if let Some(&i) = self.index.get(&id) {
            return i;
        }
        let i = self.slots.len() as u32;
        self.slots.push(Slot { id, conn: None, dialing: false, warned_size: false });
        self.index.insert(id, i);
        i
    }
}

struct Iroh {
    rt: tokio::runtime::Runtime,
    endpoint: Endpoint,
    peers: Arc<Mutex<Peers>>,
    tx: Sender<(SocketAddr, Bytes)>,
    rx: Mutex<Receiver<(SocketAddr, Bytes)>>,
}

impl Iroh {
    fn start() -> Result<Iroh, String> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("iroh").enable_all().build().map_err(|e| e.to_string())?;
        // Binding is local and quick; the relay and the DNS record come up
        // in the background (a join before that is retried by the link).
        // DNS (finding a peer by its join code, reaching n0's relays): the
        // system's nameservers with the public ones (Cloudflare, Google,
        // Quad9) racing them from the start, rather than iroh's default of
        // trying the public ones only after every system one has failed. A
        // system nameserver can be unusable: a router's link-local IPv6
        // address (`fe80::1`) answers from `fe80::1%<interface>`, which
        // n0-dns-resolver 0.1 drops as an unexpected source (seen on a Windows
        // laptop: every lookup timed out and joins by code failed).
        let dns = DnsResolver::builder().with_system_defaults().with_fallback_mode(FallbackMode::Eager).build();
        let mut builder = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]).dns_resolver(dns);
        // Dev switch: relay only (no direct paths), the case when hole
        // punching fails.
        if std::env::var("RTS_NET_IROH_RELAY_ONLY").is_ok_and(|v| v == "1") {
            builder = builder.clear_ip_transports();
            tracing::warn!("network: iroh relay only (RTS_NET_IROH_RELAY_ONLY)");
        }
        let endpoint = rt.block_on(builder.bind()).map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let iroh = Iroh { rt, endpoint, peers: Arc::new(Mutex::new(Peers::default())), tx, rx: Mutex::new(rx) };
        tracing::info!(code = format_code(iroh.endpoint.id()), "network: iroh up (internet join code)");
        let (ep, peers, tx) = (iroh.endpoint.clone(), iroh.peers.clone(), iroh.tx.clone());
        iroh.rt.spawn(async move {
            while let Some(incoming) = ep.accept().await {
                let (peers, tx) = (peers.clone(), tx.clone());
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) => adopt(conn, &peers, &tx),
                        Err(e) => tracing::debug!(error = %e, "network: iroh connection refused"),
                    }
                });
            }
        });
        let ep = iroh.endpoint.clone();
        iroh.rt.spawn(async move {
            ep.online().await;
            tracing::info!("network: iroh online (relay reachable)");
        });
        Ok(iroh)
    }

    fn peer_index(&self, id: EndpointId) -> u32 {
        self.peers.lock().map(|mut p| p.index(id)).unwrap_or(0)
    }

    fn peer_id(&self, index: u32) -> Option<EndpointId> {
        self.peers.lock().ok()?.slots.get(index as usize).map(|s| s.id)
    }

    fn send(&self, index: u32, bytes: &[u8]) -> io::Result<()> {
        let mut peers = self.peers.lock().map_err(|_| io::Error::other("iroh state poisoned"))?;
        let Some(slot) = peers.slots.get_mut(index as usize) else { return Err(io::Error::new(io::ErrorKind::NotFound, "unknown iroh peer")) };
        if let Some(conn) = &slot.conn {
            if conn.close_reason().is_none() {
                return match conn.send_datagram(Bytes::copy_from_slice(bytes)) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        if !slot.warned_size && conn.max_datagram_size().is_some_and(|m| bytes.len() > m) {
                            slot.warned_size = true;
                            tracing::warn!(len = bytes.len(), max = ?conn.max_datagram_size(), "network: packet larger than the iroh path allows");
                        }
                        Err(io::Error::other(e.to_string()))
                    }
                };
            }
            slot.conn = None;
        }
        // Not connected: dial (once at a time) and drop this packet.
        if !slot.dialing {
            slot.dialing = true;
            let (id, ep, peers, tx) = (slot.id, self.endpoint.clone(), self.peers.clone(), self.tx.clone());
            self.rt.spawn(async move {
                match ep.connect(id, ALPN).await {
                    Ok(conn) => adopt(conn, &peers, &tx),
                    Err(e) => tracing::warn!(peer = %id.fmt_short(), error = %e, "network: iroh dial failed"),
                }
                if let Ok(mut p) = peers.lock() {
                    let i = p.index(id);
                    p.slots[i as usize].dialing = false;
                }
            });
        }
        Ok(())
    }
}

/// A connection (dialed or accepted) becomes the peer's send path, and its
/// datagrams are read into the receive queue until it closes.
fn adopt(conn: Connection, peers: &Arc<Mutex<Peers>>, tx: &Sender<(SocketAddr, Bytes)>) {
    let id = conn.remote_id();
    let Ok(mut p) = peers.lock() else { return };
    let index = p.index(id);
    p.slots[index as usize].conn = Some(conn.clone());
    drop(p);
    tracing::info!(peer = %id.fmt_short(), max_datagram = ?conn.max_datagram_size(), "network: iroh connected");
    let (peers, tx) = (peers.clone(), tx.clone());
    tokio::spawn(async move {
        let from = standin(index);
        let reason = loop {
            match conn.read_datagram().await {
                Ok(data) => {
                    if tx.send((from, data)).is_err() {
                        return;
                    }
                }
                Err(e) => break e,
            }
        };
        tracing::info!(peer = %id.fmt_short(), %reason, "network: iroh connection closed");
        if let Ok(mut p) = peers.lock() {
            let slot = &mut p.slots[index as usize];
            if slot.conn.as_ref().is_some_and(|c| c.stable_id() == conn.stable_id()) {
                slot.conn = None;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standin_addresses_round_trip_and_never_match_real_ones() {
        for i in [0, 1, 65535, 65536, 1 << 20] {
            assert_eq!(index_of(standin(i)), Some(i));
        }
        assert!(!is_iroh("127.0.0.1:16010".parse().unwrap()));
        assert!(!is_iroh("[fd69:726f:6801::1]:1".parse().unwrap()));
        assert!(!is_iroh("[::1]:16010".parse().unwrap()));
    }

    #[test]
    fn join_codes_parse_however_they_were_pasted() {
        let id = iroh::SecretKey::from_bytes(&[7; 32]).public();
        let code = format_code(id);
        assert_eq!(code.len(), 52);
        for text in [code.clone(), format!("iroh:{code}"), code.to_uppercase(), format!("  {} - {}  ", &code[..26], &code[26..])] {
            assert_eq!(parse_code(&text), Some(id), "{text}");
        }
        // As copied from a log line or a chat message.
        for text in [format!("\"{code}\""), format!("code=\"{code}\""), format!("'{code}'")] {
            assert_eq!(parse_code(&text), Some(id), "{text}");
        }
        assert_eq!(parse_code("192.168.1.4:16010"), None);
        assert_eq!(parse_code(&code[1..]), None);
    }
}
