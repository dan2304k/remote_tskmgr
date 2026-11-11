//! `load`: 1-minute load average (Linux/macOS). Not built on Windows, where sysinfo has no
//! load average; `Collectors::new` omits it there.

use crate::util::{detail, num, round};
use crate::Collector;
use rtm_protocol::Metric;
use std::time::Duration;
use sysinfo::System;

pub struct LoadCollector {
    cores: f64,
}

impl LoadCollector {
    pub fn new() -> Option<Self> {
        if cfg!(windows) {
            return None;
        }
        System::load_average().ok()?;
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as f64;
        Some(Self { cores })
    }
}

impl Collector for LoadCollector {
    fn id(&self) -> &str {
        "load"
    }

    fn sample(&mut self, _dt: Duration) -> Option<Metric> {
        let la = System::load_average().ok();
        let mut d = detail();
        if let Some(la) = &la {
            d.insert("load5".into(), num(round(la.five, 2)));
            d.insert("load15".into(), num(round(la.fifteen, 2)));
        }
        Some(Metric {
            v: la.map(|l| round(l.one, 2)),
            unit: String::new(),
            label: "LOAD".into(),
            min: Some(0.0),
            max: Some(self.cores),
            detail: Some(d),
        })
    }
}
