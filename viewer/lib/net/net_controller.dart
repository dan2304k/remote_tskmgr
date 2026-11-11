// Owns the listeners and the staleness timer. Agent viewer-core owns this file.
import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../config/display_config.dart';
import '../core/host_registry.dart';
import '../core/packet.dart';
import 'multicast_lock.dart';
import 'packet_intake.dart';
import 'tcp_listener.dart';
import 'udp_listener.dart';

/// Owns the UDP and TCP listeners, feeds [HostRegistry], runs the staleness timer.
///
/// Restarts itself (debounced 300 ms) when [config]'s udpPort / tcpEnabled / tcpPort
/// change. The shared secret is read per packet, so changing it applies immediately
/// without a restart. Notifies on start/stop, bind errors, connection count changes and,
/// once per second, when the diagnostic numbers changed.
class NetController extends ChangeNotifier {
  NetController({
    required this.registry,
    required this.config,
    InternetAddress? bindAddress,
    this.restartDebounce = const Duration(milliseconds: 300),
  }) : bindAddress = bindAddress ?? InternetAddress.anyIPv4 {
    _intake = PacketIntake(
      registry: registry,
      secret: () => config.sharedSecret,
      onAccepted: _onAccepted,
    );
    _netSnapshot = _snapshot();
    config.addListener(_onConfigChanged);
  }

  final HostRegistry registry;
  final DisplayConfig config;

  /// Interface address to bind (anyIPv4 in the app; loopback in tests).
  final InternetAddress bindAddress;
  final Duration restartDebounce;

  late final PacketIntake _intake;
  UdpListener? _udp;
  TcpListener? _tcp;
  Timer? _ticker;
  Timer? _debounce;
  String? _bindError;
  late String _netSnapshot;
  bool _disposed = false;
  Future<void> _pending = Future.value();
  String _diagSnapshot = '';
  int _packetsPerSecond = 0;
  bool _multicastLockHeld = false;
  bool _wifiLockHeld = false;

  /// True while at least one listener is bound.
  bool get running =>
      (_udp?.isListening ?? false) || (_tcp?.isListening ?? false);
  bool get udpListening => _udp?.isListening ?? false;
  bool get tcpListening => _tcp?.isListening ?? false;

  /// Whether the Android Wi-Fi multicast lock is held (needed for broadcast receive on
  /// many devices). Always false off Android and in tests.
  bool get multicastLockHeld => _multicastLockHeld;

  /// Whether the Android low-latency Wi-Fi lock is held (keeps the radio out of power
  /// save so broadcasts are not batched to DTIM beacons). Always false off Android.
  bool get wifiLockHeld => _wifiLockHeld;

  /// Bind failure (e.g. port in use) if any, otherwise the last rejected-packet reason.
  String? get lastError => _bindError ?? _intake.lastError;

  /// Only the bind failure part of [lastError].
  String? get bindError => _bindError;

  int get udpPort => config.udpPort;
  int get tcpPort => config.tcpPort;

  /// Actually bound ports (differ from the config only when it asks for port 0).
  int? get boundUdpPort => _udp?.boundPort;
  int? get boundTcpPort => _tcp?.boundPort;

  int get tcpConnections => _tcp?.connections ?? 0;

  /// Accepted packets per second (5 s average, see [HostRegistry.packetRate]), refreshed
  /// by the 1 s timer.
  int get packetsPerSecond => _packetsPerSecond;
  int get hostCount => registry.hosts.length;

  /// Rejected-packet counters.
  int get parseErrors => _intake.parseErrors;
  int get authFailures => _intake.authFailures;
  int get incompatiblePackets => _intake.incompatible;
  int get errorCount => _intake.errorCount;
  int get acceptedPackets => _intake.accepted;

  /// Source of the last packet with an unsupported protocol version, for the
  /// "incompatible sender" hint; null if none seen.
  String? get lastIncompatibleSource => _intake.lastIncompatibleSource;

  /// Binds UDP always and TCP when enabled. Never throws for bind failures: they land in
  /// [lastError] and the controller still notifies.
  Future<void> start() => _serial(_start);
  Future<void> stop() => _serial(_stop);

  /// Stop + start with the current [config] ports.
  Future<void> restart() => _serial(() async {
    await _stop();
    await _start();
  });

  Future<void> _serial(Future<void> Function() op) {
    final next = _pending.then((_) => op());
    _pending = next.catchError((Object _) {});
    return next;
  }

  Future<void> _start() async {
    if (_disposed || running) return;
    final errors = <String>[];
    final udp = UdpListener(
      intake: _intake,
      port: config.udpPort,
      address: bindAddress,
    );
    try {
      await udp.start();
      _udp = udp;
      await _acquireMulticastLock();
    } on SocketException catch (e) {
      errors.add('UDP ${config.udpPort}: ${_describe(e)}');
    }
    if (config.tcpEnabled) {
      final tcp = TcpListener(
        intake: _intake,
        port: config.tcpPort,
        address: bindAddress,
        onConnectionsChanged: _notify,
      );
      try {
        await tcp.start();
        _tcp = tcp;
      } on SocketException catch (e) {
        errors.add('TCP ${config.tcpPort}: ${_describe(e)}');
      }
    }
    _bindError = errors.isEmpty
        ? null
        : 'Cannot listen on ${errors.join('; ')}';
    _netSnapshot = _snapshot();
    _ticker ??= Timer.periodic(const Duration(seconds: 1), (_) => _onTick());
    _notify();
  }

  Future<void> _stop() async {
    _ticker?.cancel();
    _ticker = null;
    final udp = _udp;
    final tcp = _tcp;
    _udp = null;
    _tcp = null;
    await udp?.stop();
    await tcp?.stop();
    if (_multicastLockHeld || _wifiLockHeld) {
      _multicastLockHeld = false;
      _wifiLockHeld = false;
      await MulticastLock.release();
    }
    _notify();
  }

  Future<void> _acquireMulticastLock() async {
    _multicastLockHeld = await MulticastLock.acquire();
    _wifiLockHeld = await MulticastLock.wifiLockHeld();
    if ((!_multicastLockHeld || !_wifiLockHeld) &&
        defaultTargetPlatform == TargetPlatform.android) {
      debugPrint(
        'NetController: multicast lock held=$_multicastLockHeld, '
        'wifi lock held=$_wifiLockHeld; UDP broadcast may be filtered or '
        'delayed (unicast still works)',
      );
    }
  }

  void _onTick() {
    registry.tick(registry.clock());
    _packetsPerSecond = registry.packetsPerSecond;
    final diag =
        '$_packetsPerSecond $hostCount ${_intake.errorCount} ${_intake.lastError} '
        '$_multicastLockHeld $_wifiLockHeld';
    if (diag != _diagSnapshot) {
      _diagSnapshot = diag;
      _notify();
    }
  }

  void _onAccepted(Packet p) {
    p.metrics.forEach(
      (key, m) => config.ensureSlot(key, isPercent: m.isPercent),
    );
  }

  String _snapshot() =>
      '${config.udpPort}|${config.tcpEnabled}|${config.tcpPort}';

  void _onConfigChanged() {
    if (_disposed || _snapshot() == _netSnapshot) return;
    _debounce?.cancel();
    _debounce = Timer(restartDebounce, () {
      if (_disposed || _snapshot() == _netSnapshot) return;
      _netSnapshot = _snapshot();
      restart();
    });
  }

  static String _describe(SocketException e) {
    final os = e.osError;
    final code = os?.errorCode;
    // EADDRINUSE: 98 Linux/Android, 48 macOS/iOS, 10048 Windows.
    if (code == 98 || code == 48 || code == 10048) return 'port already in use';
    // Same-process duplicate bind is caught by the Dart VM before the OS sees it.
    final text = os?.message.isNotEmpty == true ? os!.message : e.message;
    if (text.contains('binding multiple times')) return 'port already in use';
    return text;
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _debounce?.cancel();
    config.removeListener(_onConfigChanged);
    _ticker?.cancel();
    _ticker = null;
    unawaited(_udp?.stop());
    unawaited(_tcp?.stop());
    if (_multicastLockHeld || _wifiLockHeld) {
      _multicastLockHeld = false;
      _wifiLockHeld = false;
      unawaited(MulticastLock.release());
    }
    _udp = null;
    _tcp = null;
    super.dispose();
  }
}
