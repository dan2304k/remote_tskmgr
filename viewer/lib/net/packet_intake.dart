// Shared parse path for UDP datagrams and TCP lines. Agent viewer-core owns this file.
import 'dart:convert';

import '../core/host_registry.dart';
import '../core/packet.dart';

/// Decodes, parses, authenticates and ingests packets, and keeps the diagnostic counters.
/// One instance is shared by the UDP and TCP listeners of a [NetController].
class PacketIntake {
  PacketIntake({
    required this.registry,
    required this.secret,
    this.onAccepted,
    DateTime Function()? clock,
    this.errorTtl = const Duration(seconds: 30),
  }) : _clock = clock ?? registry.clock;

  final HostRegistry registry;

  /// Current shared secret; read per packet so a change applies without a restart.
  /// Empty = no authentication required.
  final String Function() secret;

  /// Called after the registry accepted a packet (NetController uses it to add slots).
  final void Function(Packet packet)? onAccepted;

  /// Packets that were not valid UTF-8 / JSON / protocol structure (version errors are
  /// counted in [incompatible] instead).
  int parseErrors = 0;

  /// Packets dropped because a secret is configured and `auth` was missing or wrong.
  int authFailures = 0;

  /// Packets with a protocol version other than 1.
  int incompatible = 0;

  /// Packets handed to the registry and accepted.
  int accepted = 0;

  final DateTime Function() _clock;

  /// How long [lastError] stays visible after the rejection that set it.
  final Duration errorTtl;

  String? _lastError;
  DateTime? _lastErrorAt;

  /// Human-readable description of the most recent rejected packet, with its source.
  /// Expires (reads null) [errorTtl] after the rejection, so one stray datagram does
  /// not leave an error showing forever.
  String? get lastError {
    final at = _lastErrorAt;
    if (_lastError == null || at == null) return null;
    if (_clock().difference(at) >= errorTtl) return null;
    return _lastError;
  }

  /// Time since [lastError] was recorded; null when there is no (unexpired) error.
  Duration? get lastErrorAge =>
      lastError == null ? null : _clock().difference(_lastErrorAt!);

  set lastError(String? message) {
    _lastError = message;
    _lastErrorAt = message == null ? null : _clock();
  }

  /// Source ("ip:port udp") of the last packet with an unsupported version, for the
  /// "incompatible sender" hint. Null if none seen.
  String? lastIncompatibleSource;

  /// Total rejected packets (parse + auth + version).
  int get errorCount => parseErrors + authFailures + incompatible;

  /// Handles raw datagram bytes. Returns true if the packet was ingested.
  bool handleBytes(List<int> bytes, String source) =>
      ingestBytes(bytes, source) != null;

  /// Like [handleBytes], returning the ingested packet (null when rejected).
  Packet? ingestBytes(List<int> bytes, String source) {
    final String text;
    try {
      text = utf8.decode(bytes);
    } on FormatException catch (e) {
      parseErrors++;
      lastError = '$source: not valid UTF-8 (${e.message})';
      return null;
    }
    return ingestText(text, source);
  }

  /// Handles one JSON document. Returns true if the packet was ingested. Never throws:
  /// this runs inside socket callbacks, where an escaped error would kill the listener.
  bool handleText(String text, String source) =>
      ingestText(text, source) != null;

  /// Like [handleText], returning the ingested packet (null when rejected).
  Packet? ingestText(String text, String source) {
    try {
      return _handleText(text, source);
    } catch (e) {
      parseErrors++;
      lastError = '$source: rejected packet ($e)';
      return null;
    }
  }

  Packet? _handleText(String text, String source) {
    final Packet packet;
    try {
      packet = Packet.parse(text);
    } on UnsupportedVersionException catch (e) {
      incompatible++;
      lastIncompatibleSource = source;
      lastError = '$source: ${e.message}';
      return null;
    } on FormatException catch (e) {
      parseErrors++;
      lastError = '$source: ${e.message}';
      return null;
    }
    final key = secret();
    if (key.isNotEmpty) {
      final bad = authUnsafeNumber(text);
      if (bad != null) {
        authFailures++;
        lastError =
            '$source: packet from "${packet.host}" cannot be verified: $bad';
        return null;
      }
    }
    if (key.isNotEmpty && !packet.verifyAuth(key)) {
      authFailures++;
      lastError = packet.auth == null
          ? '$source: packet from "${packet.host}" has no auth but a shared secret is set'
          : '$source: auth mismatch for "${packet.host}" (check the shared secret)';
      return null;
    }
    if (!registry.ingest(packet, source: source)) return null;
    accepted++;
    onAccepted?.call(packet);
    return packet;
  }
}
