import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/ui/host_detail_screen.dart';
import 'package:rtm_viewer/ui/settings_screen.dart';

import 'ui_fixtures.dart';

void main() {
  setUp(() {
    PackageInfo.setMockInitialValues(
      appName: 'RTM Viewer',
      packageName: 'rtm_viewer',
      version: '1.0.2',
      buildNumber: '3',
      buildSignature: '',
    );
  });

  Future<UiHarness> openSettings(WidgetTester tester) async {
    setScreen(tester, 360, 4000);
    final h = UiHarness(hosts: [makeHost('a', 'alpha'), makeHost('b', 'beta')]);
    await tester.pumpWidget(h.app());
    await tester.pump();
    tester
        .state<NavigatorState>(find.byType(Navigator))
        .pushNamed(SettingsScreen.route);
    await tester.pumpAndSettle();
    return h;
  }

  testWidgets('shared secret field explains the unsigned risk', (tester) async {
    await openSettings(tester);
    expect(
      find.text(
        'Without a secret any device on this network can send fake stats.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('swipe deletes a metric slot; Reset metrics restores defaults', (
    tester,
  ) async {
    final h = await openSettings(tester);
    h.config.ensureSlots(['junk_key']);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('slot-junk_key')), findsOneWidget);
    await tester.drag(
      find.byKey(const ValueKey('slot-junk_key')),
      const Offset(-600, 0),
    );
    await tester.pumpAndSettle();
    expect(h.config.slotFor('junk_key'), isNull);
    expect(find.byKey(const ValueKey('slot-junk_key')), findsNothing);
    h.config.removeSlot('cpu');
    h.config.ensureSlots(['net']);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('reset-metrics')));
    await tester.pumpAndSettle();
    expect(h.config.slots.map((s) => s.key), kDefaultSlotKeys);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a live host can be removed from Settings', (tester) async {
    final h = await openSettings(tester);
    h.config.setHostOrder(['b', 'a']);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('remove-host-b')));
    await tester.pumpAndSettle();
    expect(h.registry.hosts.keys, ['a']);
    expect(h.config.hostOrder, ['a']);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a host can be removed from its detail screen', (tester) async {
    setScreen(tester, 360, 2000);
    final h = UiHarness(hosts: [makeHost('a', 'alpha')]);
    await tester.pumpWidget(h.app());
    await tester.pump();
    tester
        .state<NavigatorState>(find.byType(Navigator))
        .pushNamed(HostDetailScreen.route, arguments: 'a');
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('detail-remove-host')));
    await tester.pumpAndSettle();
    expect(h.registry.hosts, isEmpty);
    expect(find.byType(HostDetailScreen), findsNothing);
  });
}
