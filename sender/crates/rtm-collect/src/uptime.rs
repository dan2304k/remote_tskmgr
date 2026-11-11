//! `uptime`: seconds since boot.

use crate::Collector;
use rtm_protocol::Metric;
use std::time::Duration;
use sysinfo::System;

pub struct UptimeCollector;

impl UptimeCollector {
    pub fn new() -> Option<Self> {
        System::uptime().ok().map(|_| Self)
    }
}

impl Collector for UptimeCollector {
    fn id(&self) -> &str {
        "uptime"
    }

    fn sample(&mut self, _dt: Duration) -> Option<Metric> {
        let v = System::uptime().ok().map(|s| s as f64);
        Some(Metric {
            v,
            unit: "s".into(),
            label: "UP".into(),
            min: Some(0.0),
            max: None,
            detail: None,
        })
    }
}
