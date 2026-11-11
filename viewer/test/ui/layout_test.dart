import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/ui/format.dart';
import 'package:rtm_viewer/ui/settings_screen.dart';
import 'package:rtm_viewer/ui/widgets/bar_row.dart';
import 'package:rtm_viewer/ui/widgets/gauge_arc.dart';
import 'package:rtm_viewer/ui/widgets/line_graph.dart';
import 'package:rtm_viewer/ui/widgets/sparkline.dart';

import 'ui_fixtures.dart';

RingBuffer ring(List<double?> xs, {int capacity = 120}) {
  final r = RingBuffer(capacity);
  xs.forEach(r.add);
  return r;
}

/// Paints [p] onto a throwaway canvas of [size].
void paintOnce(CustomPainter p, [Size size = const Size(120, 60)]) {
  final rec = ui.PictureRecorder();
  p.paint(Canvas(rec), size);
  rec.endRecording().dispose();
}

T painterOf<T extends CustomPainter>(WidgetTester tester, Finder within) =>
    tester
            .widgetList<CustomPaint>(
              find.descendant(of: within, matching: find.byType(CustomPaint)),
            )
            .map((w) => w.painter)
            .firstWhere((p) => p is T)
        as T;

void main() {
  recencyAndSmoothingTests();

  setUp(() {
    PackageInfo.setMockInitialValues(
      appName: 'RTM Viewer',
      packageName: 'rtm_viewer',
      version: '1.0.0',
      buildNumber: '1',
      buildSignature: '',
    );
  });

  group('peakOf', () {
    test('empty / null ring / zero window -> null', () {
      expect(peakOf(ring([]), 5), isNull);
      expect(peakOf(null, 5), isNull);
      expect(peakOf(ring([1, 2, 3]), 0), isNull);
      expect(peakOf(ring([1, 2, 3]), -1), isNull);
    });

    test('all null -> null', () {
      expect(peakOf(ring([null, null, null]), 2), isNull);
      expect(peakOf(ring([9, null, null]), 2), isNull);
    });

    test('window smaller than history uses only recent samples', () {
      final r = ring([90, 10, 20, null, 30]);
      expect(peakOf(r, 3), 30);
      expect(peakOf(r, 4), 30);
      expect(peakOf(r, 5), 90);
      expect(peakOf(ring([5, 80, 10]), 2), 80);
    });

    test('window larger than history uses all samples', () {
      expect(peakOf(ring([90, 10, 20]), 50), 90);
      expect(peakOf(ring([null, 4]), 200), 4);
    });

    test('peak drops once it ages out of the sliding window', () {
      final r = ring([10, 95, 20]);
      expect(peakOf(r, 3), 95);
      r.add(15);
      expect(peakOf(r, 3), 95);
      r.add(12);
      expect(peakOf(r, 3), 20);
    });

    test('window samples from hold and interval', () {
      expect(peakWindowSamples(5, const Duration(seconds: 1)), 5);
      expect(peakWindowSamples(5, const Duration(seconds: 2)), 3);
      expect(peakWindowSamples(1, const Duration(milliseconds: 300)), 4);
      expect(peakWindowSamples(0, const Duration(seconds: 1)), 0);
      final h = makeHost('a', 'a');
      // makeHost ends with the current value after 30 ramp samples (1 s interval).
      final hist = h.history['cpu']!.values.toList();
      final last5 = hist.sublist(hist.length - 5).whereType<double>();
      expect(peakFor(h, 'cpu', 5), last5.reduce((a, b) => a > b ? a : b));
      expect(peakFor(h, 'cpu', 0), isNull);
      expect(peakFor(h, 'nope', 5), isNull);
    });
  });

  group('painters', () {
    for (final pk in [null, 0.0, 0.5, 1.0, 1.7]) {
      test('paint with peak $pk', () {
        paintOnce(
          GaugeArcPainter(
            fraction: 0.4,
            color: Colors.teal,
            track: Colors.grey,
            stroke: 8,
            peakFraction: pk,
          ),
          const Size(80, 80),
        );
        paintOnce(
          HBarPainter(
            fraction: 0.4,
            color: Colors.teal,
            track: Colors.grey,
            barHeight: 10,
            peakFraction: pk,
          ),
          const Size(200, 16),
        );
        paintOnce(
          SparklinePainter(
            values: const [1, 2, null, 3, 50],
            min: 0,
            max: null,
            color: Colors.teal,
            peak: pk == null ? null : pk * 50,
            plotFill: Colors.black12,
            plotBorder: Colors.grey,
            gridColor: Colors.grey,
            fillColor: areaFillFor(Colors.teal, dark: true),
          ),
          const Size(160, 80),
        );
      });
    }

    test('degenerate sizes do not throw', () {
      paintOnce(
        GaugeArcPainter(
          fraction: 1,
          color: Colors.teal,
          track: Colors.grey,
          stroke: 4,
          peakFraction: 1,
        ),
        const Size(6, 6),
      );
      paintOnce(
        HBarPainter(
          fraction: 1,
          color: Colors.teal,
          track: Colors.grey,
          barHeight: 10,
          peakFraction: 0,
        ),
        const Size(1, 4),
      );
    });

    testWidgets('widgets render with and without peak', (tester) async {
      for (final pk in [null, 80.0]) {
        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: SizedBox(
                width: 300,
                child: Column(
                  children: [
                    SizedBox(
                      width: 100,
                      child: GaugeArc(
                        value: 40,
                        unit: '%',
                        label: 'CPU',
                        max: 100,
                        peak: pk,
                      ),
                    ),
                    BarRow(
                      value: 40,
                      unit: '%',
                      label: 'CPU',
                      max: 100,
                      peak: pk,
                    ),
                    LineGraph(
                      ring: ring([10, 80, 40]),
                      value: 40,
                      unit: '%',
                      label: 'CPU',
                      max: 100,
                      peak: pk,
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull);
        final g = painterOf<GaugeArcPainter>(tester, find.byType(GaugeArc));
        final b = painterOf<HBarPainter>(tester, find.byType(BarRow));
        final l = painterOf<SparklinePainter>(tester, find.byType(LineGraph));
        if (pk == null) {
          expect(g.peakFraction, isNull);
          expect(b.peakFraction, isNull);
          expect(l.peak, isNull);
        } else {
          expect(g.peakFraction, closeTo(0.8, 1e-9));
          expect(b.peakFraction, closeTo(0.8, 1e-9));
          expect(l.peak, 80);
          // % graphs use a fixed 0..100 scale, boxed, with a clear top edge.
          expect(l.max, 100);
          expect(l.plotFill, isNotNull);
          expect(l.plotBorder, isNotNull);
          expect(l.boundedTop, isTrue);
        }
      }
    });
  });

  group('dashboard layouts', () {
    testWidgets('cycle button walks all four modes in order', (tester) async {
      setScreen(tester, 360, 780);
      final h = UiHarness(hosts: [makeHost('a', 'workstation')]);
      await tester.pumpWidget(h.app());
      await tester.pump();
      expect(find.byTooltip('Layout: Dials (tap for Bars)'), findsOneWidget);
      final order = [
        CardLayout.bars,
        CardLayout.barTiles,
        CardLayout.lines,
        CardLayout.custom,
        CardLayout.dials,
      ];
      for (final want in order) {
        await tester.tap(find.byKey(const ValueKey('layout-cycle')));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 300));
        expect(h.config.cardLayout, want);
        expect(
          find.byTooltip('Layout: ${want.label} (tap for ${want.next.label})'),
          findsOneWidget,
        );
        expect(
          find.descendant(
            of: find.byType(SnackBar),
            matching: find.text(want.label),
          ),
          findsOneWidget,
        );
        expect(tester.takeException(), isNull, reason: '$want');
      }
      await tester.pump(const Duration(seconds: 3));
    });

    testWidgets('bars mode renders one bar row per visible slot', (
      tester,
    ) async {
      setScreen(tester, 360, 1200);
      final h = UiHarness(
        hosts: [makeHost('a', 'alpha'), makeHost('b', 'beta')],
      );
      h.config.setSlotVisible('gpu', false);
      h.config.cardLayout = CardLayout.bars;
      await tester.pumpWidget(h.app());
      await tester.pump(const Duration(seconds: 1));
      expect(find.byType(BarRow), findsNWidgets(6));
      for (final k in ['cpu', 'mem', 'disk']) {
        expect(find.byKey(ValueKey('bar-a-$k')), findsOneWidget);
      }
      expect(find.byKey(const ValueKey('bar-a-gpu')), findsNothing);
      expect(find.byType(GaugeArc), findsNothing);
      // Each row spans (nearly) the card width; rows stack vertically.
      final cpu = tester.getRect(find.byKey(const ValueKey('bar-a-cpu')));
      final mem = tester.getRect(find.byKey(const ValueKey('bar-a-mem')));
      expect(cpu.width, greaterThan(280));
      expect(mem.top, greaterThan(cpu.bottom - 0.1));
      // Peak tick on by default (5 s).
      final p = painterOf<HBarPainter>(
        tester,
        find.byKey(const ValueKey('bar-a-cpu')),
      );
      expect(p.peakFraction, isNotNull);
      expect(tester.takeException(), isNull);
    });

    testWidgets('lines mode renders one line graph per slot, 2 per row', (
      tester,
    ) async {
      setScreen(tester, 360, 1200);
      final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
      h.config.cardLayout = CardLayout.lines;
      await tester.pumpWidget(h.app());
      await tester.pump(const Duration(seconds: 1));
      expect(find.byType(LineGraph), findsNWidgets(4));
      final ys = [
        for (final k in ['cpu', 'gpu', 'mem', 'disk'])
          tester.getTopLeft(find.byKey(ValueKey('line-a-$k'))).dy,
      ];
      expect(ys[0], ys[1]);
      expect(ys[2], greaterThan(ys[0]));
      expect(ys[2], ys[3]);
      expect(tester.takeException(), isNull);
    });

    for (final (w, hgt) in [(1280.0, 800.0), (640.0, 360.0)]) {
      testWidgets('lines mode puts 4 across at ${w}x$hgt', (tester) async {
        setScreen(tester, w, hgt);
        final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
        h.config.cardLayout = CardLayout.lines;
        await tester.pumpWidget(h.app());
        await tester.pump(const Duration(seconds: 1));
        final ys = {
          for (final k in ['cpu', 'gpu', 'mem', 'disk'])
            tester.getTopLeft(find.byKey(ValueKey('line-a-$k'))).dy,
        };
        expect(ys, hasLength(1));
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('bar tiles mode renders one dial-sized tile per visible slot', (
      tester,
    ) async {
      setScreen(tester, 360, 780);
      final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
      h.config.setSlotVisible('disk', false);
      h.config.cardLayout = CardLayout.barTiles;
      await tester.pumpWidget(h.app());
      await tester.pump(const Duration(seconds: 1));
      expect(find.byType(BarGaugeTile), findsNWidgets(3));
      expect(find.byType(GaugeArc), findsNothing);
      final ys = {
        for (final k in ['cpu', 'gpu', 'mem'])
          tester.getTopLeft(find.byKey(ValueKey('tile-a-$k'))).dy,
      };
      expect(ys, hasLength(1));
      final r = tester.getRect(find.byType(BarGaugeTile).first);
      expect(r.width, closeTo(r.height, 0.01));
      final p = painterOf<HBarPainter>(
        tester,
        find.byKey(const ValueKey('tile-a-cpu')),
      );
      expect(p.peakFraction, isNotNull);
      expect(p.tickWidth, 2);
      expect(p.cornerRadius, 3);
      // Title row holds label and value; nothing is drawn inside the bar.
      final title = find.descendant(
        of: find.byKey(const ValueKey('tile-a-cpu')),
        matching: find.byKey(const ValueKey('bar-tile-title')),
      );
      expect(
        find.descendant(of: title, matching: find.text('CPU')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: title, matching: find.textContaining('23')),
        findsOneWidget,
      );
      final bar = tester.getRect(
        find.descendant(
          of: find.byKey(const ValueKey('tile-a-cpu')),
          matching: find.byKey(const ValueKey('bar-tile-bar')),
        ),
      );
      expect(bar.height, greaterThan(r.height * 0.5), reason: 'fat bar');
      expect(tester.takeException(), isNull);
    });

    testWidgets('4 bar tiles fit one row on a 360 dp phone like dials', (
      tester,
    ) async {
      setScreen(tester, 360, 640);
      final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
      final dialYs = <double>{};
      for (final l in [CardLayout.dials, CardLayout.barTiles]) {
        h.config.cardLayout = l;
        await tester.pumpWidget(h.app());
        await tester.pump(const Duration(seconds: 1));
        final ys = {
          for (final k in ['cpu', 'gpu', 'mem', 'disk'])
            tester.getTopLeft(find.byKey(ValueKey('tile-a-$k'))).dy,
        };
        expect(ys, hasLength(1), reason: '$l');
        dialYs.addAll(ys);
      }
      expect(dialYs, hasLength(1));
    });

    testWidgets(
      'line graphs: no text in plot, thin line, colour-variant fill',
      (tester) async {
        setScreen(tester, 360, 1200);
        final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
        h.config.cardLayout = CardLayout.lines;
        await tester.pumpWidget(h.app());
        await tester.pump(const Duration(seconds: 1));
        final disk = painterOf<SparklinePainter>(
          tester,
          find.byKey(const ValueKey('line-a-disk')),
        );
        expect(disk.max, isNull);
        expect(disk.boundedTop, isFalse);
        expect(disk.strokeWidth, 1.6);
        expect(disk.fillColor, isNot(disk.color));
        // The only text in a line graph tile is the header label + value.
        final texts = tester.widgetList<RichText>(
          find.descendant(
            of: find.byKey(const ValueKey('line-a-disk')),
            matching: find.byType(RichText),
          ),
        );
        expect(texts, hasLength(2));
        final cpu = painterOf<SparklinePainter>(
          tester,
          find.byKey(const ValueKey('line-a-cpu')),
        );
        expect(cpu.max, 100);
      },
    );

    testWidgets('dials mode forces gauges; custom honours slot styles', (
      tester,
    ) async {
      setScreen(tester, 360, 780);
      final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
      h.config.setSlotStyle('mem', TileStyle.number);
      await tester.pumpWidget(h.app());
      await tester.pump(const Duration(seconds: 1));
      expect(find.byType(GaugeArc), findsNWidgets(4));
      h.config.cardLayout = CardLayout.custom;
      await tester.pump(const Duration(seconds: 1));
      expect(find.byType(GaugeArc), findsNWidgets(3));
    });

    testWidgets('peak hold 0 removes the gauge peak tick', (tester) async {
      setScreen(tester, 360, 780);
      final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
      await tester.pumpWidget(h.app());
      await tester.pump(const Duration(seconds: 1));
      final tile = find.byKey(const ValueKey('tile-a-cpu'));
      expect(painterOf<GaugeArcPainter>(tester, tile).peakFraction, isNotNull);
      h.config.peakHoldSeconds = 0;
      await tester.pump(const Duration(seconds: 1));
      expect(painterOf<GaugeArcPainter>(tester, tile).peakFraction, isNull);
    });

    testWidgets('settings selects layout and shows peak hold', (tester) async {
      setScreen(tester, 360, 3000);
      final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
      await tester.pumpWidget(h.app());
      await tester.pump();
      tester
          .state<NavigatorState>(find.byType(Navigator))
          .pushNamed(SettingsScreen.route);
      await tester.pumpAndSettle();
      expect(find.text('Card layout'), findsOneWidget);
      expect(find.text('5 s'), findsOneWidget);
      expect(find.textContaining('only in the Custom layout'), findsOneWidget);
      await tester.tap(
        find.descendant(
          of: find.byKey(const ValueKey('layout-segments')),
          matching: find.text('Lines'),
        ),
      );
      await tester.pump();
      expect(h.config.cardLayout, CardLayout.lines);
      h.config.peakHoldSeconds = 0;
      await tester.pump();
      expect(find.text('Off'), findsOneWidget);
      // Drag the slider to the far right -> 60 s.
      final slider = find.byKey(const ValueKey('peak-hold-slider'));
      await tester.drag(slider, const Offset(1000, 0));
      await tester.pump();
      expect(h.config.peakHoldSeconds, 60);
      await tester.tap(
        find.descendant(
          of: find.byKey(const ValueKey('layout-segments')),
          matching: find.text('Custom'),
        ),
      );
      await tester.pump();
      expect(find.textContaining('style chosen here'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    for (final (w, hgt) in [(360.0, 640.0), (640.0, 360.0), (800.0, 1280.0)]) {
      for (final layout in [
        CardLayout.bars,
        CardLayout.barTiles,
        CardLayout.lines,
      ]) {
        testWidgets('no overflow in ${layout.name} at ${w}x$hgt', (
          tester,
        ) async {
          setScreen(tester, w, hgt);
          final metrics = {
            ...standardMetrics(),
            'net': const Metric(v: 123456.7, unit: 'MB/s', label: 'NETWORK-IO'),
            'uptime': const Metric(v: 123456, unit: 's', label: 'UP'),
          };
          final h = UiHarness(
            hosts: [
              makeHost(
                'a',
                'a-very-long-hostname-for-overflow',
                metrics: metrics,
              ),
              makeHost('b', 'beta', metrics: {...metrics}..remove('gpu')),
              makeHost(
                'c',
                'gamma',
                metrics: metrics,
                status: HostStatus.offline,
              ),
            ],
          );
          h.config.ensureSlot('net', isPercent: false);
          h.config.ensureSlot('uptime', isPercent: false);
          h.config.cardLayout = layout;
          for (final d in GridDensity.values) {
            h.config.density = d;
            await tester.pumpWidget(h.app());
            await tester.pump(const Duration(seconds: 1));
            expect(tester.takeException(), isNull, reason: '$d');
          }
          final rows = switch (layout) {
            CardLayout.bars => BarRow,
            CardLayout.barTiles => BarGaugeTile,
            _ => LineGraph,
          };
          expect(find.byType(rows), findsWidgets);
        });
      }
    }
  });
}

void recencyAndSmoothingTests() {
  group('metric recency', () {
    testWidgets(
      'host that stops reporting mem loses the tile; other host keeps it',
      (tester) async {
        setScreen(tester, 360, 1200);
        var now = DateTime(2026, 11, 11, 12);
        final reg = HostRegistry(now: () => now);
        final h = UiHarness(registry: reg);
        Packet p(String id, int seq, Map<String, Metric> m) => Packet(
          v: 1,
          id: id,
          host: id,
          os: 'linux',
          ts: 1762640000000 + seq * 1000,
          seq: seq,
          intervalMs: 1000,
          metrics: m,
        );
        final all = standardMetrics();
        final noMem = {...all}..remove('mem');
        reg.ingest(p('a', 1, all));
        reg.ingest(p('b', 1, all));
        await tester.pumpWidget(h.app());
        await tester.pump();
        expect(find.byKey(const ValueKey('tile-a-mem')), findsOneWidget);
        // a drops mem; both keep sending for 10 s.
        for (var s = 2; s <= 11; s++) {
          now = now.add(const Duration(seconds: 1));
          reg.ingest(p('a', s, noMem));
          reg.ingest(p('b', s, all));
        }
        await tester.pump();
        expect(
          find.byKey(const ValueKey('missing-mem')),
          findsOneWidget,
          reason: 'within the window: placeholder',
        );
        // Window passes without a new packet: the 1 s tick removes the tile.
        now = now.add(const Duration(milliseconds: 900));
        reg.tick(now);
        await tester.pump();
        expect(find.byKey(const ValueKey('tile-a-mem')), findsNothing);
        expect(find.byKey(const ValueKey('tile-b-mem')), findsOneWidget);
        expect(find.byKey(const ValueKey('tile-a-cpu')), findsOneWidget);
        expect(h.config.slotFor('mem'), isNotNull, reason: 'global slot kept');
        expect(tester.takeException(), isNull);
      },
    );
  });

  group('smoothing', () {
    test('smoothedValue', () {
      expect(smoothedValue(ring([]), 3), isNull);
      expect(smoothedValue(null, 3), isNull);
      expect(smoothedValue(ring([1, 2, 3]), 0), isNull);
      expect(smoothedValue(ring([1, 2, 7]), 1), 7);
      expect(smoothedValue(ring([1, 2, 3, 4]), 2), 3.5);
      expect(
        smoothedValue(ring([1, 6, null, 2]), 3),
        4,
        reason: 'null skipped',
      );
      expect(smoothedValue(ring([4, 8]), 10), 6, reason: 'fewer than n');
      expect(
        smoothedValue(ring([4, 8, null]), 2),
        8,
        reason: 'newest null keeps the recent value',
      );
      expect(
        smoothedValue(ring([4, null, null]), 2),
        isNull,
        reason: 'all-null window',
      );
      expect(smoothedValue(ring([null, null, null]), 10), isNull);
    });

    test('displayValue bridges nulls, blanks only all-null windows', () {
      HostState host(int intervalMs, List<double?> xs) {
        final h = HostState(
          id: 'g',
          latest: Packet(
            v: 1,
            id: 'g',
            host: 'g',
            os: 'windows',
            ts: 1,
            seq: 1,
            intervalMs: intervalMs,
            metrics: {'gpu': Metric(v: xs.last, unit: '%', label: 'GPU')},
          ),
          lastSeen: DateTime.now(),
        );
        h.history['gpu'] = ring(xs);
        return h;
      }

      // Smoothing off (1 s sender): last non-null within 3 samples.
      var h = host(1000, [0, null, 0, null]);
      expect(displayValue(h, 'gpu', null, 1000), 0);
      h = host(1000, [7, null, null]);
      expect(displayValue(h, 'gpu', null, 0), 7);
      h = host(1000, [7, null, null, null]);
      expect(displayValue(h, 'gpu', null, 1000), isNull);
      // Permanently null (iGPU).
      h = host(1000, [null, null]);
      expect(displayValue(h, 'gpu', null, 1000), isNull);
      // Smoothing on (5/s): mean of non-null in the 5-sample window.
      h = host(200, [9, 0, null, 0, null, 4, null]);
      // Newest 5: [null, 0, null, 4, null] -> mean of 0 and 4.
      expect(displayValue(h, 'gpu', null, 1000), 2);
      h = host(200, [9, null, null, null, null, null]);
      expect(displayValue(h, 'gpu', null, 1000), isNull);
    });

    test('smoothingSamples', () {
      expect(smoothingSamples(1000, const Duration(milliseconds: 100)), 10);
      expect(smoothingSamples(1000, const Duration(milliseconds: 200)), 5);
      expect(smoothingSamples(1000, const Duration(milliseconds: 300)), 4);
      expect(smoothingSamples(1000, const Duration(seconds: 1)), 1);
      expect(smoothingSamples(1000, const Duration(seconds: 2)), 1);
      expect(smoothingSamples(0, const Duration(milliseconds: 100)), 1);
    });

    testWidgets('fast host shows the moving average; peak stays raw', (
      tester,
    ) async {
      setScreen(tester, 360, 780);
      final pk = Packet(
        v: 1,
        id: 'f',
        host: 'fast',
        os: 'linux',
        ts: 1,
        seq: 1,
        intervalMs: 200,
        metrics: {
          'cpu': const Metric(v: 50, unit: '%', label: 'CPU', max: 100),
        },
      );
      final host = HostState(id: 'f', latest: pk, lastSeen: DateTime.now());
      host.metricLastSeen['cpu'] = DateTime.now();
      final r = host.history['cpu'] = RingBuffer(120);
      for (final v in <double>[99, 10, 20, 30, 40, 50]) {
        r.add(v);
      }
      final h = UiHarness(hosts: [host]);
      h.config.peakHoldSeconds = 1; // 5 samples: 10..50
      h.config.smoothAnimation = false; // test the average, not the glide
      await tester.pumpWidget(h.app());
      await tester.pump(const Duration(seconds: 1));
      // 5 samples (1000 / 200) -> mean of 10..50 = 30.
      expect(find.text('30'), findsOneWidget);
      final g = painterOf<GaugeArcPainter>(
        tester,
        find.byKey(const ValueKey('tile-f-cpu')),
      );
      expect(g.peakFraction, closeTo(0.5, 1e-9));
      h.config.smoothingWindowMs = 0;
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('50'), findsOneWidget);
    });
  });
}
