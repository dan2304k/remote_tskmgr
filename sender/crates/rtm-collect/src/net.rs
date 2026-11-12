//! `net`: received + transmitted MB/s over physical-ish interfaces.
//!
//! Loopback and the configured prefixes (default `docker*`, `br-*`, `veth*`, `virbr*`, `lo*`,
//! matched like `rtm_net`'s broadcast selection, see `skip_interface`) are skipped, otherwise
//! container traffic would be counted twice. Counters are differenced against our own
//! baseline: a newly appearing interface contributes 0 on its first tick.

use crate::util::{detail, mbps, num, round, Hold, Stopwatch, MIN_WINDOW};
use crate::Collector;
use rtm_protocol::Metric;
use std::collections::HashMap;
use std::time::Duration;
use sysinfo::Networks;

/// Loopback (always; sysinfo has no loopback flag, so by name) or a name matched by one of
/// `prefixes`, with the same rule as `rtm_net::iface::skipped_by_prefix`: a plain
/// case-sensitive prefix match (`br-` matches `br-d902...`, `lo` does not match
/// "Local Area Connection").
pub(crate) fn skip_interface(name: &str, prefixes: &[String]) -> bool {
    name == "lo"
        || name.to_ascii_lowercase().contains("loopback")
        || prefixes
            .iter()
            .map(|p| p.trim())
            .any(|p| !p.is_empty() && name.starts_with(p))
}

pub struct NetCollector {
    nets: Networks,
    skip: Vec<String>,
    prev: HashMap<String, (u64, u64)>,
    clock: Stopwatch,
    /// Last reading, repeated for ticks closer than `MIN_WINDOW` to it.
    hold: Hold<Metric>,
}

impl NetCollector {
    pub fn new(skip_prefixes: &[String]) -> Option<Self> {
        let nets = Networks::new_with_refreshed_list().ok()?;
        let mut me = Self {
            nets,
            skip: skip_prefixes.to_vec(),
            prev: HashMap::new(),
            clock: Stopwatch::new(),
            hold: Hold::new(),
        };
        me.prev = me.totals();
        Some(me)
    }

    fn totals(&self) -> HashMap<String, (u64, u64)> {
        self.nets
            .iter()
            .filter_map(|(name, data)| {
                let name = name.to_string_lossy();
                (!skip_interface(&name, &self.skip)).then(|| {
                    (
                        name.into_owned(),
                        (data.total_received(), data.total_transmitted()),
                    )
                })
            })
            .collect()
    }
}

impl Collector for NetCollector {
    fn id(&self) -> &str {
        "net"
    }

    fn sample(&mut self, dt: Duration) -> Option<Metric> {
        // Below MIN_WINDOW since the last reading, repeat it: the counters keep accumulating
        // and the next real reading covers the whole interval (no spikes at 10/s).
        let now = std::time::Instant::now();
        if let Some(m) = self.hold.recent(now, MIN_WINDOW) {
            return Some(m);
        }
        self.nets.refresh(true);
        let secs = self.clock.lap(dt);
        let totals = self.totals();
        let (mut rx, mut tx) = (0u64, 0u64);
        for (k, (r, t)) in &totals {
            if let Some((pr, pt)) = self.prev.get(k) {
                rx += r.saturating_sub(*pr);
                tx += t.saturating_sub(*pt);
            }
        }
        self.prev = totals;
        let rx = round(mbps(rx, secs), 3);
        let tx = round(mbps(tx, secs), 3);
        let mut d = detail();
        d.insert("rx_mbps".into(), num(rx));
        d.insert("tx_mbps".into(), num(tx));
        let m = Metric::rate_mbps("NET", Some(round(rx + tx, 3))).with_detail(d);
        self.hold.store(now, m.clone());
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_filter() {
        let skip: Vec<String> = crate::DEFAULT_NET_SKIP_PREFIXES
            .iter()
            .map(|s| s.to_string())
            .collect();
        for n in [
            "lo",
            "docker0",
            "br-1a2b3c",
            "veth12ab",
            "virbr0",
            "Loopback Pseudo-Interface 1",
        ] {
            assert!(skip_interface(n, &skip), "{n}");
        }
        for n in ["enp0s31f6", "wlan0", "eth0", "Ethernet"] {
            assert!(!skip_interface(n, &skip), "{n}");
        }
        for n in ["lo0", "br-1ee4", "br-d902716df028", "vetha1b2c3d"] {
            assert!(skip_interface(n, &skip), "{n}");
        }
        for n in ["Local Area Connection", "vEthernet (External)", "Docker1"] {
            assert!(!skip_interface(n, &skip), "{n}");
        }
        // Configured prefixes are case-sensitive; loopback is skipped even with an empty list.
        let custom = vec!["wlan".to_string(), "".to_string()];
        assert!(skip_interface("wlan0", &custom));
        assert!(!skip_interface("WLAN0", &custom));
        assert!(!skip_interface("docker0", &custom));
        assert!(skip_interface("lo", &[]));
        assert!(skip_interface("Loopback Pseudo-Interface 1", &[]));
    }
}
