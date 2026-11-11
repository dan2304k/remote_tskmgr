import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../core/host_registry.dart';
import '../core/packet.dart';

/// A value split into the big number and its (possibly rescaled) unit.
typedef FormattedValue = ({String number, String unit});

/// Formats [v] for a tile. Percent values drop decimals; small rates keep one or two;
/// seconds are rescaled to min/h/d so uptime stays readable.
FormattedValue formatValue(double? v, String unit) {
  if (v == null || v.isNaN) return (number: '–', unit: unit);
  if (unit == 's') {
    if (v >= 86400) return (number: _num(v / 86400), unit: 'd');
    if (v >= 3600) return (number: _num(v / 3600), unit: 'h');
    if (v >= 60) return (number: _num(v / 60), unit: 'min');
  }
  if (unit == '%') return (number: v.round().toString(), unit: unit);
  return (number: _num(v), unit: unit);
}

String _num(double v) {
  final a = v.abs();
  if (a >= 100 || (a >= 10 && a == a.roundToDouble())) {
    return v.round().toString();
  }
  if (a >= 10) return v.toStringAsFixed(1);
  if (a == 0) return '0';
  return v.toStringAsFixed(a < 1 ? 2 : 1);
}

/// Formats a detail scalar: trims trailing zeros of doubles, joins lists.
String formatDetail(Object? v) {
  if (v == null) return '–';
  if (v is double) {
    if (v == v.roundToDouble()) return v.toInt().toString();
    return v.toStringAsFixed(2).replaceFirst(RegExp(r'0+$'), '');
  }
  if (v is List) return v.map(formatDetail).join(', ');
  if (v is Map) {
    return v.entries
        .map((e) => '${e.key}: ${formatDetail(e.value)}')
        .join(', ');
  }
  return v.toString();
}

/// Flattens a detail map into display rows: nested maps as `a.b`, lists comma-joined.
List<MapEntry<String, String>> flattenDetail(
  Map<String, dynamic> m, [
  String prefix = '',
]) {
  final out = <MapEntry<String, String>>[];
  for (final e in m.entries) {
    final k = prefix.isEmpty ? e.key : '$prefix.${e.key}';
    final v = e.value;
    if (v is Map) {
      out.addAll(flattenDetail(Map<String, dynamic>.from(v), k));
    } else {
      out.add(MapEntry(k, formatDetail(v)));
    }
  }
  return out;
}

/// The gauge upper bound: the metric's max, or for auto-scale metrics the largest
/// recent sample × 1.2 (at least 1).
double resolveMax(Metric m, RingBuffer? history) {
  final max = m.max;
  if (max != null && max > m.min) return max;
  var hi = m.v ?? 0;
  if (history != null) {
    for (final x in history.values) {
      if (x != null && x > hi) hi = x;
    }
  }
  return math.max(1.0, hi * 1.2);
}

String ageText(DateTime lastSeen, DateTime now) {
  final s = now.difference(lastSeen).inSeconds;
  if (s < 1) return 'now';
  if (s < 60) return '${s}s ago';
  if (s < 3600) return '${s ~/ 60}m ago';
  if (s < 86400) return '${s ~/ 3600}h ago';
  return '${s ~/ 86400}d ago';
}

IconData osIcon(String os) => switch (os.toLowerCase()) {
  'linux' => Icons.terminal,
  'windows' => Icons.window,
  'macos' || 'darwin' => Icons.laptop_mac,
  _ => Icons.computer,
};

/// Largest non-null sample among the newest [windowSamples] of [ring], or null when the
/// window is empty, <= 0, or holds only nulls. Because the window slides with every
/// packet, a peak holds until it ages out of the window, then drops.
double? peakOf(RingBuffer? ring, int windowSamples) {
  if (ring == null || windowSamples <= 0 || ring.length == 0) return null;
  final skip = math.max(0, ring.length - windowSamples);
  double? m;
  for (final v in ring.values.skip(skip)) {
    if (v != null && !v.isNaN && (m == null || v > m)) m = v;
  }
  return m;
}

/// Samples covering [holdSeconds] at the sender's [interval]: ceil(hold / interval);
/// 0 when the hold is off.
int peakWindowSamples(double holdSeconds, Duration interval) {
  if (holdSeconds <= 0 || interval.inMicroseconds <= 0) return 0;
  return (holdSeconds * 1e6 / interval.inMicroseconds).ceil();
}

/// The held peak of metric [key] on [host], or null when off / no samples.
double? peakFor(HostState host, String key, double holdSeconds) =>
    peakOf(host.history[key], peakWindowSamples(holdSeconds, host.interval));

/// Mean of the non-null samples among the newest [n] samples of [ring] (all of them
/// if the ring holds fewer). Null only when that window has no non-null sample (or the
/// ring is empty / [n] <= 0), so an occasional null (e.g. an idle GPU flickering
/// between 0 and null) does not blank the tile; a permanently null metric still does.
/// n = 1 returns the last sample.
double? smoothedValue(RingBuffer? ring, int n) {
  if (ring == null || n <= 0 || ring.length == 0) return null;
  final xs = ring.values.toList(growable: false);
  var sum = 0.0;
  var count = 0;
  for (var i = xs.length - 1; i >= 0 && i >= xs.length - n; i--) {
    final v = xs[i];
    if (v == null || v.isNaN) continue;
    sum += v;
    count++;
  }
  return count == 0 ? null : sum / count;
}

/// Newest non-null sample among the newest [n] samples, or null if all are null.
double? lastNonNull(RingBuffer? ring, int n) {
  if (ring == null || n <= 0) return null;
  final xs = ring.values.toList(growable: false);
  for (var i = xs.length - 1; i >= 0 && i >= xs.length - n; i--) {
    final v = xs[i];
    if (v != null && !v.isNaN) return v;
  }
  return null;
}

/// With smoothing off, a null sample is bridged by the last non-null value within this
/// many samples.
const int kNullHoldSamples = 3;

/// Samples averaged for display: ceil(window / interval), or 1 (no smoothing) when the
/// window is off or not longer than one interval.
int smoothingSamples(int windowMs, Duration interval) {
  final ms = interval.inMilliseconds;
  if (windowMs <= 0 || ms <= 0 || ms >= windowMs) return 1;
  return (windowMs / ms).ceil();
}

/// The value to display for [key] on [host]: the moving average over [windowMs] for
/// senders faster than the window, else [raw] (the latest packet's value). Either way a
/// null newest sample shows the recent non-null value (window, or the last
/// [kNullHoldSamples] samples) and only all-null windows show as null. Peaks and line
/// graphs use raw samples, not this.
double? displayValue(HostState host, String key, double? raw, int windowMs) {
  final ring = host.history[key];
  final n = smoothingSamples(windowMs, host.interval);
  if (n <= 1) return raw ?? lastNonNull(ring, kNullHoldSamples);
  return smoothedValue(ring, n) ?? raw;
}
