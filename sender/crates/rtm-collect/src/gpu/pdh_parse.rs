//! Pure helpers for the Windows PDH backend (`pdh.rs`), kept platform-independent so they are
//! unit tested everywhere.
//!
//! `\GPU Engine(*)\Utilization Percentage` instance names look like
//! `pid_1234_luid_0x00000000_0x0000C2B6_phys_0_eng_0_engtype_3D`: one instance per
//! (process, adapter, engine).
//!
//! Task Manager semantics: one engine is `(luid, phys, eng_N)`; its utilisation is the sum
//! over the processes using it (capped at 100). An adapter's load is its busiest engine, and
//! an engine type's load (`detail.engines`) is its busiest engine of that type. Two Copy
//! engines at 40 % are "Copy 40 %", not 80 %.

use super::adapters::is_auto;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};

/// Most engine types listed in `detail.engines`.
pub const MAX_ENGINES: usize = 6;

/// The engine type of an instance name (`3D`, `Copy`, `VideoDecode`, `Compute_0`, ...).
pub fn engine_type(instance: &str) -> Option<&str> {
    let (_, t) = instance.rsplit_once("engtype_")?;
    let t = t.trim();
    (!t.is_empty()).then_some(t)
}

fn usable(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

#[cfg(test)]
/// Sum of `engtype_3D` utilisation over all instances (all processes, all adapters), capped
/// at 100 and rounded to 0.1.
pub fn sum_3d(items: &[(String, f64)]) -> f64 {
    let s: f64 = items
        .iter()
        .filter(|(n, v)| engine_type(n) == Some("3D") && usable(*v))
        .map(|(_, v)| *v)
        .sum();
    round1(s.min(100.0))
}

/// One physical engine: `(luid, phys, eng, engtype)`.
type EngineKey<'a> = (String, Option<u32>, Option<u32>, &'a str);

/// Utilisation per individual engine, summed over processes and capped at 100. Instances
/// without a LUID or engine type are ignored.
fn per_engine(items: &[(String, f64)]) -> Vec<(EngineKey<'_>, f64)> {
    let mut out: Vec<(EngineKey<'_>, f64)> = Vec::new();
    for (n, v) in items {
        let Some(i) = parse_instance(n) else { continue };
        let Some(t) = i.engtype else { continue };
        let key = (i.luid, i.phys, i.eng, t);
        let v = if usable(*v) { *v } else { 0.0 };
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, s)) => *s += v,
            None => out.push((key, v)),
        }
    }
    for (_, s) in &mut out {
        *s = s.min(100.0);
    }
    out
}

/// Utilisation per engine type (its busiest engine, see the module doc), busiest first, at
/// most `MAX_ENGINES` entries, as a JSON object for `detail.engines`.
pub fn engines(items: &[(String, f64)]) -> Map<String, Value> {
    let mut by: Vec<(String, f64)> = Vec::new();
    for ((_, _, _, t), v) in per_engine(items) {
        match by.iter_mut().find(|(k, _)| k == t) {
            Some((_, s)) => *s = s.max(v),
            None => by.push((t.to_string(), v)),
        }
    }
    by.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    by.into_iter()
        .take(MAX_ENGINES)
        .map(|(k, v)| (k, Value::from(round1(v.min(100.0)))))
        .collect()
}

// PDH status codes (values from pdh.h; `pdh.rs` asserts they match the `windows` crate).
pub const PDH_NO_DATA: u32 = 0x8000_07D5;
pub const PDH_CSTATUS_NO_INSTANCE: u32 = 0x8000_07D1;
pub const PDH_INVALID_DATA: u32 = 0xC000_0BC6;
pub const PDH_CSTATUS_INVALID_DATA: u32 = 0xC000_0BBA;

/// What one tick of the `GPU Engine` wildcard counter produced.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineRead {
    /// Instances with valid values.
    Values(Vec<(String, f64)>),
    /// The query worked but there are no instances / no data: nothing ran on the GPU.
    NoInstances,
    /// Data not valid this tick, typically because the instance set changed since the last
    /// collect (GPU Engine instances are per process and come and go even at idle).
    Invalid,
    /// `PdhCollectQueryData` itself failed.
    QueryFailed,
}

/// Map a failing `PdhCollectQueryData` status.
pub fn classify_collect(status: u32) -> EngineRead {
    match status {
        PDH_NO_DATA | PDH_CSTATUS_NO_INSTANCE => EngineRead::NoInstances,
        _ => EngineRead::QueryFailed,
    }
}

/// Map a failing `PdhGetFormattedCounterArrayW` status (not success, not PDH_MORE_DATA).
pub fn classify_array(status: u32) -> EngineRead {
    match status {
        PDH_NO_DATA | PDH_CSTATUS_NO_INSTANCE => EngineRead::NoInstances,
        _ => EngineRead::Invalid,
    }
}

/// Consecutive outright query failures before `v` becomes null.
pub const MAX_QUERY_FAILURES: u32 = 3;

/// Turns per-tick `EngineRead`s into the reported utilisation: idle is 0.0, a tick with
/// invalid data repeats the previous value, and only `MAX_QUERY_FAILURES` failed queries in a
/// row give `None`.
#[derive(Debug, Default)]
pub struct UtilFilter {
    last: Option<f64>,
    failures: u32,
}

impl UtilFilter {
    /// `update_with` summing 3D over every adapter (tests).
    #[cfg(test)]
    pub fn update(&mut self, r: &EngineRead) -> Option<f64> {
        self.update_with(r, sum_3d)
    }

    /// `util` turns a tick's valid instances into the utilisation to report.
    pub fn update_with(
        &mut self,
        r: &EngineRead,
        util: impl FnOnce(&[(String, f64)]) -> f64,
    ) -> Option<f64> {
        let v = match r {
            EngineRead::Values(items) => util(items),
            EngineRead::NoInstances => 0.0,
            EngineRead::Invalid => self.last.unwrap_or(0.0),
            EngineRead::QueryFailed => {
                self.failures += 1;
                if self.failures >= MAX_QUERY_FAILURES {
                    return None;
                }
                return Some(self.last.unwrap_or(0.0));
            }
        };
        self.failures = 0;
        self.last = Some(v);
        Some(v)
    }
}

// ---------------------------------------------------------------------------------------------
// Adapters

/// The parts of a `GPU Engine` / `GPU Adapter Memory` instance name we use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance<'a> {
    /// Normalised adapter LUID, `0x<high>_0x<low>` in lowercase (see `luid_key`).
    pub luid: String,
    pub phys: Option<u32>,
    /// `eng_N`: the engine index on that physical adapter.
    pub eng: Option<u32>,
    pub engtype: Option<&'a str>,
}

/// `luid_0xHIGH_0xLOW` as it appears in instance names, lowercase.
pub fn luid_key(high: u32, low: u32) -> String {
    format!("0x{high:08x}_0x{low:08x}")
}

/// Parse `pid_1234_luid_0x00000000_0x0000C2B6_phys_0_eng_0_engtype_3D` (engine) or
/// `luid_0x00000000_0x0000C2B6_phys_0` (adapter memory). `None` without a LUID.
pub fn parse_instance(name: &str) -> Option<Instance<'_>> {
    let (_, rest) = name.split_once("luid_")?;
    let mut parts = rest.splitn(3, '_');
    let hex = |s: Option<&str>| {
        let s = s?;
        let h = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
        u32::from_str_radix(h, 16).ok()
    };
    let high = hex(parts.next())?;
    let low = hex(parts.next())?;
    let num_after = |tag: &str| {
        name.split_once(tag)
            .and_then(|(_, r)| r.split('_').next())
            .and_then(|n| n.parse().ok())
    };
    Some(Instance {
        luid: luid_key(high, low),
        phys: num_after("phys_"),
        eng: num_after("_eng_"),
        engtype: engine_type(name),
    })
}

/// Load and engine-instance count of one adapter in one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterLoad {
    pub luid: String,
    /// Task Manager semantics: the busiest engine (any type: 3D, Compute_0, Copy, ...), each
    /// engine summed over processes and capped at 100. Covers compute-only cards.
    pub util: f64,
    pub instances: usize,
}

/// Group engine instances by adapter LUID (sorted by LUID). Instances without a LUID are
/// ignored.
pub fn per_adapter(items: &[(String, f64)]) -> Vec<AdapterLoad> {
    let mut groups: Vec<(String, Vec<(String, f64)>)> = Vec::new();
    for (n, v) in items {
        let Some(inst) = parse_instance(n) else {
            continue;
        };
        match groups.iter_mut().find(|(l, _)| *l == inst.luid) {
            Some((_, g)) => g.push((n.clone(), *v)),
            None => groups.push((inst.luid, vec![(n.clone(), *v)])),
        }
    }
    let mut out: Vec<AdapterLoad> = groups
        .into_iter()
        .map(|(luid, g)| AdapterLoad {
            luid,
            util: busiest_engine(&g),
            instances: g.len(),
        })
        .collect();
    out.sort_by(|a, b| a.luid.cmp(&b.luid));
    out
}

/// Max over individual engines of the per-engine sum (each capped at 100), rounded to 0.1.
pub fn busiest_engine(items: &[(String, f64)]) -> f64 {
    engines(items)
        .values()
        .filter_map(Value::as_f64)
        .fold(0.0, f64::max)
}

/// The instances of one adapter.
pub fn of_adapter(items: &[(String, f64)], luid: &str) -> Vec<(String, f64)> {
    items
        .iter()
        .filter(|(n, _)| parse_instance(n).is_some_and(|i| i.luid == luid))
        .cloned()
        .collect()
}

/// `GPU Adapter Memory` values (bytes) summed per LUID.
pub fn per_luid(items: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for (n, v) in items {
        let Some(inst) = parse_instance(n) else {
            continue;
        };
        let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
        match out.iter_mut().find(|(l, _)| *l == inst.luid) {
            Some((_, s)) => *s += v,
            None => out.push((inst.luid, v)),
        }
    }
    out
}

/// A hardware adapter as DXGI describes it (Windows; empty elsewhere or if DXGI fails).
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterInfo {
    pub luid: String,
    pub name: String,
    /// Total dedicated video memory, bytes (0 for most integrated GPUs).
    pub dedicated_total: u64,
}

/// Which adapter `v` reports and why (`detail.selected_by`).
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub luid: String,
    pub reason: &'static str,
}

/// Does `pin` name this adapter (LUID, its low part, or part of its name)?
pub fn pin_matches(pin: &str, luid: &str, name: Option<&str>) -> bool {
    let p = pin.trim().to_ascii_lowercase();
    if p.is_empty() {
        return false;
    }
    let low = luid.rsplit('_').next().unwrap_or(luid);
    luid == p
        || low == p
        || luid.replace("_", "") == p.replace("_", "")
        || name.is_some_and(|n| n.to_ascii_lowercase().contains(&p))
}

/// Choose the adapter `v` reports:
/// 1. `pin`, if not auto and it matches an adapter;
/// 2. the adapter with the most total dedicated memory per DXGI (the discrete GPU);
/// 3. the adapter with the highest `Dedicated Usage` (`mem`);
/// 4. the adapter with the most engine instances;
/// 5. the busiest adapter.
pub fn select_adapter(
    loads: &[AdapterLoad],
    mem: &[(String, f64)],
    infos: &[AdapterInfo],
    pin: &str,
) -> Option<Selection> {
    let mut luids: Vec<&str> = loads.iter().map(|a| a.luid.as_str()).collect();
    for i in infos {
        if !luids.contains(&i.luid.as_str()) {
            luids.push(&i.luid);
        }
    }
    let name_of = |l: &str| infos.iter().find(|i| i.luid == l).map(|i| i.name.as_str());
    let pick = |l: &str, reason| {
        Some(Selection {
            luid: l.to_string(),
            reason,
        })
    };
    if !is_auto(pin) {
        if let Some(l) = luids.iter().find(|l| pin_matches(pin, l, name_of(l))) {
            return pick(l, "pinned");
        }
    }
    if let Some(i) = infos
        .iter()
        .filter(|i| i.dedicated_total > 0)
        .max_by_key(|i| i.dedicated_total)
    {
        return pick(&i.luid, "dedicated_memory");
    }
    if let Some((l, _)) = mem
        .iter()
        .filter(|(_, b)| *b > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
    {
        return pick(l, "dedicated_usage");
    }
    let most = loads.iter().map(|a| a.instances).max().unwrap_or(0);
    let with_most: Vec<&AdapterLoad> = loads.iter().filter(|a| a.instances == most).collect();
    if most > 0 && with_most.len() == 1 {
        return pick(&with_most[0].luid, "engine_count");
    }
    loads
        .iter()
        .max_by(|a, b| a.util.total_cmp(&b.util))
        .and_then(|a| pick(&a.luid, "busiest"))
        .or_else(|| luids.first().and_then(|l| pick(l, "only")))
}

/// The target LUID must be missing for at least this long ...
pub const LUID_LOST_AFTER: Duration = Duration::from_secs(5);
/// ... and for at least this many ticks in a row before the adapters are enumerated again.
pub const LUID_LOST_TICKS: u32 = 2;
/// After an enumeration that found the target unchanged, wait this long before the next.
pub const REENUMERATE_COOLDOWN: Duration = Duration::from_secs(60);

/// Notices that the reported adapter's LUID has gone from the counter data (a driver reset or
/// update gives the adapter a new LUID) so the adapter list can be enumerated again; without
/// this the old LUID would read as a valid-looking 0 % forever.
#[derive(Debug, Default)]
pub struct LuidWatch {
    missing_since: Option<Instant>,
    ticks: u32,
    quiet_until: Option<Instant>,
}

impl LuidWatch {
    /// `seen`: the LUIDs present in this tick's counter data (engines and adapter memory).
    /// True when the target has been absent for `LUID_LOST_AFTER` and `LUID_LOST_TICKS`
    /// ticks while other LUIDs were present: enumerate the adapters again.
    pub fn observe(&mut self, now: Instant, target: &str, seen: &[&str]) -> bool {
        if seen.is_empty() || seen.contains(&target) {
            self.missing_since = None;
            self.ticks = 0;
            return false;
        }
        if self.quiet_until.is_some_and(|q| now < q) {
            return false;
        }
        let since = *self.missing_since.get_or_insert(now);
        self.ticks += 1;
        if self.ticks >= LUID_LOST_TICKS && now.saturating_duration_since(since) >= LUID_LOST_AFTER
        {
            self.missing_since = None;
            self.ticks = 0;
            return true;
        }
        false
    }

    /// Report the outcome of an enumeration: `changed` false (same target LUID) waits
    /// `REENUMERATE_COOLDOWN` before trying again.
    pub fn enumerated(&mut self, now: Instant, changed: bool) {
        self.quiet_until = (!changed).then(|| now + REENUMERATE_COOLDOWN);
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(n: &str, v: f64) -> (String, f64) {
        (n.to_string(), v)
    }

    const P: &str = "pid_1234_luid_0x00000000_0x0000C2B6_phys_0_eng_";

    #[test]
    fn engine_names() {
        assert_eq!(engine_type(&format!("{P}0_engtype_3D")), Some("3D"));
        assert_eq!(
            engine_type(&format!("{P}5_engtype_VideoDecode")),
            Some("VideoDecode")
        );
        assert_eq!(
            engine_type(&format!("{P}9_engtype_Compute_0")),
            Some("Compute_0")
        );
        assert_eq!(engine_type(&format!("{P}9_engtype_")), None);
        assert_eq!(engine_type("_Total"), None);
    }

    #[test]
    fn sums_only_3d_and_caps() {
        let items = vec![
            it(&format!("{P}0_engtype_3D"), 20.04),
            it("pid_99_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 15.0),
            it(&format!("{P}1_engtype_Copy"), 50.0),
            it(&format!("{P}2_engtype_3D_extra"), 30.0), // not exactly 3D
            it(&format!("{P}0_engtype_3D"), f64::NAN),
            it(&format!("{P}0_engtype_3D"), -3.0),
        ];
        assert_eq!(sum_3d(&items), 35.0);
        let many = vec![it(&format!("{P}0_engtype_3D"), 70.0); 3];
        assert_eq!(sum_3d(&many), 100.0);
        assert_eq!(sum_3d(&[]), 0.0);
    }

    #[test]
    fn idle_and_churn_never_null() {
        let mut f = UtilFilter::default();
        let busy = EngineRead::Values(vec![it(&format!("{P}0_engtype_3D"), 12.0)]);
        // Idle start: no instances -> 0, not null.
        assert_eq!(f.update(&EngineRead::NoInstances), Some(0.0));
        assert_eq!(f.update(&busy), Some(12.0));
        // Instance churn: repeat the previous value.
        assert_eq!(f.update(&EngineRead::Invalid), Some(12.0));
        assert_eq!(f.update(&EngineRead::Values(vec![])), Some(0.0));
        assert_eq!(f.update(&EngineRead::Invalid), Some(0.0));
        // Alternating idle/churn (the reported 0 / "-" flicker) stays at 0.
        for _ in 0..10 {
            assert_eq!(f.update(&EngineRead::NoInstances), Some(0.0));
            assert_eq!(f.update(&EngineRead::Invalid), Some(0.0));
        }
    }

    #[test]
    fn null_only_after_three_failed_queries() {
        let mut f = UtilFilter::default();
        f.update(&EngineRead::Values(vec![it(
            &format!("{P}0_engtype_3D"),
            5.0,
        )]));
        assert_eq!(f.update(&EngineRead::QueryFailed), Some(5.0));
        assert_eq!(f.update(&EngineRead::QueryFailed), Some(5.0));
        assert_eq!(f.update(&EngineRead::QueryFailed), None);
        assert_eq!(f.update(&EngineRead::QueryFailed), None);
        // Recovery resets the count.
        assert_eq!(f.update(&EngineRead::NoInstances), Some(0.0));
        assert_eq!(f.update(&EngineRead::QueryFailed), Some(0.0));
        assert_eq!(f.update(&EngineRead::Invalid), Some(0.0));
        assert_eq!(f.update(&EngineRead::QueryFailed), Some(0.0));
    }

    #[test]
    fn status_classification() {
        assert_eq!(classify_collect(PDH_NO_DATA), EngineRead::NoInstances);
        assert_eq!(classify_collect(0xC000_0BB8), EngineRead::QueryFailed);
        assert_eq!(
            classify_array(PDH_CSTATUS_NO_INSTANCE),
            EngineRead::NoInstances
        );
        assert_eq!(classify_array(PDH_INVALID_DATA), EngineRead::Invalid);
        assert_eq!(
            classify_array(PDH_CSTATUS_INVALID_DATA),
            EngineRead::Invalid
        );
        assert_eq!(classify_array(0xDEAD_BEEF), EngineRead::Invalid);
    }

    const IGPU: &str = "0x00000000_0x0000c2b6";
    const DGPU: &str = "0x00000000_0x0001d3a4";

    fn eng(luid_hi_lo: &str, pid: u32, engtype: &str, v: f64) -> (String, f64) {
        let (h, l) = luid_hi_lo.split_once('_').unwrap();
        (
            format!(
                "pid_{pid}_luid_{}_{}_phys_0_eng_0_engtype_{engtype}",
                h,
                l.to_uppercase().replace("0X", "0x")
            ),
            v,
        )
    }

    #[test]
    fn compute_only_load_counts() {
        // AI workload on the R9700: no 3D at all, compute queues from several processes.
        let items = vec![
            eng(DGPU, 1, "Compute_0", 60.0),
            eng(DGPU, 2, "Compute_0", 35.5),
            eng(DGPU, 2, "Compute_1", 20.0),
            eng(DGPU, 2, "3D", 0.0),
            eng(DGPU, 3, "Copy", 70.0),
        ];
        assert_eq!(busiest_engine(&items), 95.5);
        assert_eq!(per_adapter(&items)[0].util, 95.5);
        let more = [items.clone(), vec![eng(DGPU, 4, "Compute_0", 30.0)]].concat();
        assert_eq!(busiest_engine(&more), 100.0, "per-type sum is capped");
        assert_eq!(busiest_engine(&[]), 0.0);
    }

    #[test]
    fn target_adapter_reports_its_own_busiest_engine() {
        // iGPU busy with the desktop (3D 40), R9700 running an AI job (Compute_0 88).
        let items = vec![
            eng(IGPU, 10, "3D", 40.0),
            eng(IGPU, 11, "VideoDecode", 5.0),
            eng(DGPU, 20, "Compute_0", 88.0),
            eng(DGPU, 20, "3D", 0.0),
        ];
        let loads = per_adapter(&items);
        let util = |l: &str| loads.iter().find(|a| a.luid == l).unwrap().util;
        assert_eq!(util(DGPU), 88.0);
        assert_eq!(util(IGPU), 40.0);
        let e = engines(&of_adapter(&items, DGPU));
        assert_eq!(e.keys().next().map(String::as_str), Some("Compute_0"));
        assert!(
            !e.contains_key("VideoDecode"),
            "other adapter's engines are excluded"
        );
    }

    #[test]
    fn instance_names() {
        let i =
            parse_instance("pid_1234_luid_0x00000000_0x0000C2B6_phys_0_eng_3_engtype_3D").unwrap();
        assert_eq!(i.luid, IGPU);
        assert_eq!(i.phys, Some(0));
        assert_eq!(i.eng, Some(3));
        assert_eq!(i.engtype, Some("3D"));
        let m = parse_instance("luid_0x00000000_0x0001D3A4_phys_1").unwrap();
        assert_eq!(
            (m.luid.as_str(), m.phys, m.eng, m.engtype),
            (DGPU, Some(1), None, None)
        );
        let c = parse_instance("pid_9_luid_0x00000001_0xFFFFFFFF_phys_0_eng_9_engtype_Compute_1")
            .unwrap();
        assert_eq!(c.luid, "0x00000001_0xffffffff");
        assert_eq!(c.engtype, Some("Compute_1"));
        assert_eq!(parse_instance("_Total"), None);
        assert_eq!(parse_instance("pid_1_luid_0xZZ_0x1_phys_0"), None);
        assert_eq!(luid_key(0, 0xC2B6), IGPU);
    }

    #[test]
    fn grouped_by_adapter() {
        let items = vec![
            eng(IGPU, 1, "3D", 30.0),
            eng(IGPU, 2, "3D", 20.0),
            eng(IGPU, 2, "Copy", 90.0),
            eng(DGPU, 1, "3D", 5.0),
        ];
        let a = per_adapter(&items);
        assert_eq!(
            a,
            [
                AdapterLoad {
                    luid: IGPU.into(),
                    util: 90.0, // 3D 30+20 = 50, Copy 90: busiest engine
                    instances: 3
                },
                AdapterLoad {
                    luid: DGPU.into(),
                    util: 5.0,
                    instances: 1
                },
            ]
        );
        assert_eq!(of_adapter(&items, DGPU).len(), 1);
        let mem = per_luid(&[
            ("luid_0x00000000_0x0001D3A4_phys_1".into(), 3.0),
            ("luid_0x00000000_0x0001D3A4_phys_1".into(), 2.0),
        ]);
        assert_eq!(mem, [(DGPU.to_string(), 5.0)]);
    }

    fn engine_n(luid_hi_lo: &str, pid: u32, n: u32, engtype: &str, v: f64) -> (String, f64) {
        let (name, v) = eng(luid_hi_lo, pid, engtype, v);
        (name.replace("_eng_0_", &format!("_eng_{n}_")), v)
    }

    #[test]
    fn engines_of_one_type_are_not_added_together() {
        // Two Copy engines at 40 % each: Copy is 40 %, not 80 %.
        let items = vec![
            engine_n(DGPU, 1, 1, "Copy", 40.0),
            engine_n(DGPU, 2, 2, "Copy", 40.0),
            engine_n(DGPU, 1, 0, "3D", 25.0),
            engine_n(DGPU, 2, 0, "3D", 10.0), // same engine, other process: summed
        ];
        assert_eq!(busiest_engine(&items), 40.0);
        assert_eq!(per_adapter(&items)[0].util, 40.0);
        let e = engines(&items);
        assert_eq!(e["Copy"], Value::from(40.0));
        assert_eq!(e["3D"], Value::from(35.0));
        // The same engine index on another physical adapter of a linked pair is another engine.
        let linked = vec![
            engine_n(DGPU, 1, 0, "3D", 60.0),
            (
                engine_n(DGPU, 2, 0, "3D", 60.0)
                    .0
                    .replace("phys_0", "phys_1"),
                60.0,
            ),
        ];
        assert_eq!(busiest_engine(&linked), 60.0);
        // Engines on different adapters never add up either.
        let two = vec![eng(IGPU, 1, "3D", 50.0), eng(DGPU, 1, "3D", 50.0)];
        assert_eq!(engines(&two)["3D"], Value::from(50.0));
    }

    #[test]
    fn luid_watch_needs_time_ticks_and_other_adapters() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut w = LuidWatch::default();
        // Target present, or no data at all: never.
        assert!(!w.observe(s(0), DGPU, &[DGPU, IGPU]));
        for i in 0..20 {
            assert!(!w.observe(s(i), DGPU, &[]));
        }
        // Missing while another adapter is present: after 5 s and 2 ticks.
        assert!(!w.observe(s(100), DGPU, &[IGPU]));
        assert!(!w.observe(s(103), DGPU, &[IGPU]));
        assert!(w.observe(s(105), DGPU, &[IGPU]));
        // Enumeration found the same LUID (e.g. an idle card): quiet for a minute.
        w.enumerated(s(105), false);
        for i in 106..165 {
            assert!(!w.observe(s(i), DGPU, &[IGPU]));
        }
        assert!(!w.observe(s(165), DGPU, &[IGPU]));
        assert!(w.observe(s(170), DGPU, &[IGPU]));
        // Seeing the target again resets the count; a long single tick is not enough.
        w.enumerated(s(170), true);
        assert!(!w.observe(s(171), DGPU, &[DGPU]));
        assert!(!w.observe(s(200), DGPU, &[IGPU]));
        assert!(w.observe(s(3600), DGPU, &[IGPU]));
    }

    fn load(luid: &str, util: f64, n: usize) -> AdapterLoad {
        AdapterLoad {
            luid: luid.into(),
            util,
            instances: n,
        }
    }
    fn info(luid: &str, name: &str, mb: u64) -> AdapterInfo {
        AdapterInfo {
            luid: luid.into(),
            name: name.into(),
            dedicated_total: mb << 20,
        }
    }

    #[test]
    fn selection_prefers_the_discrete_gpu() {
        // Hybrid laptop: the iGPU is busier and has more instances; DXGI says the NVIDIA
        // adapter has 6 GB dedicated, the Intel one 128 MB.
        let loads = [load(IGPU, 40.0, 30), load(DGPU, 0.0, 4)];
        let infos = [
            info(IGPU, "Intel(R) UHD Graphics", 128),
            info(DGPU, "NVIDIA GeForce RTX 3060 Laptop GPU", 6144),
        ];
        let s = select_adapter(&loads, &[], &infos, "auto").unwrap();
        assert_eq!((s.luid.as_str(), s.reason), (DGPU, "dedicated_memory"));
        // Pinned by name, by LUID, by low part.
        let s = select_adapter(&loads, &[], &infos, "intel").unwrap();
        assert_eq!((s.luid.as_str(), s.reason), (IGPU, "pinned"));
        assert_eq!(
            select_adapter(&loads, &[], &infos, "0x0000C2B6")
                .unwrap()
                .luid,
            IGPU
        );
        assert_eq!(
            select_adapter(&loads, &[], &infos, IGPU).unwrap().luid,
            IGPU
        );
        // A pin that matches nothing falls back to auto.
        assert_eq!(
            select_adapter(&loads, &[], &infos, "radeon").unwrap().luid,
            DGPU
        );
    }

    #[test]
    fn selection_fallbacks_without_dxgi() {
        let loads = [load(IGPU, 40.0, 30), load(DGPU, 1.0, 4)];
        let mem = [(IGPU.to_string(), 0.0), (DGPU.to_string(), 300e6)];
        assert_eq!(
            select_adapter(&loads, &mem, &[], "auto").unwrap(),
            Selection {
                luid: DGPU.into(),
                reason: "dedicated_usage"
            }
        );
        assert_eq!(
            select_adapter(&loads, &[], &[], "auto").unwrap(),
            Selection {
                luid: IGPU.into(),
                reason: "engine_count"
            }
        );
        let tie = [load(IGPU, 10.0, 4), load(DGPU, 25.0, 4)];
        assert_eq!(
            select_adapter(&tie, &[], &[], "auto").unwrap(),
            Selection {
                luid: DGPU.into(),
                reason: "busiest"
            }
        );
        assert_eq!(select_adapter(&[], &[], &[], "auto"), None);
        // Name pins need DXGI names; a LUID pin works without.
        assert_eq!(
            select_adapter(&loads, &[], &[], "nvidia").unwrap().reason,
            "engine_count"
        );
        assert_eq!(select_adapter(&loads, &[], &[], DGPU).unwrap().luid, DGPU);
    }

    #[test]
    fn engines_grouped_sorted_capped() {
        let mut items = vec![
            it(&format!("{P}0_engtype_3D"), 10.0),
            it(&format!("{P}0_engtype_3D"), 5.0),
            it(&format!("{P}1_engtype_Copy"), 80.0),
            it(&format!("{P}1_engtype_Copy"), 80.0),
        ];
        for t in ["A", "B", "C", "D", "E", "F"] {
            items.push(it(&format!("{P}9_engtype_{t}"), 0.0));
        }
        let e = engines(&items);
        assert_eq!(e.len(), MAX_ENGINES);
        let keys: Vec<&str> = e.keys().map(String::as_str).collect();
        assert!(keys.contains(&"Copy") && keys.contains(&"3D"), "{keys:?}");
        assert_eq!(e["Copy"], Value::from(100.0));
        assert_eq!(e["3D"], Value::from(15.0));
    }
}
