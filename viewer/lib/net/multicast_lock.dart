// Android Wi-Fi MulticastLock via a tiny platform channel (see MulticastLockPlugin.kt).
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Many Android Wi-Fi drivers drop broadcast/multicast frames unless an app holds a
/// `WifiManager.MulticastLock`; without it the sender's UDP broadcasts never arrive
/// while unicast still does. [acquire] also takes a low-latency `WifiLock` (high-perf
/// below API 29) so the radio stays out of power save; otherwise broadcasts arrive only
/// at DTIM beacons, in bursts seconds apart. [release] drops both.
///
/// Every call returns false (never throws) off Android or when no platform handler is
/// registered (tests).
abstract final class MulticastLock {
  static const channel = MethodChannel('rtm_viewer/multicast_lock');

  /// Acquires both locks; returns whether the multicast lock is held.
  static Future<bool> acquire() => _call('acquire');

  /// Releases both locks.
  static Future<bool> release() => _call('release');

  /// Whether the multicast lock is held.
  static Future<bool> isHeld() => _call('isHeld');

  /// Whether the Wi-Fi (low-latency / high-perf) lock is held.
  static Future<bool> wifiLockHeld() => _call('isWifiLockHeld');

  static Future<bool> _call(String method) async {
    if (kIsWeb || defaultTargetPlatform != TargetPlatform.android) return false;
    try {
      return await channel.invokeMethod<bool>(method) ?? false;
    } on MissingPluginException {
      return false;
    } on PlatformException catch (e) {
      debugPrint('MulticastLock.$method failed: ${e.message}');
      return false;
    }
  }
}
