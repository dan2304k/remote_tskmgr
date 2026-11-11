//! Host name resolution for unicast targets given by name (`viewer.lan:48000`).
//!
//! Names are never resolved on the caller's thread: TCP workers resolve before every connect
//! attempt on their own thread, and UDP host targets are refreshed by a background thread
//! (every [`UDP_RESOLVE_INTERVAL`] by default), so a viewer whose DHCP address changes is
//! followed and [`crate::Sender::send`] never waits for DNS.

use crossbeam_channel::{bounded, RecvTimeoutError, Sender as ChanTx};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How often UDP host targets are re-resolved.
pub const UDP_RESOLVE_INTERVAL: Duration = Duration::from_secs(60);

/// `(host, port)` -> addresses. Blocking; only ever called on a background thread.
pub type Resolver = Arc<dyn Fn(&str, u16) -> std::io::Result<Vec<SocketAddr>> + Send + Sync>;

/// The OS resolver (`getaddrinfo`).
pub fn system_resolver() -> Resolver {
    Arc::new(|host, port| Ok((host, port).to_socket_addrs()?.collect()))
}

/// IPv4 addresses first (most viewers listen on IPv4 only), then IPv6; duplicates removed,
/// order otherwise kept.
pub fn order_addrs(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::with_capacity(addrs.len());
    for a in addrs
        .iter()
        .filter(|a| a.is_ipv4())
        .chain(addrs.iter().filter(|a| a.is_ipv6()))
    {
        if !out.contains(a) {
            out.push(*a);
        }
    }
    out
}

/// Resolve now with the system resolver and return the preferred address (first IPv4, else
/// the first IPv6). Blocking: not for the sample thread.
pub fn resolve_host(host: &str, port: u16) -> Result<SocketAddr, String> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve '{host}': {e}"))?
        .collect();
    order_addrs(addrs)
        .first()
        .copied()
        .ok_or_else(|| format!("'{host}' has no addresses"))
}

/// One UDP host target and its last known address.
#[derive(Debug, Clone)]
pub(crate) struct HostSlot {
    pub host: String,
    pub port: u16,
    /// Last successful resolution; kept when a later lookup fails.
    pub addr: Option<SocketAddr>,
}

impl HostSlot {
    pub fn label(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// UDP host targets, re-resolved by a background thread. Dropping it stops the thread
/// (without waiting for a lookup in progress).
pub(crate) struct UdpHosts {
    slots: Arc<Mutex<Vec<HostSlot>>>,
    _stop: ChanTx<()>,
}

fn lock(m: &Mutex<Vec<HostSlot>>) -> MutexGuard<'_, Vec<HostSlot>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl UdpHosts {
    pub fn spawn(
        targets: Vec<(String, u16)>,
        resolver: Resolver,
        every: Duration,
    ) -> std::io::Result<Self> {
        let slots: Vec<HostSlot> = targets
            .into_iter()
            .map(|(host, port)| HostSlot {
                host,
                port,
                addr: None,
            })
            .collect();
        let slots = Arc::new(Mutex::new(slots));
        let (stop, stopped) = bounded::<()>(0);
        let shared = slots.clone();
        std::thread::Builder::new()
            .name("rtm-udp-resolve".into())
            .spawn(move || {
                let names: Vec<(String, u16)> = lock(&shared)
                    .iter()
                    .map(|s| (s.host.clone(), s.port))
                    .collect();
                let mut failing = vec![false; names.len()];
                loop {
                    for (i, (host, port)) in names.iter().enumerate() {
                        let r = resolver(host, *port).map(order_addrs);
                        let mut slots = lock(&shared);
                        let slot = &mut slots[i];
                        match r.as_deref() {
                            Ok([first, ..]) => {
                                if slot.addr != Some(*first) {
                                    tracing::info!(target = %slot.label(), addr = %first, "udp target resolved");
                                    slot.addr = Some(*first);
                                }
                                failing[i] = false;
                            }
                            other => {
                                let why = match other {
                                    Err(e) => e.to_string(),
                                    _ => "no addresses".into(),
                                };
                                if !failing[i] {
                                    tracing::warn!(target = %slot.label(), error = %why, kept = ?slot.addr, "udp target does not resolve; retrying");
                                }
                                failing[i] = true;
                            }
                        }
                    }
                    match stopped.recv_timeout(every) {
                        Err(RecvTimeoutError::Timeout) => continue,
                        _ => break,
                    }
                }
            })?;
        Ok(Self { slots, _stop: stop })
    }

    /// A snapshot of the targets and their current addresses.
    pub fn snapshot(&self) -> Vec<HostSlot> {
        lock(&self.slots).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_first_and_deduped() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        let v = vec![
            a("[::1]:1"),
            a("10.0.0.1:1"),
            a("[fe80::1]:1"),
            a("10.0.0.1:1"),
            a("10.0.0.2:1"),
        ];
        assert_eq!(
            order_addrs(v),
            [
                a("10.0.0.1:1"),
                a("10.0.0.2:1"),
                a("[::1]:1"),
                a("[fe80::1]:1")
            ]
        );
    }

    #[test]
    fn resolve_host_literal_and_failure() {
        assert_eq!(
            resolve_host("127.0.0.1", 9).unwrap(),
            "127.0.0.1:9".parse().unwrap()
        );
        assert!(resolve_host("no-such-host.invalid", 9).is_err());
    }
}
