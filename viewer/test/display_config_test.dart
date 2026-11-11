import 'dart:convert';

import 'package:flutter/material.dart' show ThemeMode;
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:shared_preferences/shared_preferences.dart';

Future<Map<String, dynamic>> stored() async {
  final prefs = await SharedPreferences.getInstance();
  return jsonDecode(prefs.getString(kDisplayConfigPrefsKey)!)
      as Map<String, dynamic>;
}

/// Let fire-and-forget saves complete.
Future<void> settle() => Future<void>.delayed(Duration.zero);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() => SharedPreferences.setMockInitialValues({}));

  layoutAndPeakTests();
  migrationTests();
  smoothingConfigTests();

  test('defaults on first run', () async {
    final c = await DisplayConfig.load();
    expect(c.slots.map((s) => s.key), ['cpu', 'gpu', 'mem', 'disk']);
    expect(
      c.slots.every((s) => s.visible && s.style == TileStyle.gauge),
      isTrue,
    );
    expect(c.themeMode, ThemeMode.dark);
    expect(c.udpPort, 48000);
    expect(c.tcpPort, 48000);
    expect(c.tcpEnabled, isTrue);
    expect(c.keepAwake, isTrue);
    expect(c.sharedSecret, '');
    expect(c.showDetails, isTrue, reason: 'sparklines default on');
    expect(c.density, GridDensity.comfortable);
  });

  test('ensureSlot appends once, always as a gauge', () async {
    final c = await DisplayConfig.load();
    var n = 0;
    c.addListener(() => n++);
    c.ensureSlot('fan_rpm');
    c.ensureSlot('battery');
    c.ensureSlot('fan_rpm');
    c.ensureSlot('cpu');
    expect(n, 2);
    expect(c.slots.map((s) => s.key), [
      'cpu',
      'gpu',
      'mem',
      'disk',
      'fan_rpm',
      'battery',
    ]);
    expect(c.slotFor('fan_rpm')!.style, TileStyle.gauge);
    expect(c.slotFor('battery')!.style, TileStyle.gauge);
    await settle();
    expect((await stored())['slots'], hasLength(6));
  });

  test('hidden slot is not re-added; visibleSlots', () async {
    final c = await DisplayConfig.load();
    c.setSlotVisible('gpu', false);
    c.ensureSlot('gpu');
    expect(c.slots, hasLength(4));
    expect(c.visibleSlots.map((s) => s.key), ['cpu', 'mem', 'disk']);
  });

  test('reorder uses onReorderItem semantics', () async {
    final c = await DisplayConfig.load();
    c.reorder(0, 3);
    expect(c.slots.map((s) => s.key), ['gpu', 'mem', 'disk', 'cpu']);
    c.reorder(3, 0);
    expect(c.slots.map((s) => s.key), ['cpu', 'gpu', 'mem', 'disk']);
    c.reorder(1, 99);
    expect(c.slots.map((s) => s.key), ['cpu', 'mem', 'disk', 'gpu']);
  });

  test('every setter notifies and persists; round trip', () async {
    final c = await DisplayConfig.load();
    var n = 0;
    c.addListener(() => n++);
    c
      ..showDetails = false
      ..themeMode = ThemeMode.light
      ..density = GridDensity.compact
      ..keepAwake = false
      ..udpPort = 48001
      ..tcpEnabled = false
      ..tcpPort = 48002
      ..sharedSecret = 's3cret';
    expect(n, 8);
    c.showDetails = false;
    expect(n, 8, reason: 'unchanged value does not notify');
    c.setSlotStyle('disk', TileStyle.sparkline);
    c.reorder(3, 0);
    await settle();

    final d = await DisplayConfig.load();
    expect(
      d.showDetails,
      isFalse,
      reason: 'explicit off survives (flag saved)',
    );
    expect(d.themeMode, ThemeMode.light);
    expect(d.density, GridDensity.compact);
    expect(d.keepAwake, isFalse);
    expect(d.udpPort, 48001);
    expect(d.tcpEnabled, isFalse);
    expect(d.tcpPort, 48002);
    expect(d.sharedSecret, 's3cret');
    expect(d.slots.map((s) => s.key), ['disk', 'cpu', 'gpu', 'mem']);
    expect(d.slots.first.style, TileStyle.sparkline);
  });

  test('invalid port rejected', () async {
    final c = await DisplayConfig.load();
    expect(() => c.udpPort = 0, throwsRangeError);
    expect(() => c.tcpPort = 70000, throwsRangeError);
    expect(c.udpPort, 48000);
  });

  test('corrupt stored JSON falls back to defaults', () async {
    SharedPreferences.setMockInitialValues({
      kDisplayConfigPrefsKey: '{not json',
    });
    final c = await DisplayConfig.load();
    expect(c.slots, hasLength(4));
  });

  test('partially wrong stored JSON keeps valid fields', () async {
    SharedPreferences.setMockInitialValues({
      kDisplayConfigPrefsKey: jsonEncode({
        'slots': [
          {'key': 'net', 'visible': false, 'style': 'bar'},
          {'key': 'net'},
          42,
          {'key': 'cpu', 'style': 'bogus'},
        ],
        'udpPort': 'x',
        'themeMode': 'system',
      }),
    });
    final c = await DisplayConfig.load();
    expect(c.slots.map((s) => s.key), ['net', 'cpu']);
    expect(c.slots[0].visible, isFalse);
    expect(c.slots[0].style, TileStyle.bar);
    expect(c.slots[1].style, TileStyle.gauge);
    expect(c.udpPort, 48000);
    expect(c.themeMode, ThemeMode.system);
  });
}

void layoutAndPeakTests() {
  test(
    'cardLayout and peakHoldSeconds default, persist and round trip',
    () async {
      final c = await DisplayConfig.load();
      expect(c.cardLayout, CardLayout.dials);
      expect(c.peakHoldSeconds, kDefaultPeakHoldSeconds);
      expect(c.peakHoldSeconds, 5);
      var n = 0;
      c.addListener(() => n++);
      c.cardLayout = CardLayout.lines;
      c.peakHoldSeconds = 12.5;
      expect(n, 2);
      c.cardLayout = CardLayout.lines;
      c.peakHoldSeconds = 12.5;
      expect(n, 2, reason: 'unchanged value does not notify');
      await settle();
      expect((await stored())['cardLayout'], 'lines');
      expect((await stored())['peakHoldSeconds'], 12.5);
      final d = await DisplayConfig.load();
      expect(d.cardLayout, CardLayout.lines);
      expect(d.peakHoldSeconds, 12.5);
    },
  );

  test('peakHoldSeconds clamps to 0..60 and ignores NaN', () async {
    final c = await DisplayConfig.load();
    c.peakHoldSeconds = -3;
    expect(c.peakHoldSeconds, 0);
    c.peakHoldSeconds = 600;
    expect(c.peakHoldSeconds, 60);
    c.peakHoldSeconds = double.nan;
    expect(c.peakHoldSeconds, 60);
    SharedPreferences.setMockInitialValues({
      kDisplayConfigPrefsKey: jsonEncode({
        'peakHoldSeconds': 999,
        'cardLayout': 'bogus',
      }),
    });
    final d = await DisplayConfig.load();
    expect(d.peakHoldSeconds, 60);
    expect(d.cardLayout, CardLayout.dials);
    SharedPreferences.setMockInitialValues({
      kDisplayConfigPrefsKey: jsonEncode({
        'peakHoldSeconds': 'x',
        'cardLayout': 'barTiles',
      }),
    });
    final e = await DisplayConfig.load();
    expect(e.peakHoldSeconds, 5);
    expect(e.cardLayout, CardLayout.barTiles);
  });

  test('CardLayout.next cycles dials -> bars -> lines -> custom -> dials', () {
    var l = CardLayout.dials;
    final seen = <CardLayout>[];
    for (var i = 0; i < 5; i++) {
      l = l.next;
      seen.add(l);
    }
    expect(seen, [
      CardLayout.bars,
      CardLayout.barTiles,
      CardLayout.lines,
      CardLayout.custom,
      CardLayout.dials,
    ]);
    expect(CardLayout.bars.label, 'Bars');
    expect(CardLayout.barTiles.label, 'Bar tiles');
  });
}

void smoothingConfigTests() {
  test('smoothingWindowMs default, clamp, persist', () async {
    final c = await DisplayConfig.load();
    expect(c.smoothingWindowMs, 1000);
    c.smoothingWindowMs = 9000;
    expect(c.smoothingWindowMs, kMaxSmoothingWindowMs);
    c.smoothingWindowMs = -5;
    expect(c.smoothingWindowMs, 0);
    c.smoothingWindowMs = 2500;
    await settle();
    expect((await stored())['smoothingWindowMs'], 2500);
    expect((await DisplayConfig.load()).smoothingWindowMs, 2500);
    SharedPreferences.setMockInitialValues({
      kDisplayConfigPrefsKey: jsonEncode({'smoothingWindowMs': 99999}),
    });
    expect((await DisplayConfig.load()).smoothingWindowMs, 5000);
  });

  test('smoothAnimation defaults on and persists', () async {
    final c = await DisplayConfig.load();
    expect(c.smoothAnimation, isTrue);
    var n = 0;
    c.addListener(() => n++);
    c.smoothAnimation = false;
    expect(n, 1);
    await settle();
    expect((await stored())['smoothAnimation'], isFalse);
    expect((await DisplayConfig.load()).smoothAnimation, isFalse);
  });
}

void migrationTests() {
  test('hostOrder persists; move, remove, reset', () async {
    final c = await DisplayConfig.load();
    expect(c.hostOrder, isEmpty);
    var n = 0;
    c.addListener(() => n++);
    c.setHostOrder(['b', 'a', 'b', '']);
    expect(c.hostOrder, ['b', 'a']);
    c.setHostOrder(['b', 'a']);
    expect(n, 1, reason: 'unchanged order does not notify');
    c.setHostOrder(['a', 'b', 'z']);
    expect(c.hostOrder, ['a', 'b', 'z']);
    c.removeHostFromOrder('b');
    await settle();
    expect((await stored())['hostOrder'], ['a', 'z']);
    expect((await DisplayConfig.load()).hostOrder, ['a', 'z']);
    c.resetHostOrder();
    await settle();
    expect((await DisplayConfig.load()).hostOrder, isEmpty);
  });

  test('stored config without the v3 flag gets sparklines on once', () async {
    SharedPreferences.setMockInitialValues({
      kDisplayConfigPrefsKey: jsonEncode({
        'showDetails': false,
        'slotsMigratedV2': true,
      }),
    });
    final c = await DisplayConfig.load();
    expect(c.showDetails, isTrue);
    final saved = await stored();
    expect(saved[kSparklinesDefaultOnKey], isTrue);
    expect(saved['showDetails'], isTrue);
    c.showDetails = false;
    await settle();
    expect((await DisplayConfig.load()).showDetails, isFalse);
  });

  test('new non-% slot (net MB/s) defaults to gauge', () async {
    final c = await DisplayConfig.load();
    c.ensureSlot('net');
    expect(c.slotFor('net')!.style, TileStyle.gauge);
    await settle();
    expect((await stored())[kSlotsMigratedV2Key], isTrue);
  });

  test(
    'legacy number slots migrate to gauge once and the flag is saved',
    () async {
      SharedPreferences.setMockInitialValues({
        kDisplayConfigPrefsKey: jsonEncode({
          'slots': [
            {'key': 'cpu', 'style': 'gauge'},
            {'key': 'net', 'style': 'number'},
            {'key': 'mem', 'style': 'bar'},
            {'key': 'uptime', 'visible': false, 'style': 'number'},
          ],
        }),
      });
      final c = await DisplayConfig.load();
      expect(c.slots.map((s) => s.style), [
        TileStyle.gauge,
        TileStyle.gauge,
        TileStyle.bar,
        TileStyle.gauge,
      ]);
      expect(c.slotFor('uptime')!.visible, isFalse);
      final saved = await stored();
      expect(saved[kSlotsMigratedV2Key], isTrue);
      expect((saved['slots'] as List)[1]['style'], 'gauge');
    },
  );

  test('number chosen after migration is kept', () async {
    final c = await DisplayConfig.load();
    c.ensureSlot('net');
    c.setSlotStyle('net', TileStyle.number);
    await settle();
    final d = await DisplayConfig.load();
    expect(d.slotFor('net')!.style, TileStyle.number);
  });
}
