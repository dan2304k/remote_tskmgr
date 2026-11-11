import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:rtm_viewer/core/packet.dart';

import 'fixtures_util.dart';

void main() {
  group('golden valid_*.json', () {
    final files = fixtureFiles('valid_');
    test(
      'fixtures present',
      () => expect(files, hasLength(greaterThanOrEqualTo(5))),
    );
    for (final f in files) {
      test('parses ${f.uri.pathSegments.last}', () {
        final p = Packet.parse(f.readAsStringSync());
        expect(p.v, 1);
        expect(p.id, isNotEmpty);
        expect(p.intervalMs, greaterThan(0));
      });
    }
  });

  group('golden invalid_*.json', () {
    final files = fixtureFiles('invalid_');
    test(
      'fixtures present',
      () => expect(files, hasLength(greaterThanOrEqualTo(3))),
    );
    for (final f in files) {
      test('rejects ${f.uri.pathSegments.last}', () {
        expect(() => Packet.parse(f.readAsStringSync()), throwsFormatException);
      });
    }
    test('version error mentions "version"', () {
      expect(
        () => Packet.parse(fixtureText('invalid_version.json')),
        throwsA(
          isA<UnsupportedVersionException>()
              .having((e) => e.message, 'message', contains('version'))
              .having((e) => e.version, 'version', 2),
        ),
      );
    });
  });

  test('valid_full field spot checks', () {
    final p = Packet.parse(fixtureText('valid_full.json'));
    expect(p.id, '0b2d4a4e-6a2c-4c0e-8f2d-2f6b2b6f9a11');
    expect(p.host, 'workstation');
    expect(p.os, 'linux');
    expect(p.ts, 1762640000123);
    expect(p.seq, 48213);
    expect(p.intervalMs, 1000);
    expect(p.metrics.keys.toList(), [
      'cpu',
      'gpu',
      'mem',
      'disk',
      'net',
      'temp',
    ]);
    final cpu = p.metrics['cpu']!;
    expect(cpu.v, 23.4);
    expect(cpu.unit, '%');
    expect(cpu.isPercent, isTrue);
    expect(cpu.min, 0);
    expect(cpu.max, 100);
    expect(cpu.detail!['cores'], [12.0, 44.1, 8.3, 29.0]);
    expect(cpu.detail!['freq_mhz'], 3800);
    final disk = p.metrics['disk']!;
    expect(disk.max, isNull);
    expect(disk.isPercent, isFalse);
    expect(p.metrics['temp']!.unit, '°C');
    expect(p.metrics['temp']!.detail, isNull);
    expect(p.auth, isNull);
  });

  test('valid_minimal has no metrics', () {
    final p = Packet.parse(fixtureText('valid_minimal.json'));
    expect(p.metrics, isEmpty);
    expect(p.os, 'windows');
  });

  test('valid_null_gpu keeps null v', () {
    final p = Packet.parse(fixtureText('valid_null_gpu.json'));
    expect(p.metrics['gpu']!.v, isNull);
    expect(p.metrics['gpu']!.detail!['approx'], true);
    expect(p.metrics['disk']!.v, 0.0);
    expect(p.intervalMs, 5000);
  });

  test('valid_unknown_fields: ints as doubles, defaults, order', () {
    final p = Packet.parse(fixtureText('valid_unknown_fields.json'));
    expect(p.metrics.keys.toList(), ['cpu', 'fan_rpm', 'battery']);
    final fan = p.metrics['fan_rpm']!;
    expect(fan.v, 1450.0);
    expect(fan.v, isA<double>());
    expect(fan.max, 3000.0);
    final cpu = p.metrics['cpu']!;
    expect(cpu.min, 0);
    expect(cpu.max, isNull);
    expect(p.metrics['battery']!.isPercent, isTrue);
  });

  test('valid_with_auth parses auth', () {
    final p = Packet.parse(fixtureText('valid_with_auth.json'));
    expect(p.auth, hasLength(64));
  });

  group('structural errors', () {
    Map<String, dynamic> base() => fixtureJson('valid_minimal.json');

    for (final field in [
      'v',
      'id',
      'host',
      'os',
      'ts',
      'seq',
      'interval_ms',
      'metrics',
    ]) {
      test('missing $field', () {
        final j = base()..remove(field);
        expect(
          () => Packet.fromJson(j),
          throwsA(
            isA<FormatException>().having(
              (e) => e.message,
              'message',
              contains(field),
            ),
          ),
        );
      });
    }

    test('metric without label', () {
      final j = base()
        ..['metrics'] = {
          'cpu': {'v': 1, 'unit': '%'},
        };
      expect(
        () => Packet.fromJson(j),
        throwsA(
          isA<FormatException>().having(
            (e) => e.message,
            'm',
            contains('label'),
          ),
        ),
      );
    });

    test('metric without unit', () {
      final j = base()
        ..['metrics'] = {
          'cpu': {'v': 1, 'label': 'CPU'},
        };
      expect(() => Packet.fromJson(j), throwsFormatException);
    });

    test('metric v as string', () {
      final j = base()
        ..['metrics'] = {
          'cpu': {'v': 'x', 'unit': '%', 'label': 'CPU'},
        };
      expect(() => Packet.fromJson(j), throwsFormatException);
    });

    test('not JSON / not an object', () {
      expect(() => Packet.parse('{nope'), throwsFormatException);
      expect(() => Packet.parse('[1,2]'), throwsFormatException);
      expect(() => Packet.parse(''), throwsFormatException);
    });

    test('v = "1" string is a version error', () {
      final j = base()..['v'] = '1';
      expect(
        () => Packet.fromJson(j),
        throwsA(isA<UnsupportedVersionException>()),
      );
    });

    test('integral double ts tolerated', () {
      final j = base()..['ts'] = 5.0;
      expect(Packet.fromJson(j).ts, 5);
    });
  });

  group('auth', () {
    const secret = 'correct horse';

    test('canonical body: sorted, no whitespace, number tokens preserved', () {
      final j = fixtureJson('valid_with_auth.json')..remove('_comment');
      expect(
        Packet.canonicalBody(j),
        '{"host":"secure","id":"auth-box","interval_ms":1000,'
        '"metrics":{"cpu":{"label":"CPU","unit":"%","v":10.0}},'
        '"os":"windows","seq":1,"ts":1762640003000,"v":1}',
      );
    });

    test('HMAC matches an independent implementation (Python hmac)', () {
      final j = fixtureJson('valid_with_auth.json')..remove('_comment');
      expect(
        Packet.computeAuth(j, secret),
        'db863564617ffdf0aaecac5aed1d9747f255a9eb708975ea88a50e1af642c746',
      );
    });

    test('golden valid_with_auth.json verifies as shipped', () {
      // The fixture is signed over the whole object including the unknown
      // `_comment` key (PROTOCOL.md Auth). The Rust side verifies the same file.
      final j = fixtureJson('valid_with_auth.json');
      expect(Packet.fromJson(j).verifyAuth(secret), isTrue);
      expect(
        Packet.computeAuth(j, secret),
        '0a2fbe3180d92083d961ccb1dc43d3ee17b4dc007dbde1e1ca11b0665644dce2',
      );
      expect(Packet.fromJson(j).verifyAuth('wrong horse'), isFalse);
    });

    test('verifyAuth with a recomputed HMAC over a modified packet', () {
      final j = fixtureJson('valid_with_auth.json');
      j['host'] = 'renamed';
      expect(Packet.fromJson(j).verifyAuth(secret), isFalse);
      final mac = Packet.computeAuth(j, secret);
      expect(mac, matches(RegExp(r'^[0-9a-f]{64}$')));
      j['auth'] = mac;
      final p = Packet.parse(jsonEncode(j));
      expect(p.verifyAuth(secret), isTrue);
      expect(p.verifyAuth('wrong horse'), isFalse);
      expect(p.verifyAuth(''), isFalse);
    });

    test('auth covers unknown fields and values', () {
      final j = fixtureJson('valid_full.json');
      j['auth'] = Packet.computeAuth(j, secret);
      expect(Packet.fromJson(j).verifyAuth(secret), isTrue);
      final tampered = Map<String, dynamic>.of(j)..['host'] = 'evil';
      expect(Packet.fromJson(tampered).verifyAuth(secret), isFalse);
    });

    test('missing auth fails verification', () {
      expect(
        Packet.parse(fixtureText('valid_full.json')).verifyAuth(secret),
        isFalse,
      );
    });

    test('string escaping matches serde_json', () {
      expect(
        Packet.canonicalBody({'a': 'q"\\\n\u0001é/'}),
        '{"a":"q\\"\\\\\\n\\u0001é/"}',
      );
    });

    test('key order is by UTF-8 bytes, nested', () {
      expect(
        Packet.canonicalBody({
          'b': 1,
          'a': {
            'z': true,
            'B': null,
            'aa': [2.5, 3],
          },
        }),
        '{"a":{"B":null,"aa":[2.5,3],"z":true},"b":1}',
      );
    });
  });

  test('formatDoubleLikeRyu matches serde_json/ryu layout', () {
    const cases = <(double, String)>[
      (10.0, '10.0'),
      (0.0, '0.0'),
      (-0.0, '-0.0'),
      (23.4, '23.4'),
      (-1.5, '-1.5'),
      (0.1, '0.1'),
      (0.30000000000000004, '0.30000000000000004'),
      (0.0001, '0.0001'),
      (0.00001, '0.00001'),
      (0.000001, '1e-6'),
      (1.5e-7, '1.5e-7'),
      (1234567890123456.0, '1234567890123456.0'),
      (1e16, '1e16'),
      (1.5e17, '1.5e17'),
      (1e21, '1e21'),
      (1762640003000.0, '1762640003000.0'),
    ];
    for (final (d, s) in cases) {
      expect(formatDoubleLikeRyu(d), s, reason: '$d');
    }
  });
}
