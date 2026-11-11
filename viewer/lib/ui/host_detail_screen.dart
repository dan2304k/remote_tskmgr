import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../config/display_config.dart';
import '../core/host_registry.dart';
import 'format.dart';
import 'theme.dart';
import 'widgets/host_card.dart' show AgeText;
import 'widgets/metric_tile.dart';
import 'widgets/sparkline.dart';

/// Everything about one host: big gauges, history sparklines, detail tables, packet info.
class HostDetailScreen extends StatelessWidget {
  const HostDetailScreen({super.key, required this.hostId});

  static const route = '/host';

  final String hostId;

  @override
  Widget build(BuildContext context) {
    final registry = context.watch<HostRegistry>();
    final config = context.watch<DisplayConfig>();
    final peakHold = config.peakHoldSeconds;
    final host = registry.hosts[hostId];
    if (host == null) {
      return Scaffold(
        appBar: AppBar(title: const Text('Host')),
        body: const Center(child: Text('This host was removed.')),
      );
    }
    final metrics = host.latest.metrics;
    final landscape =
        MediaQuery.orientationOf(context) == Orientation.landscape;
    final perRow = landscape ? 4 : 2;
    final c = RtmColors.of(context);
    final theme = Theme.of(context);

    return Scaffold(
      appBar: AppBar(title: Text(host.latest.host)),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 32),
        children: [
          if (metrics.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 24),
              child: Text(
                'This host reports no metrics.',
                style: TextStyle(color: c.muted),
              ),
            )
          else
            LayoutBuilder(
              builder: (context, box) {
                const gap = 16.0;
                final w = (box.maxWidth - gap * (perRow - 1)) / perRow;
                return Wrap(
                  spacing: gap,
                  runSpacing: gap,
                  children: [
                    for (final k in metrics.keys)
                      SizedBox(
                        width: w,
                        child: Center(
                          child: MetricTile(
                            host: host,
                            slot: MetricSlot(key: k),
                            style: TileStyle.gauge,
                            gaugeSize: math.min(w, 220),
                            peakHoldSeconds: peakHold,
                            smoothingWindowMs: config.smoothingWindowMs,
                            smoothAnimation: config.smoothAnimation,
                          ),
                        ),
                      ),
                  ],
                );
              },
            ),
          if (metrics.isNotEmpty) ...[
            _SectionTitle('History'),
            for (final e in metrics.entries)
              Padding(
                padding: const EdgeInsets.only(bottom: 14),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            e.value.label,
                            style: theme.textTheme.labelLarge?.copyWith(
                              color: c.muted,
                              letterSpacing: 0.8,
                            ),
                          ),
                        ),
                        Text(() {
                          final f = formatValue(e.value.v, e.value.unit);
                          return e.value.v == null
                              ? f.number
                              : '${f.number} ${f.unit}'.trim();
                        }(), style: numberStyle(context, size: 15)),
                      ],
                    ),
                    const SizedBox(height: 4),
                    Sparkline(
                      key: ValueKey('detail-spark-${e.key}'),
                      values: host.history[e.key]?.values ?? const <double?>[],
                      capacity: host.history[e.key]?.capacity ?? 120,
                      min: e.value.min,
                      max: graphMax(e.value.max, e.value.unit),
                      color: c.forValue(
                        e.value.v,
                        min: e.value.min,
                        max: e.value.max,
                      ),
                      height: 48,
                      peak: peakFor(host, e.key, peakHold),
                      unit: e.value.unit,
                    ),
                  ],
                ),
              ),
          ],
          for (final e in metrics.entries)
            if (e.value.detail != null && e.value.detail!.isNotEmpty) ...[
              _SectionTitle('${e.value.label} details'),
              _KvTable(rows: flattenDetail(e.value.detail!)),
            ],
          _SectionTitle('Packet'),
          _KvTable(
            rows: [
              MapEntry('id', host.id),
              MapEntry('os', host.latest.os),
              MapEntry('seq', '${host.latest.seq}'),
              MapEntry('interval', '${host.latest.intervalMs} ms'),
              MapEntry('source', host.lastSource ?? '–'),
              MapEntry('status', host.status.name),
              MapEntry(
                'sender clock',
                DateTime.fromMillisecondsSinceEpoch(host.latest.ts)
                    .toLocal()
                    .toString(),
              ),
            ],
            trailing: Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Row(
                children: [
                  Expanded(
                    child: Text('last seen', style: TextStyle(color: c.muted)),
                  ),
                  AgeText(lastSeen: host.lastSeen),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _SectionTitle extends StatelessWidget {
  const _SectionTitle(this.text);
  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: 24, bottom: 8),
    child: Text(
      text.toUpperCase(),
      style: Theme.of(context).textTheme.labelMedium?.copyWith(
        color: RtmColors.of(context).accent,
        letterSpacing: 1.2,
        fontWeight: FontWeight.w700,
      ),
    ),
  );
}

class _KvTable extends StatelessWidget {
  const _KvTable({required this.rows, this.trailing});
  final List<MapEntry<String, String>> rows;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final r in rows)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  flex: 2,
                  child: Text(r.key, style: TextStyle(color: c.muted)),
                ),
                const SizedBox(width: 12),
                Expanded(
                  flex: 3,
                  child: Text(
                    r.value,
                    textAlign: TextAlign.right,
                    style: numberStyle(
                      context,
                      size: 14,
                      weight: FontWeight.w500,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ?trailing,
      ],
    );
  }
}
