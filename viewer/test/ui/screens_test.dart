import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rtm_viewer/ui/host_detail_screen.dart';
import 'package:rtm_viewer/ui/settings_screen.dart';

import 'ui_fixtures.dart';

void main() {
  setUp(() {
    PackageInfo.setMockInitialValues(
      appName: 'RTM Viewer',
      packageName: 'rtm_viewer',
      version: '1.0.0',
      buildNumber: '1',
      buildSignature: '',
    );
  });

  testWidgets('settings screen renders all sections', (tester) async {
    setScreen(tester, 400, 3000);
    final h = UiHarness(hosts: [makeHost('a', 'workstation')]);
    await tester.pumpWidget(h.app());
    await tester.pump();
    final nav = tester.state<NavigatorState>(find.byType(Navigator));
    nav.pushNamed(SettingsScreen.route);
    await tester.pumpAndSettle();
    for (final s in ['NETWORK', 'DISPLAY', 'METRICS', 'HOSTS', 'ABOUT']) {
      expect(find.text(s), findsOneWidget, reason: s);
    }
    expect(find.text('UDP port'), findsOneWidget);
    expect(find.text('TCP port'), findsOneWidget);
    expect(find.byKey(const ValueKey('shared-secret')), findsOneWidget);
    expect(find.byKey(const ValueKey('diagnostics')), findsOneWidget);
    expect(find.textContaining('appear here automatically'), findsOneWidget);
    for (final k in ['cpu', 'gpu', 'mem', 'disk']) {
      expect(find.byKey(ValueKey('slot-$k')), findsOneWidget);
    }
    expect(find.text('Version 1.0.0+1'), findsOneWidget);
    expect(find.text('workstation'), findsOneWidget);

    // Toggles go through the config setters.
    expect(h.config.showDetails, isTrue, reason: 'default on');
    await tester.tap(find.text('Show sparklines on cards'));
    await tester.pump();
    expect(h.config.showDetails, isFalse);
    expect(find.textContaining('PLAN.md'), findsNothing);
    expect(find.textContaining('PROTOCOL.md'), findsNothing);
    await tester.tap(find.text('Light'));
    await tester.pump();
    expect(h.config.themeMode, ThemeMode.light);
    await tester.tap(find.text('Listen on TCP'));
    await tester.pump();
    expect(h.config.tcpEnabled, isFalse);
    expect(find.text('TCP port'), findsNothing);
    h.net.dispose(); // cancels the debounced restart the port change scheduled
    expect(tester.takeException(), isNull);
  });

  testWidgets('host detail renders detail table and packet info', (
    tester,
  ) async {
    setScreen(tester, 400, 3000);
    final h = UiHarness(hosts: [makeHost('a', 'workstation')]);
    await tester.pumpWidget(h.app());
    await tester.pump();
    final nav = tester.state<NavigatorState>(find.byType(Navigator));
    nav.pushNamed(HostDetailScreen.route, arguments: 'a');
    await tester.pumpAndSettle();
    expect(find.widgetWithText(AppBar, 'workstation'), findsOneWidget);
    expect(find.text('CPU DETAILS'), findsOneWidget);
    expect(find.text('cores'), findsOneWidget);
    expect(find.text('12, 44.1, 8.3, 29'), findsOneWidget);
    expect(find.text('NVIDIA GeForce RTX 3070'), findsOneWidget);
    expect(find.text('disks.nvme0.write_mbps'), findsOneWidget);
    expect(find.text('48213'), findsOneWidget);
    expect(find.text('1000 ms'), findsOneWidget);
    expect(find.text('192.168.1.10:51234 udp'), findsOneWidget);
    expect(find.byKey(const ValueKey('detail-spark-disk')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('host detail in landscape lays gauges 4 across', (tester) async {
    setScreen(tester, 900, 400);
    final h = UiHarness(hosts: [makeHost('a', 'workstation')]);
    await tester.pumpWidget(h.app());
    await tester.pump();
    tester
        .state<NavigatorState>(find.byType(Navigator))
        .pushNamed(HostDetailScreen.route, arguments: 'a');
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}
