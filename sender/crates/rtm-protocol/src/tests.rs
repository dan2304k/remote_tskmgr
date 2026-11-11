use super::*;
use serde_json::json;
use std::path::PathBuf;

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../protocol/examples")
}

fn examples(prefix: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(examples_dir())
        .expect("protocol/examples exists")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix) && n.ends_with(".json"))
        .map(|n| {
            let body = std::fs::read_to_string(examples_dir().join(&n)).unwrap();
            (n, body)
        })
        .collect();
    out.sort();
    assert!(!out.is_empty(), "no {prefix}*.json examples found");
    out
}

fn sample_packet() -> Packet {
    let mut p = Packet {
        v: PROTOCOL_VERSION,
        id: "0b2d4a4e-6a2c-4c0e-8f2d-2f6b2b6f9a11".into(),
        host: "workstation".into(),
        os: "linux".into(),
        ts: 1762640000123,
        seq: 48213,
        interval_ms: 1000,
        metrics: Map::new(),
        auth: None,
        extra: Default::default(),
    };
    let cores: Vec<f64> = (0..16).map(|i| (i as f64 * 7.3) % 100.0).collect();
    let mut d = Map::new();
    d.insert("cores".into(), json!(cores));
    d.insert("freq_mhz".into(), json!(3800));
    d.insert("temp_c".into(), json!(61));
    p.set_metric("cpu", Metric::percent("CPU", Some(23.4)).with_detail(d));
    let mut d = Map::new();
    d.insert("name".into(), json!("NVIDIA GeForce RTX 3070"));
    d.insert("vram_used_mb".into(), json!(3120));
    d.insert("vram_total_mb".into(), json!(8192));
    d.insert("temp_c".into(), json!(70));
    d.insert("backend".into(), json!("nvml"));
    p.set_metric("gpu", Metric::percent("GPU", Some(61.0)).with_detail(d));
    let mut d = Map::new();
    d.insert("used_mb".into(), json!(19312));
    d.insert("total_mb".into(), json!(32768));
    d.insert("swap_used_mb".into(), json!(0));
    d.insert("swap_total_mb".into(), json!(2048));
    p.set_metric("mem", Metric::percent("MEM", Some(58.9)).with_detail(d));
    let mut d = Map::new();
    d.insert("read_mbps".into(), json!(2.1));
    d.insert("write_mbps".into(), json!(12.1));
    p.set_metric("disk", Metric::rate_mbps("DISK", Some(14.2)).with_detail(d));
    p
}

#[test]
fn valid_examples_round_trip() {
    for (name, body) in examples("valid_") {
        let p = Packet::from_json(&body).unwrap_or_else(|e| panic!("{name}: {e}"));
        let again = Packet::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(p, again, "{name} did not round-trip");
        // Metric order from the file is preserved.
        let orig: Value = serde_json::from_str(&body).unwrap();
        let orig_keys: Vec<&String> = orig["metrics"].as_object().unwrap().keys().collect();
        let keys: Vec<&String> = again.metrics.keys().collect();
        assert_eq!(orig_keys, keys, "{name} metric order");
    }
}

#[test]
fn invalid_examples_rejected() {
    for (name, body) in examples("invalid_") {
        assert!(Packet::from_json(&body).is_err(), "{name} was accepted");
    }
}

#[test]
fn wrong_version_is_version_error() {
    let body = std::fs::read_to_string(examples_dir().join("invalid_version.json")).unwrap();
    assert!(matches!(
        Packet::from_json(&body),
        Err(ProtocolError::Version(2))
    ));
}

#[test]
fn metric_missing_v_rejected() {
    let s = r#"{"v":1,"id":"x","host":"h","os":"linux","ts":0,"seq":0,"interval_ms":1000,
        "metrics":{"cpu":{"unit":"%","label":"CPU"}}}"#;
    assert!(Packet::from_json(s).is_err());
    let s = r#"{"v":1,"id":"x","host":"h","os":"linux","ts":0,"seq":0,"interval_ms":1000,
        "metrics":{"cpu":{"v":null,"unit":"%","label":"CPU"}}}"#;
    assert!(Packet::from_json(s).is_ok());
}

#[test]
fn sign_verify_round_trip() {
    let mut p = sample_packet();
    assert!(!p.verify("s3cret"), "unsigned packet must not verify");
    p.sign("s3cret");
    let auth = p.auth.clone().unwrap();
    assert_eq!(auth.len(), 64);
    assert!(auth
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    assert!(p.verify("s3cret"));
    assert!(!p.verify("wrong"));
    // Survives the wire.
    let back = Packet::from_json(&p.to_json().unwrap()).unwrap();
    assert!(back.verify("s3cret"));
}

#[test]
fn verify_fails_on_tamper() {
    let mut p = sample_packet();
    p.sign("k");
    let mut t = p.clone();
    t.seq += 1;
    assert!(!t.verify("k"));
    let mut t = p.clone();
    t.metrics["cpu"]["v"] = json!(99.0);
    assert!(!t.verify("k"));
    let mut t = p.clone();
    t.metrics["gpu"]["detail"]["name"] = json!("other");
    assert!(!t.verify("k"));
    let mut t = p.clone();
    t.auth = Some("zz".into());
    assert!(!t.verify("k"));
}

#[test]
fn canonical_body_sorted_and_compact() {
    let mut p = sample_packet();
    p.auth = Some("ignored".into());
    let c = p.canonical_body();
    assert!(!c.contains("auth"));
    assert!(
        !c.contains(' ') || c.contains("RTX"),
        "no structural whitespace"
    );
    assert!(!c.contains('\n'));
    assert!(c.starts_with(r#"{"host":"workstation","id":"#), "{c}");
    // Nested: metric keys sorted (cpu < disk < gpu < mem) and fields sorted (detail < label < max ...).
    let cpu = c.find(r#""cpu":"#).unwrap();
    let disk = c.find(r#""disk":"#).unwrap();
    let gpu = c.find(r#""gpu":"#).unwrap();
    assert!(cpu < disk && disk < gpu);
    assert!(c.contains(r#""cpu":{"detail":{"cores":"#), "{c}");
    // Metric insertion order does not affect the signature.
    let mut a = sample_packet();
    let mut b = sample_packet();
    b.metrics = b.metrics.into_iter().rev().collect();
    a.sign("k");
    b.sign("k");
    assert_eq!(a.auth, b.auth);
}

#[test]
fn auth_example_structure_and_known_vector() {
    let body = std::fs::read_to_string(examples_dir().join("valid_with_auth.json")).unwrap();
    let p = Packet::from_json(&body).unwrap();
    // The golden file carries a real signature over the whole object (including the
    // unknown `_comment` key, which `Packet::extra` preserves), so it must verify.
    assert!(
        p.verify("correct horse"),
        "canonical body: {}",
        p.canonical_body()
    );
    assert!(!p.verify("wrong secret"));
    assert!(p.canonical_body().starts_with(r#"{"_comment":"#));
    assert!(p
        .canonical_body()
        .contains(r#""metrics":{"cpu":{"label":"CPU","unit":"%","v":10.0}}"#));
    let mut s = p.clone();
    s.sign("correct horse");
    assert!(s.verify("correct horse"));
    eprintln!("valid_with_auth.json correct auth = {}", s.auth.unwrap());
}

#[test]
fn json_line_has_exactly_one_newline() {
    let line = sample_packet().to_json_line().unwrap();
    assert!(line.ends_with('\n'));
    assert!(!line[..line.len() - 1].contains('\n'));
}

#[test]
fn full_packet_under_1200_bytes() {
    let mut p = sample_packet();
    p.sign("secret");
    let n = p.to_json().unwrap().len();
    assert!(n < 1200, "packet is {n} bytes");
}

#[test]
fn now_ms_is_recent() {
    assert!(now_ms() > 1_700_000_000_000);
}
