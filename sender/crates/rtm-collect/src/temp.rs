//! `temp`: hottest CPU temperature sensor in °C (`max` 100), falling back to the hottest
//! sensor of any kind when no CPU-ish sensor exists. `detail.sensors` lists up to 6 sensors
//! of every kind, hottest first.

use crate::cpu::{is_cpu_sensor, valid_temp};
use crate::util::{detail, num, round};
use crate::Collector;
use rtm_protocol::Metric;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};
use sysinfo::Components;

/// Packet size: the hottest few are what matters.
const MAX_SENSORS: usize = 6;

/// Windows: how often the sensors are read again. Each read is a WMI query that makes the
/// kernel evaluate the ACPI thermal zones, which is expensive, and a thermal zone moves
/// slowly. Elsewhere a read is a few small files and happens every tick.
const READ_EVERY: Duration = if cfg!(windows) {
    Duration::from_secs(5)
} else {
    Duration::ZERO
};

pub struct TempCollector {
    comps: Components,
    read_at: Instant,
}

impl TempCollector {
    /// `None` when the host exposes no temperature sensors (typical in VMs and on Windows
    /// without admin / WMI support), so the metric is omitted rather than permanently null.
    pub fn new() -> Option<Self> {
        let comps = Components::new_with_refreshed_list().ok()?;
        comps
            .iter()
            .any(|c| valid_temp(c.temperature()).is_some())
            .then(|| Self {
                comps,
                read_at: Instant::now(),
            })
    }
}

/// Build `(hottest, sensors)` from `(label, °C)` readings: hottest first, capped, labels
/// deduplicated by suffixing `#2`, `#3`.
pub(crate) fn summarise(mut readings: Vec<(String, f64)>) -> (Option<f64>, Map<String, Value>) {
    readings.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let hottest = readings.first().map(|r| round(r.1, 1));
    let mut sensors = Map::new();
    for (label, t) in readings {
        if sensors.len() >= MAX_SENSORS {
            break;
        }
        let mut key = label.clone();
        let mut n = 2;
        while sensors.contains_key(&key) {
            key = format!("{label} #{n}");
            n += 1;
        }
        sensors.insert(key, num(round(t, 1)));
    }
    (hottest, sensors)
}

impl Collector for TempCollector {
    fn id(&self) -> &str {
        "temp"
    }

    fn sample(&mut self, _dt: Duration) -> Option<Metric> {
        let now = Instant::now();
        if now.duration_since(self.read_at) >= READ_EVERY {
            self.comps.refresh(false);
            self.read_at = now;
        }
        let readings: Vec<(String, f64)> = self
            .comps
            .iter()
            .filter_map(|c| valid_temp(c.temperature()).map(|t| (c.label().to_string(), t)))
            .collect();
        let cpu_hot = readings
            .iter()
            .filter(|(l, _)| is_cpu_sensor(l))
            .map(|(_, t)| *t)
            .reduce(f64::max)
            .map(|t| round(t, 1));
        let (hottest, sensors) = summarise(readings);
        let mut d = detail();
        d.insert("sensors".into(), Value::Object(sensors));
        Some(Metric {
            v: cpu_hot.or(hottest),
            unit: "°C".into(),
            label: "TEMP".into(),
            min: Some(0.0),
            max: Some(100.0),
            detail: Some(d),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarise_caps_and_sorts() {
        let mut r: Vec<(String, f64)> = (0..20).map(|i| (format!("s{i}"), i as f64)).collect();
        r.push(("s19".into(), 5.0));
        let (hot, map) = summarise(r);
        assert_eq!(hot, Some(19.0));
        assert_eq!(map.len(), MAX_SENSORS);
        assert_eq!(map.keys().next().unwrap(), "s19");
    }
}
