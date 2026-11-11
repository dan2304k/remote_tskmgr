import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../../core/host_registry.dart';
import '../format.dart';
import '../theme.dart';
import 'sparkline.dart';

/// Label (left) and current value (right) above a boxed [Sparkline] plot. Used by the
/// "lines" card layout. `%` metrics plot against a fixed 0..100 scale (see [graphMax]);
/// others auto-scale. [peak] draws a faint dashed line (no text inside the plot).
class LineGraph extends StatelessWidget {
  const LineGraph({
    super.key,
    required this.ring,
    required this.value,
    required this.unit,
    required this.label,
    this.min = 0,
    this.max,
    this.peak,
    this.color,
    this.height = 80,
    this.phase,
    this.values,
  });

  final RingBuffer? ring;

  /// Samples to plot; defaults to [ring]'s. Pass a list computed once per packet so
  /// rebuilds during a value glide do not copy the ring or repaint the plot.
  final List<double?>? values;
  final double? value;
  final String unit;
  final String label;
  final double min;

  /// Fixed upper bound; null auto-scales (or 0..100 for `%`).
  final double? max;
  final double? peak;
  final Color? color;

  /// Total height, header included.
  final double height;

  /// Scroll phase passed to [Sparkline.phase] (repaints the plot only).
  final ValueListenable<double>? phase;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final fv = formatValue(value, unit);
    final small = height < 72;
    final labelSize = small ? 10.0 : 11.0;
    final numSize = small ? 14.0 : 17.0;
    return SizedBox(
      height: height,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: numSize + 2,
            child: Row(
              children: [
                Expanded(
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
                Flexible(
                  flex: 2,
                  child: FittedBox(
                    fit: BoxFit.scaleDown,
                    alignment: Alignment.centerRight,
                    child: Text.rich(
                      TextSpan(
                        text: fv.number,
                        style: numberStyle(
                          context,
                          size: numSize,
                          color: value == null ? c.muted : null,
                        ),
                        children: [
                          if (value != null && fv.unit.isNotEmpty)
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
          ),
          const SizedBox(height: 4),
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) => Sparkline(
                values: values ?? ring?.values ?? const <double?>[],
                capacity: ring?.capacity,
                min: min,
                max: graphMax(max, unit),
                color: color,
                height: box.maxHeight,
                peak: peak,
                unit: unit,
                phase: phase,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
