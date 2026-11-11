//! RTM wire protocol v1. See `protocol/PROTOCOL.md` at the repo root; it is the source of truth.
//!
//! Agent `sender-core` owns this crate. Other crates depend on these names; add, do not rename.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

/// Re-exported so crates that build packets need no direct dependency of their own.
pub use serde_json;

type HmacSha256 = Hmac<Sha256>;

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 48000;
/// Senders log a warning above this many bytes (UDP fragmentation risk).
pub const PACKET_WARN_BYTES: usize = 1400;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    /// `None` serialises as JSON `null` and means "collector exists but has no value this tick".
    pub v: Option<f64>,
    pub unit: String,
    pub label: String,
    /// Omitted on the wire when 0, the protocol default.
    #[serde(default, skip_serializing_if = "min_is_default")]
    pub min: Option<f64>,
    /// `Some(None)` would be ambiguous in serde; we model "auto-scale" as `None` and simply omit the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Packet {
    pub v: u32,
    pub id: String,
    pub host: String,
    pub os: String,
    pub ts: u64,
    pub seq: u64,
    pub interval_ms: u64,
    /// Insertion order is preserved (serde_json `preserve_order`) so the sender's
    /// `[metrics].enabled` order reaches the viewer as a display-order hint.
    pub metrics: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    /// Unknown top-level keys, preserved so `verify` hashes exactly what was received
    /// (PROTOCOL.md Auth). Senders leave this empty.
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported protocol version {0}")]
    Version(u32),
    #[error("invalid packet: {0}")]
    Invalid(String),
}

fn min_is_default(min: &Option<f64>) -> bool {
    min.is_none_or(|m| m == 0.0)
}

impl Metric {
    pub fn percent(label: &str, v: Option<f64>) -> Self {
        Self {
            v,
            unit: "%".into(),
            label: label.into(),
            min: Some(0.0),
            max: Some(100.0),
            detail: None,
        }
    }
    pub fn rate_mbps(label: &str, v: Option<f64>) -> Self {
        Self {
            v,
            unit: "MB/s".into(),
            label: label.into(),
            min: Some(0.0),
            max: None,
            detail: None,
        }
    }
    pub fn with_detail(mut self, detail: Map<String, Value>) -> Self {
        self.detail = Some(detail);
        self
    }
}

impl Packet {
    pub fn to_json(&self) -> Result<String, ProtocolError> {
        Ok(serde_json::to_string(self)?)
    }
    /// JSON followed by a single `\n`, for the TCP line transport.
    pub fn to_json_line(&self) -> Result<String, ProtocolError> {
        let mut s = self.to_json()?;
        s.push('\n');
        Ok(s)
    }
    pub fn from_json(s: &str) -> Result<Packet, ProtocolError> {
        let p: Packet = serde_json::from_str(s)?;
        p.validate()?;
        Ok(p)
    }
    /// Structural validation per PROTOCOL.md: version, metric shapes (object with `v`,
    /// `unit`, `label`), and `min`/`max`/`detail` types. Label length (<= 6) is a
    /// recommendation only and is not rejected.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.v != PROTOCOL_VERSION {
            return Err(ProtocolError::Version(self.v));
        }
        for (k, m) in &self.metrics {
            let obj = m
                .as_object()
                .ok_or_else(|| ProtocolError::Invalid(format!("metric {k}: not an object")))?;
            // serde treats a missing Option field as None; the spec says `v` is required.
            if !obj.contains_key("v") {
                return Err(ProtocolError::Invalid(format!("metric {k}: missing `v`")));
            }
            serde_json::from_value::<Metric>(m.clone())
                .map_err(|e| ProtocolError::Invalid(format!("metric {k}: {e}")))?;
        }
        Ok(())
    }
    /// Insert a typed metric. Insertion order is kept on the wire.
    pub fn set_metric(&mut self, key: &str, m: Metric) {
        self.metrics.insert(
            key.to_string(),
            serde_json::to_value(m).expect("metric is serialisable"),
        );
    }
    /// The canonical body used for `auth`: the packet without `auth`, object keys sorted
    /// lexicographically at every level, no whitespace (PROTOCOL.md, Auth).
    pub fn canonical_body(&self) -> String {
        let mut v = serde_json::to_value(self).expect("packet is serialisable");
        if let Value::Object(o) = &mut v {
            o.remove("auth");
        }
        serde_json::to_string(&canonicalize(v)).expect("value is serialisable")
    }
    /// Lowercase hex HMAC-SHA256 of [`Packet::canonical_body`] keyed by `secret`.
    pub fn compute_auth(&self, secret: &str) -> String {
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(self.canonical_body().as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }
    /// Compute and attach `auth` (HMAC-SHA256 over the canonical body).
    pub fn sign(&mut self, secret: &str) {
        self.auth = Some(self.compute_auth(secret));
    }
    /// Verify `auth` against a secret. Missing or malformed `auth` fails. Constant-time compare.
    pub fn verify(&self, secret: &str) -> bool {
        let Some(auth) = &self.auth else {
            return false;
        };
        let Ok(tag) = hex::decode(auth) else {
            return false;
        };
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(self.canonical_body().as_bytes());
        mac.verify_slice(&tag).is_ok()
    }
}

/// Recursively rebuild every object with keys in lexicographic (byte) order. Needed because
/// serde_json is built with `preserve_order`, so maps keep insertion order otherwise.
pub fn canonicalize(v: Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut entries: Vec<(String, Value)> = o.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, canonicalize(v)))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

/// Wall clock in milliseconds since the Unix epoch (0 if the clock is before 1970).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
