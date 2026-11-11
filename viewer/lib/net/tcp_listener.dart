// TCP listener (newline-delimited JSON from senders that connect to us).
// Agent viewer-core owns this file.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'packet_intake.dart';

/// Accepts sender connections; each `\n`-terminated line is one packet fed to [intake].
class TcpListener {
  TcpListener({
    required this.intake,
    required this.port,
    InternetAddress? address,
    this.onConnectionsChanged,
  }) : address = address ?? InternetAddress.anyIPv4;

  final PacketIntake intake;

  /// Port to bind; 0 picks a free one (tests). See [boundPort].
  final int port;
  final InternetAddress address;

  /// Called whenever [connections] changes.
  final void Function()? onConnectionsChanged;

  ServerSocket? _server;
  StreamSubscription<Socket>? _serverSub;
  final Set<Socket> _sockets = {};

  bool get isListening => _server != null;

  /// The actual bound port, or null when not listening.
  int? get boundPort => _server?.port;

  /// Number of currently open sender connections.
  int get connections => _sockets.length;

  /// Binds the server socket. Throws [SocketException] (e.g. port in use).
  Future<void> start() async {
    if (_server != null) return;
    final server = await ServerSocket.bind(address, port);
    _server = server;
    _serverSub = server.listen(
      _onConnection,
      onError: (Object e) => intake.lastError = 'TCP server error: $e',
    );
  }

  void _onConnection(Socket socket) {
    final String source;
    try {
      source = '${socket.remoteAddress.address}:${socket.remotePort} tcp';
    } on SocketException {
      socket.destroy();
      return;
    }
    _sockets.add(socket);
    onConnectionsChanged?.call();
    var closed = false;
    void close() {
      if (closed) return;
      closed = true;
      _sockets.remove(socket);
      socket.destroy();
      onConnectionsChanged?.call();
    }

    utf8.decoder
        .bind(socket)
        .transform(const LineSplitter())
        .listen(
          (line) {
            if (line.trim().isEmpty) return;
            intake.handleText(line, source);
          },
          onError: (Object e) {
            // Malformed UTF-8 or a socket error: the stream state is unreliable, so drop
            // the connection; the sender reconnects with backoff.
            if (e is FormatException) intake.parseErrors++;
            intake.lastError = '$source: connection error ($e)';
            close();
          },
          onDone: close,
          cancelOnError: true,
        );
  }

  Future<void> stop() async {
    await _serverSub?.cancel();
    _serverSub = null;
    await _server?.close();
    _server = null;
    final open = _sockets.toList();
    _sockets.clear();
    for (final s in open) {
      s.destroy();
    }
    if (open.isNotEmpty) onConnectionsChanged?.call();
  }
}
