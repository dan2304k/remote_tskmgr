//! `general.privacy`: strip device-identifying names from a tick's metrics before the packet
//! is built. GPU adapter names become "GPU #N" (N = the detected adapter index), Windows
//! adapter LUIDs are dropped, disk names become "disk0".., sensor names "sensor0..". Values,
//! units and per-core arrays are left as they are.

use rtm_protocol::serde_json::{Map, Value};
use rtm_protocol::Metric;

/// Apply the privacy transformation to every metric in place.
pub fn anonymise(metrics: &mut [(String, Metric)]) {
    for (key, m) in metrics.iter_mut() {
        let Some(d) = m.detail.as_mut() else { continue };
        if key == "gpu" {
            anonymise_gpu(d);
        }
        renumber_keys(d, "disks", "disk");
        renumber_keys(d, "sensors", "sensor");
    }
}

fn gpu_label(index: impl std::fmt::Display) -> Value {
    Value::from(format!("GPU #{index}"))
}

fn anonymise_gpu(d: &mut Map<String, Value>) {
    let index = d.get("adapter_index").and_then(Value::as_u64).unwrap_or(0);
    if d.contains_key("name") {
        d.insert("name".into(), gpu_label(index));
    }
    // PDH: the chosen adapter's LUID.
    d.remove("adapter");
    if let Some(Value::Object(adapters)) = d.get_mut("adapters") {
        let old = std::mem::take(adapters);
        let mut extra = 0;
        for (k, mut v) in old {
            // "#N" keys are detected-adapter indices; anything else (a LUID) is renumbered.
            let k = match k.strip_prefix('#').filter(|n| n.parse::<u64>().is_ok()) {
                Some(n) => {
                    if let Some(e) = v.as_object_mut().filter(|e| e.contains_key("name")) {
                        e.insert("name".into(), gpu_label(n));
                    }
                    k
                }
                None => {
                    extra += 1;
                    format!("other{}", extra - 1)
                }
            };
            if let Some(e) = v.as_object_mut() {
                e.remove("luid");
            }
            adapters.insert(k, v);
        }
    }
}

/// `detail[field]` is an object keyed by device name: rename the keys `{prefix}0`, `{prefix}1`..
/// in their current order (the collectors emit them sorted by name, so this is stable).
fn renumber_keys(d: &mut Map<String, Value>, field: &str, prefix: &str) {
    if let Some(Value::Object(o)) = d.get_mut(field) {
        let old = std::mem::take(o);
        o.extend(
            old.into_iter()
                .enumerate()
                .map(|(i, (_, v))| (format!("{prefix}{i}"), v)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtm_protocol::serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn sample() -> Vec<(String, Metric)> {
        let cores = rtm_collect::cores_detail(&[10.0, 20.0, 30.0]);
        let mut cpu = obj(json!({"freq_mhz": 4000, "temp_c": 55.0}));
        cpu.extend(cores);
        vec![
            (
                "cpu".into(),
                Metric::percent("CPU", Some(20.0)).with_detail(cpu),
            ),
            (
                "gpu".into(),
                Metric::percent("GPU", Some(50.0)).with_detail(obj(json!({
                    "name": "AMD Radeon AI PRO R9700",
                    "adapter_index": 1,
                    "adapter": "0x0000_0000_0001_2345",
                    "selected_by": "auto",
                    "vram_used_mb": 1000,
                    "backend": "pdh",
                    "adapters": {
                        "#0": {"name": "Intel(R) UHD Graphics 770", "luid": "0x1", "util": 1.0, "chosen": false},
                        "#1": {"name": "AMD Radeon AI PRO R9700", "luid": "0x2", "util": 50.0, "chosen": true},
                        "0x0000_0000_0009_9999": {"util": 3.0, "chosen": false}
                    }
                }))),
            ),
            (
                "disk".into(),
                Metric::rate_mbps("DISK", Some(3.0)).with_detail(obj(json!({
                    "read_mbps": 1.0, "write_mbps": 2.0,
                    "disks": {"Samsung SSD 990 PRO (C:)": {"read_mbps": 1.0, "write_mbps": 0.0},
                              "nvme1n1": {"read_mbps": 0.0, "write_mbps": 2.0}}
                }))),
            ),
            (
                "temp".into(),
                Metric::percent("TEMP", Some(60.0)).with_detail(obj(json!({
                    "sensors": {"coretemp Package id 0": 60.0, "nvme Composite WD_BLACK SN850X": 41.0}
                }))),
            ),
            ("mem".into(), Metric::percent("MEM", Some(40.0))),
        ]
    }

    #[test]
    fn names_are_replaced_and_values_kept() {
        let before = sample();
        let mut m = sample();
        anonymise(&mut m);
        let text = rtm_protocol::serde_json::to_string(
            &m.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        for leak in [
            "R9700", "Intel(R)", "0x1", "0x2", "0x0000", "Samsung", "nvme1n1", "coretemp",
            "WD_BLACK",
        ] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }

        let gpu = m[1].1.detail.as_ref().unwrap();
        assert_eq!(gpu["name"], "GPU #1");
        assert!(gpu.get("adapter").is_none());
        assert_eq!(gpu["adapter_index"], 1);
        assert_eq!(
            (gpu["vram_used_mb"].clone(), gpu["backend"].clone()),
            (json!(1000), json!("pdh"))
        );
        let ad = gpu["adapters"].as_object().unwrap();
        assert_eq!(ad.keys().collect::<Vec<_>>(), ["#0", "#1", "other0"]);
        assert_eq!(
            ad["#0"],
            json!({"name": "GPU #0", "util": 1.0, "chosen": false})
        );
        assert_eq!(
            ad["#1"],
            json!({"name": "GPU #1", "util": 50.0, "chosen": true})
        );
        assert_eq!(ad["other0"], json!({"util": 3.0, "chosen": false}));

        let disk = m[2].1.detail.as_ref().unwrap();
        assert_eq!(
            disk["disks"],
            json!({"disk0": {"read_mbps": 1.0, "write_mbps": 0.0}, "disk1": {"read_mbps": 0.0, "write_mbps": 2.0}})
        );
        assert_eq!(disk["read_mbps"], 1.0);
        let temp = m[3].1.detail.as_ref().unwrap();
        assert_eq!(temp["sensors"], json!({"sensor0": 60.0, "sensor1": 41.0}));

        // Metrics without names are untouched, per-core arrays included.
        assert_eq!(m[0], before[0]);
        assert_eq!(m[4], before[4]);
        assert!(m[0].1.detail.as_ref().unwrap().contains_key("cores"));
        for (a, b) in m.iter().zip(&before) {
            assert_eq!((a.0.as_str(), a.1.v), (b.0.as_str(), b.1.v));
        }
    }

    #[test]
    fn gpu_without_index_or_details_is_fine() {
        let mut m = vec![
            (
                "gpu".into(),
                Metric::percent("GPU", Some(1.0)).with_detail(obj(json!({"name": "RTX 4090"}))),
            ),
            ("gpu2".into(), Metric::percent("GPU", None)),
        ];
        anonymise(&mut m);
        assert_eq!(m[0].1.detail.as_ref().unwrap()["name"], "GPU #0");
        assert!(m[1].1.detail.is_none());
        anonymise(&mut []);
    }
}
