import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../config/display_config.dart';
import '../core/host_registry.dart';
import '../net/net_controller.dart';
import 'dashboard_screen.dart';
import 'host_detail_screen.dart';
import 'settings_screen.dart';
import 'theme.dart';

/// Root widget: providers, theme and routes. [manageWakelock] is off in tests.
class RtmApp extends StatelessWidget {
  const RtmApp({
    super.key,
    required this.config,
    required this.registry,
    required this.net,
    this.manageWakelock = true,
  });

  final DisplayConfig config;
  final HostRegistry registry;
  final NetController net;
  final bool manageWakelock;

  @override
  Widget build(BuildContext context) {
    return MultiProvider(
      providers: [
        ChangeNotifierProvider<DisplayConfig>.value(value: config),
        ChangeNotifierProvider<HostRegistry>.value(value: registry),
        ChangeNotifierProvider<NetController>.value(value: net),
      ],
      child: Consumer<DisplayConfig>(
        builder: (context, cfg, _) => MaterialApp(
          title: 'RTM Viewer',
          debugShowCheckedModeBanner: false,
          theme: rtmLightTheme,
          darkTheme: rtmDarkTheme,
          themeMode: cfg.themeMode,
          initialRoute: DashboardScreen.route,
          onGenerateRoute: (s) => MaterialPageRoute(
            settings: s,
            builder: (_) => switch (s.name) {
              HostDetailScreen.route => HostDetailScreen(
                hostId: s.arguments as String? ?? '',
              ),
              SettingsScreen.route => const SettingsScreen(),
              _ => DashboardScreen(manageWakelock: manageWakelock),
            },
          ),
        ),
      ),
    );
  }
}
