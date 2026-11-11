// Host registry: every sender heard so far, with status and sample history.
// Agent viewer-core owns this file.
import 'dart:collection';

import 'package:flutter/foundation.dart';

import 'packet.dart';

enum HostStatus { live, stale, offline }

/// Samples kept per metric for sparklines.
const int kHistoryLength = 120;

/// A host goes offline after this long without a packet (or 6 x interval, if longer; see
/// [HostState.offlineAfter]).
const Duration kOfflineAfter = Duration(seconds: 30);

/// Floor for [HostState.staleAfter]; see PROTOCOL.md "Host lifecycle".
const Duration kMinStaleAfter = Duration(seconds: 2);

/// A packet whose seq is not newer than the latest one is dropped as a duplicate or
/// reordered datagram, unless its ts is this far from the latest ts (daemon restarted).
const Duration kReorderWindow = Duration(seconds: 5);

/// A metric a host stops sending is hidden after this long (or 5 x interval, if
/// longer; see [HostState.metricWindow]).
const Duration kMetricWindow = Duration(seconds: 10);

/// Averaging window for [HostRegistry.packetRate].
const Duration kRateWindow = Duration(seconds: 5);

/// Fixed-capacity ring of recent samples for sparklines. Oldest first in [values].
class RingBuffer {
  RingBuffer(this.capacity);
  final int capacity;
  final ListQueue<double?> _q = ListQueue();
  void add(double? v) {
    if (_q.length == capacity) _q.removeFirst();
    _q.addLast(v);
  }

  Iterable<double?> get values => _q;
  int get length => _q.length;

  /// Most recent sample, or null when empty (also null when the last sample was null).
  double? get last => _q.isEmpty ? null : _q.last;

  /// Largest non-null sample, or null if there is none. Handy for auto-scaling (max: null).
  double? get maxValue {
    double? m;
    for (final v in _q) {
      if (v != null && (m == null || v > m)) m = v;
    }
    return m;
  }
}

class HostState {
  HostState({required this.id, required this.latest, required this.lastSeen});
  final String id;
  Packet latest;
  DateTime lastSeen;
  HostStatus status = HostStatus.live;

  /// One ring per metric key; created on first sight. 120 samples.
  final Map<String, RingBuffer> history = {};

  /// Source address string of the last packet, for diagnostics ("192.168.1.10:51234 udp").
  String? lastSource;

  /// When each metric key last appeared in an accepted packet (null value included).
  final Map<String, DateTime> metricLastSeen = {};

  /// Recent-key set as of the last [HostRegistry.tick], to notify when it changes.
  Set<String> _recentAtTick = const {};

  /// How long a metric may be absent before the UI drops it: 10 s or 5 x interval.
  Duration get metricWindow {
    final five = interval * 5;
    return five > kMetricWindow ? five : kMetricWindow;
  }

  /// Whether [key] was reported within [metricWindow]. Measured from [now] while the
  /// host is live; once it is stale/offline, from [lastSeen], so a silent host keeps
  /// its last layout instead of emptying out.
  bool isMetricRecent(String key, DateTime now) {
    final seen = metricLastSeen[key];
    if (seen == null) return false;
    final ref = statusAt(now) == HostStatus.live ? now : lastSeen;
    return ref.difference(seen) <= metricWindow;
  }

  /// Keys passing [isMetricRecent] at [now].
  Set<String> recentMetricKeys(DateTime now) => {
    for (final k in metricLastSeen.keys)
      if (isMetricRecent(k, now)) k,
  };

  /// Display name from the latest packet.
  String get host => latest.host;

  /// The sender's current interval, clamped to at least 100 ms against bogus values.
  Duration get interval =>
      Duration(milliseconds: latest.intervalMs < 100 ? 100 : latest.intervalMs);

  /// No packet for this long -> stale: 3 x interval, but never under [kMinStaleAfter] so
  /// that fast senders (100-200 ms) do not flicker to stale on ordinary Wi-Fi jitter.
  Duration get staleAfter {
    final three = interval * 3;
    return three < kMinStaleAfter ? kMinStaleAfter : three;
  }

  /// No packet for this long -> offline: 30 s, or 6 x interval for slow senders so that a
  /// 15 s / 30 s sender is stale before it is offline.
  Duration get offlineAfter {
    final six = interval * 6;
    return six > kOfflineAfter ? six : kOfflineAfter;
  }

  /// Status implied by the clock alone.
  HostStatus statusAt(DateTime now) {
    final age = now.difference(lastSeen);
    if (age > offlineAfter) return HostStatus.offline;
    if (age > staleAfter) return HostStatus.stale;
    return HostStatus.live;
  }
}

/// All hosts heard so far, keyed by packet id. Notifies on every accepted packet and on
/// status transitions (a timer re-evaluates staleness every second).
class HostRegistry extends ChangeNotifier {
  HostRegistry({DateTime Function()? now}) : _now = now ?? DateTime.now;

  final DateTime Function() _now;

  /// The clock this registry stamps packets with (injectable for tests).
  DateTime Function() get clock => _now;
  final Map<String, HostState> hosts = {};
  final ListQueue<DateTime> _recent = ListQueue();
  DateTime? _firstAccepted;

  /// Packets dropped as duplicates / out of order since start (diagnostics).
  int droppedOutOfOrder = 0;

  /// Hosts in a stable order: live first, then stale, then offline; ties by host name.
  List<HostState> get ordered {
    final list = hosts.values.toList();
    list.sort((a, b) {
      final s = a.status.index.compareTo(b.status.index);
      if (s != 0) return s;
      final n = a.host.toLowerCase().compareTo(b.host.toLowerCase());
      if (n != 0) return n;
      return a.id.compareTo(b.id);
    });
    return list;
  }

  /// Union of metric keys over all hosts, in first-seen order.
  Set<String> get knownMetricKeys {
    final keys = <String>{};
    for (final h in hosts.values) {
      keys.addAll(h.history.keys);
    }
    return keys;
  }

  /// Accepts [p] unless it is a duplicate or reordered packet from a known host. Returns
  /// whether it was accepted.
  bool ingest(Packet p, {String? source}) {
    final now = _now();
    final existing = hosts[p.id];
    final HostState state;
    if (existing == null) {
      state = HostState(id: p.id, latest: p, lastSeen: now);
      hosts[p.id] = state;
    } else {
      final prev = existing.latest;
      final sameRun = (p.ts - prev.ts).abs() <= kReorderWindow.inMilliseconds;
      if (p.seq <= prev.seq && sameRun) {
        droppedOutOfOrder++;
        return false;
      }
      state = existing
        ..latest = p
        ..lastSeen = now;
    }
    state.status = HostStatus.live;
    state.lastSource = source;
    p.metrics.forEach((key, m) {
      state.history.putIfAbsent(key, () => RingBuffer(kHistoryLength)).add(m.v);
      state.metricLastSeen[key] = now;
    });
    state._recentAtTick = state.recentMetricKeys(now);
    _firstAccepted ??= now;
    _recent.addLast(now);
    _pruneRecent(now);
    notifyListeners();
    return true;
  }

  void remove(String id) {
    if (hosts.remove(id) != null) notifyListeners();
  }

  /// Removes every offline host. Returns how many were removed.
  int removeOffline() {
    final before = hosts.length;
    hosts.removeWhere((_, h) => h.status == HostStatus.offline);
    final n = before - hosts.length;
    if (n > 0) notifyListeners();
    return n;
  }

  /// Re-evaluate statuses and recently reported metrics from the clock. Called by a
  /// periodic timer in NetController.
  void tick(DateTime now) {
    var changed = false;
    for (final h in hosts.values) {
      final s = h.statusAt(now);
      if (s != h.status) {
        h.status = s;
        changed = true;
      }
      final recent = h.recentMetricKeys(now);
      if (!setEquals(recent, h._recentAtTick)) {
        h._recentAtTick = recent;
        changed = true;
      }
    }
    _pruneRecent(now);
    if (changed) notifyListeners();
  }

  /// Accepted packets per second, averaged over the last [kRateWindow] (or since the
  /// first packet plus 1 s, if shorter) so a 1 Hz sender sampled by a
  /// 1 Hz timer reads a steady 1 instead of flickering between 0 and 2.
  double get packetRate {
    final now = _now();
    _pruneRecent(now);
    if (_recent.isEmpty) return 0;
    // +1 s: samples at both ends of [first, now] are counted.
    var window = now.difference(_firstAccepted!) + const Duration(seconds: 1);
    if (window > kRateWindow) window = kRateWindow;
    return _recent.length * 1000 / window.inMilliseconds;
  }

  /// [packetRate] rounded, for the diagnostics row ("12 pkts/s from 3 hosts").
  int get packetsPerSecond => packetRate.round();

  void _pruneRecent(DateTime now) {
    final cutoff = now.subtract(kRateWindow);
    while (_recent.isNotEmpty && !_recent.first.isAfter(cutoff)) {
      _recent.removeFirst();
    }
  }
}
