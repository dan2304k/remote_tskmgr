import 'package:flutter/material.dart';

import '../format.dart';
import '../theme.dart';
import 'bar_row.dart';

/// Label + value on one line, horizontal bar beneath.
class BarTile extends StatelessWidget {
  const BarTile({
    super.key,
    required this.value,
    required this.unit,
    required this.label,
    this.min = 0,
    required this.max,
    this.color,
    this.peak,
  });

  final double? value;
  final String unit;
  final String label;
  final double min;
  final double max;
  final Color? color;

  /// Held peak; drawn as a thin tick on the bar. Null = none.
  final double? peak;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final v = value;
    final fv = formatValue(v, unit);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.fade,
                softWrap: false,
                style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.8,
                  color: c.muted,
                ),
              ),
            ),
            const SizedBox(width: 6),
            Flexible(
              flex: 2,
              child: FittedBox(
                fit: BoxFit.scaleDown,
                alignment: Alignment.centerRight,
                child: Text.rich(
                  TextSpan(
                    text: fv.number,
                    style: numberStyle(context, size: 20),
                    children: [
                      if (v != null && fv.unit.isNotEmpty)
                        TextSpan(
                          text: ' ${fv.unit}',
                          style: numberStyle(context, size: 11, color: c.muted),
                        ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
        const SizedBox(height: 3),
        HBar(value: v, min: min, max: max, peak: peak, color: color),
      ],
    );
  }
}
