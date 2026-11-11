//! Sender configuration. Agent `sender-core` owns this crate.
//! Field names are identical to the TOML keys; see the generated template for documentation.
//! Unknown keys are ignored (serde default), missing keys take defaults.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

/// Smallest accepted `general.interval_ms`.
pub const MIN_INTERVAL_MS: u64 = 100;
/// Accepted values of `metrics.gpu_backend`.
pub const GPU_BACKENDS: &[&str] = &["auto", "nvml", "amdgpu", "intel", "pdh", "none"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub network: Network,
    pub metrics: Metrics,
    pub gui: Gui,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// Empty = system hostname.
    pub host_name: String,
    pub interval_ms: u64,
    /// "auto" = generate a UUID v4 once and persist it back into the file.
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum BroadcastInterfaces {
    /// "all" | "default"
    Mode(String),
    Named(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Network {
    pub udp_broadcast: bool,
    pub udp_port: u16,
    pub broadcast_interfaces: BroadcastInterfaces,
    pub skip_interface_prefixes: Vec<String>,
    pub udp_unicast: Vec<String>,
    pub tcp_unicast: Vec<String>,
    pub shared_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Metrics {
    pub enabled: Vec<String>,
    pub gpu_backend: String,
    /// Which GPU to report: "auto" (most dedicated VRAM), "#N" (detected index), or a
    /// case-insensitive part of the adapter name, e.g. "R9700".
    pub gpu_adapter: String,
    pub disk_include: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Gui {
    pub start_minimized: bool,
    pub autostart: bool,
    /// With `start_minimized`: show a short splash pointing at the tray icon on start.
    pub show_splash: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            host_name: String::new(),
            interval_ms: 1000,
            id: "auto".into(),
        }
    }
}
impl Default for Network {
    fn default() -> Self {
        Self {
            udp_broadcast: true,
            udp_port: 48000,
            broadcast_interfaces: BroadcastInterfaces::Mode("all".into()),
            skip_interface_prefixes: ["docker", "br-", "veth", "virbr", "lo"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            udp_unicast: vec![],
            tcp_unicast: vec![],
            shared_secret: String::new(),
        }
    }
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            enabled: ["cpu", "gpu", "mem", "disk"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            gpu_backend: "auto".into(),
            gpu_adapter: "auto".into(),
            disk_include: vec![],
        }
    }
}
impl Default for Gui {
    fn default() -> Self {
        Self {
            start_minimized: true,
            autostart: false,
            show_splash: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serialise: {0}")]
    Ser(#[from] toml::ser::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl Config {
    /// Linux: ~/.config/rtm/sender.toml, Windows: %APPDATA%\rtm\sender.toml, macOS: ~/Library/Application Support/rtm/sender.toml
    pub fn default_path() -> PathBuf {
        match directories::ProjectDirs::from("", "", "rtm") {
            Some(d) => d.config_dir().join("sender.toml"),
            // No home directory (e.g. a stripped-down service account): use the CWD.
            None => PathBuf::from("sender.toml"),
        }
    }

    /// Parse a TOML string. Does not validate and does not resolve `id = "auto"`.
    pub fn from_toml(s: &str) -> Result<Config, ConfigError> {
        Ok(toml::from_str(s)?)
    }

    /// Load an existing file. Does not create it and does not resolve `id = "auto"`.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        Self::from_toml(&std::fs::read_to_string(path)?)
    }

    /// Load, or write defaults (with comments) and return them. Resolves `id = "auto"` (or an
    /// empty id) to a UUID v4 and persists it back to the file, keeping the user's comments.
    pub fn load_or_create(path: &Path) -> Result<Config, ConfigError> {
        if !path.exists() {
            let mut cfg = Config::default();
            cfg.general.id = uuid::Uuid::new_v4().to_string();
            write_atomic(path, &default_template(&cfg.general.id))?;
            tracing::info!(path = %path.display(), "created default config");
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(path)?;
        let mut cfg = Self::from_toml(&text)?;
        if cfg.id_is_auto() {
            cfg.general.id = uuid::Uuid::new_v4().to_string();
            match replace_id_line(&text, &cfg.general.id) {
                Some(new_text) => write_atomic(path, &new_text)?,
                None => cfg.save(path)?,
            }
            tracing::info!(id = %cfg.general.id, "generated and persisted sender id");
        }
        Ok(cfg)
    }

    /// Write pretty TOML atomically (temp file + rename). Comments in an existing file are lost.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        write_atomic(path, &toml::to_string_pretty(self)?)
    }

    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    fn id_is_auto(&self) -> bool {
        let id = self.general.id.trim();
        id.is_empty() || id.eq_ignore_ascii_case("auto")
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errs = Vec::new();
        if self.general.interval_ms < MIN_INTERVAL_MS {
            errs.push(format!(
                "general.interval_ms = {} (must be >= {MIN_INTERVAL_MS})",
                self.general.interval_ms
            ));
        }
        if self.network.udp_port == 0 {
            errs.push("network.udp_port must not be 0".into());
        }
        if let Err(e) = self.broadcast_interfaces() {
            errs.push(e);
        }
        for (key, list) in [
            ("udp_unicast", &self.network.udp_unicast),
            ("tcp_unicast", &self.network.tcp_unicast),
        ] {
            for a in list {
                if let Err(e) = parse_target_spec(a, self.network.udp_port) {
                    errs.push(format!("network.{key}: '{}': {e}", a.trim()));
                }
            }
        }
        if !GPU_BACKENDS.contains(&self.metrics.gpu_backend.to_ascii_lowercase().as_str()) {
            errs.push(format!(
                "metrics.gpu_backend = '{}' (expected one of {})",
                self.metrics.gpu_backend,
                GPU_BACKENDS.join("|")
            ));
        }
        for k in &self.metrics.enabled {
            if k.trim().is_empty() {
                errs.push("metrics.enabled contains an empty key".into());
            }
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Invalid(errs.join("; ")))
        }
    }

    /// `general.host_name` if set, else the system hostname, else "unknown".
    pub fn effective_host_name(&self) -> String {
        let n = self.general.host_name.trim();
        if !n.is_empty() {
            return n.to_string();
        }
        hostname::get()
            .ok()
            .map(|h| h.to_string_lossy().trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "unknown".into())
    }

    /// The shared secret, or `None` when empty (auth disabled).
    pub fn secret(&self) -> Option<&str> {
        Some(self.network.shared_secret.as_str()).filter(|s| !s.is_empty())
    }

    pub fn to_collector_config(&self) -> rtm_collect::CollectorConfig {
        rtm_collect::CollectorConfig {
            enabled: self
                .metrics
                .enabled
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            gpu_backend: self.metrics.gpu_backend.parse().unwrap_or_default(),
            gpu_adapter: self.metrics.gpu_adapter.trim().to_string(),
            disk_include: self.metrics.disk_include.clone(),
        }
    }

    fn broadcast_interfaces(&self) -> Result<rtm_net::BroadcastInterfaces, String> {
        match &self.network.broadcast_interfaces {
            BroadcastInterfaces::Mode(m) => match m.to_ascii_lowercase().as_str() {
                "all" => Ok(rtm_net::BroadcastInterfaces::All),
                "default" => Ok(rtm_net::BroadcastInterfaces::Default),
                other => Err(format!(
                    "network.broadcast_interfaces = '{other}' (expected \"all\", \"default\" or a list of names)"
                )),
            },
            BroadcastInterfaces::Named(v) => Ok(rtm_net::BroadcastInterfaces::Named(v.clone())),
        }
    }

    /// The unicast targets as written (syntax checked, ports defaulted, names not resolved).
    /// Two configs with equal plans send to the same places, DNS aside.
    pub fn target_plan(&self) -> Result<TargetPlan, ConfigError> {
        let broadcast = if self.network.udp_broadcast {
            Some(rtm_net::Target::UdpBroadcast {
                port: self.network.udp_port,
                interfaces: self.broadcast_interfaces().map_err(ConfigError::Invalid)?,
                skip_prefixes: self.network.skip_interface_prefixes.clone(),
            })
        } else {
            None
        };
        let specs = |key: &str, list: &[String]| {
            list.iter()
                .map(|a| {
                    parse_target_spec(a, self.network.udp_port).map_err(|e| {
                        ConfigError::Invalid(format!("network.{key}: '{}': {e}", a.trim()))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(TargetPlan {
            broadcast,
            udp: specs("udp_unicast", &self.network.udp_unicast)?,
            tcp: specs("tcp_unicast", &self.network.tcp_unicast)?,
        })
    }

    /// Broadcast (if enabled) first, then UDP unicast, then TCP unicast, in file order.
    /// Host names are resolved here; one that does not resolve is logged and skipped (see
    /// `resolve_targets` to learn which were skipped).
    pub fn to_targets(&self) -> Result<Vec<rtm_net::Target>, ConfigError> {
        let r = self.resolve_targets()?;
        for (spec, e) in &r.unresolved {
            tracing::warn!("target {spec} skipped: {e}");
        }
        Ok(r.targets)
    }

    /// Like `to_targets`, but reports skipped (unresolvable) targets instead of logging them.
    pub fn resolve_targets(&self) -> Result<ResolvedTargets, ConfigError> {
        let plan = self.target_plan()?;
        let mut out = ResolvedTargets::default();
        out.targets.extend(plan.broadcast);
        for (specs, tcp) in [(plan.udp, false), (plan.tcp, true)] {
            for spec in specs {
                match spec.resolve() {
                    Ok(a) if tcp => out.targets.push(rtm_net::Target::TcpUnicast(a)),
                    Ok(a) => out.targets.push(rtm_net::Target::UdpUnicast(a)),
                    Err(e) => out.unresolved.push((spec, e)),
                }
            }
        }
        Ok(out)
    }
}

/// See `Config::target_plan`.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetPlan {
    pub broadcast: Option<rtm_net::Target>,
    pub udp: Vec<TargetSpec>,
    pub tcp: Vec<TargetSpec>,
}

/// See `Config::resolve_targets`.
#[derive(Debug, Clone, Default)]
pub struct ResolvedTargets {
    pub targets: Vec<rtm_net::Target>,
    /// Targets whose host name did not resolve, with the reason.
    pub unresolved: Vec<(TargetSpec, String)>,
}

/// A unicast target after syntax checking: an address, or a host name still to be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSpec {
    Addr(SocketAddr),
    Host(String, u16),
}

impl std::fmt::Display for TargetSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Addr(a) => write!(f, "{a}"),
            Self::Host(h, p) => write!(f, "{h}:{p}"),
        }
    }
}

impl TargetSpec {
    /// Resolve a host name (first IPv4 address preferred). Addresses are returned as is.
    pub fn resolve(&self) -> Result<SocketAddr, String> {
        match self {
            Self::Addr(a) => Ok(*a),
            Self::Host(h, p) => {
                let addrs: Vec<SocketAddr> = (h.as_str(), *p)
                    .to_socket_addrs()
                    .map_err(|e| format!("cannot resolve '{h}': {e}"))?
                    .collect();
                addrs
                    .iter()
                    .find(|a| a.is_ipv4())
                    .or_else(|| addrs.first())
                    .copied()
                    .ok_or_else(|| format!("'{h}' has no addresses"))
            }
        }
    }
}

/// Parse a unicast target without resolving names. Accepts `ip`, `ip:port`, `[v6]`,
/// `[v6]:port`, bare `v6`, `host` and `host:port`; a missing port becomes `default_port`.
/// Error messages are short (they are shown next to an input row).
pub fn parse_target_spec(s: &str, default_port: u16) -> Result<TargetSpec, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("empty".into());
    }
    if let Ok(a) = t.parse::<SocketAddr>() {
        if a.port() == 0 {
            return Err("port must be 1-65535".into());
        }
        return Ok(TargetSpec::Addr(a));
    }
    if let Ok(ip) = t.parse::<IpAddr>() {
        return Ok(TargetSpec::Addr(SocketAddr::new(ip, default_port)));
    }
    let port = |p: &str| match p.parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(format!("bad port '{p}' (1-65535)")),
    };
    if let Some(rest) = t.strip_prefix('[') {
        let (ip, after) = rest.split_once(']').ok_or("missing ']'")?;
        let ip: Ipv6Addr = ip.parse().map_err(|_| "bad IPv6 address")?;
        let p = match after {
            "" => default_port,
            _ => port(after.strip_prefix(':').ok_or("expected :port after ']'")?)?,
        };
        return Ok(TargetSpec::Addr(SocketAddr::new(IpAddr::V6(ip), p)));
    }
    let (host, p) = match t.rsplit_once(':') {
        Some((h, _)) if h.contains(':') => return Err("bad IPv6 address (use [addr]:port)".into()),
        Some((h, p)) => (h, port(p)?),
        None => (t, default_port),
    };
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(TargetSpec::Addr(SocketAddr::new(ip, p)));
    }
    check_hostname(host)?;
    Ok(TargetSpec::Host(host.to_ascii_lowercase(), p))
}

/// Parse and resolve a unicast target (see `parse_target_spec`); host names are resolved
/// now, first IPv4 address preferred.
pub fn parse_target(s: &str, default_port: u16) -> Result<SocketAddr, String> {
    parse_target_spec(s, default_port)?.resolve()
}

/// RFC 1123 host name syntax. A last label that is all digits is rejected, which catches
/// mistyped IPv4 addresses such as `192.168.1.300` or `10.0.0`.
fn check_hostname(h: &str) -> Result<(), String> {
    let h = h.strip_suffix('.').unwrap_or(h);
    let bad = || Err("not an IP address or host name".to_string());
    if h.is_empty() || h.len() > 253 {
        return bad();
    }
    for label in h.split('.') {
        let ok = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return bad();
        }
    }
    if h.rsplit('.')
        .next()
        .is_some_and(|l| l.chars().all(|c| c.is_ascii_digit()))
    {
        return Err("not a valid IP address".into());
    }
    Ok(())
}

/// Replace the value of `id` inside `[general]` keeping everything else byte-for-byte.
/// Returns `None` when no such line exists (caller falls back to a full rewrite).
fn replace_id_line(text: &str, id: &str) -> Option<String> {
    let mut section = String::new();
    let mut done = false;
    let mut out = String::with_capacity(text.len() + 40);
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.trim_matches(|c| c == '[' || c == ']').trim().to_string();
        } else if !done && section == "general" {
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == "id" {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    let nl = if line.ends_with("\r\n") {
                        "\r\n"
                    } else if line.ends_with('\n') {
                        "\n"
                    } else {
                        ""
                    };
                    out.push_str(&format!(
                        "{indent}id = \"{id}\"              # generated from \"auto\"; delete to regenerate{nl}"
                    ));
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    done.then_some(out)
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), ConfigError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The commented default file written on first run. Must parse to `Config::default()` with
/// `general.id` replaced by `id`.
pub fn default_template(id: &str) -> String {
    format!(
        r#"# RTM sender configuration. Edits are picked up while the sender runs (hot reload).

[general]
host_name = ""            # empty = system hostname
interval_ms = 1000        # 100 | 200 | 1000 | 5000 | 15000 | 30000 (any value >= 100 accepted)
id = "{id}"   # stable per-install id; "auto" = generate once and persist

[network]
udp_broadcast = true
udp_port = 48000
broadcast_interfaces = "all"      # "all" | "default" | ["enp0s31f6", "wlan0"]
skip_interface_prefixes = ["docker", "br-", "veth", "virbr", "lo"]
udp_unicast = []                  # ["192.168.1.50", "viewer.lan:48001"]; no port = udp_port
tcp_unicast = []                  # ["192.168.1.50:48000"]; no port = udp_port
shared_secret = ""                # empty = no auth field

[metrics]
enabled = ["cpu", "gpu", "mem", "disk"]   # order is the display order hint; extras: net, temp, load, uptime
gpu_backend = "auto"                      # auto | nvml | amdgpu | intel | pdh | none
gpu_adapter = "auto"                      # auto (most VRAM) | an index like #1 | part of the name, e.g. R9700
disk_include = []                         # empty = all physical disks

[gui]
start_minimized = true     # tray only at launch; false = open the settings window
autostart = false
show_splash = true         # with start_minimized: briefly point at the tray icon on start
"#
    )
}

#[cfg(test)]
mod tests;
