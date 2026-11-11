import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/net/multicast_lock.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  tearDown(
    () => messenger.setMockMethodCallHandler(MulticastLock.channel, null),
  );

  test('acquire / isHeld / release go through the channel', () async {
    final calls = <String>[];
    var held = false;
    var wifiHeld = false;
    messenger.setMockMethodCallHandler(MulticastLock.channel, (call) async {
      calls.add(call.method);
      switch (call.method) {
        case 'acquire':
          held = true;
          wifiHeld = true;
          return true;
        case 'release':
          held = false;
          wifiHeld = false;
          return true;
        case 'isHeld':
          return held;
        case 'isWifiLockHeld':
          return wifiHeld;
      }
      return null;
    });
    expect(await MulticastLock.acquire(), isTrue);
    expect(await MulticastLock.isHeld(), isTrue);
    expect(await MulticastLock.wifiLockHeld(), isTrue);
    expect(await MulticastLock.release(), isTrue);
    expect(await MulticastLock.isHeld(), isFalse);
    expect(await MulticastLock.wifiLockHeld(), isFalse);
    expect(calls, [
      'acquire',
      'isHeld',
      'isWifiLockHeld',
      'release',
      'isHeld',
      'isWifiLockHeld',
    ]);
  });

  test('platform error returns false', () async {
    messenger.setMockMethodCallHandler(MulticastLock.channel, (call) async {
      throw PlatformException(code: 'boom');
    });
    expect(await MulticastLock.acquire(), isFalse);
  });

  test(
    'without a handler every call returns false and does not throw',
    () async {
      expect(await MulticastLock.acquire(), isFalse);
      expect(await MulticastLock.release(), isFalse);
      expect(await MulticastLock.isHeld(), isFalse);
      expect(await MulticastLock.wifiLockHeld(), isFalse);
    },
  );

  test(
    'off Android every call returns false without touching the channel',
    () async {
      var called = false;
      messenger.setMockMethodCallHandler(MulticastLock.channel, (call) async {
        called = true;
        return true;
      });
      debugDefaultTargetPlatformOverride = TargetPlatform.linux;
      try {
        expect(await MulticastLock.acquire(), isFalse);
      } finally {
        debugDefaultTargetPlatformOverride = null;
      }
      expect(called, isFalse);
    },
  );
}
