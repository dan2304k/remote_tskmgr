use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

fn tmp_path(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rtm-config-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("sub").join(name)
}

#[test]
fn defaults_round_trip_through_toml() {
    let c = Config::default();
    let s = c.to_toml_string().unwrap();
    assert_eq!(Config::from_toml(&s).unwrap(), c);
    c.validate().unwrap();
}

#[test]
fn template_parses_to_defaults() {
    let mut want = Config::default();
    want.general.id = "abc".into();
    assert_eq!(Config::from_toml(&default_template("abc")).unwrap(), want);
}

#[test]
fn default_path_ends_with_rtm_sender_toml() {
    let p = Config::default_path();
    assert!(
        p.ends_with("rtm/sender.toml") || p.ends_with("sender.toml"),
        "{p:?}"
    );
}

#[test]
fn load_or_create_writes_file_with_uuid() {
    let p = tmp_path("sender.toml");
    let c = Config::load_or_create(&p).unwrap();
    assert!(p.exists());
    uuid::Uuid::parse_str(&c.general.id).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(
        text.contains("# RTM sender configuration"),
        "comments written"
    );
    // Stable across loads.
    let again = Config::load_or_create(&p).unwrap();
    assert_eq!(again.general.id, c.general.id);
    assert_eq!(again, c);
}

#[test]
fn auto_id_persisted_and_comments_kept() {
    let p = tmp_path("sender.toml");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        "# my notes\n[general]\nid = \"auto\"\ninterval_ms = 5000 # five\n[metrics]\nid = \"auto\"\n",
    )
    .unwrap();
    let c = Config::load_or_create(&p).unwrap();
    uuid::Uuid::parse_str(&c.general.id).unwrap();
    assert_eq!(c.general.interval_ms, 5000);
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("# my notes") && text.contains("# five"));
    assert!(text.contains(&c.general.id));
    assert!(
        text.contains("[metrics]\nid = \"auto\""),
        "only [general].id replaced"
    );
    assert_eq!(Config::load(&p).unwrap().general.id, c.general.id);
}

#[test]
fn missing_id_without_general_table_appends_one() {
    let p = tmp_path("sender.toml");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "# keep me\n[network]\nudp_port = 49000 # mine\n").unwrap();
    let c = Config::load_or_create(&p).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(
        text.starts_with("# keep me\n[network]\nudp_port = 49000 # mine\n"),
        "{text}"
    );
    assert!(text.contains("\n[general]\nid = \""), "{text}");
    let back = Config::load(&p).unwrap();
    assert_eq!(back.general.id, c.general.id);
    assert_eq!(back.network.udp_port, 49000);
}

#[test]
fn missing_id_inserted_under_general_keeping_comments() {
    let p = tmp_path("sender.toml");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let orig = "# top\r\n[general] # hdr\r\ninterval_ms = 5000 # five\r\n\r\n[gui]\r\nautostart = true\r\n";
    std::fs::write(&p, orig).unwrap();
    let c = Config::load_or_create(&p).unwrap();
    uuid::Uuid::parse_str(&c.general.id).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    let expected_prefix = format!("# top\r\n[general] # hdr\r\nid = \"{}\"", c.general.id);
    assert!(text.starts_with(&expected_prefix), "{text}");
    assert!(text.ends_with("interval_ms = 5000 # five\r\n\r\n[gui]\r\nautostart = true\r\n"));
    let back = Config::load(&p).unwrap();
    assert_eq!(back.general.id, c.general.id);
    assert_eq!(back.general.interval_ms, 5000);
    assert!(back.gui.autostart);
    // Second load: id present now, file untouched.
    Config::load_or_create(&p).unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
}

#[test]
fn dotted_general_id_falls_back_to_full_rewrite() {
    let p = tmp_path("sender.toml");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "general.interval_ms = 2000\n").unwrap();
    let c = Config::load_or_create(&p).unwrap();
    let back = Config::load(&p).unwrap();
    assert_eq!(back.general.id, c.general.id);
    assert_eq!(back.general.interval_ms, 2000);
}

#[test]
fn write_atomic_leaves_no_temp_files_and_overwrites() {
    let p = tmp_path("w.toml");
    write_atomic(&p, "a = 1\n").unwrap();
    write_atomic(&p, "a = 2\n").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "a = 2\n");
    let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("w.toml")]);
}

#[test]
fn write_atomic_concurrent_writers_do_not_collide() {
    let p = tmp_path("c.toml");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let p = p.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    write_atomic(&p, &format!("v = {i}\n")).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.starts_with("v = ") && text.ends_with('\n'), "{text}");
    assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn save_then_load() {
    let p = tmp_path("x.toml");
    let mut c = Config::default();
    c.general.id = "fixed".into();
    c.network.tcp_unicast = vec!["10.0.0.2:48000".into()];
    c.save(&p).unwrap();
    assert_eq!(Config::load(&p).unwrap(), c);
}

fn invalid(f: impl FnOnce(&mut Config)) -> String {
    let mut c = Config::default();
    f(&mut c);
    match c.validate() {
        Err(ConfigError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn validation_errors() {
    assert!(invalid(|c| c.general.interval_ms = 99).contains("interval_ms"));
    assert!(invalid(|c| c.network.udp_port = 0).contains("udp_port"));
    assert!(invalid(|c| c.network.udp_unicast = vec!["no pe".into()]).contains("udp_unicast"));
    assert!(invalid(|c| c.network.tcp_unicast = vec!["1.2.3.4:x".into()]).contains("tcp_unicast"));
    assert!(invalid(|c| c.metrics.gpu_backend = "cuda".into()).contains("gpu_backend"));
    assert!(invalid(|c| c.metrics.enabled = vec!["cpu".into(), " ".into()]).contains("empty"));
    assert!(
        invalid(|c| c.network.broadcast_interfaces = BroadcastInterfaces::Mode("some".into()))
            .contains("broadcast_interfaces")
    );
    // Multiple errors are all reported.
    let m = invalid(|c| {
        c.general.interval_ms = 1;
        c.network.udp_port = 0;
    });
    assert!(m.contains("interval_ms") && m.contains("udp_port"));
    // Boundary: 10 packets/s is the fastest rate.
    assert!(invalid(|c| c.general.interval_ms = 50).contains("interval_ms"));
    // Upper bound: one hour.
    assert!(invalid(|c| c.general.interval_ms = MAX_INTERVAL_MS + 1).contains("interval_ms"));
    assert!(invalid(|c| c.general.interval_ms = u64::MAX).contains("interval_ms"));
    let mut c = Config::default();
    c.general.interval_ms = MAX_INTERVAL_MS;
    c.validate().unwrap();
    // Underscores are allowed in host names (Windows machine names).
    c.network.tcp_unicast = vec!["my_pc:48000".into(), "lab_box.lan".into()];
    c.validate().unwrap();
    assert_eq!(
        parse_target_spec("My_PC:48001", 1),
        Ok(TargetSpec::Host("my_pc".into(), 48001))
    );
    c.network.tcp_unicast = vec!["bad host".into()];
    assert!(c.validate().is_err());
    let mut c = Config::default();
    c.general.interval_ms = 100;
    c.metrics.gpu_backend = "NVML".into();
    c.network.udp_unicast = vec!["[::1]:48000".into()];
    c.validate().unwrap();
    // Bare addresses and host names are valid; names are not resolved by validate().
    c.network.udp_unicast = vec!["192.168.1.60".into(), "no-such-host.invalid".into()];
    c.network.tcp_unicast = vec!["viewer.lan:48001".into()];
    c.validate().unwrap();
    // gpu_adapter: auto, #N, or a non-empty part of a name.
    for ok in [
        "auto",
        "AUTO",
        "#0",
        "# 2",
        "R9700",
        "0x00000000_0x0000c2b6",
    ] {
        c.metrics.gpu_adapter = ok.into();
        c.validate().unwrap();
    }
    for bad in ["", "  ", "#", "#x", "#-1"] {
        assert!(invalid(|c| c.metrics.gpu_adapter = bad.into()).contains("gpu_adapter"));
    }
    assert!(
        invalid(|c| c.metrics.disk_include = vec!["sda".into(), " ".into()])
            .contains("disk_include")
    );
}

#[test]
fn parse_target_forms() {
    let a = |s: &str| s.parse::<SocketAddr>().unwrap();
    assert_eq!(
        parse_target("192.168.1.60", 48000),
        Ok(a("192.168.1.60:48000"))
    );
    assert_eq!(
        parse_target(" 10.0.0.2:48001 ", 48000),
        Ok(a("10.0.0.2:48001"))
    );
    assert_eq!(parse_target("[::1]:48000", 1), Ok(a("[::1]:48000")));
    assert_eq!(parse_target("[::1]", 48000), Ok(a("[::1]:48000")));
    assert_eq!(parse_target("fe80::1", 48000), Ok(a("[fe80::1]:48000")));
    assert_eq!(
        parse_target("192.168.1.5", 48123),
        Ok(a("192.168.1.5:48123"))
    );
    let l = parse_target("localhost", 48000).unwrap();
    assert!(l.ip().is_loopback() && l.port() == 48000, "{l}");
    assert_eq!(parse_target("localhost:9", 48000).unwrap().port(), 9);
    assert_eq!(
        parse_target_spec("Viewer.LAN:48001", 48000),
        Ok(TargetSpec::Host("viewer.lan".into(), 48001))
    );
    assert_eq!(TargetSpec::Host("pc".into(), 1).to_string(), "pc:1");
    for bad in [
        "",
        "garbage!",
        "two words",
        "1.2.3.4:x",
        "1.2.3.4:0",
        "1.2.3.4:70000",
        "192.168.1.300",
        "10.0.0",
        "fe80::1:zz:1",
        "[::1",
        "[::1]48000",
        "-host",
    ] {
        let e = parse_target(bad, 48000).unwrap_err();
        assert!(e.len() <= 40, "message too long for a row: {e}");
    }
    assert!(parse_target("1.2.3.4:x", 1).unwrap_err().contains("port"));
    // Syntax is fine but the name does not resolve.
    assert!(parse_target_spec("no-such-host.invalid", 1).is_ok());
    assert!(parse_target("no-such-host.invalid", 1).is_err());
}

#[test]
fn bare_ip_round_trips_and_uses_udp_port() {
    let toml = "[network]\nudp_port = 48123\nudp_unicast = [\"192.168.1.60\"]\ntcp_unicast = [\"10.0.0.9\", \"[::1]\"]\n";
    let c = Config::from_toml(toml).unwrap();
    c.validate().unwrap();
    assert_eq!(c.network.udp_unicast, ["192.168.1.60"]);
    let again = Config::from_toml(&c.to_toml_string().unwrap()).unwrap();
    assert_eq!(again, c, "stored as written, not rewritten with the port");
    let t = c.to_targets().unwrap();
    assert_eq!(
        t[1..],
        [
            rtm_net::Target::UdpUnicast("192.168.1.60:48123".parse().unwrap()),
            rtm_net::Target::TcpUnicast("10.0.0.9:48123".parse().unwrap()),
            rtm_net::Target::TcpUnicast("[::1]:48123".parse().unwrap()),
        ]
    );
}

#[test]
fn host_names_are_passed_on_unresolved() {
    let mut c = Config::default();
    c.network.udp_broadcast = false;
    c.network.udp_unicast = vec!["no-such-host.invalid".into(), "10.0.0.7".into()];
    c.network.tcp_unicast = vec!["Viewer.LAN:48001".into()];
    let r = c.resolve_targets().unwrap();
    assert!(r.unresolved.is_empty());
    assert_eq!(
        r.targets,
        [
            rtm_net::Target::UdpHost {
                host: "no-such-host.invalid".into(),
                port: 48000
            },
            rtm_net::Target::UdpUnicast("10.0.0.7:48000".parse().unwrap()),
            rtm_net::Target::TcpHost {
                host: "viewer.lan".into(),
                port: 48001
            },
        ]
    );
    assert_eq!(c.to_targets().unwrap(), r.targets);
    // The plan (used to diff reloads) needs no DNS.
    let p = c.target_plan().unwrap();
    assert_eq!(
        p.udp[0],
        TargetSpec::Host("no-such-host.invalid".into(), 48000)
    );
}

#[test]
fn gpu_adapter_defaults_to_auto_and_round_trips() {
    assert_eq!(Metrics::default().gpu_adapter, "auto");
    assert_eq!(Config::default().to_collector_config().gpu_adapter, "auto");
    let c = Config::from_toml("[metrics]\ngpu_adapter = \" R9700 \"\n").unwrap();
    c.validate().unwrap();
    assert_eq!(c.to_collector_config().gpu_adapter, "R9700");
    let again = Config::from_toml(&c.to_toml_string().unwrap()).unwrap();
    assert_eq!(again, c);
    assert!(default_template("x").contains("gpu_adapter = \"auto\""));
}

#[test]
fn show_splash_defaults_on() {
    assert!(Gui::default().show_splash);
    let c = Config::from_toml("[gui]\nstart_minimized = true\n").unwrap();
    assert!(c.gui.show_splash, "missing key takes the default");
    let c = Config::from_toml("[gui]\nshow_splash = false\n").unwrap();
    assert!(!c.gui.show_splash);
    let again = Config::from_toml(&c.to_toml_string().unwrap()).unwrap();
    assert!(!again.gui.show_splash);
    assert!(default_template("x").contains("show_splash = true"));
}

const FULL: &str = r#"
[general]
host_name = "desk"
interval_ms = 5000
id = "11111111-2222-4333-8444-555555555555"

[network]
udp_broadcast = false
udp_port = 48001
broadcast_interfaces = ["enp0s31f6", "wlan0"]
skip_interface_prefixes = ["docker"]
udp_unicast = ["192.168.1.50:48000"]
tcp_unicast = ["192.168.1.51:48002", "10.0.0.9:48000"]
shared_secret = "hunter2"

[metrics]
enabled = ["cpu", "mem", "net"]
gpu_backend = "amdgpu"
disk_include = ["nvme0n1"]

[gui]
start_minimized = false
autostart = true
"#;

#[test]
fn full_hand_written_toml_parses() {
    let c = Config::from_toml(FULL).unwrap();
    c.validate().unwrap();
    assert_eq!(c.general.host_name, "desk");
    assert_eq!(c.effective_host_name(), "desk");
    assert_eq!(c.general.interval_ms, 5000);
    assert!(!c.network.udp_broadcast);
    assert_eq!(c.network.udp_port, 48001);
    assert_eq!(
        c.network.broadcast_interfaces,
        BroadcastInterfaces::Named(vec!["enp0s31f6".into(), "wlan0".into()])
    );
    assert_eq!(c.secret(), Some("hunter2"));
    assert_eq!(c.metrics.disk_include, vec!["nvme0n1".to_string()]);
    assert!(c.gui.autostart && !c.gui.start_minimized);

    let cc = c.to_collector_config();
    assert_eq!(cc.enabled, vec!["cpu", "mem", "net"]);
    assert_eq!(cc.gpu_backend, rtm_collect::GpuBackend::Amdgpu);
    assert_eq!(cc.disk_include, vec!["nvme0n1".to_string()]);
    assert_eq!(cc.net_skip_prefixes, vec!["docker".to_string()]);

    let t = c.to_targets().unwrap();
    assert_eq!(
        t,
        vec![
            rtm_net::Target::UdpUnicast("192.168.1.50:48000".parse().unwrap()),
            rtm_net::Target::TcpUnicast("192.168.1.51:48002".parse().unwrap()),
            rtm_net::Target::TcpUnicast("10.0.0.9:48000".parse().unwrap()),
        ]
    );
}

#[test]
fn default_targets_are_broadcast_all() {
    let t = Config::default().to_targets().unwrap();
    assert_eq!(
        t,
        vec![rtm_net::Target::UdpBroadcast {
            port: 48000,
            interfaces: rtm_net::BroadcastInterfaces::All,
            skip_prefixes: vec![
                "docker".into(),
                "br-".into(),
                "veth".into(),
                "virbr".into(),
                "lo".into()
            ],
        }]
    );
    let mut c = Config::default();
    c.network.broadcast_interfaces = BroadcastInterfaces::Mode("default".into());
    assert!(matches!(
        &c.to_targets().unwrap()[0],
        rtm_net::Target::UdpBroadcast {
            interfaces: rtm_net::BroadcastInterfaces::Default,
            ..
        }
    ));
}

#[test]
fn unknown_keys_tolerated_and_missing_keys_defaulted() {
    let c = Config::from_toml(
        "future = 1\n[general]\ninterval_ms = 15000\nnew_thing = \"x\"\n[brand_new_section]\na = 1\n",
    )
    .unwrap();
    assert_eq!(c.general.interval_ms, 15000);
    assert_eq!(c.general.id, "auto");
    assert_eq!(c.network, Network::default());
    assert_eq!(c.metrics, Metrics::default());
}

#[test]
fn empty_host_name_falls_back_to_system() {
    let c = Config::default();
    assert!(!c.effective_host_name().is_empty());
    let mut c = Config::default();
    c.general.host_name = "  ".into();
    assert!(!c.effective_host_name().trim().is_empty());
}

#[test]
fn secret_empty_is_none() {
    assert_eq!(Config::default().secret(), None);
}
