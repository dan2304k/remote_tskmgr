//! `mem`: used / total RAM in percent.

use crate::util::{detail, mib, round};
use crate::Collector;
use rtm_protocol::Metric;
use std::time::Duration;
use sysinfo::{MemoryRefreshKind, System};

pub struct MemCollector {
    sys: System,
}

impl MemCollector {
    pub fn new() -> Option<Self> {
        let sys = System::new().ok()?;
        Some(Self { sys })
    }
}

impl Collector for MemCollector {
    fn id(&self) -> &str {
        "mem"
    }

    fn sample(&mut self, _dt: Duration) -> Option<Metric> {
        self.sys
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
        let total = self.sys.total_memory();
        let used = self.sys.used_memory().min(total);
        let v = (total > 0).then(|| round(used as f64 / total as f64 * 100.0, 1));
        let mut d = detail();
        d.insert("used_mb".into(), mib(used));
        d.insert("total_mb".into(), mib(total));
        d.insert("swap_used_mb".into(), mib(self.sys.used_swap()));
        d.insert("swap_total_mb".into(), mib(self.sys.total_swap()));
        Some(Metric::percent("MEM", v).with_detail(d))
    }
}
