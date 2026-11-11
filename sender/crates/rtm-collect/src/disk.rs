//! `disk`: read + write throughput in MB/s summed over physical disks.
//!
//! sysinfo lists mounted filesystems. We drop pseudo / virtual filesystems, dedupe by device
//! name (bind mounts and btrfs subvolumes show the same device several times; on Windows,
//! where the "name" is the volume label, by mount point instead), and difference
//! the cumulative `total_read_bytes` / `total_written_bytes` counters ourselves. A device seen
//! for the first time contributes 0 on that tick, so the first sample after construction (and
//! after a disk is hot-plugged) is 0, never a spike; deltas use `saturating_sub` so a counter
//! reset can never yield a negative rate.

use crate::util::{detail, mbps, num, round, Hold, Stopwatch, MIN_WINDOW};
use crate::Collector;
use rtm_protocol::Metric;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use sysinfo::{Disk, DiskRefreshKind, Disks};

/// Filesystems that never correspond to a physical block device.
const PSEUDO_FS: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "ramfs",
    "overlay",
    "overlayfs",
    "squashfs",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "devpts",
    "autofs",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "nsfs",
    "efivarfs",
    "tracefs",
    "debugfs",
    "zram",
];

/// Most per-disk entries put in `detail.disks` (busiest first, then by name) so a host with
/// many disks stays under the 1200-byte packet target. The `v` total covers every disk.
const MAX_DETAIL_DISKS: usize = 4;

pub struct DiskCollector {
    disks: Disks,
    include: Vec<String>,
    /// key -> (total_read, total_written) at the previous tick.
    prev: HashMap<String, (u64, u64)>,
    clock: Stopwatch,
    /// Last reading, repeated for ticks closer than `MIN_WINDOW` to it.
    hold: Hold<Metric>,
}

fn refresh_kind() -> DiskRefreshKind {
    DiskRefreshKind::nothing().with_io_usage()
}

/// Display / dedupe key for a disk: device name without `/dev/`, or the mount point when the
/// platform gives no name. On Windows always the mount point (`C:`): sysinfo's name there is
/// the volume label, and two volumes labelled e.g. "Data" must not collapse into one.
pub(crate) fn disk_key(name: &str, mount: &str) -> String {
    disk_key_for(name, mount, cfg!(windows))
}

pub(crate) fn disk_key_for(name: &str, mount: &str, by_mount: bool) -> String {
    let m = mount.trim().trim_end_matches(['\\', '/']);
    if by_mount && !m.is_empty() {
        return m.to_string();
    }
    let n = name.trim();
    let n = n.strip_prefix("/dev/").unwrap_or(n);
    let n = n.strip_prefix("mapper/").unwrap_or(n);
    if n.is_empty() {
        m.to_string()
    } else {
        n.to_string()
    }
}

pub(crate) fn is_pseudo(fs: &str, name: &str) -> bool {
    let fs = fs.to_ascii_lowercase();
    PSEUDO_FS.contains(&fs.as_str())
        || name.starts_with("/dev/loop")
        || name.starts_with("/dev/ram")
        || name.starts_with("/dev/zram")
}

pub(crate) fn included(include: &[String], key: &str, name: &str, mount: &str) -> bool {
    include.is_empty()
        || include
            .iter()
            .any(|i| i == key || i == name || i == mount || i.trim_start_matches("/dev/") == key)
}

impl DiskCollector {
    pub fn new(include: &[String]) -> Option<Self> {
        let disks = Disks::new_with_refreshed_list_specifics(
            DiskRefreshKind::nothing().with_kind().with_io_usage(),
        )
        .ok()?;
        let mut me = Self {
            disks,
            include: include.to_vec(),
            prev: HashMap::new(),
            clock: Stopwatch::new(),
            hold: Hold::new(),
        };
        // Baseline the counters so the first real sample has something to diff against.
        let totals = me.totals();
        me.prev = totals.into_iter().collect();
        Some(me)
    }

    fn totals(&self) -> BTreeMap<String, (u64, u64)> {
        let mut out = BTreeMap::new();
        for d in self.disks.iter() {
            if let Some((k, r, w)) = self.disk_totals(d) {
                out.entry(k).or_insert((r, w));
            }
        }
        out
    }

    fn disk_totals(&self, d: &Disk) -> Option<(String, u64, u64)> {
        let name = d.name().to_string_lossy();
        let fs = d.file_system().to_string_lossy();
        let mount = d.mount_point().to_string_lossy();
        if is_pseudo(&fs, &name) {
            return None;
        }
        let key = disk_key(&name, &mount);
        if key.is_empty() || !included(&self.include, &key, &name, &mount) {
            return None;
        }
        let u = d.usage();
        Some((key, u.total_read_bytes, u.total_written_bytes))
    }
}

impl Collector for DiskCollector {
    fn id(&self) -> &str {
        "disk"
    }

    fn sample(&mut self, dt: Duration) -> Option<Metric> {
        // Below MIN_WINDOW since the last reading, repeat it: the counters keep accumulating
        // and the next real reading covers the whole interval (no spikes at 10/s).
        let now = std::time::Instant::now();
        if let Some(m) = self.hold.recent(now, MIN_WINDOW) {
            return Some(m);
        }
        // `true` drops unmounted disks; new mounts are picked up by the list refresh.
        self.disks.refresh_specifics(true, refresh_kind());
        let secs = self.clock.lap(dt);
        let totals = self.totals();

        let mut sum_r = 0u64;
        let mut sum_w = 0u64;
        let mut per: Vec<(String, f64, f64)> = Vec::new();
        for (k, (r, w)) in &totals {
            let (dr, dw) = match self.prev.get(k) {
                Some((pr, pw)) => (r.saturating_sub(*pr), w.saturating_sub(*pw)),
                None => (0, 0),
            };
            sum_r += dr;
            sum_w += dw;
            per.push((k.clone(), mbps(dr, secs), mbps(dw, secs)));
        }
        self.prev = totals.into_iter().collect();

        let read = round(mbps(sum_r, secs), 2);
        let write = round(mbps(sum_w, secs), 2);

        // Busiest first for the cap, then back to name order for a stable display.
        per.sort_by(|a, b| (b.1 + b.2).total_cmp(&(a.1 + a.2)).then(a.0.cmp(&b.0)));
        per.truncate(MAX_DETAIL_DISKS);
        per.sort_by(|a, b| a.0.cmp(&b.0));
        let mut disks = Map::new();
        for (k, r, w) in per {
            let mut e = Map::new();
            e.insert("read_mbps".into(), num(round(r, 2)));
            e.insert("write_mbps".into(), num(round(w, 2)));
            disks.insert(k, Value::Object(e));
        }

        let mut d = detail();
        d.insert("read_mbps".into(), num(read));
        d.insert("write_mbps".into(), num(write));
        d.insert("disks".into(), Value::Object(disks));
        let m = Metric::rate_mbps("DISK", Some(round(read + write, 2))).with_detail(d);
        self.hold.store(now, m.clone());
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_filters() {
        #[cfg(not(windows))]
        assert_eq!(disk_key("/dev/nvme0n1p2", "/boot"), "nvme0n1p2");
        assert_eq!(disk_key_for("/dev/mapper/vg-root", "/", false), "vg-root");
        assert_eq!(disk_key_for("", "C:\\", false), "C:");
        // Windows: two volumes with the same label stay apart; label-less ones too.
        assert_eq!(disk_key_for("Data", "D:\\", true), "D:");
        assert_eq!(disk_key_for("Data", "E:\\", true), "E:");
        assert_eq!(disk_key_for("", "C:\\", true), "C:");
        assert_eq!(disk_key_for("Data", "", true), "Data");
        assert!(is_pseudo("tmpfs", "tmpfs"));
        assert!(is_pseudo("squashfs", "/dev/loop3"));
        assert!(!is_pseudo("ext4", "/dev/sda1"));
        let inc = vec!["sda1".to_string()];
        assert!(included(&inc, "sda1", "/dev/sda1", "/x"));
        assert!(!included(&inc, "sdb1", "/dev/sdb1", "/y"));
        assert!(included(&[], "sdb1", "/dev/sdb1", "/y"));
        let inc = vec!["/dev/sda1".to_string()];
        assert!(included(&inc, "sda1", "/dev/sda1", "/x"));
    }
}
