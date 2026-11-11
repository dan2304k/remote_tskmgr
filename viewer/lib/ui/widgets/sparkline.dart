import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../theme.dart';

/// The fixed upper bound for a history graph: the metric's max, or 100 for `%` units
/// that do not state one, so bounded graphs always read against 0..100. Null means
/// auto-scale.
double? graphMax(double? max, String unit) =>
    max ?? (unit == '%' ? 100.0 : null);

/// Line over recent samples, oldest left, drawn inside a visible plot box (subtle fill,
/// 1 px border, faint reference lines at 50 % and 100 % of the scale). Nulls break the
/// line; the area under it is filled with a darker (dark theme) / lighter (light theme)
/// variant of the line colour. Bounded metrics use a fixed [min]..[max] scale; [max]
/// null auto-scales to the samples. [peak], when set, draws a faint dashed horizontal
/// line. No text is drawn inside the plot.
class Sparkline extends StatelessWidget {
  const Sparkline({
    super.key,
    required this.values,
    this.min = 0,
    this.max,
    this.capacity,
    this.color,
    this.height = 24,
    this.strokeWidth = 1.6,
    this.peak,
    this.unit,
    this.phase,
  });

  /// Samples, oldest first. Pass a [List] that is reused across rebuilds while the data
  /// is unchanged and the painter skips repainting.
  final Iterable<double?> values;
  final double min;
  final double? max;

  /// Number of x slots; defaults to the sample count. Pass the ring capacity so a young
  /// host's line grows from the right instead of stretching.
  final int? capacity;
  final Color? color;
  final double height;
  final double strokeWidth;
  final double? peak;

  /// Metric unit; `%` with a fixed [max] gets a clearer 100 % edge.
  final String? unit;

  /// 0..1 fraction of one sample slot to scroll left (time since the newest sample
  /// over the interval), so the line slides continuously between packets. Listened to
  /// by the painter directly (repaint only, no rebuild); null = no scrolling.
  final ValueListenable<double>? phase;

  @override
  Widget build(BuildContext context) {
    final rc = RtmColors.of(context);
    final scheme = Theme.of(context).colorScheme;
    final dark = Theme.of(context).brightness == Brightness.dark;
    final c = color ?? rc.accent;
    final u = unit;
    final vs = values;
    // Own layer: a gliding gauge or value text next to it does not repaint the plot,
    // and the scroll phase repaints only this.
    return RepaintBoundary(
      child: SizedBox(
        height: height,
        width: double.infinity,
        child: CustomPaint(
          painter: SparklinePainter(
            values: vs is List<double?> ? vs : vs.toList(growable: false),
            min: min,
            max: max,
            capacity: capacity,
            color: c,
            strokeWidth: strokeWidth,
            peak: peak,
            peakColor: rc.muted,
            plotFill: scheme.surfaceContainerHighest.withValues(
              alpha: dark ? 0.55 : 0.7,
            ),
            plotBorder: scheme.outline.withValues(alpha: dark ? 0.35 : 0.3),
            gridColor: scheme.outline.withValues(alpha: dark ? 0.22 : 0.2),
            boundedTop: u == '%' && max != null,
            fillColor: areaFillFor(c, dark: dark),
            phase: phase,
          ),
        ),
      ),
    );
  }
}

/// Fill under the line: the line colour pushed toward black (dark theme) or white
/// (light theme), half transparent.
Color areaFillFor(Color line, {required bool dark}) => Color.lerp(
  line,
  dark ? const Color(0xFF000000) : const Color(0xFFFFFFFF),
  0.55,
)!.withValues(alpha: 0.5);

class SparklinePainter extends CustomPainter {
  SparklinePainter({
    required this.values,
    required this.min,
    required this.max,
    required this.color,
    this.capacity,
    this.strokeWidth = 1.6,
    this.peak,
    this.peakColor = const Color(0xFF8B949E),
    this.plotFill,
    this.plotBorder,
    this.gridColor,
    this.boundedTop = false,
    this.fillColor,
    this.phase,
  }) : super(repaint: phase);

  final List<double?> values;
  final double min;
  final double? max;
  final int? capacity;
  final Color color;
  final double strokeWidth;
  final double? peak;
  final Color peakColor;

  /// Plot box fill / 1 px border / reference-line colour; null = not drawn.
  final Color? plotFill;
  final Color? plotBorder;
  final Color? gridColor;

  /// Draw the 100 % line more clearly (bounded % graphs).
  final bool boundedTop;

  /// Area fill under the line; null = [color] at low alpha.
  final Color? fillColor;

  /// Scroll offset in sample slots (0..1); see [Sparkline.phase]. Also the painter's
  /// repaint listenable.
  final ValueListenable<double>? phase;

  /// Current [phase], 0 when none.
  double get phaseValue => phase?.value ?? 0;

  @override
  void paint(Canvas canvas, Size size) {
    if (size.width <= 0 || size.height <= 0) return;
    final box = Offset.zero & size;
    final rrect = RRect.fromRectAndRadius(
      box.deflate(0.5),
      Radius.circular(math.min(6.0, size.height / 4)),
    );
    final boxFill = plotFill;
    if (boxFill != null) canvas.drawRRect(rrect, Paint()..color = boxFill);

    var lo = min;
    var hi = max;
    if (hi == null || hi <= lo) {
      var m = 0.0;
      for (final v in values) {
        if (v != null && v > m) m = v;
      }
      hi = math.max(lo + 1e-6, m * 1.15);
      if (m <= lo) hi = lo + 1;
    }
    final pad = strokeWidth;
    final h = math.max(1.0, size.height - pad * 2);
    double y(double v) => pad + h - ((v - lo) / (hi! - lo)).clamp(0.0, 1.0) * h;

    // Reference lines at 50 % and 100 % of the scale.
    final grid = gridColor;
    if (grid != null && size.height >= 12) {
      final g = Paint()..strokeWidth = 1;
      canvas.drawLine(
        Offset(1, y(lo + (hi - lo) / 2)),
        Offset(size.width - 1, y(lo + (hi - lo) / 2)),
        g..color = grid,
      );
      final topColor = boundedTop
          ? grid.withValues(alpha: math.min(1.0, grid.a * 2.2))
          : grid;
      canvas.drawLine(
        Offset(1, y(hi)),
        Offset(size.width - 1, y(hi)),
        g..color = topColor,
      );
    }

    canvas.save();
    canvas.clipRRect(rrect);
    if (values.isNotEmpty) _paintLine(canvas, size, y);
    final pk = peak;
    if (pk != null && pk.isFinite) _paintPeak(canvas, size, y(pk));
    canvas.restore();

    final border = plotBorder;
    if (border != null) {
      canvas.drawRRect(
        rrect,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1
          ..color = border,
      );
    }
  }

  void _paintLine(Canvas canvas, Size size, double Function(double) y) {
    final slots = math.max(capacity ?? values.length, values.length);
    final dx = slots <= 1 ? 0.0 : size.width / (slots - 1);
    final ph = phaseValue;
    final x0 =
        size.width -
        dx * (values.length - 1) -
        dx * (ph.isFinite ? ph.clamp(0.0, 1.0) : 0.0);
    final line = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..strokeJoin = StrokeJoin.round
      ..strokeCap = StrokeCap.round
      ..color = color;
    final fill = Paint()..color = fillColor ?? color.withValues(alpha: 0.25);

    // Split into runs of non-null samples.
    var i = 0;
    while (i < values.length) {
      while (i < values.length && values[i] == null) {
        i++;
      }
      final start = i;
      while (i < values.length && values[i] != null) {
        i++;
      }
      final end = i; // exclusive
      if (end <= start) continue;
      if (end - start == 1) {
        canvas.drawCircle(
          Offset(x0 + dx * start, y(values[start]!)),
          strokeWidth,
          Paint()..color = color,
        );
        continue;
      }
      final path = Path()..moveTo(x0 + dx * start, y(values[start]!));
      for (var k = start + 1; k < end; k++) {
        path.lineTo(x0 + dx * k, y(values[k]!));
      }
      final area = Path.from(path)
        ..lineTo(x0 + dx * (end - 1), size.height)
        ..lineTo(x0 + dx * start, size.height)
        ..close();
      canvas.drawPath(area, fill);
      canvas.drawPath(path, line);
    }
  }

  void _paintPeak(Canvas canvas, Size size, double py) {
    final dash = Paint()
      ..strokeWidth = 1
      ..color = peakColor.withValues(alpha: 0.6);
    const on = 4.0, off = 3.0;
    for (var x = 0.0; x < size.width; x += on + off) {
      canvas.drawLine(
        Offset(x, py),
        Offset(math.min(x + on, size.width), py),
        dash,
      );
    }
  }

  @override
  bool shouldRepaint(SparklinePainter old) =>
      !(identical(values, old.values) || listEquals(values, old.values)) ||
      min != old.min ||
      max != old.max ||
      capacity != old.capacity ||
      color != old.color ||
      strokeWidth != old.strokeWidth ||
      peak != old.peak ||
      peakColor != old.peakColor ||
      plotFill != old.plotFill ||
      plotBorder != old.plotBorder ||
      gridColor != old.gridColor ||
      boundedTop != old.boundedTop ||
      fillColor != old.fillColor ||
      phase != old.phase;
}
