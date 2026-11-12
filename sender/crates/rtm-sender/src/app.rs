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

/// The per-packet header fields that come from config, plus `general.privacy`.
/// `Debug` is written by hand so the secret never reaches a log line.
#[derive(Clone, PartialEq)]
pub struct Meta {
    pub id: String,
    pub host: String,
    pub os: String,
    pub interval_ms: u64,
    pub secret: Option<String>,
    /// Replace device names in metric details (see `privacy`).
    pub privacy: bool,
}

impl std::fmt::Debug for Meta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meta")
            .field("id", &self.id)
            .field("host", &self.host)
            .field("os", &self.os)
            .field("interval_ms", &self.interval_ms)
            .field(
                "secret",
                &self.secret.as_ref().map(|_| rtm_config::SECRET_MASK),
            )
            .field("privacy", &self.privacy)
            .finish()
    }
}

impl Meta {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            id: cfg.general.id.clone(),
            host: cfg.effective_host_name(),
            os: std::env::consts::OS.to_string(),
            interval_ms: cfg.general.interval_ms,
            secret: cfg.secret().map(str::to_string),
            privacy: cfg.general.privacy,
        }
    }
}

/// Packet timestamps: wall-clock milliseconds, but strictly increasing. Viewers with a shared
/// secret reject a `ts` that is not greater than the previous one from the same sender id, so
/// a clock stepped backwards (NTP, manual change) or two packets in the same millisecond must
/// still yield a larger value: `max(now, last + 1)`.
pub struct TsClock {
    now: Box<dyn FnMut() -> u64 + Send>,
    last: Option<u64>,
}

impl TsClock {
    /// The system clock (`rtm_protocol::now_ms`).
    pub fn system() -> Self {
        Self::with_clock(rtm_protocol::now_ms)
    }

    /// An injected clock (tests).
    pub fn with_clock(now: impl FnMut() -> u64 + Send + 'static) -> Self {
        Self {
            now: Box::new(now),
            last: None,
        }
    }

    /// The `ts` for the next packet.
    pub fn next(&mut self) -> u64 {
        let now = (self.now)();
        let ts = match self.last {
            Some(last) => now.max(last.saturating_add(1)),
            None => now,
        };
        self.last = Some(ts);
        ts
    }
}

/// Assemble a packet; metrics keep their order. With `meta.privacy` device names are replaced
/// first. Signs when a secret is set. A packet that would exceed `PACKET_WARN_BYTES` loses
/// `detail` objects, largest first, until it fits.
pub fn build_packet(meta: &Meta, seq: u64, ts: u64, mut metrics: Vec<(String, Metric)>) -> Packet {
    if meta.privacy {
        crate::privacy::anonymise(&mut metrics);
    }
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
    if meta.secret.is_some() {
        // Same length as the real tag, so the size check counts it.
        p.auth = Some("0".repeat(64));
    }
    let dropped = trim_details(&mut p, PACKET_WARN_BYTES);
    if !dropped.is_empty() {
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(
                ?dropped,
                "packet over {PACKET_WARN_BYTES} bytes; dropped these metrics' detail (logged once)"
            );
        }
    }
    if let Some(s) = &meta.secret {
        p.sign(s);
    }
    p
}

fn packet_len(p: &Packet) -> usize {
    p.to_json().map_or(0, |s| s.len())
}

/// Last resort against UDP fragmentation: while the packet is over `limit` bytes, remove the
/// largest `detail` object. Returns the keys of the metrics that lost theirs.
pub fn trim_details(p: &mut Packet, limit: usize) -> Vec<String> {
    let mut dropped = Vec::new();
    while packet_len(p) > limit {
        let largest = p
            .metrics
            .iter()
            .filter_map(|(k, m)| {
                let d = m.get("detail")?;
                Some((d.to_string().len(), k.clone()))
            })
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let Some((_, key)) = largest else { break };
        if let Some(m) = p.metrics.get_mut(&key).and_then(|m| m.as_object_mut()) {
            m.remove("detail");
        }
        dropped.push(key);
    }
    dropped
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
        (
            x.enabled,
            x.gpu_backend,
            x.gpu_adapter,
            x.disk_include,
            x.net_skip_prefixes,
        )
    };
    Changes {
        interval: old.general.interval_ms != new.general.interval_ms,
        // Compared as written: host names are resolved (and re-resolved) by rtm-net itself.
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

/// The configured targets. No DNS here: host-name targets are resolved by rtm-net.
fn targets_of(cfg: &Config) -> anyhow::Result<Vec<Target>> {
    Ok(cfg.to_targets()?)
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
    sender: Option<rtm_net::Sender>,
    last_sender_attempt: Instant,
    seq: u64,
    ts: TsClock,
    last_sample: Instant,
    warned_size: bool,
    send_ok: bool,
    /// While paused the loop keeps ticking and sampling (so counter baselines stay fresh and
    /// the first packet after resuming has correct rates) but builds and sends nothing.
    paused: bool,
    last_bytes: Option<usize>,
    last_error: Option<String>,
    sent: u64,
    status: Option<SharedStatus>,
}

impl State {
    fn new(cfg: Config) -> anyhow::Result<Self> {
        let targets = targets_of(&cfg)?;
        let collector_cfg = cfg.to_collector_config();
        let mut collectors = Collectors::new(&collector_cfg);
        tracing::info!(metrics = ?collectors.ids(), "collectors ready");
        prime(&mut collectors, interval_of(&cfg));
        Ok(Self {
            meta: Meta::from_config(&cfg),
            sender: make_sender(&targets),
            last_sender_attempt: Instant::now(),
            targets,
            collector_cfg,
            collectors,
            cfg,
            seq: 0,
            ts: TsClock::system(),
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
            crashed: false,
        };
    }

    /// Returns true when the tick schedule must be re-anchored. If the new targets cannot be
    /// built, the old network settings stay current (so the same reload later retries them).
    fn apply(&mut self, mut new: Config) -> bool {
        let ch = diff(&self.cfg, &new);
        if ch == Changes::default() {
            tracing::debug!("config file changed but nothing relevant differs");
            self.cfg = new;
            return false;
        }
        tracing::info!(?ch, "applying reloaded config");
        if ch.targets {
            match targets_of(&new) {
                Ok(t) => {
                    self.sender = None; // drop old sockets / TCP threads first
                    self.targets = t;
                    self.sender = make_sender(&self.targets);
                    self.last_sender_attempt = Instant::now();
                }
                Err(e) => {
                    tracing::error!("targets invalid, keeping the old network settings: {e}");
                    new.network = self.cfg.network.clone();
                }
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
        if self.paused {
            // Sampled only to keep the rate baselines fresh. No packet is built, so `seq` does
            // not advance and the first packet after resuming follows the last one sent.
            return;
        }
        let packet = build_packet(&self.meta, self.seq, self.ts.next(), metrics);
        self.seq += 1;

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
            privacy: false,
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

    /// Worst case: 128 threads, 4 GPUs with long names, 6 sensors, 4 disks, signed.
    fn huge() -> Vec<(String, Metric)> {
        use rtm_protocol::serde_json::{json, Map, Value};
        let obj = |v: Value| -> Map<String, Value> { v.as_object().unwrap().clone() };
        let usages: Vec<f64> = (0..128).map(|i| f64::from((i * 37) % 101) + 0.5).collect();
        let mut cpu: Map<String, Value> = rtm_collect::cores_detail(&usages).into_iter().collect();
        cpu.insert("freq_mhz".into(), json!(4895));
        cpu.insert("temp_c".into(), json!(88.0));
        cpu.insert("load1".into(), json!(123.45));
        let adapters: Map<String, Value> = (0..4)
            .map(|i| {
                (
                    format!("#{i}"),
                    json!({"name": format!("NVIDIA GeForce RTX 4090 Laptop GPU number {i}"),
                           "vram_total_mb": 24564, "util": 99.9, "chosen": i == 0}),
                )
            })
            .collect();
        let gpu = obj(
            json!({"name": "NVIDIA GeForce RTX 4090 Laptop GPU number 0",
            "nvml_index": 0, "adapter_index": 0, "vram_used_mb": 12345, "vram_total_mb": 24564,
            "temp_c": 77.0, "freq_mhz": 2520, "backend": "nvml", "adapters": adapters,
            "engines": {"3D": 99.9, "Copy": 12.3, "VideoDecode": 45.6, "VideoEncode": 7.8,
                        "Compute_0": 55.5, "Compute_1": 66.6}}),
        );
        let sensors: Map<String, Value> = (0..6)
            .map(|i| (format!("coretemp Package id {i} sensor"), json!(80.5)))
            .collect();
        let disks: Map<String, Value> = (0..4)
            .map(|i| {
                (
                    format!("nvme{i}n1"),
                    json!({"read_mbps": 1234.56, "write_mbps": 987.65}),
                )
            })
            .collect();
        let mut temp = Metric::percent("TEMP", Some(88.0));
        temp.unit = "°C".into();
        vec![
            ("cpu".into(), Metric::percent("CPU", Some(55.5)).with_detail(cpu)),
            ("gpu".into(), Metric::percent("GPU", Some(99.9)).with_detail(gpu)),
            ("mem".into(), Metric::percent("MEM", Some(77.7)).with_detail(obj(json!(
                {"used_mb": 123456, "total_mb": 262144, "swap_used_mb": 1234, "swap_total_mb": 65536}
            )))),
            ("disk".into(), Metric::rate_mbps("DISK", Some(2222.21)).with_detail(obj(json!(
                {"read_mbps": 1234.56, "write_mbps": 987.65, "disks": disks}
            )))),
            ("net".into(), Metric::rate_mbps("NET", Some(117.5)).with_detail(obj(json!(
                {"rx_mbps": 100.25, "tx_mbps": 17.25}
            )))),
            ("temp".into(), temp.with_detail(obj(json!({"sensors": sensors})))),
        ]
    }

    #[test]
    fn huge_packet_stays_under_the_fragmentation_limit() {
        let mut m = meta(Some("secret"));
        m.host = "a-rather-long-host-name.example.internal".into();
        let p = build_packet(&m, u64::MAX, u64::MAX, huge());
        let len = p.to_json().unwrap().len();
        assert!(len <= PACKET_WARN_BYTES, "{len} bytes");
        assert!(p.verify("secret"), "signed after trimming");
        assert_eq!(p.metrics["cpu"]["detail"]["cores_n"], 128);
    }

    #[test]
    fn trim_drops_largest_detail_first() {
        use rtm_protocol::serde_json::{json, Map, Value};
        let big: Map<String, Value> = (0..200).map(|i| (format!("k{i}"), json!(i))).collect();
        let small: Map<String, Value> = [("a".to_string(), json!(1))].into_iter().collect();
        let mut v = sample();
        v[0].1 = v[0].1.clone().with_detail(small);
        v[1].1 = v[1].1.clone().with_detail(big);
        let p = build_packet(&meta(None), 1, 1, v);
        assert!(p.to_json().unwrap().len() <= PACKET_WARN_BYTES);
        assert!(p.metrics["cpu"].get("detail").is_none(), "largest dropped");
        assert_eq!(p.metrics["mem"]["detail"]["a"], 1, "small one kept");
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
        b.network.skip_interface_prefixes.push("tailscale".into());
        let d = diff(&a, &b);
        assert!(d.targets && d.collectors, "{d:?}");

        let mut b = a.clone();
        b.gui.autostart = true;
        assert_eq!(diff(&a, &b), Changes::default());
    }

    /// Valid, sends nowhere (no broadcast, no unicast), one cheap metric.
    fn quiet_cfg() -> Config {
        let mut cfg = Config::default();
        cfg.general.id = "t".into();
        cfg.general.host_name = "quiet".into();
        cfg.network.udp_broadcast = false;
        cfg.metrics.enabled = vec!["mem".into()];
        cfg.validate().unwrap();
        cfg
    }

    #[test]
    fn ts_is_strictly_increasing_even_when_the_clock_steps_back() {
        let script = [1000u64, 1000, 1000, 1500, 900, 0, 1499, 1503, 1503, 5000];
        let mut i = 0;
        let mut c = TsClock::with_clock(move || {
            i += 1;
            script[i - 1]
        });
        let got: Vec<u64> = (0..script.len()).map(|_| c.next()).collect();
        assert_eq!(
            got,
            [1000, 1001, 1002, 1500, 1501, 1502, 1503, 1504, 1505, 5000]
        );
        assert!(got.windows(2).all(|w| w[0] < w[1]));
        // No overflow at the top of the range.
        let mut c = TsClock::with_clock(|| u64::MAX);
        assert_eq!((c.next(), c.next()), (u64::MAX, u64::MAX));
    }

    #[test]
    fn loop_packets_use_the_monotonic_clock() {
        let mut st = State::new(quiet_cfg()).unwrap();
        // Every tick in the same millisecond, then a step back by an hour.
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let n = calls.clone();
        st.ts = TsClock::with_clock(move || {
            match n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0..=2 => 10_000_000,
                _ => 10_000_000 - 3_600_000,
            }
        });
        let mut seen = Vec::new();
        for _ in 0..5 {
            st.tick();
            seen.push(st.ts.last.unwrap());
        }
        assert_eq!(seen, (10_000_000..10_000_005).collect::<Vec<_>>());
        st.paused = true;
        st.tick();
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            5,
            "paused: no ts taken"
        );
    }

    #[test]
    fn meta_debug_hides_the_secret() {
        let s = format!("{:?}", meta(Some("hunter2")));
        assert!(!s.contains("hunter2") && s.contains("********"), "{s}");
        assert!(format!("{:?}", meta(None)).contains("secret: None"));
    }

    #[test]
    fn privacy_applies_to_built_packets() {
        use rtm_protocol::serde_json::json;
        let gpu = || {
            Metric::percent("GPU", Some(1.0)).with_detail(
                json!({"name": "NVIDIA GeForce RTX 4090", "adapter_index": 0})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
        };
        let mut m = meta(Some("k"));
        let p = build_packet(&m, 1, 1, vec![("gpu".into(), gpu())]);
        assert_eq!(
            p.metrics["gpu"]["detail"]["name"], "NVIDIA GeForce RTX 4090",
            "off by default"
        );
        m.privacy = true;
        let p = build_packet(&m, 1, 1, vec![("gpu".into(), gpu())]);
        assert_eq!(p.metrics["gpu"]["detail"]["name"], "GPU #0");
        assert!(p.verify("k"), "signed after the transformation");
        let mut c = Config::default();
        c.general.privacy = true;
        assert!(Meta::from_config(&c).privacy);
        assert!(
            diff(&Config::default(), &c).meta,
            "toggling privacy applies on reload"
        );
    }

    #[test]
    fn seq_does_not_advance_while_paused() {
        let mut st = State::new(quiet_cfg()).unwrap();
        assert!(st.targets.is_empty() && st.sender.is_none());
        st.tick();
        st.tick();
        assert_eq!(st.seq, 2);
        st.paused = true;
        st.tick();
        st.tick();
        assert_eq!(st.seq, 2, "paused ticks build no packet");
        st.paused = false;
        st.tick();
        assert_eq!(st.seq, 3);
    }

    #[test]
    fn rejected_targets_keep_the_old_network_config() {
        let mut st = State::new(quiet_cfg()).unwrap();
        let mut bad = quiet_cfg();
        bad.network.tcp_unicast = vec!["no pe".into()];
        bad.network.shared_secret = "new-secret".into();
        bad.general.host_name = "renamed".into();
        assert!(bad.target_plan().is_err());

        st.apply(bad.clone());
        assert_eq!(
            st.cfg.network,
            quiet_cfg().network,
            "rejected network not stored"
        );
        assert!(st.targets.is_empty());
        assert_eq!(st.meta.host, "renamed", "the rest of the reload applies");
        assert_eq!(
            st.meta.secret, None,
            "secret is part of the kept network settings"
        );
        // The same reload again is still a target change, so it is retried, not ignored.
        assert!(diff(&st.cfg, &bad).targets);
        st.apply(bad);
        assert_eq!(st.cfg.network, quiet_cfg().network);

        // A good reload afterwards applies normally.
        let mut good = quiet_cfg();
        good.network.udp_port = 48123;
        st.apply(good.clone());
        assert_eq!(st.cfg.network, good.network);
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
        let until = Instant::now() + Duration::from_secs(10);
        while !status.read().paused {
            assert!(Instant::now() < until, "pause not applied");
            std::thread::sleep(Duration::from_millis(20));
        }
        while rx_sock.recv(&mut buf).is_ok() {} // drain anything in flight
        let (sent_at_pause, seq_at_pause) = (status.read().sent, status.read().seq);
        assert_eq!(count(700, &mut buf), 0, "nothing sent while paused");
        assert!(status.read().paused);
        assert_eq!(status.read().sent, sent_at_pause);
        assert_eq!(status.read().seq, seq_at_pause, "seq frozen while paused");

        tx.send(Control::Pause(false)).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        let n = loop {
            assert!(Instant::now() < until, "no packet after resuming");
            if let Ok(n) = rx_sock.recv(&mut buf) {
                break n;
            }
        };
        let p = Packet::from_json(std::str::from_utf8(&buf[..n]).unwrap()).unwrap();
        assert_eq!(p.seq, seq_at_pause + 1, "no gap in seq after resuming");
        tx.send(Control::Shutdown).unwrap();
        h.join().unwrap().unwrap();
    }
}
