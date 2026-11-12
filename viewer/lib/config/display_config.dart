// Display + network settings, persisted in shared_preferences.
// Agent viewer-core owns this file.
import 'dart:collection';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart' show ThemeMode;
import 'package:shared_preferences/shared_preferences.dart';

import '../core/packet.dart' show isValidMetricKey;

/// Per-slot look (used by [CardLayout.custom]). [barTile] is a dial-sized tile with the
/// value above a horizontal bar.
enum TileStyle { gauge, bar, sparkline, number, barTile }

class MetricSlot {
  MetricSlot({
    required this.key,
    this.visible = true,
    this.style = TileStyle.gauge,
  });
  final String key;
  bool visible;
  TileStyle style;

  Map<String, Object?> toJson() => {
    'key': key,
    'visible': visible,
    'style': style.name,
  };

  /// Returns null for an entry that is not a usable slot.
  static MetricSlot? fromJson(Object? json) {
    if (json is! Map) return null;
    final key = json['key'];
    if (key is! String || key.isEmpty) return null;
    final visible = json['visible'];
    return MetricSlot(
      key: key,
      visible: visible is bool ? visible : true,
      style: _enumByName(TileStyle.values, json['style']) ?? TileStyle.gauge,
    );
  }
}

enum GridDensity { comfortable, compact }

/// How a host card renders its metrics. [dials], [bars], [barTiles] and [lines] force
/// one look for every visible slot; [custom] uses each slot's own [TileStyle].
enum CardLayout {
  dials,
  bars,
  barTiles,
  lines,
  custom;

  /// Next mode for the dashboard's cycle button (wraps around).
  CardLayout get next => values[(index + 1) % values.length];

  /// Human-readable name ("Dials").
  String get label => switch (this) {
    dials => 'Dials',
    bars => 'Bars',
    barTiles => 'Bar tiles',
    lines => 'Lines',
    custom => 'Custom',
  };
}

/// Peak hold default and range, in seconds (0 = off).
const double kDefaultPeakHoldSeconds = 5;
const double kMaxPeakHoldSeconds = 60;

/// Display smoothing default and range, in ms (0 = off).
const int kDefaultSmoothingWindowMs = 1000;
const int kMaxSmoothingWindowMs = 5000;

/// shared_preferences key holding the whole config as one JSON string.
const String kDisplayConfigPrefsKey = 'display_config_v1';

/// Metric keys shown (as gauges) before any packet has arrived.
const List<String> kDefaultSlotKeys = ['cpu', 'gpu', 'mem', 'disk'];

const int kDefaultPort = 48000;

/// Most metric slots kept. Keys first seen once the list is full get no slot.
const int kMaxSlots = 64;

/// Most entries kept in [DisplayConfig.hostOrder].
const int kMaxHostOrder = 128;

/// Stored-JSON flag: showDetails (card sparklines) was switched on once for configs
/// saved before it defaulted to on.
const String kSparklinesDefaultOnKey = 'sparklinesDefaultOnV3';

/// Stored-JSON flag: number-styled slots were converted to gauges once (see
/// [DisplayConfig.load]).
const String kSlotsMigratedV2Key = 'slotsMigratedV2';

/// What to show and how. Persisted as JSON in shared_preferences. Unknown metric keys seen
/// on the wire are auto-appended once as visible gauge slots (gauges auto-scale when the
/// metric has no max).
///
/// Every setter notifies listeners and saves (fire-and-forget; errors are swallowed so a
/// missing platform plugin in widget tests does not crash). If you mutate a [MetricSlot]
/// in [slots] directly, call [markChanged] afterwards, or use [setSlotVisible] /
/// [setSlotStyle].
class DisplayConfig extends ChangeNotifier {
  /// A config with defaults (cpu/gpu/mem/disk gauges). Not persisted until changed.
  DisplayConfig() {
    _resetToDefaults();
  }

  final List<MetricSlot> _slots = [];
  final Map<String, MetricSlot> _slotIndex = {};

  /// All slots in display order (read-only; use the methods below to change it).
  late final List<MetricSlot> slots = UnmodifiableListView(_slots);

  bool _showDetails = true;
  ThemeMode _themeMode = ThemeMode.dark;
  GridDensity _density = GridDensity.comfortable;
  bool _keepAwake = true;
  int _udpPort = kDefaultPort;
  bool _tcpEnabled = true;
  int _tcpPort = kDefaultPort;
  String _sharedSecret = '';
  CardLayout _cardLayout = CardLayout.dials;
  double _peakHoldSeconds = kDefaultPeakHoldSeconds;
  int _smoothingWindowMs = kDefaultSmoothingWindowMs;
  bool _smoothAnimation = true;
  final List<String> _hostOrder = [];

  bool get showDetails => _showDetails;
  set showDetails(bool v) => _set(_showDetails != v, () => _showDetails = v);

  ThemeMode get themeMode => _themeMode;
  set themeMode(ThemeMode v) => _set(_themeMode != v, () => _themeMode = v);

  GridDensity get density => _density;
  set density(GridDensity v) => _set(_density != v, () => _density = v);

  bool get keepAwake => _keepAwake;
  set keepAwake(bool v) => _set(_keepAwake != v, () => _keepAwake = v);

  /// 1..65535; out-of-range values throw [RangeError].
  int get udpPort => _udpPort;
  set udpPort(int v) => _set(_udpPort != _checkPort(v), () => _udpPort = v);

  bool get tcpEnabled => _tcpEnabled;
  set tcpEnabled(bool v) => _set(_tcpEnabled != v, () => _tcpEnabled = v);

  /// 1..65535; out-of-range values throw [RangeError].
  int get tcpPort => _tcpPort;
  set tcpPort(int v) => _set(_tcpPort != _checkPort(v), () => _tcpPort = v);

  /// Empty = accept unauthenticated packets.
  String get sharedSecret => _sharedSecret;
  set sharedSecret(String v) =>
      _set(_sharedSecret != v, () => _sharedSecret = v);

  CardLayout get cardLayout => _cardLayout;
  set cardLayout(CardLayout v) => _set(_cardLayout != v, () => _cardLayout = v);

  /// How long a metric's peak marker holds, 0..[kMaxPeakHoldSeconds]; 0 = no marker.
  /// Out-of-range values are clamped; NaN is ignored.
  double get peakHoldSeconds => _peakHoldSeconds;
  set peakHoldSeconds(double v) {
    if (v.isNaN) return;
    final c = v.clamp(0.0, kMaxPeakHoldSeconds).toDouble();
    _set(_peakHoldSeconds != c, () => _peakHoldSeconds = c);
  }

  /// Moving-average window for displayed values of fast senders, 0..
  /// [kMaxSmoothingWindowMs]; 0 = off. Out-of-range values are clamped.
  int get smoothingWindowMs => _smoothingWindowMs;
  set smoothingWindowMs(int v) {
    final c = v.clamp(0, kMaxSmoothingWindowMs);
    _set(_smoothingWindowMs != c, () => _smoothingWindowMs = c);
  }

  /// Glide displayed values at frame rate between packets; false = snap.
  bool get smoothAnimation => _smoothAnimation;
  set smoothAnimation(bool v) =>
      _set(_smoothAnimation != v, () => _smoothAnimation = v);

  /// Manual host-card order (host ids); empty = automatic (live first, then name).
  /// Hosts missing from the list are shown after the listed ones, automatically
  /// ordered (see `orderHosts` in ui/host_order.dart).
  List<String> get hostOrder => List.unmodifiable(_hostOrder);

  /// Replaces the manual order (duplicates and empty ids dropped).
  void setHostOrder(List<String> ids) {
    final next = <String>[];
    final seen = <String>{};
    for (final id in ids) {
      if (next.length >= kMaxHostOrder) break;
      if (id.isNotEmpty && seen.add(id)) next.add(id);
    }
    if (listEquals(next, _hostOrder)) return;
    _hostOrder
      ..clear()
      ..addAll(next);
    markChanged();
  }

  /// Drops [id] from [hostOrder] (host removed from the dashboard).
  void removeHostFromOrder(String id) {
    if (_hostOrder.remove(id)) markChanged();
  }

  /// Back to automatic host order.
  void resetHostOrder() => setHostOrder(const []);

  /// Slots with visible == true, in display order.
  List<MetricSlot> get visibleSlots => slots.where((s) => s.visible).toList();

  MetricSlot? slotFor(String key) => _slotIndex[key];

  void _replaceSlots(Iterable<MetricSlot> next) {
    _slots
      ..clear()
      ..addAll(next);
    _slotIndex
      ..clear()
      ..addEntries(_slots.map((s) => MapEntry(s.key, s)));
  }

  /// Loads from shared_preferences; defaults on first run or if the stored JSON is corrupt.
  static Future<DisplayConfig> load() async {
    final config = DisplayConfig();
    try {
      final prefs = await SharedPreferences.getInstance();
      final text = prefs.getString(kDisplayConfigPrefsKey);
      if (text != null && config._applyJson(jsonDecode(text))) {
        await config._saveQuietly();
      }
    } catch (e) {
      debugPrint('DisplayConfig.load: using defaults ($e)');
      config._resetToDefaults();
    }
    return config;
  }

  Future<void> save() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(kDisplayConfigPrefsKey, jsonEncode(toJson()));
  }

  Map<String, Object?> toJson() => {
    'slots': [for (final s in slots) s.toJson()],
    'showDetails': _showDetails,
    'themeMode': _themeMode.name,
    'density': _density.name,
    'keepAwake': _keepAwake,
    'udpPort': _udpPort,
    'tcpEnabled': _tcpEnabled,
    'tcpPort': _tcpPort,
    'sharedSecret': _sharedSecret,
    'cardLayout': _cardLayout.name,
    'peakHoldSeconds': _peakHoldSeconds,
    'smoothingWindowMs': _smoothingWindowMs,
    'smoothAnimation': _smoothAnimation,
    'hostOrder': [..._hostOrder],
    kSlotsMigratedV2Key: true,
    kSparklinesDefaultOnKey: true,
  };

  /// Appends a visible gauge slot for [key] once; no-op if the key exists, even when
  /// the user hid it. See [ensureSlots].
  void ensureSlot(String key) => ensureSlots([key]);

  /// Called by NetController with every key of an accepted packet. Appends a visible
  /// gauge slot for each key not in [slots] yet (whatever the unit), then notifies and
  /// saves once for the whole batch; no-op when every key exists. Keys that are not
  /// lowercase snake_case, and new keys once [kMaxSlots] slots exist, are ignored.
  void ensureSlots(Iterable<String> keys) {
    var added = false;
    for (final key in keys) {
      if (_slotIndex.containsKey(key) || !isValidMetricKey(key)) continue;
      if (_slots.length >= kMaxSlots) break;
      final slot = MetricSlot(key: key);
      _slots.add(slot);
      _slotIndex[key] = slot;
      added = true;
    }
    if (added) markChanged();
  }

  /// Deletes the slot for [key]. It comes back (as a new visible gauge) only if a
  /// host sends that key again.
  void removeSlot(String key) {
    final s = _slotIndex.remove(key);
    if (s == null) return;
    _slots.remove(s);
    markChanged();
  }

  /// Moves the slot at [oldIndex] so it ends up at [newIndex] in the resulting list.
  /// These are `ReorderableListView.onReorderItem` semantics (newIndex already adjusted
  /// for the removed item), NOT the deprecated `onReorder` ones. Indices are clamped.
  void reorder(int oldIndex, int newIndex) {
    if (oldIndex < 0 || oldIndex >= _slots.length) return;
    final target = newIndex.clamp(0, _slots.length - 1);
    if (target == oldIndex) return;
    final slot = _slots.removeAt(oldIndex);
    _slots.insert(target, slot);
    markChanged();
  }

  void setSlotVisible(String key, bool visible) {
    final s = slotFor(key);
    if (s == null || s.visible == visible) return;
    s.visible = visible;
    markChanged();
  }

  void setSlotStyle(String key, TileStyle style) {
    final s = slotFor(key);
    if (s == null || s.style == style) return;
    s.style = style;
    markChanged();
  }

  /// Restores the default slot list (cpu/gpu/mem/disk gauges). Other settings unchanged.
  void resetSlots() {
    _replaceSlots(_defaultSlots());
    markChanged();
  }

  /// Notify listeners and persist. Call after mutating a [MetricSlot] directly.
  void markChanged() {
    notifyListeners();
    _saveQuietly();
  }

  void _set(bool changed, void Function() apply) {
    if (!changed) return;
    apply();
    markChanged();
  }

  /// Number of saves started (tests).
  @visibleForTesting
  int saveCount = 0;

  Future<void> _saveQuietly() async {
    saveCount++;
    try {
      await save();
    } catch (e) {
      debugPrint('DisplayConfig.save failed: $e');
    }
  }

  static int _checkPort(int v) =>
      RangeError.checkValueInInterval(v, 1, 65535, 'port');

  static List<MetricSlot> _defaultSlots() => [
    for (final k in kDefaultSlotKeys) MetricSlot(key: k),
  ];

  void _resetToDefaults() {
    _replaceSlots(_defaultSlots());
    _showDetails = true;
    _themeMode = ThemeMode.dark;
    _density = GridDensity.comfortable;
    _keepAwake = true;
    _udpPort = kDefaultPort;
    _tcpEnabled = true;
    _tcpPort = kDefaultPort;
    _sharedSecret = '';
    _cardLayout = CardLayout.dials;
    _peakHoldSeconds = kDefaultPeakHoldSeconds;
    _smoothingWindowMs = kDefaultSmoothingWindowMs;
    _smoothAnimation = true;
    _hostOrder.clear();
  }

  /// Tolerant: missing or wrongly typed fields keep their defaults. Returns true when a
  /// one-time migration changed something that should be saved back.
  bool _applyJson(Object? json) {
    if (json is! Map) return false;
    var migrated = false;
    final rawSlots = json['slots'];
    if (rawSlots is List) {
      final seen = <String>{};
      final parsed = <MetricSlot>[];
      for (final e in rawSlots) {
        final s = MetricSlot.fromJson(e);
        // Sanitise configs polluted before keys were validated (<= 1.0.1): drop
        // invalid keys, keep at most kMaxSlots, and save the cleaned list back.
        if (s == null || !isValidMetricKey(s.key)) {
          migrated = true;
          continue;
        }
        if (!seen.add(s.key)) continue;
        if (parsed.length >= kMaxSlots) {
          migrated = true;
          break;
        }
        parsed.add(s);
      }
      // v2: non-% metrics used to default to number tiles. Before the flag existed the
      // style could not be told apart from a user choice, so turn every number slot
      // into a gauge once; after that, a number style is the user's and is kept.
      if (json[kSlotsMigratedV2Key] != true) {
        for (final s in parsed) {
          if (s.style == TileStyle.number) s.style = TileStyle.gauge;
        }
        migrated = true;
      }
      _replaceSlots(parsed);
    }
    final showDetails = json['showDetails'];
    if (json[kSparklinesDefaultOnKey] == true) {
      if (showDetails is bool) _showDetails = showDetails;
    } else {
      // v3: sparklines default to on; flip older configs once, then respect the user.
      _showDetails = true;
      migrated = true;
    }
    _themeMode = _enumByName(ThemeMode.values, json['themeMode']) ?? _themeMode;
    _density = _enumByName(GridDensity.values, json['density']) ?? _density;
    final keepAwake = json['keepAwake'];
    if (keepAwake is bool) _keepAwake = keepAwake;
    final udpPort = json['udpPort'];
    if (udpPort is int && udpPort >= 1 && udpPort <= 65535) _udpPort = udpPort;
    final tcpEnabled = json['tcpEnabled'];
    if (tcpEnabled is bool) _tcpEnabled = tcpEnabled;
    final tcpPort = json['tcpPort'];
    if (tcpPort is int && tcpPort >= 1 && tcpPort <= 65535) _tcpPort = tcpPort;
    final secret = json['sharedSecret'];
    if (secret is String) _sharedSecret = secret;
    _cardLayout =
        _enumByName(CardLayout.values, json['cardLayout']) ?? _cardLayout;
    final peak = json['peakHoldSeconds'];
    if (peak is num && !peak.isNaN) {
      _peakHoldSeconds = peak.toDouble().clamp(0.0, kMaxPeakHoldSeconds);
    }
    final smooth = json['smoothingWindowMs'];
    if (smooth is num && smooth.isFinite) {
      _smoothingWindowMs = smooth.round().clamp(0, kMaxSmoothingWindowMs);
    }
    final order = json['hostOrder'];
    if (order is List) {
      _hostOrder
        ..clear()
        ..addAll(
          order
              .whereType<String>()
              .where((s) => s.isNotEmpty)
              .toSet()
              .take(kMaxHostOrder),
        );
    }
    final smoothAnim = json['smoothAnimation'];
    if (smoothAnim is bool) _smoothAnimation = smoothAnim;
    return migrated;
  }
}

T? _enumByName<T extends Enum>(List<T> values, Object? name) {
  for (final v in values) {
    if (v.name == name) return v;
  }
  return null;
}
