//! Runs the real collectors on the build host.

use rtm_collect::{CollectorConfig, Collectors, GpuBackend, KNOWN_IDS, WARMUP};
use rtm_protocol::Metric;
use std::str::FromStr;
use std::time::Duration;

fn cfg(ids: &[&str]) -> CollectorConfig {
    CollectorConfig {
        enabled: ids.iter().map(|s| s.to_string()).collect(),
        gpu_backend: GpuBackend::Auto,
        gpu_adapter: "auto".into(),
        disk_include: Vec::new(),
        ..CollectorConfig::default()
    }
}

fn expected_label_unit(id: &str) -> (&'static str, &'static str) {
    match id {
        "cpu" => ("CPU", "%"),
        "gpu" => ("GPU", "%"),
        "mem" => ("MEM", "%"),
        "disk" => ("DISK", "MB/s"),
        "net" => ("NET", "MB/s"),
        "temp" => ("TEMP", "°C"),
        "load" => ("LOAD", ""),
        "uptime" => ("UP", "s"),
        _ => unreachable!(),
    }
}

fn check(id: &str, m: &Metric) {
    let (label, unit) = expected_label_unit(id);
    assert_eq!(m.label, label, "{id} label");
    assert_eq!(m.unit, unit, "{id} unit");
    let min = m.min.unwrap_or(0.0);
    if let Some(v) = m.v {
        assert!(v.is_finite(), "{id}: {v}");
        assert!(v >= min, "{id}: {v} < {min}");
        if let Some(max) = m.max {
            // Load average may legitimately exceed the core count; everything else must not.
            if id != "load" {
                assert!(v <= max, "{id}: {v} > {max}");
            }
        }
    }
    match id {
        "disk" | "net" | "uptime" => assert_eq!(m.max, None, "{id} max must be auto"),
        "cpu" | "gpu" | "mem" | "temp" => assert_eq!(m.max, Some(100.0), "{id} max"),
        _ => {}
    }
    // Every metric must serialise.
    serde_json::to_string(m).unwrap();
}

#[test]
fn every_collector_sane_on_host() {
    let mut c = Collectors::new(&cfg(KNOWN_IDS));
    let ids: Vec<String> = c.ids().iter().map(|s| s.to_string()).collect();
    for must in ["cpu", "mem", "disk", "net", "uptime"] {
        assert!(ids.iter().any(|i| i == must), "{must} missing from {ids:?}");
    }
    #[cfg(target_os = "linux")]
    assert!(ids.iter().any(|i| i == "load"));
    std::thread::sleep(WARMUP + Duration::from_millis(50));
    for round in 0..2 {
        let s = c.sample_all(Duration::from_millis(300));
        let got: Vec<&str> = s.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(got, ids, "sample order follows config order");
        for (k, m) in &s {
            check(k, m);
        }
        if round == 0 {
            let cpu = &s.iter().find(|(k, _)| k == "cpu").unwrap().1;
            assert!(cpu.v.is_some(), "cpu has a value after warmup");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[test]
fn gpu_metric_has_backend_when_present() {
    let mut c = Collectors::new(&cfg(&["gpu"]));
    if c.is_empty() {
        return; // no backend on this host: metric omitted, which is correct.
    }
    let s = c.sample_all(Duration::from_secs(1));
    let (k, m) = &s[0];
    assert_eq!(k, "gpu");
    check(k, m);
    assert!(m.detail.as_ref().unwrap().contains_key("backend"));
}

#[test]
fn gpu_none_is_omitted() {
    let mut c = cfg(&["gpu", "mem"]);
    c.gpu_backend = GpuBackend::None;
    assert_eq!(Collectors::new(&c).ids(), ["mem"]);
}

#[test]
fn rates_non_negative_across_samples() {
    let mut c = Collectors::new(&cfg(&["disk", "net"]));
    assert_eq!(c.ids(), ["disk", "net"]);
    for _ in 0..3 {
        for (k, m) in c.sample_all(Duration::from_millis(300)) {
            let v = m.v.expect("rate always has a value");
            assert!(v >= 0.0, "{k}: {v}");
            let d = m.detail.unwrap();
            let parts: &[&str] = if k == "disk" {
                &["read_mbps", "write_mbps"]
            } else {
                &["rx_mbps", "tx_mbps"]
            };
            for p in parts {
                assert!(d[*p].as_f64().unwrap() >= 0.0, "{k}.{p}");
            }
            if k == "disk" {
                for (name, e) in d["disks"].as_object().unwrap() {
                    assert!(e["read_mbps"].as_f64().unwrap() >= 0.0, "{name}");
                    assert!(e["write_mbps"].as_f64().unwrap() >= 0.0, "{name}");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[test]
fn config_order_and_unknown_keys() {
    let c = Collectors::new(&cfg(&["cpu", "bogus", "mem"]));
    assert_eq!(c.ids(), ["cpu", "mem"]);
    let c = Collectors::new(&cfg(&["mem", "cpu", "mem"]));
    assert_eq!(c.ids(), ["mem", "cpu"]);
    let c = Collectors::new(&cfg(&[]));
    assert!(c.is_empty());
}

#[test]
fn disk_include_filters() {
    let mut c = cfg(&["disk"]);
    c.disk_include = vec!["definitely-not-a-disk".into()];
    let mut cs = Collectors::new(&c);
    let s = cs.sample_all(Duration::from_secs(1));
    let d = s[0].1.detail.as_ref().unwrap();
    assert!(d["disks"].as_object().unwrap().is_empty());
    assert_eq!(s[0].1.v, Some(0.0));
}

#[test]
fn gpu_backend_from_str() {
    for (s, b) in [
        ("auto", GpuBackend::Auto),
        ("NVML", GpuBackend::Nvml),
        ("amdgpu", GpuBackend::Amdgpu),
        ("Intel", GpuBackend::Intel),
        ("pdh", GpuBackend::Pdh),
        (" none ", GpuBackend::None),
    ] {
        assert_eq!(GpuBackend::from_str(s), Ok(b), "{s}");
        assert_eq!(GpuBackend::from_str(b.as_str()), Ok(b));
    }
    assert!(GpuBackend::from_str("cuda").is_err());
    assert!(GpuBackend::from_str("").is_err());
    assert_eq!(GpuBackend::default(), GpuBackend::Auto);
}
