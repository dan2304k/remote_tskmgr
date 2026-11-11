//! Intel sysfs backend (Linux). Busy % needs perf/root, so this backend reports `v: null` and
//! puts the current and maximum GT frequency in detail with `approx: true`. It never
//! fabricates a percentage.
//!
//! Frequency files, first found wins:
//! * i915 legacy: `card*/gt_cur_freq_mhz`, `card*/gt_max_freq_mhz`
//! * i915 per-GT: `card*/gt/gt0/rps_cur_freq_mhz`, `rps_max_freq_mhz`
//! * xe: `card*/device/tile0/gt0/freq0/cur_freq`, `max_freq`

use super::sysfs::{self, cards, gpu_name, pci_ids, read_u64};
use super::GpuSource;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

const FREQ_LAYOUTS: &[(&str, &str)] = &[
    ("gt_cur_freq_mhz", "gt_max_freq_mhz"),
    ("gt/gt0/rps_cur_freq_mhz", "gt/gt0/rps_max_freq_mhz"),
    (
        "device/tile0/gt0/freq0/cur_freq",
        "device/tile0/gt0/freq0/max_freq",
    ),
];

pub struct IntelGpu {
    name: String,
    cur: PathBuf,
    max: PathBuf,
}

impl IntelGpu {
    pub fn probe(drm_root: &Path) -> Option<Self> {
        cards(drm_root).into_iter().find_map(|card| {
            let (vendor, device) = pci_ids(&card)?;
            if vendor != sysfs::VENDOR_INTEL {
                return None;
            }
            FREQ_LAYOUTS.iter().find_map(|(cur, max)| {
                let cur = card.join(cur);
                read_u64(&cur)?;
                Some(Self {
                    name: gpu_name(vendor, device),
                    cur,
                    max: card.join(max),
                })
            })
        })
    }
}

impl GpuSource for IntelGpu {
    fn backend(&self) -> &'static str {
        "intel-sysfs"
    }

    fn reports_utilisation(&self) -> bool {
        false
    }

    fn read(&mut self) -> (Option<f64>, Map<String, Value>) {
        let mut d = Map::new();
        d.insert("name".into(), Value::from(self.name.clone()));
        if let Some(f) = read_u64(&self.cur) {
            d.insert("freq_mhz".into(), Value::from(f));
        }
        if let Some(f) = read_u64(&self.max) {
            d.insert("max_freq_mhz".into(), Value::from(f));
        }
        d.insert("backend".into(), Value::from(self.backend()));
        d.insert("approx".into(), Value::Bool(true));
        (None, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn w(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn legacy_layout() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("card1-HDMI-A-1")).unwrap();
        w(r, "card1/device/vendor", "0x8086\n");
        w(r, "card1/device/device", "0x5912\n");
        w(r, "card1/gt_cur_freq_mhz", "350\n");
        w(r, "card1/gt_max_freq_mhz", "1150\n");
        let mut g = IntelGpu::probe(r).expect("intel found");
        let (v, d) = g.read();
        assert_eq!(v, None);
        assert_eq!(d["name"], "Intel GPU [8086:5912]");
        assert_eq!(d["freq_mhz"], 350);
        assert_eq!(d["max_freq_mhz"], 1150);
        assert_eq!(d["backend"], "intel-sysfs");
        assert_eq!(d["approx"], true);
        let keys: Vec<&str> = d.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["name", "freq_mhz", "max_freq_mhz", "backend", "approx"]
        );
    }

    #[test]
    fn per_gt_and_xe_layouts() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        w(r, "card0/device/vendor", "0x8086\n");
        w(r, "card0/device/device", "0x56a0\n");
        w(r, "card0/gt/gt0/rps_cur_freq_mhz", "900\n");
        w(r, "card0/gt/gt0/rps_max_freq_mhz", "2400\n");
        let (_, d) = IntelGpu::probe(r).unwrap().read();
        assert_eq!(d["freq_mhz"], 900);
        assert_eq!(d["max_freq_mhz"], 2400);

        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        w(r, "card0/device/vendor", "0x8086\n");
        w(r, "card0/device/device", "0xe20b\n");
        w(r, "card0/device/tile0/gt0/freq0/cur_freq", "1200\n");
        let (_, d) = IntelGpu::probe(r).unwrap().read();
        assert_eq!(d["freq_mhz"], 1200);
        assert!(!d.contains_key("max_freq_mhz"));
    }

    #[test]
    fn non_intel_or_no_freq() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        w(r, "card0/device/vendor", "0x1002\n");
        w(r, "card0/gt_cur_freq_mhz", "350\n");
        w(r, "card1/device/vendor", "0x8086\n");
        assert!(IntelGpu::probe(r).is_none());
    }
}
