import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/ui/host_detail_screen.dart';
import 'package:rtm_viewer/ui/host_order.dart';
import 'package:rtm_viewer/ui/settings_screen.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'ui_fixtures.dart';

List<String> ids(List<HostState> hs) => [for (final h in hs) h.id];

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    PackageInfo.setMockInitialValues(
      appName: 'RTM Viewer',
      packageName: 'rtm_viewer',
      version: '1.0.0',
      buildNumber: '1',
      buildSignature: '',
    );
  });

  group('orderHosts', () {
    final auto = [makeHost('a', 'a'), makeHost('b', 'b'), makeHost('c', 'c')];

    test('empty order keeps the automatic order', () {
      expect(ids(orderHosts(auto, const [])), ['a', 'b', 'c']);
      expect(ids(orderHosts(const [], const ['a'])), isEmpty);
    });

    test('partial order: listed first, then the rest automatically', () {
      expect(ids(orderHosts(auto, const ['c'])), ['c', 'a', 'b']);
      expect(ids(orderHosts(auto, const ['b', 'a'])), ['b', 'a', 'c']);
      expect(ids(orderHosts(auto, const ['c', 'b', 'a'])), ['c', 'b', 'a']);
    });

    test('unknown and duplicate ids are ignored', () {
      expect(ids(orderHosts(auto, const ['zz', 'b', 'b', 'gone'])), [
        'b',
        'a',
        'c',
      ]);
    });

    test('dropInto takes the target slot', () {
      expect(dropInto(['a', 'b', 'c'], 'a', 'c'), ['b', 'c', 'a']);
      expect(dropInto(['a', 'b', 'c'], 'c', 'a'), ['c', 'a', 'b']);
      expect(dropInto(['a', 'b', 'c'], 'a', 'b'), ['b', 'a', 'c']);
      expect(dropInto(['a', 'b'], 'a', 'x'), ['a', 'b']);
    });
  });

  Future<UiHarness> pumpThree(WidgetTester tester) async {
    setScreen(tester, 360, 640);
    final h = UiHarness(
      hosts: [
        makeHost('a', 'alpha'),
        makeHost('b', 'beta'),
        makeHost('c', 'gamma'),
      ],
    );
    h.config.showDetails = false;
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    return h;
  }

  List<String> cardOrder(WidgetTester tester) {
    final ys = {
      for (final id in ['a', 'b', 'c'])
        id: tester.getTopLeft(find.byKey(ValueKey('host-card-$id'))).dy,
    };
    return ys.keys.toList()..sort((x, y) => ys[x]!.compareTo(ys[y]!));
  }

  testWidgets('long-press drag card 1 onto card 3 reorders and saves', (
    tester,
  ) async {
    final h = await pumpThree(tester);
    expect(cardOrder(tester), ['a', 'b', 'c']);
    final from = tester.getCenter(find.byKey(const ValueKey('host-card-a')));
    final to = tester.getCenter(find.byKey(const ValueKey('host-card-c')));
    final g = await tester.startGesture(from);
    await tester.pump(kLongPressTimeout + const Duration(milliseconds: 100));
    for (var i = 1; i <= 10; i++) {
      await g.moveTo(Offset.lerp(from, to, i / 10)!);
      await tester.pump(const Duration(milliseconds: 16));
    }
    await g.up();
    await tester.pumpAndSettle();
    expect(h.config.hostOrder, ['b', 'c', 'a']);
    expect(cardOrder(tester), ['b', 'c', 'a']);
    final prefs = await SharedPreferences.getInstance();
    expect(
      prefs.getString('display_config_v1'),
      contains('"hostOrder":["b","c","a"]'),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('a quick tap still opens the detail screen', (tester) async {
    final h = await pumpThree(tester);
    await tester.tap(find.text('beta'));
    await tester.pumpAndSettle();
    expect(find.byType(HostDetailScreen), findsOneWidget);
    expect(h.config.hostOrder, isEmpty);
  });

  testWidgets('removing an offline host drops it from the order', (
    tester,
  ) async {
    setScreen(tester, 360, 1200);
    final h = UiHarness(
      hosts: [
        makeHost('a', 'alpha'),
        makeHost(
          'b',
          'beta',
          status: HostStatus.offline,
          lastSeen: DateTime.now().subtract(const Duration(minutes: 2)),
        ),
      ],
    );
    h.config.setHostOrder(['b', 'a']);
    await tester.pumpWidget(h.app());
    await tester.pump(const Duration(seconds: 1));
    expect(cardOrder2(tester), ['b', 'a'], reason: 'offline keeps its slot');
    await tester.tap(find.text('Remove'));
    await tester.pump();
    expect(h.config.hostOrder, ['a']);
  });

  testWidgets('settings host list reorders and resets', (tester) async {
    setScreen(tester, 400, 3000);
    final h = UiHarness(
      hosts: [
        makeHost('a', 'alpha'),
        makeHost('b', 'beta'),
        makeHost('c', 'gamma'),
      ],
    );
    await tester.pumpWidget(h.app());
    await tester.pump();
    tester
        .state<NavigatorState>(find.byType(Navigator))
        .pushNamed(SettingsScreen.route);
    await tester.pumpAndSettle();
    for (final id in ['a', 'b', 'c']) {
      expect(find.byKey(ValueKey('host-row-$id')), findsOneWidget);
    }
    final reset = find.byKey(const ValueKey('reset-host-order'));
    expect(tester.widget<TextButton>(reset).onPressed, isNull);
    // Drag gamma's handle above alpha.
    final handle = find.descendant(
      of: find.byKey(const ValueKey('host-row-c')),
      matching: find.byIcon(Icons.drag_indicator),
    );
    final top = tester.getTopLeft(find.byKey(const ValueKey('host-row-a')));
    final g = await tester.startGesture(tester.getCenter(handle));
    await tester.pump(kLongPressTimeout);
    // Move in steps until the dragged row's centre passes alpha's top.
    final start = tester.getCenter(handle);
    final target = top + const Offset(20, -24);
    for (var i = 1; i <= 10; i++) {
      await g.moveTo(Offset.lerp(start, target, i / 10)!);
      await tester.pump(const Duration(milliseconds: 50));
    }
    await g.up();
    await tester.pumpAndSettle();
    expect(h.config.hostOrder, ['c', 'a', 'b']);
    await tester.tap(reset);
    await tester.pump();
    expect(h.config.hostOrder, isEmpty);
    expect(tester.takeException(), isNull);
  });
}

List<String> cardOrder2(WidgetTester tester) {
  final ys = {
    for (final id in ['a', 'b'])
      id: tester.getTopLeft(find.byKey(ValueKey('host-card-$id'))).dy,
  };
  return ys.keys.toList()..sort((x, y) => ys[x]!.compareTo(ys[y]!));
}
