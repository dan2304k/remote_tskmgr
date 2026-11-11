import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// test/fixtures must be byte-identical copies of ../protocol/examples, so the Dart
/// tests exercise the same golden packets as the sender's.
void main() {
  test('test/fixtures/*.json match ../protocol/examples/*.json', () {
    List<String> names(Directory d) =>
        d
            .listSync()
            .whereType<File>()
            .map((f) => f.uri.pathSegments.last)
            .where((n) => n.endsWith('.json'))
            .toList()
          ..sort();
    final fixtures = Directory('test/fixtures');
    final examples = Directory('../protocol/examples');
    expect(names(fixtures), names(examples));
    expect(names(fixtures), isNotEmpty);
    for (final n in names(examples)) {
      expect(
        File('${fixtures.path}/$n').readAsBytesSync(),
        File('${examples.path}/$n').readAsBytesSync(),
        reason: '$n differs from protocol/examples',
      );
    }
  });
}
