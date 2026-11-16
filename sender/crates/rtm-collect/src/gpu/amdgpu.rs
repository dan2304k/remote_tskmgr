//! amdgpu sysfs backend (Linux): `device/gpu_busy_percent`, `mem_info_vram_{used,total}`,
//! hwmon `temp1_input` (edge), and the active `pp_dpm_sclk` level for frequency.
//!
//! Reading these files resumes a runtime-suspended dGPU, and reading them every tick keeps
//! an idle one from suspending. So a card whose `power/runtime_status` is `suspended` (a
//! read that wakes nothing) is not read at all: it reports `v: 0.0` with
//! `detail.state = "asleep"`. A card that powers down when idle (`sysfs::can_sleep`) is,
//! once read idle, left alone for `sysfs::IDLE_RECHECK`. The other cards'
//! `detail.adapters` values are refreshed every [`OTHERS_EVERY`], skipping suspended ones.

use super::adapters::{adapters_detail, from_sysfs, Adapter};
use super::sysfs::{self, hwmon_temp_path, read_str, read_u64, Backoff};
use super::{Ctx, GpuSource};
use crate::util::{mib, num, MIB};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the cards that are not reported are read for `detail.adapters`.
pub const OTHERS_EVERY: Duration = Duration::from_secs(10);

pub struct AmdGpu {
    root: PathBuf,
    dev: PathBuf,
    adapter: Adapter,
    temp: Option<PathBuf>,
    /// Every GPU detected, for `detail.adapters`.
    list: Vec<Adapter>,
    /// `detail.adapters` extras of the other cards (by id) and when they were read.
    others: HashMap<String, Map<String, Value>>,
    others_at: Option<Instant>,
    backoff: Backoff,
    /// The last real read, repeated while `backoff` says not to read again.
    last: Option<(Option<f64>, Map<String, Value>)>,
}

impl AmdGpu {
    /// The `gpu_adapter` target if it is an AMD card exposing `gpu_busy_percent`, else (or
    /// with no target) the AMD card with the most VRAM (a discrete card beats an APU).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn probe(drm_root: &Path, ctx: &Ctx) -> Option<Self> {
        let list = if ctx.list.is_empty() {
            from_sysfs(drm_root)
        } else {
            ctx.list.clone()
        };
        let usable = |a: &&Adapter| {
            a.vendor == Some(sysfs::VENDOR_AMD)
                && read_u64(&drm_root.join(&a.id).join("device/gpu_busy_percent")).is_some()
        };
        let chosen = ctx
            .target
            .as_ref()
            .and_then(|t| list.iter().filter(usable).find(|a| a.id == t.id))
            .or_else(|| {
                list.iter()
                    .filter(usable)
                    .max_by_key(|a| (a.vram_total.unwrap_or(0), std::cmp::Reverse(a.index)))
            })?
            .clone();
        let card = drm_root.join(&chosen.id);
        Some(Self {
            root: drm_root.to_path_buf(),
            dev: card.join("device"),
            temp: hwmon_temp_path(&card),
            adapter: chosen,
            list,
            others: HashMap::new(),
            others_at: None,
            backoff: Backoff::default(),
            last: None,
        })
    }
}

/// Parses `pp_dpm_sclk` (`"0: 500Mhz\n1: 1800Mhz *\n"`) and returns the active level in MHz.
pub(crate) fn active_sclk_mhz(s: &str) -> Option<u64> {
    let line = s.lines().find(|l| l.trim_end().ends_with('*'))?;
    let f = line.split(':').nth(1)?.trim();
    let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

impl GpuSource for AmdGpu {
    fn backend(&self) -> &'static str {
        "amdgpu-sysfs"
    }

    fn read(&mut self) -> (Option<f64>, Map<String, Value>) {
        self.read_at(Instant::now())
    }
}

/// `detail.adapters` extras of one card: `card`, `busy`, `vram_used_mb`.
fn card_extra(root: &Path, a: &Adapter) -> Map<String, Value> {
    let dev = root.join(&a.id).join("device");
    let mut m = Map::new();
    m.insert("card".into(), Value::from(a.id.clone()));
    if sysfs::suspended(&dev) {
        m.insert("state".into(), Value::from("asleep"));
    } else if a.vendor == Some(sysfs::VENDOR_AMD) {
        if let Some(b) = read_u64(&dev.join("gpu_busy_percent")) {
            m.insert("busy".into(), Value::from(b.min(100)));
        }
        if let Some(u) = read_u64(&dev.join("mem_info_vram_used")) {
            m.insert("vram_used_mb".into(), mib(u));
        }
    }
    m
}

impl AmdGpu {
    fn read_at(&mut self, now: Instant) -> (Option<f64>, Map<String, Value>) {
        let asleep = sysfs::suspended(&self.dev);
        if !asleep && !self.backoff.due(now) {
            if let Some(last) = &self.last {
                return last.clone();
            }
        }
        let out = self.read_now(now, asleep);
        if !asleep {
            let took = now.elapsed();
            let sleeps = sysfs::can_sleep(&self.dev);
            self.backoff.record(now, took, out.0, sleeps);
            self.last = Some(out.clone());
        }
        out
    }

    /// `asleep`: the card is powered down; report it idle without reading its files.
    fn read_now(&mut self, now: Instant, asleep: bool) -> (Option<f64>, Map<String, Value>) {
        let read = |name: &str| (!asleep).then(|| read_u64(&self.dev.join(name))).flatten();
        let busy = if asleep {
            Some(0.0)
        } else {
            read("gpu_busy_percent").map(|v| v.min(100) as f64)
        };
        let mut d = Map::new();
        if asleep {
            d.insert("state".into(), Value::from("asleep"));
        }
        d.insert("name".into(), Value::from(self.adapter.name.clone()));
        d.insert("adapter_index".into(), Value::from(self.adapter.index));
        d.insert("card".into(), Value::from(self.adapter.id.clone()));
        let used = read("mem_info_vram_used");
        if let (Some(used), Some(total)) = (used, read("mem_info_vram_total")) {
            if total >= MIB {
                d.insert("vram_used_mb".into(), mib(used));
                d.insert("vram_total_mb".into(), mib(total));
            }
        }
        let temp = self.temp.as_deref().filter(|_| !asleep);
        if let Some(t) = temp.and_then(read_u64) {
            d.insert("temp_c".into(), num((t as f64 / 1000.0).round()));
        }
        let sclk = (!asleep).then(|| read_str(&self.dev.join("pp_dpm_sclk")));
        if let Some(f) = sclk.flatten().and_then(|s| active_sclk_mhz(&s)) {
            d.insert("freq_mhz".into(), Value::from(f));
        }
        if self
            .others_at
            .is_none_or(|t| now.saturating_duration_since(t) >= OTHERS_EVERY)
        {
            self.others_at = Some(now);
            self.others = self
                .list
                .iter()
                .filter(|a| a.id != self.adapter.id)
                .map(|a| (a.id.clone(), card_extra(&self.root, a)))
                .collect();
        }
        let me = &self.adapter.id;
        let others = &self.others;
        let adapters = adapters_detail(&self.list, Some(self.adapter.index), |a| {
            if a.id != *me {
                return others.get(&a.id).cloned().unwrap_or_default();
            }
            // The chosen card: this tick's values, no extra reads.
            let mut m = Map::new();
            m.insert("card".into(), Value::from(a.id.clone()));
            if let Some(b) = busy {
                m.insert("busy".into(), Value::from(b as u64));
            }
            if let Some(u) = used {
                m.insert("vram_used_mb".into(), mib(u));
            }
            m
        });
        d.insert("adapters".into(), adapters);
        d.insert("backend".into(), Value::from(self.backend()));
        (busy, d)
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
    fn sclk_parse() {
        assert_eq!(
            active_sclk_mhz("0: 500Mhz\n1: 1800Mhz *\n2: 2400Mhz\n"),
            Some(1800)
        );
        assert_eq!(active_sclk_mhz("0: 500Mhz\n"), None);
    }

    #[test]
    fn probe_and_read_fake_tree() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        // Intel iGPU at card0 must be ignored by the amdgpu backend.
        w(r, "card0/device/vendor", "0x8086\n");
        w(r, "card0/device/device", "0x5912\n");
        w(r, "card0/gt_cur_freq_mhz", "350\n");
        // APU with little VRAM.
        w(r, "card1/device/vendor", "0x1002\n");
        w(r, "card1/device/device", "0x1638\n");
        w(r, "card1/device/gpu_busy_percent", "3\n");
        w(
            r,
            "card1/device/mem_info_vram_total",
            &(512 * MIB).to_string(),
        );
        // Discrete card: should win.
        w(r, "card2/device/vendor", "0x1002\n");
        w(r, "card2/device/device", "0x73bf\n");
        w(r, "card2/device/gpu_busy_percent", "42\n");
        w(
            r,
            "card2/device/mem_info_vram_used",
            &(3120 * MIB).to_string(),
        );
        w(
            r,
            "card2/device/mem_info_vram_total",
            &(16384 * MIB).to_string(),
        );
        w(r, "card2/device/hwmon/hwmon5/temp1_input", "61000\n");
        w(r, "card2/device/pp_dpm_sclk", "0: 500Mhz\n1: 2100Mhz *\n");

        let mut g = AmdGpu::probe(r, &Ctx::default()).expect("amdgpu found");
        let (v, d) = g.read();
        assert_eq!(v, Some(42.0));
        assert_eq!(d["name"], "AMD GPU [1002:73bf]");
        assert_eq!(d["adapter_index"], 2);
        assert_eq!(d["adapters"]["#1"]["busy"], 3);
        assert_eq!(d["adapters"]["#2"]["chosen"], true);
        assert_eq!(d["adapters"]["#0"]["name"], "Intel GPU [8086:5912]");
        assert_eq!(d["vram_used_mb"], 3120);
        assert_eq!(d["vram_total_mb"], 16384);
        assert_eq!(d["temp_c"], 61.0);
        assert_eq!(d["freq_mhz"], 2100);
        assert_eq!(d["backend"], "amdgpu-sysfs");

        // Busy value out of range is capped.
        w(r, "card2/device/gpu_busy_percent", "250\n");
        assert_eq!(g.read().0, Some(100.0));
        // Other cards are re-read only every OTHERS_EVERY (no waking a sleeping dGPU).
        w(r, "card1/device/gpu_busy_percent", "55\n");
        let t0 = Instant::now();
        let mut g = AmdGpu::probe(r, &Ctx::default()).unwrap();
        let (_, d) = g.read_at(t0);
        assert_eq!(d["adapters"]["#1"]["busy"], 55);
        w(r, "card1/device/gpu_busy_percent", "66\n");
        w(r, "card2/device/gpu_busy_percent", "43\n");
        let (v, d) = g.read_at(t0 + Duration::from_secs(5));
        assert_eq!(v, Some(43.0), "the chosen card is read every tick");
        assert_eq!(d["adapters"]["#2"]["busy"], 43);
        assert_eq!(d["adapters"]["#1"]["busy"], 55, "cached");
        let (_, d) = g.read_at(t0 + OTHERS_EVERY);
        assert_eq!(d["adapters"]["#1"]["busy"], 66);
        // File vanishing (GPU reset) gives null, not a panic.
        fs::remove_file(r.join("card2/device/gpu_busy_percent")).unwrap();
        assert_eq!(g.read().0, None);
    }

    /// A discrete card at card1 that powers down when idle, next to an Intel iGPU.
    fn sleepy_card(r: &Path, status: &str, busy: &str) {
        w(r, "card0/device/vendor", "0x8086\n");
        w(r, "card1/device/vendor", "0x1002\n");
        w(r, "card1/device/device", "0x73bf\n");
        w(r, "card1/device/gpu_busy_percent", busy);
        w(r, "card1/device/power/control", "auto\n");
        w(r, "card1/device/power/runtime_suspended_time", "5000\n");
        w(r, "card1/device/power/runtime_status", status);
    }

    #[test]
    fn suspended_card_is_reported_idle_without_being_read() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        sleepy_card(r, "active\n", "42\n");
        let mut g = AmdGpu::probe(r, &Ctx::default()).unwrap();
        let t0 = Instant::now();
        assert_eq!(g.read_at(t0).0, Some(42.0));
        // Suspended: the statistics file is not consulted (it still says 42).
        w(r, "card1/device/power/runtime_status", "suspended\n");
        let (v, d) = g.read_at(t0 + Duration::from_secs(1));
        assert_eq!(v, Some(0.0));
        assert_eq!(d["state"], "asleep");
        assert!(!d.contains_key("vram_used_mb"));
        // Awake again: read at once.
        w(r, "card1/device/power/runtime_status", "active\n");
        let (v, d) = g.read_at(t0 + Duration::from_secs(2));
        assert_eq!(v, Some(42.0));
        assert!(!d.contains_key("state"));
    }

    #[test]
    fn idle_card_that_sleeps_is_left_alone_for_a_while() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        sleepy_card(r, "active\n", "0\n");
        let mut g = AmdGpu::probe(r, &Ctx::default()).unwrap();
        let t0 = Instant::now();
        assert_eq!(g.read_at(t0).0, Some(0.0));
        // Load starts, but the card is not read again until IDLE_RECHECK has passed.
        w(r, "card1/device/gpu_busy_percent", "80\n");
        assert_eq!(g.read_at(t0 + Duration::from_secs(9)).0, Some(0.0));
        assert_eq!(g.read_at(t0 + sysfs::IDLE_RECHECK).0, Some(80.0));
        // Busy: read every tick.
        w(r, "card1/device/gpu_busy_percent", "70\n");
        let later = t0 + sysfs::IDLE_RECHECK + Duration::from_secs(1);
        assert_eq!(g.read_at(later).0, Some(70.0));

        // A card that has never suspended (it drives a display) is read every tick.
        w(r, "card1/device/power/runtime_suspended_time", "0\n");
        w(r, "card1/device/gpu_busy_percent", "0\n");
        let later = later + Duration::from_secs(1);
        assert_eq!(g.read_at(later).0, Some(0.0));
        w(r, "card1/device/gpu_busy_percent", "5\n");
        assert_eq!(g.read_at(later + Duration::from_secs(1)).0, Some(5.0));
    }

    #[test]
    fn no_amd_card() {
        let t = tempfile::tempdir().unwrap();
        w(t.path(), "card0/device/vendor", "0x8086\n");
        assert!(AmdGpu::probe(t.path(), &Ctx::default()).is_none());
        assert!(AmdGpu::probe(&t.path().join("missing"), &Ctx::default()).is_none());
    }

    #[test]
    fn target_selects_the_card() {
        let t = tempfile::tempdir().unwrap();
        crate::gpu::adapters::two_amd_cards_for_tests(t.path());
        let list = from_sysfs(t.path());
        // auto: the R9700 (most VRAM), with every card in detail.adapters.
        let (target, pinned) = crate::gpu::adapters::resolve(&list, "auto");
        let ctx = Ctx {
            list: list.clone(),
            target,
            pinned,
            strict: true,
            ..Ctx::default()
        };
        let (v, d) = AmdGpu::probe(t.path(), &ctx).unwrap().read();
        assert_eq!(v, Some(97.0));
        assert_eq!(d["name"], "AMD Radeon AI PRO R9700");
        assert_eq!(d["adapter_index"], 1);
        assert_eq!(d["vram_total_mb"], 32768);
        assert_eq!(d["adapters"]["#0"]["busy"], 12);
        assert_eq!(d["adapters"]["#0"]["vram_used_mb"], 300);
        assert_eq!(d["adapters"]["#1"]["chosen"], true);
        // Pinned to the iGPU by index.
        let (target, pinned) = crate::gpu::adapters::resolve(&list, "#0");
        let ctx = Ctx {
            list,
            target,
            pinned,
            strict: true,
            ..Ctx::default()
        };
        let (v, d) = AmdGpu::probe(t.path(), &ctx).unwrap().read();
        assert_eq!(v, Some(12.0));
        assert_eq!(d["adapter_index"], 0);
    }
}
