import 'package:flutter/material.dart';

import '../../config/display_config.dart';
import '../../core/host_registry.dart';
import '../../core/packet.dart';
import '../format.dart';
import '../theme.dart';
import 'bar_row.dart';
import 'bar_tile.dart';
import 'gauge_arc.dart';
import 'line_graph.dart';
import 'number_tile.dart';
import 'sparkline.dart';
import 'value_follower.dart';

/// Renders one [MetricSlot] of one host in the slot's [TileStyle]. A slot the host does
/// not report becomes a dimmed placeholder of the same footprint so cards line up.
class MetricTile extends StatelessWidget {
  const MetricTile({
    super.key,
    required this.host,
    required this.slot,
    this.showSparkline = false,
    this.sparklineHeight = 20,
    this.gaugeSize,
    this.style,
    this.peakHoldSeconds = 0,
    this.smoothingWindowMs = 0,
    this.smoothAnimation = false,
  });

  final HostState host;
  final MetricSlot slot;
  final bool showSparkline;
  final double sparklineHeight;

  /// Gauge diameter; null fits the tile width.
  final double? gaugeSize;

  /// Overrides [MetricSlot.style] (the detail screen always uses gauges).
  final TileStyle? style;

  /// Peak hold window; 0 = no peak marker.
  final double peakHoldSeconds;

  /// Moving-average window for the displayed value (see [displayValue]); 0 = raw.
  final int smoothingWindowMs;

  /// Glide the displayed value at frame rate ([ValueFollower]); false = snap.
  final bool smoothAnimation;

  @override
  Widget build(BuildContext context) {
    final m = host.latest.metrics[slot.key];
    final st = style ?? slot.style;
    if (m == null) return _placeholder(context, st);
    final ring = host.history[slot.key];
    final max = resolveMax(m, ring);
    final target = displayValue(host, slot.key, m.v, smoothingWindowMs);
    final peak = peakFor(host, slot.key, peakHoldSeconds);

    final Widget body = ValueFollower(
      target: target,
      tau: followerTau(host.interval),
      range: max - m.min,
      enabled: smoothAnimation,
      builder: (context, v) => _body(context, st, m, v, max, peak, ring),
    );
    return _withSparkline(context, body, st, m, ring, peak);
  }

  /// The tile for displayed value [v]; the colour ramp follows [v].
  Widget _body(
    BuildContext context,
    TileStyle st,
    Metric m,
    double? v,
    double max,
    double? peak,
    RingBuffer? ring,
  ) {
    final color = RtmColors.of(context).forValue(v, min: m.min, max: m.max);
    return switch (st) {
      TileStyle.gauge => GaugeArc(
        value: v,
        unit: m.unit,
        label: m.label,
        min: m.min,
        max: max,
        color: color,
        size: gaugeSize,
        peak: peak,
      ),
      TileStyle.bar => BarTile(
        value: v,
        unit: m.unit,
        label: m.label,
        min: m.min,
        max: max,
        color: color,
        peak: peak,
      ),
      TileStyle.barTile => BarGaugeTile(
        value: v,
        unit: m.unit,
        label: m.label,
        min: m.min,
        max: max,
        color: color,
        size: gaugeSize,
        peak: peak,
      ),
      TileStyle.number => NumberTile(
        value: v,
        unit: m.unit,
        label: m.label,
        color: m.max == null ? null : color,
      ),
      TileStyle.sparkline => _SparkTile(
        metric: m,
        value: v,
        ring: ring,
        color: color,
        peak: peak,
      ),
    };
  }

  Widget _withSparkline(
    BuildContext context,
    Widget body,
    TileStyle st,
    Metric m,
    RingBuffer? ring,
    double? peak,
  ) {
    final c = RtmColors.of(context);
    if (!showSparkline || st == TileStyle.sparkline || ring == null) {
      return body;
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        body,
        const SizedBox(height: 4),
        Sparkline(
          values: ring.values,
          capacity: ring.capacity,
          min: m.min,
          max: graphMax(m.max, m.unit),
          color: c.accent,
          height: sparklineHeight,
          peak: peak,
          unit: m.unit,
        ),
      ],
    );
  }

  Widget _placeholder(BuildContext context, TileStyle st) {
    final label = slot.key.toUpperCase();
    final Widget body = switch (st) {
      TileStyle.gauge => GaugeArc(
        value: null,
        unit: '',
        label: label,
        max: 1,
        size: gaugeSize,
      ),
      TileStyle.bar => BarTile(value: null, unit: '', label: label, max: 1),
      TileStyle.number => NumberTile(value: null, unit: '', label: label),
      TileStyle.barTile => BarGaugeTile(
        value: null,
        unit: '',
        label: label,
        max: 1,
        size: gaugeSize,
      ),
      TileStyle.sparkline => _SparkTile(metric: null, ring: null, label: label),
    };
    return Opacity(
      key: ValueKey('missing-${slot.key}'),
      opacity: 0.35,
      child: body,
    );
  }
}

class _SparkTile extends StatelessWidget {
  const _SparkTile({
    required this.metric,
    required this.ring,
    this.value,
    this.color,
    this.label,
    this.peak,
  });

  final Metric? metric;

  /// Displayed (possibly smoothed) value.
  final double? value;
  final RingBuffer? ring;
  final Color? color;
  final String? label;
  final double? peak;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final m = metric;
    final fv = formatValue(value, m?.unit ?? '');
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          crossAxisAlignment: CrossAxisAlignment.baseline,
          textBaseline: TextBaseline.alphabetic,
          children: [
            Expanded(
              child: Text(
                m?.label ?? label ?? '',
                maxLines: 1,
                overflow: TextOverflow.fade,
                softWrap: false,
                style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.8,
                  color: c.muted,
                ),
              ),
            ),
            Flexible(
              child: FittedBox(
                fit: BoxFit.scaleDown,
                alignment: Alignment.centerRight,
                child: Text(
                  value == null ? fv.number : '${fv.number} ${fv.unit}'.trim(),
                  style: numberStyle(context, size: 15),
                ),
              ),
            ),
          ],
        ),
        const SizedBox(height: 4),
        Sparkline(
          values: ring?.values ?? const <double?>[],
          capacity: ring?.capacity,
          min: m?.min ?? 0,
          max: m == null ? null : graphMax(m.max, m.unit),
          color: color,
          height: 40,
          peak: peak,
          unit: m?.unit,
        ),
      ],
    );
  }
}

/// One slot as a full-width [BarRow] ("bars" card layout). Bounded metrics use the
/// value colour ramp; unbounded ones use the accent and auto-scale like gauges
/// ([resolveMax]: largest recent sample × 1.2).
class MetricBarRow extends StatelessWidget {
  const MetricBarRow({
    super.key,
    required this.host,
    required this.slot,
    this.peakHoldSeconds = 0,
    this.smoothingWindowMs = 0,
    this.smoothAnimation = false,
    this.compact = false,
  });

  final HostState host;
  final MetricSlot slot;
  final double peakHoldSeconds;

  /// Moving-average window for the displayed value (see [displayValue]); 0 = raw.
  final int smoothingWindowMs;

  /// Glide the displayed value at frame rate ([ValueFollower]); false = snap.
  final bool smoothAnimation;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final m = host.latest.metrics[slot.key];
    if (m == null) {
      return Opacity(
        key: ValueKey('missing-${slot.key}'),
        opacity: 0.35,
        child: BarRow(
          value: null,
          unit: '',
          label: slot.key.toUpperCase(),
          max: 1,
          compact: compact,
        ),
      );
    }
    final c = RtmColors.of(context);
    final max = resolveMax(m, host.history[slot.key]);
    final peak = peakFor(host, slot.key, peakHoldSeconds);
    return ValueFollower(
      target: displayValue(host, slot.key, m.v, smoothingWindowMs),
      tau: followerTau(host.interval),
      range: max - m.min,
      enabled: smoothAnimation,
      builder: (context, v) => BarRow(
        value: v,
        unit: m.unit,
        label: m.label,
        min: m.min,
        max: max,
        peak: peak,
        color: c.forValue(v, min: m.min, max: m.max),
        compact: compact,
      ),
    );
  }
}

/// One slot as a [LineGraph] panel ("lines" card layout).
class MetricLineGraph extends StatelessWidget {
  const MetricLineGraph({
    super.key,
    required this.host,
    required this.slot,
    this.peakHoldSeconds = 0,
    this.smoothingWindowMs = 0,
    this.smoothAnimation = false,
    this.height = 80,
  });

  final HostState host;
  final MetricSlot slot;
  final double peakHoldSeconds;

  /// Moving-average window for the displayed value (see [displayValue]); 0 = raw.
  final int smoothingWindowMs;

  /// Glide the displayed value at frame rate ([ValueFollower]); false = snap.
  final bool smoothAnimation;
  final double height;

  @override
  Widget build(BuildContext context) {
    final m = host.latest.metrics[slot.key];
    if (m == null) {
      return Opacity(
        key: ValueKey('missing-${slot.key}'),
        opacity: 0.35,
        child: LineGraph(
          ring: null,
          value: null,
          unit: '',
          label: slot.key.toUpperCase(),
          height: height,
        ),
      );
    }
    final c = RtmColors.of(context);
    final ring = host.history[slot.key];
    final peak = peakFor(host, slot.key, peakHoldSeconds);
    // Value text glides; the plot scrolls by the time since the last packet.
    return ScrollPhase(
      sampleKey: host.latest.seq,
      interval: host.interval,
      enabled: smoothAnimation,
      builder: (context, phase) => ValueFollower(
        target: displayValue(host, slot.key, m.v, smoothingWindowMs),
        tau: followerTau(host.interval),
        range: resolveMax(m, ring) - m.min,
        enabled: smoothAnimation,
        builder: (context, v) => LineGraph(
          ring: ring,
          value: v,
          unit: m.unit,
          label: m.label,
          min: m.min,
          max: m.max,
          peak: peak,
          color: c.forValue(v, min: m.min, max: m.max),
          height: height,
          phase: phase,
        ),
      ),
    );
  }
}
