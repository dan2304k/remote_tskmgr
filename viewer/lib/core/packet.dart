// Packet model for RTM wire protocol v1. Mirrors protocol/PROTOCOL.md exactly; that file is
// the arbiter. Agent viewer-core owns this file.
//
// Everything `dynamic` that jsonDecode produces stays inside this file: callers get typed
// fields, plus `detail` (free-form by spec) as a Map of JSON values.
import 'dart:convert';

import 'package:crypto/crypto.dart';

/// The only protocol version this viewer understands.
const int kProtocolVersion = 1;

/// Thrown when a packet is well-formed JSON but carries a protocol version other than
/// [kProtocolVersion]. It is a [FormatException] whose message contains "version", so
/// callers that only catch [FormatException] still work; callers that care can show the
/// "incompatible sender" hint.
class UnsupportedVersionException extends FormatException {
  UnsupportedVersionException(this.version)
    : super(
        'Unsupported protocol version $version '
        '(this viewer understands version $kProtocolVersion)',
      );

  /// The `v` value the sender used (may be a non-integer JSON value, rendered as a string).
  final Object? version;
}

class Metric {
  const Metric({
    required this.v,
    required this.unit,
    required this.label,
    this.min = 0,
    this.max,
    this.detail,
  });

  /// null = collector present but no value this tick (render a dash).
  final double? v;
  final String unit;
  final String label;
  final double min;

  /// null = auto-scale from recent samples.
  final double? max;

  /// Free-form key -> JSON value (String, num, bool, null, List, Map). Insertion-ordered.
  final Map<String, dynamic>? detail;

  bool get isPercent => unit == '%';

  /// Parses one metric object. Throws [FormatException] when `unit` or `label` is missing
  /// or a known field has the wrong type. Unknown keys are ignored.
  static Metric fromJson(Map<String, dynamic> json) =>
      _metricFromJson(json, '');

  Map<String, Object?> toJson() => {
    'v': v,
    'unit': unit,
    'label': label,
    'min': min,
    'max': max,
    if (detail != null) 'detail': detail,
  };
}

class Packet {
  const Packet({
    required this.v,
    required this.id,
    required this.host,
    required this.os,
    required this.ts,
    required this.seq,
    required this.intervalMs,
    required this.metrics,
    this.auth,
    this.raw,
  });

  final int v;
  final String id;
  final String host;
  final String os;
  final int ts;
  final int seq;
  final int intervalMs;

  /// Insertion-ordered: the sender's order is the default display order hint.
  final Map<String, Metric> metrics;
  final String? auth;

  /// The decoded JSON object this packet came from (unknown keys included). Used by
  /// [verifyAuth]; null for packets built in code.
  final Map<String, Object?>? raw;

  /// Throws [FormatException] on structural errors or unsupported version
  /// ([UnsupportedVersionException], message mentions "version").
  static Packet fromJson(Map<String, dynamic> json) {
    final version = json['v'];
    if (!json.containsKey('v')) {
      throw const FormatException(
        'Packet is missing required field "v" (version)',
      );
    }
    if (version is! num || version != kProtocolVersion) {
      throw UnsupportedVersionException(version);
    }
    _rejectNonFinite(json, 'Packet');
    final metricsJson = json['metrics'];
    if (metricsJson == null) {
      throw const FormatException('Packet is missing required field "metrics"');
    }
    if (metricsJson is! Map) {
      throw const FormatException('Packet field "metrics" must be an object');
    }
    final metrics = <String, Metric>{};
    metricsJson.forEach((key, value) {
      if (value is! Map) {
        throw FormatException('Metric "$key" must be an object');
      }
      metrics[key as String] = _metricFromJson(_stringMap(value), key);
    });
    final auth = json['auth'];
    return Packet(
      v: kProtocolVersion,
      id: _reqString(json, 'id', 'Packet'),
      host: _reqString(json, 'host', 'Packet'),
      os: _reqString(json, 'os', 'Packet'),
      ts: _reqInt(json, 'ts'),
      seq: _reqInt(json, 'seq'),
      intervalMs: _reqInt(json, 'interval_ms'),
      metrics: metrics,
      auth: auth is String ? auth : null,
      raw: Map<String, Object?>.unmodifiable(json),
    );
  }

  /// Parses one UTF-8 JSON document (one UDP datagram or one TCP line).
  static Packet parse(String text) {
    final Object? decoded;
    try {
      decoded = jsonDecode(text);
    } on FormatException catch (e) {
      throw FormatException('Packet is not valid JSON: ${e.message}');
    }
    if (decoded is! Map<String, dynamic>) {
      throw const FormatException('Packet must be a JSON object');
    }
    return fromJson(decoded);
  }

  /// JSON object for this packet. Uses [raw] when available so unknown fields survive.
  Map<String, Object?> toJson() =>
      raw ??
      {
        'v': v,
        'id': id,
        'host': host,
        'os': os,
        'ts': ts,
        'seq': seq,
        'interval_ms': intervalMs,
        'metrics': {for (final e in metrics.entries) e.key: e.value.toJson()},
        if (auth != null) 'auth': auth,
      };

  /// True when [auth] is present and equals HMAC-SHA256([secret], canonical body).
  /// See [canonicalBody] for the exact byte layout.
  bool verifyAuth(String secret) {
    final given = auth;
    if (given == null) return false;
    final expected = computeAuth(toJson(), secret);
    return _constantTimeEquals(given.toLowerCase(), expected);
  }

  /// Lowercase hex HMAC-SHA256 of [canonicalBody] of [json] keyed by [secret].
  static String computeAuth(Map<String, Object?> json, String secret) {
    final hmac = Hmac(sha256, utf8.encode(secret));
    return hmac.convert(utf8.encode(canonicalBody(json))).toString();
  }

  /// Canonical body per PROTOCOL.md "Auth": the packet object without its top-level `auth`
  /// key, object keys sorted at every level, no whitespace, numbers in shortest round-trip
  /// form.
  ///
  /// Number subtlety: the HMAC is over text, so the viewer must reproduce the sender's
  /// number tokens byte for byte. We keep the decoded value's Dart type, which follows the
  /// wire token: `61` decodes to an int and is re-emitted as `61`; `10.0` decodes to a
  /// double and is re-emitted as `10.0` (serde_json prints an f64 10.0 as `10.0`, an i64 as
  /// `10`). We never normalise ints to doubles or vice versa. Doubles are printed with
  /// Dart's shortest round-trip digits laid out the way Rust's `ryu` (used by serde_json)
  /// lays them out: plain notation for 1e-5 <= |x| < 1e16 with a mandatory `.0` for
  /// integral values, otherwise `1.5e16` / `1e-7` (no `+`, no leading-zero exponent).
  /// Dart's own `toString` differs for |x| >= 1e16 and writes `1e+21`, hence the custom
  /// formatter. Keys are sorted by UTF-8 bytes (Rust `BTreeMap<String>` order). Strings are
  /// escaped like serde_json: `"` `\` and control chars only, `\u00xx` lowercase, non-ASCII
  /// written raw.
  static String canonicalBody(Map<String, Object?> json) {
    final body = Map<String, Object?>.of(json)..remove('auth');
    final sb = StringBuffer();
    _writeCanonical(sb, body);
    return sb.toString();
  }
}

/// 2^63: integer tokens at or beyond this magnitude (other than -2^63) do not fit a
/// signed 64-bit integer.
final BigInt _twoTo63 = BigInt.one << 63;

/// Scans the number tokens of the JSON document [text] (assumed to be valid JSON) for
/// values whose wire spelling the viewer cannot reproduce byte for byte, which would make
/// an HMAC over the canonical body unverifiable (see PROTOCOL.md "Auth"):
/// * `-0` as an integer token (decodes to 0, re-serialised as `0`);
/// * integer tokens outside the signed 64-bit range (decode to a rounded double).
///
/// Non-finite values (`1e400`) are rejected for every packet by [Packet.fromJson].
/// Returns a description of the first offending token, or null when there is none.
String? authUnsafeNumber(String text) {
  final n = text.length;
  var i = 0;
  while (i < n) {
    final c = text.codeUnitAt(i);
    if (c == 0x22) {
      // Skip a string, honouring backslash escapes.
      i++;
      while (i < n) {
        final d = text.codeUnitAt(i);
        if (d == 0x5C) {
          i += 2;
        } else if (d == 0x22) {
          i++;
          break;
        } else {
          i++;
        }
      }
      continue;
    }
    if (c == 0x2D || (c >= 0x30 && c <= 0x39)) {
      final start = i;
      i++;
      var integral = true;
      while (i < n) {
        final d = text.codeUnitAt(i);
        if (d >= 0x30 && d <= 0x39) {
          i++;
        } else if (d == 0x2E ||
            d == 0x65 ||
            d == 0x45 ||
            d == 0x2B ||
            d == 0x2D) {
          integral = false;
          i++;
        } else {
          break;
        }
      }
      if (!integral) continue;
      final token = text.substring(start, i);
      if (token == '-0') return 'integer -0';
      final digits = token.startsWith('-') ? token.length - 1 : token.length;
      if (digits >= 19) {
        final v = BigInt.parse(token);
        if (v >= _twoTo63 || v < -_twoTo63) {
          return 'integer $token outside the 64-bit range';
        }
      }
      continue;
    }
    i++;
  }
  return null;
}

// ---------------------------------------------------------------------------------------
// Parsing helpers

Metric _metricFromJson(Map<String, dynamic> json, String key) {
  final where = key.isEmpty ? 'Metric' : 'Metric "$key"';
  final v = json['v'];
  if (v != null && v is! num) {
    throw FormatException('$where field "v" must be a number or null');
  }
  final min = json['min'];
  if (min != null && min is! num) {
    throw FormatException('$where field "min" must be a number');
  }
  final max = json['max'];
  if (max != null && max is! num) {
    throw FormatException('$where field "max" must be a number or null');
  }
  final detail = json['detail'];
  return Metric(
    v: (v as num?)?.toDouble(),
    unit: _reqString(json, 'unit', where),
    label: _reqString(json, 'label', where),
    min: (min as num?)?.toDouble() ?? 0,
    max: (max as num?)?.toDouble(),
    // detail is optional and never needed to render; tolerate a non-object by dropping it.
    detail: detail is Map
        ? Map<String, dynamic>.unmodifiable(_stringMap(detail))
        : null,
  );
}

/// Throws [FormatException] if any number anywhere in [value] is NaN or infinite
/// (`jsonDecode` turns an out-of-range literal like `1e400` into infinity). Such values
/// cannot be rendered sensibly nor re-serialised for auth, so the whole packet is rejected.
void _rejectNonFinite(Object? value, String where) {
  if (value is double && !value.isFinite) {
    throw FormatException('$where contains a non-finite number');
  } else if (value is Map) {
    value.forEach((k, v) => _rejectNonFinite(v, '$where field "$k"'));
  } else if (value is List) {
    for (final e in value) {
      _rejectNonFinite(e, where);
    }
  }
}

Map<String, dynamic> _stringMap(Map m) =>
    m is Map<String, dynamic> ? m : m.map((k, v) => MapEntry(k.toString(), v));

String _reqString(Map<String, dynamic> json, String field, String where) {
  final value = json[field];
  if (value == null) {
    throw FormatException('$where is missing required field "$field"');
  }
  if (value is! String) {
    throw FormatException('$where field "$field" must be a string');
  }
  return value;
}

int _reqInt(Map<String, dynamic> json, String field) {
  final value = json[field];
  if (value == null) {
    throw FormatException('Packet is missing required field "$field"');
  }
  if (value is int) return value;
  // Tolerate 1000.0 from a sender that serialises integers as floats.
  if (value is double && value.isFinite && value == value.truncateToDouble()) {
    return value.toInt();
  }
  throw FormatException('Packet field "$field" must be an integer');
}

bool _constantTimeEquals(String a, String b) {
  if (a.length != b.length) return false;
  var diff = 0;
  for (var i = 0; i < a.length; i++) {
    diff |= a.codeUnitAt(i) ^ b.codeUnitAt(i);
  }
  return diff == 0;
}

// ---------------------------------------------------------------------------------------
// Canonical JSON writer (see Packet.canonicalBody)

void _writeCanonical(StringBuffer sb, Object? value) {
  if (value == null) {
    sb.write('null');
  } else if (value is bool) {
    sb.write(value ? 'true' : 'false');
  } else if (value is int) {
    sb.write(value.toString());
  } else if (value is double) {
    sb.write(formatDoubleLikeRyu(value));
  } else if (value is String) {
    _writeString(sb, value);
  } else if (value is List) {
    sb.write('[');
    for (var i = 0; i < value.length; i++) {
      if (i > 0) sb.write(',');
      _writeCanonical(sb, value[i]);
    }
    sb.write(']');
  } else if (value is Map) {
    final keys = value.keys.map((k) => k.toString()).toList()
      ..sort(_compareUtf8);
    sb.write('{');
    for (var i = 0; i < keys.length; i++) {
      if (i > 0) sb.write(',');
      _writeString(sb, keys[i]);
      sb.write(':');
      _writeCanonical(sb, value[keys[i]]);
    }
    sb.write('}');
  } else {
    throw ArgumentError.value(value, 'value', 'not a JSON value');
  }
}

int _compareUtf8(String a, String b) {
  final ab = utf8.encode(a);
  final bb = utf8.encode(b);
  final n = ab.length < bb.length ? ab.length : bb.length;
  for (var i = 0; i < n; i++) {
    final d = ab[i] - bb[i];
    if (d != 0) return d;
  }
  return ab.length - bb.length;
}

void _writeString(StringBuffer sb, String s) {
  sb.write('"');
  for (final c in s.codeUnits) {
    switch (c) {
      case 0x22:
        sb.write(r'\"');
      case 0x5C:
        sb.write(r'\\');
      case 0x08:
        sb.write(r'\b');
      case 0x0C:
        sb.write(r'\f');
      case 0x0A:
        sb.write(r'\n');
      case 0x0D:
        sb.write(r'\r');
      case 0x09:
        sb.write(r'\t');
      default:
        if (c < 0x20) {
          sb.write(r'\u00');
          sb.write(c.toRadixString(16).padLeft(2, '0'));
        } else {
          sb.writeCharCode(c);
        }
    }
  }
  sb.write('"');
}

/// Formats a finite double the way Rust's `ryu::Buffer::format_finite` does (which is what
/// serde_json emits for f64): shortest round-trip digits, `.0` on integral values, scientific
/// notation outside 1e-5 <= |x| < 1e16. Exposed for tests.
String formatDoubleLikeRyu(double d) {
  if (!d.isFinite) {
    throw ArgumentError.value(
      d,
      'd',
      'JSON cannot represent non-finite numbers',
    );
  }
  if (d == 0) return d.isNegative ? '-0.0' : '0.0';
  final sign = d < 0 ? '-' : '';
  // Dart's toStringAsExponential() with no argument gives the shortest round-trip digits,
  // e.g. 23.4 -> "2.34e+1".
  final exp = d.abs().toStringAsExponential();
  final eAt = exp.indexOf('e');
  final digits = exp.substring(0, eAt).replaceAll('.', '');
  final length = digits.length;
  // ryu's kk: 10^(kk-1) <= |d| < 10^kk
  final kk = int.parse(exp.substring(eAt + 1)) + 1;
  final String body;
  if (length <= kk && kk <= 16) {
    body = '$digits${'0' * (kk - length)}.0';
  } else if (0 < kk && kk <= 16) {
    body = '${digits.substring(0, kk)}.${digits.substring(kk)}';
  } else if (-5 < kk && kk <= 0) {
    body = '0.${'0' * -kk}$digits';
  } else if (length == 1) {
    body = '${digits}e${kk - 1}';
  } else {
    body = '${digits[0]}.${digits.substring(1)}e${kk - 1}';
  }
  return '$sign$body';
}
