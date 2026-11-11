import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/net/net_controller.dart';
import 'package:rtm_viewer/ui/app.dart';

Map<String, Metric> standardMetrics({double? gpu = 61}) => {
  'cpu': const Metric(
    v: 23.4,
    unit: '%',
    label: 'CPU',
    max: 100,
    detail: {
      'cores': [12.0, 44.1, 8.3, 29.0],
      'freq_mhz': 3800,
      'temp_c': 61,
    },
  ),
  'gpu': Metric(
    v: gpu,
    unit: '%',
    label: 'GPU',
    max: 100,
    detail: const {'name': 'NVIDIA GeForce RTX 3070', 'backend': 'nvml'},
  ),
  'mem': const Metric(v: 91, unit: '%', label: 'MEM', max: 100),
  'disk': const Metric(
    v: 14.2,
    unit: 'MB/s',
    label: 'DISK',
    detail: {
      'read_mbps': 2.1,
      'write_mbps': 12.1,
      'disks': {
        'nvme0': {'read_mbps': 2.0, 'write_mbps': 12.0},
      },
    },
  ),
};

HostState makeHost(
  String id,
  String name, {
  String os = 'linux',
  Map<String, Metric>? metrics,
  HostStatus status = HostStatus.live,
  DateTime? lastSeen,
}) {
  final p = Packet(
    v: 1,
    id: id,
    host: name,
    os: os,
    ts: 1762640000123,
    seq: 48213,
    intervalMs: 1000,
    metrics: metrics ?? standardMetrics(),
  );
  final h = HostState(id: id, latest: p, lastSeen: lastSeen ?? DateTime.now())
    ..status = status
    ..lastSource = '192.168.1.10:51234 udp';
  p.metrics.forEach((k, m) {
    h.metricLastSeen[k] = h.lastSeen;
    final r = h.history[k] = RingBuffer(120);
    for (var i = 0; i < 30; i++) {
      // A null metric (e.g. an unsupported GPU) is null throughout.
      r.add(i == 10 || m.v == null ? null : m.v! * (0.5 + (i % 7) / 10));
    }
    r.add(m.v);
  });
  return h;
}

class UiHarness {
  UiHarness({List<HostState> hosts = const [], HostRegistry? registry})
    : registry = registry ?? HostRegistry() {
    for (final h in hosts) {
      this.registry.hosts[h.id] = h;
    }
  }

  final config = DisplayConfig();
  final HostRegistry registry;
  late final net = NetController(registry: registry, config: config);

  Widget app() => RtmApp(
    config: config,
    registry: registry,
    net: net,
    manageWakelock: false,
  );
}

/// Sets the logical screen size (dpr 3, like a typical phone).
void setScreen(WidgetTester tester, double w, double h) {
  tester.view.devicePixelRatio = 3;
  tester.view.physicalSize = Size(w * 3, h * 3);
  addTearDown(tester.view.reset);
}
