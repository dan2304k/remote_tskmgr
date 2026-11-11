import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:wakelock_plus/wakelock_plus.dart';

import '../config/display_config.dart';
import '../core/host_registry.dart';
import '../net/net_controller.dart';
import 'dashboard_screen.dart';
import 'host_detail_screen.dart';
import 'settings_screen.dart';
import 'theme.dart';

/// Root widget: providers, theme, routes and app lifecycle.
///
/// While the app is in the background (paused / hidden) the network is stopped (sockets,
/// the 1 s timer and the Android multicast / Wi-Fi locks are released) and the screen
/// wakelock is dropped; both come back on resume. [manageWakelock] is off in tests.
class RtmApp extends StatefulWidget {
  const RtmApp({
    super.key,
    required this.config,
    required this.registry,
    required this.net,
    this.manageWakelock = true,
    this.setWakelock,
  });

  final DisplayConfig config;
  final HostRegistry registry;
  final NetController net;

  /// Hold the screen on while in the foreground and config.keepAwake is set.
  final bool manageWakelock;

  /// Wakelock toggle; defaults to the wakelock_plus plugin. Injectable for tests.
  final Future<void> Function(bool on)? setWakelock;

  @override
  State<RtmApp> createState() => _RtmAppState();
}

class _RtmAppState extends State<RtmApp> {
  late final AppLifecycleListener _lifecycle;
  bool _foreground = true;
  bool? _wakeOn;

  @override
  void initState() {
    super.initState();
    _lifecycle = AppLifecycleListener(onStateChange: _onLifecycle);
    widget.config.addListener(_syncWakelock);
    _syncWakelock();
  }

  @override
  void didUpdateWidget(RtmApp old) {
    super.didUpdateWidget(old);
    if (!identical(old.config, widget.config)) {
      old.config.removeListener(_syncWakelock);
      widget.config.addListener(_syncWakelock);
      _syncWakelock();
    }
  }

  void _onLifecycle(AppLifecycleState s) {
    switch (s) {
      case AppLifecycleState.paused || AppLifecycleState.hidden:
        if (!_foreground) return;
        _foreground = false;
        unawaited(_guard('stop', widget.net.stop));
      case AppLifecycleState.resumed:
        if (_foreground) return;
        _foreground = true;
        unawaited(_guard('start', widget.net.start));
      case AppLifecycleState.inactive || AppLifecycleState.detached:
        return;
    }
    _syncWakelock();
  }

  static Future<void> _guard(String what, Future<void> Function() op) async {
    try {
      await op();
    } catch (e) {
      debugPrint('rtm: net $what failed: $e');
    }
  }

  void _syncWakelock() {
    if (!widget.manageWakelock) return;
    final want = _foreground && widget.config.keepAwake;
    if (want == _wakeOn) return;
    _wakeOn = want;
    unawaited(_applyWakelock(want));
  }

  Future<void> _applyWakelock(bool on) async {
    try {
      await (widget.setWakelock ?? _pluginWakelock)(on);
    } catch (e) {
      debugPrint('rtm: wakelock unavailable: $e');
    }
  }

  static Future<void> _pluginWakelock(bool on) =>
      WakelockPlus.toggle(enable: on);

  @override
  void dispose() {
    _lifecycle.dispose();
    widget.config.removeListener(_syncWakelock);
    if (widget.manageWakelock && _wakeOn == true) {
      unawaited(_applyWakelock(false));
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MultiProvider(
      providers: [
        ChangeNotifierProvider<DisplayConfig>.value(value: widget.config),
        ChangeNotifierProvider<HostRegistry>.value(value: widget.registry),
        ChangeNotifierProvider<NetController>.value(value: widget.net),
      ],
      // Only the theme mode rebuilds the root; everything else selects its own config.
      child: Selector<DisplayConfig, ThemeMode>(
        selector: (_, cfg) => cfg.themeMode,
        builder: (context, themeMode, _) => MaterialApp(
          title: 'RTM Viewer',
          debugShowCheckedModeBanner: false,
          theme: rtmLightTheme,
          darkTheme: rtmDarkTheme,
          themeMode: themeMode,
          initialRoute: DashboardScreen.route,
          onGenerateRoute: (s) => MaterialPageRoute(
            settings: s,
            builder: (_) => switch (s.name) {
              HostDetailScreen.route => HostDetailScreen(
                hostId: s.arguments as String? ?? '',
              ),
              SettingsScreen.route => const SettingsScreen(),
              _ => const DashboardScreen(),
            },
          ),
        ),
      ),
    );
  }
}
