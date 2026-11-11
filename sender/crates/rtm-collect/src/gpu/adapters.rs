//! The GPUs on this machine, as one ordered list shared by every backend and the GUI, plus
//! `metrics.gpu_adapter` resolution.
//!
//! * Linux: every `/sys/class/drm/cardN` with a PCI `device/vendor`, in card order. Name from
//!   `device/product_name` (amdgpu boards usually have it), else `"AMD GPU [1002:7551]"`.
//!   VRAM from `device/mem_info_vram_total` (amdgpu); sysfs has none for NVIDIA, so with the
//!   `nvml` feature an NVIDIA card's VRAM is NVML's memory total (matched by PCI bus id, else
//!   the k-th NVIDIA card is the k-th NVML device).
//! * Windows: DXGI `EnumAdapters1`, hardware adapters only, in DXGI order. DXGI gives the
//!   driver's description (the same text as the registry `DriverDesc`), the adapter LUID that
//!   PDH instance names carry, and the dedicated VRAM, in one call; compute-only cards with no
//!   display output are listed too.
//!
//! `gpu_adapter`: `"auto"` (discrete before integrated, Intel iGPUs last; then largest dedicated
//! VRAM, then lowest index),
//! `"#N"` (index in this list), an id (`card2` / a LUID), or a case-insensitive substring of
//! the name (`"R9700"`). A value that matches nothing logs a warning and acts as `auto`.

use serde_json::{Map, Value};
use std::sync::Mutex;

pub(crate) use super::sysfs::{VENDOR_INTEL, VENDOR_NVIDIA};

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
    /// Best guess, see [`guess_integrated`]. Auto picks a discrete GPU over an integrated one.
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

/// Integrated when: Intel without a known >= 2 GiB of VRAM (Arc cards report theirs on
/// Windows), or any other non-NVIDIA vendor with a known VRAM under 2 GiB (APU carve-out).
/// NVIDIA is always discrete. Unknown VRAM (sysfs has none for NVIDIA) is not evidence of an
/// iGPU, so a non-Intel card with unknown VRAM counts as discrete.
pub(crate) fn guess_integrated(vendor: Option<u16>, vram: Option<u64>) -> bool {
    const BIG: u64 = 2 << 30;
    match vendor {
        Some(VENDOR_NVIDIA) => false,
        Some(VENDOR_INTEL) => vram.is_none_or(|v| v < BIG),
        _ => vram.is_some_and(|v| v < BIG),
    }
}

/// Detect the GPUs (cheap: a few sysfs reads, or one DXGI enumeration).
pub fn list_adapters() -> Vec<Adapter> {
    #[cfg(target_os = "linux")]
    {
        let root = std::path::Path::new(super::sysfs::DRM_ROOT);
        #[cfg(feature = "nvml")]
        {
            from_sysfs_with(root, || super::nvml::memory_totals().unwrap_or_default())
        }
        #[cfg(not(feature = "nvml"))]
        {
            from_sysfs(root)
        }
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

/// Linux: the cards under a DRM root (a tempdir in tests), without NVML.
pub fn from_sysfs(drm_root: &std::path::Path) -> Vec<Adapter> {
    from_sysfs_with(drm_root, Vec::new)
}

/// One NVML device: PCI bus id (`00000000:01:00.0`, if known) and memory total in bytes.
pub type NvmlMem = (Option<String>, u64);

/// `bb:dd.f` of a PCI address, lowercase, without the domain (sysfs uses a 4-digit domain,
/// NVML an 8-digit one).
fn pci_tail(addr: &str) -> String {
    let a = addr.trim().to_ascii_lowercase();
    match a.split_once(':') {
        Some((_, rest)) if rest.contains(':') => rest.to_string(),
        _ => a,
    }
}

/// [`from_sysfs`], filling the unknown VRAM of NVIDIA cards from `nvml()` (called only when
/// there is such a card): by PCI bus id, else the k-th NVIDIA card gets the k-th device.
pub fn from_sysfs_with(
    drm_root: &std::path::Path,
    nvml: impl FnOnce() -> Vec<NvmlMem>,
) -> Vec<Adapter> {
    use super::sysfs::{cards, gpu_name, pci_ids, read_str, read_u64};
    struct Row {
        id: String,
        vendor: u16,
        name: String,
        vram: Option<u64>,
        bus: Option<String>,
    }
    let mut rows: Vec<Row> = cards(drm_root)
        .into_iter()
        .filter_map(|card| {
            let (vendor, device) = pci_ids(&card)?;
            let dev = card.join("device");
            let vram = read_u64(&dev.join("mem_info_vram_total"));
            let name = read_str(&dev.join("product_name"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| gpu_name(vendor, device));
            // `device` is a symlink to `/sys/devices/pci0000:00/.../0000:01:00.0`.
            let bus = std::fs::canonicalize(&dev)
                .ok()
                .and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()));
            Some(Row {
                id: card.file_name()?.to_string_lossy().into_owned(),
                vendor,
                name,
                vram,
                bus,
            })
        })
        .collect();
    if rows
        .iter()
        .any(|r| r.vendor == VENDOR_NVIDIA && r.vram.is_none_or(|v| v == 0))
    {
        let devs = nvml();
        let nvidia = rows.iter_mut().filter(|r| r.vendor == VENDOR_NVIDIA);
        for (k, r) in nvidia.enumerate() {
            let by_bus = r.bus.as_deref().and_then(|b| {
                let b = pci_tail(b);
                devs.iter()
                    .find(|(nb, _)| nb.as_deref().is_some_and(|nb| pci_tail(nb) == b))
            });
            let found = by_bus.or_else(|| devs.get(k)).map(|d| d.1);
            if r.vram.is_none_or(|v| v == 0) {
                r.vram = found.filter(|&v| v > 0).or(r.vram);
            }
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(index, r)| Adapter {
            index,
            name: r.name,
            id: r.id,
            vendor: Some(r.vendor),
            vram_total: r.vram,
            integrated: guess_integrated(Some(r.vendor), r.vram),
        })
        .collect()
}

/// `gpu_adapter` means "auto" (also when empty).
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

/// Discrete before integrated, an integrated Intel GPU last of all; then largest dedicated
/// VRAM (unknown = 0); then lowest index.
pub fn auto_choice(list: &[Adapter]) -> Option<&Adapter> {
    let rank = |a: &Adapter| {
        let intel = a.vendor == Some(VENDOR_INTEL) || a.name.to_ascii_lowercase().contains("intel");
        (
            !a.integrated,
            !(a.integrated && intel),
            a.vram_total.unwrap_or(0),
        )
    };
    list.iter()
        .max_by(|a, b| rank(a).cmp(&rank(b)).then(b.index.cmp(&a.index)))
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

/// Most entries in `detail.adapters` (packet size).
pub const MAX_DETAIL_ADAPTERS: usize = 4;

/// `detail.adapters`: `{"#0": {"name": .., "vram_total_mb": .., "chosen": bool, ..extra}}`.
/// At most [`MAX_DETAIL_ADAPTERS`] entries, in list order, always including the chosen one.
pub fn adapters_detail(
    list: &[Adapter],
    chosen: Option<usize>,
    mut extra: impl FnMut(&Adapter) -> Map<String, Value>,
) -> Value {
    let chosen_in_head = list
        .iter()
        .take(MAX_DETAIL_ADAPTERS)
        .any(|a| Some(a.index) == chosen);
    let keep = |pos: usize, a: &Adapter| {
        if chosen_in_head || chosen.is_none() || !list.iter().any(|x| Some(x.index) == chosen) {
            pos < MAX_DETAIL_ADAPTERS
        } else {
            pos < MAX_DETAIL_ADAPTERS - 1 || Some(a.index) == chosen
        }
    };
    let mut out = Map::new();
    for (pos, a) in list.iter().enumerate() {
        if !keep(pos, a) {
            continue;
        }
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
pub(crate) mod tests {
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

    fn w(root: &std::path::Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// card0 = `first` (vendor, optional VRAM MiB), card1 = NVIDIA with no VRAM in sysfs.
    pub(crate) fn igpu_plus_nvidia(r: &std::path::Path, vendor: &str, vram_mb: Option<u64>) {
        w(r, "card0/device/vendor", vendor);
        w(r, "card0/device/device", "0x1234\n");
        if let Some(mb) = vram_mb {
            w(
                r,
                "card0/device/mem_info_vram_total",
                &(mb << 20).to_string(),
            );
            w(r, "card0/device/gpu_busy_percent", "5\n");
        }
        w(r, "card1/device/vendor", "0x10de\n");
        w(r, "card1/device/device", "0x2684\n");
    }

    #[test]
    fn intel_igpu_plus_nvidia_picks_nvidia() {
        let t = tempfile::tempdir().unwrap();
        igpu_plus_nvidia(t.path(), "0x8086\n", None);
        let l = from_sysfs(t.path());
        assert!(l[0].integrated, "Intel without VRAM is integrated");
        assert!(!l[1].integrated, "NVIDIA with unknown VRAM is discrete");
        assert_eq!(l[1].vram_total, None);
        let (target, pinned) = resolve(&l, "auto");
        assert_eq!((target.unwrap().id, pinned), ("card1".into(), false));
    }

    #[test]
    fn amd_apu_plus_nvidia_picks_nvidia() {
        let t = tempfile::tempdir().unwrap();
        igpu_plus_nvidia(t.path(), "0x1002\n", Some(512));
        let l = from_sysfs(t.path());
        assert!(l[0].integrated && !l[1].integrated);
        assert_eq!(auto_choice(&l).unwrap().id, "card1");
    }

    #[test]
    fn nvml_memory_fills_nvidia_vram() {
        let t = tempfile::tempdir().unwrap();
        // A 4 GiB-carve-out APU looks discrete; NVML's total for the NVIDIA card wins.
        igpu_plus_nvidia(t.path(), "0x1002\n", Some(4096));
        let l = from_sysfs(t.path());
        assert_eq!(
            auto_choice(&l).unwrap().id,
            "card0",
            "unknown VRAM ranks as 0"
        );
        let l = from_sysfs_with(t.path(), || vec![(None, 8u64 << 30)]);
        assert_eq!(l[1].vram_total, Some(8 << 30));
        assert_eq!(l[0].vram_total, Some(4096 << 20), "non-NVIDIA untouched");
        assert_eq!(auto_choice(&l).unwrap().id, "card1");
        // No NVIDIA card: NVML is not consulted.
        let t2 = tempfile::tempdir().unwrap();
        two_amd_cards(t2.path());
        let l = from_sysfs_with(t2.path(), || panic!("nvml queried"));
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn pci_bus_ids_match_across_domain_widths() {
        assert_eq!(pci_tail("0000:01:00.0"), "01:00.0");
        assert_eq!(pci_tail("00000000:01:00.0"), "01:00.0");
        assert_eq!(pci_tail("01:00.0"), "01:00.0");
    }

    #[test]
    fn integrated_guess() {
        assert!(!guess_integrated(Some(VENDOR_NVIDIA), None));
        assert!(!guess_integrated(Some(VENDOR_NVIDIA), Some(0)));
        assert!(guess_integrated(Some(VENDOR_INTEL), None));
        assert!(guess_integrated(Some(VENDOR_INTEL), Some(128 << 20)));
        assert!(!guess_integrated(Some(VENDOR_INTEL), Some(16 << 30)), "Arc");
        assert!(!guess_integrated(Some(0x1002), None));
        assert!(guess_integrated(Some(0x1002), Some(512 << 20)));
        assert!(!guess_integrated(Some(0x1002), Some(16 << 30)));
    }

    #[test]
    fn integrated_intel_is_last_resort() {
        let i = fake(0, "Intel UHD", "card0", VENDOR_INTEL, 128);
        let apu = fake(1, "AMD Radeon Graphics", "card1", 0x1002, 512);
        assert_eq!(auto_choice(&[i.clone(), apu]).unwrap().index, 1);
        assert_eq!(auto_choice(&[i]).unwrap().index, 0);
    }

    #[test]
    fn detail_is_capped_and_keeps_the_chosen_adapter() {
        let l: Vec<Adapter> = (0..7)
            .map(|i| fake(i, &format!("G{i}"), &format!("card{i}"), 0x1002, 4096))
            .collect();
        let d = adapters_detail(&l, Some(1), |_| Map::new());
        assert_eq!(d.as_object().unwrap().len(), MAX_DETAIL_ADAPTERS);
        let d = adapters_detail(&l, Some(6), |_| Map::new());
        let o = d.as_object().unwrap();
        assert_eq!(o.len(), MAX_DETAIL_ADAPTERS);
        assert_eq!(o["#6"]["chosen"], true);
        assert!(o.contains_key("#0") && !o.contains_key("#3"));
        assert_eq!(
            adapters_detail(&l, None, |_| Map::new())
                .as_object()
                .unwrap()
                .len(),
            4
        );
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
