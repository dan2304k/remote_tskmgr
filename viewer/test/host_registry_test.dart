import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/core/packet.dart';

Packet pkt({
  String id = 'a',
  String host = 'alpha',
  int seq = 1,
  int ts = 1000000,
  int intervalMs = 1000,
  Map<String, Metric>? metrics,
}) => Packet(
  v: 1,
  id: id,
  host: host,
  os: 'linux',
  ts: ts,
  seq: seq,
  intervalMs: intervalMs,
  metrics:
      metrics ??
      {
        'cpu': const Metric(v: 10, unit: '%', label: 'CPU', max: 100),
        'gpu': const Metric(v: null, unit: '%', label: 'GPU', max: 100),
      },
);

void main() {
  late DateTime t;
  late HostRegistry reg;
  late int notifications;

  setUp(() {
    t = DateTime(2026, 11, 8, 12);
    reg = HostRegistry(now: () => t);
    notifications = 0;
    reg.addListener(() => notifications++);
  });

  test('ingest creates host, history, source, notifies', () {
    expect(reg.ingest(pkt(), source: '1.2.3.4:5 udp'), isTrue);
    final h = reg.hosts['a']!;
    expect(h.status, HostStatus.live);
    expect(h.lastSeen, t);
    expect(h.lastSource, '1.2.3.4:5 udp');
    expect(h.history['cpu']!.values, [10.0]);
    expect(h.history['gpu']!.values, [null]);
    expect(notifications, 1);
  });

  test('live -> stale -> offline -> live', () {
    reg.ingest(pkt());
    notifications = 0;
    reg.tick(t.add(const Duration(seconds: 3)));
    expect(reg.hosts['a']!.status, HostStatus.live);
    expect(notifications, 0, reason: 'no change, no notify');
    reg.tick(t.add(const Duration(milliseconds: 3001)));
    expect(reg.hosts['a']!.status, HostStatus.stale);
    expect(notifications, 1);
    reg.tick(t.add(const Duration(seconds: 20)));
    expect(notifications, 1);
    reg.tick(t.add(const Duration(seconds: 31)));
    expect(reg.hosts['a']!.status, HostStatus.offline);
    expect(notifications, 2);
    t = t.add(const Duration(seconds: 40));
    reg.ingest(pkt(seq: 2, ts: 1040000));
    expect(reg.hosts['a']!.status, HostStatus.live);
  });

  test('staleness scales with interval; slow senders stale before offline', () {
    reg.ingest(pkt(intervalMs: 15000));
    reg.tick(t.add(const Duration(seconds: 44)));
    expect(reg.hosts['a']!.status, HostStatus.live);
    reg.tick(t.add(const Duration(seconds: 46)));
    expect(reg.hosts['a']!.status, HostStatus.stale);
    reg.tick(t.add(const Duration(seconds: 91)));
    expect(reg.hosts['a']!.status, HostStatus.offline);
  });

  test('ordered: live, stale, offline; ties by name', () {
    reg.ingest(pkt(id: 'z', host: 'zed'));
    reg.ingest(pkt(id: 'b', host: 'Bravo'));
    t = t.add(const Duration(seconds: 10));
    reg.ingest(pkt(id: 'c', host: 'charlie'));
    reg.ingest(pkt(id: 'a', host: 'alpha'));
    reg.tick(t);
    expect(reg.ordered.map((h) => h.id), ['a', 'c', 'b', 'z']);
    expect(reg.ordered[2].status, HostStatus.stale);
    reg.tick(t.add(const Duration(seconds: 25)));
    expect(reg.ordered.map((h) => h.status), [
      HostStatus.stale,
      HostStatus.stale,
      HostStatus.offline,
      HostStatus.offline,
    ]);
  });

  test('ring buffer keeps the last 120 samples', () {
    for (var i = 0; i < 130; i++) {
      reg.ingest(
        pkt(
          seq: i + 1,
          ts: 1000000 + i * 1000,
          metrics: {'cpu': Metric(v: i.toDouble(), unit: '%', label: 'CPU')},
        ),
      );
    }
    final ring = reg.hosts['a']!.history['cpu']!;
    expect(ring.length, 120);
    expect(ring.values.first, 10.0);
    expect(ring.values.last, 129.0);
    expect(ring.maxValue, 129.0);
    final r = RingBuffer(2)
      ..add(1)
      ..add(2)
      ..add(3);
    expect(r.values, [2.0, 3.0]);
  });

  test('out-of-order and duplicate packets dropped; restart accepted', () {
    reg.ingest(pkt(seq: 100, ts: 1000000));
    notifications = 0;
    expect(reg.ingest(pkt(seq: 99, ts: 999000)), isFalse);
    expect(
      reg.ingest(pkt(seq: 100, ts: 1000000)),
      isFalse,
      reason: 'duplicate',
    );
    expect(reg.hosts['a']!.latest.seq, 100);
    expect(reg.hosts['a']!.history['cpu']!.length, 1);
    expect(notifications, 0);
    expect(reg.droppedOutOfOrder, 2);
    // Daemon restart: seq resets but ts is far ahead.
    expect(reg.ingest(pkt(seq: 1, ts: 1060000)), isTrue);
    expect(reg.hosts['a']!.latest.seq, 1);
  });

  test('packetsPerSecond averages over a 5 s window', () {
    expect(reg.packetsPerSecond, 0);
    reg.ingest(pkt(id: 'a', seq: 1));
    expect(reg.packetsPerSecond, 1, reason: 'window floor is 1 s');
    reg.ingest(pkt(id: 'b', seq: 1));
    expect(reg.packetsPerSecond, 2);
    // Two hosts at 1 Hz for 10 s.
    for (var i = 1; i <= 10; i++) {
      t = t.add(const Duration(seconds: 1));
      reg.ingest(pkt(id: 'a', seq: 1 + i, ts: 1000000 + i * 1000));
      reg.ingest(pkt(id: 'b', seq: 1 + i, ts: 1000000 + i * 1000));
      expect(reg.packetsPerSecond, 2, reason: 'after $i s');
    }
    t = t.add(const Duration(seconds: 3));
    expect(
      reg.packetsPerSecond,
      1,
      reason: '2 s of packets left in 5 s window -> 0.8',
    );
    t = t.add(const Duration(seconds: 3));
    expect(reg.packetsPerSecond, 0);
  });

  test('remove', () {
    reg.ingest(pkt(id: 'a'));
    reg.ingest(pkt(id: 'b'));
    notifications = 0;
    reg.remove('a');
    expect(reg.hosts.keys, ['b']);
    expect(notifications, 1);
    reg.remove('nope');
    expect(notifications, 1);
  });

  test('host keyed by id survives rename', () {
    reg.ingest(pkt(host: 'old'));
    reg.ingest(pkt(host: 'new', seq: 2, ts: 1001000));
    expect(reg.hosts, hasLength(1));
    expect(reg.hosts['a']!.host, 'new');
  });

  test(
    'staleAfter has a 2 s floor for fast senders, 3x interval otherwise',
    () {
      Packet pk(int ms) => Packet(
        v: 1,
        id: 'f',
        host: 'f',
        os: 'linux',
        ts: 0,
        seq: 0,
        intervalMs: ms,
        metrics: const {},
      );
      final fast = HostState(
        id: 'f',
        latest: pk(100),
        lastSeen: DateTime(2026),
      );
      expect(fast.staleAfter, const Duration(seconds: 2));
      final slow = HostState(
        id: 'f',
        latest: pk(5000),
        lastSeen: DateTime(2026),
      );
      expect(slow.staleAfter, const Duration(seconds: 15));
    },
  );

  test('metricLastSeen tracks keys; tick notifies when a metric ages out', () {
    final cpuOnly = {
      'cpu': const Metric(v: 10, unit: '%', label: 'CPU', max: 100),
    };
    reg.ingest(pkt(seq: 1));
    final h = reg.hosts['a']!;
    var hostNotifications = 0;
    h.changes.addListener(() => hostNotifications++);
    expect(h.metricLastSeen, {'cpu': t, 'gpu': t}, reason: 'null value counts');
    expect(h.metricWindow, kMetricWindow);
    final gpuSeen = t;
    // Sender drops gpu; keeps sending cpu every second.
    for (var i = 2; i <= 10; i++) {
      t = t.add(const Duration(seconds: 1));
      reg.ingest(pkt(seq: i, ts: 1000000 + i * 1000, metrics: cpuOnly));
    }
    expect(h.metricLastSeen['gpu'], gpuSeen);
    expect(h.recentMetricKeys(t), {'cpu', 'gpu'});
    reg.tick(t);
    final before = hostNotifications;
    final regBefore = notifications;
    t = t.add(const Duration(milliseconds: 1500));
    reg.tick(t);
    expect(h.recentMetricKeys(t), {'cpu'});
    expect(hostNotifications, before + 1, reason: 'recent set changed');
    expect(notifications, regBefore, reason: 'host-level change only');
    reg.tick(t);
    expect(hostNotifications, before + 1, reason: 'no change, no notify');
  });

  test(
    'metric window is 5 x interval for slow senders; silent host keeps keys',
    () {
      reg.ingest(pkt(intervalMs: 5000));
      final h = reg.hosts['a']!;
      expect(h.metricWindow, const Duration(seconds: 25));
      // No packets at all for a minute: measured from lastSeen, keys stay.
      t = t.add(const Duration(minutes: 1));
      expect(h.recentMetricKeys(t), {'cpu', 'gpu'});
    },
  );

  test('large seq drop is a sender restart, accepted at once', () {
    reg.ingest(pkt(seq: 5000, ts: 1000000));
    // Same ts window, seq reset near zero: restart.
    expect(reg.ingest(pkt(seq: 3, ts: 1000500)), isTrue);
    expect(reg.hosts['a']!.latest.seq, 3);
    reg.ingest(pkt(seq: 9000, ts: 1001000));
    // Dropped by more than 1000: restart.
    expect(reg.ingest(pkt(seq: 7000, ts: 1001500)), isTrue);
    // Small step back, not near zero: reordered datagram.
    expect(reg.ingest(pkt(seq: 6990, ts: 1001400)), isFalse);
    expect(reg.droppedOutOfOrder, 1);
    expect(HostRegistry.isSeqRestart(9, 50), isTrue);
    expect(HostRegistry.isSeqRestart(10, 50), isFalse);
    expect(HostRegistry.isSeqRestart(50, 50), isFalse);
    expect(HostRegistry.isSeqRestart(11, 1011), isFalse);
    expect(HostRegistry.isSeqRestart(1000, 2001), isTrue);
  });

  test('registry notifies on host set / status, host on every packet', () {
    reg.ingest(pkt(id: 'a', seq: 1));
    reg.ingest(pkt(id: 'b', seq: 1));
    expect(notifications, 2, reason: 'two new hosts');
    var aN = 0, bN = 0;
    reg.hosts['a']!.changes.addListener(() => aN++);
    reg.hosts['b']!.changes.addListener(() => bN++);
    reg.ingest(pkt(id: 'a', seq: 2, ts: 1001000));
    expect((aN, bN, notifications), (1, 0, 2));
    // Rename is structural (order by name).
    reg.ingest(pkt(id: 'a', host: 'zeta', seq: 3, ts: 1002000));
    expect(notifications, 3);
    // Stale, then a packet brings it back: status change notifies the registry.
    t = t.add(const Duration(seconds: 4));
    reg.tick(t);
    expect(notifications, 4);
    expect(aN, 3, reason: 'status change notifies the host too');
    reg.ingest(pkt(id: 'a', host: 'zeta', seq: 4, ts: 1003000));
    expect(notifications, 5);
  });

  test('metric keys not seen for 10 x metricWindow are pruned', () {
    final cpuOnly = {
      'cpu': const Metric(v: 10, unit: '%', label: 'CPU', max: 100),
    };
    reg.ingest(pkt(seq: 1));
    final h = reg.hosts['a']!;
    // 100 s of cpu-only packets (window 10 s -> prune after 100 s).
    for (var i = 2; i <= 100; i++) {
      t = t.add(const Duration(seconds: 1));
      reg.ingest(pkt(seq: i, ts: 1000000 + i * 1000, metrics: cpuOnly));
    }
    reg.tick(t);
    expect(h.history.keys, containsAll(['cpu', 'gpu']), reason: '99 s');
    for (var i = 101; i <= 102; i++) {
      t = t.add(const Duration(seconds: 1));
      reg.ingest(pkt(seq: i, ts: 1000000 + i * 1000, metrics: cpuOnly));
    }
    reg.tick(t);
    expect(h.history.keys, ['cpu']);
    expect(h.metricLastSeen.keys, ['cpu']);
  });

  test('silent host keeps its keys; offline > 24 h is removed', () {
    reg.ingest(pkt(id: 'a'));
    reg.ingest(pkt(id: 'b'));
    final h = reg.hosts['a']!;
    t = t.add(const Duration(hours: 23));
    reg.ingest(pkt(id: 'b', seq: 2, ts: 1001000));
    reg.tick(t);
    expect(h.status, HostStatus.offline);
    expect(h.history.keys, ['cpu', 'gpu'], reason: 'measured from lastSeen');
    notifications = 0;
    t = t.add(const Duration(hours: 1, seconds: 31));
    reg.tick(t);
    expect(reg.hosts.keys, ['b'], reason: 'a offline for > 24 h');
    expect(notifications, 1);
  });
}
