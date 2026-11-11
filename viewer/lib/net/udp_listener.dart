// UDP listener (broadcast + unicast datagrams). Agent viewer-core owns this file.
import 'dart:async';
import 'dart:io';

import 'packet_intake.dart';

/// Listens for one-JSON-object-per-datagram packets and feeds them to [intake].
class UdpListener {
  UdpListener({
    required this.intake,
    required this.port,
    InternetAddress? address,
  }) : address = address ?? InternetAddress.anyIPv4;

  final PacketIntake intake;

  /// Port to bind; 0 picks a free one (tests). See [boundPort].
  final int port;
  final InternetAddress address;

  RawDatagramSocket? _socket;
  StreamSubscription<RawSocketEvent>? _sub;

  bool get isListening => _socket != null;

  /// The actual bound port, or null when not listening.
  int? get boundPort => _socket?.port;

  /// Binds the socket. Throws [SocketException] (e.g. port in use).
  ///
  /// reuseAddress only: reusePort is unsupported on Android (Android limitation).
  Future<void> start() async {
    if (_socket != null) return;
    final socket = await RawDatagramSocket.bind(
      address,
      port,
      reuseAddress: true,
    );
    socket.broadcastEnabled = true;
    _socket = socket;
    _sub = socket.listen((event) {
      if (event != RawSocketEvent.read) return;
      Datagram? d;
      while ((d = socket.receive()) != null) {
        final dg = d!;
        intake.handleBytes(dg.data, '${dg.address.address}:${dg.port} udp');
      }
    }, onError: (Object e) => intake.lastError = 'UDP socket error: $e');
  }

  Future<void> stop() async {
    await _sub?.cancel();
    _sub = null;
    _socket?.close();
    _socket = null;
  }
}
