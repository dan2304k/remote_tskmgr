import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../format.dart';
import '../theme.dart';

const double _start = math.pi * 0.75; // 135°, bottom-left
const double _sweep = math.pi * 1.5; // 270°

/// 270° arc gauge: track + value arc with rounded caps, value centred, label beneath.
/// [value] null renders a dash over a dimmed track. [max] must already be resolved
/// (see [resolveMax] for auto-scale metrics). [peak], when set, draws a radial tick just
/// outside the arc at the peak angle; it snaps (is not animated).
class GaugeArc extends StatelessWidget {
  const GaugeArc({
    super.key,
    required this.value,
    required this.unit,
    required this.label,
    this.min = 0,
    required this.max,
    this.color,
    this.size,
    this.peak,
  });

  final double? value;
  final String unit;
  final String label;
  final double min;
  final double max;
  final Color? color;

  /// Diameter; null = fit the available width.
  final double? size;

  /// Held peak in the same units as [value]; null = no marker.
  final double? peak;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final v = value;
    final span = max - min;
    final frac = v == null || span <= 0
        ? 0.0
        : ((v - min) / span).clamp(0.0, 1.0).toDouble();
    final pk = peak;
    final peakFrac = pk == null || span <= 0
        ? null
        : ((pk - min) / span).clamp(0.0, 1.0).toDouble();
    final arcColor = color ?? c.accent;
    final peakColor = Theme.of(context).colorScheme.onSurface;
    final fv = formatValue(v, unit);

    return LayoutBuilder(
      builder: (context, box) {
        final d =
            size ??
            (box.maxWidth.isFinite ? box.maxWidth : 96.0).clamp(40.0, 400.0);
        final stroke = math.max(4.0, d * 0.085);
        final numSize = d * 0.26;
        return SizedBox.square(
          dimension: d,
          child: CustomPaint(
            painter: GaugeArcPainter(
              fraction: frac,
              color: arcColor,
              track: v == null ? c.track.withValues(alpha: 0.5) : c.track,
              stroke: stroke,
              showValue: v != null,
              peakFraction: peakFrac,
              peakColor: peakColor,
            ),
            child: Stack(
              children: [
                Align(
                  alignment: const Alignment(0, -0.08),
                  child: Padding(
                    padding: EdgeInsets.symmetric(horizontal: stroke * 1.8),
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.baseline,
                        textBaseline: TextBaseline.alphabetic,
                        children: [
                          Text(
                            fv.number,
                            style: numberStyle(
                              context,
                              size: numSize,
                              color: v == null ? c.muted : null,
                            ),
                          ),
                          if (v != null && fv.unit.isNotEmpty)
                            Padding(
                              padding: const EdgeInsets.only(left: 1),
                              child: Text(
                                fv.unit,
                                style: numberStyle(
                                  context,
                                  size: numSize * 0.45,
                                  weight: FontWeight.w500,
                                  color: c.muted,
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ),
                Align(
                  alignment: Alignment.bottomCenter,
                  child: Padding(
                    padding: EdgeInsets.symmetric(horizontal: d * 0.22),
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      child: Text(
                        label,
                        maxLines: 1,
                        softWrap: false,
                        style: TextStyle(
                          fontSize: math.max(10.0, d * 0.13),
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.8,
                          height: 1.0,
                          color: c.muted,
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class GaugeArcPainter extends CustomPainter {
  GaugeArcPainter({
    required this.fraction,
    required this.color,
    required this.track,
    required this.stroke,
    this.showValue = true,
    this.peakFraction,
    this.peakColor = const Color(0xFFFFFFFF),
  });

  final double fraction;
  final Color color;
  final Color track;
  final double stroke;
  final bool showValue;

  /// 0..1 position of the peak tick; null = none.
  final double? peakFraction;
  final Color peakColor;

  /// How far the peak tick reaches beyond the arc's outer edge. Always reserved, so the
  /// gauge does not change size when peak hold is toggled.
  static double peakOverhang(double stroke) => math.max(2.0, stroke * 0.45);

  @override
  void paint(Canvas canvas, Size size) {
    final out = peakOverhang(stroke);
    final r = (math.min(size.width, size.height) - stroke) / 2 - out;
    if (r <= 0) return;
    final center = size.center(Offset.zero);
    final rect = Rect.fromCircle(center: center, radius: r);
    final p = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = stroke
      ..strokeCap = StrokeCap.round;
    canvas.drawArc(rect, _start, _sweep, false, p..color = track);
    final f = fraction.clamp(0.0, 1.0);
    if (showValue && f > 0.001) {
      canvas.drawArc(rect, _start, _sweep * f, false, p..color = color);
    }
    final pf = peakFraction;
    if (pf != null && pf.isFinite) {
      // Radial tick from the middle of the stroke to just outside the arc.
      final a = _start + _sweep * pf.clamp(0.0, 1.0);
      final dir = Offset(math.cos(a), math.sin(a));
      final tick = Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = (stroke * 0.35).clamp(2.0, 3.0)
        ..strokeCap = StrokeCap.butt
        ..color = peakColor;
      canvas.drawLine(
        center + dir * r,
        center + dir * (r + stroke / 2 + out),
        tick,
      );
    }
  }

  @override
  bool shouldRepaint(GaugeArcPainter old) =>
      old.fraction != fraction ||
      old.color != color ||
      old.track != track ||
      old.stroke != stroke ||
      old.showValue != showValue ||
      old.peakFraction != peakFraction ||
      old.peakColor != peakColor;
}
