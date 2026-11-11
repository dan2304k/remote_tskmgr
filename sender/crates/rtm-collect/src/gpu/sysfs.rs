//! Helpers for reading `/sys/class/drm`. Pure path-based so tests can point them at a fake
//! tree in a tempdir.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const DRM_ROOT: &str = "/sys/class/drm";

pub(crate) const VENDOR_AMD: u16 = 0x1002;
pub(crate) const VENDOR_INTEL: u16 = 0x8086;
pub(crate) const VENDOR_NVIDIA: u16 = 0x10de;

/// `card<N>` directories (not connectors like `card1-HDMI-A-1`), sorted by N.
pub(crate) fn cards(drm_root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(drm_root) else {
        return Vec::new();
    };
    let mut v: Vec<(u32, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n: u32 = name.strip_prefix("card")?.parse().ok()?;
            Some((n, e.path()))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

pub(crate) fn read_str(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub(crate) fn read_u64(p: &Path) -> Option<u64> {
    read_str(p)?.parse().ok()
}

/// Parses `0x8086` style PCI id files.
pub(crate) fn read_hex_u16(p: &Path) -> Option<u16> {
    let s = read_str(p)?;
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(&s);
    u16::from_str_radix(s, 16).ok()
}

/// `(vendor, device)` PCI ids of a card's `device/` directory.
pub(crate) fn pci_ids(card: &Path) -> Option<(u16, u16)> {
    let dev = card.join("device");
    Some((
        read_hex_u16(&dev.join("vendor"))?,
        read_hex_u16(&dev.join("device")).unwrap_or(0),
    ))
}

/// Human-ish name without a PCI database: `"Intel GPU [8086:5912]"`.
pub(crate) fn gpu_name(vendor: u16, device: u16) -> String {
    let v = match vendor {
        VENDOR_AMD => "AMD",
        VENDOR_INTEL => "Intel",
        VENDOR_NVIDIA => "NVIDIA",
        _ => "Unknown",
    };
    format!("{v} GPU [{vendor:04x}:{device:04x}]")
}

/// First `hwmon*/temp1_input` under the card's device dir, in °C.
pub(crate) fn hwmon_temp_path(card: &Path) -> Option<PathBuf> {
    let rd = fs::read_dir(card.join("device").join("hwmon")).ok()?;
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| d.join("temp1_input"))
        .find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_and_ids() {
        let t = tempfile::tempdir().unwrap();
        for d in [
            "card10/device",
            "card2/device",
            "card2-HDMI-A-1",
            "renderD128",
        ] {
            fs::create_dir_all(t.path().join(d)).unwrap();
        }
        fs::write(t.path().join("card2/device/vendor"), "0x1002\n").unwrap();
        fs::write(t.path().join("card2/device/device"), "0x73bf\n").unwrap();
        let c = cards(t.path());
        assert_eq!(c.len(), 2);
        assert!(c[0].ends_with("card2"));
        assert_eq!(pci_ids(&c[0]), Some((0x1002, 0x73bf)));
        assert_eq!(pci_ids(&c[1]), None);
        assert_eq!(gpu_name(0x1002, 0x73bf), "AMD GPU [1002:73bf]");
    }
}
