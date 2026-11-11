//! Outbound transports for RTM: UDP broadcast (multi-interface), UDP unicast and TCP unicast
//! (newline-delimited JSON, reconnecting). `rtm-sender` depends on `Target`,
//! `BroadcastInterfaces`, `Sender::new`, `Sender::send`, `Sender::describe`.
//!
//! Everything is plain std: one IPv4 UDP socket shared by broadcast and unicast (plus an IPv6
//! one, bound the first time an IPv6 destination is sent to), and one background thread per
//! TCP target fed by a bounded drop-oldest queue, so [`Sender::send`] never blocks on the
//! network. Host-name targets are resolved off the calling thread (see [`resolve`]).

pub mod iface;
pub mod resolve;
mod tcp;

pub use iface::{select_broadcast, BroadcastDest, IfaceInfo};
pub use resolve::{resolve_host, system_resolver, Resolver, UDP_RESOLVE_INTERVAL};

use resolve::UdpHosts;
use rtm_protocol::{Packet, PACKET_WARN_BYTES};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tcp::{Dest, TcpWriter};

/// How long the interface list is cached before re-enumerating.
pub const IFACE_REFRESH: Duration = Duration::from_secs(10);
/// Minimum gap between two logged errors for the same destination.
pub const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BroadcastInterfaces {
    /// Every up, non-loopback IPv4 interface not matched by `skip_prefixes` (see `iface::skipped_by_prefix`).
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
    /// UDP unicast to a host name, re-resolved in the background every
    /// [`UDP_RESOLVE_INTERVAL`]; nothing is sent to it until the first lookup succeeds, and
    /// the last good address is kept while lookups fail.
    UdpHost {
        host: String,
        port: u16,
    },
    /// Like `TcpUnicast`, but the name is resolved on the TCP thread before every connect
    /// attempt (IPv4 addresses first), so a viewer whose address changes is followed.
    TcpHost {
        host: String,
        port: u16,
    },
}

/// Knobs for [`Sender::with_options`]; [`Sender::new`] uses the defaults.
#[derive(Clone)]
pub struct SenderOptions {
    /// Host-name lookups (the system resolver by default; tests inject a fake).
    pub resolver: Resolver,
    /// How often UDP host targets are re-resolved.
    pub udp_resolve_interval: Duration,
}

impl Default for SenderOptions {
    fn default() -> Self {
        Self {
            resolver: system_resolver(),
            udp_resolve_interval: UDP_RESOLVE_INTERVAL,
        }
    }
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
    /// IPv4 socket (broadcast + unicast), bound in `new` when there is any UDP target.
    udp: Option<UdpSocket>,
    /// IPv6 socket, bound on the first send to an IPv6 destination.
    udp6: Option<UdpSocket>,
    broadcasts: Vec<Broadcast>,
    unicast: Vec<SocketAddr>,
    udp_hosts: Option<UdpHosts>,
    tcp: Vec<TcpWriter>,
    last_error_log: HashMap<SocketAddr, Instant>,
    last_size_warn: Option<Instant>,
}

impl Sender {
    pub fn new(targets: Vec<Target>) -> Result<Self, NetError> {
        Self::with_options(targets, SenderOptions::default())
    }

    pub fn with_options(targets: Vec<Target>, opts: SenderOptions) -> Result<Self, NetError> {
        if targets.is_empty() {
            return Err(NetError::NoTargets);
        }
        let mut broadcasts = Vec::new();
        let mut unicast = Vec::new();
        let mut hosts: Vec<(String, u16)> = Vec::new();
        let mut tcp_dests: Vec<Dest> = Vec::new();
        let mut add_tcp = |d: Dest| {
            if !tcp_dests.iter().any(|x| x.label() == d.label()) {
                tcp_dests.push(d);
            }
        };
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
                Target::TcpUnicast(a) => add_tcp(Dest::Addr(*a)),
                Target::UdpHost { host, port } => {
                    let h = (host.clone(), *port);
                    if !hosts.contains(&h) {
                        hosts.push(h);
                    }
                }
                Target::TcpHost { host, port } => add_tcp(Dest::Host {
                    host: host.clone(),
                    port: *port,
                    resolver: opts.resolver.clone(),
                }),
            }
        }
        // IPv4 socket only when something may be sent over IPv4 (a host name may resolve to
        // either family); IPv6 destinations get their own socket on first use.
        let need_v4 =
            !broadcasts.is_empty() || !hosts.is_empty() || unicast.iter().any(SocketAddr::is_ipv4);
        let udp = if !need_v4 {
            None
        } else {
            let s = UdpSocket::bind("0.0.0.0:0")?;
            s.set_broadcast(true)?;
            // Never stall the sample loop on a full socket buffer.
            s.set_nonblocking(true)?;
            Some(s)
        };
        let udp_hosts = if hosts.is_empty() {
            None
        } else {
            Some(UdpHosts::spawn(
                hosts,
                opts.resolver.clone(),
                opts.udp_resolve_interval,
            )?)
        };
        let tcp = tcp_dests
            .into_iter()
            .map(TcpWriter::spawn)
            .collect::<Result<Vec<_>, _>>()?;
        let mut me = Self {
            targets,
            udp,
            udp6: None,
            broadcasts,
            unicast,
            udp_hosts,
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

        let mut dests: Vec<SocketAddr> = Vec::new();
        for b in &mut self.broadcasts {
            b.refresh_if_due();
            for a in b.addrs() {
                if !dests.contains(&a) {
                    dests.push(a);
                }
            }
        }
        let resolved = self
            .udp_hosts
            .as_ref()
            .map(|h| h.snapshot())
            .unwrap_or_default();
        for a in self
            .unicast
            .iter()
            .copied()
            .chain(resolved.iter().filter_map(|s| s.addr))
        {
            if !dests.contains(&a) {
                dests.push(a);
            }
        }
        for a in dests {
            let r = match self.socket_for(&a) {
                Ok(sock) => sock.send_to(json.as_bytes(), a).map(|_| ()),
                Err(e) => Err(e),
            };
            if let Err(e) = r {
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

    /// The socket for `dest`'s address family; the IPv6 one is bound on first use.
    fn socket_for(&mut self, dest: &SocketAddr) -> std::io::Result<&UdpSocket> {
        if dest.is_ipv4() {
            return self.udp.as_ref().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotConnected, "no IPv4 socket")
            });
        }
        if self.udp6.is_none() {
            let s = UdpSocket::bind("[::]:0")?;
            s.set_nonblocking(true)?;
            tracing::debug!(local = ?s.local_addr().ok(), "bound IPv6 UDP socket");
            self.udp6 = Some(s);
        }
        Ok(self.udp6.as_ref().expect("just bound"))
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
        if let Some(h) = &self.udp_hosts {
            for s in h.snapshot() {
                match s.addr {
                    Some(a) => parts.push(format!("UDP {} via {a}", s.label())),
                    None => parts.push(format!("UDP {} (resolving)", s.label())),
                }
            }
        }
        for w in &self.tcp {
            let state = if w.is_connected() {
                "connected"
            } else {
                "reconnecting"
            };
            parts.push(format!("TCP {} ({state})", w.label()));
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
