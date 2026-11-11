import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/config/display_config.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/net/multicast_lock.dart';
import 'package:rtm_viewer/net/net_controller.dart';
import 'package:rtm_viewer/net/packet_intake.dart';
import 'package:rtm_viewer/net/tcp_listener.dart';
import 'package:rtm_viewer/net/udp_listener.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'fixtures_util.dart';

final loopback = InternetAddress.loopbackIPv4;

/// Compact one-line JSON of a fixture (fixtures are pretty-printed).
String line(String fixture, [void Function(Map<String, dynamic>)? edit]) {
  final j = fixtureJson(fixture);
  edit?.call(j);
  return jsonEncode(j);
}

Future<void> sendUdp(int port, String text) async {
  final s = await RawDatagramSocket.bind(loopback, 0);
  s.send(utf8.encode(text), loopback, port);
  s.close();
}

/// Polls [cond] until true or times out.
Future<void> eventually(bool Function() cond, {String? reason}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 3));
  while (!cond()) {
    if (DateTime.now().isAfter(deadline)) fail('timed out: ${reason ?? ''}');
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
}

Future<int> freeTcpPort() async {
  final s = await ServerSocket.bind(loopback, 0);
  final p = s.port;
  await s.close();
  return p;
}

Future<int> freeUdpPort() async {
  final s = await RawDatagramSocket.bind(loopback, 0);
  final p = s.port;
  s.close();
  return p;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late HostRegistry registry;
  late String secret;
  late PacketIntake intake;

  setUp(() {
    registry = HostRegistry();
    secret = '';
    intake = PacketIntake(registry: registry, secret: () => secret);
  });

  group('UdpListener', () {
    late UdpListener udp;
    setUp(() async {
      udp = UdpListener(intake: intake, port: 0, address: loopback);
      await udp.start();
    });
    tearDown(() => udp.stop());

    test('datagram fixture lands in registry', () async {
      await sendUdp(udp.boundPort!, line('valid_full.json'));
      await eventually(() => registry.hosts.isNotEmpty);
      final h = registry.hosts['0b2d4a4e-6a2c-4c0e-8f2d-2f6b2b6f9a11']!;
      expect(h.lastSource, matches(RegExp(r'^127\.0\.0\.1:\d+ udp$')));
      expect(h.latest.metrics, hasLength(6));
    });

    test(
      'bad JSON, bad UTF-8 and wrong version counted, listener survives',
      () async {
        final port = udp.boundPort!;
        await sendUdp(port, '{not json');
        final s = await RawDatagramSocket.bind(loopback, 0);
        s.send([0xff, 0xfe, 0x7b], loopback, port);
        s.close();
        await sendUdp(port, line('invalid_version.json'));
        await eventually(() => intake.errorCount == 3);
        expect(intake.parseErrors, 2);
        expect(intake.incompatible, 1);
        expect(intake.lastError, contains('version'));
        expect(intake.lastIncompatibleSource, endsWith(' udp'));
        await sendUdp(port, line('valid_minimal.json'));
        await eventually(() => registry.hosts.containsKey('min'));
      },
    );

    test('secret mismatch drops packet; correct auth accepted', () async {
      secret = 'correct horse';
      final port = udp.boundPort!;
      // No auth at all, then a signed packet whose body was tampered after signing.
      await sendUdp(port, line('valid_minimal.json'));
      await sendUdp(
        port,
        line('valid_with_auth.json', (j) => j['host'] = 'evil'),
      );
      await eventually(() => intake.authFailures == 2);
      expect(registry.hosts, isEmpty);
      // The golden file as shipped is correctly signed with 'correct horse'.
      await sendUdp(port, line('valid_with_auth.json'));
      await eventually(() => registry.hosts.containsKey('auth-box'));
      expect(intake.accepted, 1);
    });
  });

  group('TcpListener', () {
    late TcpListener tcp;
    late int changes;
    setUp(() async {
      changes = 0;
      tcp = TcpListener(
        intake: intake,
        port: 0,
        address: loopback,
        onConnectionsChanged: () => changes++,
      );
      await tcp.start();
    });
    tearDown(() => tcp.stop());

    test('two lines on one connection are both ingested', () async {
      final sock = await Socket.connect(loopback, tcp.boundPort!);
      await eventually(() => tcp.connections == 1);
      sock.write(
        '${line('valid_full.json')}\n${line('valid_null_gpu.json')}\n',
      );
      await sock.flush();
      await eventually(() => registry.hosts.length == 2);
      expect(registry.hosts['intel-box']!.lastSource, endsWith(' tcp'));
      await sock.close();
      await eventually(() => tcp.connections == 0);
      expect(changes, 2);
    });

    test('line split across writes, CRLF, blank lines', () async {
      final sock = await Socket.connect(loopback, tcp.boundPort!);
      final l = line('valid_minimal.json');
      sock.write(l.substring(0, 10));
      await sock.flush();
      await Future<void>.delayed(const Duration(milliseconds: 20));
      sock.write('${l.substring(10)}\r\n\n');
      await sock.flush();
      await eventually(() => registry.hosts.containsKey('min'));
      expect(intake.errorCount, 0);
      sock.destroy();
    });

    test('bad JSON counted and connection keeps working', () async {
      final sock = await Socket.connect(loopback, tcp.boundPort!);
      sock.write('garbage\n${line('valid_minimal.json')}\n');
      await sock.flush();
      await eventually(() => registry.hosts.containsKey('min'));
      expect(intake.parseErrors, 1);
      expect(intake.lastError, contains('tcp'));
      sock.destroy();
    });

    test('stop closes open connections', () async {
      final sock = await Socket.connect(loopback, tcp.boundPort!);
      await eventually(() => tcp.connections == 1);
      final done = Completer<void>();
      sock.listen(
        (_) {},
        onDone: done.complete,
        onError: (_) => done.complete(),
      );
      await tcp.stop();
      expect(tcp.connections, 0);
      await done.future.timeout(const Duration(seconds: 3));
    });
  });

  group('NetController', () {
    late DisplayConfig config;
    late NetController net;

    setUp(() async {
      SharedPreferences.setMockInitialValues({});
      config = await DisplayConfig.load();
      config
        ..udpPort = await freeUdpPort()
        ..tcpPort = await freeTcpPort();
      net = NetController(
        registry: registry,
        config: config,
        bindAddress: loopback,
      );
    });
    tearDown(() => net.dispose());

    test('start binds both, ingests, adds slots for new metric keys', () async {
      await net.start();
      expect(net.running, isTrue);
      expect(net.udpListening, isTrue);
      expect(net.tcpListening, isTrue);
      expect(net.lastError, isNull);
      await sendUdp(config.udpPort, line('valid_unknown_fields.json'));
      await eventually(() => registry.hosts.containsKey('custom'));
      expect(config.slotFor('fan_rpm')!.style, TileStyle.gauge);
      expect(config.slotFor('battery')!.style, TileStyle.gauge);
      expect(config.slots.map((s) => s.key), [
        'cpu', 'gpu', 'mem', 'disk', 'fan_rpm', 'battery', //
      ]);
      await net.stop();
      expect(net.running, isFalse);
    });

    test(
      'port in use sets lastError, still notifies, UDP still works',
      () async {
        final blocker = await ServerSocket.bind(loopback, config.tcpPort);
        addTearDown(blocker.close);
        var notified = 0;
        net.addListener(() => notified++);
        await net.start();
        expect(net.lastError, contains('TCP ${config.tcpPort}'));
        expect(net.lastError, contains('in use'));
        expect(net.udpListening, isTrue);
        expect(net.tcpListening, isFalse);
        expect(notified, greaterThan(0));
      },
    );

    test('config change restarts listeners (debounced)', () async {
      await net.start();
      expect(net.tcpListening, isTrue);
      config.tcpEnabled = false;
      expect(net.tcpListening, isTrue, reason: 'debounced');
      await eventually(() => !net.tcpListening, reason: 'tcp stopped');
      expect(net.udpListening, isTrue);
      final newPort = await freeUdpPort();
      config.udpPort = newPort;
      await eventually(() => net.boundUdpPort == newPort, reason: 'udp moved');
      await sendUdp(newPort, line('valid_minimal.json'));
      await eventually(() => registry.hosts.containsKey('min'));
    });

    test('shared secret applies without restart', () async {
      await net.start();
      config.sharedSecret = 'correct horse';
      await sendUdp(config.udpPort, line('valid_minimal.json'));
      await eventually(() => net.authFailures == 1);
      expect(registry.hosts, isEmpty);
    });

    test('NetController acquires and releases locks via the channel', () async {
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      final calls = <String>[];
      messenger.setMockMethodCallHandler(MulticastLock.channel, (call) async {
        calls.add(call.method);
        return true;
      });
      addTearDown(
        () => messenger.setMockMethodCallHandler(MulticastLock.channel, null),
      );
      await net.start();
      expect(net.multicastLockHeld, isTrue);
      expect(net.wifiLockHeld, isTrue);
      await net.stop();
      expect(net.multicastLockHeld, isFalse);
      expect(net.wifiLockHeld, isFalse);
      expect(calls, ['acquire', 'isWifiLockHeld', 'release']);
    });

    test(
      'multicastLockHeld is false in tests; start/stop still work',
      () async {
        expect(net.multicastLockHeld, isFalse);
        await net.start();
        expect(net.udpListening, isTrue);
        expect(net.multicastLockHeld, isFalse);
        expect(net.wifiLockHeld, isFalse);
        await net.stop();
        expect(net.running, isFalse);
        expect(net.multicastLockHeld, isFalse);
      },
    );

    test('dispose during start closes what start opened', () async {
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      final calls = <String>[];
      messenger.setMockMethodCallHandler(MulticastLock.channel, (call) async {
        calls.add(call.method);
        // UDP is bound by now; dispose while start is awaiting the lock.
        if (call.method == 'acquire') net.dispose();
        return true;
      });
      addTearDown(
        () => messenger.setMockMethodCallHandler(MulticastLock.channel, null),
      );
      await net.start();
      expect(net.running, isFalse);
      expect(net.multicastLockHeld, isFalse);
      expect(calls, contains('release'));
      // Both ports are free again (TCP was never bound).
      final u = await RawDatagramSocket.bind(loopback, config.udpPort);
      u.close();
      final t = await ServerSocket.bind(loopback, config.tcpPort);
      await t.close();
    });

    test('start re-evaluates host staleness immediately', () async {
      var now = DateTime.now();
      final reg = HostRegistry(now: () => now);
      final c = NetController(
        registry: reg,
        config: config,
        bindAddress: loopback,
      );
      addTearDown(c.dispose);
      expect(
        PacketIntake(
          registry: reg,
          secret: () => '',
        ).handleText(line('valid_minimal.json'), 'x'),
        isTrue,
      );
      expect(reg.hosts['min']!.status, HostStatus.live);
      // As after a long stay in the background.
      now = now.add(const Duration(minutes: 10));
      await c.start();
      expect(reg.hosts['min']!.status, HostStatus.offline);
    });

    test('timer ticks registry and refreshes packetsPerSecond', () async {
      await net.start();
      // Stream at ~5 Hz so the 1 s timer sees a non-zero rate regardless of scheduling.
      var seq = 0;
      final sender = Timer.periodic(const Duration(milliseconds: 200), (_) {
        seq++;
        sendUdp(
          config.udpPort,
          line('valid_minimal.json', (j) => j['seq'] = seq),
        );
      });
      addTearDown(sender.cancel);
      await eventually(() => registry.hosts.isNotEmpty);
      await eventually(
        () => net.packetsPerSecond >= 2,
        reason: 'pps refreshed',
      );
      expect(net.hostCount, 1);
    });
  });
}
