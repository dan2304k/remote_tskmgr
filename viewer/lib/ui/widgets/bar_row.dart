import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../format.dart';
import '../theme.dart';

/// Horizontal bar: rounded track, value fill from the left, optional peak tick.
/// The bar is [barHeight] tall and vertically centred; the peak tick spans the full
/// paint height so it stands proud of the bar.
class HBarPainter extends CustomPainter {
  HBarPainter({
    required this.fraction,
    required this.color,
    required this.track,
    required this.barHeight,
    this.showValue = true,
    this.peakFraction,
    this.peakColor = const Color(0xFFFFFFFF),
    this.cornerRadius,
    this.tickWidth = 2.5,
  });

  final double fraction;
  final Color color;
  final Color track;
  final double barHeight;
  final bool showValue;

  /// 0..1 position of the peak tick; null = none.
  final double? peakFraction;
  final Color peakColor;

  /// Bar corner radius; null = fully rounded (half the bar height).
  final double? cornerRadius;
  final double tickWidth;

  @override
  void paint(Canvas canvas, Size size) {
    if (size.width <= 0 || size.height <= 0) return;
    final h = barHeight.clamp(1.0, size.height).toDouble();
    final top = (size.height - h) / 2;
    final radius = Radius.circular(cornerRadius ?? h / 2);
    final trackRect = Rect.fromLTWH(0, top, size.width, h);
    canvas.drawRRect(
      RRect.fromRectAndRadius(trackRect, radius),
      Paint()..color = track,
    );
    final f = fraction.clamp(0.0, 1.0);
    if (showValue && f > 0.001) {
      canvas.save();
      canvas.clipRRect(RRect.fromRectAndRadius(trackRect, radius));
      canvas.drawRRect(
        RRect.fromRectAndRadius(
          Rect.fromLTWH(0, top, size.width * f, h),
          radius,
        ),
        Paint()..color = color,
      );
      canvas.restore();
    }
    final pf = peakFraction;
    if (pf != null && pf.isFinite) {
      final w = math.min(tickWidth, size.width);
      final x = (size.width * pf.clamp(0.0, 1.0))
          .clamp(w / 2, size.width - w / 2)
          .toDouble();
      canvas.drawLine(
        Offset(x, 0),
        Offset(x, size.height),
        Paint()
          ..strokeWidth = w
          ..color = peakColor,
      );
    }
  }

  @override
  bool shouldRepaint(HBarPainter old) =>
      old.fraction != fraction ||
      old.color != color ||
      old.track != track ||
      old.barHeight != barHeight ||
      old.showValue != showValue ||
      old.peakFraction != peakFraction ||
      old.peakColor != peakColor ||
      old.cornerRadius != cornerRadius ||
      old.tickWidth != tickWidth;
}

/// Animated [HBarPainter]: the fill tweens to [value], the [peak] tick snaps.
class HBar extends StatelessWidget {
  const HBar({
    super.key,
    required this.value,
    this.min = 0,
    required this.max,
    this.peak,
    this.color,
    this.barHeight = 8,
  });

  final double? value;
  final double min;
  final double max;
  final double? peak;
  final Color? color;
  final double barHeight;

  static double? _frac(double? v, double min, double max) {
    final span = max - min;
    if (v == null || span <= 0) return null;
    return ((v - min) / span).clamp(0.0, 1.0).toDouble();
  }

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final v = value;
    final peakColor = Theme.of(context).colorScheme.onSurface;
    // Room above/below the bar for the peak tick.
    final height = barHeight + 6;
    return SizedBox(
      height: height,
      width: double.infinity,
      child: CustomPaint(
        painter: HBarPainter(
          fraction: _frac(v, min, max) ?? 0,
          color: color ?? c.accent,
          track: v == null ? c.track.withValues(alpha: 0.5) : c.track,
          barHeight: barHeight,
          showValue: v != null,
          peakFraction: _frac(peak, min, max),
          peakColor: peakColor,
        ),
      ),
    );
  }
}

/// One metric as a full-width row: label left, bar in the middle, value right. Used by
/// the "bars" card layout.
class BarRow extends StatelessWidget {
  const BarRow({
    super.key,
    required this.value,
    required this.unit,
    required this.label,
    this.min = 0,
    required this.max,
    this.peak,
    this.color,
    this.compact = false,
  });

  final double? value;
  final String unit;
  final String label;
  final double min;
  final double max;
  final double? peak;
  final Color? color;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final v = value;
    final fv = formatValue(v, unit);
    return LayoutBuilder(
      builder: (context, box) {
        final w = box.maxWidth.isFinite ? box.maxWidth : 300.0;
        final labelW = (w * 0.2).clamp(36.0, 110.0);
        final valueW = (w * 0.24).clamp(52.0, 130.0);
        final gap = w < 300 ? 6.0 : 10.0;
        final numSize = compact ? 15.0 : 18.0;
        return SizedBox(
          height: compact ? 22 : 28,
          child: Row(
            children: [
              SizedBox(
                width: labelW,
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.fade,
                  softWrap: false,
                  style: TextStyle(
                    fontSize: compact ? 11 : 12,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.8,
                    color: c.muted,
                  ),
                ),
              ),
              SizedBox(width: gap),
              Expanded(
                child: HBar(
                  value: v,
                  min: min,
                  max: max,
                  peak: peak,
                  color: color,
                  barHeight: compact ? 8 : 10,
                ),
              ),
              SizedBox(width: gap),
              SizedBox(
                width: valueW,
                child: FittedBox(
                  fit: BoxFit.scaleDown,
                  alignment: Alignment.centerRight,
                  child: Text.rich(
                    TextSpan(
                      text: fv.number,
                      style: numberStyle(
                        context,
                        size: numSize,
                        color: v == null ? c.muted : null,
                      ),
                      children: [
                        if (v != null && fv.unit.isNotEmpty)
                          TextSpan(
                            text: ' ${fv.unit}',
                            style: numberStyle(
                              context,
                              size: numSize * 0.6,
                              weight: FontWeight.w500,
                              color: c.muted,
                            ),
                          ),
                      ],
                    ),
                    maxLines: 1,
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// A dial-sized square tile: one title row (label left, value + unit right) above a
/// single fat bar filling the rest of the tile (small corner radius, 2 px peak tick,
/// no text inside). Used by the "bar tiles" layout and [TileStyle.barTile], so it
/// lines up exactly like [GaugeArc] tiles.
class BarGaugeTile extends StatelessWidget {
  const BarGaugeTile({
    super.key,
    required this.value,
    required this.unit,
    required this.label,
    this.min = 0,
    required this.max,
    this.peak,
    this.color,
    this.size,
  });

  final double? value;
  final String unit;
  final String label;
  final double min;
  final double max;
  final double? peak;
  final Color? color;

  /// Side length; null = fit the available width.
  final double? size;

  static double? _frac(double? v, double min, double max) {
    final span = max - min;
    if (v == null || span <= 0) return null;
    return ((v - min) / span).clamp(0.0, 1.0).toDouble();
  }

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final v = value;
    final fv = formatValue(v, unit);
    final peakColor = Theme.of(context).colorScheme.onSurface;
    return LayoutBuilder(
      builder: (context, box) {
        final d =
            size ??
            (box.maxWidth.isFinite ? box.maxWidth : 96.0).clamp(40.0, 400.0);
        // Same sizes as the dial's label and value, on one line.
        final labelSize = math.max(10.0, d * 0.13);
        final numSize = d * 0.26;
        const inset = 4.0;
        return SizedBox.square(
          dimension: d,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                key: const ValueKey('bar-tile-title'),
                height: numSize,
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.center,
                  children: [
                    // The label keeps its size (up to half the row); the value
                    // shrinks first when the row is tight.
                    ConstrainedBox(
                      constraints: BoxConstraints(maxWidth: d * 0.5),
                      child: Text(
                        label,
                        maxLines: 1,
                        overflow: TextOverflow.fade,
                        softWrap: false,
                        style: TextStyle(
                          fontSize: labelSize,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.8,
                          height: 1.0,
                          color: c.muted,
                        ),
                      ),
                    ),
                    const SizedBox(width: 4),
                    Expanded(
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        alignment: Alignment.centerRight,
                        child: Text.rich(
                          TextSpan(
                            text: fv.number,
                            style: numberStyle(
                              context,
                              size: numSize,
                              color: v == null ? c.muted : null,
                            ),
                            children: [
                              if (v != null && fv.unit.isNotEmpty)
                                TextSpan(
                                  text: fv.unit,
                                  style: numberStyle(
                                    context,
                                    size: numSize * 0.45,
                                    weight: FontWeight.w500,
                                    color: c.muted,
                                  ),
                                ),
                            ],
                          ),
                          maxLines: 1,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.all(inset),
                  child: CustomPaint(
                    key: const ValueKey('bar-tile-bar'),
                    size: Size.infinite,
                    painter: HBarPainter(
                      fraction: _frac(v, min, max) ?? 0,
                      color: color ?? c.accent,
                      track: v == null
                          ? c.track.withValues(alpha: 0.5)
                          : c.track,
                      barHeight: double.infinity,
                      showValue: v != null,
                      peakFraction: _frac(peak, min, max),
                      peakColor: peakColor,
                      cornerRadius: 3,
                      tickWidth: 2,
                    ),
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
