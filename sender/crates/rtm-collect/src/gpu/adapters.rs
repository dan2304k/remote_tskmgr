//! The GPUs on this machine, as one ordered list shared by every backend and the GUI, plus
//! `metrics.gpu_adapter` resolution.
//!
//! * Linux: every `/sys/class/drm/cardN` with a PCI `device/vendor`, in card order. Name from
//!   `device/product_name` (amdgpu boards usually have it), else `"AMD GPU [1002:7551]"`.
//!   VRAM from `device/mem_info_vram_total` (amdgpu).
//! * Windows: DXGI `EnumAdapters1`, hardware adapters only, in DXGI order. DXGI gives the
//!   driver's description (the same text as the registry `DriverDesc`), the adapter LUID that
//!   PDH instance names carry, and the dedicated VRAM, in one call; compute-only cards with no
//!   display output are listed too.
//!
//! `gpu_adapter`: `"auto"` (largest dedicated VRAM; tie: not integrated, then lowest index),
//! `"#N"` (index in this list), an id (`card2` / a LUID), or a case-insensitive substring of
//! the name (`"R9700"`). A value that matches nothing logs a warning and acts as `auto`.

use serde_json::{Map, Value};
use std::sync::Mutex;

pub(crate) const VENDOR_NVIDIA: u16 = 0x10de;
pub(crate) const VENDOR_INTEL: u16 = 0x8086;

/// One detected GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    /// Position in `list_adapters()`; `"#N"` in `gpu_adapter` refers to it.
    pub index: usize,
    pub name: String,
    /// `card2` (Linux) or the LUID as in PDH instance names, `0x00000000_0x0000c2b6` (Windows).
    pub id: String,
    pub vendor: Option<u16>,
    /// Dedicated VRAM in bytes, when known.
    pub vram_total: Option<u64>,
    /// Best guess: Intel, or less than 2 GiB of dedicated VRAM. Only breaks VRAM ties.
    pub integrated: bool,
}

impl Adapter {
    /// `"#1 AMD Radeon AI PRO R9700 (32 GB)"`.
    pub fn label(&self) -> String {
        match self.vram_total.filter(|&b| b > 0) {
            Some(b) if b >= 1 << 30 => format!(
                "#{} {} ({} GB)",
                self.index,
                self.name,
                (b as f64 / (1u64 << 30) as f64).round()
            ),
            Some(b) => format!("#{} {} ({} MB)", self.index, self.name, b >> 20),
            None => format!("#{} {}", self.index, self.name),
        }
    }

    pub fn is_nvidia(&self) -> bool {
        self.vendor == Some(VENDOR_NVIDIA) || self.name.to_ascii_lowercase().contains("nvidia")
    }
}

pub(crate) fn guess_integrated(vendor: Option<u16>, vram: Option<u64>) -> bool {
    vendor == Some(VENDOR_INTEL) || vram.unwrap_or(0) < (2 << 30)
}

/// Detect the GPUs (cheap: a few sysfs reads, or one DXGI enumeration).
pub fn list_adapters() -> Vec<Adapter> {
    #[cfg(target_os = "linux")]
    {
        from_sysfs(std::path::Path::new(super::sysfs::DRM_ROOT))
    }
    #[cfg(windows)]
    {
        super::dxgi::list()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Vec::new()
    }
}

/// Linux: the cards under a DRM root (a tempdir in tests).
pub fn from_sysfs(drm_root: &std::path::Path) -> Vec<Adapter> {
    use super::sysfs::{cards, gpu_name, pci_ids, read_str, read_u64};
    cards(drm_root)
        .into_iter()
        .filter_map(|card| {
            let (vendor, device) = pci_ids(&card)?;
            let dev = card.join("device");
            let vram = read_u64(&dev.join("mem_info_vram_total"));
            let name = read_str(&dev.join("product_name"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| gpu_name(vendor, device));
            Some((
                card.file_name()?.to_string_lossy().into_owned(),
                vendor,
                name,
                vram,
            ))
        })
        .enumerate()
        .map(|(index, (id, vendor, name, vram))| Adapter {
            index,
            name,
            id,
            vendor: Some(vendor),
            vram_total: vram,
            integrated: guess_integrated(Some(vendor), vram),
        })
        .collect()
}

pub fn is_auto(pin: &str) -> bool {
    let p = pin.trim();
    p.is_empty() || p.eq_ignore_ascii_case("auto")
}

/// The adapter `pin` names (`#N`, id, or name substring), if any.
pub fn find<'a>(list: &'a [Adapter], pin: &str) -> Option<&'a Adapter> {
    let p = pin.trim();
    if is_auto(p) {
        return None;
    }
    if let Some(n) = p
        .strip_prefix('#')
        .and_then(|n| n.trim().parse::<usize>().ok())
    {
        return list.iter().find(|a| a.index == n);
    }
    let lp = p.to_ascii_lowercase();
    list.iter()
        .find(|a| a.id.eq_ignore_ascii_case(p))
        .or_else(|| {
            list.iter()
                .find(|a| a.name.to_ascii_lowercase().contains(&lp))
        })
}

/// Largest dedicated VRAM; tie: not integrated, then lowest index.
pub fn auto_choice(list: &[Adapter]) -> Option<&Adapter> {
    list.iter().max_by(|a, b| {
        a.vram_total
            .unwrap_or(0)
            .cmp(&b.vram_total.unwrap_or(0))
            .then((!a.integrated).cmp(&!b.integrated))
            .then(b.index.cmp(&a.index))
    })
}

/// The adapter to report and whether the user pinned it. A pin that matches nothing is
/// logged and treated as auto.
pub fn resolve(list: &[Adapter], pin: &str) -> (Option<Adapter>, bool) {
    if !is_auto(pin) {
        if let Some(a) = find(list, pin) {
            return (Some(a.clone()), true);
        }
        tracing::warn!(
            gpu_adapter = pin,
            detected = ?list.iter().map(Adapter::label).collect::<Vec<_>>(),
            "gpu_adapter matches no detected GPU; using auto"
        );
    }
    (auto_choice(list).cloned(), false)
}

/// `detail.adapters`: `{"#0": {"name": .., "vram_total_mb": .., "chosen": bool, ..extra}}`.
pub fn adapters_detail(
    list: &[Adapter],
    chosen: Option<usize>,
    mut extra: impl FnMut(&Adapter) -> Map<String, Value>,
) -> Value {
    let mut out = Map::new();
    for a in list {
        let mut e = Map::new();
        e.insert("name".into(), Value::from(a.name.clone()));
        if let Some(b) = a.vram_total.filter(|&b| b > 0) {
            e.insert("vram_total_mb".into(), Value::from(b >> 20));
        }
        e.extend(extra(a));
        e.insert("chosen".into(), Value::from(chosen == Some(a.index)));
        out.insert(format!("#{}", a.index), Value::Object(e));
    }
    Value::Object(out)
}

/// What the GPU collector reported last: `(adapter index, name)`. Read by the settings
/// window to mark the reported adapter.
static CURRENT: Mutex<Option<(Option<usize>, String)>> = Mutex::new(None);

pub fn current_adapter() -> Option<(Option<usize>, String)> {
    CURRENT.lock().ok()?.clone()
}

pub(crate) fn set_current(v: Option<(Option<usize>, String)>) {
    if let Ok(mut c) = CURRENT.lock() {
        if *c != v {
            *c = v;
        }
    }
}

#[cfg(test)]
pub(crate) fn fake(index: usize, name: &str, id: &str, vendor: u16, vram_mb: u64) -> Adapter {
    let vram = Some(vram_mb << 20);
    Adapter {
        index,
        name: name.into(),
        id: id.into(),
        vendor: Some(vendor),
        vram_total: vram,
        integrated: guess_integrated(Some(vendor), vram),
    }
}

/// The owner's desktop: Radeon iGPU (card0) driving the display, R9700 (card1) for AI.
#[cfg(test)]
pub(crate) fn two_amd_cards_for_tests(r: &std::path::Path) {
    fn w(root: &std::path::Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
    w(r, "card0/device/vendor", "0x1002\n");
    w(r, "card0/device/device", "0x13c0\n");
    w(r, "card0/device/gpu_busy_percent", "12\n");
    w(
        r,
        "card0/device/mem_info_vram_total",
        &(512u64 << 20).to_string(),
    );
    w(
        r,
        "card0/device/mem_info_vram_used",
        &(300u64 << 20).to_string(),
    );
    w(r, "card0-HDMI-A-1/status", "connected\n");
    w(r, "card1/device/vendor", "0x1002\n");
    w(r, "card1/device/device", "0x7551\n");
    w(r, "card1/device/product_name", "AMD Radeon AI PRO R9700\n");
    w(r, "card1/device/gpu_busy_percent", "97\n");
    w(
        r,
        "card1/device/mem_info_vram_total",
        &(32768u64 << 20).to_string(),
    );
    w(
        r,
        "card1/device/mem_info_vram_used",
        &(20000u64 << 20).to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_amd_cards(r: &std::path::Path) {
        two_amd_cards_for_tests(r)
    }

    #[test]
    fn sysfs_list_and_selection() {
        let t = tempfile::tempdir().unwrap();
        two_amd_cards(t.path());
        let l = from_sysfs(t.path());
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "AMD GPU [1002:13c0]");
        assert!(l[0].integrated);
        assert_eq!(l[1].name, "AMD Radeon AI PRO R9700");
        assert_eq!((l[1].index, l[1].id.as_str()), (1, "card1"));
        assert!(!l[1].integrated);
        assert_eq!(l[1].label(), "#1 AMD Radeon AI PRO R9700 (32 GB)");
        // auto = most VRAM.
        assert_eq!(resolve(&l, "auto"), (Some(l[1].clone()), false));
        // Pins: substring, index, id.
        assert_eq!(resolve(&l, "r9700"), (Some(l[1].clone()), true));
        assert_eq!(resolve(&l, "#0"), (Some(l[0].clone()), true));
        assert_eq!(resolve(&l, " card0 "), (Some(l[0].clone()), true));
        // No match: auto.
        assert_eq!(resolve(&l, "#7"), (Some(l[1].clone()), false));
        assert_eq!(resolve(&l, "geforce"), (Some(l[1].clone()), false));
        assert_eq!(from_sysfs(&t.path().join("missing")), vec![]);
    }

    #[test]
    fn windows_style_list_selection() {
        // DXGI order on the owner's desktop: the iGPU drives the display and comes first.
        let l = [
            fake(
                0,
                "AMD Radeon(TM) Graphics",
                "0x00000000_0x0000c2b6",
                0x1002,
                512,
            ),
            fake(
                1,
                "AMD Radeon AI PRO R9700",
                "0x00000000_0x0001d3a4",
                0x1002,
                32624,
            ),
        ];
        assert_eq!(
            resolve(&l, "auto").0.unwrap().index,
            1,
            "most dedicated VRAM"
        );
        assert_eq!(resolve(&l, "R9700"), (Some(l[1].clone()), true));
        assert_eq!(resolve(&l, "radeon(tm)"), (Some(l[0].clone()), true));
        assert_eq!(resolve(&l, "#0").0.unwrap().index, 0);
        assert_eq!(resolve(&l, "0x00000000_0x0000C2B6").0.unwrap().index, 0);
        // A substring matching both picks the first in list order.
        assert_eq!(resolve(&l, "radeon").0.unwrap().index, 0);
        assert!(!l[1].is_nvidia());
    }

    #[test]
    fn auto_ties_prefer_discrete_then_lowest_index() {
        let i = fake(0, "Intel UHD", "card0", VENDOR_INTEL, 0);
        let mut a = fake(1, "AMD", "card1", 0x1002, 0);
        a.integrated = false;
        let b = fake(2, "AMD 2", "card2", 0x1002, 0);
        assert_eq!(auto_choice(&[i.clone(), a.clone(), b]).unwrap().index, 1);
        assert_eq!(auto_choice(&[i]).unwrap().index, 0);
        assert_eq!(auto_choice(&[]), None);
    }

    #[test]
    fn detail_marks_the_chosen_adapter() {
        let l = [
            fake(0, "iGPU", "card0", 0x1002, 512),
            fake(1, "R9700", "card1", 0x1002, 32768),
        ];
        let d = adapters_detail(&l, Some(1), |a| {
            let mut m = Map::new();
            m.insert("util".into(), Value::from(a.index as f64 * 50.0));
            m
        });
        assert_eq!(d["#1"]["name"], "R9700");
        assert_eq!(d["#1"]["vram_total_mb"], 32768);
        assert_eq!(d["#1"]["util"], 50.0);
        assert_eq!(d["#1"]["chosen"], true);
        assert_eq!(d["#0"]["chosen"], false);
    }
}
