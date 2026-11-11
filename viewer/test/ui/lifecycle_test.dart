import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/net/net_controller.dart';
import 'package:rtm_viewer/ui/app.dart';
import 'package:rtm_viewer/ui/settings_screen.dart';

import 'ui_fixtures.dart';

/// Records start/stop instead of binding sockets; error getters are settable.
class FakeNet extends NetController {
  FakeNet({required super.registry, required super.config});

  final calls = <String>[];
  String? fakeBindError;
  String? fakePacketError;
  Duration? fakePacketErrorAge;

  @override
  Future<void> start() async => calls.add('start');
  @override
  Future<void> stop() async => calls.add('stop');
  @override
  Future<void> restart() async => calls.add('restart');

  @override
  String? get bindError => fakeBindError;
  @override
  String? get lastPacketError => fakePacketError;
  @override
  Duration? get lastPacketErrorAge => fakePacketErrorAge;
  @override
  String? get lastError => fakeBindError ?? fakePacketError;

  void poke() => notifyListeners();
}

void main() {
  late DisplayConfig config;
  late HostRegistry registry;
  late FakeNet net;
  late List<bool> wake;

  setUp(() {
    config = DisplayConfig();
    registry = HostRegistry();
    net = FakeNet(registry: registry, config: config);
    wake = [];
    PackageInfo.setMockInitialValues(
      appName: 'RTM Viewer',
      packageName: 'rtm_viewer',
      version: '1.0.0',
      buildNumber: '1',
      buildSignature: '',
    );
  });

  Widget app() => RtmApp(
    config: config,
    registry: registry,
    net: net,
    setWakelock: (on) async => wake.add(on),
  );

  Future<void> go(WidgetTester tester, List<AppLifecycleState> states) async {
    for (final s in states) {
      tester.binding.handleAppLifecycleStateChanged(s);
    }
    await tester.pump();
  }

  const toBackground = [
    AppLifecycleState.inactive,
    AppLifecycleState.hidden,
    AppLifecycleState.paused,
  ];
  const toForeground = [
    AppLifecycleState.hidden,
    AppLifecycleState.inactive,
    AppLifecycleState.resumed,
  ];

  group('app lifecycle', () {
    testWidgets('background stops net and drops wakelock; resume restores', (
      tester,
    ) async {
      await tester.pumpWidget(app());
      expect(wake, [true]);
      expect(net.calls, isEmpty);

      await go(tester, toBackground);
      expect(net.calls, ['stop'], reason: 'once, on hidden');
      expect(wake, [true, false]);

      await go(tester, toForeground);
      expect(net.calls, ['stop', 'start']);
      expect(wake, [true, false, true]);
    });

    testWidgets('inactive alone (e.g. notification shade) changes nothing', (
      tester,
    ) async {
      await tester.pumpWidget(app());
      await go(tester, [AppLifecycleState.inactive]);
      await go(tester, [AppLifecycleState.resumed]);
      expect(net.calls, isEmpty);
      expect(wake, [true]);
    });

    testWidgets('keepAwake off: no wakelock on resume', (tester) async {
      config.keepAwake = false;
      await tester.pumpWidget(app());
      expect(wake, [false]);
      await go(tester, toBackground);
      await go(tester, toForeground);
      expect(wake, [false]);
      expect(net.calls, ['stop', 'start']);
    });

    testWidgets('keepAwake toggled while backgrounded applies on resume', (
      tester,
    ) async {
      config.keepAwake = false;
      await tester.pumpWidget(app());
      await go(tester, toBackground);
      config.keepAwake = true;
      await tester.pump();
      expect(wake, [false], reason: 'not while in the background');
      await go(tester, toForeground);
      expect(wake, [false, true]);
    });
  });

  group('error banner', () {
    testWidgets('packet errors do not show the banner', (tester) async {
      net.fakePacketError = '1.2.3.4:5 udp: auth mismatch';
      net.fakePacketErrorAge = const Duration(seconds: 3);
      await tester.pumpWidget(app());
      await tester.pump();
      expect(find.byKey(const ValueKey('error-banner')), findsNothing);
    });

    testWidgets('bind errors show the banner; Retry restarts', (tester) async {
      await tester.pumpWidget(app());
      net.fakeBindError = 'Cannot listen on UDP 48000: port already in use';
      net.poke();
      await tester.pump();
      expect(find.byKey(const ValueKey('error-banner')), findsOneWidget);
      await tester.tap(find.text('Retry'));
      await tester.pump();
      expect(net.calls, ['restart']);
      net.fakeBindError = null;
      net.poke();
      await tester.pump();
      expect(find.byKey(const ValueKey('error-banner')), findsNothing);
    });

    testWidgets('diagnostics row shows the packet error with its age', (
      tester,
    ) async {
      setScreen(tester, 400, 3000);
      net.fakePacketError = '1.2.3.4:5 udp: auth mismatch';
      net.fakePacketErrorAge = const Duration(seconds: 12);
      await tester.pumpWidget(app());
      await tester.pump();
      tester
          .state<NavigatorState>(find.byType(Navigator))
          .pushNamed(SettingsScreen.route);
      await tester.pumpAndSettle();
      expect(find.textContaining('auth mismatch (12 s ago)'), findsOneWidget);
    });

    testWidgets('diagnostics row shows out-of-order drops', (tester) async {
      setScreen(tester, 400, 3000);
      registry.droppedOutOfOrder = 7;
      await tester.pumpWidget(app());
      await tester.pump();
      tester
          .state<NavigatorState>(find.byType(Navigator))
          .pushNamed(SettingsScreen.route);
      await tester.pumpAndSettle();
      expect(find.textContaining('7 out of order'), findsOneWidget);
    });
  });
}
