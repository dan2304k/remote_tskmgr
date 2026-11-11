//! Transport tests on loopback only (ephemeral ports, fake resolver, nothing leaves the host),
//! except `broadcast_target_send_ok_and_describe`, which is `#[ignore]`d because a broadcast
//! target always also sends to 255.255.255.255 on the LAN.

use rtm_net::{BroadcastInterfaces, NetError, Resolver, Sender, SenderOptions, Target};
use rtm_protocol::{Metric, Packet};
use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn packet(seq: u64) -> Packet {
    let mut p = Packet {
        v: 1,
        id: "test-id".into(),
        host: "testhost".into(),
        os: "linux".into(),
        ts: 1_762_640_000_000 + seq,
        seq,
        interval_ms: 1000,
        metrics: serde_json::Map::new(),
        auth: None,
        extra: Default::default(),
    };
    p.set_metric("cpu", Metric::percent("CPU", Some(12.5)));
    p.set_metric("disk", Metric::rate_mbps("DISK", None));
    p
}

fn read_line(r: &mut BufReader<TcpStream>) -> Packet {
    let mut s = String::new();
    r.read_line(&mut s).expect("read line");
    assert!(s.ends_with('\n'), "line not newline-terminated: {s:?}");
    Packet::from_json(s.trim_end()).expect("valid packet")
}

#[test]
fn new_with_no_targets_is_error() {
    assert!(matches!(Sender::new(vec![]), Err(NetError::NoTargets)));
}

#[test]
fn udp_unicast_roundtrip() {
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let addr = rx.local_addr().unwrap();
    let mut s = Sender::new(vec![Target::UdpUnicast(addr)]).unwrap();
    let p = packet(7);
    let n = s.send(&p).unwrap();
    let mut buf = [0u8; 2048];
    let (got, _) = rx.recv_from(&mut buf).unwrap();
    assert_eq!(got, n);
    let back = Packet::from_json(std::str::from_utf8(&buf[..got]).unwrap()).unwrap();
    assert_eq!(back, p);
    assert_eq!(s.describe(), format!("UDP {addr}"));
}

#[test]
fn tcp_unicast_three_lines() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let mut s = Sender::new(vec![Target::TcpUnicast(addr)]).unwrap();
    let (conn, _) = l.accept().unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut r = BufReader::new(conn);
    wait_until(
        || s.describe().ends_with("(connected)"),
        Duration::from_secs(2),
    );
    for seq in 1..=3 {
        s.send(&packet(seq)).unwrap();
        // Pace slightly so the 4-deep queue never has to drop anything.
        std::thread::sleep(Duration::from_millis(10));
    }
    for seq in 1..=3 {
        assert_eq!(read_line(&mut r).seq, seq);
    }
}

#[test]
fn tcp_reconnects_after_listener_restart() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = l.local_addr().unwrap();
    let mut s = Sender::new(vec![Target::TcpUnicast(addr)]).unwrap();
    let (conn, _) = l.accept().unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut r = BufReader::new(conn);
    s.send(&packet(1)).unwrap();
    assert_eq!(read_line(&mut r).seq, 1);

    // Viewer goes away.
    drop(r);
    drop(l);
    let t = Instant::now();
    assert!(s.send(&packet(2)).is_ok());
    assert!(t.elapsed() < Duration::from_millis(50));
    wait_until(
        || s.describe().ends_with("(reconnecting)"),
        Duration::from_secs(2),
    );

    // While down, send must not block.
    for seq in 3..10 {
        let t = Instant::now();
        assert!(s.send(&packet(seq)).is_ok());
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "send blocked {:?}",
            t.elapsed()
        );
    }

    // Viewer comes back on the same port (std sets SO_REUSEADDR on Unix listeners).
    let l = TcpListener::bind(addr).expect("rebind same port");
    l.set_nonblocking(true).unwrap();
    let start = Instant::now();
    let mut seq = 100;
    let conn = loop {
        s.send(&packet(seq)).unwrap();
        seq += 1;
        match l.accept() {
            Ok((c, _)) => break c,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("accept: {e}"),
        }
        assert!(start.elapsed() < Duration::from_secs(3), "no reconnect");
        std::thread::sleep(Duration::from_millis(50));
    };
    conn.set_nonblocking(false).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut r = BufReader::new(conn);
    s.send(&packet(500)).unwrap();
    // A recent pending packet may be flushed on connect first (on Windows the hang-up is
    // only noticed when a write fails, so the failed packet, seq 3..9, is the one resent);
    // read until ours arrives and only require that nothing newer than ours shows up.
    loop {
        let p = read_line(&mut r);
        if p.seq == 500 {
            break;
        }
        assert!(p.seq < 500, "unexpected packet seq {}", p.seq);
    }
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "reconnect took {:?}",
        start.elapsed()
    );
    assert!(s.describe().ends_with("(connected)"));
}

#[test]
fn send_does_not_block_when_tcp_target_down() {
    // Grab a free port, then close it so connects are refused.
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    // Also a non-routable address (TEST-NET-1) whose connect would hang until timeout.
    let blackhole: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let mut s = Sender::new(vec![
        Target::TcpUnicast(addr),
        Target::TcpUnicast(blackhole),
    ])
    .unwrap();
    let mut worst = Duration::ZERO;
    for seq in 0..20 {
        let t = Instant::now();
        s.send(&packet(seq)).unwrap();
        worst = worst.max(t.elapsed());
    }
    assert!(worst < Duration::from_millis(50), "send took {worst:?}");
    assert!(s.describe().contains("(reconnecting)"));
    let t = Instant::now();
    drop(s);
    // Drop must not hang on the thread stuck in a 2 s connect.
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "drop took {:?}",
        t.elapsed()
    );
}

/// Sends a real limited broadcast (255.255.255.255) on the LAN, so it is not run by default:
/// `cargo test -p rtm-net -- --ignored` on a machine where that is fine.
#[test]
#[ignore = "sends a real LAN broadcast"]
fn broadcast_target_send_ok_and_describe() {
    let mut s = Sender::new(vec![Target::UdpBroadcast {
        port: free_udp_port(),
        interfaces: BroadcastInterfaces::All,
        skip_prefixes: vec![
            "docker".into(),
            "br-".into(),
            "veth".into(),
            "virbr".into(),
            "lo".into(),
        ],
    }])
    .unwrap();
    // Errors (e.g. no network on CI) are logged, never returned.
    assert!(s.send(&packet(1)).is_ok());
    let d = s.describe();
    eprintln!("describe: {d}");
    assert!(d.starts_with("UDP bcast :"), "{d}");
    assert!(d.ends_with("255.255.255.255"), "{d}");
    assert!(!d.contains("docker"), "{d}");
}

#[test]
fn mixed_targets_describe() {
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ua = rx.local_addr().unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let ta = l.local_addr().unwrap();
    let s = Sender::new(vec![
        Target::UdpBroadcast {
            port: 48000,
            interfaces: BroadcastInterfaces::Named(vec!["nonexistent0".into()]),
            skip_prefixes: vec![],
        },
        Target::UdpUnicast(ua),
        Target::TcpUnicast(ta),
    ])
    .unwrap();
    let _c = l.accept().unwrap();
    wait_until(
        || s.describe().ends_with("(connected)"),
        Duration::from_secs(2),
    );
    assert_eq!(
        s.describe(),
        format!("UDP bcast :48000 via 255.255.255.255; UDP {ua}; TCP {ta} (connected)")
    );
}

#[test]
fn udp_ipv6_unicast_roundtrip() {
    let Ok(rx) = UdpSocket::bind("[::1]:0") else {
        eprintln!("no IPv6 loopback on this host; skipped");
        return;
    };
    rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let addr = rx.local_addr().unwrap();
    // Together with an IPv4 target: both sockets are used.
    let rx4 = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx4.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut s = Sender::new(vec![
        Target::UdpUnicast(addr),
        Target::UdpUnicast(rx4.local_addr().unwrap()),
    ])
    .unwrap();
    let p = packet(3);
    let n = s.send(&p).unwrap();
    let mut buf = [0u8; 2048];
    assert_eq!(rx.recv_from(&mut buf).unwrap().0, n);
    assert_eq!(
        Packet::from_json(std::str::from_utf8(&buf[..n]).unwrap()).unwrap(),
        p
    );
    assert_eq!(rx4.recv_from(&mut buf).unwrap().0, n);
}

/// A resolver whose answer the test can change, like a viewer getting a new DHCP lease.
fn fake_dns(first: SocketAddr) -> (Resolver, Arc<Mutex<SocketAddr>>) {
    let cur = Arc::new(Mutex::new(first));
    let c = cur.clone();
    let r: Resolver = Arc::new(move |host, port| {
        assert_eq!(host, "viewer.test");
        let a = *c.lock().unwrap();
        // The port comes from the answer here so each "lease" can be a fresh ephemeral port.
        assert_eq!(port, 1);
        Ok(vec![a])
    });
    (r, cur)
}

fn opts(resolver: Resolver, every: Duration) -> SenderOptions {
    SenderOptions {
        resolver,
        udp_resolve_interval: every,
    }
}

fn host(tcp: bool) -> Target {
    let (host, port) = ("viewer.test".to_string(), 1);
    if tcp {
        Target::TcpHost { host, port }
    } else {
        Target::UdpHost { host, port }
    }
}

#[test]
fn udp_host_target_follows_address_changes() {
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").unwrap();
    for r in [&a, &b] {
        r.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
    }
    let (dns, cur) = fake_dns(a.local_addr().unwrap());
    let mut s =
        Sender::with_options(vec![host(false)], opts(dns, Duration::from_millis(50))).unwrap();
    // Sends until `rx` gets a packet (the background lookup may not have happened yet).
    let mut buf = [0u8; 2048];
    let mut until_received = |s: &mut Sender, rx: &UdpSocket| {
        let t = Instant::now();
        loop {
            s.send(&packet(1)).unwrap();
            if rx.recv_from(&mut buf).is_ok() {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "nothing received");
        }
    };
    until_received(&mut s, &a);
    assert!(s.describe().starts_with("UDP viewer.test:1 via 127.0.0.1:"));
    *cur.lock().unwrap() = b.local_addr().unwrap();
    until_received(&mut s, &b);
    assert_eq!(
        s.describe(),
        format!("UDP viewer.test:1 via {}", b.local_addr().unwrap())
    );
}

#[test]
fn tcp_host_target_resolves_before_each_connect() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let (dns, cur) = fake_dns(l.local_addr().unwrap());
    let mut s = Sender::with_options(vec![host(true)], opts(dns, Duration::from_secs(60))).unwrap();
    let (conn, _) = l.accept().unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(conn);
    s.send(&packet(1)).unwrap();
    assert_eq!(read_line(&mut r).seq, 1);
    assert!(s.describe().starts_with("TCP viewer.test:1 via 127.0.0.1:"));

    // The viewer moves to another address.
    let l2 = TcpListener::bind("127.0.0.1:0").unwrap();
    l2.set_nonblocking(true).unwrap();
    *cur.lock().unwrap() = l2.local_addr().unwrap();
    drop(r);
    drop(l);
    let t = Instant::now();
    let conn = loop {
        s.send(&packet(2)).unwrap();
        match l2.accept() {
            Ok((c, _)) => break c,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("accept: {e}"),
        }
        assert!(t.elapsed() < Duration::from_secs(15), "never followed");
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(conn);
}

#[test]
fn send_does_not_wait_for_slow_dns() {
    let r: Resolver = Arc::new(|_, _| {
        std::thread::sleep(Duration::from_secs(2));
        Err(std::io::Error::other("timed out"))
    });
    let t = Instant::now();
    let mut s = Sender::with_options(
        vec![host(false), host(true)],
        opts(r, Duration::from_secs(60)),
    )
    .unwrap();
    for seq in 0..5 {
        s.send(&packet(seq)).unwrap();
    }
    assert!(
        t.elapsed() < Duration::from_millis(1000),
        "new + send took {:?}",
        t.elapsed()
    );
    let d = s.describe();
    assert!(
        d.contains("UDP viewer.test:1 (resolving)") && d.contains("TCP viewer.test:1"),
        "{d}"
    );
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_until(mut f: impl FnMut() -> bool, max: Duration) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < max, "condition not met within {max:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
