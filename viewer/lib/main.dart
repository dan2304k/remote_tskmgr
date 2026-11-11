import 'dart:async';

import 'package:flutter/material.dart';

import 'config/display_config.dart';
import 'core/host_registry.dart';
import 'net/net_controller.dart';
import 'ui/app.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  DisplayConfig config;
  try {
    config = await DisplayConfig.load();
  } catch (e) {
    debugPrint('rtm: config load failed, using defaults: $e');
    config = DisplayConfig();
  }
  final registry = HostRegistry();
  final net = NetController(registry: registry, config: config);
  runApp(RtmApp(config: config, registry: registry, net: net));
  WidgetsBinding.instance.addPostFrameCallback((_) {
    unawaited(
      net.start().catchError((Object e) {
        debugPrint('rtm: net start failed: $e');
      }),
    );
  });
}
