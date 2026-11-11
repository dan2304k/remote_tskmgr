import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/ui/widgets/host_card.dart';
import 'package:rtm_viewer/ui/widgets/sparkline.dart';

import 'ui_fixtures.dart';

const frame = Duration(milliseconds: 16);

/// The next packet for [id] (seq/ts one step after makeHost's), cpu/mem moved.
Packet nextPacket(String id, String name, {int step = 1}) => Packet(
  v: 1,
  id: id,
  host: name,
  os: 'linux',
  ts: 1762640000123 + step * 1000,
  seq: 48213 + step,
  intervalMs: 1000,
  metrics: {
    ...standardMetrics(),
    'cpu': Metric(v: 23.4 + 40 * step, unit: '%', label: 'CPU', max: 100),
    'mem': Metric(v: 91 - 30.0 * step, unit: '%', label: 'MEM', max: 100),
  },
);

/// Pumps 60 fps frames while one is scheduled; returns how many it took.
Future<int> pumpUntilIdle(WidgetTester tester, {int max = 1000}) async {
  var n = 0;
  while (tester.binding.hasScheduledFrame && n < max) {
    await tester.pump(frame);
    n++;
  }
  return n;
}

void main() {
  final builds = <String, int>{};
  setUp(() {
    builds.clear();
    HostCard.debugOnBuild = (id) => builds[id] = (builds[id] ?? 0) + 1;
  });
  tearDown(() => HostCard.debugOnBuild = null);

  Future<UiHarness> twoHosts(WidgetTester tester, CardLayout layout) async {
    setScreen(tester, 360, 1400);
    final h = UiHarness(hosts: [makeHost('a', 'alpha'), makeHost('b', 'beta')]);
    h.config.cardLayout = layout;
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    await pumpUntilIdle(tester);
    return h;
  }

  for (final layout in [CardLayout.dials, CardLayout.bars]) {
    testWidgets('${layout.name}: no frames once values settle', (tester) async {
      final h = await twoHosts(tester, layout);
      expect(tester.binding.hasScheduledFrame, isFalse);
      h.registry.ingest(nextPacket('a', 'alpha'));
      await tester.pump();
      expect(tester.binding.hasScheduledFrame, isTrue, reason: 'gliding');
      final frames = await pumpUntilIdle(tester);
      // tau 500 ms, settle at 0.4 % of range: ~2.3 s for a 40-point move.
      expect(frames, lessThan(200), reason: 'settles at display precision');
      expect(tester.binding.hasScheduledFrame, isFalse);
      // Quiet for a while (AgeText ticks once a second): still no animation frames.
      await tester.pump(const Duration(seconds: 3));
      expect(tester.binding.hasScheduledFrame, isFalse);
    });
  }

  for (final layout in CardLayout.values) {
    testWidgets('${layout.name}: a packet for A does not rebuild B', (
      tester,
    ) async {
      final h = await twoHosts(tester, layout);
      builds.clear();
      h.registry.ingest(nextPacket('a', 'alpha'));
      await tester.pump();
      await pumpUntilIdle(tester);
      expect(builds['a'], 1, reason: 'one rebuild per packet, not per frame');
      expect(builds['b'], isNull);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('lines: one shared phase repaints plots without rebuilding', (
    tester,
  ) async {
    final h = await twoHosts(tester, CardLayout.lines);
    h.registry.ingest(nextPacket('a', 'alpha'));
    await tester.pump();
    builds.clear();
    List<SparklinePainter> painters(String id) => tester
        .widgetList<CustomPaint>(
          find.descendant(
            of: find.byKey(ValueKey('host-card-$id')),
            matching: find.byType(CustomPaint),
          ),
        )
        .map((w) => w.painter)
        .whereType<SparklinePainter>()
        .toList();
    final pa = painters('a');
    expect(pa, hasLength(4));
    expect(pa.map((p) => p.phase).toSet(), hasLength(1), reason: 'shared');
    await tester.pump(const Duration(milliseconds: 300));
    final mid = pa.first.phaseValue;
    expect(mid, greaterThan(0));
    expect(mid, lessThan(1));
    final frames = await pumpUntilIdle(tester);
    expect(frames, lessThan(200), reason: "value text glide, then idle");
    expect(painters('a').first.phaseValue, 1);
    expect(builds, isEmpty, reason: 'scrolling repaints, never rebuilds cards');
    expect(tester.binding.hasScheduledFrame, isFalse);
  });

  test('sparkline painter repaints only on a real change', () {
    SparklinePainter p(List<double?> v, {Color color = Colors.teal}) =>
        SparklinePainter(values: v, min: 0, max: 100, color: color);
    final v = <double?>[1, 2, 3];
    expect(p(v).shouldRepaint(p(v)), isFalse);
    expect(p([1, 2, 3]).shouldRepaint(p(v)), isFalse, reason: 'equal lists');
    expect(p([1, 2, 4]).shouldRepaint(p(v)), isTrue);
    expect(p(v, color: Colors.red).shouldRepaint(p(v)), isTrue);
  });
}
