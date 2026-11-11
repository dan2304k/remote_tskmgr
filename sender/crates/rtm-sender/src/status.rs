//! A snapshot of what the daemon loop is doing, published after every tick and reload. Only the
//! `gui` feature reads it; the headless build pays for one uncontended lock per tick.
#![cfg_attr(not(feature = "gui"), allow(dead_code))]

use parking_lot::RwLock;
use std::sync::Arc;

pub type SharedStatus = Arc<RwLock<Status>>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// Has the loop published at least once?
    pub running: bool,
    /// Effective host name in the packets.
    pub host: String,
    pub interval_ms: u64,
    /// `rtm_net::Sender::describe()`, or why there is no sender.
    pub targets: String,
    /// Number of configured targets (broadcast counts as one).
    pub target_count: usize,
    pub paused: bool,
    /// Sequence number of the most recent packet built.
    pub seq: u64,
    /// Packets successfully handed to the network since start.
    pub sent: u64,
    /// Size of the last packet sent.
    pub last_bytes: Option<usize>,
    /// Last send/setup error; cleared when sending recovers.
    pub last_error: Option<String>,
    /// The daemon loop panicked (unwinding builds only; release aborts the process).
    /// `last_error` then reads [`DAEMON_CRASHED`].
    pub crashed: bool,
}

/// `last_error` after the daemon loop panicked.
pub const DAEMON_CRASHED: &str = "daemon stopped unexpectedly";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty_and_update_is_visible() {
        let s: SharedStatus = Default::default();
        assert_eq!(*s.read(), Status::default());
        assert!(!s.read().running && !s.read().paused && s.read().last_bytes.is_none());
        let other = s.clone();
        {
            let mut w = other.write();
            w.running = true;
            w.seq = 5;
            w.last_bytes = Some(321);
            w.last_error = Some("boom".into());
        }
        let r = s.read();
        assert!(r.running);
        assert_eq!((r.seq, r.last_bytes), (5, Some(321)));
        assert_eq!(r.last_error.as_deref(), Some("boom"));
    }
}
