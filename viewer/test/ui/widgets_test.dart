import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/ui/format.dart';
import 'package:rtm_viewer/ui/theme.dart';
import 'package:rtm_viewer/ui/widgets/gauge_arc.dart';
import 'package:rtm_viewer/ui/widgets/metric_tile.dart';
import 'package:rtm_viewer/ui/widgets/sparkline.dart';

import 'ui_fixtures.dart';

Widget wrap(Widget child) => MaterialApp(
  theme: rtmDarkTheme,
  home: Scaffold(
    body: Center(child: SizedBox(width: 120, child: child)),
  ),
);

void main() {
  for (final (name, v, max) in [
    ('null', null, 100.0),
    ('zero', 0.0, 100.0),
    ('full', 100.0, 100.0),
    ('over', 250.0, 100.0),
  ]) {
    testWidgets('gauge paints with v $name', (tester) async {
      await tester.pumpWidget(
        wrap(GaugeArc(value: v, unit: '%', label: 'CPU', max: max)),
      );
      await tester.pump(const Duration(seconds: 1));
      expect(tester.takeException(), isNull);
      expect(find.text('CPU'), findsOneWidget);
      if (v == null) expect(find.text('–'), findsOneWidget);
      if (v == 100.0) expect(find.text('100'), findsOneWidget);
    });
  }

  testWidgets('gauge with max null auto-scales from history', (tester) async {
    final h = makeHost(
      'x',
      'x',
      metrics: {
        'disk': const Metric(v: 14.2, unit: 'MB/s', label: 'DISK'),
        'up': const Metric(v: null, unit: 's', label: 'UP'),
      },
    );
    for (final k in ['disk', 'up']) {
      for (final st in TileStyle.values) {
        await tester.pumpWidget(
          wrap(
            MetricTile(
              host: h,
              slot: MetricSlot(key: k, style: st),
              showSparkline: true,
            ),
          ),
        );
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull, reason: '$k $st');
      }
    }
    final m = h.latest.metrics['disk']!;
    expect(resolveMax(m, h.history['disk']), greaterThan(14.2));
    expect(resolveMax(const Metric(v: 0, unit: 'x', label: 'x'), null), 1.0);
  });

  testWidgets('sparkline handles empty, single, all-null, gapped series', (
    tester,
  ) async {
    for (final vals in <List<double?>>[
      [],
      [5],
      [null, null],
      [1, 2, null, 3, 4, null, 5],
      [0, 0, 0],
    ]) {
      await tester.pumpWidget(wrap(Sparkline(values: vals, capacity: 120)));
      expect(tester.takeException(), isNull);
    }
  });

  test('value colour ramp', () {
    const c = RtmColors.dark;
    expect(c.forValue(10, max: 100), c.ok);
    expect(c.forValue(70, max: 100), c.warn);
    expect(c.forValue(90, max: 100), c.hot);
    expect(c.forValue(900), c.accent); // unbounded
    expect(c.forValue(null, max: 100), c.accent);
  });

  test('formatting', () {
    expect(formatValue(23.4, '%'), (number: '23', unit: '%'));
    expect(formatValue(14.23, 'MB/s'), (number: '14.2', unit: 'MB/s'));
    expect(formatValue(0.84, 'MB/s'), (number: '0.84', unit: 'MB/s'));
    expect(formatValue(null, '%').number, '–');
    expect(formatValue(7200, 's'), (number: '2.0', unit: 'h'));
    expect(
      flattenDetail({
        'cores': [12.0, 44.1],
        'disks': {
          'sda': {'read_mbps': 1.5},
        },
      }).map((e) => '${e.key}=${e.value}'),
      ['cores=12, 44.1', 'disks.sda.read_mbps=1.5'],
    );
  });

  test('ring helper sanity', () {
    final r = RingBuffer(3)
      ..add(1)
      ..add(2)
      ..add(3)
      ..add(4);
    expect(r.values, [2, 3, 4]);
  });
}
