import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:wakelock_plus/wakelock_plus.dart';

import '../config/display_config.dart';
import '../core/host_registry.dart';
import '../net/net_controller.dart';
import 'host_detail_screen.dart';
import 'host_order.dart';
import 'settings_screen.dart';
import 'theme.dart';
import 'widgets/host_card.dart';

/// Column count for the host grid. Phones (shortest side < 600): 1 in portrait,
/// 2 in landscape. Tablets: one column per ~420 dp, 2..3.
int dashboardColumns(Size size, Orientation o) {
  if (size.shortestSide < 600) return o == Orientation.portrait ? 1 : 2;
  return (size.width / 420).floor().clamp(2, 3);
}

class DashboardScreen extends StatefulWidget {
  const DashboardScreen({super.key, this.manageWakelock = true});

  static const route = '/';

  /// Hold the screen on while this screen is mounted and config.keepAwake is set.
  /// Tests pass false (no platform plugin).
  final bool manageWakelock;

  @override
  State<DashboardScreen> createState() => _DashboardScreenState();
}

class _DashboardScreenState extends State<DashboardScreen> {
  DisplayConfig? _config;
  bool? _wakeOn;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!widget.manageWakelock) return;
    final c = context.read<DisplayConfig>();
    if (!identical(c, _config)) {
      _config?.removeListener(_syncWakelock);
      _config = c..addListener(_syncWakelock);
      _syncWakelock();
    }
  }

  void _syncWakelock() {
    final want = _config?.keepAwake ?? false;
    if (want == _wakeOn) return;
    _wakeOn = want;
    unawaited(_setWakelock(want));
  }

  static Future<void> _setWakelock(bool on) async {
    try {
      await WakelockPlus.toggle(enable: on);
    } catch (e) {
      debugPrint('rtm: wakelock unavailable: $e');
    }
  }

  @override
  void dispose() {
    _config?.removeListener(_syncWakelock);
    if (_wakeOn == true) unawaited(_setWakelock(false));
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        // Compact toolbar: the default 64 dp bar reads as empty space above the cards.
        toolbarHeight: 44,
        title: const Text('RTM'),
        actions: [
          const _StatsChip(),
          const LayoutCycleButton(),
          IconButton(
            tooltip: 'Settings',
            icon: const Icon(Icons.settings_outlined),
            onPressed: () =>
                Navigator.of(context).pushNamed(SettingsScreen.route),
          ),
        ],
      ),
      body: const Column(
        children: [
          _ErrorBanner(),
          Expanded(child: _HostGrid()),
        ],
      ),
    );
  }
}

/// Icon for each card layout (shared by the AppBar button and Settings).
IconData cardLayoutIcon(CardLayout l) => switch (l) {
  CardLayout.dials => Icons.speed,
  CardLayout.bars => Icons.align_horizontal_left,
  CardLayout.barTiles => Icons.grid_view,
  CardLayout.lines => Icons.show_chart,
  CardLayout.custom => Icons.dashboard_customize_outlined,
};

/// Cycles the card layout dials -> bars -> bar tiles -> lines -> custom -> dials and flashes the new
/// mode's name in a SnackBar.
class LayoutCycleButton extends StatelessWidget {
  const LayoutCycleButton({super.key});

  @override
  Widget build(BuildContext context) {
    final config = context.watch<DisplayConfig>();
    final mode = config.cardLayout;
    return IconButton(
      key: const ValueKey('layout-cycle'),
      tooltip: 'Layout: ${mode.label} (tap for ${mode.next.label})',
      icon: Icon(cardLayoutIcon(mode)),
      onPressed: () {
        final next = config.cardLayout.next;
        config.cardLayout = next;
        final messenger = ScaffoldMessenger.maybeOf(context);
        messenger
          ?..hideCurrentSnackBar()
          ..showSnackBar(
            SnackBar(
              key: const ValueKey('layout-snack'),
              content: Row(
                children: [
                  Icon(
                    cardLayoutIcon(next),
                    size: 18,
                    color: Theme.of(context).colorScheme.onInverseSurface,
                  ),
                  const SizedBox(width: 10),
                  Text(next.label),
                ],
              ),
              behavior: SnackBarBehavior.floating,
              duration: const Duration(milliseconds: 1200),
            ),
          );
      },
    );
  }
}

class _StatsChip extends StatelessWidget {
  const _StatsChip();

  @override
  Widget build(BuildContext context) {
    // NetController refreshes its counters once a second, so the chip decays to 0
    // when packets stop.
    final net = context.watch<NetController>();
    final c = RtmColors.of(context);
    final n = net.hostCount;
    final narrow = MediaQuery.sizeOf(context).width < 360;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: Chip(
        key: const ValueKey('stats-chip'),
        visualDensity: VisualDensity.compact,
        padding: const EdgeInsets.symmetric(horizontal: 2),
        labelPadding: const EdgeInsets.symmetric(horizontal: 6),
        side: BorderSide.none,
        backgroundColor: Theme.of(context).colorScheme.surfaceContainerHigh,
        label: Text(
          // Below 360 dp the AppBar is tight: drop "pkt".
          '${net.packetsPerSecond}${narrow ? '/s' : ' pkt/s'} · '
          '$n ${n == 1 ? 'host' : 'hosts'}',
          style: numberStyle(
            context,
            size: 12,
            weight: FontWeight.w500,
            color: c.muted,
          ),
        ),
      ),
    );
  }
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner();

  @override
  Widget build(BuildContext context) {
    final net = context.watch<NetController>();
    final err = net.lastError;
    if (err == null) return const SizedBox.shrink();
    final scheme = Theme.of(context).colorScheme;
    return MaterialBanner(
      key: const ValueKey('error-banner'),
      backgroundColor: scheme.errorContainer,
      leading: Icon(Icons.error_outline, color: scheme.onErrorContainer),
      content: Text(err, style: TextStyle(color: scheme.onErrorContainer)),
      actions: [
        TextButton(
          onPressed: () => unawaited(_restart(net)),
          child: const Text('Retry'),
        ),
      ],
    );
  }

  static Future<void> _restart(NetController net) async {
    try {
      await net.restart();
    } catch (e) {
      debugPrint('rtm: restart failed: $e');
    }
  }
}

class _HostGrid extends StatefulWidget {
  const _HostGrid();

  @override
  State<_HostGrid> createState() => _HostGridState();
}

/// Host cards in a 1..3 column grid. Long-press a card to lift it, drop it on another
/// card to take that card's slot; the result is saved as [DisplayConfig.hostOrder].
/// Dragging near the top/bottom edge scrolls the list.
class _HostGridState extends State<_HostGrid> {
  final _scroll = ScrollController();
  final _viewportKey = GlobalKey();
  Timer? _autoScroll;
  double _autoScrollStep = 0;

  /// Height of the edge band that triggers auto-scroll while dragging.
  static const double _edge = 72;

  @override
  void dispose() {
    _autoScroll?.cancel();
    _scroll.dispose();
    super.dispose();
  }

  void _onDragUpdate(DragUpdateDetails d) {
    final box = _viewportKey.currentContext?.findRenderObject() as RenderBox?;
    if (box == null || !box.hasSize) return;
    final y = box.globalToLocal(d.globalPosition).dy;
    final h = box.size.height;
    double step = 0;
    if (y < _edge) {
      step = -12 * (1 - (y / _edge).clamp(0.0, 1.0));
    } else if (y > h - _edge) {
      step = 12 * (1 - ((h - y) / _edge).clamp(0.0, 1.0));
    }
    _autoScrollStep = step;
    if (step == 0) {
      _stopAutoScroll();
    } else {
      _autoScroll ??= Timer.periodic(const Duration(milliseconds: 16), (_) {
        if (!_scroll.hasClients) return;
        final pos = _scroll.position;
        final next = (pos.pixels + _autoScrollStep).clamp(
          pos.minScrollExtent,
          pos.maxScrollExtent,
        );
        if (next != pos.pixels) _scroll.jumpTo(next);
      });
    }
  }

  void _stopAutoScroll() {
    _autoScroll?.cancel();
    _autoScroll = null;
  }

  static void _haptic() {
    try {
      unawaited(HapticFeedback.mediumImpact().catchError((Object _) {}));
    } catch (_) {}
  }

  @override
  Widget build(BuildContext context) {
    final config = context.watch<DisplayConfig>();
    final order = config.hostOrder;
    // Rebuild the grid only when the set/order of hosts changes; each card listens
    // to the registry on its own for value updates.
    final ids = context.select<HostRegistry, _Ids>(
      (r) => _Ids(orderHosts(r.ordered, order).map((h) => h.id).toList()),
    );
    if (ids.list.isEmpty) return const _EmptyState();
    final size = MediaQuery.sizeOf(context);
    final orientation = MediaQuery.orientationOf(context);
    final cols = dashboardColumns(size, orientation);
    final phoneLandscape =
        size.shortestSide < 600 && orientation == Orientation.landscape;
    final compact = phoneLandscape || config.density == GridDensity.compact;
    final gap = compact ? 8.0 : 12.0;

    final rows = (ids.list.length + cols - 1) ~/ cols;
    return KeyedSubtree(
      key: _viewportKey,
      child: _list(rows, cols, ids.list, compact, gap),
    );
  }

  Widget _list(int rows, int cols, List<String> ids, bool compact, double gap) {
    return ListView.separated(
      key: const ValueKey('host-grid'),
      controller: _scroll,
      // 4 dp below the compact AppBar, whatever the density.
      padding: EdgeInsets.fromLTRB(gap, 4, gap, gap * 2),
      itemCount: rows,
      separatorBuilder: (_, _) => SizedBox(height: gap),
      itemBuilder: (context, row) {
        final cells = <Widget>[];
        for (var c = 0; c < cols; c++) {
          final i = row * cols + c;
          if (c > 0) cells.add(SizedBox(width: gap));
          cells.add(
            Expanded(
              child: i < ids.length
                  ? _cell(context, ids, ids[i], compact)
                  : const SizedBox.shrink(),
            ),
          );
        }
        return Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: cells,
        );
      },
    );
  }

  Widget _cell(
    BuildContext context,
    List<String> ids,
    String id,
    bool compact,
  ) {
    final accent = RtmColors.of(context).accent;
    Widget card({Key? key}) => HostCard(
      key: key,
      hostId: id,
      compact: compact,
      onTap: () =>
          Navigator.of(context)
              .pushNamed(HostDetailScreen.route, arguments: id),
    );
    return LayoutBuilder(
      builder: (context, box) => DragTarget<String>(
        key: ValueKey('drop-$id'),
        onWillAcceptWithDetails: (d) => d.data != id,
        onAcceptWithDetails: (d) {
          _stopAutoScroll();
          context.read<DisplayConfig>().setHostOrder(dropInto(ids, d.data, id));
        },
        builder: (context, candidates, _) => DecoratedBox(
          position: DecorationPosition.foreground,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            border: candidates.isEmpty
                ? null
                : Border.all(color: accent, width: 2),
          ),
          child: LongPressDraggable<String>(
            data: id,
            onDragStarted: _haptic,
            onDragUpdate: _onDragUpdate,
            onDragEnd: (_) => _stopAutoScroll(),
            onDraggableCanceled: (_, _) => _stopAutoScroll(),
            feedback: Material(
              type: MaterialType.transparency,
              child: Opacity(
                opacity: 0.75,
                child: Transform.scale(
                  scale: 0.92,
                  child: SizedBox(width: box.maxWidth, child: card()),
                ),
              ),
            ),
            childWhenDragging: Opacity(
              opacity: 0.3,
              child: card(key: ValueKey('host-card-$id')),
            ),
            child: card(key: ValueKey('host-card-$id')),
          ),
        ),
      ),
    );
  }
}

/// Value-equal id list so `select` only rebuilds on real changes.
class _Ids {
  _Ids(this.list);
  final List<String> list;

  @override
  bool operator ==(Object other) =>
      other is _Ids && listEquals(other.list, list);

  @override
  int get hashCode => Object.hashAll(list);
}

class _EmptyState extends StatelessWidget {
  const _EmptyState();

  @override
  Widget build(BuildContext context) {
    final config = context.watch<DisplayConfig>();
    final net = context.watch<NetController>();
    final c = RtmColors.of(context);
    final theme = Theme.of(context);
    final incompatible = net.incompatiblePackets;
    final tcp = config.tcpEnabled ? ' · TCP ${config.tcpPort}' : '';
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.wifi_tethering, size: 56, color: c.accent),
            const SizedBox(height: 20),
            Text(
              'Listening on UDP ${config.udpPort}$tcp …',
              textAlign: TextAlign.center,
              style: theme.textTheme.titleMedium,
            ),
            const SizedBox(height: 4),
            Text(
              'no hosts yet',
              style: theme.textTheme.titleMedium?.copyWith(color: c.muted),
            ),
            const SizedBox(height: 16),
            Text(
              'Run rtm-sender on your PCs, with this device on the same Wi-Fi network.',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyMedium?.copyWith(color: c.muted),
            ),
            if (incompatible > 0) ...[
              const SizedBox(height: 16),
              Text(
                'Ignored $incompatible packets from an incompatible sender'
                '${net.lastIncompatibleSource != null ? ' (${net.lastIncompatibleSource})' : ''}. '
                'Update rtm-sender.',
                key: const ValueKey('incompatible-hint'),
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium?.copyWith(color: c.warn),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
