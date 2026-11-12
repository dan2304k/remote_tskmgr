// Regression tests for the 1.0.2 security findings: each test reproduces the attack
// shape from the review and checks it is now contained.
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/net/packet_intake.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'fixtures_util.dart';

Map<String, Object?> metric(String label, {Object? v = 1}) => {
  'v': v,
  'unit': '%',
  'label': label,
};

/// valid_minimal.json as a map with [metrics] and optional edits.
Map<String, dynamic> packetJson({
  Map<String, Object?>? metrics,
  String id = 'h1',
  int ts = 1762640000000,
  int seq = 1,
  int intervalMs = 1000,
}) => fixtureJson('valid_minimal.json')
  ..['id'] = id
  ..['ts'] = ts
  ..['seq'] = seq
  ..['interval_ms'] = intervalMs
  ..['metrics'] = metrics ?? {'cpu': metric('CPU')};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('1. metric keys cannot pollute settings', () {
    late HostRegistry registry;
    late DisplayConfig config;
    late PacketIntake intake;
    setUp(() {
      SharedPreferences.setMockInitialValues({});
      registry = HostRegistry();
      config = DisplayConfig();
      intake = PacketIntake(
        registry: registry,
        secret: () => '',
        onAccepted: (p) => config.ensureSlots(p.metrics.keys),
      );
    });

    test('1650 junk keys in one datagram: rejected, no slots added', () {
      final j = packetJson(
        metrics: {for (var i = 0; i < 1650; i++) 'Junk-$i': metric('J')},
      );
      final bytes = utf8.encode(jsonEncode(j));
      expect(intake.handleBytes(bytes, 'x'), isFalse);
      expect(intake.parseErrors, 1);
      expect(config.slots.map((s) => s.key), kDefaultSlotKeys);
      // The same shape below the size cap still fails on the metric count.
      expect(() => Packet.fromJson(j), throwsFormatException);
    });

    test('more than 32 metrics is a parse error; 32 is fine', () {
      Map<String, Object?> n(int count) => {
        for (var i = 0; i < count; i++) 'm$i': metric('M'),
      };
      expect(
        () => Packet.fromJson(packetJson(metrics: n(33))),
        throwsFormatException,
      );
      expect(
        Packet.fromJson(packetJson(metrics: n(32))).metrics,
        hasLength(32),
      );
    });

    test('invalid keys are dropped, the rest of the packet is kept', () {
      final p = Packet.fromJson(
        packetJson(
          metrics: {
            'cpu': metric('CPU'),
            'CPU': metric('X'),
            '9lives': metric('X'),
            'has space': metric('X'),
            'a' * 33: metric('X'),
            'é': metric('X'),
            'bad_shape_but_bad_key!': 5,
            'fan_rpm2': metric('FAN'),
            'a' * 32: metric('LONG'),
          },
        ),
      );
      expect(p.metrics.keys, ['cpu', 'fan_rpm2', 'a' * 32]);
    });

    test('ensureSlots validates keys and stops at 64 slots', () {
      config.ensureSlots(['Bad', '', 'x y', 'ok_1']);
      expect(config.slots.map((s) => s.key), [...kDefaultSlotKeys, 'ok_1']);
      config.ensureSlots([for (var i = 0; i < 200; i++) 'k$i']);
      expect(config.slots, hasLength(kMaxSlots));
      expect(config.slotFor('k58'), isNotNull);
      expect(config.slotFor('k59'), isNull, reason: 'beyond the cap');
      expect(config.slotFor('k199'), isNull);
    });

    test('slotFor stays correct through reorder, remove and reset', () {
      config.ensureSlots(['net', 'temp']);
      config.reorder(5, 0);
      expect(config.slots.first.key, 'temp');
      expect(config.slotFor('temp'), same(config.slots.first));
      config.removeSlot('temp');
      expect(config.slotFor('temp'), isNull);
      expect(config.slots.map((s) => s.key), [...kDefaultSlotKeys, 'net']);
      // A deleted key comes back only when a host sends it again.
      config.ensureSlots(['temp']);
      expect(config.slots.last.key, 'temp');
      config.resetSlots();
      expect(config.slotFor('net'), isNull);
      expect(config.slotFor('cpu'), same(config.slots.first));
      expect(
        () => config.slots.add(config.slots.first),
        throwsUnsupportedError,
      );
    });

    test('load sanitises a polluted stored config and saves it back', () async {
      SharedPreferences.setMockInitialValues({
        kDisplayConfigPrefsKey: jsonEncode({
          'slots': [
            {'key': 'cpu', 'visible': false, 'style': 'bar'},
            {'key': 'Junk-1'},
            {'key': '<script>'},
            for (var i = 0; i < 1650; i++) {'key': 'k$i'},
          ],
          'slotsMigratedV2': true,
          'sparklinesDefaultOnV3': true,
        }),
      });
      final c = await DisplayConfig.load();
      expect(c.slots, hasLength(kMaxSlots));
      expect(c.slots.first.key, 'cpu');
      expect(c.slots.first.visible, isFalse);
      expect(c.slots.any((s) => !isValidMetricKey(s.key)), isFalse);
      await Future<void>.delayed(Duration.zero);
      final prefs = await SharedPreferences.getInstance();
      final saved = jsonDecode(prefs.getString(kDisplayConfigPrefsKey)!) as Map;
      expect(saved['slots'], hasLength(kMaxSlots));
    });
  });

  group('2. hosts are bounded and spoofed hosts expire', () {
    late DateTime t;
    late HostRegistry reg;
    setUp(() {
      t = DateTime.utc(2026, 11, 11, 12);
      reg = HostRegistry(now: () => t);
    });

    Packet pkt(
      String id, {
      int seq = 1,
      int intervalMs = 1000,
      Map<String, Object?>? metrics,
    }) => Packet.fromJson(
      packetJson(id: id, seq: seq, intervalMs: intervalMs, metrics: metrics),
    );

    test('interval_ms = 1e12 is clamped to 60 s for every timing', () {
      final p = Packet.parse(
        jsonEncode(packetJson())
            .replaceFirst('"interval_ms":1000', '"interval_ms":1e12'),
      );
      expect(p.intervalMs, 1000000000000);
      reg.ingest(p);
      final h = reg.hosts['h1']!;
      expect(h.interval, const Duration(seconds: 60));
      expect(h.staleAfter, const Duration(minutes: 3));
      expect(h.offlineAfter, const Duration(minutes: 6));
      expect(h.metricWindow, const Duration(minutes: 5));
      t = t.add(const Duration(minutes: 7));
      reg.tick(t);
      expect(h.status, HostStatus.offline);
      t = t.add(const Duration(hours: 24));
      reg.tick(t);
      expect(reg.hosts, isEmpty, reason: 'auto-removed like any offline host');
    });

    test('at 64 live hosts a new id is ignored and counted', () {
      for (var i = 0; i < kMaxHosts; i++) {
        expect(reg.ingest(pkt('h$i')), isTrue);
      }
      expect(reg.ingest(pkt('new')), isFalse);
      expect(reg.hosts, hasLength(kMaxHosts));
      expect(reg.rejectedHosts, 1);
      // Known hosts still update.
      expect(reg.ingest(pkt('h3', seq: 2)), isTrue);
    });

    test('when full, the oldest non-live host is evicted', () {
      for (var i = 0; i < kMaxHosts; i++) {
        reg.ingest(pkt('h$i'));
        t = t.add(const Duration(milliseconds: 10));
      }
      // h0 and h1 fall silent; the rest keep sending.
      t = t.add(const Duration(seconds: 5));
      for (var i = 2; i < kMaxHosts; i++) {
        reg.ingest(pkt('h$i', seq: 2));
      }
      expect(reg.ingest(pkt('new')), isTrue);
      expect(reg.hosts.containsKey('h0'), isFalse, reason: 'oldest non-live');
      expect(reg.hosts.containsKey('h1'), isTrue);
      expect(reg.hosts, hasLength(kMaxHosts));
      expect(reg.rejectedHosts, 0);
    });

    test('a host rotating metric keys keeps at most 32 histories', () {
      for (var round = 0; round < 10; round++) {
        reg.ingest(
          pkt(
            'h1',
            seq: round + 1,
            metrics: {for (var i = 0; i < 32; i++) 'r${round}_$i': metric('R')},
          ),
        );
        t = t.add(const Duration(seconds: 1));
      }
      final h = reg.hosts['h1']!;
      expect(h.history.length, kMaxMetricKeysPerHost);
      expect(h.metricLastSeen.length, kMaxMetricKeysPerHost);
      expect(h.history.keys.every((k) => k.startsWith('r9_')), isTrue);
    });

    test(
      'any host can be removed; a live one comes back on its next packet',
      () {
        reg.ingest(pkt('h1'));
        reg.remove('h1');
        expect(reg.hosts, isEmpty);
        expect(reg.ingest(pkt('h1', seq: 2)), isTrue);
      },
    );

    test('id/host/label/unit length limits follow the schema', () {
      Map<String, dynamic> base() => packetJson();
      expect(Packet.fromJson(base()..['id'] = 'i' * 64).id, hasLength(64));
      expect(
        () => Packet.fromJson(base()..['id'] = 'i' * 65),
        throwsFormatException,
      );
      expect(Packet.fromJson(base()..['host'] = 'é' * 64).host, hasLength(64));
      expect(
        () => Packet.fromJson(base()..['host'] = 'h' * 65),
        throwsFormatException,
      );
      Map<String, dynamic> withMetric(String label, String unit) =>
          base()
            ..['metrics'] = {
              'cpu': {'v': 1, 'unit': unit, 'label': label},
            };
      expect(
        Packet.fromJson(withMetric('L' * 12, '°C' * 4)).metrics,
        hasLength(1),
      );
      expect(
        () => Packet.fromJson(withMetric('L' * 13, '%')),
        throwsFormatException,
      );
      expect(
        () => Packet.fromJson(withMetric('CPU', 'u' * 9)),
        throwsFormatException,
      );
    });

    test('control and bidi-override characters are stripped for display', () {
      final j = packetJson(
        metrics: {
          'cpu': {'v': 1, 'unit': '%\u202E', 'label': 'C\u0000PU\u2066'},
        },
      )..['host'] = 'evil\u202Etxt.exe\n\u200F\u0085';
      final p = Packet.fromJson(j);
      expect(p.host, 'eviltxt.exe');
      expect(p.metrics['cpu']!.label, 'CPU');
      expect(p.metrics['cpu']!.unit, '%');
      expect(sanitizeDisplayText('plain ünïcødé 中文'), 'plain ünïcødé 中文');
      // The raw map (hashed for auth) is untouched.
      expect(p.raw!['host'], contains('\u202E'));
    });
  });

  group('3. oversized and deeply nested input is cheap to reject', () {
    test('a 13,000-level nested 65 KB payload is rejected quickly', () {
      final deep = '{"v":1,"metrics":${'[' * 13000}${']' * 13000}}';
      expect(deep.length, greaterThan(26000));
      final payload = deep.padRight(65 * 1024);
      final intake = PacketIntake(registry: HostRegistry(), secret: () => '');
      final sw = Stopwatch()..start();
      expect(intake.handleBytes(utf8.encode(payload), 'x'), isFalse);
      expect(() => Packet.parse(payload), throwsFormatException);
      // Also through the depth pre-scan alone (no size cut-off).
      expect(jsonNestingExceeds(payload, kMaxJsonDepth), isTrue);
      sw.stop();
      // Spec target < 20 ms; generous bound so a loaded CI runner does not flake.
      expect(sw.elapsedMilliseconds, lessThan(200));
      expect(intake.parseErrors, 1);
      expect(intake.lastError, contains('larger than 8192'));
    });

    test('nesting deeper than 16 is rejected even under the size cap', () {
      String nested(int depth) => jsonEncode(packetJson()).replaceFirst(
        '"metrics":{',
        '"x":${'[' * (depth - 1)}${']' * (depth - 1)},"metrics":{',
      );
      // Root object is depth 1, so the array run adds depth - 1 more levels.
      expect(Packet.parse(nested(16)).id, 'h1');
      final deep = nested(17);
      expect(deep.length, lessThan(kMaxPacketBytes));
      expect(
        () => Packet.parse(deep),
        throwsA(
          isA<FormatException>().having(
            (e) => e.message,
            'message',
            contains('nested'),
          ),
        ),
      );
    });

    test('brackets inside strings do not count towards depth', () {
      final j = packetJson()..['note'] = '[[[[[[[[[[[[[[[[[[[[{{{{{{{{{{\\"]]]';
      expect(Packet.parse(jsonEncode(j)).id, 'h1');
      expect(jsonNestingExceeds(r'{"a":"\"[[[[["}', 2), isFalse);
      expect(jsonNestingExceeds('{"a":[[1]]}', 2), isTrue);
    });

    test('datagrams and lines over 8 KiB are rejected before decoding', () {
      final intake = PacketIntake(registry: HostRegistry(), secret: () => '');
      final ok = packetJson()..['pad'] = 'p' * 7000;
      expect(intake.handleText(jsonEncode(ok), 'x'), isTrue);
      final big = packetJson(seq: 2)..['pad'] = 'p' * 8200;
      // Invalid UTF-8 would be a different error: size is checked first.
      final bytes = [...utf8.encode(jsonEncode(big)), 0xFF];
      expect(intake.handleBytes(bytes, 'x'), isFalse);
      expect(intake.lastError, contains('larger than'));
      expect(() => Packet.parse(jsonEncode(big)), throwsFormatException);
    });

    test('non-finite path is still reported, lazily', () {
      final text = jsonEncode(packetJson())
          .replaceFirst('"v":1,"unit"', '"v":1e400,"unit"');
      expect(
        () => Packet.parse(text),
        throwsA(
          isA<FormatException>().having(
            (e) => e.message,
            'message',
            'Packet field "metrics" field "cpu" field "v" contains a non-finite number',
          ),
        ),
      );
    });
  });

  group('5. signed packets cannot be replayed', () {
    const key = 'correct horse';
    late DateTime t;
    late HostRegistry registry;
    late PacketIntake intake;
    setUp(() {
      t = DateTime.utc(2026, 11, 11, 12);
      registry = HostRegistry(now: () => t);
      intake = PacketIntake(registry: registry, secret: () => key);
    });

    String signed({required int ts, int seq = 1, String id = 'h1'}) {
      final j = packetJson(id: id, ts: ts, seq: seq);
      j['auth'] = Packet.computeAuth(j, key);
      return jsonEncode(j);
    }

    int now() => t.millisecondsSinceEpoch;

    test('a validly signed packet 2 h old is rejected as stale', () {
      final old = signed(ts: now() - const Duration(hours: 2).inMilliseconds);
      expect(intake.handleText(old, 'x'), isFalse);
      expect(intake.authFailures, 1);
      expect(intake.lastError, contains('stale timestamp'));
      expect(registry.hosts, isEmpty);
      // Same for a ts in the future.
      expect(
        intake.handleText(signed(ts: now() + 6 * 60 * 1000), 'x'),
        isFalse,
      );
      expect(intake.authFailures, 2);
      // Within the window is fine.
      expect(intake.handleText(signed(ts: now() - 4 * 60 * 1000), 'x'), isTrue);
    });

    test('ts must strictly increase per id', () {
      expect(intake.handleText(signed(ts: now(), seq: 10), 'x'), isTrue);
      expect(intake.handleText(signed(ts: now() + 1000, seq: 11), 'x'), isTrue);
      // Out-of-order / replayed ts, even with a fresh seq.
      expect(intake.handleText(signed(ts: now() + 500, seq: 12), 'x'), isFalse);
      expect(intake.authFailures, 1);
      expect(intake.lastError, contains('replayed or out-of-order'));
      // Another id has its own sequence.
      expect(intake.handleText(signed(ts: now(), id: 'h2'), 'x'), isTrue);
      expect(intake.handleText(signed(ts: now() + 2000, seq: 13), 'x'), isTrue);
    });

    test('an exact repeat (multi-interface broadcast) is a duplicate', () {
      final p = signed(ts: now(), seq: 5);
      expect(intake.handleText(p, 'a'), isTrue);
      expect(intake.handleText(p, 'b'), isFalse);
      expect(intake.authFailures, 0);
      expect(registry.droppedOutOfOrder, 1);
      expect(kAuthClockWindow, const Duration(minutes: 5));
    });

    test('without a secret old and reordered ts are accepted as before', () {
      final open = PacketIntake(registry: registry, secret: () => '');
      final old = packetJson(ts: 1000, seq: 1);
      expect(open.handleText(jsonEncode(old), 'x'), isTrue);
      expect(open.authFailures, 0);
    });
  });
}
