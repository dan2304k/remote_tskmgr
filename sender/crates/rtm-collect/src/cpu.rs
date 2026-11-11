//! `cpu`: global utilisation, per-core %, average frequency, CPU package temperature.
//!
//! Up to [`MAX_LISTED_CORES`] logical cores are sent as `detail.cores: [pct, ...]`. Above
//! that (packet size) a summary replaces it: `cores_n`, `cores_max`, `cores_min` and the
//! [`TOP_CORES`] busiest as `cores_top: [[index, pct], ...]`, busiest first.
//!
//! sysinfo computes usage from the difference between two reads of the kernel counters, so
//! the collector takes a baseline reading when it is constructed, and two reads must be at
//! least `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` (200 ms) apart. Ticks faster than that
//! (10/s) repeat the last reading. Only before the first valid reading (within 200 ms of
//! construction) is the metric returned with `v: null`. Callers that sample once (`--once`)
//! should wait at least [`crate::WARMUP`] after `Collectors::new`.

use crate::util::{detail, num, round, Hold, MIN_WINDOW};
use crate::Collector;
use rtm_protocol::Metric;
use serde_json::Value;
use std::time::{Duration, Instant};
use sysinfo::{Components, CpuRefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL};

/// Sensor-label fragments (lowercased) that identify a CPU temperature sensor.
pub(crate) const CPU_SENSOR_HINTS: &[&str] =
    &["cpu", "core", "package", "tctl", "k10temp", "coretemp"];

pub(crate) fn is_cpu_sensor(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    CPU_SENSOR_HINTS.iter().any(|h| l.contains(h))
}

pub(crate) fn valid_temp(t: Option<f32>) -> Option<f64> {
    t.map(f64::from)
        .filter(|t| t.is_finite() && *t > -40.0 && *t < 150.0)
}

/// Most logical cores sent as a full `cores` list.
pub const MAX_LISTED_CORES: usize = 16;
/// Busiest cores listed in `cores_top` when the full list is too long.
pub const TOP_CORES: usize = 8;

/// The per-core detail keys for these usages (already rounded, 0..=100).
pub fn cores_detail(usages: &[f64]) -> Vec<(String, Value)> {
    if usages.len() <= MAX_LISTED_CORES {
        let cores = usages.iter().map(|&u| num(u)).collect();
        return vec![("cores".into(), Value::Array(cores))];
    }
    let max = usages.iter().copied().fold(f64::MIN, f64::max);
    let min = usages.iter().copied().fold(f64::MAX, f64::min);
    let mut idx: Vec<usize> = (0..usages.len()).collect();
    idx.sort_by(|&a, &b| usages[b].total_cmp(&usages[a]).then(a.cmp(&b)));
    let top = idx
        .into_iter()
        .take(TOP_CORES)
        .map(|i| Value::Array(vec![Value::from(i), num(usages[i])]))
        .collect();
    vec![
        ("cores_n".into(), Value::from(usages.len())),
        ("cores_max".into(), num(max)),
        ("cores_min".into(), num(min)),
        ("cores_top".into(), Value::Array(top)),
    ]
}

pub struct CpuCollector {
    sys: System,
    last_refresh: Instant,
    /// Last valid reading, repeated for ticks closer than `MIN_WINDOW` to it.
    hold: Hold<Metric>,
    /// Only CPU-ish sensors are kept so a tick refreshes just those files.
    components: Option<Components>,
}

impl CpuCollector {
    pub fn new() -> Option<Self> {
        let mut sys = System::new().ok()?;
        sys.refresh_cpu_list(CpuRefreshKind::everything());
        if sys.cpus().is_empty() {
            return None;
        }
        let components = Components::new_with_refreshed_list().ok().and_then(|c| {
            let any = c.iter().any(|c| is_cpu_sensor(c.label()));
            any.then_some(c)
        });
        Some(Self {
            sys,
            last_refresh: Instant::now(),
            hold: Hold::new(),
            components,
        })
    }

    fn cpu_temp(&mut self) -> Option<f64> {
        let comps = self.components.as_mut()?;
        let mut best: Option<f64> = None;
        // Prefer a package/die sensor over the hottest single core.
        let mut package: Option<f64> = None;
        for c in comps.iter_mut() {
            if !is_cpu_sensor(c.label()) {
                continue;
            }
            c.refresh();
            let Some(t) = valid_temp(c.temperature()) else {
                continue;
            };
            let l = c.label().to_ascii_lowercase();
            if l.contains("package") || l.contains("tctl") || l.contains("tdie") {
                package = Some(package.map_or(t, |p: f64| p.max(t)));
            }
            best = Some(best.map_or(t, |b: f64| b.max(t)));
        }
        package.or(best).map(|t| round(t, 0))
    }
}

impl Collector for CpuCollector {
    fn id(&self) -> &str {
        "cpu"
    }

    fn sample(&mut self, _dt: Duration) -> Option<Metric> {
        let now = Instant::now();
        if let Some(m) = self.hold.recent(now, MIN_WINDOW) {
            return Some(m);
        }
        if now.duration_since(self.last_refresh) < MINIMUM_CPU_UPDATE_INTERVAL {
            // Only possible before the first valid reading.
            return Some(Metric::percent("CPU", None));
        }
        self.sys.refresh_cpu_specifics(CpuRefreshKind::everything());
        self.last_refresh = now;

        let cpus = self.sys.cpus();
        let v = round(f64::from(self.sys.global_cpu_usage()).clamp(0.0, 100.0), 1);
        let cores: Vec<f64> = cpus
            .iter()
            .map(|c| round(f64::from(c.usage()).clamp(0.0, 100.0), 1))
            .collect();
        let freqs: Vec<u64> = cpus
            .iter()
            .map(|c| c.frequency())
            .filter(|f| *f > 0)
            .collect();

        let mut d = detail();
        d.extend(cores_detail(&cores));
        if !freqs.is_empty() {
            let avg = freqs.iter().sum::<u64>() / freqs.len() as u64;
            d.insert("freq_mhz".into(), Value::from(avg));
        }
        if let Some(t) = self.cpu_temp() {
            d.insert("temp_c".into(), num(t));
        }
        #[cfg(not(windows))]
        if let Ok(la) = System::load_average() {
            d.insert("load1".into(), num(round(la.one, 2)));
        }
        let m = Metric::percent("CPU", Some(v)).with_detail(d);
        self.hold.store(now, m.clone());
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_ticks_are_never_null_after_warmup() {
        let Some(mut c) = CpuCollector::new() else {
            return; // no CPU info on this host
        };
        let start = Instant::now();
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(100));
            let m = c.sample(Duration::from_millis(100)).unwrap();
            if start.elapsed() >= MINIMUM_CPU_UPDATE_INTERVAL {
                assert!(m.v.is_some(), "null at {:?}", start.elapsed());
            }
        }
    }

    #[test]
    fn many_cores_are_summarised() {
        let few: Vec<f64> = (0..16).map(f64::from).collect();
        let d = cores_detail(&few);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].0, "cores");
        assert_eq!(d[0].1.as_array().unwrap().len(), 16);

        let many: Vec<f64> = (0..128).map(|i| f64::from((i * 37) % 101)).collect();
        let d: serde_json::Map<String, Value> = cores_detail(&many).into_iter().collect();
        assert!(!d.contains_key("cores"));
        assert_eq!(d["cores_n"], 128);
        assert_eq!(d["cores_max"], 100.0);
        assert_eq!(d["cores_min"], 0.0);
        let top = d["cores_top"].as_array().unwrap();
        assert_eq!(top.len(), TOP_CORES);
        assert_eq!(top[0][1], 100.0);
        let i = top[0][0].as_u64().unwrap() as usize;
        assert_eq!(many[i], 100.0);
        let pcts: Vec<f64> = top.iter().map(|e| e[1].as_f64().unwrap()).collect();
        assert!(pcts.windows(2).all(|w| w[0] >= w[1]), "busiest first");
    }

    #[test]
    fn sensor_names() {
        assert!(is_cpu_sensor("coretemp Package id 0"));
        assert!(is_cpu_sensor("k10temp Tctl"));
        assert!(is_cpu_sensor("Core 3"));
        assert!(!is_cpu_sensor("nvme Composite"));
        assert!(!is_cpu_sensor("amdgpu edge"));
    }
}
