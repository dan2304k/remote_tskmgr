import 'package:flutter/material.dart';

import '../format.dart';
import '../theme.dart';

/// Big number + unit, label beneath. Tabular figures.
class NumberTile extends StatelessWidget {
  const NumberTile({
    super.key,
    required this.value,
    required this.unit,
    required this.label,
    this.color,
    this.numberSize = 26,
  });

  final double? value;
  final String unit;
  final String label;
  final Color? color;
  final double numberSize;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final fv = formatValue(value, unit);
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        FittedBox(
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
                  size: numberSize,
                  color: value == null ? c.muted : color,
                ),
              ),
              if (value != null && fv.unit.isNotEmpty)
                Text(
                  ' ${fv.unit}',
                  style: numberStyle(
                    context,
                    size: numberSize * 0.45,
                    weight: FontWeight.w500,
                    color: c.muted,
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Text(
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
      ],
    );
  }
}
