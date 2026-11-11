import 'package:flutter/material.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:provider/provider.dart';

import '../config/display_config.dart';
import '../core/host_registry.dart';
import '../net/net_controller.dart';
import 'format.dart';
import 'host_order.dart';
import 'dashboard_screen.dart' show cardLayoutIcon;
import 'theme.dart';

class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  static const route = '/settings';

  @override
  Widget build(BuildContext context) {
    final config = context.watch<DisplayConfig>();
    return Scaffold(
      appBar: AppBar(title: const Text('Settings')),
      body: CustomScrollView(
        key: const ValueKey('settings-scroll'),
        slivers: [
          const _Header('Network'),
          SliverList.list(children: [_NetworkSection(config: config)]),
          const _Header('Display'),
          SliverList.list(children: [_DisplaySection(config: config)]),
          const _Header('Metrics'),
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
              child: Text(
                'Drag to reorder. New metrics from senders appear here automatically. '
                '${_styleHint(config.cardLayout)}',
                key: const ValueKey('metrics-hint'),
                style: TextStyle(color: RtmColors.of(context).muted),
              ),
            ),
          ),
          _MetricsSection(config: config),
          const _Header('Hosts'),
          _HostsSection(config: config),
          SliverToBoxAdapter(child: _HostOrderReset(config: config)),
          const _Header('About'),
          const SliverToBoxAdapter(child: _AboutSection()),
          const SliverToBoxAdapter(child: SizedBox(height: 32)),
        ],
      ),
    );
  }
}

String _styleHint(CardLayout l) => l == CardLayout.custom
    ? 'Each metric uses the style chosen here (Custom layout).'
    : 'Per-metric styles apply only in the Custom layout '
          '(now: ${l.label}).';

class _Header extends StatelessWidget {
  const _Header(this.text);
  final String text;

  @override
  Widget build(BuildContext context) => SliverToBoxAdapter(
    child: Padding(
      padding: const EdgeInsets.fromLTRB(16, 24, 16, 8),
      child: Text(
        text.toUpperCase(),
        style: Theme.of(context).textTheme.labelMedium?.copyWith(
          color: RtmColors.of(context).accent,
          letterSpacing: 1.2,
          fontWeight: FontWeight.w700,
        ),
      ),
    ),
  );
}

// ---------------------------------------------------------------- Network

class _NetworkSection extends StatelessWidget {
  const _NetworkSection({required this.config});
  final DisplayConfig config;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        _NumberField(
          key: const ValueKey('udp-port'),
          label: 'UDP port',
          value: config.udpPort,
          onSubmitted: (v) => config.udpPort = v,
        ),
        SwitchListTile(
          title: const Text('Listen on TCP'),
          subtitle: const Text(
            'For senders that cannot reach this device by broadcast',
          ),
          value: config.tcpEnabled,
          onChanged: (v) => config.tcpEnabled = v,
        ),
        if (config.tcpEnabled)
          _NumberField(
            key: const ValueKey('tcp-port'),
            label: 'TCP port',
            value: config.tcpPort,
            onSubmitted: (v) => config.tcpPort = v,
          ),
        _SecretField(
          value: config.sharedSecret,
          onSubmitted: (v) => config.sharedSecret = v,
        ),
        const _Diagnostics(),
      ],
    );
  }
}

class _Diagnostics extends StatelessWidget {
  const _Diagnostics();

  @override
  Widget build(BuildContext context) {
    final net = context.watch<NetController>();
    final c = RtmColors.of(context);
    final parts = [
      '${net.packetsPerSecond} pkts/s',
      '${net.hostCount} hosts',
      '${net.tcpConnections} tcp',
      net.running ? 'running' : 'stopped',
      if (net.bindError != null) net.bindError!,
      if (net.lastPacketError != null)
        '${net.lastPacketError!} (${net.lastPacketErrorAge!.inSeconds} s ago)',
    ];
    final isError = net.bindError != null || net.lastPacketError != null;
    return ListTile(
      key: const ValueKey('diagnostics'),
      leading: Icon(Icons.monitor_heart_outlined, color: c.muted),
      title: const Text('Diagnostics'),
      subtitle: Text(
        parts.join(' · '),
        style: numberStyle(
          context,
          size: 13,
          weight: FontWeight.w400,
          color: isError ? Theme.of(context).colorScheme.error : c.muted,
        ),
      ),
    );
  }
}

/// Port entry; applies on submit or focus loss when the value is a valid port.
class _NumberField extends StatefulWidget {
  const _NumberField({
    super.key,
    required this.label,
    required this.value,
    required this.onSubmitted,
  });

  final String label;
  final int value;
  final ValueChanged<int> onSubmitted;

  @override
  State<_NumberField> createState() => _NumberFieldState();
}

class _NumberFieldState extends State<_NumberField> {
  late final _ctl = TextEditingController(text: '${widget.value}');
  final _focus = FocusNode();
  String? _error;

  @override
  void initState() {
    super.initState();
    _focus.addListener(() {
      if (!_focus.hasFocus) _apply(_ctl.text);
    });
  }

  @override
  void didUpdateWidget(_NumberField old) {
    super.didUpdateWidget(old);
    if (!_focus.hasFocus && old.value != widget.value) {
      _ctl.text = '${widget.value}';
    }
  }

  void _apply(String s) {
    final v = int.tryParse(s.trim());
    if (v == null || v < 1 || v > 65535) {
      setState(() => _error = '1–65535');
      return;
    }
    setState(() => _error = null);
    if (v != widget.value) widget.onSubmitted(v);
  }

  @override
  void dispose() {
    _ctl.dispose();
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
    child: TextField(
      controller: _ctl,
      focusNode: _focus,
      keyboardType: TextInputType.number,
      decoration: InputDecoration(
        labelText: widget.label,
        errorText: _error,
        border: const OutlineInputBorder(),
        isDense: true,
      ),
      onSubmitted: _apply,
    ),
  );
}

class _SecretField extends StatefulWidget {
  const _SecretField({required this.value, required this.onSubmitted});
  final String value;
  final ValueChanged<String> onSubmitted;

  @override
  State<_SecretField> createState() => _SecretFieldState();
}

class _SecretFieldState extends State<_SecretField> {
  late final _ctl = TextEditingController(text: widget.value);
  final _focus = FocusNode();
  bool _obscure = true;

  @override
  void initState() {
    super.initState();
    _focus.addListener(() {
      if (!_focus.hasFocus) _apply();
    });
  }

  void _apply() {
    if (_ctl.text != widget.value) widget.onSubmitted(_ctl.text);
  }

  @override
  void dispose() {
    _ctl.dispose();
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
    child: TextField(
      key: const ValueKey('shared-secret'),
      controller: _ctl,
      focusNode: _focus,
      obscureText: _obscure,
      autocorrect: false,
      enableSuggestions: false,
      decoration: InputDecoration(
        labelText: 'Shared secret',
        helperText: 'Empty = accept unsigned packets',
        border: const OutlineInputBorder(),
        isDense: true,
        suffixIcon: IconButton(
          tooltip: _obscure ? 'Show' : 'Hide',
          icon: Icon(_obscure ? Icons.visibility : Icons.visibility_off),
          onPressed: () => setState(() => _obscure = !_obscure),
        ),
      ),
      onSubmitted: (_) => _apply(),
    ),
  );
}

// ---------------------------------------------------------------- Display

class _DisplaySection extends StatelessWidget {
  const _DisplaySection({required this.config});
  final DisplayConfig config;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        _LabeledControl(
          label: 'Theme',
          child: SegmentedButton<ThemeMode>(
            showSelectedIcon: false,
            segments: const [
              ButtonSegment(value: ThemeMode.system, label: Text('Auto')),
              ButtonSegment(value: ThemeMode.light, label: Text('Light')),
              ButtonSegment(value: ThemeMode.dark, label: Text('Dark')),
            ],
            selected: {config.themeMode},
            onSelectionChanged: (s) => config.themeMode = s.first,
          ),
        ),
        _LabeledControl(
          label: 'Card layout',
          // Chips wrap; five segments do not fit a 320-360 dp phone in one row.
          child: Wrap(
            key: const ValueKey('layout-segments'),
            spacing: 8,
            runSpacing: 4,
            children: [
              for (final l in CardLayout.values)
                ChoiceChip(
                  avatar: Icon(cardLayoutIcon(l), size: 18),
                  showCheckmark: false,
                  label: Text(l.label),
                  selected: config.cardLayout == l,
                  onSelected: (_) => config.cardLayout = l,
                ),
            ],
          ),
        ),
        _PeakHoldControl(config: config),
        _SmoothingControl(config: config),
        SwitchListTile(
          key: const ValueKey('smooth-animation'),
          title: const Text('Smooth animation'),
          subtitle: const Text(
            'Glide between samples at the screen frame rate',
          ),
          value: config.smoothAnimation,
          onChanged: (v) => config.smoothAnimation = v,
        ),
        _LabeledControl(
          label: 'Density',
          child: SegmentedButton<GridDensity>(
            showSelectedIcon: false,
            segments: const [
              ButtonSegment(
                value: GridDensity.comfortable,
                label: Text('Comfy'),
              ),
              ButtonSegment(value: GridDensity.compact, label: Text('Compact')),
            ],
            selected: {config.density},
            onSelectionChanged: (s) => config.density = s.first,
          ),
        ),
        SwitchListTile(
          title: const Text('Show sparklines on cards'),
          value: config.showDetails,
          onChanged: (v) => config.showDetails = v,
        ),
        SwitchListTile(
          title: const Text('Keep screen awake'),
          subtitle: const Text('While the dashboard is open'),
          value: config.keepAwake,
          onChanged: (v) => config.keepAwake = v,
        ),
      ],
    );
  }
}

/// Peak hold slider, 0..60 s in 1 s steps; 0 = off.
class _PeakHoldControl extends StatelessWidget {
  const _PeakHoldControl({required this.config});
  final DisplayConfig config;

  static String _text(double s) => s <= 0 ? 'Off' : '${s.round()} s';

  @override
  Widget build(BuildContext context) {
    final v = config.peakHoldSeconds;
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text('Peak hold', style: theme.textTheme.bodyLarge),
              ),
              Text(
                _text(v),
                key: const ValueKey('peak-hold-value'),
                style: numberStyle(context, size: 15, weight: FontWeight.w500),
              ),
            ],
          ),
          Text(
            'Marks each metric\'s highest recent value; 0 turns it off',
            style: theme.textTheme.bodySmall?.copyWith(
              color: RtmColors.of(context).muted,
            ),
          ),
          Slider(
            key: const ValueKey('peak-hold-slider'),
            value: v,
            max: kMaxPeakHoldSeconds,
            divisions: kMaxPeakHoldSeconds.round(),
            label: _text(v),
            onChanged: (x) => config.peakHoldSeconds = x.roundToDouble(),
          ),
        ],
      ),
    );
  }
}

/// Smoothing window slider, 0..5 s in 100 ms steps; 0 = off.
class _SmoothingControl extends StatelessWidget {
  const _SmoothingControl({required this.config});
  final DisplayConfig config;

  static String _text(int ms) =>
      ms <= 0 ? 'Off' : '${(ms / 1000).toStringAsFixed(1)} s';

  @override
  Widget build(BuildContext context) {
    final v = config.smoothingWindowMs;
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  'Smoothing window',
                  style: theme.textTheme.bodyLarge,
                ),
              ),
              Text(
                _text(v),
                key: const ValueKey('smoothing-value'),
                style: numberStyle(context, size: 15, weight: FontWeight.w500),
              ),
            ],
          ),
          Text(
            'Averages displayed values from senders faster than this; '
            'graphs and peaks stay raw',
            style: theme.textTheme.bodySmall?.copyWith(
              color: RtmColors.of(context).muted,
            ),
          ),
          Slider(
            key: const ValueKey('smoothing-slider'),
            value: v.toDouble(),
            max: kMaxSmoothingWindowMs.toDouble(),
            divisions: kMaxSmoothingWindowMs ~/ 100,
            label: _text(v),
            onChanged: (x) =>
                config.smoothingWindowMs = (x / 100).round() * 100,
          ),
        ],
      ),
    );
  }
}

/// Title above a full-width control (segmented buttons do not fit a ListTile trailing
/// slot on narrow phones).
class _LabeledControl extends StatelessWidget {
  const _LabeledControl({required this.label, required this.child});
  final String label;
  final Widget child;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(label, style: Theme.of(context).textTheme.bodyLarge),
        const SizedBox(height: 8),
        child,
      ],
    ),
  );
}

// ---------------------------------------------------------------- Metrics

class _MetricsSection extends StatelessWidget {
  const _MetricsSection({required this.config});
  final DisplayConfig config;

  static const _styleNames = {
    TileStyle.gauge: 'Gauge',
    TileStyle.bar: 'Bar',
    TileStyle.sparkline: 'Sparkline',
    TileStyle.number: 'Number',
    TileStyle.barTile: 'Bar tile',
  };

  @override
  Widget build(BuildContext context) {
    final slots = config.slots;
    if (slots.isEmpty) {
      return SliverToBoxAdapter(
        child: ListTile(
          title: Text(
            'No metrics seen yet.',
            style: TextStyle(color: RtmColors.of(context).muted),
          ),
        ),
      );
    }
    return SliverReorderableList(
      itemCount: slots.length,
      onReorderItem: config.reorder,
      itemBuilder: (context, i) {
        final s = slots[i];
        return Material(
          key: ValueKey('slot-${s.key}'),
          type: MaterialType.transparency,
          child: ListTile(
            contentPadding: const EdgeInsets.only(left: 4, right: 8),
            leading: ReorderableDragStartListener(
              index: i,
              child: const Padding(
                padding: EdgeInsets.all(8),
                child: Icon(Icons.drag_indicator),
              ),
            ),
            title: Text(s.key),
            trailing: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                // Dimmed (still editable) outside the Custom layout.
                Opacity(
                  opacity: config.cardLayout == CardLayout.custom ? 1 : 0.5,
                  child: DropdownButton<TileStyle>(
                    key: ValueKey('style-${s.key}'),
                    value: s.style,
                    underline: const SizedBox.shrink(),
                    items: [
                      for (final t in TileStyle.values)
                        DropdownMenuItem(
                          value: t,
                          child: Text(_styleNames[t]!),
                        ),
                    ],
                    onChanged: (t) {
                      if (t != null) config.setSlotStyle(s.key, t);
                    },
                  ),
                ),
                Switch(
                  value: s.visible,
                  onChanged: (v) => config.setSlotVisible(s.key, v),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

// ---------------------------------------------------------------- Hosts

/// Hosts in dashboard order; drag the handle to reorder (saved as
/// [DisplayConfig.hostOrder]). Offline hosts can be removed here.
class _HostsSection extends StatelessWidget {
  const _HostsSection({required this.config});
  final DisplayConfig config;

  @override
  Widget build(BuildContext context) {
    final reg = context.watch<HostRegistry>();
    final c = RtmColors.of(context);
    final hosts = orderHosts(reg.ordered, config.hostOrder);
    if (hosts.isEmpty) {
      return SliverToBoxAdapter(
        child: ListTile(
          title: Text('No hosts yet.', style: TextStyle(color: c.muted)),
        ),
      );
    }
    final ids = [for (final h in hosts) h.id];
    return SliverReorderableList(
      itemCount: hosts.length,
      onReorderItem: (from, to) {
        final next = [...ids];
        final id = next.removeAt(from);
        next.insert(to.clamp(0, next.length), id);
        config.setHostOrder(next);
      },
      itemBuilder: (context, i) {
        final h = hosts[i];
        return Material(
          key: ValueKey('host-row-${h.id}'),
          type: MaterialType.transparency,
          child: ListTile(
            contentPadding: const EdgeInsets.only(left: 4, right: 8),
            leading: ReorderableDragStartListener(
              index: i,
              child: const Padding(
                padding: EdgeInsets.all(8),
                child: Icon(Icons.drag_indicator),
              ),
            ),
            title: Text(h.latest.host),
            subtitle: Text(
              '${h.status.name} · ${ageText(h.lastSeen, DateTime.now())}',
            ),
            trailing: h.status == HostStatus.offline
                ? TextButton(
                    onPressed: () {
                      reg.remove(h.id);
                      config.removeHostFromOrder(h.id);
                    },
                    child: const Text('Remove'),
                  )
                : Icon(osIcon(h.latest.os), color: c.muted),
          ),
        );
      },
    );
  }
}

class _HostOrderReset extends StatelessWidget {
  const _HostOrderReset({required this.config});
  final DisplayConfig config;

  @override
  Widget build(BuildContext context) {
    final manual = config.hostOrder.isNotEmpty;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 0),
      child: Row(
        children: [
          Expanded(
            child: Text(
              manual
                  ? 'Custom order. Long-press a card on the dashboard to move it.'
                  : 'Automatic order (live first, then by name). '
                        'Long-press a card on the dashboard to move it.',
              style: TextStyle(color: RtmColors.of(context).muted),
            ),
          ),
          TextButton(
            key: const ValueKey('reset-host-order'),
            onPressed: manual ? config.resetHostOrder : null,
            child: const Text('Reset order'),
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------- About

class _AboutSection extends StatefulWidget {
  const _AboutSection();

  @override
  State<_AboutSection> createState() => _AboutSectionState();
}

class _AboutSectionState extends State<_AboutSection> {
  late final Future<String> _version = _loadVersion();

  static Future<String> _loadVersion() async {
    try {
      final p = await PackageInfo.fromPlatform();
      return p.buildNumber.isEmpty
          ? p.version
          : '${p.version}+${p.buildNumber}';
    } catch (_) {
      return 'unknown';
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = RtmColors.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        FutureBuilder<String>(
          future: _version,
          builder: (context, snap) => ListTile(
            title: const Text('RTM Viewer'),
            subtitle: Text('Version ${snap.data ?? '…'}'),
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Text(
            'Live CPU / GPU / memory / disk stats from PCs running rtm-sender. '
            'Defaults: UDP 48000 broadcast, TCP 48000.',
            style: TextStyle(color: c.muted, height: 1.4),
          ),
        ),
      ],
    );
  }
}
