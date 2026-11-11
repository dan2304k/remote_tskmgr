//! `gpu`: utilisation with a runtime-selected backend.
//!
//! `GpuBackend::Auto` tries, in order: NVML (feature `nvml`; libnvidia-ml / nvml.dll is
//! loaded dynamically so its absence is a soft failure), then on Linux amdgpu sysfs, then
//! Intel sysfs (frequency only, `v: null`, `approx: true`), then on Windows PDH. A candidate
//! that reports utilisation is chosen if its first read returns a value. If none does, the
//! first one that initialised is used and the others that initialised are kept as a reserve.
//!
//! Run-time rules (all backends except Intel sysfs, which is null by design):
//! * a null read repeats the last value for up to `HOLD_TICKS` ticks, so a single bad tick
//!   never shows as "-";
//! * Auto only: a backend that has never produced a value is replaced by the reserve after
//!   `MAX_MISSES` null reads in a row. This happens at most once, never back, and never once
//!   the backend has produced any value (no flip-flopping between backends).
//!
//! Explicitly chosen backends never switch. If nothing initialises the metric is omitted.

use crate::Collector;
use crate::GpuBackend;
use rtm_protocol::Metric;
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::time::Duration;

pub mod adapters;
pub mod amdgpu;
#[cfg(windows)]
mod dxgi;
pub mod intel;
#[cfg(feature = "nvml")]
pub mod nvml;
#[cfg(windows)]
pub mod pdh;
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod pdh_parse;
pub(crate) mod sysfs;

/// Consecutive reads without a utilisation value after which Auto tries the reserve backend
/// (only if the current one has never produced a value).
pub const MAX_MISSES: u32 = 5;
/// Null reads for which the last good value is repeated.
pub const HOLD_TICKS: u32 = 3;

/// One GPU backend after successful initialisation.
pub(crate) trait GpuSource: Send {
    /// Wire name, e.g. "nvml", "amdgpu-sysfs", "intel-sysfs", "windows-pdh".
    fn backend(&self) -> &'static str;
    /// Utilisation 0..=100 (or `None`) and detail fields. `backend` is added by the caller if
    /// the source did not insert it.
    fn read(&mut self) -> (Option<f64>, Map<String, Value>);
    /// False for sources that never report utilisation (Intel sysfs); their `None` is not a
    /// failure.
    fn reports_utilisation(&self) -> bool {
        true
    }
}

/// What backends need to know to serve the right adapter.
#[derive(Debug, Clone, Default)]
pub(crate) struct Ctx {
    /// `adapters::list_adapters()` at construction.
    pub list: Vec<adapters::Adapter>,
    /// `gpu_adapter` resolved (pinned, or auto = largest VRAM); `None` if nothing detected.
    pub target: Option<adapters::Adapter>,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub pinned: bool,
    /// Auto: a backend that cannot serve `target` declines, so the next one gets it.
    /// Explicit backend: it serves its own default instead.
    pub strict: bool,
}

impl Ctx {
    /// `strict` and the target is known and not of this vendor family.
    fn declines(&self, serves: impl Fn(&adapters::Adapter) -> bool) -> bool {
        self.strict && self.target.as_ref().is_some_and(|t| !serves(t))
    }
}

/// Constructor for one backend; `None` when it is not usable here.
type Probe = fn(&Ctx) -> Option<Box<dyn GpuSource>>;

pub struct GpuCollector {
    src: Box<dyn GpuSource>,
    /// Auto only: the reserve, used at most once (cleared after a switch).
    rest: VecDeque<Probe>,
    ctx: Ctx,
    /// Consecutive null reads from `src`.
    misses: u32,
    /// `src` has produced a value at least once; it is then never replaced.
    proven: bool,
    /// Last good value, repeated for up to `HOLD_TICKS` null reads.
    last: Option<f64>,
}

impl GpuCollector {
    /// Initialise the requested backend (or the first working one for `Auto`).
    /// `None` when no backend is usable or `GpuBackend::None` was requested.
    /// `adapter` is `metrics.gpu_adapter` (`auto`, `#N`, or part of a name).
    pub fn new(backend: GpuBackend, adapter: &str) -> Option<Self> {
        if backend == GpuBackend::None {
            return None;
        }
        let list = adapters::list_adapters();
        let (target, pinned) = adapters::resolve(&list, adapter);
        tracing::info!(
            detected = ?list.iter().map(adapters::Adapter::label).collect::<Vec<_>>(),
            target = ?target.as_ref().map(adapters::Adapter::label),
            pinned,
            "gpu adapters"
        );
        let verify = backend == GpuBackend::Auto;
        let ctx = Ctx {
            list,
            target,
            pinned,
            strict: verify,
        };
        let cands: Vec<Probe> = match backend {
            GpuBackend::Nvml => vec![try_nvml],
            GpuBackend::Amdgpu => vec![try_amdgpu],
            GpuBackend::Intel => vec![try_intel],
            GpuBackend::Pdh => vec![try_pdh],
            _ => vec![try_nvml, try_amdgpu, try_intel, try_pdh],
        };
        let c = Self::from_candidates(cands, verify, ctx)?;
        tracing::info!(backend = c.backend(), "gpu collector initialised");
        Some(c)
    }

    /// `verify`: Auto behaviour (check the first read, fall back at run time).
    fn from_candidates(cands: Vec<Probe>, verify: bool, ctx: Ctx) -> Option<Self> {
        let mut rest: VecDeque<Probe> = cands.into();
        let (src, proven) = if verify {
            pick(&mut rest, &ctx)?
        } else {
            let s = rest.pop_front().and_then(|p| p(&ctx))?;
            rest.clear();
            (s, false)
        };
        Some(Self {
            src,
            rest,
            ctx,
            misses: 0,
            proven,
            last: None,
        })
    }

    /// Name of the backend in use.
    pub fn backend(&self) -> &'static str {
        self.src.backend()
    }

    /// One read, with the Auto run-time fallback.
    fn read(&mut self) -> (Option<f64>, Map<String, Value>) {
        let (v, mut d) = self.src.read();
        // Name the backend that produced this reading (before a possible switch below).
        d.insert("backend".into(), Value::from(self.src.backend()));
        // Every backend shows what was detected (`detail.adapters`), even Intel sysfs.
        if !d.contains_key("adapters") && !self.ctx.list.is_empty() {
            let name = d.get("name").and_then(Value::as_str).map(str::to_string);
            let chosen = d
                .get("adapter_index")
                .and_then(Value::as_u64)
                .map(|i| i as usize)
                .or_else(|| {
                    let n = name?;
                    let found = self.ctx.list.iter().find(|a| a.name == n)?;
                    d.insert("adapter_index".into(), Value::from(found.index));
                    Some(found.index)
                });
            d.insert(
                "adapters".into(),
                adapters::adapters_detail(&self.ctx.list, chosen, |_| Map::new()),
            );
        }
        let v = v.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 100.0));
        if !self.src.reports_utilisation() {
            return (v, d);
        }
        if let Some(v) = v {
            self.misses = 0;
            self.proven = true;
            self.last = Some(v);
            return (Some(v), d);
        }
        self.misses += 1;
        let held = self.last.filter(|_| self.misses <= HOLD_TICKS);
        if !self.proven && self.misses >= MAX_MISSES && !self.rest.is_empty() {
            let from = self.src.backend();
            if let Some((next, proven)) = pick(&mut self.rest, &self.ctx) {
                tracing::info!(
                    from,
                    to = next.backend(),
                    "gpu: {from} never reported utilisation; switching (once)"
                );
                self.src = next;
                self.proven = proven;
                self.last = None;
            }
            self.rest.clear();
            self.misses = 0;
        }
        (held, d)
    }
}

/// Take candidates from the front of `rest` until one initialises and (if it reports
/// utilisation) returns a value on its first read; returns it with `true`. If none does,
/// return the first one that initialised with `false`, and leave in `rest` the other probes
/// that initialised (the reserve for a run-time switch).
fn pick(rest: &mut VecDeque<Probe>, ctx: &Ctx) -> Option<(Box<dyn GpuSource>, bool)> {
    let mut first = None;
    let mut reserve = VecDeque::new();
    while let Some(probe) = rest.pop_front() {
        let Some(mut s) = probe(ctx) else { continue };
        if !s.reports_utilisation() {
            return Some((s, false));
        }
        if s.read().0.is_some_and(f64::is_finite) {
            return Some((s, true));
        }
        tracing::info!(
            backend = s.backend(),
            "gpu: backend initialised but returned no utilisation; trying the next one"
        );
        if first.is_none() {
            first = Some(s);
        } else {
            reserve.push_back(probe);
        }
    }
    *rest = reserve;
    first.map(|s| (s, false))
}

impl Collector for GpuCollector {
    fn id(&self) -> &str {
        "gpu"
    }

    fn sample(&mut self, _dt: Duration) -> Option<Metric> {
        let (v, d) = self.read();
        let name = d.get("name").and_then(Value::as_str).map(str::to_string);
        let index = d
            .get("adapter_index")
            .and_then(Value::as_u64)
            .map(|i| i as usize);
        adapters::set_current(name.map(|n| (index, n)));
        Some(Metric::percent("GPU", v).with_detail(d))
    }
}

fn try_nvml(ctx: &Ctx) -> Option<Box<dyn GpuSource>> {
    if ctx.declines(adapters::Adapter::is_nvidia) {
        tracing::debug!("gpu: target is not an NVIDIA GPU; nvml skipped");
        return None;
    }
    #[cfg(feature = "nvml")]
    {
        match nvml::NvmlSource::new(ctx) {
            Ok(s) => return Some(Box::new(s)),
            Err(e) => tracing::debug!("gpu: nvml unavailable: {e}"),
        }
    }
    #[cfg(not(feature = "nvml"))]
    tracing::debug!("gpu: nvml support not compiled in");
    None
}

fn try_amdgpu(ctx: &Ctx) -> Option<Box<dyn GpuSource>> {
    if ctx.declines(|t| t.vendor == Some(sysfs::VENDOR_AMD)) {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(s) = amdgpu::AmdGpu::probe(std::path::Path::new(sysfs::DRM_ROOT), ctx) {
            return Some(Box::new(s));
        }
        tracing::debug!("gpu: no amdgpu with gpu_busy_percent found");
    }
    None
}

fn try_intel(ctx: &Ctx) -> Option<Box<dyn GpuSource>> {
    if ctx.declines(|t| t.vendor == Some(sysfs::VENDOR_INTEL)) {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(s) = intel::IntelGpu::probe(std::path::Path::new(sysfs::DRM_ROOT)) {
            return Some(Box::new(s));
        }
        tracing::debug!("gpu: no intel gpu frequency files found");
    }
    None
}

fn try_pdh(ctx: &Ctx) -> Option<Box<dyn GpuSource>> {
    #[cfg(windows)]
    {
        // PDH sees every adapter, so it can always serve the target.
        // A registry-fallback target has no LUID, so PDH cannot match it: use the heuristics.
        let target = ctx.target.clone().filter(|t| t.id.starts_with("0x"));
        match pdh::PdhSource::new(ctx.list.clone(), target, ctx.pinned) {
            Ok(s) => return Some(Box::new(s)),
            Err(e) => tracing::debug!("gpu: pdh unavailable: {e}"),
        }
    }
    let _ = ctx;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns `script` values in order, then `then` forever.
    struct Fake {
        name: &'static str,
        script: VecDeque<Option<f64>>,
        then: Option<f64>,
        util: bool,
    }

    impl GpuSource for Fake {
        fn backend(&self) -> &'static str {
            self.name
        }
        fn read(&mut self) -> (Option<f64>, Map<String, Value>) {
            (self.script.pop_front().unwrap_or(self.then), Map::new())
        }
        fn reports_utilisation(&self) -> bool {
            self.util
        }
    }

    fn fake(name: &'static str, script: &[Option<f64>], then: Option<f64>) -> Box<dyn GpuSource> {
        Box::new(Fake {
            name,
            script: script.iter().copied().collect(),
            then,
            util: true,
        })
    }

    /// NVML-like: initialises, every read fails.
    fn dead(_: &Ctx) -> Option<Box<dyn GpuSource>> {
        Some(fake("dead", &[], None))
    }
    /// Works for the first read (the probe), then breaks.
    fn breaks(_: &Ctx) -> Option<Box<dyn GpuSource>> {
        Some(fake("breaks", &[Some(10.0)], None))
    }
    fn good(_: &Ctx) -> Option<Box<dyn GpuSource>> {
        Some(fake("good", &[], Some(42.0)))
    }
    fn absent(_: &Ctx) -> Option<Box<dyn GpuSource>> {
        None
    }
    fn freq_only(_: &Ctx) -> Option<Box<dyn GpuSource>> {
        Some(Box::new(Fake {
            name: "freq",
            script: VecDeque::new(),
            then: None,
            util: false,
        }))
    }

    fn sample(c: &mut GpuCollector) -> (Option<f64>, String) {
        let m = c.sample(Duration::from_secs(1)).unwrap();
        let v = serde_json::to_value(&m).unwrap();
        let backend = v["detail"]["backend"].as_str().unwrap().to_string();
        (v["v"].as_f64(), backend)
    }

    /// First read (the probe) null, then 42: a backend that needs a warm-up tick.
    fn late(_: &Ctx) -> Option<Box<dyn GpuSource>> {
        Some(fake("late", &[None], Some(42.0)))
    }

    #[test]
    fn auto_skips_backend_whose_first_read_fails() {
        let mut c =
            GpuCollector::from_candidates(vec![absent, dead, good], true, Ctx::default()).unwrap();
        assert_eq!(c.backend(), "good");
        assert_eq!(sample(&mut c), (Some(42.0), "good".into()));
    }

    #[test]
    fn auto_keeps_first_initialised_if_none_reads() {
        let c = GpuCollector::from_candidates(vec![dead, absent], true, Ctx::default()).unwrap();
        assert_eq!(c.backend(), "dead");
        assert!(GpuCollector::from_candidates(vec![absent], true, Ctx::default()).is_none());
    }

    #[test]
    fn unproven_backend_is_replaced_once_by_the_reserve() {
        // NVML-like `dead` and PDH-like `late` both fail their first read: `dead` is kept,
        // `late` is the reserve.
        let mut c = GpuCollector::from_candidates(vec![dead, late], true, Ctx::default()).unwrap();
        assert_eq!(c.backend(), "dead");
        for _ in 0..MAX_MISSES {
            assert_eq!(sample(&mut c), (None, "dead".into()));
        }
        assert_eq!(c.backend(), "late");
        // Its null warm-up read was consumed when it was picked; it reports right away.
        assert_eq!(sample(&mut c), (Some(42.0), "late".into()));
    }

    #[test]
    fn switch_happens_at_most_once() {
        fn dead2(_: &Ctx) -> Option<Box<dyn GpuSource>> {
            Some(fake("dead2", &[], None))
        }
        let c =
            GpuCollector::from_candidates(vec![dead, dead2, good], true, Ctx::default()).unwrap();
        assert_eq!(c.backend(), "good", "a working candidate wins at start");
        let mut c = GpuCollector::from_candidates(vec![dead, dead2], true, Ctx::default()).unwrap();
        for _ in 0..(10 * MAX_MISSES) {
            sample(&mut c);
        }
        assert_eq!(c.backend(), "dead2", "switched once and never back");
    }

    #[test]
    fn proven_backend_never_switches_and_holds_values() {
        let mut c =
            GpuCollector::from_candidates(vec![breaks, good], true, Ctx::default()).unwrap();
        assert_eq!(c.backend(), "breaks");
        // `breaks` produced 10.0 on its probe read and then only nulls. No first value has
        // reached the collector yet, so there is nothing to hold.
        for _ in 0..(3 * MAX_MISSES) {
            assert_eq!(sample(&mut c), (None, "breaks".into()));
        }
        assert_eq!(c.backend(), "breaks", "proven at the probe: never replaced");
    }

    #[test]
    fn null_reads_hold_the_last_value_for_three_ticks() {
        fn blip(_: &Ctx) -> Option<Box<dyn GpuSource>> {
            // probe, 0, then a 2-tick gap, 0, then a long gap
            Some(fake(
                "blip",
                &[
                    Some(0.0),
                    Some(0.0),
                    None,
                    None,
                    Some(7.0),
                    None,
                    None,
                    None,
                    None,
                ],
                None,
            ))
        }
        let mut c = GpuCollector::from_candidates(vec![blip], true, Ctx::default()).unwrap();
        let v: Vec<Option<f64>> = (0..9).map(|_| sample(&mut c).0).collect();
        assert_eq!(
            v,
            [
                Some(0.0),
                Some(0.0),
                Some(0.0),
                Some(7.0),
                Some(7.0),
                Some(7.0),
                Some(7.0),
                None,
                None
            ]
        );
    }

    #[test]
    fn alternating_zero_and_null_reports_zero() {
        fn flicker(_: &Ctx) -> Option<Box<dyn GpuSource>> {
            let mut s = vec![Some(0.0)];
            for _ in 0..20 {
                s.extend([Some(0.0), None]);
            }
            Some(fake("flicker", &s, Some(0.0)))
        }
        let mut c =
            GpuCollector::from_candidates(vec![flicker, good], true, Ctx::default()).unwrap();
        for _ in 0..40 {
            assert_eq!(sample(&mut c), (Some(0.0), "flicker".into()));
        }
    }

    #[test]
    fn explicit_backend_never_switches() {
        let mut c = GpuCollector::from_candidates(vec![dead, good], false, Ctx::default()).unwrap();
        for _ in 0..(2 * MAX_MISSES) {
            assert_eq!(sample(&mut c), (None, "dead".into()));
        }
        assert!(GpuCollector::from_candidates(vec![absent, good], false, Ctx::default()).is_none());
    }

    #[test]
    fn frequency_only_source_is_not_a_failure() {
        let mut c =
            GpuCollector::from_candidates(vec![freq_only, good], true, Ctx::default()).unwrap();
        for _ in 0..(2 * MAX_MISSES) {
            assert_eq!(sample(&mut c).0, None);
        }
        assert_eq!(c.backend(), "freq");
    }

    #[test]
    fn values_are_clamped() {
        fn hot(_: &Ctx) -> Option<Box<dyn GpuSource>> {
            Some(fake("hot", &[], Some(140.0)))
        }
        let mut c = GpuCollector::from_candidates(vec![hot], true, Ctx::default()).unwrap();
        assert_eq!(sample(&mut c).0, Some(100.0));
    }
}
