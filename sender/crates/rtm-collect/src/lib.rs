//! Metric collectors for the RTM sender. `rtm-sender` depends on `Collector`,
//! `CollectorConfig`, `Collectors`, `GpuBackend`.
//!
//! Each collector owns the minimal sysinfo object it needs (a `System` refreshed only for CPU
//! or memory, `Disks` refreshed only for I/O counters, ...); processes are never refreshed.
//! Labels, units and bounds follow the well-known table in `protocol/PROTOCOL.md`.
//!
//! First-tick behaviour: rate collectors (`disk`, `net`) take a counter baseline in
//! `Collectors::new` and measure real elapsed time themselves, so the first `sample_all`
//! after construction yields a rate over that interval (0 if it is called immediately; never
//! negative). `cpu` returns `v: null` only if sampled sooner than [`WARMUP`] after
//! construction. One-shot callers should sleep [`WARMUP`] between `new` and `sample_all`.
//!
//! Fast ticks: `cpu`, `disk` and `net` compute a new reading at most every 200 ms
//! ([`WARMUP`]); a tick closer than that to the last reading repeats it, so 10 packets/s
//! neither nulls the CPU nor makes rates spiky. The GPU sources need no such guard (NVML
//! reports its own sampled average; PDH computes over whatever interval passed).

use rtm_protocol::Metric;
use std::time::Duration;

mod cpu;
mod disk;
pub mod gpu;
#[cfg(not(windows))]
mod load;
mod mem;
mod net;
mod temp;
mod uptime;
mod util;

pub use cpu::{cores_detail, CpuCollector};
pub use disk::DiskCollector;
pub use gpu::GpuCollector;
#[cfg(not(windows))]
pub use load::LoadCollector;
pub use mem::MemCollector;
pub use net::NetCollector;
pub use temp::TempCollector;
pub use uptime::UptimeCollector;

/// Metric keys `Collectors::new` knows how to build.
pub const KNOWN_IDS: &[&str] = &["cpu", "gpu", "mem", "disk", "net", "temp", "load", "uptime"];

/// Minimum time between collector construction (or two samples) for CPU usage to be
/// meaningful. Equal to sysinfo's `MINIMUM_CPU_UPDATE_INTERVAL`.
pub const WARMUP: Duration = sysinfo::MINIMUM_CPU_UPDATE_INTERVAL;

/// One metric source. `sample` is called once per tick with the elapsed time since the
/// previous call (for rate metrics). Return `None` to omit the metric from this packet
/// entirely (e.g. backend unavailable); return `Some(Metric { v: None, .. })` for
/// "present but no value this tick".
pub trait Collector: Send {
    /// Metric key as it appears in the packet, e.g. "cpu".
    fn id(&self) -> &str;
    fn sample(&mut self, dt: Duration) -> Option<Metric>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GpuBackend {
    #[default]
    Auto,
    Nvml,
    Amdgpu,
    Intel,
    Pdh,
    None,
}

impl std::str::FromStr for GpuBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "nvml" => Self::Nvml,
            "amdgpu" => Self::Amdgpu,
            "intel" => Self::Intel,
            "pdh" => Self::Pdh,
            "none" => Self::None,
            other => return Err(format!("unknown gpu backend '{other}'")),
        })
    }
}

#[derive(Debug, Clone)]
pub struct CollectorConfig {
    /// Metric keys to enable, in packet order. Known: cpu, gpu, mem, disk, net, temp, load, uptime.
    pub enabled: Vec<String>,
    pub gpu_backend: GpuBackend,
    /// Which GPU to report: "auto" (most dedicated VRAM), "#N" (index in
    /// `gpu::adapters::list_adapters()`), or part of the adapter's name.
    pub gpu_adapter: String,
    /// Disk names to include (empty = all physical disks).
    pub disk_include: Vec<String>,
    /// `net`: interface-name prefixes to leave out (the same list and matching as
    /// `network.skip_interface_prefixes` for broadcasting: case-sensitive, `lo` matches `lo0`
    /// but not `local`). Loopback is always left out.
    pub net_skip_prefixes: Vec<String>,
}

/// Default `net_skip_prefixes` (same as the config default).
pub const DEFAULT_NET_SKIP_PREFIXES: &[&str] = &["docker", "br-", "veth", "virbr", "lo"];

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            enabled: Vec::new(),
            gpu_backend: GpuBackend::default(),
            gpu_adapter: String::new(),
            disk_include: Vec::new(),
            net_skip_prefixes: DEFAULT_NET_SKIP_PREFIXES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// The enabled set, in config order.
pub struct Collectors {
    inner: Vec<Box<dyn Collector>>,
}

impl Collectors {
    /// Build collectors for every key in `cfg.enabled`. Unknown keys are logged and skipped.
    /// Collectors that cannot work on this host (no GPU backend, no sensors, `load` on
    /// Windows) are omitted, logged at info. Duplicate keys are ignored after the first.
    /// Never panics.
    pub fn new(cfg: &CollectorConfig) -> Self {
        let mut inner: Vec<Box<dyn Collector>> = Vec::new();
        for key in &cfg.enabled {
            let key = key.trim().to_ascii_lowercase();
            if inner.iter().any(|c| c.id() == key) {
                tracing::warn!(metric = %key, "duplicate metric key in config, ignored");
                continue;
            }
            let built: Option<Box<dyn Collector>> = match key.as_str() {
                "cpu" => CpuCollector::new().map(|c| Box::new(c) as _),
                "mem" => MemCollector::new().map(|c| Box::new(c) as _),
                "disk" => DiskCollector::new(&cfg.disk_include).map(|c| Box::new(c) as _),
                "gpu" => {
                    GpuCollector::new(cfg.gpu_backend, &cfg.gpu_adapter).map(|c| Box::new(c) as _)
                }
                "net" => NetCollector::new(&cfg.net_skip_prefixes).map(|c| Box::new(c) as _),
                "temp" => TempCollector::new().map(|c| Box::new(c) as _),
                #[cfg(not(windows))]
                "load" => LoadCollector::new().map(|c| Box::new(c) as _),
                #[cfg(windows)]
                "load" => None,
                "uptime" => UptimeCollector::new().map(|c| Box::new(c) as _),
                other => {
                    tracing::warn!(metric = %other, known = ?KNOWN_IDS, "unknown metric key, skipped");
                    continue;
                }
            };
            match built {
                Some(c) => {
                    tracing::debug!(metric = %key, "collector enabled");
                    inner.push(c);
                }
                None => tracing::info!(
                    metric = %key,
                    gpu_backend = ?cfg.gpu_backend,
                    "collector unavailable on this host, metric omitted"
                ),
            }
        }
        Self { inner }
    }
    /// Sample every collector. Returns `(key, metric)` pairs in config order; collectors that
    /// returned `None` are absent.
    pub fn sample_all(&mut self, dt: Duration) -> Vec<(String, Metric)> {
        self.inner
            .iter_mut()
            .filter_map(|c| c.sample(dt).map(|m| (c.id().to_string(), m)))
            .collect()
    }
    pub fn ids(&self) -> Vec<&str> {
        self.inner.iter().map(|c| c.id()).collect()
    }
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    /// Add a custom collector at the end (extensibility hook; the built-in set is driven by
    /// config keys).
    pub fn push(&mut self, c: Box<dyn Collector>) {
        self.inner.push(c);
    }
}

impl CollectorConfig {
    /// The standard default: `["cpu", "gpu", "mem", "disk"]`, auto GPU backend, all disks.
    pub fn standard() -> Self {
        Self {
            enabled: ["cpu", "gpu", "mem", "disk"].map(String::from).to_vec(),
            gpu_backend: GpuBackend::Auto,
            gpu_adapter: "auto".into(),
            ..Self::default()
        }
    }
}

impl GpuBackend {
    /// Config spelling, inverse of `FromStr`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Nvml => "nvml",
            Self::Amdgpu => "amdgpu",
            Self::Intel => "intel",
            Self::Pdh => "pdh",
            Self::None => "none",
        }
    }
}

impl std::fmt::Display for GpuBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
