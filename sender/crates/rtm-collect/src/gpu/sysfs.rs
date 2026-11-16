//! Helpers for reading `/sys/class/drm`. Pure path-based so tests can point them at a fake
//! tree in a tempdir.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// Is the PCI device at `dev` (a card's `device/` directory) powered down by runtime power
/// management? Reading `power/runtime_status` does not wake it; reading its statistics does.
pub(crate) fn suspended(dev: &Path) -> bool {
    read_str(&dev.join("power/runtime_status")).as_deref() == Some("suspended")
}

/// Does the device at `dev` power down when idle? True when runtime power management is
/// allowed (`power/control` is `auto`) and it has been suspended at some point since boot
/// (`power/runtime_suspended_time` > 0). The second condition leaves out a GPU that drives a
/// display: it is allowed to suspend but never does, and is read every tick as before.
pub(crate) fn can_sleep(dev: &Path) -> bool {
    read_str(&dev.join("power/control")).as_deref() == Some("auto")
        && read_u64(&dev.join("power/runtime_suspended_time")).is_some_and(|ms| ms > 0)
}

/// `/sys/bus/pci/devices/<addr>` for a PCI address as NVML prints it (`00000000:01:00.0`,
/// an 8-digit domain) or as sysfs does (`0000:01:00.0`).
pub(crate) fn pci_device_dir(pci_root: &Path, bus_id: &str) -> Option<PathBuf> {
    let (domain, rest) = bus_id.trim().split_once(':')?;
    let domain = &domain[domain.len().saturating_sub(4)..];
    let dir = pci_root.join(format!("{domain:0>4}:{rest}").to_ascii_lowercase());
    dir.is_dir().then_some(dir)
}

pub(crate) const PCI_ROOT: &str = "/sys/bus/pci/devices";

/// A read at least this long means the GPU had to be woken (a normal read is ~1 ms).
pub(crate) const SLOW_READ: Duration = Duration::from_millis(50);
/// How long the reading of an idle GPU that powers down is repeated before it is read again.
pub(crate) const IDLE_RECHECK: Duration = Duration::from_secs(10);

/// Decides when a GPU is read again; between reads the last result is repeated.
///
/// Reading a GPU that powers down when idle either wakes it (slow, and the driver burns
/// kernel CPU time doing it) or, if it is still awake, keeps it from going to sleep. So after
/// reading such a GPU idle, it is left alone for `IDLE_RECHECK`. A busy GPU, and one that
/// never sleeps, is read every tick.
#[derive(Default)]
pub(crate) struct Backoff {
    until: Option<Instant>,
}

impl Backoff {
    /// Is a real read due at `now`?
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.until.is_none_or(|u| now >= u)
    }
    /// Record a read that finished at `now`, took `took` and returned utilisation `v`.
    /// `sleeps`: the GPU is known to power down when idle (`can_sleep`).
    pub(crate) fn record(&mut self, now: Instant, took: Duration, v: Option<f64>, sleeps: bool) {
        let idle = v.is_none_or(|v| v <= 0.0);
        self.until = (idle && (sleeps || took >= SLOW_READ)).then(|| now + IDLE_RECHECK);
    }
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

    #[test]
    fn idle_reads_back_off() {
        let ms = Duration::from_millis;
        let t = Instant::now();
        let mut b = Backoff::default();
        assert!(b.due(t));
        // Fast reads of a GPU that never sleeps, and reads of a busy GPU, happen every tick.
        b.record(t, ms(1), Some(0.0), false);
        assert!(b.due(t));
        b.record(t, ms(400), Some(35.0), false);
        assert!(b.due(t));
        b.record(t, ms(1), Some(35.0), true);
        assert!(b.due(t));
        // A slow read of an idle or unreadable GPU, and any idle read of one that sleeps,
        // is not repeated for IDLE_RECHECK.
        for (took, v, sleeps) in [
            (ms(400), Some(0.0), false),
            (ms(400), None, false),
            (ms(1), Some(0.0), true),
        ] {
            b.record(t, took, v, sleeps);
            assert!(!b.due(t + ms(9_999)));
            assert!(b.due(t + IDLE_RECHECK));
        }
        b.record(t, ms(2), Some(0.0), false);
        assert!(b.due(t), "a fast read ends the back-off");
    }

    #[test]
    fn runtime_power_state() {
        let t = tempfile::tempdir().unwrap();
        let dev = t.path().join("0000:01:00.0");
        fs::create_dir_all(dev.join("power")).unwrap();
        assert!(!suspended(&dev) && !can_sleep(&dev), "no power files");
        fs::write(dev.join("power/runtime_status"), "active\n").unwrap();
        fs::write(dev.join("power/control"), "auto\n").unwrap();
        fs::write(dev.join("power/runtime_suspended_time"), "0\n").unwrap();
        assert!(!suspended(&dev));
        assert!(!can_sleep(&dev), "allowed to suspend but never has");
        fs::write(dev.join("power/runtime_suspended_time"), "81234\n").unwrap();
        assert!(can_sleep(&dev));
        fs::write(dev.join("power/control"), "on\n").unwrap();
        assert!(!can_sleep(&dev), "runtime power management switched off");
        fs::write(dev.join("power/runtime_status"), "suspended\n").unwrap();
        assert!(suspended(&dev));

        for id in ["00000000:01:00.0", "0000:01:00.0"] {
            assert_eq!(pci_device_dir(t.path(), id), Some(dev.clone()), "{id}");
        }
        assert_eq!(pci_device_dir(t.path(), "0000:02:00.0"), None);
        assert_eq!(pci_device_dir(t.path(), "garbage"), None);
    }
}
