//! The daemon loop: fixed-rate ticks, sample, build packet, send. Control messages (shutdown,
//! reloaded config) arrive on a channel and wake the loop immediately.

use crate::status::{SharedStatus, Status};
use anyhow::Context;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use rtm_collect::{CollectorConfig, Collectors};
use rtm_config::Config;
use rtm_net::Target;
use rtm_protocol::{Metric, Packet, PACKET_WARN_BYTES, PROTOCOL_VERSION};
use std::time::{Duration, Instant};

/// How often to retry building the network sender after it failed (e.g. no network yet at login).
const SENDER_RETRY: Duration = Duration::from_secs(10);
/// `--once` takes two samples this far apart so rate metrics are meaningful.
pub const ONCE_GAP: Duration = Duration::from_millis(200);

pub enum Control {
    Shutdown,
    /// Already merged with CLI overrides and validated.
    Reload(Box<Config>),
    /// Pause/resume sending (runtime only, never persisted). Sent by the `gui` feature.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    Pause(bool),
}

/// The per-packet header fields that come from config.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub id: String,
    pub host: String,
    pub os: String,
    pub interval_ms: u64,
    pub secret: Option<String>,
}

impl Meta {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            id: cfg.general.id.clone(),
            host: cfg.effective_host_name(),
            os: std::env::consts::OS.to_string(),
            interval_ms: cfg.general.interval_ms,
            secret: cfg.secret().map(str::to_string),
        }
    }
}

/// Assemble a packet; metrics keep their order. Signs when a secret is set.
pub fn build_packet(meta: &Meta, seq: u64, ts: u64, metrics: Vec<(String, Metric)>) -> Packet {
    let mut p = Packet {
        v: PROTOCOL_VERSION,
        id: meta.id.clone(),
        host: meta.host.clone(),
        os: meta.os.clone(),
        ts,
        seq,
        interval_ms: meta.interval_ms,
        metrics: Default::default(),
        auth: None,
        extra: Default::default(),
    };
    for (k, m) in metrics {
        p.set_metric(&k, m);
    }
    if let Some(s) = &meta.secret {
        p.sign(s);
    }
    p
}

/// Fixed-rate schedule: ticks are at `anchor + k * interval` for k >= 1. Returns the first
/// tick strictly after `now` and its index. Ticks that were missed (work overran) are skipped,
/// never bunched up.
pub fn next_tick(anchor: Instant, interval: Duration, now: Instant) -> (u64, Instant) {
    let interval = interval.max(Duration::from_millis(1));
    let elapsed = now.saturating_duration_since(anchor).as_nanos();
    let k = (elapsed / interval.as_nanos()) as u64 + 1;
    let at = anchor + interval * u32::try_from(k).unwrap_or(u32::MAX);
    (k, at)
}

/// What a reload changes, so the loop only rebuilds what it has to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub interval: bool,
    pub targets: bool,
    pub collectors: bool,
    pub meta: bool,
}

pub fn diff(old: &Config, new: &Config) -> Changes {
    let cc = |c: &Config| {
        let x = c.to_collector_config();
        (x.enabled, x.gpu_backend, x.gpu_adapter, x.disk_include)
    };
    Changes {
        interval: old.general.interval_ms != new.general.interval_ms,
        // Compared without DNS: a host name that resolves differently is picked up by
        // `State::apply` re-resolving whenever a target was skipped.
        targets: old.network != new.network && old.target_plan().ok() != new.target_plan().ok(),
        collectors: cc(old) != cc(new),
        meta: Meta::from_config(old) != Meta::from_config(new),
    }
}

fn make_sender(targets: &[Target]) -> Option<rtm_net::Sender> {
    if targets.is_empty() {
        tracing::warn!(
            "no targets configured (broadcast off, no unicast); sampling without sending"
        );
        return None;
    }
    match rtm_net::Sender::new(targets.to_vec()) {
        Ok(s) => {
            tracing::info!("sending to {}", s.describe());
            Some(s)
        }
        Err(e) => {
            tracing::error!("cannot set up network sender: {e}; retrying every {SENDER_RETRY:?}");
            None
        }
    }
}

/// `to_targets`, logging each skipped host name; the flag says whether any was skipped.
fn resolve(cfg: &Config) -> anyhow::Result<(Vec<Target>, bool)> {
    let r = cfg.resolve_targets()?;
    for (spec, e) in &r.unresolved {
        tracing::warn!("target {spec} skipped ({e}); will retry on the next config reload");
    }
    Ok((r.targets, !r.unresolved.is_empty()))
}

fn prime(collectors: &mut Collectors, nominal: Duration) {
    // First read establishes counter baselines; its rates are discarded.
    let _ = collectors.sample_all(nominal);
}

fn interval_of(cfg: &Config) -> Duration {
    Duration::from_millis(cfg.general.interval_ms)
}

/// `--once`: two samples `ONCE_GAP` apart, returns the packet JSON. Sends nothing.
pub fn run_once(cfg: &Config) -> anyhow::Result<String> {
    let mut collectors = Collectors::new(&cfg.to_collector_config());
    prime(&mut collectors, ONCE_GAP);
    let t0 = Instant::now();
    std::thread::sleep(ONCE_GAP);
    let metrics = collectors.sample_all(t0.elapsed());
    let p = build_packet(&Meta::from_config(cfg), 0, rtm_protocol::now_ms(), metrics);
    p.validate().context("built packet failed validation")?;
    Ok(p.to_json()?)
}

struct State {
    cfg: Config,
    meta: Meta,
    collector_cfg: CollectorConfig,
    collectors: Collectors,
    targets: Vec<Target>,
    /// Some unicast host name did not resolve; retried on the next reload.
    unresolved: bool,
    sender: Option<rtm_net::Sender>,
    last_sender_attempt: Instant,
    seq: u64,
    last_sample: Instant,
    warned_size: bool,
    send_ok: bool,
    /// While paused the loop keeps ticking and sampling (so counter baselines stay fresh and
    /// the first packet after resuming has correct rates) but sends nothing.
    paused: bool,
    last_bytes: Option<usize>,
    last_error: Option<String>,
    sent: u64,
    status: Option<SharedStatus>,
}

impl State {
    fn new(cfg: Config) -> anyhow::Result<Self> {
        let (targets, unresolved) = resolve(&cfg)?;
        let collector_cfg = cfg.to_collector_config();
        let mut collectors = Collectors::new(&collector_cfg);
        tracing::info!(metrics = ?collectors.ids(), "collectors ready");
        prime(&mut collectors, interval_of(&cfg));
        Ok(Self {
            meta: Meta::from_config(&cfg),
            sender: make_sender(&targets),
            last_sender_attempt: Instant::now(),
            targets,
            unresolved,
            collector_cfg,
            collectors,
            cfg,
            seq: 0,
            last_sample: Instant::now(),
            warned_size: false,
            send_ok: true,
            paused: false,
            last_bytes: None,
            last_error: None,
            sent: 0,
            status: None,
        })
    }

    /// Copy the loop's view into the shared `Status` (no-op without a GUI).
    fn publish(&self) {
        let Some(status) = &self.status else { return };
        let targets = match &self.sender {
            Some(s) => s.describe(),
            None if self.targets.is_empty() => "no targets configured".into(),
            None => "network sender unavailable (retrying)".into(),
        };
        *status.write() = Status {
            running: true,
            host: self.meta.host.clone(),
            interval_ms: self.cfg.general.interval_ms,
            targets,
            target_count: self.targets.len(),
            paused: self.paused,
            seq: self.seq.saturating_sub(1),
            sent: self.sent,
            last_bytes: self.last_bytes,
            last_error: self.last_error.clone(),
        };
    }

    /// Returns true when the tick schedule must be re-anchored.
    fn apply(&mut self, new: Config) -> bool {
        let mut ch = diff(&self.cfg, &new);
        if self.unresolved && !ch.targets {
            tracing::info!("retrying host names that did not resolve");
            ch.targets = true;
        }
        if ch == Changes::default() {
            tracing::debug!("config file changed but nothing relevant differs");
            self.cfg = new;
            return false;
        }
        tracing::info!(?ch, "applying reloaded config");
        if ch.targets {
            match resolve(&new) {
                Ok((t, unresolved)) => {
                    self.sender = None; // drop old sockets / TCP threads first
                    self.targets = t;
                    self.unresolved = unresolved;
                    self.sender = make_sender(&self.targets);
                    self.last_sender_attempt = Instant::now();
                }
                Err(e) => tracing::error!("targets invalid, keeping old ones: {e}"),
            }
        }
        if ch.collectors {
            self.collector_cfg = new.to_collector_config();
            self.collectors = Collectors::new(&self.collector_cfg);
            prime(&mut self.collectors, interval_of(&new));
            self.last_sample = Instant::now();
            tracing::info!(metrics = ?self.collectors.ids(), "collectors rebuilt");
        }
        self.meta = Meta::from_config(&new);
        self.cfg = new;
        ch.interval
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_sample);
        self.last_sample = now;
        let metrics = self.collectors.sample_all(dt);
        let packet = build_packet(&self.meta, self.seq, rtm_protocol::now_ms(), metrics);
        self.seq += 1;
        if self.paused {
            return;
        }

        if self.sender.is_none()
            && !self.targets.is_empty()
            && self.last_sender_attempt.elapsed() >= SENDER_RETRY
        {
            self.last_sender_attempt = Instant::now();
            self.sender = make_sender(&self.targets);
        }
        let Some(sender) = self.sender.as_mut() else {
            tracing::trace!(seq = packet.seq, "no sender; packet dropped");
            return;
        };
        match sender.send(&packet) {
            Ok(bytes) => {
                if !self.send_ok {
                    tracing::info!("sending recovered");
                    self.send_ok = true;
                }
                self.sent += 1;
                self.last_bytes = Some(bytes);
                self.last_error = None;
                if bytes > PACKET_WARN_BYTES && !self.warned_size {
                    tracing::warn!(
                        bytes,
                        "packet exceeds {PACKET_WARN_BYTES} bytes and may fragment; disable some metrics"
                    );
                    self.warned_size = true;
                }
                tracing::debug!(seq = packet.seq, bytes, "sent");
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
                if self.send_ok {
                    tracing::warn!("send failed: {e}");
                    self.send_ok = false;
                } else {
                    tracing::debug!("send failed: {e}");
                }
            }
        }
    }
}

/// Run until `Control::Shutdown` (or every control sender is dropped).
pub fn run(cfg: Config, control: Receiver<Control>) -> anyhow::Result<()> {
    run_with_status(cfg, control, None)
}

/// `run`, additionally publishing a `Status` snapshot after every tick and control message.
pub fn run_with_status(
    cfg: Config,
    control: Receiver<Control>,
    status: Option<SharedStatus>,
) -> anyhow::Result<()> {
    let mut st = State::new(cfg)?;
    st.status = status;
    st.publish();
    let mut interval = interval_of(&st.cfg);
    let mut anchor = Instant::now();
    let (mut k, mut deadline) = next_tick(anchor, interval, anchor);
    tracing::info!(
        host = %st.meta.host,
        id = %st.meta.id,
        interval_ms = st.cfg.general.interval_ms,
        auth = st.meta.secret.is_some(),
        "rtm-sender running"
    );
    loop {
        match control.recv_deadline(deadline) {
            Ok(Control::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Control::Reload(new)) => {
                if st.apply(*new) {
                    interval = interval_of(&st.cfg);
                    anchor = Instant::now();
                    (k, deadline) = next_tick(anchor, interval, anchor);
                    tracing::info!(interval_ms = st.cfg.general.interval_ms, "interval changed");
                }
                st.publish();
                continue;
            }
            Ok(Control::Pause(p)) => {
                if st.paused != p {
                    tracing::info!(
                        paused = p,
                        "sending {}",
                        if p { "paused" } else { "resumed" }
                    );
                }
                st.paused = p;
                st.publish();
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() < deadline {
            continue; // early wake-up; wait out the rest
        }
        st.tick();
        st.publish();
        let (nk, nd) = next_tick(anchor, interval, Instant::now());
        if nk > k + 1 {
            tracing::debug!(skipped = nk - k - 1, "tick overran; skipping missed ticks");
        }
        (k, deadline) = (nk, nd);
    }
    tracing::info!(packets = st.seq, "shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(secret: Option<&str>) -> Meta {
        Meta {
            id: "id-1".into(),
            host: "box".into(),
            os: "linux".into(),
            interval_ms: 1000,
            secret: secret.map(str::to_string),
        }
    }

    fn sample() -> Vec<(String, Metric)> {
        vec![
            ("mem".into(), Metric::percent("MEM", Some(40.0))),
            ("cpu".into(), Metric::percent("CPU", Some(12.5))),
            ("gpu".into(), Metric::percent("GPU", None)),
            ("disk".into(), Metric::rate_mbps("DISK", Some(1.5))),
        ]
    }

    #[test]
    fn packet_from_metrics_keeps_order_and_header() {
        let p = build_packet(&meta(None), 7, 1234, sample());
        assert_eq!(p.v, 1);
        assert_eq!(
            (p.id.as_str(), p.host.as_str(), p.os.as_str()),
            ("id-1", "box", "linux")
        );
        assert_eq!((p.seq, p.ts, p.interval_ms), (7, 1234, 1000));
        assert!(p.auth.is_none());
        let keys: Vec<&str> = p.metrics.keys().map(String::as_str).collect();
        assert_eq!(keys, ["mem", "cpu", "gpu", "disk"]);
        assert!(p.metrics["gpu"]["v"].is_null());
        let back = Packet::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(back, p);
        assert!(p.to_json().unwrap().len() < 1200);
    }

    #[test]
    fn packet_signed_when_secret_set() {
        let p = build_packet(&meta(Some("k")), 1, 1, sample());
        assert!(p.verify("k"));
        assert!(!p.verify("other"));
    }

    #[test]
    fn empty_metrics_is_valid() {
        let p = build_packet(&meta(None), 0, 0, vec![]);
        p.validate().unwrap();
        assert!(p.to_json().unwrap().contains(r#""metrics":{}"#));
    }

    #[test]
    fn meta_from_config() {
        let mut c = Config::default();
        c.general.id = "abc".into();
        c.general.host_name = "named".into();
        c.general.interval_ms = 5000;
        c.network.shared_secret = "s".into();
        let m = Meta::from_config(&c);
        assert_eq!(m.id, "abc");
        assert_eq!(m.host, "named");
        assert_eq!(m.os, std::env::consts::OS);
        assert_eq!(m.interval_ms, 5000);
        assert_eq!(m.secret.as_deref(), Some("s"));
    }

    #[test]
    fn schedule_is_fixed_rate() {
        let a = Instant::now();
        let s = Duration::from_secs(1);
        let ms = Duration::from_millis;
        assert_eq!(next_tick(a, s, a), (1, a + s));
        assert_eq!(next_tick(a, s, a + ms(999)), (1, a + s));
        // Woke exactly on / slightly after tick 1, work took 300 ms: next is still a+2s.
        assert_eq!(next_tick(a, s, a + ms(1000)), (2, a + ms(2000)));
        assert_eq!(next_tick(a, s, a + ms(1300)), (2, a + ms(2000)));
        // Overran past tick 3: skip to 4, do not drift or burst.
        assert_eq!(next_tick(a, s, a + ms(3500)), (4, a + ms(4000)));
        // No drift over many ticks.
        let (k, t) = next_tick(a, s, a + ms(86_400_000 + 17));
        assert_eq!(k, 86_401);
        assert_eq!(t, a + Duration::from_secs(86_401));
        // `now` before anchor (clock jitter) yields the first tick.
        assert_eq!(next_tick(a + s, s, a), (1, a + s + s));
        // Zero interval does not divide by zero.
        let _ = next_tick(a, Duration::ZERO, a + ms(5));
    }

    #[test]
    fn diff_detects_changes() {
        let a = Config::default();
        assert_eq!(diff(&a, &a.clone()), Changes::default());

        let mut b = a.clone();
        b.general.interval_ms = 5000;
        let d = diff(&a, &b);
        assert!(d.interval && d.meta && !d.targets && !d.collectors);

        let mut b = a.clone();
        b.network.tcp_unicast = vec!["10.0.0.1:48000".into()];
        assert_eq!(
            diff(&a, &b),
            Changes {
                targets: true,
                ..Default::default()
            }
        );

        let mut b = a.clone();
        b.network.shared_secret = "x".into();
        assert_eq!(
            diff(&a, &b),
            Changes {
                meta: true,
                ..Default::default()
            }
        );

        let mut b = a.clone();
        b.general.host_name = "renamed".into();
        assert_eq!(
            diff(&a, &b),
            Changes {
                meta: true,
                ..Default::default()
            }
        );

        let mut b = a.clone();
        b.metrics.enabled.push("net".into());
        assert_eq!(
            diff(&a, &b),
            Changes {
                collectors: true,
                ..Default::default()
            }
        );

        let mut b = a.clone();
        b.gui.autostart = true;
        assert_eq!(diff(&a, &b), Changes::default());
    }

    #[test]
    fn loop_publishes_status_and_pause_stops_sending() {
        use std::net::UdpSocket;
        let rx_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx_sock
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut cfg = Config::default();
        cfg.general.id = "t".into();
        cfg.general.host_name = "looptest".into();
        cfg.general.interval_ms = 250;
        cfg.network.udp_broadcast = false;
        cfg.network.udp_unicast = vec![rx_sock.local_addr().unwrap().to_string()];
        cfg.metrics.enabled = vec!["mem".into()];

        let status: SharedStatus = Default::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let st = status.clone();
        let h = std::thread::spawn(move || run_with_status(cfg, rx, Some(st)));
        let mut buf = [0u8; 2048];
        let count = |ms: u64, buf: &mut [u8]| {
            let end = Instant::now() + Duration::from_millis(ms);
            let mut n = 0;
            while Instant::now() < end {
                if rx_sock.recv(buf).is_ok() {
                    n += 1;
                }
            }
            n
        };

        assert!(count(900, &mut buf) >= 2, "packets arrive while running");
        {
            let s = status.read();
            assert!(s.running && !s.paused);
            assert_eq!(s.host, "looptest");
            assert_eq!((s.interval_ms, s.target_count), (250, 1));
            assert!(s.sent >= 2 && s.last_bytes.is_some_and(|b| b > 50));
            assert!(s.targets.contains("UDP"), "{}", s.targets);
            assert!(s.last_error.is_none());
        }

        tx.send(Control::Pause(true)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        while rx_sock.recv(&mut buf).is_ok() {} // drain anything in flight
        let sent_at_pause = status.read().sent;
        assert_eq!(count(700, &mut buf), 0, "nothing sent while paused");
        assert!(status.read().paused);
        assert_eq!(status.read().sent, sent_at_pause);
        assert!(status.read().seq > 0, "still sampling");

        tx.send(Control::Pause(false)).unwrap();
        assert!(count(700, &mut buf) >= 1, "resumes");
        tx.send(Control::Shutdown).unwrap();
        h.join().unwrap().unwrap();
    }
}
