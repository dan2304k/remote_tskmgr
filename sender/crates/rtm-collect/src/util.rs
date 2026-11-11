//! Small helpers shared by the collectors.

use serde_json::{Map, Value};
use std::time::{Duration, Instant};

/// Round to `places` decimals. Non-finite input becomes 0 so it never reaches the wire as
/// something serde_json would refuse.
pub(crate) fn round(v: f64, places: i32) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let f = 10f64.powi(places);
    (v * f).round() / f
}

pub(crate) fn num(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

pub(crate) const MIB: u64 = 1024 * 1024;
/// `MB/s` on the wire is 10^6 bytes per second (PROTOCOL.md).
pub(crate) const MB: f64 = 1_000_000.0;

pub(crate) fn mib(bytes: u64) -> Value {
    Value::from(bytes / MIB)
}

/// Bytes delta over `secs` as MB/s, never negative, never NaN.
pub(crate) fn mbps(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        0.0
    } else {
        bytes as f64 / MB / secs
    }
}

/// Measures the real wall time between counter reads. Rate collectors divide counter deltas
/// by this rather than by the caller's `dt`, so a late or early tick (or a first call with
/// `dt == 0`) still yields a correct rate.
pub(crate) struct Stopwatch {
    last: Instant,
}

impl Stopwatch {
    pub(crate) fn new() -> Self {
        Self {
            last: Instant::now(),
        }
    }
    /// Seconds since the previous lap (or construction). Falls back to `dt` if the clock
    /// reports ~0 elapsed.
    pub(crate) fn lap(&mut self, dt: Duration) -> f64 {
        let now = Instant::now();
        let secs = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        if secs > 1e-3 {
            secs
        } else {
            dt.as_secs_f64()
        }
    }
}

/// Shortest interval a rate or CPU reading is computed over. Faster ticks (100 ms = 10/s)
/// repeat the last reading instead of computing a noisy (or, for CPU, invalid) one.
pub(crate) const MIN_WINDOW: Duration = sysinfo::MINIMUM_CPU_UPDATE_INTERVAL;

/// The last computed value and when it was computed.
pub(crate) struct Hold<T> {
    last: Option<(Instant, T)>,
}

impl<T: Clone> Hold<T> {
    pub(crate) fn new() -> Self {
        Self { last: None }
    }
    /// The held value if it was stored less than `min` before `now`.
    pub(crate) fn recent(&self, now: Instant, min: Duration) -> Option<T> {
        self.last
            .as_ref()
            .filter(|(t, _)| now.saturating_duration_since(*t) < min)
            .map(|(_, v)| v.clone())
    }
    pub(crate) fn store(&mut self, now: Instant, v: T) {
        self.last = Some((now, v));
    }
}

pub(crate) fn detail() -> Map<String, Value> {
    Map::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_repeats_within_window() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        let mut h = Hold::new();
        assert_eq!(h.recent(t, ms(200)), None);
        h.store(t, 7);
        assert_eq!(h.recent(t + ms(100), ms(200)), Some(7));
        assert_eq!(h.recent(t + ms(199), ms(200)), Some(7));
        assert_eq!(h.recent(t + ms(200), ms(200)), None);
    }

    #[test]
    fn rounding() {
        assert_eq!(round(1.26, 1), 1.3);
        assert_eq!(round(f64::NAN, 1), 0.0);
        assert_eq!(mbps(2_000_000, 2.0), 1.0);
        assert_eq!(mbps(5, 0.0), 0.0);
    }
}
