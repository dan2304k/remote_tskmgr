//! Interface enumeration and broadcast-address selection.
//!
//! The selection logic is a pure function over [`IfaceInfo`] so it can be tested against a
//! fake interface list without touching the host's network configuration.

use crate::BroadcastInterfaces;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// The subset of an interface we care about for broadcasting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfaceInfo {
    pub name: String,
    pub ip: Ipv4Addr,
    pub broadcast: Option<Ipv4Addr>,
    pub loopback: bool,
    /// False when the OS reports the link as down / not present.
    pub up: bool,
}

/// One chosen directed-broadcast destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastDest {
    pub iface: String,
    pub addr: Ipv4Addr,
}

/// Enumerate the host's IPv4 interfaces. Errors yield an empty list (logged by the caller).
pub fn enumerate() -> std::io::Result<Vec<IfaceInfo>> {
    let list = if_addrs::get_if_addrs()?;
    Ok(list
        .into_iter()
        .filter_map(|i| {
            let up = !matches!(
                i.oper_status,
                if_addrs::IfOperStatus::Down
                    | if_addrs::IfOperStatus::NotPresent
                    | if_addrs::IfOperStatus::LowerLayerDown
            );
            let loopback = i.is_loopback();
            match i.addr {
                if_addrs::IfAddr::V4(v4) => Some(IfaceInfo {
                    name: i.name,
                    ip: v4.ip,
                    broadcast: v4.broadcast,
                    loopback,
                    up,
                }),
                if_addrs::IfAddr::V6(_) => None,
            }
        })
        .collect())
}

/// Source address the OS would use to reach the internet (UDP connect trick, nothing is sent).
pub fn default_route_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(SocketAddr::from(([8, 8, 8, 8], 53))).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Does `name` start with one of `prefixes`? Plain case-sensitive prefix match; empty
/// prefixes are ignored. `br-` matches `br-d902...`, `veth` matches `vetha1b2c3d`, `docker`
/// matches `docker0`. Case-sensitivity keeps Windows names such as "Local Area Connection"
/// or "vEthernet (External)" (which can be the main NIC) clear of the default list.
pub fn skipped_by_prefix(name: &str, prefixes: &[String]) -> bool {
    prefixes
        .iter()
        .map(|p| p.trim())
        .any(|p| !p.is_empty() && name.starts_with(p))
}

/// Pick directed-broadcast destinations.
///
/// * Always skipped: loopback, links reported down, interfaces without a broadcast address.
/// * `All`: additionally skips names matched by `skip_prefixes` (see [`skipped_by_prefix`]).
/// * `Default`: the interface owning `default_ip`; falls back to `All` when there is none.
/// * `Named`: exactly the listed names (the skip list does not apply to explicit picks).
///
/// The result is deduplicated by address (first interface name wins) and keeps input order.
/// 255.255.255.255 is not included; the caller adds it.
pub fn select_broadcast(
    ifaces: &[IfaceInfo],
    sel: &BroadcastInterfaces,
    skip_prefixes: &[String],
    default_ip: Option<Ipv4Addr>,
) -> Vec<BroadcastDest> {
    let usable = |i: &&IfaceInfo| i.up && !i.loopback && i.broadcast.is_some();
    let skipped = |i: &IfaceInfo| skipped_by_prefix(&i.name, skip_prefixes);
    let picked: Vec<&IfaceInfo> = match sel {
        BroadcastInterfaces::All => ifaces
            .iter()
            .filter(usable)
            .filter(|i| !skipped(i))
            .collect(),
        BroadcastInterfaces::Default => {
            let d: Vec<&IfaceInfo> = match default_ip {
                Some(ip) => ifaces
                    .iter()
                    .filter(usable)
                    .filter(|i| i.ip == ip)
                    .collect(),
                None => Vec::new(),
            };
            if d.is_empty() {
                ifaces
                    .iter()
                    .filter(usable)
                    .filter(|i| !skipped(i))
                    .collect()
            } else {
                d
            }
        }
        BroadcastInterfaces::Named(names) => ifaces
            .iter()
            .filter(usable)
            .filter(|i| names.iter().any(|n| n == &i.name))
            .collect(),
    };
    let mut out: Vec<BroadcastDest> = Vec::new();
    for i in picked {
        let addr = i.broadcast.expect("filtered on broadcast.is_some()");
        if addr == Ipv4Addr::BROADCAST || out.iter().any(|d| d.addr == addr) {
            continue;
        }
        out.push(BroadcastDest {
            iface: i.name.clone(),
            addr,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ifc(name: &str, ip: [u8; 4], bc: Option<[u8; 4]>) -> IfaceInfo {
        IfaceInfo {
            name: name.into(),
            ip: ip.into(),
            broadcast: bc.map(Into::into),
            loopback: ip[0] == 127,
            up: true,
        }
    }

    fn fake() -> Vec<IfaceInfo> {
        vec![
            ifc("lo", [127, 0, 0, 1], Some([127, 255, 255, 255])),
            ifc("enp0s31f6", [192, 168, 1, 65], Some([192, 168, 1, 255])),
            ifc("docker0", [172, 17, 0, 1], Some([172, 17, 255, 255])),
            ifc("br-1234abcd", [172, 18, 0, 1], Some([172, 18, 255, 255])),
            ifc("wlan0", [10, 0, 0, 5], Some([10, 0, 0, 255])),
            ifc("tun0", [10, 8, 0, 2], None),
            // second address on the LAN NIC, same broadcast: deduped
            ifc("enp0s31f6", [192, 168, 1, 66], Some([192, 168, 1, 255])),
        ]
    }

    fn skip() -> Vec<String> {
        ["docker", "br-", "veth", "virbr", "lo"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn addrs(v: &[BroadcastDest]) -> Vec<String> {
        v.iter()
            .map(|d| format!("{}={}", d.iface, d.addr))
            .collect()
    }

    #[test]
    fn all_skips_prefixes_loopback_and_no_broadcast() {
        let r = select_broadcast(&fake(), &BroadcastInterfaces::All, &skip(), None);
        assert_eq!(
            addrs(&r),
            vec!["enp0s31f6=192.168.1.255", "wlan0=10.0.0.255"]
        );
    }

    #[test]
    fn loopback_skipped_even_with_empty_skip_list() {
        let r = select_broadcast(&fake(), &BroadcastInterfaces::All, &[], None);
        assert!(r.iter().all(|d| d.iface != "lo"));
        assert!(r.iter().any(|d| d.iface == "docker0"));
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn down_interfaces_skipped() {
        let mut l = fake();
        l[4].up = false;
        let r = select_broadcast(&l, &BroadcastInterfaces::All, &skip(), None);
        assert_eq!(addrs(&r), vec!["enp0s31f6=192.168.1.255"]);
    }

    #[test]
    fn default_picks_route_owner_or_falls_back() {
        let r = select_broadcast(
            &fake(),
            &BroadcastInterfaces::Default,
            &skip(),
            Some([10, 0, 0, 5].into()),
        );
        assert_eq!(addrs(&r), vec!["wlan0=10.0.0.255"]);
        let r = select_broadcast(&fake(), &BroadcastInterfaces::Default, &skip(), None);
        assert_eq!(r.len(), 2);
        // default ip not matching any interface also falls back
        let r = select_broadcast(
            &fake(),
            &BroadcastInterfaces::Default,
            &skip(),
            Some([1, 2, 3, 4].into()),
        );
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn skip_prefix_matching() {
        for n in [
            "lo",
            "lo0",
            "docker0",
            "br-1ee4",
            "br-d902716df028",
            "veth12ab",
            "vetha1b2c3d",
            "virbr0",
        ] {
            assert!(skipped_by_prefix(n, &skip()), "{n}");
        }
        for n in [
            "Local Area Connection",
            "vEthernet (External)",
            "enp0s31f6",
            "Ethernet",
            "Docker1",
        ] {
            assert!(!skipped_by_prefix(n, &skip()), "{n}");
        }
        assert!(!skipped_by_prefix("eth0", &["".into(), " ".into()]));
        // Windows-style list: the main NIC behind a Hyper-V switch keeps broadcasting.
        let l = vec![
            ifc(
                "vEthernet (External)",
                [192, 168, 1, 9],
                Some([192, 168, 1, 255]),
            ),
            ifc(
                "Local Area Connection",
                [10, 1, 0, 2],
                Some([10, 1, 0, 255]),
            ),
        ];
        let r = select_broadcast(&l, &BroadcastInterfaces::All, &skip(), None);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn named_selects_exact_names_ignoring_skip_list() {
        let sel = BroadcastInterfaces::Named(vec!["docker0".into(), "nope".into(), "lo".into()]);
        let r = select_broadcast(&fake(), &sel, &skip(), None);
        assert_eq!(addrs(&r), vec!["docker0=172.17.255.255"]);
    }
}
