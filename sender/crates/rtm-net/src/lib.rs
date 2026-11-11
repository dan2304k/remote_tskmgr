//! Outbound transports for RTM: UDP broadcast (multi-interface), UDP unicast and TCP unicast
//! (newline-delimited JSON, reconnecting). `rtm-sender` depends on `Target`,
//! `BroadcastInterfaces`, `Sender::new`, `Sender::send`, `Sender::describe`.
//!
//! Everything is plain std: one shared UDP socket for both broadcast and unicast, and one
//! background thread per TCP target fed by a bounded drop-oldest queue, so
//! [`Sender::send`] never blocks on the network.

pub mod iface;
mod tcp;

pub use iface::{select_broadcast, BroadcastDest, IfaceInfo};

use rtm_protocol::{Packet, PACKET_WARN_BYTES};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tcp::TcpWriter;

/// How long the interface list is cached before re-enumerating.
pub const IFACE_REFRESH: Duration = Duration::from_secs(10);
/// Minimum gap between two logged errors for the same destination.
pub const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BroadcastInterfaces {
    /// Every up, non-loopback IPv4 interface not matched by `skip_prefixes`.
    All,
    /// Only the interface that owns the default route (best effort; falls back to All).
    Default,
    /// Exactly these interface names (the skip list does not apply to explicit names).
    Named(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    UdpBroadcast {
        port: u16,
        interfaces: BroadcastInterfaces,
        /// Interface-name prefixes to ignore, e.g. ["docker", "br-", "veth", "virbr", "lo"].
        skip_prefixes: Vec<String>,
    },
    UdpUnicast(SocketAddr),
    /// Sender connects to this address and streams `\n`-terminated JSON. Reconnects with
    /// backoff (250 ms .. 10 s). Never blocks the caller; packets are dropped while down.
    TcpUnicast(SocketAddr),
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(#[from] rtm_protocol::ProtocolError),
    #[error("no targets configured")]
    NoTargets,
}

struct Broadcast {
    port: u16,
    interfaces: BroadcastInterfaces,
    skip_prefixes: Vec<String>,
    dests: Vec<BroadcastDest>,
    refreshed: Option<Instant>,
}

impl Broadcast {
    fn refresh_if_due(&mut self) {
        if self.refreshed.is_some_and(|t| t.elapsed() < IFACE_REFRESH) {
            return;
        }
        self.refreshed = Some(Instant::now());
        let ifaces = match iface::enumerate() {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "interface enumeration failed; broadcasting to 255.255.255.255 only");
                Vec::new()
            }
        };
        let default_ip = match self.interfaces {
            BroadcastInterfaces::Default => iface::default_route_ip(),
            _ => None,
        };
        let dests = select_broadcast(&ifaces, &self.interfaces, &self.skip_prefixes, default_ip);
        if dests != self.dests {
            tracing::info!(
                port = self.port,
                via = %format_dests(&dests),
                "broadcast interfaces updated"
            );
            self.dests = dests;
        }
    }

    /// Directed broadcasts first, then the limited broadcast (dests never contain it).
    fn addrs(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.dests
            .iter()
            .map(|d| d.addr)
            .chain(std::iter::once(Ipv4Addr::BROADCAST))
            .map(move |a| SocketAddr::V4(SocketAddrV4::new(a, self.port)))
    }
}

fn format_dests(d: &[BroadcastDest]) -> String {
    if d.is_empty() {
        return "none".into();
    }
    d.iter()
        .map(|d| format!("{}({})", d.iface, d.addr))
        .collect::<Vec<_>>()
        .join(", ")
}

pub struct Sender {
    targets: Vec<Target>,
    udp: Option<UdpSocket>,
    broadcasts: Vec<Broadcast>,
    unicast: Vec<SocketAddr>,
    tcp: Vec<TcpWriter>,
    last_error_log: HashMap<SocketAddr, Instant>,
    last_size_warn: Option<Instant>,
}

impl Sender {
    pub fn new(targets: Vec<Target>) -> Result<Self, NetError> {
        if targets.is_empty() {
            return Err(NetError::NoTargets);
        }
        let mut broadcasts = Vec::new();
        let mut unicast = Vec::new();
        let mut tcp_addrs: Vec<SocketAddr> = Vec::new();
        for t in &targets {
            match t {
                Target::UdpBroadcast {
                    port,
                    interfaces,
                    skip_prefixes,
                } => broadcasts.push(Broadcast {
                    port: *port,
                    interfaces: interfaces.clone(),
                    skip_prefixes: skip_prefixes.clone(),
                    dests: Vec::new(),
                    refreshed: None,
                }),
                Target::UdpUnicast(a) => {
                    if !unicast.contains(a) {
                        unicast.push(*a)
                    }
                }
                Target::TcpUnicast(a) => {
                    if !tcp_addrs.contains(a) {
                        tcp_addrs.push(*a)
                    }
                }
            }
        }
        let udp = if broadcasts.is_empty() && unicast.is_empty() {
            None
        } else {
            let s = UdpSocket::bind("0.0.0.0:0")?;
            s.set_broadcast(true)?;
            // Never stall the sample loop on a full socket buffer.
            s.set_nonblocking(true)?;
            Some(s)
        };
        let tcp = tcp_addrs
            .into_iter()
            .map(TcpWriter::spawn)
            .collect::<Result<Vec<_>, _>>()?;
        let mut me = Self {
            targets,
            udp,
            broadcasts,
            unicast,
            tcp,
            last_error_log: HashMap::new(),
            last_size_warn: None,
        };
        for b in &mut me.broadcasts {
            b.refresh_if_due();
        }
        Ok(me)
    }

    /// The targets this sender was built from.
    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    /// Serialise once, fan out to every target. UDP errors are logged per target and do not
    /// abort the others. Returns the serialised size in bytes (callers may warn > PACKET_WARN_BYTES).
    pub fn send(&mut self, packet: &Packet) -> Result<usize, NetError> {
        let json = packet.to_json()?;
        let len = json.len();
        if len > PACKET_WARN_BYTES
            && self
                .last_size_warn
                .is_none_or(|t| t.elapsed() >= ERROR_LOG_INTERVAL)
        {
            self.last_size_warn = Some(Instant::now());
            tracing::warn!(
                bytes = len,
                limit = PACKET_WARN_BYTES,
                "packet exceeds UDP size warning threshold"
            );
        }

        if let Some(sock) = &self.udp {
            let mut dests: Vec<SocketAddr> = Vec::new();
            for b in &mut self.broadcasts {
                b.refresh_if_due();
                for a in b.addrs() {
                    if !dests.contains(&a) {
                        dests.push(a);
                    }
                }
            }
            for a in &self.unicast {
                if !dests.contains(a) {
                    dests.push(*a);
                }
            }
            for a in dests {
                if let Err(e) = sock.send_to(json.as_bytes(), a) {
                    let due = self
                        .last_error_log
                        .get(&a)
                        .is_none_or(|t| t.elapsed() >= ERROR_LOG_INTERVAL);
                    if due {
                        self.last_error_log.insert(a, Instant::now());
                        tracing::warn!(dest = %a, error = %e, "udp send failed (further errors for this destination suppressed for 30 s)");
                    }
                }
            }
        }

        if !self.tcp.is_empty() {
            let mut line = json;
            line.push('\n');
            let line: Arc<str> = line.into();
            for w in &self.tcp {
                w.push(line.clone());
            }
        }
        Ok(len)
    }

    /// Human-readable summary for logs / tray tooltip, e.g.
    /// `UDP bcast :48000 via enp0s31f6(192.168.1.255), 255.255.255.255; UDP 192.168.1.50:48000; TCP 192.168.1.50:48000 (connected)`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for b in &self.broadcasts {
            let mut s = format!("UDP bcast :{} via ", b.port);
            if !b.dests.is_empty() {
                s.push_str(&format_dests(&b.dests));
                s.push_str(", ");
            }
            s.push_str("255.255.255.255");
            parts.push(s);
        }
        for a in &self.unicast {
            parts.push(format!("UDP {a}"));
        }
        for w in &self.tcp {
            let state = if w.is_connected() {
                "connected"
            } else {
                "reconnecting"
            };
            parts.push(format!("TCP {} ({state})", w.addr()));
        }
        parts.join("; ")
    }

    /// Whether every TCP target currently has a live connection (true when there are none).
    pub fn tcp_all_connected(&self) -> bool {
        self.tcp.iter().all(|w| w.is_connected())
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        // Signal every worker first so they wind down in parallel, then each TcpWriter's
        // Drop joins its thread (bounded wait).
        for w in &mut self.tcp {
            w.signal_stop();
        }
        self.tcp.clear();
    }
}
