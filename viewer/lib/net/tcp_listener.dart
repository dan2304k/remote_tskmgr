// TCP listener (newline-delimited JSON from senders that connect to us).
// Agent viewer-core owns this file.
import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import '../core/host_registry.dart' show clampIntervalMs;
import '../core/packet.dart' show kMaxPacketBytes;
import 'packet_intake.dart';

/// Longest line (one packet) buffered before the connection is dropped.
const int kTcpMaxLineBytes = kMaxPacketBytes;

/// Floor of the idle timeout; see [tcpIdleTimeout].
const Duration kTcpMinIdleTimeout = Duration(seconds: 15);

/// Ceiling of the idle timeout, whatever interval the peer claims.
const Duration kTcpMaxIdleTimeout = Duration(seconds: 120);

/// Concurrent sender connections in total; see [TcpListener] for what happens when full.
const int kTcpMaxConnections = 32;

/// Concurrent connections from one remote IP address.
const int kTcpMaxConnectionsPerIp = 4;

/// How long a connection may go without a complete line: max([minimum], 5 x the
/// sender's interval, clamped by [clampIntervalMs]) once its interval is known, else
/// [minimum]; never more than [kTcpMaxIdleTimeout] (unless [minimum] is larger).
Duration tcpIdleTimeout(Duration minimum, int? intervalMs) {
  if (intervalMs == null || intervalMs <= 0) return minimum;
  var t = Duration(milliseconds: clampIntervalMs(intervalMs) * 5);
  if (t > kTcpMaxIdleTimeout) t = kTcpMaxIdleTimeout;
  return t > minimum ? t : minimum;
}

/// Accepts sender connections; each `\n`-terminated line is one packet fed to [intake].
///
/// Hardened against misbehaving peers: a line longer than [maxLineBytes] drops the
/// connection, a connection without a complete line for [tcpIdleTimeout] is closed,
/// one remote IP may hold at most [maxConnectionsPerIp], and at most [maxConnections]
/// are open at once. When either limit is hit the connection that has gone longest
/// without a complete line (from that IP, or overall) is closed to make room, so idle
/// squatters cannot lock out a real sender.
class TcpListener {
  TcpListener({
    required this.intake,
    required this.port,
    InternetAddress? address,
    this.onConnectionsChanged,
    this.maxLineBytes = kTcpMaxLineBytes,
    this.minIdleTimeout = kTcpMinIdleTimeout,
    this.maxConnections = kTcpMaxConnections,
    this.maxConnectionsPerIp = kTcpMaxConnectionsPerIp,
  }) : address = address ?? InternetAddress.anyIPv4;

  final PacketIntake intake;

  /// Port to bind; 0 picks a free one (tests). See [boundPort].
  final int port;
  final InternetAddress address;

  /// Called whenever [connections] changes.
  final void Function()? onConnectionsChanged;

  final int maxLineBytes;
  final Duration minIdleTimeout;
  final int maxConnections;
  final int maxConnectionsPerIp;

  /// Connections closed to make room for a newcomer because [maxConnections] or
  /// [maxConnectionsPerIp] was reached (diagnostics).
  int rejectedConnections = 0;

  ServerSocket? _server;
  StreamSubscription<Socket>? _serverSub;
  final Map<Socket, _Conn> _sockets = {};

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

  /// Closes the open connection (optionally only those from [ip]) that has gone
  /// longest without a complete line.
  void _evictIdlest({String? ip}) {
    _Conn? idlest;
    for (final c in _sockets.values) {
      if (ip != null && c.ip != ip) continue;
      if (idlest == null || c.lastLine < idlest.lastLine) idlest = c;
    }
    if (idlest == null) return;
    rejectedConnections++;
    intake.lastError =
        '${idlest.source}: closed the idlest connection to make room '
        '(${ip == null ? '$maxConnections open' : '$maxConnectionsPerIp from $ip'})';
    idlest.close();
  }

  void _onConnection(Socket socket) {
    final String ip;
    final String source;
    try {
      ip = socket.remoteAddress.address;
      source = '$ip:${socket.remotePort} tcp';
    } on SocketException {
      socket.destroy();
      return;
    }
    if (maxConnections <= 0 || maxConnectionsPerIp <= 0) {
      socket.destroy();
      return;
    }
    while (_sockets.values.where((c) => c.ip == ip).length >=
        maxConnectionsPerIp) {
      _evictIdlest(ip: ip);
    }
    while (_sockets.length >= maxConnections) {
      _evictIdlest();
    }
    var closed = false;
    Timer? idle;
    int? intervalMs;
    StreamSubscription<Uint8List>? sub;
    final pending = BytesBuilder(copy: true);

    void close() {
      if (closed) return;
      closed = true;
      idle?.cancel();
      unawaited(sub?.cancel());
      _sockets.remove(socket);
      socket.destroy();
      onConnectionsChanged?.call();
    }

    final conn = _Conn(ip, source, close, _clock.elapsedMicroseconds);

    void armIdle() {
      conn.lastLine = _clock.elapsedMicroseconds;
      idle?.cancel();
      idle = Timer(tcpIdleTimeout(minIdleTimeout, intervalMs), () {
        intake.lastError = '$source: no complete line in time, closed';
        close();
      });
    }

    void line(Uint8List bytes) {
      armIdle();
      var end = bytes.length;
      if (end > 0 && bytes[end - 1] == 0x0D) end--; // CRLF
      var blank = true;
      for (var k = 0; k < end; k++) {
        final b = bytes[k];
        if (b != 0x20 && b != 0x09 && b != 0x0D) {
          blank = false;
          break;
        }
      }
      if (blank) return;
      final p = intake.ingestBytes(
        Uint8List.sublistView(bytes, 0, end),
        source,
      );
      if (p != null) {
        intervalMs = p.intervalMs;
        armIdle();
      }
    }

    void tooLong() {
      intake.parseErrors++;
      intake.lastError =
          '$source: line longer than $maxLineBytes bytes, connection dropped';
      close();
    }

    _sockets[socket] = conn;
    onConnectionsChanged?.call();
    armIdle();
    sub = socket.listen(
      (data) {
        var start = 0;
        while (!closed) {
          final nl = data.indexOf(0x0A, start);
          if (nl < 0) break;
          final Uint8List bytes;
          if (pending.isEmpty) {
            bytes = Uint8List.sublistView(data, start, nl);
          } else {
            pending.add(Uint8List.sublistView(data, start, nl));
            bytes = pending.takeBytes();
          }
          start = nl + 1;
          if (bytes.length > maxLineBytes) return tooLong();
          line(bytes);
        }
        if (closed || start >= data.length) return;
        if (pending.length + data.length - start > maxLineBytes) {
          return tooLong();
        }
        pending.add(Uint8List.sublistView(data, start));
      },
      onError: (Object e) {
        // A socket error: the stream state is unreliable, so drop the connection; the
        // sender reconnects with backoff.
        intake.lastError = '$source: connection error ($e)';
        close();
      },
      onDone: () {
        // A final line without '\n' still counts.
        if (!closed && pending.isNotEmpty) line(pending.takeBytes());
        close();
      },
      cancelOnError: true,
    );
  }

  Future<void> stop() async {
    await _serverSub?.cancel();
    _serverSub = null;
    await _server?.close();
    _server = null;
    final open = _sockets.values.toList();
    for (final c in open) {
      c.close();
    }
  }

  /// Monotonic clock for [_Conn.lastLine].
  final Stopwatch _clock = Stopwatch()..start();
}

class _Conn {
  _Conn(this.ip, this.source, this.close, this.lastLine);
  final String ip;
  final String source;
  final void Function() close;

  /// [TcpListener._clock] microseconds at the last complete line (or the connect).
  int lastLine;
}
