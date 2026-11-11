import 'package:flutter/material.dart';

/// Ambient-display palette. Dark first (OLED friendly), one accent, status colours.
class RtmColors extends ThemeExtension<RtmColors> {
  const RtmColors({
    required this.accent,
    required this.live,
    required this.stale,
    required this.offline,
    required this.ok,
    required this.warn,
    required this.hot,
    required this.track,
    required this.muted,
  });

  final Color accent;
  final Color live;
  final Color stale;
  final Color offline;
  final Color ok;
  final Color warn;
  final Color hot;

  /// Unfilled part of gauges and bars.
  final Color track;

  /// Secondary text (labels, units, ages).
  final Color muted;

  static const dark = RtmColors(
    accent: Color(0xFF2DD4BF),
    live: Color(0xFF2DD4BF),
    stale: Color(0xFFF5B841),
    offline: Color(0xFF6B7280),
    ok: Color(0xFF2DD4BF),
    warn: Color(0xFFF5B841),
    hot: Color(0xFFF2545B),
    track: Color(0xFF242A31),
    muted: Color(0xFF8B949E),
  );

  static const light = RtmColors(
    accent: Color(0xFF0F8A80),
    live: Color(0xFF0F8A80),
    stale: Color(0xFFC98A00),
    offline: Color(0xFF8A9099),
    ok: Color(0xFF0F8A80),
    warn: Color(0xFFC98A00),
    hot: Color(0xFFD12F38),
    track: Color(0xFFE1E5EA),
    muted: Color(0xFF5F6670),
  );

  static RtmColors of(BuildContext context) =>
      Theme.of(context).extension<RtmColors>() ?? dark;

  /// Colour for a value. Bounded metrics (known max) ramp ok < 60 % <= warn <= 85 % < hot
  /// of their range; unbounded metrics and null values use the accent.
  Color forValue(double? v, {double min = 0, double? max}) {
    if (v == null || max == null || max <= min) return accent;
    final f = (v - min) / (max - min);
    if (f > 0.85) return hot;
    if (f >= 0.60) return warn;
    return ok;
  }

  @override
  RtmColors copyWith({
    Color? accent,
    Color? live,
    Color? stale,
    Color? offline,
    Color? ok,
    Color? warn,
    Color? hot,
    Color? track,
    Color? muted,
  }) => RtmColors(
    accent: accent ?? this.accent,
    live: live ?? this.live,
    stale: stale ?? this.stale,
    offline: offline ?? this.offline,
    ok: ok ?? this.ok,
    warn: warn ?? this.warn,
    hot: hot ?? this.hot,
    track: track ?? this.track,
    muted: muted ?? this.muted,
  );

  @override
  RtmColors lerp(RtmColors? other, double t) {
    if (other == null) return this;
    return RtmColors(
      accent: Color.lerp(accent, other.accent, t)!,
      live: Color.lerp(live, other.live, t)!,
      stale: Color.lerp(stale, other.stale, t)!,
      offline: Color.lerp(offline, other.offline, t)!,
      ok: Color.lerp(ok, other.ok, t)!,
      warn: Color.lerp(warn, other.warn, t)!,
      hot: Color.lerp(hot, other.hot, t)!,
      track: Color.lerp(track, other.track, t)!,
      muted: Color.lerp(muted, other.muted, t)!,
    );
  }
}

const _tabular = [FontFeature.tabularFigures()];

/// Numeric text styles with tabular figures so values do not jitter between samples.
TextStyle numberStyle(
  BuildContext context, {
  double size = 20,
  FontWeight weight = FontWeight.w600,
  Color? color,
}) => TextStyle(
  fontSize: size,
  fontWeight: weight,
  height: 1.0,
  color: color ?? Theme.of(context).colorScheme.onSurface,
  fontFeatures: _tabular,
);

TextTheme _withTabular(TextTheme t) {
  TextStyle? f(TextStyle? s) => s?.copyWith(fontFeatures: _tabular);
  return t.copyWith(
    displayLarge: f(t.displayLarge),
    displayMedium: f(t.displayMedium),
    displaySmall: f(t.displaySmall),
    headlineLarge: f(t.headlineLarge),
    headlineMedium: f(t.headlineMedium),
    headlineSmall: f(t.headlineSmall),
    titleLarge: f(t.titleLarge),
    titleMedium: f(t.titleMedium),
    labelLarge: f(t.labelLarge),
    labelMedium: f(t.labelMedium),
    labelSmall: f(t.labelSmall),
    bodySmall: f(t.bodySmall),
  );
}

ThemeData _build(Brightness b) {
  final dark = b == Brightness.dark;
  final ext = dark ? RtmColors.dark : RtmColors.light;
  var scheme = ColorScheme.fromSeed(
    seedColor: ext.accent,
    brightness: b,
  ).copyWith(primary: ext.accent);
  if (dark) {
    scheme = scheme.copyWith(
      surface: const Color(0xFF0B0D10),
      onSurface: const Color(0xFFE6EDF3),
      surfaceContainerLowest: const Color(0xFF07080A),
      surfaceContainerLow: const Color(0xFF111418),
      surfaceContainer: const Color(0xFF15191E),
      surfaceContainerHigh: const Color(0xFF1B2026),
      surfaceContainerHighest: const Color(0xFF222830),
      onSurfaceVariant: ext.muted,
      outlineVariant: const Color(0xFF2A3038),
      onPrimary: const Color(0xFF00201C),
    );
  }
  final base = ThemeData(useMaterial3: true, colorScheme: scheme);
  return base.copyWith(
    scaffoldBackgroundColor: scheme.surface,
    textTheme: _withTabular(base.textTheme),
    extensions: [ext],
    appBarTheme: AppBarTheme(
      backgroundColor: scheme.surface,
      surfaceTintColor: Colors.transparent,
      elevation: 0,
      scrolledUnderElevation: 0,
      titleTextStyle: base.textTheme.titleLarge?.copyWith(
        fontWeight: FontWeight.w700,
        letterSpacing: 1.2,
        color: scheme.onSurface,
      ),
    ),
    cardTheme: CardThemeData(
      color: dark ? const Color(0xFF14181D) : scheme.surfaceContainerLow,
      surfaceTintColor: Colors.transparent,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(16),
        side: BorderSide(color: scheme.outlineVariant.withValues(alpha: 0.5)),
      ),
    ),
    dividerTheme: DividerThemeData(
      color: scheme.outlineVariant.withValues(alpha: 0.6),
      space: 1,
    ),
  );
}

final ThemeData rtmDarkTheme = _build(Brightness.dark);
final ThemeData rtmLightTheme = _build(Brightness.light);
