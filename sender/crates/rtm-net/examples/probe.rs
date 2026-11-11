//! Manual check: broadcast N synthetic packets and print `describe()`.
//!
//!     cargo run -p rtm-net --example probe -- [port] [count] [tcp_host:port]
//!
//! Pair with `python3 tools/udp-listen.py --port <port> [--tcp]` on another machine.

use rtm_net::{BroadcastInterfaces, Sender, Target};
use rtm_protocol::{Metric, Packet};
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(48000);
    let count: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut targets = vec![Target::UdpBroadcast {
        port,
        interfaces: BroadcastInterfaces::All,
        skip_prefixes: ["docker", "br-", "veth", "virbr", "lo"]
            .map(String::from)
            .to_vec(),
    }];
    if let Some(t) = args.next() {
        targets.push(Target::TcpUnicast(t.parse().expect("tcp target host:port")));
    }
    let mut s = Sender::new(targets).expect("sender");
    for seq in 1..=count {
        let mut p = Packet {
            v: 1,
            id: "rtm-net-probe".into(),
            host: "rtm-net-probe".into(),
            os: std::env::consts::OS.into(),
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            seq,
            interval_ms: 200,
            metrics: Default::default(),
            auth: None,
            extra: Default::default(),
        };
        p.set_metric("cpu", Metric::percent("CPU", Some(seq as f64 % 100.0)));
        let n = s.send(&p).expect("send");
        println!("seq {seq}: {n} bytes -> {}", s.describe());
        std::thread::sleep(Duration::from_millis(if count > 50 { 2 } else { 200 }));
    }
}
