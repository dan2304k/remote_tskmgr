import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/ui/theme.dart';
import 'package:rtm_viewer/ui/widgets/gauge_arc.dart';
import 'package:rtm_viewer/ui/widgets/metric_tile.dart';
import 'package:rtm_viewer/ui/widgets/sparkline.dart';
import 'package:rtm_viewer/ui/widgets/value_follower.dart';

const ms16 = Duration(milliseconds: 16);

void main() {
  group('follower math', () {
    test('converges toward the target without overshooting', () {
      for (final (from, to) in [(0.0, 100.0), (100.0, 0.0), (-5.0, 5.0)]) {
        var v = from;
        var prevGap = (to - from).abs();
        for (var i = 0; i < 200; i++) {
          v = followStep(v, to, ms16, const Duration(milliseconds: 250));
          final gap = (to - v).abs();
          expect(gap, lessThanOrEqualTo(prevGap));
          // Never past the target.
          expect((v - from).abs(), lessThanOrEqualTo((to - from).abs()));
          prevGap = gap;
        }
        expect(prevGap, lessThan(0.01));
      }
    });

    test('one tau covers ~63 %; tau 0 snaps; dt 0 holds', () {
      const tau = Duration(milliseconds: 250);
      expect(followStep(0, 100, tau, tau), closeTo(63.2, 0.1));
      expect(followStep(0, 100, ms16, Duration.zero), 100);
      expect(followStep(7, 100, Duration.zero, tau), 7);
    });

    test('tau by interval', () {
      const t250 = Duration(milliseconds: 250);
      const t500 = Duration(milliseconds: 500);
      expect(followerTau(const Duration(milliseconds: 100)), t250);
      expect(followerTau(const Duration(milliseconds: 999)), t250);
      expect(followerTau(const Duration(seconds: 1)), t500);
      expect(followerTau(const Duration(seconds: 10)), t500);
    });

    test('settle threshold is 0.4 % of range (display precision)', () {
      expect(followerSettled(99.65, 100, 100), isTrue);
      expect(followerSettled(99.5, 100, 100), isFalse);
      expect(followerSettled(4, 0, 1000), isTrue);
      expect(followerSettled(5, 0, 1000), isFalse);
    });
  });

  group('ValueFollower widget', () {
    Widget follower(double? target, {bool enabled = true}) => Directionality(
      textDirection: TextDirection.ltr,
      child: ValueFollower(
        target: target,
        tau: const Duration(milliseconds: 250),
        range: 100,
        enabled: enabled,
        builder: (_, v) => Text('${v ?? '-'}'),
      ),
    );

    ValueFollowerState state(WidgetTester t) =>
        t.state<ValueFollowerState>(find.byType(ValueFollower));

    testWidgets('glides through intermediate values, then stops ticking', (
      tester,
    ) async {
      await tester.pumpWidget(follower(0));
      expect(state(tester).shown, 0, reason: 'first value snaps');
      expect(state(tester).isAnimating, isFalse);
      await tester.pumpWidget(follower(100));
      final seen = <double>[];
      for (var i = 0; i < 20; i++) {
        await tester.pump(ms16);
        seen.add(state(tester).shown!);
      }
      final mid = seen.where((v) => v > 0 && v < 100).toList();
      expect(mid.length, greaterThan(10));
      for (var i = 1; i < seen.length; i++) {
        expect(seen[i], greaterThanOrEqualTo(seen[i - 1]));
      }
      expect(state(tester).isAnimating, isTrue);
      for (var i = 0; i < 300 && state(tester).isAnimating; i++) {
        await tester.pump(ms16);
      }
      expect(state(tester).shown, 100);
      expect(state(tester).isAnimating, isFalse);
      expect(tester.binding.hasScheduledFrame, isFalse);
    });

    testWidgets('disabled or null target snaps', (tester) async {
      await tester.pumpWidget(follower(0, enabled: false));
      await tester.pumpWidget(follower(80, enabled: false));
      expect(state(tester).shown, 80);
      expect(state(tester).isAnimating, isFalse);
      await tester.pumpWidget(follower(80));
      await tester.pumpWidget(follower(null));
      expect(state(tester).shown, isNull);
      expect(tester.binding.hasScheduledFrame, isFalse);
    });
  });

  testWidgets('gauge tile shows intermediate values between two packets', (
    tester,
  ) async {
    Packet pkt(int seq, double v) => Packet(
      v: 1,
      id: 'a',
      host: 'a',
      os: 'linux',
      ts: seq * 1000,
      seq: seq,
      intervalMs: 1000,
      metrics: {'cpu': Metric(v: v, unit: '%', label: 'CPU', max: 100)},
    );
    final host = HostState(
      id: 'a',
      latest: pkt(1, 10),
      lastSeen: DateTime.now(),
    );
    final ring = host.history['cpu'] = RingBuffer(120)..add(10);
    Widget app() => MaterialApp(
      theme: rtmDarkTheme,
      home: Center(
        child: SizedBox(
          width: 120,
          child: MetricTile(
            host: host,
            slot: MetricSlot(key: 'cpu'),
            smoothAnimation: true,
          ),
        ),
      ),
    );
    double shown() => tester.widget<GaugeArc>(find.byType(GaugeArc)).value!;
    await tester.pumpWidget(app());
    expect(shown(), 10);
    host.latest = pkt(2, 90);
    ring.add(90);
    await tester.pumpWidget(app());
    final values = <double>[];
    for (var i = 0; i < 30; i++) {
      await tester.pump(ms16);
      values.add(shown());
    }
    expect(values.any((v) => v > 20 && v < 80), isTrue);
    expect(values.last, lessThan(90));
    // Colour ramp follows the displayed value, not the target.
    final painter = tester
        .widgetList<CustomPaint>(
          find.descendant(
            of: find.byType(GaugeArc),
            matching: find.byType(CustomPaint),
          ),
        )
        .map((w) => w.painter)
        .whereType<GaugeArcPainter>()
        .first;
    expect(painter.color, RtmColors.dark.forValue(shown(), max: 100));
    await tester.pump(const Duration(seconds: 10));
    await tester.pump(const Duration(seconds: 10));
    expect(shown(), 90);
    expect(tester.binding.hasScheduledFrame, isFalse);
  });

  testWidgets('line graph scrolls between packets and stops at one slot', (
    tester,
  ) async {
    final p = Packet(
      v: 1,
      id: 'a',
      host: 'a',
      os: 'linux',
      ts: 1,
      seq: 1,
      intervalMs: 500,
      metrics: {'cpu': const Metric(v: 5, unit: '%', label: 'CPU', max: 100)},
    );
    final host = HostState(id: 'a', latest: p, lastSeen: DateTime.now());
    host.history['cpu'] = RingBuffer(120)
      ..add(1)
      ..add(5);
    await tester.pumpWidget(
      MaterialApp(
        theme: rtmDarkTheme,
        home: SizedBox(
          width: 200,
          child: ScrollPhase(
            sampleKey: host.latest.seq,
            interval: host.interval,
            builder: (_, phase) => MetricLineGraph(
              host: host,
              slot: MetricSlot(key: 'cpu'),
              smoothAnimation: true,
              phase: phase,
            ),
          ),
        ),
      ),
    );
    double phase() => tester
        .widgetList<CustomPaint>(find.byType(CustomPaint))
        .map((w) => w.painter)
        .whereType<SparklinePainter>()
        .first
        .phaseValue;
    await tester.pump(const Duration(milliseconds: 100));
    await tester.pump(const Duration(milliseconds: 150));
    expect(phase(), greaterThan(0));
    expect(phase(), lessThan(1));
    await tester.pump(const Duration(seconds: 1));
    expect(phase(), 1);
    expect(tester.binding.hasScheduledFrame, isFalse);
  });
}
