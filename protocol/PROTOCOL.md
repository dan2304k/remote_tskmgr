# RTM wire protocol, version 1

This file is the source of truth for the packet format. The Rust `rtm-protocol` crate and the
Dart `viewer/lib/core/packet.dart` model must both match it, and both must round-trip every
file in `examples/`. Change this file first, then the code.

## Transport

| Mode | Default | Framing | Notes |
|---|---|---|---|
| UDP broadcast | on | one JSON object per datagram | sent to each interface's directed broadcast address and to 255.255.255.255, port 48000 |
| UDP unicast | off | one JSON object per datagram | list of `ip:port` targets |
| TCP unicast | off | one JSON object per line, `\n` terminated, UTF-8 | sender connects to `ip:port`; viewer listens; sender reconnects with backoff |

* Default port is **48000** for every mode. Configurable on both ends.
* A sender emits a **full packet every interval**. No deltas, no sessions, no handshake.
  A viewer may join at any time; a lost datagram costs one sample and nothing else.
* Keep packets under 1200 bytes. Senders warn above 1400 bytes (UDP fragmentation risk).
* A viewer must tolerate unknown fields anywhere and unknown metric keys.

### Viewer limits

Everything a viewer receives is untrusted LAN input. The reference viewer (1.0.2+) enforces
these limits; a conforming sender never comes near them:

| Limit | Value | On violation |
|---|---|---|
| datagram / TCP line size | 8 KiB (8192 bytes) | packet rejected before decoding (TCP: connection dropped) |
| JSON nesting depth (packet object = 1) | 16 | packet rejected before decoding |
| metrics per packet | 32 | packet rejected |
| metric key | `^[a-z][a-z0-9_]{0,31}$` | that metric is dropped, the rest of the packet is kept |
| `id`, `host` length | 64 characters | packet rejected |
| metric `label` / `unit` length | 12 / 8 characters | packet rejected |
| `interval_ms` used for timing | clamped to 100 ms – 60 s | (staleness, offline, idle timeouts never exceed what 60 s implies) |
| hosts tracked | 64 | the oldest non-live host is evicted; if all are live the new `id` is ignored |
| metric display slots (persisted) | 64 | new keys beyond the cap get no slot |
| TCP connections | 32 total, 4 per remote IP | the connection idle longest is closed to make room |

Control characters and Unicode bidi overrides are stripped from displayed strings.

## Packet

```json
{
  "v": 1,
  "id": "0b2d4a4e-6a2c-4c0e-8f2d-2f6b2b6f9a11",
  "host": "workstation",
  "os": "linux",
  "ts": 1762640000123,
  "seq": 48213,
  "interval_ms": 1000,
  "metrics": { "...": "see below" },
  "auth": "optional hex hmac"
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `v` | integer | yes | protocol version. Always `1`. Viewers ignore other values and surface an "incompatible sender" hint |
| `id` | string | yes | stable per-install identifier (UUID v4 generated once, persisted in sender config). The viewer keys hosts by `id`, so renaming a host does not create a duplicate card |
| `host` | string | yes | display name. Defaults to the system hostname; user may override |
| `os` | string | yes | `linux`, `windows`, `macos`, or another lowercase token. Used only to pick an icon |
| `ts` | integer | yes | sender wall-clock, milliseconds since Unix epoch |
| `seq` | integer | yes | increases by 1 per packet since daemon start. Viewers may use it to detect loss; it is not required to be continuous across daemon restarts |
| `interval_ms` | integer | yes | the sender's current interval, >= 100. Viewers compute staleness from it (see Host lifecycle), clamped to at most 60000 |
| `metrics` | object | yes | map of metric key → Metric. May be empty |
| `auth` | string | no | lowercase hex HMAC-SHA256 over the canonical body (see Auth). Absent when no secret is configured |

## Metric

```json
"cpu": {
  "v": 23.4,
  "unit": "%",
  "label": "CPU",
  "min": 0,
  "max": 100,
  "detail": { "cores": [12.0, 44.1], "freq_mhz": 3800, "temp_c": 61 }
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `v` | number or null | yes | current value. `null` means "collector exists but could not read a value this tick" (viewer shows a dash, keeps the tile) |
| `unit` | string | yes | display unit. `%` triggers percentage colour ramps; anything else is shown verbatim |
| `label` | string | yes | short label for gauges, at most 6 characters recommended |
| `min` | number | no | lower bound for gauges, default 0 |
| `max` | number or null | no | upper bound for gauges. Absent or `null` means auto-scale from recent samples |
| `detail` | object | no | free-form key → scalar / list of scalars / nested object. Shown as a key/value table in the expanded view. Keys snake_case. Viewers never need `detail` to render the tile |

Metric keys are lowercase `snake_case`, at most 32 characters (`^[a-z][a-z0-9_]{0,31}$`); at
most 32 metrics per packet. A metric that is not available on a host at all is
**omitted** (not sent as null). A viewer that has a display slot for an omitted key may show a
dimmed placeholder so cards stay aligned; it must not show a gauge at zero.

## Well-known metrics

Senders should use these keys, labels and units when they emit the concept, so hosts of
different OSes line up on the viewer. Everything else is still allowed.

| key | label | unit | v | detail keys (optional) |
|---|---|---|---|---|
| `cpu` | `CPU` | `%` | total utilisation 0–100 | `cores: [number]` (per logical core, at most 16), or instead for more cores `cores_n` (count), `cores_max`, `cores_min`, `cores_top: [[index, pct], ...]` (busiest few, busiest first); `freq_mhz`, `temp_c`, `load1` |
| `gpu` | `GPU` | `%` | utilisation 0–100, null if unknown | `name`, `vram_used_mb`, `vram_total_mb`, `temp_c`, `freq_mhz`, `backend`, `approx: true` when the value is a proxy |
| `mem` | `MEM` | `%` | used / total × 100 | `used_mb`, `total_mb`, `swap_used_mb`, `swap_total_mb` |
| `disk` | `DISK` | `MB/s` | read + write, `max` null | `read_mbps`, `write_mbps`, `disks: {name: {read_mbps, write_mbps}}` |
| `net` | `NET` | `MB/s` | rx + tx, `max` null | `rx_mbps`, `tx_mbps` |
| `temp` | `TEMP` | `°C` | hottest CPU sensor, `max` 100 | `sensors: {name: c}` |
| `load` | `LOAD` | `` | 1-minute load average, `max` = core count | `load5`, `load15` |
| `uptime` | `UP` | `s` | seconds since boot, `max` null | |

`MB/s` means megabytes per second (10^6). Percentages are 0–100, not 0–1.

## Auth (optional)

When a shared secret is configured the sender adds `auth`: lowercase hex of
`HMAC-SHA256(secret, canonical_body)`. A viewer with a secret configured drops packets whose
`auth` is missing or wrong. A viewer without a secret ignores `auth`. This is tamper
detection on a LAN, not encryption.

`canonical_body` is the **entire received JSON object minus the `auth` key**, re-serialised:
* object keys sorted lexicographically (byte order) at every level, including inside `detail`;
* no whitespace anywhere;
* **unknown keys are included** (a verifier must hash what it parsed, so parsers keep unknown
  top-level keys around for this purpose; the Rust `Packet` has `extra`, Dart verifies the raw map);
* numbers re-serialised as they were parsed: an integer token stays an integer (`10`), a
  floating token keeps a fractional part (`10.0` stays `10.0`), floats use shortest round-trip
  digits. serde_json (with `float_roundtrip`), Dart `jsonEncode`, and Python `json.dumps` all
  agree on this for values in normal ranges. Senders must not emit exponent notation or
  non-finite values;
* strings escaped minimally (only `"`, `\`, and control characters; non-ASCII emitted as UTF-8,
  not `\uXXXX`). Python: `ensure_ascii=False`.

Numbers that must not appear in an authenticated packet (no verifier can reproduce their
bytes, so a viewer with a secret set rejects the packet): the integer token `-0`, integers
outside the signed 64-bit range (≥ 2^63 or < −2^63), and non-finite values (rejected always).

**Replay protection.** A viewer with a secret configured also rejects (as an auth failure) a
correctly signed packet whose `ts`
* differs from the viewer's own clock by more than **5 minutes** ("stale timestamp"), or
* is not strictly greater than the `ts` of the last accepted packet from the same `id`
  (an exact repeat of that packet, same `ts` and `seq`, is dropped silently as a duplicate).

The last-`ts` table is kept in memory only; after a viewer restart the 5-minute window bounds
what can be replayed. Signed senders and the viewer therefore **must have roughly correct
clocks** (phones and PCs normally do via NTP / network time); a sender whose clock is more than
5 minutes off is rejected with "stale timestamp" until it is fixed.

**Without a secret there is no authentication at all**: any device on the LAN can send
packets with any `id`, impersonate an existing host, or invent new ones. Packets are never
encrypted either, with or without a secret: every broadcast discloses to the whole network
the sender's hostname, its stable `id`, its load figures, and device names in `detail`
(e.g. GPU model, disk names, sensor names).

`examples/valid_with_auth.json` is signed with the secret `correct horse` over the whole file
(its `_comment` included); both implementations must verify it (its `ts` is long past, so a
viewer applying the replay rule above rejects it when it arrives over the network; it is a
test vector for the HMAC only).

## Host lifecycle on the viewer

| State | Rule |
|---|---|
| live | packet received within 3 × `interval_ms` |
| stale | no packet for max(2 s, 3 × `interval_ms`); card greys out, last values shown |
| offline | no packet for max(30 s, 6 × `interval_ms`); card collapses to a header. The user can remove any host (live ones reappear with their next packet) |

Viewers also drop duplicates: a packet whose `seq` is not greater than the last accepted one
from the same `id` (and whose `ts` is within 5 s of it) is ignored. Senders broadcasting on
several interfaces routinely produce such duplicates.

## Examples

`examples/` contains golden packets. Both implementations must parse all `valid_*.json`
and reject all `invalid_*.json` (reason in each file's `_comment` is for humans; parsers
must ignore the `_comment` key like any unknown key).
