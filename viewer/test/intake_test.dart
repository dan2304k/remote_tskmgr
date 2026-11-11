import 'dart:convert';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/core/host_registry.dart';
import 'package:rtm_viewer/core/packet.dart';
import 'package:rtm_viewer/net/packet_intake.dart';

import 'fixtures_util.dart';

/// valid_full.json as one line with the cpu value replaced by a raw JSON literal.
String withCpuLiteral(String literal) {
  final j = fixtureJson('valid_full.json');
  (j['metrics'] as Map)['cpu']['v'] = 12345.5;
  return jsonEncode(j).replaceFirst('12345.5', literal);
}

void main() {
  var t = DateTime.utc(2026, 11, 11, 12);
  late HostRegistry registry;
  late PacketIntake intake;

  setUp(() {
    t = DateTime.utc(2026, 11, 11, 12);
    registry = HostRegistry(now: () => t);
    intake = PacketIntake(registry: registry, secret: () => '');
  });

  group('lastError expiry', () {
    test('expires after errorTtl (30 s) and reports its age', () {
      expect(intake.handleText('{not json', '1.2.3.4:5 udp'), isFalse);
      expect(intake.lastError, contains('1.2.3.4:5 udp'));
      expect(intake.lastErrorAge, Duration.zero);
      t = t.add(const Duration(seconds: 29));
      expect(intake.lastError, isNotNull);
      expect(intake.lastErrorAge, const Duration(seconds: 29));
      t = t.add(const Duration(seconds: 1));
      expect(intake.lastError, isNull);
      expect(intake.lastErrorAge, isNull);
      expect(intake.parseErrors, 1, reason: 'counters do not expire');
    });

    test('a new rejection restarts the clock', () {
      intake.handleText('{not json', 'a');
      t = t.add(const Duration(seconds: 25));
      intake.handleText('[]', 'b');
      t = t.add(const Duration(seconds: 25));
      expect(intake.lastError, startsWith('b:'));
      expect(intake.lastErrorAge, const Duration(seconds: 25));
    });

    test('custom ttl', () {
      intake = PacketIntake(
        registry: registry,
        secret: () => '',
        errorTtl: const Duration(seconds: 5),
      );
      intake.handleText('{not json', 'a');
      t = t.add(const Duration(seconds: 5));
      expect(intake.lastError, isNull);
    });
  });

  group('non-finite numbers', () {
    test('sanity: the literal helper produces a parseable packet', () {
      expect(Packet.parse(withCpuLiteral('12.5')).metrics['cpu']!.v, 12.5);
    });

    for (final lit in ['1e400', '-1e400']) {
      test('Packet.parse rejects $lit with a FormatException', () {
        expect(
          () => Packet.parse(withCpuLiteral(lit)),
          throwsA(
            isA<FormatException>().having(
              (e) => e.message,
              'message',
              contains('non-finite'),
            ),
          ),
        );
      });
    }

    test('non-finite inside detail / unknown fields is rejected too', () {
      final j = fixtureJson('valid_full.json');
      final text = jsonEncode({
        ...j,
        'extra': [1, 7777.25],
      }).replaceFirst('7777.25', '1e999');
      expect(() => Packet.parse(text), throwsFormatException);
    });

    test('handleText counts it as a parse error and does not throw', () {
      expect(intake.handleText(withCpuLiteral('1e400'), 'x'), isFalse);
      expect(intake.parseErrors, 1);
      expect(intake.lastError, contains('non-finite'));
      expect(registry.hosts, isEmpty);
    });

    test('handleText does not throw with a secret set either', () {
      intake = PacketIntake(registry: registry, secret: () => 'k');
      final j = fixtureJson('valid_full.json');
      j['auth'] = '00';
      (j['metrics'] as Map)['cpu']['v'] = 12345.5;
      final text = jsonEncode(j).replaceFirst('12345.5', '1e400');
      expect(intake.handleText(text, 'x'), isFalse);
      expect(intake.errorCount, 1);
    });
  });

  test('handleText catches any error (here from onAccepted)', () {
    intake = PacketIntake(
      registry: registry,
      secret: () => '',
      onAccepted: (_) => throw StateError('boom'),
    );
    expect(
      intake.handleText(jsonEncode(fixtureJson('valid_minimal.json')), 'x'),
      isFalse,
    );
    expect(intake.lastError, contains('boom'));
  });

  group('auth: numbers the canonical body cannot reproduce', () {
    const key = 'correct horse';

    /// valid_full.json signed the way a correct sender would sign it, with the cpu
    /// value spelled [literal] on the wire and in the canonical body.
    String signedWith(String literal, {int seq = 1}) {
      final j = fixtureJson('valid_full.json')..remove('auth');
      j['seq'] = seq;
      (j['metrics'] as Map)['cpu']['v'] = 12345.5;
      final body = Packet.canonicalBody(j).replaceFirst('12345.5', literal);
      final mac = Hmac(sha256, utf8.encode(key)).convert(utf8.encode(body));
      j['auth'] = mac.toString();
      return jsonEncode(j).replaceFirst('12345.5', literal);
    }

    late PacketIntake authed;
    setUp(() => authed = PacketIntake(registry: registry, secret: () => key));

    test('control: an ordinary signed value verifies', () {
      expect(authed.handleText(signedWith('42'), 's'), isTrue);
      expect(
        authed.handleText(signedWith('-9223372036854775808', seq: 2), 's'),
        isTrue,
      );
    });

    for (final (literal, why) in [
      ('-0', '-0'),
      ('9223372036854775808', 'outside the 64-bit range'),
      ('-9223372036854775809', 'outside the 64-bit range'),
      ('18446744073709551615', 'outside the 64-bit range'),
    ]) {
      test('$literal is rejected with a reason when a secret is set', () {
        expect(authed.handleText(signedWith(literal), 's'), isFalse);
        expect(authed.authFailures, 1);
        expect(authed.lastError, contains(why));
        expect(registry.hosts, isEmpty);
      });

      test('$literal is accepted when no secret is set', () {
        expect(intake.handleText(withCpuLiteral(literal), 's'), isTrue);
      });
    }

    test('non-finite is rejected as a parse error, secret or not', () {
      expect(authed.handleText(signedWith('1e400'), 's'), isFalse);
      expect(authed.parseErrors, 1);
      expect(authed.authFailures, 0);
    });

    test('scanner ignores numbers inside strings; floats are fine', () {
      expect(authUnsafeNumber('{"a":"-0","b":"99999999999999999999"}'), isNull);
      expect(authUnsafeNumber(r'{"a":"x\"-0","b":-0.0,"c":-0e0}'), isNull);
      expect(authUnsafeNumber('{"a":[1,-0]}'), 'integer -0');
      expect(authUnsafeNumber('{"a":-0}'), 'integer -0');
      expect(authUnsafeNumber('{"a":9223372036854775807}'), isNull);
      expect(authUnsafeNumber('{"a":12345678901234567890.5}'), isNull);
    });
  });
}
