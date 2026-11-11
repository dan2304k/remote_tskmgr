import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/ui/dashboard_screen.dart';
import 'package:rtm_viewer/ui/host_detail_screen.dart';
import 'package:rtm_viewer/ui/widgets/host_card.dart';

import 'ui_fixtures.dart';

void main() {
  testWidgets('empty state shows listening port and hint', (tester) async {
    setScreen(tester, 360, 780);
    final h = UiHarness();
    await tester.pumpWidget(h.app());
    await tester.pump();
    expect(find.textContaining('Listening on UDP 48000'), findsOneWidget);
    expect(find.textContaining('TCP 48000'), findsOneWidget);
    expect(find.text('no hosts yet'), findsOneWidget);
    expect(find.textContaining('Run rtm-sender'), findsOneWidget);
    expect(find.text('RTM'), findsOneWidget);
    expect(find.byType(HostCard), findsNothing);
  });

  testWidgets('three hosts render three cards with names', (tester) async {
    setScreen(tester, 360, 1400);
    final h = UiHarness(
      hosts: [
        makeHost('a', 'workstation'),
        makeHost('b', 'gamer', os: 'windows'),
        makeHost('c', 'nuc', metrics: standardMetrics(gpu: null)),
      ],
    );
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    expect(find.byType(HostCard), findsNWidgets(3));
    for (final n in ['workstation', 'gamer', 'nuc']) {
      expect(find.text(n), findsOneWidget);
    }
    expect(find.text('CPU'), findsNWidgets(3));
    expect(find.text('23'), findsNWidgets(3)); // cpu 23.4 % -> "23"
    expect(find.text('–'), findsOneWidget); // nuc gpu null
    expect(find.byIcon(Icons.window), findsOneWidget);
    expect(find.textContaining('3 hosts'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('first card sits just below the AppBar; tiles close to header', (
    tester,
  ) async {
    setScreen(tester, 360, 640);
    tester.view.padding = const FakeViewPadding(top: 72); // 24 dp status bar
    final h = UiHarness(hosts: [makeHost('a', 'workstation')]);
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    final bar = tester.getRect(find.byType(AppBar));
    final card = tester.getRect(find.byKey(const ValueKey('host-card-a')));
    expect(card.top - bar.bottom, inInclusiveRange(4, 12));
    final name = tester.getRect(find.text('workstation'));
    final tile = tester.getRect(find.byKey(const ValueKey('tile-a-cpu')));
    expect(tile.top - name.bottom, lessThanOrEqualTo(14));
    expect(tester.takeException(), isNull);
  });

  testWidgets('4 tiles fit across one row on a 360 dp portrait phone', (
    tester,
  ) async {
    setScreen(tester, 360, 780);
    final h = UiHarness(hosts: [makeHost('a', 'workstation')]);
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    final ys = [
      for (final k in ['cpu', 'gpu', 'mem', 'disk'])
        tester.getTopLeft(find.byKey(ValueKey('tile-a-$k'))).dy,
    ];
    expect(ys.toSet(), hasLength(1));
    expect(tester.takeException(), isNull);
  });

  Future<List<Offset>> cardOrigins(WidgetTester tester, UiHarness h) async {
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    return [
      for (final id in ['a', 'b'])
        tester.getTopLeft(find.byKey(ValueKey('host-card-$id'))),
    ];
  }

  testWidgets('portrait phone uses 1 column', (tester) async {
    setScreen(tester, 360, 780);
    final h = UiHarness(hosts: [makeHost('a', 'alpha'), makeHost('b', 'beta')]);
    final o = await cardOrigins(tester, h);
    expect(o[0].dx, o[1].dx);
    expect(o[1].dy, greaterThan(o[0].dy));
    expect(dashboardColumns(const Size(360, 780), Orientation.portrait), 1);
    expect(tester.takeException(), isNull);
  });

  testWidgets('landscape phone uses 2 columns', (tester) async {
    setScreen(tester, 780, 360);
    final h = UiHarness(hosts: [makeHost('a', 'alpha'), makeHost('b', 'beta')]);
    final o = await cardOrigins(tester, h);
    expect(o[0].dy, o[1].dy);
    expect(o[1].dx, greaterThan(o[0].dx));
    expect(tester.takeException(), isNull);
  });

  test('tablet columns scale with width', () {
    expect(dashboardColumns(const Size(800, 1280), Orientation.portrait), 2);
    expect(dashboardColumns(const Size(1280, 800), Orientation.landscape), 3);
  });

  testWidgets('stale card is dimmed, offline card collapses and is removable', (
    tester,
  ) async {
    setScreen(tester, 360, 1000);
    final h = UiHarness(
      hosts: [
        makeHost('a', 'alpha', status: HostStatus.stale),
        makeHost(
          'b',
          'beta',
          status: HostStatus.offline,
          lastSeen: DateTime.now().subtract(const Duration(minutes: 2)),
        ),
      ],
    );
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    final op = tester.widget<AnimatedOpacity>(
      find.descendant(
        of: find.byKey(const ValueKey('host-card-a')),
        matching: find.byType(AnimatedOpacity),
      ),
    );
    expect(op.opacity, 0.5);
    expect(find.byKey(const ValueKey('tile-b-cpu')), findsNothing);
    expect(find.text('2m ago'), findsOneWidget);
    await tester.tap(find.text('Remove'));
    await tester.pump();
    expect(find.text('beta'), findsNothing);
    expect(h.registry.hosts.containsKey('b'), isFalse);
  });

  testWidgets(
    'recently seen slot missing from the packet is a dim placeholder',
    (tester) async {
      setScreen(tester, 360, 780);
      final host = makeHost(
        'a',
        'alpha',
        metrics: {...standardMetrics()}..remove('gpu'),
      );
      host.metricLastSeen['gpu'] = DateTime.now();
      final h = UiHarness(hosts: [host]);
      await tester.pumpWidget(h.app());
      await tester.pump(const Duration(seconds: 1));
      final ph = tester.widget<Opacity>(
        find.byKey(const ValueKey('missing-gpu')),
      );
      expect(ph.opacity, lessThan(0.5));
      expect(find.text('GPU'), findsOneWidget);
    },
  );

  testWidgets('tapping a card opens the detail screen', (tester) async {
    setScreen(tester, 360, 780);
    final h = UiHarness(hosts: [makeHost('a', 'workstation')]);
    await tester.pumpWidget(h.app());
    await tester.pump();
    await tester.tap(find.text('workstation'));
    await tester.pumpAndSettle();
    expect(find.byType(HostDetailScreen), findsOneWidget);
  });

  for (final (w, hgt) in [
    (320.0, 640.0),
    (360.0, 780.0),
    (640.0, 360.0),
    (780.0, 360.0),
    (800.0, 1280.0),
    (1280.0, 800.0),
  ]) {
    testWidgets('no overflow at ${w}x$hgt with every style and sparklines', (
      tester,
    ) async {
      setScreen(tester, w, hgt);
      final metrics = {
        ...standardMetrics(),
        'net': const Metric(v: 0.84, unit: 'MB/s', label: 'NET'),
        'uptime': const Metric(v: 123456, unit: 's', label: 'UP'),
      };
      final h = UiHarness(
        hosts: [
          makeHost('a', 'a-very-long-hostname-for-overflow', metrics: metrics),
          makeHost('b', 'beta', metrics: metrics),
          makeHost('c', 'gamma', metrics: metrics, status: HostStatus.offline),
        ],
      );
      h.config.showDetails = true;
      h.config.ensureSlot('net');
      h.config.ensureSlot('uptime');
      h.config.setSlotStyle('mem', TileStyle.bar);
      h.config.setSlotStyle('disk', TileStyle.sparkline);
      for (final d in GridDensity.values) {
        h.config.density = d;
        await tester.pumpWidget(h.app());
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull, reason: '$d');
      }
      tester
          .state<NavigatorState>(find.byType(Navigator))
          .pushNamed('/settings');
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull, reason: 'settings');
      tester
          .state<NavigatorState>(find.byType(Navigator))
          .pushReplacementNamed('/host', arguments: 'a');
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull, reason: 'detail');
    });
  }
}
