//! Windows backend via Performance Data Helper counters. Vendor-neutral (Intel/AMD/NVIDIA)
//! and needs no admin rights; it is what Task Manager's GPU graph is built on.
//!
//! * `\GPU Engine(*)\Utilization Percentage`: one instance per (process, adapter, engine).
//!   Instances are grouped by the adapter LUID in their name. Per adapter, `v` is the busiest
//!   engine type (each type summed over processes, capped at 100): Task Manager's semantics,
//!   so compute-only work (AI on a card that drives no display) counts.
//! * `\GPU Adapter Memory(*)\Dedicated Usage`: bytes per adapter -> `vram_used_mb`.
//!
//! The reported adapter is `gpu_adapter` if set, else the one with the most dedicated VRAM
//! (DXGI, see `adapters`); without a DXGI list, `pdh_parse::select_adapter`'s heuristics.
//! `detail.name`, `adapter_index`, `adapter` (LUID), `selected_by` and `adapters` (every
//! adapter with its load) say which one was chosen and why.
//!
//! These are rate counters: PDH needs two `PdhCollectQueryData` calls some time apart before a
//! formatted value is valid. The constructor collects twice, `WARMUP` apart, so the very first
//! `read` already has a value.
//!
//! Idle is 0.0, never null: no instances / no data is 0, a tick whose data is invalid
//! (engine instances are per process and change between collects even at idle) repeats the
//! previous value, and only 3 failed collects in a row give `v: null` (`pdh_parse::UtilFilter`).
//!
//! `detail.engines` lists utilisation per engine type (at most 6, busiest first) so a user
//! whose GPU work is not on the 3D engine (e.g. video decode) can see where it is.

use super::adapters::{adapters_detail, Adapter};
use super::pdh_parse::{
    self, classify_array, classify_collect, engines, of_adapter, per_adapter, per_luid,
    select_adapter, AdapterInfo, AdapterLoad, EngineRead, UtilFilter,
};
use super::GpuSource;
use crate::util::MIB;
use serde_json::{Map, Value};
use windows::core::{w, PCWSTR};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};

const ERROR_SUCCESS: u32 = 0;
// The platform-independent copies in `pdh_parse` must match the SDK values.
const _: () = assert!(pdh_parse::PDH_NO_DATA == windows::Win32::System::Performance::PDH_NO_DATA);
const _: () = assert!(
    pdh_parse::PDH_CSTATUS_NO_INSTANCE
        == windows::Win32::System::Performance::PDH_CSTATUS_NO_INSTANCE
);
const _: () =
    assert!(pdh_parse::PDH_INVALID_DATA == windows::Win32::System::Performance::PDH_INVALID_DATA);
const _: () = assert!(
    pdh_parse::PDH_CSTATUS_INVALID_DATA
        == windows::Win32::System::Performance::PDH_CSTATUS_INVALID_DATA
);
/// Gap between the two warm-up collects in `new`.
const WARMUP: std::time::Duration = std::time::Duration::from_millis(100);

pub struct PdhSource {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    vram: Option<PDH_HCOUNTER>,
    util: UtilFilter,
    /// Detected adapters (DXGI order) and the one to report, if known up front.
    list: Vec<Adapter>,
    target: Option<Adapter>,
    pinned: bool,
    /// Last chosen adapter LUID and reason, kept for ticks without valid instances.
    chosen: Option<(String, &'static str)>,
    loads: Vec<AdapterLoad>,
}

// SAFETY: PDH query and counter handles are opaque tokens owned exclusively by this struct;
// PDH does not tie them to the creating thread. The collector is used from one thread at a
// time (`&mut self`), so moving it between threads is sound.
unsafe impl Send for PdhSource {}

impl PdhSource {
    pub fn new(list: Vec<Adapter>, target: Option<Adapter>, pinned: bool) -> Result<Self, String> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            let st = PdhOpenQueryW(PCWSTR::null(), 0, &mut query);
            if st != ERROR_SUCCESS {
                return Err(format!("PdhOpenQueryW failed: 0x{st:08x}"));
            }
            let mut engine = PDH_HCOUNTER::default();
            let st = PdhAddEnglishCounterW(
                query,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut engine,
            );
            if st != ERROR_SUCCESS {
                PdhCloseQuery(query);
                return Err(format!("GPU Engine counter unavailable: 0x{st:08x}"));
            }
            let mut vram_c = PDH_HCOUNTER::default();
            let vram = (PdhAddEnglishCounterW(
                query,
                w!("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                0,
                &mut vram_c,
            ) == ERROR_SUCCESS)
                .then_some(vram_c);
            // Warm-up: two collects so the first read() computes a rate over a real interval.
            for i in 0..2 {
                if i == 1 {
                    std::thread::sleep(WARMUP);
                }
                let st = PdhCollectQueryData(query);
                if st != ERROR_SUCCESS {
                    PdhCloseQuery(query);
                    return Err(format!("PdhCollectQueryData failed: 0x{st:08x}"));
                }
            }
            Ok(Self {
                query,
                engine,
                vram,
                util: UtilFilter::default(),
                list,
                target,
                pinned,
                chosen: None,
                loads: Vec::new(),
            })
        }
    }
}

impl Drop for PdhSource {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// `(instance name, value)` for every instance of a wildcard counter with valid data, or why
/// there is none this tick (see `EngineRead`).
fn counter_array(counter: PDH_HCOUNTER) -> EngineRead {
    // Instances can appear between the size query and the fetch; retry a few times.
    for _ in 0..3 {
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        let st = unsafe {
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
        };
        if st == ERROR_SUCCESS {
            return EngineRead::NoInstances;
        }
        if st != PDH_MORE_DATA {
            return classify_array(st);
        }
        // `size` is in bytes and covers the item array plus the name strings after it.
        let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); (size as usize).div_ceil(item)];
        let st = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buf.as_mut_ptr()),
            )
        };
        if st == PDH_MORE_DATA {
            continue;
        }
        if st != ERROR_SUCCESS {
            return classify_array(st);
        }
        let n = (count as usize).min(buf.len());
        if n == 0 {
            return EngineRead::NoInstances;
        }
        let out: Vec<(String, f64)> = buf[..n]
            .iter()
            .filter(|it| {
                it.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA
                    || it.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA
            })
            .map(|it| {
                // SAFETY: szName points into `buf`, NUL-terminated, written by PDH; the value
                // union holds a double because we asked for PDH_FMT_DOUBLE.
                let name = unsafe { it.szName.to_string() }.unwrap_or_default();
                let v = unsafe { it.FmtValue.Anonymous.doubleValue };
                (name, v)
            })
            .collect();
        // Instances exist but none has a valid value: the set changed since the last collect.
        return if out.is_empty() {
            EngineRead::Invalid
        } else {
            EngineRead::Values(out)
        };
    }
    EngineRead::Invalid
}

impl GpuSource for PdhSource {
    fn backend(&self) -> &'static str {
        "windows-pdh"
    }

    fn read(&mut self) -> (Option<f64>, Map<String, Value>) {
        let mut d = Map::new();
        let st = unsafe { PdhCollectQueryData(self.query) };
        let read = if st == ERROR_SUCCESS {
            counter_array(self.engine)
        } else {
            classify_collect(st)
        };
        let mem = match self.vram.map(counter_array) {
            Some(EngineRead::Values(items)) => per_luid(&items),
            _ => Vec::new(),
        };
        if let EngineRead::Values(items) = &read {
            self.loads = per_adapter(items);
            self.chosen = match &self.target {
                Some(t) => Some((
                    t.id.clone(),
                    if self.pinned {
                        "gpu_adapter"
                    } else {
                        "largest_vram"
                    },
                )),
                None => {
                    let infos: Vec<AdapterInfo> = Vec::new();
                    select_adapter(&self.loads, &mem, &infos, "auto").map(|s| (s.luid, s.reason))
                }
            };
        } else if let (EngineRead::NoInstances, Some(t)) = (&read, &self.target) {
            self.loads.clear();
            self.chosen = Some((
                t.id.clone(),
                if self.pinned {
                    "gpu_adapter"
                } else {
                    "largest_vram"
                },
            ));
        }
        let luid = self.chosen.as_ref().map(|(l, _)| l.clone());
        let load_of = |l: &str| self.loads.iter().find(|a| a.luid == l).map(|a| a.util);
        let v = luid.as_deref().and_then(load_of).unwrap_or(0.0);
        let util = self.util.update_with(&read, |_| v);

        if let EngineRead::Values(items) = &read {
            let mine = luid
                .as_deref()
                .map(|l| of_adapter(items, l))
                .unwrap_or_default();
            d.insert("engines".into(), Value::Object(engines(&mine)));
        }
        let mem_of = |l: &str| mem.iter().find(|(m, _)| m == l).map(|(_, b)| *b);
        if let Some((l, reason)) = &self.chosen {
            let a = self.list.iter().find(|a| a.id == *l);
            d.insert(
                "name".into(),
                Value::from(a.map_or(l.clone(), |a| a.name.clone())),
            );
            if let Some(a) = a {
                d.insert("adapter_index".into(), Value::from(a.index));
                if let Some(t) = a.vram_total.filter(|&t| t > 0) {
                    d.insert("vram_total_mb".into(), Value::from(t / MIB));
                }
            }
            d.insert("adapter".into(), Value::from(l.clone()));
            d.insert("selected_by".into(), Value::from(*reason));
            if let Some(b) = mem_of(l) {
                d.insert("vram_used_mb".into(), Value::from((b / MIB as f64) as u64));
            }
        }
        let chosen_idx = luid
            .as_deref()
            .and_then(|l| self.list.iter().find(|a| a.id == l))
            .map(|a| a.index);
        let mut adapters = adapters_detail(&self.list, chosen_idx, |a| {
            let mut m = Map::new();
            m.insert("luid".into(), Value::from(a.id.clone()));
            m.insert("util".into(), Value::from(load_of(&a.id).unwrap_or(0.0)));
            if let Some(b) = mem_of(&a.id) {
                m.insert("vram_used_mb".into(), Value::from((b / MIB as f64) as u64));
            }
            m
        });
        // PDH adapters DXGI did not list (rare): keyed by LUID.
        if let Value::Object(o) = &mut adapters {
            for l in self
                .loads
                .iter()
                .filter(|l| !self.list.iter().any(|a| a.id == l.luid))
            {
                let mut m = Map::new();
                m.insert("util".into(), Value::from(l.util));
                m.insert(
                    "chosen".into(),
                    Value::from(luid.as_deref() == Some(l.luid.as_str())),
                );
                o.insert(l.luid.clone(), Value::Object(m));
            }
        }
        d.insert("adapters".into(), adapters);
        d.insert("backend".into(), Value::from(self.backend()));
        (util, d)
    }
}
