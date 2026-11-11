import 'dart:math' as math;

/// Width of each of [perRow] equal cells across [maxWidth] with [gap] between them.
double cellWidth(double maxWidth, int perRow, double gap) =>
    (maxWidth - gap * (perRow - 1)) / perRow;

/// Columns for a grid of graphs / gauges: 2 when [twoUp] (portrait phone), else 4.
/// Halved while a cell would be narrower than [minWidth] (so never an uneven 3 + 1),
/// and capped at [count] when given. Shared by host cards ("lines" layout) and the
/// host detail screen.
int graphColumns({
  required bool twoUp,
  required double maxWidth,
  required double gap,
  double minWidth = 0,
  int? count,
}) {
  var perRow = twoUp ? 2 : 4;
  while (perRow > 1 && cellWidth(maxWidth, perRow, gap) < minWidth) {
    perRow ~/= 2;
  }
  if (count != null) perRow = math.min(perRow, count);
  return math.max(1, perRow);
}
