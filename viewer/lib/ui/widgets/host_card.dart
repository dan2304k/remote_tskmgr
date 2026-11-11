import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../../config/display_config.dart';
import '../../core/host_registry.dart';
import '../format.dart';
import '../grid_math.dart';
import '../theme.dart';
import 'metric_tile.dart';
import 'value_follower.dart';

/// Visible slots in display order, limited to metrics [host] has reported recently
/// ([HostState.isMetricRecent] at [now]), so a metric the sender stopped sending drops
/// off this card while other hosts may still show it. If the config has no slots yet
/// (fresh install before the core appends any) fall back to the host's own metric order.
List<MetricSlot> visibleSlots(
  DisplayConfig config,
  HostState host, {
  DateTime? now,
}) {
  if (config.slots.isNotEmpty) {
    final t = now ?? DateTime.now();
    return [
      for (final s in config.visibleSlots)
        if (host.isMetricRecent(s.key, t)) s,
    ];
  }
  return [for (final k in host.latest.metrics.keys) MetricSlot(key: k)];
}

/// One host on the dashboard. Rebuilds on its own host's [HostState.changes], on
/// registry-level changes (host set / status) and on config changes; a packet for
/// another host does not rebuild it.
class HostCard extends StatelessWidget {
  const HostCard({
    super.key,
    required this.hostId,
    this.compact = false,
    this.onTap,
  });

  final String hostId;

  /// Landscape phone / compact density: all tiles on one smaller row.
  final bool compact;
  final VoidCallback? onTap;

  /// Test hook: called with [hostId] each time a card body is built.
  @visibleForTesting
  static void Function(String hostId)? debugOnBuild;

  @override
  Widget build(BuildContext context) {
    final registry = context.read<HostRegistry>();
    final config = context.read<DisplayConfig>();
    return ListenableBuilder(
      listenable: Listenable.merge([registry, config]),
      builder: (context, _) {
        final host = registry.hosts[hostId];
        if (host == null) return const SizedBox.shrink();
        // The host object can change (removed and heard again), so the inner builder
        // re-subscribes whenever the outer one runs.
        return ListenableBuilder(
          listenable: host.changes,
          builder: (context, _) {
            debugOnBuild?.call(hostId);
            return _buildCard(context, registry, config, host);
          },
        );
      },
    );
  }

  Widget _buildCard(
    BuildContext context,
    HostRegistry registry,
    DisplayConfig config,
    HostState host,
  ) {
    final offline = host.status == HostStatus.offline;
    final pad = compact ? 12.0 : 16.0;
    final card = Card(
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: EdgeInsets.fromLTRB(pad, pad - 4, pad, pad),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _Header(host: host, registry: registry),
              if (!offline) ...[
                SizedBox(height: compact ? 4 : 6),
                _Body(
                  host: host,
                  slots: visibleSlots(config, host, now: registry.clock()),
                  showSparklines: config.showDetails,
                  compact: compact,
                  layout: config.cardLayout,
                  peakHoldSeconds: config.peakHoldSeconds,
                  smoothingWindowMs: config.smoothingWindowMs,
                  smoothAnimation: config.smoothAnimation,
                ),
              ],
            ],
          ),
        ),
      ),
    );
    return AnimatedOpacity(
      opacity: host.status == HostStatus.stale ? 0.5 : 1.0,
      duration: const Duration(milliseconds: 300),
      child: card,
    );
  }
}

/// Header row height (fits the offline card's compact Remove button).
const double _headerHeight = 32;

class _Header extends StatelessWidget {
  const _Header({required this.host, required this.registry});
  final HostState host;
  final HostRegistry registry;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    final theme = Theme.of(context);
    final statusColor = switch (host.status) {
      HostStatus.live => c.live,
      HostStatus.stale => c.stale,
      HostStatus.offline => c.offline,
    };
    return Row(
      children: [
        Icon(osIcon(host.latest.os), size: 18, color: c.muted),
        const SizedBox(width: 8),
        Expanded(
          child: Text(
            host.latest.host,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.titleMedium?.copyWith(
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
        const SizedBox(width: 8),
        Container(
          width: 9,
          height: 9,
          decoration: BoxDecoration(color: statusColor, shape: BoxShape.circle),
        ),
        const SizedBox(width: 6),
        AgeText(lastSeen: host.lastSeen),
        if (host.status == HostStatus.offline) ...[
          const SizedBox(width: 4),
          TextButton.icon(
            onPressed: () {
              registry.remove(host.id);
              context.read<DisplayConfig>().removeHostFromOrder(host.id);
            },
            icon: const Icon(Icons.close, size: 16),
            label: const Text('Remove'),
            style: TextButton.styleFrom(
              visualDensity: VisualDensity.compact,
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
              minimumSize: const Size(0, _headerHeight),
              foregroundColor: c.muted,
            ),
          ),
        ] else
          const SizedBox(height: _headerHeight),
      ],
    );
  }
}

/// "3s ago", refreshed every second.
class AgeText extends StatefulWidget {
  const AgeText({super.key, required this.lastSeen});
  final DateTime lastSeen;

  @override
  State<AgeText> createState() => _AgeTextState();
}

class _AgeTextState extends State<AgeText> {
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Text(
    ageText(widget.lastSeen, DateTime.now()),
    style: numberStyle(
      context,
      size: 12,
      weight: FontWeight.w500,
      color: RtmColors.of(context).muted,
    ),
  );
}

class _Body extends StatelessWidget {
  const _Body({
    required this.host,
    required this.slots,
    required this.showSparklines,
    required this.compact,
    required this.layout,
    required this.peakHoldSeconds,
    required this.smoothingWindowMs,
    required this.smoothAnimation,
  });

  final HostState host;
  final List<MetricSlot> slots;
  final bool showSparklines;
  final bool compact;
  final CardLayout layout;
  final double peakHoldSeconds;
  final int smoothingWindowMs;
  final bool smoothAnimation;

  static const double _gap = 10;

  @override
  Widget build(BuildContext context) {
    if (slots.isEmpty) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 8),
        child: Text(
          'No metrics shown. Enable some in Settings.',
          style: TextStyle(color: RtmColors.of(context).muted),
        ),
      );
    }
    return switch (layout) {
      CardLayout.bars => _bars(),
      CardLayout.lines => _lines(context),
      CardLayout.dials => _tiles(TileStyle.gauge),
      CardLayout.barTiles => _tiles(TileStyle.barTile),
      CardLayout.custom => _tiles(null),
    };
  }

  /// One full-width bar row per slot.
  Widget _bars() => Column(
    mainAxisSize: MainAxisSize.min,
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      for (final (i, s) in slots.indexed) ...[
        if (i > 0) SizedBox(height: compact ? 4 : 8),
        KeyedSubtree(
          key: ValueKey('bar-${host.id}-${s.key}'),
          child: MetricBarRow(
            host: host,
            slot: s,
            peakHoldSeconds: peakHoldSeconds,
            smoothingWindowMs: smoothingWindowMs,
            smoothAnimation: smoothAnimation,
            compact: compact,
          ),
        ),
      ],
    ],
  );

  /// Line graphs: 2 per row on a portrait phone, 4 across otherwise. If 4 would be
  /// narrower than ~56 dp each, fall back to 2 (never an uneven 3 + 1). One scroll-phase
  /// driver per card feeds every graph's painter.
  Widget _lines(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    final portraitPhone =
        size.shortestSide < 600 &&
        MediaQuery.orientationOf(context) == Orientation.portrait;
    return ScrollPhase(
      sampleKey: host.latest.seq,
      interval: host.interval,
      enabled: smoothAnimation,
      builder: (context, phase) => LayoutBuilder(
        builder: (context, box) {
          final w = box.maxWidth;
          const gap = 8.0;
          final perRow = graphColumns(
            twoUp: portraitPhone,
            maxWidth: w,
            gap: gap,
            minWidth: 56,
            count: slots.length,
          );
          final tileW = cellWidth(w, perRow, gap);
          final height = (tileW * 0.55).clamp(compact ? 56.0 : 64.0, 120.0);
          return Wrap(
            spacing: gap,
            runSpacing: gap,
            children: [
              for (final s in slots)
                SizedBox(
                  key: ValueKey('line-${host.id}-${s.key}'),
                  width: tileW,
                  child: MetricLineGraph(
                    host: host,
                    slot: s,
                    peakHoldSeconds: peakHoldSeconds,
                    smoothingWindowMs: smoothingWindowMs,
                    smoothAnimation: smoothAnimation,
                    height: height,
                    phase: phase,
                  ),
                ),
            ],
          );
        },
      ),
    );
  }

  /// Dials / bar tiles ([style] forced) or per-slot styles ([style] null).
  Widget _tiles(TileStyle? style) {
    return LayoutBuilder(
      builder: (context, box) {
        final w = box.maxWidth;
        final n = slots.length;
        // Comfortable: ~4 across on a 360 dp phone. Compact: everything on one row
        // unless tiles would drop below 52 dp.
        final minTile = compact ? 52.0 : 64.0;
        final fit = math.max(1, ((w + _gap) / (minTile + _gap)).floor());
        final perRow = compact
            ? math.min(n, fit)
            : math.min(n, math.min(fit, 6));
        final tileW = cellWidth(w, perRow, _gap);
        final gauge = compact ? math.min(tileW, 84.0) : math.min(tileW, 120.0);
        return Wrap(
          spacing: _gap,
          runSpacing: compact ? 8 : 14,
          children: [
            for (final s in slots)
              SizedBox(
                key: ValueKey('tile-${host.id}-${s.key}'),
                width: tileW,
                child: Center(
                  child: MetricTile(
                    host: host,
                    slot: s,
                    gaugeSize: gauge,
                    showSparkline: showSparklines,
                    sparklineHeight: compact ? 14 : 20,
                    style: style,
                    peakHoldSeconds: peakHoldSeconds,
                    smoothingWindowMs: smoothingWindowMs,
                    smoothAnimation: smoothAnimation,
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}
