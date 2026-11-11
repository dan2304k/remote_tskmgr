// Shared parse path for UDP datagrams and TCP lines. Agent viewer-core owns this file.
import 'dart:convert';

import '../core/host_registry.dart';
import '../core/packet.dart';

/// Decodes, parses, authenticates and ingests packets, and keeps the diagnostic counters.
/// One instance is shared by the UDP and TCP listeners of a [NetController].
class PacketIntake {
  PacketIntake({required this.registry, required this.secret, this.onAccepted});

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

  /// Human-readable description of the most recent rejected packet, with its source.
  String? lastError;

  /// Source ("ip:port udp") of the last packet with an unsupported version, for the
  /// "incompatible sender" hint. Null if none seen.
  String? lastIncompatibleSource;

  /// Total rejected packets (parse + auth + version).
  int get errorCount => parseErrors + authFailures + incompatible;

  /// Handles raw datagram bytes. Returns true if the packet was ingested.
  bool handleBytes(List<int> bytes, String source) {
    final String text;
    try {
      text = utf8.decode(bytes);
    } on FormatException catch (e) {
      parseErrors++;
      lastError = '$source: not valid UTF-8 (${e.message})';
      return false;
    }
    return handleText(text, source);
  }

  /// Handles one JSON document. Returns true if the packet was ingested.
  bool handleText(String text, String source) {
    final Packet packet;
    try {
      packet = Packet.parse(text);
    } on UnsupportedVersionException catch (e) {
      incompatible++;
      lastIncompatibleSource = source;
      lastError = '$source: ${e.message}';
      return false;
    } on FormatException catch (e) {
      parseErrors++;
      lastError = '$source: ${e.message}';
      return false;
    }
    final key = secret();
    if (key.isNotEmpty && !packet.verifyAuth(key)) {
      authFailures++;
      lastError = packet.auth == null
          ? '$source: packet from "${packet.host}" has no auth but a shared secret is set'
          : '$source: auth mismatch for "${packet.host}" (check the shared secret)';
      return false;
    }
    if (!registry.ingest(packet, source: source)) return false;
    accepted++;
    onAccepted?.call(packet);
    return true;
  }
}
