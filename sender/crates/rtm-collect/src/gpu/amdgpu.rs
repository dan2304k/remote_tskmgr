//! amdgpu sysfs backend (Linux): `device/gpu_busy_percent`, `mem_info_vram_{used,total}`,
//! hwmon `temp1_input` (edge), and the active `pp_dpm_sclk` level for frequency.

use super::adapters::{adapters_detail, from_sysfs, Adapter};
use super::sysfs::{self, hwmon_temp_path, read_str, read_u64};
use super::{Ctx, GpuSource};
use crate::util::{mib, num, MIB};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub struct AmdGpu {
    root: PathBuf,
    dev: PathBuf,
    adapter: Adapter,
    temp: Option<PathBuf>,
    /// Every GPU detected, for `detail.adapters`.
    list: Vec<Adapter>,
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
        let busy = read_u64(&self.dev.join("gpu_busy_percent")).map(|v| v.min(100) as f64);
        let mut d = Map::new();
        d.insert("name".into(), Value::from(self.adapter.name.clone()));
        d.insert("adapter_index".into(), Value::from(self.adapter.index));
        d.insert("card".into(), Value::from(self.adapter.id.clone()));
        if let (Some(used), Some(total)) = (
            read_u64(&self.dev.join("mem_info_vram_used")),
            read_u64(&self.dev.join("mem_info_vram_total")),
        ) {
            if total >= MIB {
                d.insert("vram_used_mb".into(), mib(used));
                d.insert("vram_total_mb".into(), mib(total));
            }
        }
        if let Some(t) = self.temp.as_deref().and_then(read_u64) {
            d.insert("temp_c".into(), num((t as f64 / 1000.0).round()));
        }
        if let Some(f) = read_str(&self.dev.join("pp_dpm_sclk")).and_then(|s| active_sclk_mhz(&s)) {
            d.insert("freq_mhz".into(), Value::from(f));
        }
        let root = &self.root;
        let adapters = adapters_detail(&self.list, Some(self.adapter.index), |a| {
            let dev = root.join(&a.id).join("device");
            let mut m = Map::new();
            m.insert("card".into(), Value::from(a.id.clone()));
            if let Some(b) = read_u64(&dev.join("gpu_busy_percent")) {
                m.insert("busy".into(), Value::from(b.min(100)));
            }
            if let Some(u) = read_u64(&dev.join("mem_info_vram_used")) {
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
        // File vanishing (GPU reset) gives null, not a panic.
        fs::remove_file(r.join("card2/device/gpu_busy_percent")).unwrap();
        assert_eq!(g.read().0, None);
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
        };
        let (v, d) = AmdGpu::probe(t.path(), &ctx).unwrap().read();
        assert_eq!(v, Some(12.0));
        assert_eq!(d["adapter_index"], 0);
    }
}
